// SPDX-License-Identifier: CC0-1.0

import java.security.MessageDigest

import groovy.json.JsonSlurper

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.MapProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.OutputFile
import org.gradle.api.tasks.TaskAction

/**
 * Fetches an APK from the first source that answers, and keeps it only if it is
 * one of the copies the caller knows — a list rather than one hash, because a
 * mirror's archive of a release is not byte for byte the publisher's.
 *
 * <p>A file that is already there is verified and left alone; one that is there
 * and is not known is an error rather than something to download over.
 */
abstract class DownloadVncApk extends DefaultTask {

    /** Sources in order, each {@code [kind: …, url: …]}. */
    @Input
    abstract ListProperty<Map<String, String>> getSources()

    /** SHA-256 → where that copy came from. */
    @Input
    abstract MapProperty<String, String> getKnownCopies()

    @OutputFile
    abstract RegularFileProperty getOutputFile()

    /** Some hosts refuse anything that is not a browser. */
    private static final String AGENT = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 ' +
            '(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36'

    @TaskAction
    void download() {
        final target = outputFile.get().asFile
        final known = knownCopies.get()

        if (target.isFile()) {
            final got = sha256(target)
            if (known.containsKey(got)) {
                logger.lifecycle("${target.name} is already here — ${known[got]}")
                return
            }
            throw new GradleException("${target} is not a copy this build knows (${got}). " +
                    'Move it aside rather than downloading over it — a file that is there is ' +
                    'a file somebody put there — or, if you trust it, let the extract task ' +
                    'check the libraries inside it instead.')
        }
        target.parentFile.mkdirs()

        final failures = []
        for (Map<String, String> source : sources.get()) {
            try {
                final direct = resolve(source)
                logger.lifecycle("fetching ${direct}")
                final temp = new File(target.parentFile, target.name + '.part')
                // Read outside the closure: a decorated task's private methods
                // are not visible to a closure's dispatch.
                final bytes = read(direct, '*/*')
                temp.withOutputStream { out -> out.write(bytes) }
                final got = sha256(temp)
                if (!known.containsKey(got)) {
                    temp.delete()
                    failures << "${source.url}: fetched an unknown copy (${got})"
                    continue
                }
                if (!temp.renameTo(target)) {
                    throw new GradleException("cannot move ${temp} to ${target}")
                }
                logger.lifecycle("${target.name} verified — ${known[got]}")
                return
            } catch (Exception e) {
                failures << "${source.url}: ${e.message}"
            }
        }

        throw new GradleException([
                "could not fetch ${target.name}. Every source failed:",
                '',
                *failures.collect { "  ${it}" },
                '',
                "The download is a convenience and its sources are somebody else's servers.",
                'Put a copy of the APK at',
                '',
                "  ${target}",
                '',
                'by hand — from a device that has the app installed, `adb shell pm path <package>`',
                'names the file and `adb pull` takes it — and the build will check the libraries',
                'inside it and carry on.',
        ].join('\n'))
    }

    /** Turns a source into the URL of the file itself. */
    String resolve(Map<String, String> source) {
        switch (source.kind) {
            case 'direct':
                return source.url

            // Ask for one version code; it answers with the path to that build.
            case 'aptoide-api':
                final meta = new JsonSlurper().parse(read(source.url, 'application/json'))
                final path = meta?.data?.file?.path
                if (!path) {
                    throw new RuntimeException("no file path in the API's answer " +
                            "(status ${meta?.info?.status})")
                }
                return path

            default:
                throw new RuntimeException("unknown source kind ${source.kind}")
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
    byte[] read(String url, String accept) {
        def location = url
        for (int hop = 0; hop < 5; hop++) {
            final conn = (HttpURLConnection) new URI(location).toURL().openConnection()
            conn.setRequestProperty('User-Agent', AGENT)
            conn.setRequestProperty('Accept', accept)
            conn.instanceFollowRedirects = false
            conn.connectTimeout = 30_000
            conn.readTimeout = 120_000
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
