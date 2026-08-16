// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend;

import android.app.Activity;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;

import java.io.File;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.ServiceLoader;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.function.Consumer;

import dalvik.system.PathClassLoader;

/**
 * Which backends this build has.
 *
 * <p>Nobody writes the list: it is every {@link BackendProvider} on the
 * classpath, which is every protocol module the build depends on, plus every
 * one found in an installed add-on ({@link #discover}). That is what makes
 * "what can this app connect with?" a question about a dependency line and an
 * installed package rather than about a class somebody has to remember to edit.
 *
 * <p>An id this build does not have is not an error anywhere except
 * {@link #create}: a connection record naming one still has to draw on the home
 * screen and open in the editor, since the record outlives the build that made
 * it.
 */
public final class Backends {

    private static final String TAG = "Backends";

    /**
     * What an add-on's manifest declares, on a component nothing ever starts,
     * and what the app's {@code <queries>} matches. An action rather than a
     * package name so that neither side names the other, which is the whole
     * point of the arrangement: the app knows there may be add-ons and never
     * what any of them is for.
     */
    public static final String PLUGIN_ACTION =
            "net.pgaskin.remotedesktop.action.BACKEND_PLUGIN";

    /** What became of one installed package, for a screen that has to say so. */
    public enum PluginState {
        /** Its providers are in the list. */
        LOADED,
        /** Signed by us, but not this build's companion, or it declares nothing. */
        INCOMPATIBLE,
        /** It loaded and then threw. */
        FAILED,
    }

    /**
     * One add-on the app can see. A package that fails the signature check is
     * not one of these and is never reported: there is nothing useful to offer
     * about an impostor, and naming it invites somebody to interact with it.
     */
    public record Plugin(String packageName, PluginState state, String detail) {
    }

    private static volatile List<BackendProvider> providers;
    /** This build's own, which {@link #discover} needs before it can publish any. */
    private static List<BackendProvider> builtIn;
    private static List<Plugin> plugins = List.of();
    private static List<BackendProvider> discovered = List.of();
    private static volatile Chain chain;

    /** What an add-on's code threw where a screen called it, by package. */
    private static final Map<String, String> failedAtUse = new ConcurrentHashMap<>();

    /** Backends that have said yes to {@link #isSetup} once. */
    private static final Set<String> setUp = ConcurrentHashMap.newKeySet();

    /** Who is waiting on an {@link #isSetup} call, by backend; its own lock. */
    private static final Map<String, List<Consumer<Boolean>>> asking = new HashMap<>();
    private static ExecutorService worker; // under `asking`
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private Backends() {
    }

    /**
     * The app's own classloader, with a link on the end for add-ons.
     *
     * <p>Installed from {@code AppComponentFactory} before anything else runs.
     * It has to be the app's *own* loader rather than a child of it because
     * that is the one the platform hands to native code that asks for a class
     * from a thread of its own — a borrowed core resolving its callbacks — and
     * such a lookup has no Java frame to take a loader from.
     */
    public static ClassLoader classLoader(ClassLoader app) {
        final Chain c = new Chain(app);
        chain = c;
        return c;
    }

    /** The end of the chain: the app's dex first, then each add-on's. */
    private static final class Chain extends ClassLoader {

        private volatile List<Plugins> loaders = List.of();

        Chain(ClassLoader app) {
            super(app);
        }

        void add(Plugins loader) {
            final List<Plugins> next = new ArrayList<>(loaders);
            next.add(loader);
            loaders = List.copyOf(next);
        }

        @Override
        protected Class<?> findClass(String name) throws ClassNotFoundException {
            for (Plugins loader : loaders) {
                final Class<?> found = loader.local(name);
                if (found != null) {
                    return found;
                }
            }
            throw new ClassNotFoundException(name);
        }
    }

    /**
     * One add-on's dex. Its parent is the app's real loader rather than the
     * chain, so a lookup that reaches the chain cannot come back round.
     */
    private static final class Plugins extends PathClassLoader {

        final String packageName;

        Plugins(String packageName, String dex, String libs, ClassLoader parent) {
            super(dex, libs, parent);
            this.packageName = packageName;
        }

