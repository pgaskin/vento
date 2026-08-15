// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.IBinder;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * What keeps a session alive while nothing is looking at it.
 *
 * <p>The reason is measured: <b>about five seconds in the background costs the
 * connection</b>, because Android freezes a cached
 * process and takes its sockets with it. A foreground service is the only way
 * out of that, and the notification is not decoration — it is the price, and the
 * bargain is a fair one: something holding a live connection to somebody's
 * desktop ought to say so, and be one tap from being stopped.
 *
 * <p>It owns nothing. {@link Sessions} holds the sessions; this is the thing
 * that makes the process matter to the system, plus a notification per session
 * saying what is connected and offering to end it. Ending one here and ending it
 * from its screen are the same call.
 *
 * <p><b>One service, several notifications.</b> A foreground service has exactly
 * one notification the system counts as its own, so one live session is elected
 * to it and the rest are posted beside it — which is invisible from the shade
 * and matters in one place only: when the elected session ends while others are
 * still running, the election has to be re-run <em>before</em> its notification
 * is cancelled, or the service is left foreground with nothing to show for it.
 *
 * <p><b>Swiping a session's window away ends that session.</b> A session
 * outlives its screen so that a phone call or a locked screen does not cost a
 * connection — not so that a window somebody threw away keeps one. The
 * notification is what makes the service hard to kill, what says which
 * connections are up, and what ends one that is wedged; it is not a second
 * lifetime. Which session went is read out of the removed task's own root
 * intent, since a session has a window each and the system says which one.
 */
public final class SessionService extends Service implements Sessions.Watcher {

    private static final String CHANNEL = "session";
    private static final String GROUP = "net.pgaskin.remotedesktop.SESSIONS";

    /** Posted only to keep the {@code startForeground} promise on the way out. */
    private static final int NOTIFICATION_NONE = 1;

    /** The notification's button, and the only action this service takes. */
    private static final String ACTION_DISCONNECT = "net.pgaskin.remotedesktop.DISCONNECT";
    private static final String EXTRA_SESSION = "session";

    /**
     * Called by {@link Session#open}, and harmless when the service is already
     * running. {@code startForegroundService} obliges us to call
     * {@code startForeground} within a few seconds, which {@link #onStartCommand}
     * does unconditionally — including when the session has already gone, where
     * it starts and stops again rather than leaving the promise unkept.
     */
    static void start(Context ctx) {
        ctx.startForegroundService(new Intent(ctx, SessionService.class));
    }

    @Override
    public IBinder onBind(Intent intent) {
        // Nothing binds: a screen finds its session through Sessions, which is a
        // map in this process rather than a connection to set up. A service that
        // only needs to exist does not need an interface.
        return null;
    }

    @Override
    public void onCreate() {
        super.onCreate();
        Sessions.addWatcher(this);
    }

