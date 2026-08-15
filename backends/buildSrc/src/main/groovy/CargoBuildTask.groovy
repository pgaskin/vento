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
 * ndk}, into a directory laid out the way {@code jniLibs} wants it.
 *
 * <p>Three things here are not a {@code cargo} invocation. The NDK comes from
 * {@code androidComponents.sdkComponents.ndkDirectory}, so {@code ndkVersion} is
 * the single pin for the linker too. Tool versions are checked before they are
 * run, and a mismatch fails with the command that fixes it. And the environment
 * is scrubbed, because an ambient {@code CC} or {@code RUSTFLAGS} would silently
 * change what is linked into the APK.
 *
 * <p>Not up-to-date checked: cargo does its own incremental build, and Gradle
 * second-guessing it is how a stale {@code .so} gets packaged.
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

    CargoBuildTask() {
        outputs.upToDateWhen { false }
    }

    /** The ABIs to build, as {@code jniLibs} names them. */
    @Input
    abstract SetProperty<String> getAbiFilters()

    /** {@code cargo ndk --platform}: the API level to link against. */
    @Input
    abstract Property<Integer> getMinSdkVersion()

    @Input
    abstract Property<String> getCargoPackage()

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

        final outDir = outputDir.get().asFile
        outDir.deleteDir()
        outDir.mkdirs()

        final env = new LinkedHashMap<String, String>(System.getenv())
        env.keySet().removeIf { name ->
            ENV_IGNORED.contains(name) || ENV_IGNORED_PREFIX.any { name.startsWith(it) }
        }
        env.put('ANDROID_NDK_HOME', ndkDirectory.get().asFile.absolutePath)
        env.put('CARGO_TERM_PROGRESS_WHEN', 'never')
        env.put('CARGO_TERM_COLOR', 'never')

        checkVersions(env)

        final workspace = workspaceDir.get().asFile
        env.put('CARGO_ENCODED_RUSTFLAGS', [
                "--remap-path-prefix=${cargoHome(env)}=/cargo",
                "--remap-path-prefix=${capture(env, ['rustc', '--print', 'sysroot'])}=/rust",
                "--remap-path-prefix=${workspace.absolutePath}=/remotedesktop",
        ].join(RUSTFLAGS_SEPARATOR))

        final cmd = ['cargo', 'ndk']
        abis.toSorted().each { cmd.addAll(['-t', it]) }
        cmd.addAll([
                '--platform', minSdkVersion.get().toString(),
                '--output-dir', outDir.absolutePath,
                'build',
                '--locked',
                '--release',
                '--package', cargoPackage.get(),
        ])

        execOperations.exec {
            it.workingDir = workspace
            it.commandLine = cmd
            it.environment = env
        }
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