        Class<?> local(String name) {
            try {
                return findClass(name);
            } catch (ClassNotFoundException e) {
                return null;
            }
        }
    }

    /**
     * Load every add-on installed beside this app, once, before anything asks
     * for the backend list.
     *
     * <p>Called from {@code Application.onCreate} so that the home screen, a
     * session activity and the service all see the same list. It is once per
     * process and deliberately not repeatable: dex and a shared library that are
     * already open cannot be swapped underneath a live session, so an add-on
     * installed while the app runs is noticed ({@link #installedPlugins}) rather
     * than loaded.
     */
    public static synchronized void discover(Context context) {
        if (providers != null) {
            // Something read the list before this ran, so no add-on can be in
            // it. Loud, because it is a mistake in the app rather than in an
            // add-on — and not thrown, for the same reason the whole body below
            // is guarded.
            Log.e(TAG, "the backend list was read before discovery",
                    new IllegalStateException("too late"));
            return;
        }
        // Nothing here may be the reason the app will not start. This runs in
        // `Application.onCreate`, so a throw is every entry point gone — the
        // home screen, the session's service, and a timeout alarm's receiver
        // landing in a process that has nothing else in it. An app with no
        // add-ons is a worse app; an app that will not start is not one.
        try {
            find(context);
        } catch (Throwable t) {
            Log.e(TAG, "discovering add-ons", t);
        }
    }

    private static void find(Context context) {
        final PackageManager pm = context.getPackageManager();
        final long ours;
        try {
            ours = pm.getPackageInfo(context.getPackageName(), 0).getLongVersionCode();
        } catch (PackageManager.NameNotFoundException e) {
            throw new IllegalStateException(e); // we are installed
        }

        // This build's own, before any add-on's, because an id is what a saved
        // connection names and two things cannot answer to one.
        builtIn = builtIn();
        final Set<String> taken = new LinkedHashSet<>();
        for (BackendProvider p : builtIn) {
            taken.add(p.id());
        }

        final List<Plugin> found = new ArrayList<>();
        final List<BackendProvider> loaded = new ArrayList<>();
        for (String pkg : signedLikeUs(pm, context.getPackageName())) {
            final PackageInfo info;
            try {
                info = pm.getPackageInfo(pkg, 0);
            } catch (PackageManager.NameNotFoundException e) {
                continue; // uninstalled between the two calls
            }
            if (info.getLongVersionCode() != ours) {
                found.add(new Plugin(pkg, PluginState.INCOMPATIBLE,
                        "Built for app version " + info.getLongVersionCode() + "."));
                continue;
            }
            // Everything from here is somebody else's code being asked to
            // construct itself.
            try {
                final List<BackendProvider> mine = load(info.applicationInfo, taken);
                if (mine.isEmpty()) {
                    found.add(new Plugin(pkg, PluginState.INCOMPATIBLE, "It has no backend in it."));
                    continue;
                }
                for (BackendProvider p : mine) {
                    taken.add(p.id());
                }
                loaded.addAll(mine);
                found.add(new Plugin(pkg, PluginState.LOADED, null));
                Log.i(TAG, "loaded " + mine.size() + " backend(s) from " + pkg);
            } catch (Throwable t) {
                Log.e(TAG, "loading " + pkg, t);
                // Its own words where it has any, as everywhere else a plugin's
                // failure is repeated: a class name on a card helps nobody.
                final String said = t.getMessage();
                found.add(new Plugin(pkg, PluginState.FAILED,
                        said == null || said.isEmpty() ? String.valueOf(t) : said));
            }
        }
        plugins = List.copyOf(found);
        discovered = List.copyOf(loaded);
    }

    /** The providers this build was compiled with, in no particular order. */
    private static List<BackendProvider> builtIn() {
        final List<BackendProvider> own = new ArrayList<>();
        // The classloader is named rather than defaulted: the thread context
        // one is the app's here, but it is not ours to assume. (A minified
        // build would need these kept — nothing here is referenced by name.)
        for (BackendProvider p : ServiceLoader.load(
                BackendProvider.class, Backends.class.getClassLoader())) {
            own.add(p);
        }
        return List.copyOf(own);
    }

