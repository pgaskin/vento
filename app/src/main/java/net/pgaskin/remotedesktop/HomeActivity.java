// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.content.Intent;
import android.content.res.ColorStateList;
import android.graphics.Bitmap;
import android.os.Bundle;
import android.view.LayoutInflater;
import android.view.MenuItem;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.TextView;

import androidx.annotation.NonNull;
import androidx.appcompat.app.AppCompatActivity;
import androidx.core.graphics.Insets;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.recyclerview.widget.GridLayoutManager;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.appbar.MaterialToolbar;
import com.google.android.material.color.MaterialColors;
import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.floatingactionbutton.FloatingActionButton;

import net.pgaskin.remotedesktop.backend.Backend;
import net.pgaskin.remotedesktop.backend.Backends;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/**
 * The home screen: one saved connection per item, and the two buttons that
 * lead everywhere else.
 *
 * <p>An item is a card or a row, chosen from the bar and remembered. Cards are
 * how somebody recognises a machine by what was on its screen; rows are how
 * somebody finds one among twenty, since a name and an address on a line of
 * their own are read rather than scanned past. Both are the same bind — the
 * two layouts share their ids — so the only thing the mode decides is how big
 * the picture is and whether the items are joined into a group of rows.
 */
public final class HomeActivity extends AppCompatActivity {

    private RecyclerView list;
    private View empty;
    private MaterialToolbar toolbar;
    private FloatingActionButton add;
    private final Adapter adapter = new Adapter();

    /** Rows rather than cards. Persisted, because it is a preference and not a mode. */
    private boolean listView;

    /**
     * The bar's own height, before the status bar is added to it — the theme's
     * {@code actionBarSize}, taken from the layout rather than named again
     * here, since every other header on the screen is that same attribute and
     * Material 3's value is not appcompat's.
     */
    private int barHeight;

    /** The editor sheet, while one is open — a dialog does not survive a rotation. */
    private ConnectionEditorPanel editor;

    /**
     * The reorder sheet, likewise. Nothing of it has to be carried across a
     * rotation, unlike the editor's half-typed form: what it shows is read from
     * the saved connections and what it does is written as it is done, so the
     * bundle needs only to say that it was open.
     */
    private PinnedOrderPanel reorder;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        setContentView(R.layout.activity_home);
        toolbar = findViewById(R.id.toolbar);
        list = findViewById(R.id.list);
        empty = findViewById(R.id.empty);
        add = findViewById(R.id.add);
        add.setOnClickListener(v -> addConnection());
        empty.findViewById(R.id.playground).setOnClickListener(
                v -> startActivity(new Intent(this, PlaygroundActivity.class)));

        listView = AppSettings.listView(this);
        barHeight = toolbar.getLayoutParams().height;
        final GridLayoutManager grid = new GridLayoutManager(this, spanCount());
        // A plugin's card is a statement about the screen rather than an item
        // on it, so it takes the whole width whatever shape the items are and
        // however many columns they are in.
        grid.setSpanSizeLookup(new GridLayoutManager.SpanSizeLookup() {
            @Override
            public int getSpanSize(int position) {
                return position < adapter.cards.size() ? grid.getSpanCount() : 1;
            }
        });
        list.setLayoutManager(grid);
        // Every change here is a full reload, and the one change somebody sees
        // is the switch between the two shapes — where cards morphing into rows
        // is a distraction in a gesture whose whole content is "show me more of
        // them".
        list.setItemAnimator(null);
        list.setAdapter(adapter);
        liftToolbarOnScroll();
        applyInsets();
        setUpToolbar();

