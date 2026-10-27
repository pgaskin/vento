// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.hardware.biometrics.BiometricManager;
import android.hardware.biometrics.BiometricPrompt;
import android.net.Uri;
import android.os.Bundle;
import android.os.CancellationSignal;
import android.text.TextUtils;
import android.util.Log;
import android.view.LayoutInflater;
import android.view.View;
import android.widget.CheckBox;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.preference.ListPreference;
import androidx.preference.Preference;
import androidx.preference.PreferenceCategory;
import androidx.preference.PreferenceFragmentCompat;
import androidx.preference.PreferenceGroup;
import androidx.preference.PreferenceGroupAdapter;
import androidx.preference.PreferenceScreen;
import androidx.preference.PreferenceViewHolder;
import androidx.preference.SwitchPreferenceCompat;
import androidx.recyclerview.widget.RecyclerView;

import com.google.android.material.dialog.MaterialAlertDialogBuilder;
import com.google.android.material.snackbar.Snackbar;

import net.pgaskin.remotedesktop.backend.BackendOption;
import net.pgaskin.remotedesktop.backend.Backends;
import net.pgaskin.remotedesktop.control.playground.Recordings;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.time.LocalDate;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.function.Supplier;

/**
 * The settings tree: one activity, a section per screen, and every screen built
 * in code from setting descriptions ({@link OptionScreen}).
 *
 * <p>The sections keep separate preference files so that a backend's settings
 * are removed with the backend, and the input tuning can be exported or reset
 * on its own. {@link AppSettings}, {@link InputSettings} and
 * {@link Options#backendPrefs} are those three files.
 */
public final class SettingsActivity extends AppCompatToolbarActivity {

    public static final String EXTRA_SECTION = "section"; // absent means the root
    public static final String EXTRA_BACKEND = "backend"; // for SECTION_BACKEND: which one

    public static final String SECTION_GENERAL = "general";
    public static final String SECTION_INPUT = "input";
    public static final String SECTION_BACKEND = "backend";
    public static final String SECTION_TRANSFER = "transfer";

    /** Also the source row's summary in strings.xml, without the scheme. */
    private static final String SOURCE_URL = "https://github.com/pgaskin/vento";

    /** Which section this is, resolved once before the fragment reads it. */
    private String section;