    /**
     * The packages that declare {@link #PLUGIN_ACTION} and are signed with our
     * key, in a stable order.
     *
     * <p>An unsigned-like package is dropped here and never mentioned again.
     * {@code checkSignatures} compares certificate sets, which is what "the same
     * key" means across an app and an add-on released together.
     */
    private static Set<String> signedLikeUs(PackageManager pm, String self) {
        final Set<String> candidates = new LinkedHashSet<>();
        for (ResolveInfo r : pm.queryIntentServices(new Intent(PLUGIN_ACTION), 0)) {
            candidates.add(r.serviceInfo.packageName);
        }
        candidates.remove(self);

        final Set<String> ours = new LinkedHashSet<>();
        for (String pkg : candidates) {
            if (pm.checkSignatures(self, pkg) == PackageManager.SIGNATURE_MATCH) {
                ours.add(pkg);
            }
        }
        return ours;
    }

    /**
     * The add-on's dex, under a loader parented to ours.
     *
     * <p>The parent is the point, and {@code createPackageContext} will not do:
     * its loader hangs off the boot classloader, which cannot see
     * {@link BackendProvider}, so nothing in the add-on would resolve. Splits
     * are included because an installer may have made some.
     *
     * @param taken every id already spoken for, this build's own included
     */
    private static List<BackendProvider> load(ApplicationInfo info, Set<String> taken) {
        // Without the chain an add-on's classes are reachable from Java and
        // from nothing else: a borrowed core resolving a callback by name from
        // a thread of its own is answered with the app's own loader, which
        // cannot see this dex. Half of that is worse than none of it.
        if (chain == null) {
            throw new IllegalStateException("This build of the app cannot load plugins.");
        }
        final StringBuilder dex = new StringBuilder(info.sourceDir);
        if (info.splitSourceDirs != null) {
            for (String split : info.splitSourceDirs) {
                dex.append(File.pathSeparatorChar).append(split);
            }
        }
        final Plugins loader = new Plugins(info.packageName,
                dex.toString(), info.nativeLibraryDir, Backends.class.getClassLoader());

        final List<BackendProvider> mine = new ArrayList<>();
        for (BackendProvider p : ServiceLoader.load(BackendProvider.class, loader)) {
            // Its own loader sees ours, so it finds the built-in services too.
            if (p.getClass().getClassLoader() != loader) {
                continue;
            }
            // Against this build's own ids as well as the other add-ons': a
            // saved connection names an id, the list is sorted by `order` and
            // answered first-match, so an add-on free to claim `rfb` at a lower
            // order is one that replaces that client for every connection on
            // the phone.
            if (taken.contains(p.id())) {
                throw new IllegalStateException("Another backend is already called " + p.id() + ".");
            }
            for (BackendProvider seen : mine) {
                if (seen.id().equals(p.id())) { // and against itself
                    throw new IllegalStateException("Another backend is already called " + p.id() + ".");
                }
            }
            mine.add(p);
        }
        if (!mine.isEmpty()) {
            chain.add(loader);
        }
        return mine;
    }

    /**
     * Which add-on a class was loaded from, or null for anything of the app's.
     *
     * <p>An add-on's code sometimes has to reach its own package — its
     * provider, its setup screen — and asking a {@link Context} gives the
     * app's package, since that is the process it is running in. This is the
     * only thing that knows, because it is what built the loader.
     */
    public static String packageOf(ClassLoader loader) {
        return loader instanceof Plugins p ? p.packageName : null;
    }

    /** The add-on a backend came from, or null for one built into the app. */
    public static String packageOf(String id) {
        final BackendProvider p = provider(id);
        return p == null ? null : packageOf(p.getClass().getClassLoader());
    }