        // An editor open across a rotation: the sheet went with the window, and
        // what somebody had typed into it is in the bundle.
        final Bundle sheet = savedInstanceState == null
                ? null : savedInstanceState.getBundle("editor");
        if (sheet != null) {
            editor = ConnectionEditorPanel.restore(this, sheet, adapter::reload);
        }
        if (savedInstanceState != null && savedInstanceState.getBoolean("reorder")) {
            reorderPinned();
        }
    }

    @Override
    protected void onSaveInstanceState(Bundle out) {
        super.onSaveInstanceState(out);
        if (editor != null && editor.isShowing()) {
            out.putBundle("editor", editor.saveState());
        }
        out.putBoolean("reorder", reorder != null && reorder.isShowing());
    }

    @Override
    protected void onDestroy() {
        // A dialog outliving its activity is a leaked window, and these are
        // shown from a screen that is recreated on every rotation.
        if (editor != null) {
            editor.dismiss();
            editor = null;
        }
        if (reorder != null) {
            reorder.dismiss();
            reorder = null;
        }
        super.onDestroy();
    }

    @Override
    protected void onResume() {
        super.onResume();
        adapter.reload();
    }

    /**
     * The connected badges, while this screen is up. A session can come up or
     * go while somebody is looking at the list — in the window beside this one,
     * or from the notification — and the card would otherwise go on claiming a
     * connection that ended.
     *
     * <p>Posted rather than applied where it arrives: this can land during a
     * scroll, and a list is not allowed to be told its contents changed while
     * it is working out where they go.
     */
    private final Sessions.Watcher sessionWatcher =
            () -> list.post(adapter::notifyDataSetChanged);

    @Override
    protected void onStart() {
        super.onStart();
        Sessions.addWatcher(sessionWatcher);
    }

    @Override
    protected void onStop() {
        Sessions.removeWatcher(sessionWatcher);
        super.onStop();
    }

    /**
     * As many columns of about 240 dp as fit, and never fewer than two: a
     * roughly square card is a shape you scan rather than read, and one of them
     * per row of a phone screen would be a very large button.
     *
     * <p>240 rather than the 190 this started at, because rounding put the
     * boundary in the wrong place: at 190 a third column appears from 475 dp,
     * which several ordinary phones are, and three cards across a phone are too
     * small to be the picture they exist to be. A third column now wants 600 dp
     * — a tablet or an unfolded foldable — and the cards simply grow until then.
     *
     * <p>A row is read left to right, so it wants twice the width and can be
     * the only thing across a phone: 480 dp puts the second column on a tablet
     * and nowhere else. A single 900 dp row with a name at one end and a badge
     * at the other is not a list, it is a card that lost its picture.
     */
    private int spanCount() {
        final float dp = getResources().getDisplayMetrics().widthPixels
                / getResources().getDisplayMetrics().density;
        return listView ? Math.max(1, Math.round(dp / 480f))
                : Math.max(2, Math.round(dp / 240f));
    }

    /**
     * The inset either side of the list. Zero for rows, whose own group inset
     * is the whole of it — a card carries its margin in its layout, and a row
     * takes the same 16 dp every other group of rows in this app is given.
     */
    private int listSidePadding() {
        return listView ? 0 : Math.round(8 * getResources().getDisplayMetrics().density);
    }

    /** The same 8 dp under the bar, from whichever of the two supplies it. */
    private int listTopPadding() {
        return listView ? Math.round(8 * getResources().getDisplayMetrics().density) : 0;
    }

    /**
     * The list scrolls behind the bar, so it is the padding that keeps the first
     * card clear of it rather than the layout.
     */
    private void applyInsets() {
        ViewCompat.setOnApplyWindowInsetsListener(findViewById(R.id.root), (v, insets) -> {
            final Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars());
            // The bar's own end padding, because a menu item is not content: a
            // toolbar's content inset moves the title and does nothing to the
            // buttons, which are laid out from its padding. 8 dp puts the last
            // icon where the title starts, its cell being 12 dp wider than the
            // glyph in it either side.
            final int menuInset = Math.round(8 * getResources().getDisplayMetrics().density);
            toolbar.setPadding(bars.left, bars.top, bars.right + menuInset, 0);
            toolbar.getLayoutParams().height = bars.top + barHeight;
            // The FAB sits over the list, so the padding is its height and both
            // margins as well as the bar — otherwise the last row is a card
            // with a button on top of it.
            final int fab = Math.round(72 * getResources().getDisplayMetrics().density);
            list.setPadding(listSidePadding(),
                    toolbar.getLayoutParams().height + listTopPadding(),
                    listSidePadding(), bars.bottom + fab);
            final ViewGroup.MarginLayoutParams lp =
                    (ViewGroup.MarginLayoutParams) add.getLayoutParams();
            final int margin = Math.round(16 * getResources().getDisplayMetrics().density);
            lp.bottomMargin = bars.bottom + margin;
            lp.rightMargin = bars.right + margin;
            lp.leftMargin = bars.left + margin;
            add.setLayoutParams(lp);
            return insets;
        });
    }

    /**
     * The bar takes its shadow the moment anything is behind it. Its background
     * is the window's, so at rest there is nothing to separate — and once a card
     * has scrolled under it, a shadow is the only thing that says one is there.
     */
    private void liftToolbarOnScroll() {
        final float lifted = getResources().getDisplayMetrics().density * 4f;
        list.addOnScrollListener(new RecyclerView.OnScrollListener() {
            @Override
            public void onScrolled(@NonNull RecyclerView v, int dx, int dy) {
                toolbar.setElevation(v.canScrollVertically(-1) ? lifted : 0f);
            }
        });
    }

    // ---- the toolbar --------------------------------------------------------

    private void setUpToolbar() {
        toolbar.setNavigationIcon(null);
        toolbar.setTitle(R.string.app_name);
        toolbar.setBackgroundColor(MaterialColors.getColor(toolbar,
                com.google.android.material.R.attr.colorSurfaceContainer));
        toolbar.getMenu().clear();
        toolbar.inflateMenu(R.menu.home);
        showViewMode();
        toolbar.setOnMenuItemClickListener(item -> {
            final int id = item.getItemId();
            if (id == R.id.action_view_mode) {
                toggleViewMode();
                return true;
            }
            if (id == R.id.action_reorder) {
                reorderPinned();
                return true;
            }
            if (id == R.id.action_settings) {
                startActivity(new Intent(this, SettingsActivity.class));
                return true;
            }
            return false;
        });
    }

    /** The button offers the other shape, which is also what it is called. */
    private void showViewMode() {
        final MenuItem item = toolbar.getMenu().findItem(R.id.action_view_mode);
        item.setIcon(listView ? R.drawable.ic_view_grid : R.drawable.ic_view_list);
        item.setTitle(listView ? R.string.home_view_grid : R.string.home_view_list);
    }

    /**
     * Reordering is offered once there are two pins to be in the wrong order.
     * One pin has no order, and none is a sheet that would open empty — and an
     * item that is always there and does nothing on most phones is worse than
     * one that appears when it means something.
     *
     * <p>Counted off the front of the list rather than over all of it, because
     * that is what {@link Connections#all} sorting pinned first means.
     */
    private void showReorder() {
        int pinned = 0;
        for (Connection c : adapter.items) {
            if (!c.pinned()) {
                break;
            }
            pinned++;
        }
        toolbar.getMenu().findItem(R.id.action_reorder).setVisible(pinned > 1);
    }

    /**
     * The switch, which is a rebind of everything: the item type changes and so
     * does the span count.
     *
     * <p>Which is why the first visible item is taken first and scrolled back
     * to afterwards. A full rebind starts the list at the top, and a screen
     * that jumps to the top has lost the connection somebody was looking at —
     * the one thing they were doing when they asked for the other shape.
     */
    private void toggleViewMode() {
        final GridLayoutManager grid = (GridLayoutManager) list.getLayoutManager();
        final int firstVisible = grid.findFirstVisibleItemPosition();
        listView = !listView;
        AppSettings.setListView(this, listView);
        showViewMode();
        grid.setSpanCount(spanCount());
        list.setPadding(listSidePadding(),
                toolbar.getLayoutParams().height + listTopPadding(),
                listSidePadding(), list.getPaddingBottom());
        adapter.notifyDataSetChanged();
        if (firstVisible != RecyclerView.NO_POSITION) {
            grid.scrollToPosition(firstVisible);
        }
    }

    // ---- actions ------------------------------------------------------------

    private void addConnection() {
        if (Backends.ids().isEmpty()) {
            new MaterialAlertDialogBuilder(this, R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setMessage(R.string.session_no_backend)
                    .setPositiveButton(android.R.string.ok, null)
                    .show();
            return;
        }
        editConnection(null);
    }

    /**
     * The editor is a sheet over this screen rather than a screen of its own,
     * so the list behind it is what it is editing — and a save has to be reflected there rather than waiting for
     * {@link #onResume}, which no longer happens.
     */
    private void editConnection(String id) {
        editor = ConnectionEditorPanel.show(this, id, adapter::reload);
    }

    /**
     * The pinned connections, in a sheet that does nothing but let them be
     * dragged past one another — see {@link PinnedOrderPanel} for why the drag
     * is not on the cards themselves. Reloading behind it as it writes, since
     * the list under the sheet is the thing being rearranged.
     */
    private void reorderPinned() {
        reorder = PinnedOrderPanel.show(this, adapter::reload);
    }

    /**
     * A session gets a window of its own, and this one goes.
     *
     * <p>The chooser is a way in rather than a place: leaving it behind the
     * session would mean the launcher icon returns to whichever window is
     * already open, and choosing a second machine would need a way back out of
     * the first. Finishing it means the icon always offers the list — and since
     * a session's window is a document task, the list opened from it can start
     * a second session beside the first rather than instead of it.
     *
     * <p>The task goes with the activity, which is not the same thing: a
     * finished activity leaves its task in the recents list, so the chooser
     * lingered there as a window somebody could go back to — an empty one, since
     * the only thing in it had gone.
     *
     * <p>The intent itself is {@link SessionActivity#intentFor}'s, and shared
     * rather than built here because a launcher shortcut opens the same
     * connection and the two must be the same document — see there.
     */
    private void openConnection(Connection conn) {
        startActivity(SessionActivity.intentFor(this, conn.id()));
        finishAndRemoveTask();
    }

    // ---- the list -----------------------------------------------------------

    private final class Adapter extends RecyclerView.Adapter<RecyclerView.ViewHolder> {

        private final List<Connection> items = new ArrayList<>();
        /**
         * The plugins with something to say, above the connections. Recomputed
         * with the list, since every one of them can change while the app is in
         * the background and uninstalling from a card is how one usually does.
         */
        private final List<Plugins.Card> cards = new ArrayList<>();
        /**
         * Decoded previews, by connection id. A preview is a sealed PNG, so
         * binding a card costs a decryption and a decode, and a grid rebinds
         * every time it is scrolled. Emptied here, which is the only place a
         * preview can have changed: a session writes one on its way out and
         * this screen reloads when it comes back.
         */
        private final Map<String, Bitmap> previews = new HashMap<>();

        void reload() {
            items.clear();
            previews.clear();
            items.addAll(Connections.all(HomeActivity.this));
            // About the connections alone: a screen saying there are none is
            // still true with a card above it, and a plugin waiting to be set
            // up is a good deal of what somebody would do first.
            empty.setVisibility(items.isEmpty() ? View.VISIBLE : View.GONE);
            // Whether there is an order to change is a fact about the list, so
            // it is answered wherever the list is read.
            showReorder();
            notifyDataSetChanged();
            // The plugins land when they land: asking one whether it is set up
            // can mean hashing a library, and the connections are not made to
            // wait behind that. Where the answers are known it is this frame.
            Plugins.cards(HomeActivity.this, found -> {
                cards.clear();
                cards.addAll(found);
                notifyDataSetChanged();
            });
        }

        /**
         * The shape, which is a property of the screen rather than of an item —
         * but it is the view type all the same, because that is what makes a
         * recycled card impossible to hand to a row.
         */
        @Override
        public int getItemViewType(int position) {
            if (position < cards.size()) {
                return 2;
            }
            return listView ? 1 : 0;
        }

        @NonNull
        @Override
        public RecyclerView.ViewHolder onCreateViewHolder(@NonNull ViewGroup parent, int viewType) {
            final LayoutInflater inflater = LayoutInflater.from(parent.getContext());
            if (viewType == 2) {
                return new PluginHolder(inflater.inflate(R.layout.item_plugin, parent, false));
            }
            return new Holder(inflater.inflate(
                    viewType == 1 ? R.layout.item_connection_row : R.layout.item_connection,
                    parent, false));
        }

        @Override
        public void onBindViewHolder(@NonNull RecyclerView.ViewHolder holder, int position) {
            if (holder instanceof PluginHolder p) {
                bindCard(p, position, cards.get(position));
                return;
            }
            final Holder h = (Holder) holder;
            final int index = position - cards.size();
            final Connection c = items.get(index);
            h.title.setText(c.title());
            h.subtitle.setText(c.subtitle());
            // INVISIBLE, not GONE: a connection with no name has one line to
            // show and the one beside it may have two, and a grid row of cards
            // that are not the same height is a ragged edge for nothing.
            h.subtitle.setVisibility(c.subtitle().isEmpty() ? View.INVISIBLE : View.VISIBLE);

            // Hidden when there is only one backend to be on: a label that is
            // the same on every card says nothing, and the free flavour with
            // one backend is that case.
            final String backend = Backends.shortName(c.backendId());
            h.badge.setText(backend);
            h.badge.setVisibility(Backends.ids().size() > 1 && !backend.isEmpty()
                    ? View.VISIBLE : View.GONE);

            final Bitmap thumb = previews.computeIfAbsent(c.id(),
                    id -> Connections.thumbnail(HomeActivity.this, id));
            if (thumb != null) {
                h.preview.setImageBitmap(thumb);
                h.preview.setImageTintList(null);
                h.preview.setScaleType(ImageView.ScaleType.CENTER_CROP);
            } else {
                // Nothing has been connected to yet, so: the band's own colour
                // and one faint icon at its natural size in the middle of it.
                // CENTER, not the CENTER_CROP a photograph wants — cropping a
                // 24 dp glyph to fill 132 dp of card is what the first version
                // of this did, and it draws two grey bars nobody can read as a
                // computer.
                h.preview.setImageResource(R.drawable.ic_desktop);
                h.preview.setImageTintList(ColorStateList.valueOf(
                        MaterialColors.compositeARGBWithAlpha(
                                MaterialColors.getColor(h.preview,
                                        com.google.android.material.R.attr.colorOnSurface),
                                (int) (0.20f * 255))));
                h.preview.setScaleType(ImageView.ScaleType.CENTER);
            }

            // The one thing on a card that is about right now rather than about
            // what was saved. Tinted here rather than in the drawable so it is
            // the theme's accent on whatever the device's colours turn out to
            // be, and so the other two badges keep sharing one shape.
            final Session live = Sessions.byKey(Sessions.keyFor(c.id()));
            if (live != null && live.alive()) {
                h.connected.setText(live.state() == Backend.State.CONNECTED
                        ? R.string.home_connected : R.string.session_connecting);
                h.connected.setBackgroundTintList(ColorStateList.valueOf(
                        MaterialColors.getColor(h.connected,
                                androidx.appcompat.R.attr.colorPrimary)));
                h.connected.setTextColor(MaterialColors.getColor(h.connected,
                        com.google.android.material.R.attr.colorOnPrimary));
                h.connected.setVisibility(View.VISIBLE);
            } else {
                h.connected.setVisibility(View.GONE);
            }

            h.pin.setVisibility(c.pinned() ? View.VISIBLE : View.GONE);

            if (listView) {
                groupRow(h.card, index);
            }

            h.card.setOnClickListener(v -> openConnection(c));
            // Straight to the editor. There used to be a selection mode here,
            // whose three actions were edit, delete and pin — and all three are
            // in the editor, two of them only reachable that way. A mode whose
            // whole content is one screen is a step in front of that screen.
            h.card.setOnLongClickListener(v -> {
                editConnection(c.id());
                return true;
            });
        }

        /**
         * One plugin's card: what it is, what is wrong or missing, what its own
         * failure said, and the one or two things to do about it. Uninstall is
         * on every card about a package; the restart card is about the set and
         * has the other button instead.
         */
        private void bindCard(PluginHolder h, int position, Plugins.Card card) {
            spaceCard(h.itemView, position);
            h.title.setText(card.title());
            h.message.setText(card.message());
            h.detail.setText(card.detail());
            h.detail.setVisibility(card.detail() == null || card.detail().isEmpty()
                    ? View.GONE : View.VISIBLE);
            h.setUp.setVisibility(card.kind() == Plugins.Kind.SETUP ? View.VISIBLE : View.GONE);
            h.restart.setVisibility(card.kind() == Plugins.Kind.RESTART
                    ? View.VISIBLE : View.GONE);
            h.uninstall.setVisibility(card.packageName() == null ? View.GONE : View.VISIBLE);
            h.setUp.setOnClickListener(v -> Plugins.setup(HomeActivity.this, card.backendId()));
            h.restart.setOnClickListener(v -> Plugins.restart(HomeActivity.this));
            h.uninstall.setOnClickListener(
                    v -> Plugins.uninstall(HomeActivity.this, card.packageName()));
        }

        @Override
        public int getItemCount() {
            return cards.size() + items.size();
        }

        /**
         * A card's edges, which the two shapes below it disagree about.
         *
         * <p>It is the full width of either shape, so it has to line up with
         * both: a row is inset 16 dp with the list flush to the screen, and a
         * card carries 8 dp of its own with the list padded by 8. The inset is
         * therefore the group's less whatever the list already contributes, and
         * comes to the same 16 dp either way.
         *
         * <p>The gap is the one between two groups of rows, for the same reason
         * that is the gap: a card is a statement about the screen, not an entry
         * on it. Above the first, below every one, and less what the neighbour
         * already brings — the list's own top padding, and a connection card's
         * own margin where the shape below is cards.
         */
        private void spaceCard(View card, int position) {
            final int inset = getResources().getDimensionPixelSize(R.dimen.group_inset);
            final int spacing = getResources().getDimensionPixelSize(R.dimen.group_spacing);
            final int own = getResources().getDimensionPixelSize(R.dimen.card_margin);
            final ViewGroup.MarginLayoutParams lp =
                    (ViewGroup.MarginLayoutParams) card.getLayoutParams();
            lp.setMarginStart(inset - listSidePadding());
            lp.setMarginEnd(inset - listSidePadding());
            lp.topMargin = position == 0 ? spacing - listTopPadding() : 0;
            lp.bottomMargin = position == cards.size() - 1 && !listView
                    ? spacing - own : spacing;
            card.setLayoutParams(lp);
        }

        /**
         * A row's place in the group the whole list is: its background, its
         * inset and the gap under it, the same three a settings row is given.
         *
         * <p>Ends are grid rows rather than items, since a wide enough screen
         * puts two of them side by side — so the first span's worth are the top
         * of the group and whatever shares the bottom grid row is its end.
         */
        private void groupRow(View row, int position) {
            final int span = ((GridLayoutManager) list.getLayoutManager()).getSpanCount();
            final int last = items.size() - 1;
            final boolean bottom = position >= last - last % span;
            RowGroups.row(row, position < span, bottom, bottom ? 0 : RowGroups.gap(row));
        }
    }

    /**
     * One item, in either shape: the two layouts carry the same ids so that
     * one bind fills both, and the live badge — the part most likely to drift —
     * is written once.
     */
    private static final class Holder extends RecyclerView.ViewHolder {
        final View card;
        final ImageView preview;
        final ImageView pin;
        final TextView badge;
        final TextView connected;
        final TextView title;
        final TextView subtitle;

        Holder(View v) {
            super(v);
            card = v.findViewById(R.id.card);
            preview = v.findViewById(R.id.preview);
            pin = v.findViewById(R.id.pin);
            badge = v.findViewById(R.id.badge);
            connected = v.findViewById(R.id.connected);
            title = v.findViewById(R.id.title);
            subtitle = v.findViewById(R.id.subtitle);
        }
    }

    /** A plugin's card, which shares nothing with a connection's but its list. */
    private static final class PluginHolder extends RecyclerView.ViewHolder {
        final TextView title;
        final TextView message;
        final TextView detail;
        final View setUp;
        final View restart;
        final View uninstall;

        PluginHolder(View v) {
            super(v);
            title = v.findViewById(R.id.title);
            message = v.findViewById(R.id.message);
            detail = v.findViewById(R.id.detail);
            setUp = v.findViewById(R.id.setup);
            restart = v.findViewById(R.id.restart);
            uninstall = v.findViewById(R.id.uninstall);
        }
    }
}