    @Override
    public void onDestroy() {
        Sessions.removeWatcher(this);
        super.onDestroy();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && ACTION_DISCONNECT.equals(intent.getAction())) {
            final Session s = Sessions.byKey(intent.getStringExtra(EXTRA_SESSION));
            if (s != null) {
                // Which has already updated us, through the set: this is an
                // ordinary start with no promise attached, so there is nothing
                // to say afterwards.
                s.disconnect();
            }
            update();
            return START_NOT_STICKY;
        }
        // A start from Session.open, and the only kind that owes a
        // startForeground — including when the session it was started for died
        // in between, where the goodbye has to be said again for this call.
        stopped = false;
        update();
        // Not sticky: a restarted service would have no session to be foreground
        // for, and reconnecting behind somebody's back is not this app's job.
        return START_NOT_STICKY;
    }

    /**
     * The window is gone, so the connection is: {@link Session#disconnect} says
     * the far end was told on purpose, which is what stops the next screen for
     * this connection explaining that something went wrong.
     */
    @Override
    public void onTaskRemoved(Intent rootIntent) {
        final Session s = sessionOf(rootIntent);
        if (s != null) {
            s.disconnect();
        }
        update();
    }

    /**
     * Whose window that was. A session task is launched with the connection's
     * own URI, or — from the command line, which has no record — with the
     * address it was told to open; both are the key {@code SessionActivity}
     * would compute for the same window.
     */
    private static Session sessionOf(Intent rootIntent) {
        if (rootIntent == null) {
            return null;
        }
        final android.net.Uri data = rootIntent.getData();
        if (data != null && "connection".equals(data.getScheme())) {
            return Sessions.byKey(Sessions.keyFor(data.getSchemeSpecificPart()));
        }
        final String address = rootIntent.getStringExtra("address");
        return address == null ? null : Sessions.byKey(Sessions.keyForAddress(address));
    }

    @Override
    public void sessionsChanged() {
        update();
    }

    /** What is posted, so that a session's notification goes when it does. */
    private final Set<Integer> posted = new LinkedHashSet<>();

    /** Which of them the service is foreground for, or 0. */
    private int elected;

    /** Whether the goodbye below has already been said. */
    private boolean stopped;

    /**
     * The whole of the service's behaviour: whatever is alive gets a
     * notification, whatever is not loses one, and a service with nothing alive
     * left stops.
     *
     * <p>Called for every change to the set, and a session ending is two of
     * them — the state, and then the letting go — which is why the goodbye is
     * guarded. Said twice, the second one would post a notification for a
     * session that is already gone, under an id nothing is going to cancel
     * again.
     */
    private void update() {
        final List<Session> alive = Sessions.alive();
        final NotificationManager nm = getSystemService(NotificationManager.class);

        if (alive.isEmpty()) {
            if (stopped) {
                return;
            }
            stopped = true;
            // The promise stands even here, so it is kept and then let go of in
            // the same breath. Under the outgoing session's own id where there
            // is one, so this posts nothing new for the moment it lasts.
            startForeground(elected != 0 ? elected : NOTIFICATION_NONE, build(null),
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
            for (int id : posted) {
                nm.cancel(id);
            }
            posted.clear();
            stopSelf();
            return;
        }
        stopped = false;

        // Re-elected only when the incumbent has gone, so that the foreground
        // notification does not jump between sessions for no reason.
        Session foreground = null;
        for (Session s : alive) {
            if (s.notificationId() == elected) {
                foreground = s;
            }
        }
        if (foreground == null) {
            foreground = alive.get(0);
        }
        startForeground(foreground.notificationId(), build(foreground),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        elected = foreground.notificationId();

        final Set<Integer> live = new LinkedHashSet<>();
        for (Session s : alive) {
            live.add(s.notificationId());
            if (s != foreground) {
                nm.notify(s.notificationId(), build(s));
            }
        }
        // After the election, never before it: cancelling the outgoing
        // foreground notification first would take the service's own with it.
        for (int id : new ArrayList<>(posted)) {
            if (!live.contains(id)) {
                nm.cancel(id);
            }
        }
        posted.clear();
        posted.addAll(live);
    }

    private Notification build(Session s) {
        final NotificationManager nm = getSystemService(NotificationManager.class);
        if (nm.getNotificationChannel(CHANNEL) == null) {
            final NotificationChannel c = new NotificationChannel(CHANNEL,
                    getString(R.string.channel_session), NotificationManager.IMPORTANCE_LOW);
            c.setDescription(getString(R.string.channel_session_description));
            c.setShowBadge(false);
            nm.createNotificationChannel(c);
        }

        final Notification.Builder b = new Notification.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_desktop)
                .setContentTitle(s != null ? s.title() : getString(R.string.app_name))
                .setContentText(s != null ? s.status() : "")
                .setOngoing(true)
                .setShowWhen(false)
                // Nobody wants a connection announced with a sound every time it
                // reconnects, and IMPORTANCE_LOW already says so; this is for the
                // updates, which are frequent while connecting.
                .setOnlyAlertOnce(true)
                // No summary goes with it: the shade bundles them once there are
                // enough to be worth bundling, and a summary the system may let
                // somebody dismiss is not a thing to hang a foreground service on.
                .setGroup(GROUP)
                .setCategory(Notification.CATEGORY_SERVICE);

        // The request code, on both of these, is what keeps several sessions'
        // notifications apart: extras are not part of a PendingIntent's identity,
        // so one code for all of them would leave every button pointing at
        // whichever session was posted last.
        final int code = s != null ? s.notificationId() : 0;
        if (s != null && s.reopenIntent() != null) {
            // The intent the session was started from, which for a session
            // opened from the home screen carries the document identity of its
            // own window — so this lands in that window rather than making
            // another one.
            final Intent open = new Intent(s.reopenIntent())
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_SINGLE_TOP);
            b.setContentIntent(PendingIntent.getActivity(this, code, open,
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE));
        }
        final Intent stop = new Intent(this, SessionService.class)
                .setAction(ACTION_DISCONNECT)
                .putExtra(EXTRA_SESSION, s != null ? s.key() : null);
        b.addAction(new Notification.Action.Builder(null,
                getString(R.string.session_disconnect),
                PendingIntent.getService(this, code, stop,
                        PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE))
                .build());
        return b.build();
    }
}