    /**
     * What {@link #discover} made of each add-on, for a screen that says so,
     * with anything that has since thrown ({@link #failed}) folded in: an
     * add-on that loaded and then failed is a failed one, and the difference
     * between the two moments is of no interest to whoever has to deal with it.
     */
    public static synchronized List<Plugin> plugins() {
        if (failedAtUse.isEmpty()) {
            return plugins;
        }
        final List<Plugin> merged = new ArrayList<>(plugins.size());
        for (Plugin p : plugins) {
            final String said = failedAtUse.get(p.packageName());
            merged.add(said == null ? p
                    : new Plugin(p.packageName(), PluginState.FAILED, said));
        }
        return List.copyOf(merged);
    }

    /**
     * Somebody else's code threw where a screen called it.
     *
     * <p>Recorded against the add-on it came from, so that the failure has
     * somewhere to be said and something to be done about it — the screen that
     * caught it says so once and is gone, and the card the add-on grows is what
     * offers to uninstall it. A backend of the app's own has neither, and is
     * logged and nothing more.
     */
    public static void failed(String id, Throwable t) {
        Log.e(TAG, "backend " + id, t);
        final String pkg = packageOf(id);
        if (pkg != null) {
            final String said = t.getMessage();
            failedAtUse.put(pkg, said == null || said.isEmpty() ? String.valueOf(t) : said);
        }
    }

    /**
     * The add-ons installed now, which is not what {@link #discover} loaded if
     * one has arrived or gone since. A screen compares the two and asks for a
     * restart.
     */
    public static Set<String> installedPlugins(Context context) {
        return signedLikeUs(context.getPackageManager(), context.getPackageName());
    }

    private static synchronized List<BackendProvider> providers() {
        List<BackendProvider> found = providers;
        if (found == null) {
            // Whatever `discover` weighed the add-ons against, so that the id
            // it refused a clash on is the id in this list. Its own only where
            // discovery never ran at all.
            final List<BackendProvider> loaded =
                    new ArrayList<>(builtIn != null ? builtIn : builtIn());
            loaded.addAll(discovered);
            loaded.sort(Comparator.comparingInt(BackendProvider::order)
                    .thenComparing(BackendProvider::id));
            found = List.copyOf(loaded);
            providers = found;
        }
        return found;
    }

    private static BackendProvider provider(String id) {
        for (BackendProvider p : providers()) {
            if (p.id().equals(id)) {
                return p;
            }
        }
        return null;
    }

    public static List<String> ids() {
        return providers().stream().map(BackendProvider::id).toList();
    }

    public static String name(String id) {
        final BackendProvider p = provider(id);
        return p == null ? id : p.name();
    }

    /**
     * Empty for a backend this build does not have — a card for one still has
     * to draw, and naming something that cannot be opened is worse than saying
     * nothing.
     */
    public static String shortName(String id) {
        final BackendProvider p = provider(id);
        return p == null ? "" : p.shortName();
    }

    /** Empty for a backend this build does not have, as {@link #shortName}. */
    public static String description(String id) {
        final BackendProvider p = provider(id);
        return p == null ? "" : p.description();
    }

    public static List<BackendOption> options(String id) {
        final BackendProvider p = provider(id);
        return p == null ? List.of() : p.options();
    }

