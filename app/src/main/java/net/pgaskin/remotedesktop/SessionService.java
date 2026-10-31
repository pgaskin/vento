// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop;

import android.annotation.SuppressLint;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.IBinder;

import androidx.core.app.ServiceCompat;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Keeps sessions alive while no screen is showing them.
 *
 * <p>Measured: <b>about five seconds in the background drops the
 * connection</b>, because Android freezes a cached process along with its
 * sockets. A foreground service is the only way around that, and it requires a
 * notification, which is appropriate anyway: an app holding a live connection
 * to a remote desktop should show it, and be one tap from stopping it.
 *
 * <p>It owns nothing. {@link Sessions} holds the sessions; this keeps the
 * process in the foreground, and posts a notification per session showing what
 * is connected with a button to disconnect. Disconnecting here and from the
 * session screen is the same call.
 *
 * <p><b>One service, several notifications.</b> A foreground service has exactly
 * one notification of its own, so one live session is chosen for it and the
 * rest are posted alongside. This is not visible in the shade and matters in
 * one place: when the chosen session ends while others are still running, a
 * new one has to be chosen <em>before</em> the old notification is cancelled,
 * or the service is left in the foreground with no notification.
 *
 * <p><b>Swiping a session's window away ends that session.</b> A session
 * outlives its screen so that a phone call or a locked screen does not drop
 * the connection, not so that a dismissed window keeps one. The notification
 * keeps the service hard to kill, shows which connections are up, and ends a
 * stuck one; it does not extend a session's lifetime. The session is found from
 * the removed task's root intent, since each session has its own window.
 */
public final class SessionService extends Service implements Sessions.Watcher {

    private static final String CHANNEL = "session";
    private static final String GROUP = "net.pgaskin.remotedesktop.SESSIONS";

    /**
     * Posted only to satisfy the {@code startForeground} requirement while
     * stopping. Negative so that it never collides with a session's id, which
     * {@link Session#notificationId} counts up from 1.
     */
    private static final int NOTIFICATION_NONE = -1;

    /** The notification's button, and the only action this service handles. */
    private static final String ACTION_DISCONNECT = "net.pgaskin.remotedesktop.DISCONNECT";
    private static final String EXTRA_SESSION = "session";

    /**
     * Called by {@link Session#open}, and harmless when the service is already
     * running. {@code startForegroundService} requires a call to
     * {@code startForeground} within a few seconds, which {@link #onStartCommand}
     * always makes, even when the session has already ended, in which case it
     * starts and immediately stops.
     */
    static void start(Context ctx) {
        ctx.startForegroundService(new Intent(ctx, SessionService.class));
    }

    @Override
    public IBinder onBind(Intent intent) {
        // Not bindable: a screen finds its session through Sessions, a map in
        // this process.
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
                // This has already called update() through Sessions. A start
                // with this action does not require startForeground.
                s.disconnect();
            }
            update();
            return START_NOT_STICKY;
        }
        // A start from Session.open, the only kind that requires
        // startForeground, even if its session ended in the meantime, in which
        // case the stop in update() has to run again for this call.
        stopped = false;
        update();
        // Not sticky: a restarted service would have no session, and this app
        // does not reconnect on its own.
        return START_NOT_STICKY;
    }

    /**
     * The window was removed, so the connection ends. {@link Session#disconnect}
     * marks it as intentional, so the next screen for this connection does not
     * report an error.
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
     * The session of a removed task. A session task is launched with the
     * connection's URI, or (from the command line, with no saved connection)
     * with the address extra; both give the key {@code SessionActivity} computes
     * for the same window.
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

    /** Posted notification ids, so a session's notification is cancelled when it ends. */
    private final Set<Integer> posted = new LinkedHashSet<>();

    /** The id of the service's own foreground notification, or 0. */
    private int elected;

    /** Whether {@link #update} has already stopped the service. */
    private boolean stopped;

    /**
     * Every alive session gets a notification, every other one loses its
     * notification, and the service stops when none are alive.
     *
     * <p>Called for every change to the set, and a session ending is two changes
     * (the CLOSED state, then the removal), which is why the stop is guarded.
     * Running it twice would post a notification for a session that has already
     * ended, under an id that is never cancelled.
     */
    @SuppressLint("InlinedApi") // ServiceCompat drops the type before API 34
    private void update() {
        final List<Session> alive = Sessions.alive();
        final NotificationManager nm = getSystemService(NotificationManager.class);

        if (alive.isEmpty()) {
            if (stopped) {
                return;
            }
            stopped = true;
            // startForeground is still required here, so it is called and the
            // service stopped right after. It reuses the outgoing session's id
            // if there is one, so no new notification appears.
            ServiceCompat.startForeground(this, elected != 0 ? elected : NOTIFICATION_NONE,
                    build(null), ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
            for (int id : posted) {
                nm.cancel(id);
            }
            posted.clear();
            stopSelf();
            return;
        }
        stopped = false;

        // A new session is chosen only when the current one has ended, so the
        // foreground notification does not move between sessions.
        Session foreground = null;
        for (Session s : alive) {
            if (s.notificationId() == elected) {
                foreground = s;
            }
        }
        if (foreground == null) {
            foreground = alive.get(0);
        }
        ServiceCompat.startForeground(this, foreground.notificationId(), build(foreground),
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        elected = foreground.notificationId();

        final Set<Integer> live = new LinkedHashSet<>();
        for (Session s : alive) {
            live.add(s.notificationId());
            if (s != foreground) {
                nm.notify(s.notificationId(), build(s));
            }
        }
        // After startForeground, never before: cancelling the old foreground
        // notification first would leave the service without one.
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
                // The session status, or the address when there is no status
                // (while connected).
                .setContentText(s == null ? ""
                        : s.status().text().isEmpty() ? s.subtitle() : s.status().text())
                .setOngoing(true)
                .setShowWhen(false)
                // IMPORTANCE_LOW already makes it silent; this is for the
                // updates, which are frequent while connecting.
                .setOnlyAlertOnce(true)
                // No summary notification: the shade bundles the group once
                // there are enough of them, and a summary may be dismissable,
                // which would not suit a foreground service.
                .setGroup(GROUP)
                .setCategory(Notification.CATEGORY_SERVICE);

        // The request code keeps several sessions' pending intents apart:
        // extras are not part of a PendingIntent's identity, so with one code
        // every button would target whichever session was posted last.
        final int code = s != null ? s.notificationId() : 0;
        if (s != null && s.reopenIntent() != null) {
            // The intent the session was started from, which for a session
            // opened from the home screen carries its window's document
            // identity, so this reopens that window rather than creating one.
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
