// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import groovy.json.JsonSlurper

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
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
 * The app's licence page, as an asset: what this build is made of, and what that
 * means for handing it on.
 *
 * <p>The first-party modules come from the caller; the Rust
 * crates are resolved from {@code cargo metadata}, because a hand-written list
 * is wrong the first time a dependency moves. Per <em>target</em>, since which
 * crates are in the graph is platform-dependent, and a crate reached by more
 * than one ABI is listed once. The workspace's own members are excluded: they
 * are this project.
 */
abstract class GenerateLicensesTask extends DefaultTask {

    /** Where the page lands in the APK's assets. */
    static final String ASSET = 'licenses/index.html'

    /** Rust target triples for the Android ABIs; matches cargo-ndk. */
    private static final Map<String, String> ABI_TARGETS = [
            'arm64-v8a'  : 'aarch64-linux-android',
            'armeabi-v7a': 'armv7-linux-androideabi',
            'x86'        : 'i686-linux-android',
            'x86_64'     : 'x86_64-linux-android',
    ]

    /** Files crates conventionally ship their licence terms in. */
    private static final Pattern LICENSE_FILE =
            ~/(?i)^(licen[sc]e|copying|copyright|notice|unlicen[sc]e)([-._].*)?$/

    @InputFile
    @PathSensitive(PathSensitivity.NONE)
    abstract RegularFileProperty getCargoLock()

    @Input
    abstract SetProperty<String> getAbiFilters()

    /** This project's own modules: {@code [name, spdx, text]} each. */
    @Input
    abstract ListProperty<Map<String, String>> getFirstParty()

    /**
     * Third-party components that are not crates: {@code [name, spdx, text]}
     * each. The Cargo lock accounts for everything under a Rust protocol module
     * and nothing at all under a CMake one or in the app's own dependency
     * block, so a library vendored as a submodule and a library resolved from
     * Maven both have to be named here or the page would not know the build
     * contains them.
     */
    @Input
    abstract ListProperty<Map<String, String>> getOtherComponents()

    /**
     * The Cargo workspace whose dependencies are listed. Every member of it is
     * a root here, because what is packaged is one shared library per protocol
     * and the workspace is exactly the set of them: a protocol added or deleted
     * is a directory, and this page follows it with nothing to edit.
     */
    @Internal
    abstract DirectoryProperty getWorkspaceDir()

    @OutputDirectory
    abstract DirectoryProperty getAssetsOutputDir()

    @Inject
    abstract ExecOperations getExecOperations()

    @TaskAction
    void generate() {
        final abis = abiFilters.get()
        if (abis.isEmpty()) {
            throw new GradleException("${name}: no ABIs to resolve crates for")
        }

        final assetsRoot = assetsOutputDir.get().asFile
        assetsRoot.deleteDir()
        final asset = new File(assetsRoot, ASSET)
        asset.parentFile.mkdirs()

        final crates = new TreeMap<List<String>, Map>({ List<String> a, List<String> b ->
            final order = a[0] <=> b[0]
            order != 0 ? order : a[1] <=> b[1]
        } as Comparator)
        abis.toSorted().each { abi ->
            final target = ABI_TARGETS.get(abi)
            if (target == null) {
                throw new GradleException("unknown rust target for abi ${abi}")
            }
            resolveCrates(target).each { crate ->
                crates.putIfAbsent(
                        [crate.name.toString().toLowerCase(Locale.ROOT), crate.version.toString()],
                        crate)
            }
        }

        asset.setText(renderHtml(firstParty.get(), otherComponents.get(),
                crates.values().toList()), 'UTF-8')
        logger.lifecycle("described ${firstParty.get().size()} modules, " +
                "${otherComponents.get().size()} other components and " +
                "${crates.size()} rust crates in ${ASSET}")
    }

    /**
     * The normal (not dev, not build) dependencies of the crate, excluding the
     * workspace's own members.
     */
    List<Map> resolveCrates(String target) {
        final metadata = new JsonSlurper().parseText(cargoMetadata(target)) as Map

        final packages = [:]
        (metadata.packages as List).each { packages.put(it.id, it) }

        final nodes = [:]
        ((metadata.resolve as Map).nodes as List).each { nodes.put(it.id, it) }

        // A virtual manifest has no root package, which is what a workspace of
        // several cdylibs is; a manifest that does have one is still allowed,
        // and then it is the only root.
        final members = new HashSet<>(metadata.workspace_members as List)
        final root = (metadata.resolve as Map).root
        final roots = root != null ? [root] : (metadata.workspace_default_members ?: members).toList()
        if (roots.isEmpty()) {
            throw new GradleException(
                    "cargo metadata resolved no packages for ${workspaceDir.get().asFile}")
        }

        final seen = new HashSet<String>()
        final pending = roots as LinkedList
        while (!pending.isEmpty()) {
            final id = pending.poll()
            if (!seen.add(id.toString())) {
                continue
            }
            final node = nodes.get(id)
            if (node == null) {
                throw new GradleException("cargo metadata is missing a resolve node for ${id}")
            }
            (node.deps as List).each { dep ->
                final kinds = dep.dep_kinds as List
                if (kinds == null || kinds.isEmpty() || kinds.any { it.kind == null }) {
                    pending.add(dep.pkg)
                }
            }
        }

        return seen.findAll { !members.contains(it) }.collect { describeCrate(packages.get(it) as Map) }
    }