    /**
     * The extra if there is one, and otherwise the started component's
     * meta-data: the system's "Manage space" button can only name a component,
     * so the alias it names carries the section as meta-data (see the
     * manifest). Null for the root, which is how every other caller opens this.
     */
    private String resolveSection() {
        final String extra = getIntent().getStringExtra(EXTRA_SECTION);
        if (extra != null) {
            return extra;
        }
        try {
            final Bundle meta = getPackageManager().getActivityInfo(getComponentName(),
                    PackageManager.GET_META_DATA).metaData;
            return meta == null ? null : meta.getString(EXTRA_SECTION);
        } catch (PackageManager.NameNotFoundException e) {
            return null; // we are installed
        }
    }

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        section = resolveSection();
        setTitle(switch (section == null ? "" : section) {
            case SECTION_GENERAL -> getString(R.string.settings_general);
            case SECTION_INPUT -> getString(R.string.settings_input);
            case SECTION_TRANSFER -> getString(R.string.settings_transfer);
            case SECTION_BACKEND -> Backends.name(getIntent().getStringExtra(EXTRA_BACKEND));
            default -> getString(R.string.settings_title);
        });
        show(new SettingsFragment());
    }

    private void open(String section) {
        startActivity(new Intent(this, SettingsActivity.class)
                .putExtra(EXTRA_SECTION, section));
    }

    private void openBackend(String id) {
        startActivity(new Intent(this, SettingsActivity.class)
                .putExtra(EXTRA_SECTION, SECTION_BACKEND)
                .putExtra(EXTRA_BACKEND, id));
    }

    public static final class SettingsFragment extends PreferenceFragmentCompat {

        /**
         * Each section configures where its values are stored <em>before</em>
         * building the screen: a preference persists through whatever store the
         * manager had when the preference was attached, so the order matters.
         */
        @Override
        public void onCreatePreferences(Bundle savedInstanceState, String rootKey) {
            final SettingsActivity a = (SettingsActivity) requireActivity();
            switch (a.section == null ? "" : a.section) {
                case SECTION_GENERAL -> general();
                case SECTION_INPUT -> input();
                case SECTION_TRANSFER -> transfer();
                case SECTION_BACKEND -> backend(a.getIntent().getStringExtra(EXTRA_BACKEND));
                default -> root(a);
            }
        }

        private PreferenceScreen newScreen() {
            final PreferenceScreen screen = getPreferenceManager()
                    .createPreferenceScreen(requireContext());
            setPreferenceScreen(screen);
            return screen;
        }

        /**
         * The rows as groups: contiguous rows on a container with large corners,
         * a group ending wherever a category does.
         *
         * <p>{@code androidx.preference} has one background per row and no
         * concept of a group of rows, so a row's position in its group is worked
         * out here from its neighbours, and {@link RowGroups} turns that into a
         * background and the gaps around it.
         */
        // PreferenceGroupAdapter is androidx-internal, and subclassing it is the
        // only way to see a row's neighbours: the adapter knows the flattened
        // order, and onCreateAdapter is where a screen can supply its own.
        @SuppressLint("RestrictedApi")
        @Override
        protected RecyclerView.Adapter<PreferenceViewHolder> onCreateAdapter(
                PreferenceScreen screen) {
            return new PreferenceGroupAdapter(screen) {
                @Override
                public void onBindViewHolder(PreferenceViewHolder holder, int position) {
                    super.onBindViewHolder(holder, position);
                    final View row = holder.itemView;
                    if (getItem(position) instanceof PreferenceCategory) {
                        RowGroups.plain(row);
                        return;
                    }
                    RowGroups.row(row,
                            position == 0 || heading(position - 1),
                            heading(position + 1),
                            gapAfter(position, row));
                }

                /** A heading or the end of the screen, either of which ends a group. */
                private boolean heading(int position) {
                    return position < 0 || position >= getItemCount()
                            || getItem(position) instanceof PreferenceCategory;
                }

                /**
                 * No gap under the last row of a group, because a heading has
                 * its own space, unless the next category has no title, which is
                 * how a screen asks for a break with no heading. Then this gap is
                 * the only thing separating the two groups.
                 */
                private int gapAfter(int position, View row) {
                    if (position + 1 >= getItemCount()) {
                        return 0;
                    }
                    final Preference next = getItem(position + 1);
                    if (!(next instanceof PreferenceCategory)) {
                        return RowGroups.gap(row);
                    }
                    return TextUtils.isEmpty(next.getTitle()) ? RowGroups.spacing(row) : 0;
                }
            };
        }

        @Override
        public void onViewCreated(View view, Bundle savedInstanceState) {
            super.onViewCreated(view, savedInstanceState);
            // The group's container shows that rows belong together, and a
            // divider would cut through it.
            setDivider(null);
            setDividerHeight(0);
            final RecyclerView list = getListView();
            list.setClipToPadding(false);
            list.setPadding(0, list.getPaddingTop(),
                    0, getResources().getDimensionPixelSize(R.dimen.group_spacing));
        }

        // ---- the root -------------------------------------------------------

        private void root(SettingsActivity a) {
            final PreferenceScreen screen = newScreen();
            screen.addPreference(heading(R.string.settings_group_sessions));
            screen.addPreference(link(R.string.settings_general, R.string.settings_general_summary,
                    () -> a.open(SECTION_GENERAL)));
            screen.addPreference(link(R.string.settings_input, R.string.settings_input_summary,
                    () -> a.open(SECTION_INPUT)));
            screen.addPreference(heading(R.string.settings_group_backends));
            for (String id : Backends.ids()) {
                // A backend that still needs setup opens its setup instead of
                // its settings screen, since it cannot connect yet.
                final Preference p = link(0, 0, () -> Backends.isSetup(a, id, ready -> {
                    if (ready) {
                        a.openBackend(id);
                    } else {
                        Plugins.setup(a, id);
                    }
                }));
                p.setTitle(Backends.name(id));
                backendRows.put(id, p);
                screen.addPreference(p);
            }
            screen.addPreference(heading(R.string.settings_group_app));
            screen.addPreference(link(R.string.settings_transfer,
                    R.string.settings_transfer_summary, () -> a.open(SECTION_TRANSFER)));
            // A test surface for the input options against known geometry,
            // reachable from where those options are set.
            screen.addPreference(link(R.string.settings_playground,
                    R.string.settings_playground_summary,
                    () -> startActivity(new Intent(requireContext(), PlaygroundActivity.class))));
            screen.addPreference(link(R.string.settings_licenses,
                    R.string.settings_licenses_summary,
                    () -> startActivity(new Intent(requireContext(), LicensesActivity.class))));
            screen.addPreference(link(R.string.settings_source,
                    R.string.settings_source_summary, this::openSource));
            screen.addPreference(version());
        }

        /**
         * The root screen's backend rows, by id, and empty on every other
         * screen. Returning from an add-on's setup activity is the only sign
         * that setup worked, so each row's summary is updated on every resume.
         */
        private final Map<String, Preference> backendRows = new LinkedHashMap<>();

        @Override
        public void onResume() {
            super.onResume();
            versionTaps = 0; // taps across two visits do not count as one run
            final String notSetUp = getString(R.string.settings_backend_setup);
            for (Map.Entry<String, Preference> row : backendRows.entrySet()) {
                final Preference p = row.getValue();
                // `isAdded` in addition to Backends.isSetup's own check on the
                // activity: the activity can outlive this fragment, and the
                // rows of a replaced fragment are no longer shown.
                Backends.isSetup(requireContext(), row.getKey(), ready -> {
                    if (isAdded()) {
                        p.setSummary(ready ? null : notSetUp);
                    }
                });
            }
        }

        /** A heading over the group that follows, or with no title, a break. */
        private PreferenceCategory heading(int title) {
            final PreferenceCategory c = new PreferenceCategory(requireContext());
            if (title != 0) {
                c.setTitle(title);
            }
            c.setIconSpaceReserved(false);
            return c;
        }

        /** Caught rather than checked first: resolving an https intent needs a
         *  package-visibility declaration, so the app cannot ask in advance
         *  whether anything will open it. */
        private void openSource() {
            try {
                startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(SOURCE_URL)));
            } catch (android.content.ActivityNotFoundException e) {
                Log.w(TAG, "no browser for " + SOURCE_URL, e);
            }
        }

        /**
         * Which build this is, under the licences and the source link. An
         * informational row that is still selectable, because tapping it is
         * also how {@link #tapVersion the developer rows} are unlocked.
         */
        private Preference version() {
            final Preference p = new Preference(requireContext());
            p.setTitle(R.string.settings_version);
            p.setSummary(BuildConfig.VERSION_NAME);
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            p.setPersistent(false);
            p.setOnPreferenceClickListener(x -> {
                tapVersion();
                return true;
            });
            return p;
        }

        /** The tap count, reset by leaving the screen. */
        private int versionTaps;

        /** The last toast shown, cancelled so toasts do not queue up. */
        private Toast versionToast;

        /**
         * Unlocks developer mode the same way as Android's own developer
         * options: ten taps on the version, silent for the first six so it is
         * not found by accident, then counting down so the user can see
         * progress.
         *
         * <p>Toasts rather than this screen's snackbars: the message is about the
         * tapping rather than the row, each tap replaces the previous one, and a
         * snackbar countdown would push the list up and down repeatedly.
         */
        private void tapVersion() {
            final Context ctx = requireContext();
            if (versionToast != null) {
                versionToast.cancel();
            }
            if (AppSettings.developerMode(ctx)) {
                versionToast = Toast.makeText(ctx, R.string.settings_developer_already,
                        Toast.LENGTH_SHORT);
            } else {
                final int remaining = 10 - ++versionTaps;
                if (remaining > 3) {
                    return;
                }
                if (remaining > 0) {
                    versionToast = Toast.makeText(ctx, getResources().getQuantityString(
                            R.plurals.settings_developer_steps, remaining, remaining),
                            Toast.LENGTH_SHORT);
                } else {
                    AppSettings.setDeveloperMode(ctx, true);
                    versionToast = Toast.makeText(ctx, R.string.settings_developer_on,
                            Toast.LENGTH_SHORT);
                }
            }
            versionToast.show();
        }

        private Preference link(int title, int summary, Runnable go) {
            final Preference p = new Preference(requireContext());
            if (title != 0) {
                p.setTitle(title);
            }
            if (summary != 0) {
                p.setSummary(summary);
            }
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            p.setPersistent(false);
            p.setOnPreferenceClickListener(x -> {
                go.run();
                return true;
            });
            return p;
        }

        // ---- the sections ---------------------------------------------------

        private void general() {
            final OptionScreen.Store store =
                    OptionScreen.switches(AppSettings.prefs(requireContext()));
            getPreferenceManager().setPreferenceDataStore(store);
            final PreferenceScreen screen = newScreen();
            screen.addPreference(sessionTimeout());
            screen.addPreference(switchPref(AppSettings.KEY_HUD, R.string.settings_hud,
                    R.string.settings_hud_summary, false));
            screen.addPreference(switchPref(AppSettings.KEY_KEEP_AWAKE,
                    R.string.settings_keep_awake, R.string.settings_keep_awake_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_IMMERSIVE,
                    R.string.settings_immersive, R.string.settings_immersive_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_PRIVATE_IME,
                    R.string.settings_private_ime, R.string.settings_private_ime_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_MODIFIER_RESETS_IME,
                    R.string.settings_modifier_resets_ime,
                    R.string.settings_modifier_resets_ime_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_TWO_LINE_KEYS,
                    R.string.settings_two_line_keys,
                    R.string.settings_two_line_keys_summary, false));
            screen.addPreference(switchPref(AppSettings.KEY_MAC_KEYS,
                    R.string.settings_mac_keys,
                    R.string.settings_mac_keys_summary, true));
            // The two rows about the session's controls, next to each other:
            // which controls it has, and whether the first frame explains them
            // (and offers the same choice again).
            screen.addPreference(controls());
            screen.addPreference(switchPref(AppSettings.KEY_REGION_HINTS,
                    R.string.settings_region_hints,
                    R.string.settings_region_hints_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_CLIPBOARD_OUT,
                    R.string.settings_clipboard_out,
                    R.string.settings_clipboard_out_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_CLIPBOARD_IN,
                    R.string.settings_clipboard_in,
                    R.string.settings_clipboard_in_summary, true));
            screen.addPreference(switchPref(AppSettings.KEY_RELEASE_KEYS,
                    R.string.settings_release_keys,
                    R.string.settings_release_keys_summary, true));

            // Turning previews off deletes the existing ones too, not only
            // future ones: the previews already on disk are the reason to turn
            // it off.
            final SwitchPreferenceCompat previews = switchPref(AppSettings.KEY_PREVIEWS,
                    R.string.settings_previews, R.string.settings_previews_summary, true);
            previews.setOnPreferenceChangeListener((p, v) -> {
                if (!Boolean.TRUE.equals(v)) {
                    Connections.clearThumbnails(requireContext());
                }
                return true;
            });
            screen.addPreference(previews);

            // The developer rows, in their own group since they are not about
            // how a session behaves.
            if (AppSettings.developerMode(requireContext())) {
                screen.addPreference(heading(0));
                screen.addPreference(switchPref(AppSettings.KEY_EXTERNAL_LAUNCH,
                        R.string.settings_external_launch,
                        R.string.settings_external_launch_summary, false));
                screen.addPreference(switchPref(AppSettings.KEY_FORCE_SOCKETPAIR,
                        R.string.settings_force_socketpair,
                        R.string.settings_force_socketpair_summary, false));
            }
            store.built();
        }

        /** Which controls a session offers: toolbar, tap regions, or both. */
        private ListPreference controls() {
            final ListPreference p = new ListPreference(requireContext());
            p.setKey(AppSettings.KEY_CONTROLS);
            p.setTitle(R.string.settings_controls);
            p.setDialogTitle(R.string.settings_controls);
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            p.setEntryValues(new CharSequence[]{AppSettings.CONTROLS_TOOLBAR,
                    AppSettings.CONTROLS_REGIONS, AppSettings.CONTROLS_BOTH});
            p.setEntries(new CharSequence[]{getString(R.string.controls_toolbar),
                    getString(R.string.controls_regions), getString(R.string.controls_both)});
            p.setDefaultValue(AppSettings.CONTROLS_REGIONS);
            p.setSummaryProvider(x -> controlsLabel()
                    + "\n" + getString(R.string.settings_controls_summary));
            return p;
        }

        private String controlsLabel() {
            return switch (AppSettings.controls(requireContext())) {
                case AppSettings.CONTROLS_REGIONS -> getString(R.string.controls_regions);
                case AppSettings.CONTROLS_BOTH -> getString(R.string.controls_both);
                default -> getString(R.string.controls_toolbar);
            };
        }

        /**
         * The session timeout. Its values are minutes, as {@link AppSettings}
         * parses them, and its labels are the same ones a session that timed
         * out shows.
         */
        private ListPreference sessionTimeout() {
            final ListPreference p = new ListPreference(requireContext());
            p.setKey(AppSettings.KEY_SESSION_TIMEOUT);
            p.setTitle(R.string.settings_session_timeout);
            p.setDialogTitle(R.string.settings_session_timeout);
            p.setIconSpaceReserved(false);
            p.setSingleLineTitle(false);
            p.setEntryValues(AppSettings.SESSION_TIMEOUTS.stream()
                    .map(String::valueOf).toArray(CharSequence[]::new));
            p.setEntries(AppSettings.SESSION_TIMEOUTS.stream()
                    .map(m -> AppSettings.timeoutLabel(requireContext(), m))
                    .toArray(CharSequence[]::new));
            p.setDefaultValue("0");
            // The label rather than the row's own entry, which is null for a
            // value not in the list: an imported file may contain any number
            // of minutes.
            p.setSummaryProvider(x -> AppSettings.timeoutLabel(requireContext(),
                    AppSettings.sessionTimeout(requireContext()))
                    + "\n" + getString(R.string.settings_session_timeout_summary));
            // Every live session's deadline is computed from when it left the
            // screen, so a change here only re-arms the timers. The listener
            // runs before the value is stored, so the re-arm is posted rather
            // than run inside it.
            p.setOnPreferenceChangeListener((x, v) -> {
                requireView().post(Sessions::timeoutChanged);
                return true;
            });
            return p;
        }

        private SwitchPreferenceCompat switchPref(String key, int title, int summary,
                                                  boolean def) {
            final SwitchPreferenceCompat s = new SwitchPreferenceCompat(requireContext());
            s.setKey(key);
            s.setTitle(title);
            s.setSummary(summary);
            s.setDefaultValue(def);
            s.setIconSpaceReserved(false);
            s.setSingleLineTitle(false);
            return s;
        }

        /** The input stack: its tunables, and a way back to their defaults. */
        private void input() {
            final OptionScreen.Store store =
                    OptionScreen.store(InputSettings.prefs(requireContext()));
            getPreferenceManager().setPreferenceDataStore(store);
            final PreferenceScreen screen = newScreen();

            OptionScreen.addTunables(screen, InputSettings.tunables(),
                    InputSettings.defaults(getResources().getDisplayMetrics().density));

            // In a separate group, so the action does not look like one of the
            // values it resets.
            screen.addPreference(heading(0));
            final Preference reset = new Preference(requireContext());
            reset.setTitle(R.string.settings_reset);
            reset.setSummary(R.string.settings_reset_summary);
            reset.setIconSpaceReserved(false);
            reset.setPersistent(false);
            reset.setOnPreferenceClickListener(p -> {
                InputSettings.clearOverrides(requireContext());
                rebuild();
                return true;
            });
            screen.addPreference(reset);
            store.built();
        }

        private void backend(String id) {
            final OptionScreen.Store store =
                    OptionScreen.store(Options.backendPrefs(requireContext(), id));
            getPreferenceManager().setPreferenceDataStore(store);
            final PreferenceScreen screen = newScreen();
            OptionScreen.addOptions(screen, Backends.options(id), BackendOption.Scope.GLOBAL);
            // Under their own heading, because these rows differ from the ones
            // above: they are not the backend's settings but the defaults for
            // connections that do not override them. With nothing above them
            // the heading is left out, since a screen with one heading and
            // nothing to contrast it with is clearer without it.
            PreferenceGroup into = screen;
            if (screen.getPreferenceCount() > 0) {
                final PreferenceCategory defaults = new PreferenceCategory(requireContext());
                defaults.setTitle(R.string.settings_connection_defaults);
                defaults.setIconSpaceReserved(false);
                screen.addPreference(defaults);
                into = defaults;
            }
            OptionScreen.addOptions(into, Backends.options(id), BackendOption.Scope.LAYERED);

            // A backend from an add-on says so, and the row opens the system's
            // page for that package, where it can be uninstalled, its storage
            // cleared and its version seen. In its own group because it is
            // information about the backend, not a setting.
            final String plugin = Backends.packageOf(id);
            if (plugin != null) {
                screen.addPreference(heading(0));
                // An add-on containing third-party code generates its own
                // licence page, since it is a separate build under its own terms
                // and the app's page does not cover it. It is shown from here,
                // above the row that leads out of the app.
                if (LicensesActivity.has(requireContext(), plugin)) {
                    screen.addPreference(link(R.string.settings_licenses, 0,
                            () -> startActivity(LicensesActivity.of(requireContext(), plugin))));
                }
                final Preference p = link(R.string.settings_plugin, 0, () -> startActivity(
                        new Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                Uri.fromParts("package", plugin, null))));
                p.setSummary(plugin);
                screen.addPreference(p);
            }
            store.built();
        }

        // ---- import and export ----------------------------------------------

        private void transfer() {
            final PreferenceScreen screen = newScreen();
            screen.addPreference(link(R.string.transfer_export_connections,
                    R.string.transfer_export_connections_summary, this::exportConnections));
            screen.addPreference(link(R.string.transfer_export_settings,
                    R.string.transfer_export_settings_summary,
                    () -> exportSettings.launch(fileName(Transfer.KIND_SETTINGS))));
            screen.addPreference(link(R.string.transfer_import,
                    R.string.transfer_import_summary,
                    // Any type, because a file from email or a cloud drive
                    // arrives with whatever type that service assigned, and the
                    // only reliable check is of its contents.
                    () -> importFile.launch(new String[]{"*/*"})));
            // Next to import and export because they act on everything stored
            // at once, and in their own group because they delete data. Each
            // row deletes one store, named on the row, and no row deletes two.
            screen.addPreference(heading(0));
            screen.addPreference(link(R.string.settings_reset_all,
                    R.string.settings_reset_all_summary, this::confirmResetAll));
            screen.addPreference(link(R.string.settings_delete_connections,
                    R.string.settings_delete_connections_summary,
                    this::confirmDeleteConnections));
            screen.addPreference(link(R.string.settings_forget_hosts,
                    R.string.settings_forget_hosts_summary, this::confirmForgetHosts));
            // Only when a plugin is installed that could have provided a
            // library, or a copy is left from one that has been removed.
            if (!Backends.installedPlugins(requireContext()).isEmpty()
                    || Plugins.hasLibraries(requireContext())) {
                screen.addPreference(link(R.string.settings_clear_libraries,
                        R.string.settings_clear_libraries_summary, this::confirmClearLibraries));
            }
            // Likewise only when relevant. Recordings are only written in
            // developer mode, and after it is turned off ("Restore defaults"
            // clears the flag) the row stays while recordings remain on disk, so
            // there is always a way to delete them.
            if (AppSettings.developerMode(requireContext())
                    || Recordings.any(requireContext())) {
                screen.addPreference(link(R.string.settings_delete_recordings,
                        R.string.settings_delete_recordings_summary,
                        this::confirmDeleteRecordings));
            }
        }

        /**
         * Both directions use the storage access framework: no permission is
         * needed, and the user picks the file's location.
         *
         * <p>Registered as fields, since a launcher has to exist before the
         * fragment is started, and one per document, so the result callback
         * does not have to remember which button was pressed across a process
         * that may have died while the picker was open.
         */
        private final ActivityResultLauncher<String> exportConnections =
                registerForActivityResult(
                        new ActivityResultContracts.CreateDocument("application/json"),
                        uri -> write(uri, Transfer.KIND_CONNECTIONS));

        private final ActivityResultLauncher<String> exportSettings = registerForActivityResult(
                new ActivityResultContracts.CreateDocument("application/json"),
                uri -> write(uri, Transfer.KIND_SETTINGS));

        private final ActivityResultLauncher<String[]> importFile = registerForActivityResult(
                new ActivityResultContracts.OpenDocument(), this::read);

        /**
         * Whether the export awaiting a destination includes the passwords,
         * which the launcher above cannot carry. False if this screen has been
         * rebuilt since: a process that died while the picker was open writes
         * the file without them, which is the harmless failure.
         */
        private boolean withPasswords;

        /**
         * Asks about the passwords before the destination: if included, every
         * saved password is written in plain text, so the lock screen comes
         * first; if not, the file has no secrets and nothing needs a lock.
         */
        private void exportConnections() {
            final View view = LayoutInflater.from(requireContext())
                    .inflate(R.layout.dialog_confirm, null);
            final CheckBox tick = view.findViewById(R.id.tick);
            ((TextView) view.findViewById(R.id.message))
                    .setText(R.string.transfer_export_connections_message);
            tick.setText(R.string.transfer_export_passwords);
            tick.setChecked(true);
            new MaterialAlertDialogBuilder(requireContext(),
                    R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setTitle(R.string.transfer_export_connections)
                    .setView(view)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.transfer_export, (d, w) -> {
                        withPasswords = tick.isChecked();
                        final String name = fileName(Transfer.KIND_CONNECTIONS);
                        if (withPasswords) {
                            credential(() -> exportConnections.launch(name));
                        } else {
                            exportConnections.launch(name);
                        }
                    })
                    .show();
        }

        /**
         * The lock screen, before writing a file with every saved password in
         * the clear: {@code BiometricPrompt} with a device credential allowed,
         * a platform class, so it adds no dependency.
         *
         * <p>Before the picker rather than after it, so a cancelled prompt leaves
         * no empty document at the destination. A phone with no credential set
         * goes ahead, since there is no lock screen to show.
         */
        private void credential(Runnable then) {
            final Context ctx = requireContext();
            final int allowed = BiometricManager.Authenticators.DEVICE_CREDENTIAL
                    | BiometricManager.Authenticators.BIOMETRIC_WEAK;
            final BiometricManager manager = ctx.getSystemService(BiometricManager.class);
            if (manager == null || manager.canAuthenticate(allowed)
                    != BiometricManager.BIOMETRIC_SUCCESS) {
                then.run();
                return;
            }
            new BiometricPrompt.Builder(ctx)
                    .setTitle(getString(R.string.transfer_unlock_title))
                    .setDescription(getString(R.string.transfer_unlock_message))
                    // No negative button: the builder rejects one together with
                    // a device credential, since the credential screen has its
                    // own cancel.
                    .setAllowedAuthenticators(allowed)
                    .build()
                    .authenticate(new CancellationSignal(), ctx.getMainExecutor(),
                            new BiometricPrompt.AuthenticationCallback() {
                                @Override
                                public void onAuthenticationSucceeded(
                                        BiometricPrompt.AuthenticationResult result) {
                                    if (isAdded()) {
                                        then.run();
                                    }
                                }
                            });
        }

        private void write(Uri uri, String kind) {
            if (uri == null) {
                return; // the picker was cancelled
            }
            final Context ctx = requireContext().getApplicationContext();
            final boolean passwords = withPasswords;
            withPasswords = false;
            io(() -> {
                try {
                    final Transfer.Document doc = Transfer.write(ctx, kind, passwords);
                    // "wt", not "w": "w" does not truncate an existing document,
                    // so a shorter export written over a longer one would keep
                    // the old tail.
                    try (OutputStream out = ctx.getContentResolver().openOutputStream(uri, "wt")) {
                        if (out == null) {
                            throw new IOException("nothing to write to at " + uri);
                        }
                        out.write(doc.text().getBytes(StandardCharsets.UTF_8));
                    }
                    return () -> saidCount(Transfer.KIND_CONNECTIONS.equals(kind)
                                    ? R.plurals.transfer_exported_connections
                                    : R.plurals.transfer_exported_settings,
                            doc.count(), doc.count());
                } catch (Exception e) {
                    Log.w(TAG, "writing " + uri, e);
                    return () -> problem(R.string.transfer_write_failed);
                }
            });
        }

        private void read(Uri uri) {
            if (uri == null) {
                return;
            }
            final Context ctx = requireContext().getApplicationContext();
            io(() -> {
                try (InputStream in = ctx.getContentResolver().openInputStream(uri)) {
                    if (in == null) {
                        throw new IOException("nothing to read at " + uri);
                    }
                    // One byte more than this app ever writes, so a video
                    // picked by mistake is rejected rather than decoded.
                    final byte[] bytes = in.readNBytes(Transfer.MAX_BYTES + 1);
                    if (bytes.length > Transfer.MAX_BYTES) {
                        throw new IOException("larger than any file this app writes");
                    }
                    final Transfer.Document doc =
                            Transfer.open(new String(bytes, StandardCharsets.UTF_8));
                    return () -> confirmImport(doc);
                } catch (Exception e) {
                    Log.w(TAG, "reading " + uri, e);
                    return () -> problem(R.string.transfer_bad_file);
                }
            });
        }

        /**
         * Shows what the file contains before anything is applied, with the
         * one destructive option, the same for either kind: delete the
         * existing data first, so the result matches the file.
         */
        private void confirmImport(Transfer.Document doc) {
            final boolean connections = Transfer.KIND_CONNECTIONS.equals(doc.kind());
            final View view = LayoutInflater.from(requireContext())
                    .inflate(R.layout.dialog_confirm, null);
            final CheckBox tick = view.findViewById(R.id.tick);
            ((TextView) view.findViewById(R.id.message)).setText(
                    getResources().getQuantityString(connections
                                    ? R.plurals.transfer_import_connections_message
                                    : R.plurals.transfer_import_settings_message,
                            doc.count(), doc.count()));
            tick.setText(connections
                    ? R.string.transfer_import_replace
                    : R.string.transfer_import_reset);
            new MaterialAlertDialogBuilder(requireContext(),
                    R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setTitle(connections
                            ? R.string.transfer_import_connections_title
                            : R.string.transfer_import_settings_title)
                    .setView(view)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(R.string.transfer_import_apply,
                            (d, w) -> apply(doc, tick.isChecked()))
                    .show();
        }

        private void apply(Transfer.Document doc, boolean replaceExisting) {
            final Context ctx = requireContext().getApplicationContext();
            final boolean connections = Transfer.KIND_CONNECTIONS.equals(doc.kind());
            io(() -> {
                final Transfer.Result r = Transfer.apply(ctx, doc, replaceExisting);
                return () -> {
                    saidCount(connections
                                    ? R.plurals.transfer_imported_connections
                                    : R.plurals.transfer_imported_settings,
                            r.total(), r.applied(), r.total());
                    if (!connections) {
                        // The screens behind this one are built from the values
                        // that just changed, and so is every running session's
                        // deadline.
                        Sessions.timeoutChanged();
                        rebuild();
                    }
                };
            });
        }

        // ---- reset, and the other four destructive actions --------------

        /**
         * The dialog all five share, using the row's own summary as the
         * question: it states exactly what the row deletes, and a second
         * wording could come to disagree with it.
         */
        private void confirm(int title, int message, int action, Runnable go) {
            new MaterialAlertDialogBuilder(requireContext(),
                    R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setTitle(title)
                    .setMessage(message)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(action, (d, w) -> go.run())
                    .show();
        }

        private void confirmResetAll() {
            confirm(R.string.settings_reset_all_confirm, R.string.settings_reset_all_summary,
                    R.string.settings_reset_all_do, () -> {
                        final Context ctx = requireContext();
                        AppSettings.prefs(ctx).edit().clear().apply();
                        InputSettings.prefs(ctx).edit().clear().apply();
                        for (String id : Backends.ids()) {
                            Options.backendPrefs(ctx, id).edit().clear().apply();
                        }
                        Sessions.timeoutChanged();
                        said(R.string.settings_reset_all_done);
                        rebuild();
                    });
        }

        /**
         * The records and their previews. A session that is already open is not
         * affected: it is a live connection rather than a saved record, and is
         * ended from its own screen.
         */
        private void confirmDeleteConnections() {
            confirm(R.string.settings_delete_connections_confirm,
                    R.string.settings_delete_connections_summary,
                    R.string.settings_delete, () -> {
                        final int deleted = Connections.deleteAll(requireContext());
                        saidCount(R.plurals.settings_delete_connections_done, deleted, deleted);
                    });
        }

        private void confirmForgetHosts() {
            confirm(R.string.settings_forget_hosts_confirm,
                    R.string.settings_forget_hosts_summary,
                    R.string.settings_forget, () -> {
                        Backends.forgetHosts(requireContext());
                        said(R.string.settings_forget_hosts_done);
                    });
        }

        /**
         * The app's copies of libraries provided by add-ons. Nothing is lost
         * that the add-on cannot provide again, which is why this is worded
         * less strongly than the rows above it.
         */
        private void confirmClearLibraries() {
            confirm(R.string.settings_clear_libraries_confirm,
                    R.string.settings_clear_libraries_summary,
                    R.string.settings_clear, () -> {
                        Plugins.clearLibraries(requireContext());
                        said(R.string.settings_clear_libraries_done);
                        // The row is only shown while there is something to
                        // delete, which is now only the case if a plugin is
                        // installed.
                        rebuild();
                    });
        }

        private void confirmDeleteRecordings() {
            confirm(R.string.settings_delete_recordings_confirm,
                    R.string.settings_delete_recordings_summary,
                    R.string.settings_delete, () -> {
                        final int deleted = Recordings.clear(requireContext());
                        saidCount(R.plurals.settings_delete_recordings_done, deleted, deleted);
                        // As with the libraries: with developer mode off, the
                        // row is no longer needed.
                        rebuild();
                    });
        }

        // ---- plumbing -----------------------------------------------------

        private static final String TAG = "Settings";

        /**
         * A document provider is another process, and encrypting or decrypting
         * a password is a keystore round trip, so both directions run off the
         * main thread. The work returns the result report, which runs back on
         * the main thread and only while the screen is still there.
         */
        private static final ExecutorService IO = Executors.newSingleThreadExecutor(r -> {
            final Thread t = new Thread(r, "transfer");
            t.setDaemon(true);
            return t;
        });

        private void io(Supplier<Runnable> work) {
            final View view = requireView();
            IO.execute(() -> {
                final Runnable report = work.get();
                view.post(() -> {
                    if (isAdded()) {
                        report.run();
                    }
                });
            });
        }

        /** Reports the result of an action on the screen it was started from. */
        private void said(int text, Object... args) {
            Snackbar.make(requireView(), getString(text, args), Snackbar.LENGTH_LONG).show();
        }

        /** The same, with a plural chosen by {@code count}. */
        private void saidCount(int text, int count, Object... args) {
            Snackbar.make(requireView(), getResources().getQuantityString(text, count, args),
                    Snackbar.LENGTH_LONG).show();
        }

        /** A dialog rather than a toast, so it stays until it is read. */
        private void problem(int message) {
            new MaterialAlertDialogBuilder(requireContext(),
                    R.style.ThemeOverlay_RemoteDesktop_Dialog)
                    .setTitle(R.string.transfer_failed_title)
                    .setMessage(message)
                    .setPositiveButton(android.R.string.ok, null)
                    .show();
        }

        /** Dated, so two exports can be told apart and compared. */
        private String fileName(String kind) {
            return kind + "-" + LocalDate.now() + ".json";
        }

        /**
         * Rebuilds the screen from the stored values. Rows default to computed
         * values (such as the input stack's own), so clearing an override has to
         * rebuild the rows rather than refresh them.
         */
        private void rebuild() {
            requireActivity().getSupportFragmentManager().beginTransaction()
                    .replace(R.id.content, new SettingsFragment())
                    .commit();
        }
    }
}
