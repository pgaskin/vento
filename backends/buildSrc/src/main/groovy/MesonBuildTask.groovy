// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.Property
import org.gradle.api.provider.SetProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

/**
 * Builds one C backend's {@code libremotedesktop_<backend>.so} for every
 * packaged ABI with the Meson build at the repository root
 * ({@code -Dbackends=<backend>}), into a directory laid out the way
 * {@code jniLibs} wants it: one {@code <abi>/} directory each.
 *
 * <p>Each ABI has a build tree of its own under {@link #getBuildDir}, set up
 * once and again for another cross file, and reconfigured when the options
 * change; ninja notices everything else. The
 * cross file names the NDK's clang for the ABI and API level, so that
 * {@code ndkVersion} is the one pin of the compiler; the flags the NDK's own
 * toolchain file would add are in {@code backends/platform/android/meson.build}.
 *
 * <p>Meson is the release {@code :backends:fetchMeson} unpacked, run with the
 * {@code python3} on the PATH, and ninja is the one on the PATH. The
 * environment is scrubbed of what would change the compiler's flags.
 *
 * <p>Not up-to-date checked: ninja does its own incremental build.
 */
abstract class MesonBuildTask extends DefaultTask {

    private static final List<String> ENV_IGNORED = [
            'AR', 'AS', 'CC', 'CFLAGS', 'CPPFLAGS', 'CXX', 'CXXFLAGS', 'LDFLAGS', 'RANLIB', 'STRIP',
            'PKG_CONFIG', 'PKG_CONFIG_PATH', 'PKG_CONFIG_LIBDIR', 'PKG_CONFIG_SYSROOT_DIR',
            'SOURCE_DATE_EPOCH',
    ]

    /** The cross file's names for each ABI: the clang target, and Meson's CPU family. */
    private static final Map<String, List<String>> ABIS = [
            'arm64-v8a': ['aarch64-none-linux-android', 'aarch64'],
            'x86_64'   : ['x86_64-none-linux-android', 'x86_64'],
    ]

    MesonBuildTask() {
        outputs.upToDateWhen { false }
    }

    /** The {@code backends} option: the one backend this builds. */
    @Input
    abstract Property<String> getBackend()

    /** The ABIs to build, as {@code jniLibs} names them. */
    @Input
    abstract SetProperty<String> getAbiFilters()

    /** The API level the compiler targets. */
    @Input
    abstract Property<Integer> getMinSdkVersion()

    /** The {@code variant} option: {@code debug} or {@code release}. */
    @Input
    abstract Property<String> getVariant()

    /** {@code -PstrictWarnings}: the {@code strict_warnings} option. */
    @Input
    abstract Property<Boolean> getStrictWarnings()

    /** The {@code crate_library_dir} option: where cargo-ndk left the
     * backend's crate as {@code <abi>/lib<crate>.a}; none for RealVNC. */
    @InputDirectory
    @PathSensitive(PathSensitivity.RELATIVE)
    @Optional
    abstract DirectoryProperty getCrateLibraryDir()

    /** The unpacked Meson release. */
    @Internal
    abstract DirectoryProperty getMesonDir()

    @Internal
    abstract DirectoryProperty getNdkDirectory()

    /** The Meson source root: the repository root. */
    @Internal
    abstract DirectoryProperty getSourceDir()

    /** The build trees, one per ABI. */
    @Internal
    abstract DirectoryProperty getBuildDir()

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
        final mesonPy = new File(mesonDir.get().asFile, 'meson.py')
        if (!mesonPy.isFile()) {
            throw new GradleException("${name}: no Meson at ${mesonDir.get().asFile}; run ./gradlew :backends:fetchMeson")
        }
        final meson = ['python3', mesonPy.absolutePath]

        final env = new LinkedHashMap<String, String>(System.getenv())
        env.keySet().removeAll(ENV_IGNORED)

