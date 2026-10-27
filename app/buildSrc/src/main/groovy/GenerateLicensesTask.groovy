// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

import groovy.json.JsonSlurper

import javax.inject.Inject

import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.Project
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.provider.SetProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.Optional
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

import java.util.regex.Pattern

/**
 * The app's licence page, as an asset: what this build is made of, and what that
 * means for redistributing it.
 *
 * <p>The first-party modules come from the caller; the Rust crates are resolved
 * from Cargo, because a hand-written list goes stale as soon as a dependency
 * changes. Per <em>target</em>, since the crates in the graph depend on the
 * platform, and a crate reached by more than one ABI is listed once. The
 * workspace's own members are excluded: they are this project.
 *
 * <p>What is listed is what the artefact's libraries are compiled from, and
 * nothing that only builds them: see {@link #resolveCrates}.
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
     * each. The Cargo lock covers everything under a Rust protocol module and a
     * C module's adapter crate, but not the C library a C module builds or
     * anything in the app's own dependency block, so libraries vendored as
     * submodules and libraries resolved from Maven have to be listed here.
     */
    @Input
    abstract ListProperty<Map<String, String>> getOtherComponents()

    /**
     * The Cargo workspace whose dependencies are listed.
     */
    @Internal
    abstract DirectoryProperty getWorkspaceDir()

    /**
     * Which of the workspace's members this artefact actually contains, by
     * package name. Every member when it is empty.
     *
     * <p>The workspace holds one crate per protocol and an artefact holds only
     * the protocols it ships, which differ once a backend ships in an add-on:
     * a page listing the whole workspace would claim the app contains code
     * that is in a different APK under a different licence.
     */
    @Input
    @Optional
    abstract SetProperty<String> getCargoPackages()

    /** What the first section is called; an add-on is not "this app". */
    @Input
    @Optional
    abstract Property<String> getOwnHeading()

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

        asset.setText(renderHtml(ownHeading.getOrElse('This app'), firstParty.get(),
                otherComponents.get(), crates.values().toList()), 'UTF-8')
        logger.lifecycle("described ${firstParty.get().size()} modules, " +
                "${otherComponents.get().size()} other components and " +
                "${crates.size()} rust crates in ${ASSET}")
    }

    /**
     * The crates compiled into the libraries built from the wanted members for
     * {@code target}, excluding the workspace's own members.
     *
     * <p>That is {@code cargo tree -e normal}, once per member, rather than a
     * walk of {@code cargo metadata}'s resolve graph, whose features are
     * unified across every member and every kind of dependency at once: a
     * dev-dependency on rustls with its default features (in ryll's protocol
     * crate) enabled rustls's {@code aws_lc_rs} there, and aws-lc-rs and
     * aws-lc-sys were listed although no library contains them.
     * {@code cargo tree} resolves features as {@code cargo build} does for the
     * same package and target (no dev-dependencies, nothing for other
     * platforms), and it is run for each member alone because that is how each
     * library is built ({@code CargoBuildTask}, one {@code --package} each), so
     * one member's features do not leak into another's page either.
     *
     * <p>Two kinds of crate run during the build but are not part of its output,
     * and are left out. Build-dependencies (bindgen, protobuf-codegen, cc and
     * their dependencies): build script output is generated from inputs this
     * page already covers (the C headers of the libraries listed beside the
     * crates, RustDesk's message definitions), and the runtime the generated
     * code calls into, such as {@code protobuf}, is a normal dependency and is
     * listed. And the dependencies of proc-macro crates (syn, quote,
     * proc-macro2), which run inside the compiler. The proc-macro crates
     * themselves are kept: the code a derive generates is theirs, and it is in
     * the library.
     */
    List<Map> resolveCrates(String target) {
        final metadata = new JsonSlurper().parseText(cargoMetadata(target)) as Map

        final packages = [:]
        (metadata.packages as List).each { packages.put(it.id, it) }
        final byNameVersion = [:].withDefault { [] }
        packages.values().each { byNameVersion.get([it.name.toString(), it.version.toString()]).add(it.id) }

        // A virtual manifest, as for a workspace of several cdylibs, has no
        // root package; a manifest with one is still allowed, and then it is
        // the only root.
        final members = new HashSet<>(metadata.workspace_members as List)
        final root = (metadata.resolve as Map).root
        def roots = root != null ? [root] : (metadata.workspace_default_members ?: members).toList()
        final wanted = cargoPackages.getOrElse([] as Set)
        if (!wanted.isEmpty()) {
            // Looked up among all members, not only the default members used as
            // roots otherwise: a crate left out of the root manifest's
            // `default-members` can still ship in an APK.
            final byName = members.collectEntries { [(packages.get(it).name.toString()): it] }
            final missing = wanted - byName.keySet()
            if (!missing.isEmpty()) {
                throw new GradleException("${missing} is not a member of the cargo workspace")
            }
            roots = wanted.toSorted().collect { byName.get(it) }
        }
        if (roots.isEmpty()) {
            throw new GradleException(
                    "cargo metadata resolved no packages for ${workspaceDir.get().asFile}")
        }

        final seen = new TreeSet<String>()
        roots.collect { packages.get(it).name.toString() }.toSorted().each { member ->
            compiledInto(cargoTree(target, member)).each { nameVersion ->
                final ids = byNameVersion.get(nameVersion)
                if (ids.size() != 1) {
                    // The tree names a package by name and version, which is
                    // unique unless one crate comes from two sources.
                    throw new GradleException("cargo tree's ${nameVersion.join(' ')} is " +
                            "${ids.size()} packages of cargo metadata's for ${target}")
                }
                seen.add(ids[0].toString())
            }
        }

        return seen.findAll { !members.contains(it) }.collect { describeCrate(packages.get(it) as Map) }
    }

    /** One line of {@code cargo tree --prefix depth --format {p}}. */
    private static final Pattern TREE_LINE =
            ~/^(\d+)(\S+) v(\S+)(?: \([^)]*\))*$/

    /**
     * The {@code [name, version]} of each package in a {@code cargo tree}
     * that is compiled for the target: everything but what is below a
     * proc-macro crate, which is compiled for the host to run in the compiler.
     *
     * <p>A package the tree has already shown is shown again as {@code (*)}
     * without its dependencies. That loses nothing here, since the tree
     * distinguishes a crate built for the target from the same crate built for
     * the host, and shows each one's dependencies at its first appearance.
     */
    static Set<List<String>> compiledInto(String tree) {
        final compiled = new LinkedHashSet<List<String>>()
        // The depth of the proc-macro crate the current line is below, if any.
        int hostBelow = -1
        tree.readLines().each { line ->
            if (line.isEmpty()) {
                return
            }
            final m = TREE_LINE.matcher(line)
            if (!m.matches()) {
                throw new GradleException("could not parse a line of cargo tree: ${line}")
            }
            final depth = m.group(1) as int
            if (hostBelow >= 0 && depth <= hostBelow) {
                hostBelow = -1
            }
            if (hostBelow >= 0) {
                return
            }
            compiled.add([m.group(2), m.group(3)])
            if (line.contains(' (proc-macro)')) {
                hostBelow = depth
            }
        }
        if (compiled.isEmpty()) {
            throw new GradleException('cargo tree printed no packages')
        }
        return compiled
    }

    String cargoMetadata(String target) {
        return cargo([
                'metadata',
                '--locked',
                '--format-version', '1',
                '--filter-platform', target,
        ])
    }

    String cargoTree(String target, String member) {
        return cargo([
                'tree',
                '--locked',
                '--edges', 'normal',
                '--target', target,
                '--package', member,
                '--prefix', 'depth',
                '--format', '{p}',
                '--color', 'never',
                '--charset', 'ascii',
        ])
    }

    String cargo(List<String> args) {
        final out = new ByteArrayOutputStream()
        final cmd = ['cargo'] + args + [
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

    static String renderHtml(String ownHeading, List<Map<String, String>> modules,
                             List<Map<String, String>> others, List<Map> crates) {
        final body = new StringBuilder()
        body << "<h2>${htmlEscape(ownHeading)}</h2>\n"
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
${body}</body>
</html>
"""
    }

    static String htmlEscape(String s) {
        s.replace('&', '&amp;').replace('<', '&lt;').replace('>', '&gt;')
    }

    /**
     * The text of a licence file in a submodule or a fetched source tree, by
     * its path from the repository root, for an entry's {@code text}.
     *
     * <p>Call it from a provider, so the file is read when the page is built
     * rather than when the project is configured: a clone without submodules,
     * such as CI's for the control module alone, configures every project, and
     * none of these files is there. When the page is built without them, the
     * error says how to get them rather than being a FileNotFoundException.
     */
    static String licenceText(Project project, String path) {
        final File f = project.rootProject.file(path)
        if (!f.isFile()) {
            throw new GradleException(
                    "${path} is missing. Run `git submodule update --init` and, for "
                            + "the gnutls and nettle trees, `./gradlew :backends:fetchCryptoSources`.")
        }
        f.getText('UTF-8')
    }
}
