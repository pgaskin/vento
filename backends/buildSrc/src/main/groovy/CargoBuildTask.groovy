// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.Property
import org.gradle.api.provider.SetProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

import java.util.regex.Pattern

/**
 * Builds one crate of the workspace for every packaged ABI, with {@code cargo
 * ndk}, into a directory laid out the way {@code jniLibs} wants it: one
 * {@code <abi>/} directory each, holding the crate's library.
 *
 * <p>That library is a {@code cdylib} for a backend whose library is its Rust
 * crate, and a {@code staticlib} for one whose library Meson links: the C
 * backends, where the crate is the adapter and the JNI layer that go into the
 * C library's {@code .so}, and Meson is told where to find it. The crate type
 * is given on the command line ({@code cargo rustc --crate-type}) rather than
 * declared in the crate, so that the host build, which links the crate as an
 * ordinary dependency, does not also build an unused static library of it.
 *
 * <p>Three things here are not a {@code cargo} invocation. The NDK comes from
 * {@code androidComponents.sdkComponents.ndkDirectory}, so {@code ndkVersion} is
 * the single pin for the linker too. Tool versions are checked before they are
 * run, and a mismatch fails with the command that fixes it. And the environment
 * is scrubbed, because an ambient {@code CC} or {@code RUSTFLAGS} would silently
 * change what is linked into the APK.
 *
 * <p>Not up-to-date checked: cargo does its own incremental build, and a
 * second up-to-date check in Gradle could package a stale {@code .so}.
 */
abstract class CargoBuildTask extends DefaultTask {

    /** Removed rather than overridden: the build depends on the pins and
     * nothing else. */
    private static final List<String> ENV_IGNORED = [
            'AR', 'CC', 'CFLAGS', 'CXX', 'CXXFLAGS', 'LDFLAGS', 'RANLIB',
            'CARGO_ENCODED_RUSTFLAGS', 'CARGO_INCREMENTAL',
            'RUSTC', 'RUSTC_BOOTSTRAP', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
            'RUSTDOCFLAGS', 'RUSTFLAGS', 'RUSTUP_TOOLCHAIN',
            'SOURCE_DATE_EPOCH',
    ]
    private static final List<String> ENV_IGNORED_PREFIX = [
            'AR_', 'CC_', 'CFLAGS_', 'CXX_', 'CXXFLAGS_', 'LDFLAGS_', 'RANLIB_',
            'CARGO_BUILD_', 'CARGO_PROFILE_', 'CARGO_TARGET_', 'CARGO_UNSTABLE_',
            'TARGET_',
    ]

    /** cargo splits CARGO_ENCODED_RUSTFLAGS on ASCII unit separators. */
    private static final String RUSTFLAGS_SEPARATOR = "\u001f"