    /**
     * Whether a backend can be connected with, for a screen deciding whether to
     * offer {@link #setup} instead of what it would otherwise do.
     *
     * <p><b>Asked on a thread of its own and answered on the main one.</b> A
     * provider answers this by looking at what it was given — hashing somebody
     * else's library, or asking the process that holds it — and a screen asks
     * every time it resumes, so the question does not belong on the thread
     * drawing it. The answer is delivered <em>at once</em>, before this returns,
     * where it is already known: a screen that asks in {@code onCreate} is then
     * not a frame behind itself in the ordinary case.
     *
     * <p><b>One call at a time per backend</b>, however many screens ask: a
     * second asker joins the first's answer rather than starting a second hash.
     * A yes is remembered for the life of the process, since what a yes means is
     * that a file is in place and the load verifies it again anyway; a no never
     * is, because re-asking after {@link #setup} is the whole mechanism.
     *
     * <p>True for an id this build does not have, which is a case its callers
     * already have a sentence for. A throw is the backend saying no in the only
     * other way it can, and is recorded ({@link #failed}) — where {@link #setup}
     * is a person pressing a button, and lets the throw out so that they hear
     * about it.
     *
     * <p><b>An answer for a screen that has gone is dropped here</b>, so that
     * four call sites do not need four opinions about it: what every one of
     * them would do with a late answer is open a screen or write on one, and
     * the context it asked with is what says whether there is one.
     */
    public static void isSetup(Context context, String id, Consumer<Boolean> answer) {
        final Consumer<Boolean> to = whileThereIsAScreen(context, answer);
        final BackendProvider p = provider(id);
        if (p == null || setUp.contains(id)) {
            to.accept(true);
            return;
        }
        // The application context, because this outlives the screen that asked.
        final Context app = context.getApplicationContext();
        synchronized (asking) {
            final List<Consumer<Boolean>> waiting = asking.get(id);
            if (waiting != null) {
                waiting.add(to);
                return;
            }
            asking.put(id, new ArrayList<>(List.of(to)));
            if (worker == null) {
                worker = Executors.newSingleThreadExecutor(r -> {
                    final Thread t = new Thread(r, "backend-setup");
                    t.setDaemon(true);
                    return t;
                });
            }
        }
        worker.execute(() -> {
            boolean ok;
            try {
                ok = p.isSetup(app);
            } catch (Throwable t) {
                failed(id, t);
                ok = false;
            }
            if (ok) {
                setUp.add(id);
            }
            final List<Consumer<Boolean>> waiting;
            synchronized (asking) {
                waiting = asking.remove(id);
            }
            final boolean said = ok;
            MAIN.post(() -> {
                // One at a time, guarded: these are four unrelated screens, and
                // one of them throwing must not be the others never hearing.
                // The home screen counts its answers down to decide when to
                // draw, so a lost one is a list that is wrong until the process
                // ends.
                for (Consumer<Boolean> c : waiting) {
                    try {
                        c.accept(said);
                    } catch (Throwable t) {
                        Log.e(TAG, "answering isSetup for " + id, t);
                    }
                }
            });
        });
    }

    /**
     * The rule, in one place. A context that is not a screen — the application,
     * a service — is one nothing can have navigated away from, and its answer
     * always arrives.
     */
    private static Consumer<Boolean> whileThereIsAScreen(Context context, Consumer<Boolean> answer) {
        if (!(context instanceof Activity host)) {
            return answer;
        }
        return ok -> {
            if (!host.isDestroyed() && !host.isFinishing()) {
                answer.accept(ok);
            }
        };
    }

    public static void setup(Activity host, String id) {
        final BackendProvider p = provider(id);
        if (p != null) {
            p.setup(host);
        }
    }

    /**
     * Forget every remembered yes, so that the next asker asks the backend again.
     *
     * <p>{@link #isSetup} keeps a yes for the life of the process because what a
     * yes means is that a file is in place, and nothing takes one away. The app
     * deleting its own copies of an add-on's libraries is the one thing that
     * does, and it says so here — otherwise a screen goes on holding an answer
     * about a file that is gone: no card where there should be one, and a
     * session that fails where it should have offered to set the backend up.
     */
    public static void setupChanged() {
        setUp.clear();
    }

    /**
     * Every server this phone has accepted, forgotten — the shared store and
     * whatever each backend keeps of its own.
     *
     * <p>Both halves, because "which machines am I no longer asked about?" is
     * one question and a screen offering to answer half of it would be worse
     * than one offering nothing. A backend that throws is recorded and the rest
     * still run, for the same reason the loop over four screens' callbacks is
     * guarded: one add-on must not be the others being left pinned.
     */
    public static void forgetHosts(Context context) {
        final Context app = context.getApplicationContext();
        KnownHosts.clear(app);
        for (BackendProvider p : providers()) {
            try {
                p.forgetHosts(app);
            } catch (Throwable t) {
                failed(p.id(), t);
            }
        }
    }

    public static Backend create(Context context, String id, String address, String userName,
                                 String password, Map<String, String> options) {
        final BackendProvider p = provider(id);
        if (p == null) {
            throw new IllegalArgumentException(
                    "no such backend in this build: " + id + " (it has " + ids() + ")");
        }
        return p.create(context, address, userName, password, options);
    }
}
