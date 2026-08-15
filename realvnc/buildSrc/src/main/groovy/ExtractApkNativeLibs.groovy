// SPDX-License-Identifier: CC0-1.0

import java.security.MessageDigest
import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.ArchiveOperations
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.FileSystemOperations
import org.gradle.api.provider.MapProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.CacheableTask
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction

/**
 * Unpacks pinned {@code lib/<abi>/*.so} entries out of an APK, into the layout
 * {@code jniLibs} wants, and hands on nothing whose SHA-256 is not the pinned
 * one — so a proprietary library can be a generated input rather than a
 * committed one without anybody having to trust the download it came from.
 *
 * <p>{@code apk} is an {@link InputFiles} so that a missing APK reaches the task
 * action and can be reported in terms of what to do about it.
 */
@CacheableTask
abstract class ExtractApkNativeLibs extends DefaultTask {

    /** The first file is used; empty is reported, not validated. */
    @InputFiles
    @PathSensitive(PathSensitivity.NONE)
    abstract ConfigurableFileCollection getApk()

    /** {@code <abi>/<name>.so} → lowercase SHA-256. Nothing outside it is unpacked. */
    @Input
    abstract MapProperty<String, String> getExpected()

    @OutputDirectory
    abstract DirectoryProperty getOutputDir()

    /** What to say when there is no APK. Set by the caller, which knows why. */
    @Input
    abstract Property<String> getMissingApkHint()

    @Inject
    abstract FileSystemOperations getFs()

    @Inject
    abstract ArchiveOperations getArchives()

    @TaskAction
    void extract() {
        final files = apk.files.findAll { it.isFile() }
        if (files.isEmpty()) {
            throw new GradleException(missingApkHint.getOrElse('no APK to unpack native libraries from'))
        }
        final source = files.first()
        final wanted = expected.get()
        if (wanted.isEmpty()) {
            throw new GradleException("${name}: no libraries pinned")
        }

        // Sync, so a different APK does not leave the last one's libraries behind.
        fs.sync {
            it.from(archives.zipTree(source)) { spec ->
                wanted.keySet().each { path -> spec.include("lib/${path}") }
            }
            it.eachFile { f -> f.path = f.path.replaceFirst('^lib/', '') }
            it.includeEmptyDirs = false
            it.into(outputDir)
        }

        final root = outputDir.get().asFile
        final problems = []
        wanted.each { path, want ->
            final file = new File(root, path)
            if (!file.isFile()) {
                problems << "  ${path}: missing"
                return
            }
            final got = sha256(file)
            if (got != want.toLowerCase(Locale.ROOT)) {
                problems << "  ${path}: expected ${want}, got ${got}"
            }
        }
        if (problems) {
            // Leave nothing unverified behind for an up-to-date check to accept.
            fs.delete { it.delete(root) }
            throw new GradleException("${source.name} does not contain the pinned native " +
                    'libraries, so it is not the build these bindings were read from:\n' +
                    problems.join('\n'))
        }
        logger.lifecycle("unpacked ${wanted.size()} verified native libraries from ${source.name}")
    }

    static String sha256(File file) {
        final digest = MessageDigest.getInstance('SHA-256')
        file.withInputStream { stream ->
            final buffer = new byte[64 * 1024]
            int n
            while ((n = stream.read(buffer)) >= 0) {
                digest.update(buffer, 0, n)
            }
        }
        return digest.digest().encodeHex().toString()
    }
}