    private static final Pattern TOOLCHAIN_CHANNEL = ~/(?m)^\s*channel\s*=\s*"([^"]+)"/
    private static final Pattern RUSTC_VERSION = ~/^rustc (\S+)/
    private static final Pattern CARGO_NDK_VERSION = ~/^cargo-ndk (\S+)/
    private static final Pattern RUSTC_COMMIT_HASH = ~/(?m)^commit-hash: ([0-9a-f]{40})$/

    /** What {@code cargo ndk} calls each ABI, as the target triple cargo puts it under. */
    private static final Map<String, String> TRIPLES = [
            'arm64-v8a'  : 'aarch64-linux-android',
            'armeabi-v7a': 'armv7-linux-androideabi',
            'x86'        : 'i686-linux-android',
            'x86_64'     : 'x86_64-linux-android',
    ]

    CargoBuildTask() {
        outputs.upToDateWhen { false }
        crateType.convention('cdylib')
    }

    /** The ABIs to build, as {@code jniLibs} names them. */
    @Input
    abstract SetProperty<String> getAbiFilters()

    /** {@code cargo ndk --platform}: the API level to link against. */
    @Input
    abstract Property<Integer> getMinSdkVersion()

    @Input
    abstract Property<String> getCargoPackage()

    /** {@code cdylib}, the default, or {@code staticlib}; see above. */
    @Input
    abstract Property<String> getCrateType()

    /** The one version of {@code cargo-ndk} this build is known to work with. */
    @Input
    abstract Property<String> getCargoNdkVersion()

    @InputFile
    @PathSensitive(PathSensitivity.NONE)
    abstract RegularFileProperty getRustToolchainFile()

    @Internal
    abstract DirectoryProperty getNdkDirectory()

    /** The workspace root, which is also the Gradle root. */
    @Internal
    abstract DirectoryProperty getWorkspaceDir()

    @OutputDirectory
    abstract DirectoryProperty getOutputDir()

    @Inject
    abstract ExecOperations getExecOperations()

    @TaskAction
    void build() {
        final abis = abiFilters.get()
        if (abis.isEmpty()) {
            throw new GradleException("${name}: no ABIs to build for (set android.defaultConfig.ndk.abiFilters)")
        }

        final staticlib = crateType.get() == 'staticlib'
        if (!staticlib && crateType.get() != 'cdylib') {
            throw new GradleException("${name}: crateType is cdylib or staticlib, not ${crateType.get()}")
        }
        final outDir = outputDir.get().asFile
        // A static library is replaced only when it changed (below), so that
        // Meson, which links it, does not relink every library on every build.
        if (!staticlib) {
            outDir.deleteDir()
        }
        outDir.mkdirs()

        final env = scrubbedEnvironment()
        env.put('ANDROID_NDK_HOME', ndkDirectory.get().asFile.absolutePath)
        env.put('CARGO_TERM_PROGRESS_WHEN', 'never')
        env.put('CARGO_TERM_COLOR', 'never')

        checkVersions(env)

        final workspace = workspaceDir.get().asFile
        final sysroot = capture(env, ['rustc', '--print', 'sysroot'])
        final rustflags = [
                "--remap-path-prefix=${cargoHome(env)}=/cargo",
                "--remap-path-prefix=${sysroot}=/rust",
        ]
        // With the rust-src component installed, rustc replaces the standard
        // library's /rustc/<commit-hash> paths with that component's copy in
        // the sysroot, and the panic locations of standard library code
        // compiled into the crate keep them. Mapping the copy back keeps the
        // library the same with or without rust-src. rustc applies the last
        // matching prefix, so this follows the sysroot's.
        final commit = RUSTC_COMMIT_HASH.matcher(capture(env, ['rustc', '-vV']))
        if (commit.find()) {
            rustflags << "--remap-path-prefix=${sysroot}/lib/rustlib/src/rust=/rustc/${commit.group(1)}"
        }
        rustflags << "--remap-path-prefix=${workspace.absolutePath}=/remotedesktop"
        env.put('CARGO_ENCODED_RUSTFLAGS', rustflags.join(RUSTFLAGS_SEPARATOR))

        final cmd = ['cargo', 'ndk']
        abis.toSorted().each { cmd.addAll(['-t', it]) }
        cmd.addAll(['--platform', minSdkVersion.get().toString()])
        if (staticlib) {
            cmd.addAll(['rustc', '--locked', '--release', '--package', cargoPackage.get(),
                        '--crate-type', 'staticlib'])
        } else {
            cmd.addAll(['--output-dir', outDir.absolutePath, 'build', '--locked', '--release',
                        '--package', cargoPackage.get()])
        }

        execOperations.exec {
            it.workingDir = workspace
            it.commandLine = cmd
            it.environment = env
        }

        if (staticlib) {
            // cargo-ndk copies only shared libraries out, so the archive is
            // taken from where cargo left it: the workspace's own target
            // directory, since the environment above has no CARGO_TARGET_DIR.
            final file = "lib${cargoPackage.get().replace('-', '_')}.a"
            abis.each { abi ->
                final triple = TRIPLES[abi]
                if (triple == null) {
                    throw new GradleException("${name}: no target triple known for ${abi}")
                }
                final built = new File(workspace, "target/${triple}/release/${file}")
                final dest = new File(outDir, "${abi}/${file}")
                if (!dest.exists() || java.nio.file.Files.mismatch(built.toPath(), dest.toPath()) != -1L) {
                    dest.parentFile.mkdirs()
                    java.nio.file.Files.copy(built.toPath(), dest.toPath(),
                            java.nio.file.StandardCopyOption.REPLACE_EXISTING)
                }
            }
        }
    }

    /**
     * This process's environment less everything that would change what cargo
     * builds without being a pin: {@link #ENV_IGNORED} and the prefixes after
     * it. {@link GenerateJavaTask} runs its generator under the same.
     */
    static Map<String, String> scrubbedEnvironment() {
        final env = new LinkedHashMap<String, String>(System.getenv())
        env.keySet().removeIf { name ->
            ENV_IGNORED.contains(name) || ENV_IGNORED_PREFIX.any { name.startsWith(it) }
        }
        return env
    }

    /** See cargo's own {@code home::cargo_home}. */
    // Not private: Gradle decorates the task class at runtime, and a private
    // method is not visible on the subclass it calls this through.
    static String cargoHome(Map<String, String> env) {
        final home = env.get('CARGO_HOME')
        if (home != null && !home.isEmpty()) {
            return new File(home).absolutePath
        }
        return new File(System.getProperty('user.home'), '.cargo').absolutePath
    }

    void checkVersions(Map<String, String> env) {
        final toolchain = rustToolchainFile.get().asFile
        final channel = TOOLCHAIN_CHANNEL.matcher(toolchain.getText('UTF-8'))
        if (!channel.find()) {
            throw new GradleException("could not parse the toolchain channel from ${toolchain}")
        }
        checkVersion(env, ['rustc', '--version'], RUSTC_VERSION, channel.group(1),
                "rustc (is rustup on the PATH, so ${toolchain.name} is applied?)")
        checkVersion(env, ['cargo', 'ndk', '--version'], CARGO_NDK_VERSION, cargoNdkVersion.get(),
                "cargo-ndk (cargo install cargo-ndk@${cargoNdkVersion.get()})")
    }

    void checkVersion(Map<String, String> env, List<String> cmd, Pattern pattern,
                      String expected, String what) {
        final matcher = pattern.matcher(capture(env, cmd))
        if (!matcher.find()) {
            throw new GradleException("could not parse the version from ${cmd.join(' ')}")
        }
        if (matcher.group(1) != expected) {
            throw new GradleException("expected ${what} version ${expected}, got ${matcher.group(1)}")
        }
    }

    String capture(Map<String, String> env, List<String> cmd) {
        final out = new ByteArrayOutputStream()
        try {
            execOperations.exec {
                it.workingDir = workspaceDir.get().asFile
                it.commandLine = cmd
                it.standardOutput = out
                it.errorOutput = new ByteArrayOutputStream()
                it.environment = env
            }
        } catch (Exception ex) {
            throw new GradleException("failed to run ${cmd.join(' ')}: ${ex}", ex)
        }
        return out.toString('UTF-8').trim()
    }
}