    String cargoMetadata(String target) {
        final out = new ByteArrayOutputStream()
        final cmd = [
                'cargo', 'metadata',
                '--locked',
                '--format-version', '1',
                '--filter-platform', target,
                '--manifest-path', new File(workspaceDir.get().asFile, 'Cargo.toml').absolutePath,
        ]
        try {
            execOperations.exec {
                it.commandLine = cmd
                it.standardOutput = out
                it.environment = System.getenv() + [
                        'CARGO_TERM_PROGRESS_WHEN': 'never',
                        'CARGO_TERM_COLOR'        : 'never',
                ]
            }
        } catch (Exception ex) {
            throw new GradleException("failed to run ${cmd.join(' ')}: ${ex}", ex)
        }
        return out.toString('UTF-8')
    }

    static Map describeCrate(Map pkg) {
        if (pkg == null) {
            throw new GradleException('cargo metadata is missing a resolved package')
        }
        final dir = new File(pkg.manifest_path.toString()).parentFile

        final files = new TreeMap<String, File>()
        dir.listFiles()?.each { file ->
            if (file.isFile() && LICENSE_FILE.matcher(file.name).matches()) {
                files.put(file.name, file)
            }
        }
        // A crate can point at a licence file with a name we don't recognise.
        if (pkg.license_file != null) {
            final file = new File(dir, pkg.license_file.toString())
            if (file.isFile()) {
                files.put(file.name, file)
            }
        }

        return [
                name   : pkg.name,
                version: pkg.version,
                license: pkg.license,
                files  : files.collect { fileName, file -> [name: fileName, text: file.getText('UTF-8')] },
        ]
    }

    static String renderHtml(List<Map<String, String>> modules, List<Map<String, String>> others,
                             List<Map> crates) {
        final body = new StringBuilder()
        body << "<h2>This app</h2>\n"
        modules.each { module ->
            body << "<details>\n<summary>${htmlEscape(module.name)}</summary>\n"
            body << "<div class=\"src\">${htmlEscape(module.spdx)}</div>\n"
            body << "<pre>${htmlEscape(module.text)}</pre>\n</details>\n"
        }
        body << "<h2>Third-party</h2>\n"
        others.each { component ->
            body << "<details>\n<summary>${htmlEscape(component.name)}</summary>\n"
            body << "<div class=\"src\">${htmlEscape(component.spdx)}</div>\n"
            body << "<pre>${htmlEscape(component.text)}</pre>\n</details>\n"
        }
        crates.each { crate ->
            body << "<details>\n<summary>${htmlEscape("${crate.name} ${crate.version}".toString())}</summary>\n"
            if (crate.license != null) {
                body << "<div class=\"src\">${htmlEscape(crate.license.toString())}</div>\n"
            }
            final files = crate.files as List
            files.each { file ->
                if (files.size() > 1) {
                    body << "<div class=\"fname\">${htmlEscape(file.name.toString())}</div>\n"
                }
                body << "<pre>${htmlEscape(file.text.toString())}</pre>\n"
            }
            body << "</details>\n"
        }

        final css = '''
:root { color-scheme: light dark; }
body { font-family: sans-serif; margin: 0; padding: 28px 16px 40px; line-height: 1.5;
       background: #ffffff; color: #202124; }
h1 { font-size: 1.3rem; margin: 0 0 4px; }
h2 { font-size: 1rem; margin: 26px 0 0; }
.intro { opacity: .7; margin: 0 0 16px; }
details { margin: 10px 0; }
summary { padding: 16px; font-size: 1.05rem; font-weight: 600; cursor: pointer;
          border-radius: 16px; background: rgba(128,128,128,.14); min-height: 24px; }
.src { font-size: .82rem; margin: 10px 2px 0; overflow-wrap: anywhere; }
.fname { font-family: monospace; font-size: .8rem; opacity: .7; margin: 12px 2px 0; }
pre { white-space: pre-wrap; overflow-wrap: anywhere; font-size: .72rem; margin: 8px 0 0;
      padding: 12px; border: 1px solid rgba(128,128,128,.35); border-radius: 8px;
      background: rgba(128,128,128,.08); }
a { color: inherit; }
@media (prefers-color-scheme: dark) {
  body { background: #121212; color: #e3e3e3; }
}
'''

        """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>Licences</title>
<style>${css}</style>
</head>
<body>
<h1>Licences</h1>
<p class="intro">What this build is made of.</p>
${body}</body>
</html>
"""
    }

    static String htmlEscape(String s) {
        s.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')
    }
}
