// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import java.security.MessageDigest
import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

/**
 * Unpacks one upstream release tarball into {@code third_party/}, from the
 * first mirror that answers, and only if it hashes to the pin.
 *
 * <p>It takes the place of a submodule for the two upstreams whose git tree
 * does not build: their {@code configure} is an autotools bootstrap output
 * that a release carries and a checkout does not. So the release is the
 * upstream source, a SHA-256 names it exactly, and the downloaded file is
 * verified before it is unpacked.
 *
 * <p>No build task depends on a source tarball: the Android build never
 * fetches one, and its Meson configure reports a missing tree with the
 * command that fetches it. The Linux Meson build unpacks a missing tree
 * itself ({@code backends/fetch-crypto-sources.py}), from the same pins
 * ({@code backends/crypto-sources.txt}) and into the same layout, {@code .pin}
 * included. The other tarball is Meson's own release, which the C modules'
 * builds depend on ({@code fetchMeson} in {@code backends/build.gradle}).
 */
abstract class DownloadSourceTarball extends DefaultTask {

    /** Mirrors in order; the first that answers with the pinned bytes wins. */
    @Input
    abstract ListProperty<String> getUrls()

    @Input
    abstract Property<String> getSha256()

    /** Leading path components to drop, as {@code tar --strip-components}. */
    @Input
    abstract Property<Integer> getStripComponents()

    /** Unpacked here, with a {@code .pin} beside the sources saying what from. */
    @OutputDirectory
    abstract DirectoryProperty getOutputDir()

    @Inject
    abstract ExecOperations getExecOperations()

    @TaskAction
    void fetch() {
        final target = outputDir.get().asFile
        final want = sha256.get()
        final pin = new File(target, '.pin')

        if (pin.isFile() && pin.text.trim() == want) {
            logger.lifecycle("${target.name} is already unpacked at ${want.take(12)}…")
            return
        }

        final failures = []
        for (String url : urls.get()) {
            final temp = new File(temporaryDir, url.substring(url.lastIndexOf('/') + 1))
            try {
                logger.lifecycle("fetching ${url}")
                // Read outside the closure: a decorated task's own methods are
                // not visible to a closure's dispatch.
                final bytes = read(url)
                temp.withOutputStream { out -> out.write(bytes) }
                final got = sha256(temp)
                if (got != want) {
                    failures << "${url}: sha256 ${got}, wanted ${want}"
                    continue
                }
                unpack(temp, target)
                pin.text = want + '\n'
                logger.lifecycle("${target.name} unpacked and verified")
                return
            } catch (Exception e) {
                failures << "${url}: ${e.message}"
            } finally {
                temp.delete()
            }
        }

        throw new GradleException([
                "could not unpack ${target.name}. Every mirror failed:",
                '',
                *failures.collect { "  ${it}" },
                '',
                "Put the tarball's contents at ${target} by hand instead — any copy that",
                "hashes to ${want} will do — or fetch it again when the network is back.",
        ].join('\n'))
    }

    /**
     * Replaces the tree rather than unpacking over it: a partial or older tree
     * would keep stale files, and the build would compile sources the pin does
     * not name.
     */
    private void unpack(File tarball, File target) {
        target.deleteDir()
        target.mkdirs()
        final tar = execOperations.exec {
            commandLine 'tar', '-xf', tarball.absolutePath,
                    "--strip-components=${stripComponents.get()}", '-C', target.absolutePath
            ignoreExitValue = true
        }
        if (tar.exitValue != 0) {
            throw new RuntimeException('tar failed; it and an xz that it can call are ' +
                    'what this task needs beyond Gradle')
        }
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

    /** Redirects by hand: HttpURLConnection will not follow one across schemes. */
    static byte[] read(String url) {
        def location = url
        for (int hop = 0; hop < 5; hop++) {
            final conn = (HttpURLConnection) new URI(location).toURL().openConnection()
            conn.instanceFollowRedirects = false
            conn.connectTimeout = 30_000
            conn.readTimeout = 300_000
            final code = conn.responseCode
            if (code in [301, 302, 303, 307, 308]) {
                final next = conn.getHeaderField('Location')
                conn.disconnect()
                if (!next) {
                    throw new RuntimeException("HTTP ${code} with no Location")
                }
                location = new URI(location).resolve(next).toString()
                continue
            }
            if (code != 200) {
                conn.disconnect()
                throw new RuntimeException("HTTP ${code}")
            }
            try {
                return conn.inputStream.bytes
            } finally {
                conn.disconnect()
            }
        }
        throw new RuntimeException('too many redirects')
    }
}