        final outDir = outputDir.get().asFile
        outDir.deleteDir()
        abis.toSorted().each { abi ->
            final tree = new File(buildDir.get().asFile, abi)
            final cross = new File(buildDir.get().asFile, "${abi}.cross")
            cross.parentFile.mkdirs()
            writeIfChanged(cross, crossFile(abi))

            final options = [
                    "-Dbackends=${backend.get()}",
                    "-Dvariant=${variant.get()}",
                    "-Dstrict_warnings=${strictWarnings.get()}",
            ]*.toString()
            if (crateLibraryDir.present) {
                options << "-Dcrate_library_dir=${crateLibraryDir.get().asFile.absolutePath}".toString()
            }
            // A tree is set up afresh for another cross file (a new NDK, or a new
            // API level), and reconfigured in place for other options, which
            // recompiles only what they change (-PstrictWarnings: our sources).
            final crossStamp = new File(buildDir.get().asFile, "${abi}.cross.configured")
            final optionsStamp = new File(buildDir.get().asFile, "${abi}.options")
            final configured = new File(tree, 'build.ninja').isFile()
            if (!configured || !crossStamp.isFile() || crossStamp.getText('UTF-8') != cross.getText('UTF-8')) {
                exec(meson + ['setup'] + (configured ? ['--wipe'] : []) + ["--cross-file=${cross.absolutePath}".toString()] +
                        options + [tree.absolutePath, sourceDir.get().asFile.absolutePath], env)
                crossStamp.setText(cross.getText('UTF-8'), 'UTF-8')
                optionsStamp.setText(options.join('\n'), 'UTF-8')
            } else if (!optionsStamp.isFile() || optionsStamp.getText('UTF-8') != options.join('\n')) {
                exec(meson + ['configure', tree.absolutePath] + options, env)
                optionsStamp.setText(options.join('\n'), 'UTF-8')
            }
            exec(meson + ['compile', '-C', tree.absolutePath], env)

            final name = "libremotedesktop_${backend.get()}.so"
            final lib = new File(tree, "backends/modules/${backend.get()}/cpp/${name}")
            final dest = new File(outDir, "${abi}/${name}")
            dest.parentFile.mkdirs()
            java.nio.file.Files.copy(lib.toPath(), dest.toPath())
        }
    }

    void exec(List<String> cmd, Map<String, String> env) {
        execOperations.exec {
            it.workingDir = sourceDir.get().asFile
            it.commandLine = cmd
            it.environment = env
        }
    }

    /** The cross file for one ABI: the NDK's clang and binutils, and the machine. */
    String crossFile(String abi) {
        final machine = ABIS[abi]
        if (machine == null) {
            throw new GradleException("${name}: no Meson cross file for ABI ${abi}; add it to MesonBuildTask.ABIS")
        }
        final toolchain = new File(ndkDirectory.get().asFile, "toolchains/llvm/prebuilt/${hostTag()}")
        if (!new File(toolchain, 'bin/clang').canExecute()) {
            throw new GradleException("${name}: no NDK clang under ${toolchain}")
        }
        return """\
            # Written by MesonBuildTask for ${abi} and API level ${minSdkVersion.get()}.
            [constants]
            toolchain = '${toolchain.absolutePath}'
            target = ['--target=${machine[0]}${minSdkVersion.get()}', '--sysroot=' + toolchain / 'sysroot']

            [binaries]
            c = [toolchain / 'bin/clang'] + target
            cpp = [toolchain / 'bin/clang++'] + target
            ar = toolchain / 'bin/llvm-ar'
            strip = toolchain / 'bin/llvm-strip'

            [host_machine]
            system = 'android'
            cpu_family = '${machine[1]}'
            cpu = '${machine[1]}'
            endian = 'little'
            """.stripIndent()
    }

    /** The NDK's name for the machine it runs on. */
    static String hostTag() {
        final os = System.getProperty('os.name').toLowerCase(Locale.ROOT)
        if (os.contains('linux')) return 'linux-x86_64'
        if (os.contains('mac')) return 'darwin-x86_64'
        throw new GradleException("no NDK prebuilt for ${os}")
    }

    static void writeIfChanged(File file, String text) {
        if (!file.isFile() || file.getText('UTF-8') != text) {
            file.setText(text, 'UTF-8')
        }
    }
}
