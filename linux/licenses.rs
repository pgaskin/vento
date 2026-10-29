// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `LICENSE`, beside the binary: every third-party component compiled or
//! linked into the binary and the backends' libraries, which of those files
//! has it, its licence, and its licence and notice texts, each distinct text
//! printed once and referred to by number. `linux/Dockerfile` ships it in the
//! release tree.
//!
//! What is listed is found rather than written down, where the build can say:
//!
//! - **Crates**: `cargo tree -e normal` for the target and the features this
//!   build has, which resolves features as this build does (no
//!   dev-dependencies, nothing for other platforms), with `cargo metadata` for
//!   each package's licence and directory. Build-dependencies and the
//!   dependencies of proc-macro crates run in the build and are not in its
//!   output; the proc-macro crates themselves are kept, since the code a
//!   derive generates is theirs. This is the Android licence page's rule
//!   (`GenerateLicensesTask.resolveCrates`). A crate's texts are the licence
//!   files at its root, plus `CRATE_FILES`; a licence file deeper in the
//!   package that is not in `CRATE_FILES` or `CRATE_FILES_SKIPPED` fails the
//!   build. A crate without one gets the standard text of a licence its
//!   manifest names, from the Rust toolchain's `share/doc/rust/licenses/`.
//! - **The backends' C and C++ libraries**: the static archives on each
//!   `libvento_*.so`'s link line in the Meson tree's `build.ninja`, and the
//!   sources of the Meson targets that made them (`meson-info`). Each
//!   `third_party/<dir>` compiled into a library, and each port built by its
//!   own build system (a custom target), has to be in `VENDORED`, or the build
//!   fails. An archive from outside the tree has to be FFmpeg's, GCC's
//!   libatomic, or an empty compatibility archive of the C library.
//! - **FFmpeg's licence**: the one its configure reports (`FFMPEG_LICENSE` in
//!   its `config.h`), which has to be LGPL 2.1 or later.
//! - **The Rust standard library**: the toolchain's
//!   `share/doc/rust/COPYRIGHT-library.html`.
//!
//! The rest is listed here by hand: the components of the binary that are not
//! crates (FFmpeg, dav1d, libva's headers, RustDesk's message definitions,
//! the toolbar's icon font) and each vendored library's licence files and
//! copyright summary.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The name of the file, beside the binary.
pub const FILE: &str = "LICENSE";

/// Where the source of each release is: the tag `v<n>` for version `0.0.<n>`.
const SOURCE: &str = "https://github.com/pgaskin/vento";

/// A library under `third_party/` that the Meson build compiles into a
/// backend's library: a directory the sources of a Meson target are in, or a
/// port built by its own build system, by its custom target's name.
struct Vendored {
    key: &'static str,
    name: &'static str,
    licence: &'static str,
    /// Copyright holders, as upstream lists them.
    copyright: &'static str,
    note: &'static str,
    /// Licence and notice files, from `third_party/<key>/`.
    files: &'static [&'static str],
    /// Source files compiled into the library whose own notice is under
    /// different terms from `files`: their leading comment is included.
    headers: &'static [&'static str],
}

const VENDORED: &[Vendored] = &[
    Vendored {
        key: "libvncserver",
        name: "LibVNCServer (LibVNCClient)",
        licence: "GPL-2.0-or-later",
        copyright: "Copyright (C) 2001-2003 Johannes E. Schindelin, and the authors listed in AUTHORS.",
        note: "Its copy of the TurboJPEG wrapper, src/common/turbojpeg.c, is BSD-3-Clause.",
        files: &["COPYING", "AUTHORS"],
        headers: &["src/common/turbojpeg.c"],
    },
    Vendored {
        key: "tigervnc",
        name: "TigerVNC",
        licence: "GPL-2.0-or-later",
        copyright: "Copyright (C) 1999 AT&T Laboratories Cambridge, (C) 2002-2005 RealVNC Ltd., \
                    Pierre Ossman for Cendio AB, and others, as listed in README.rst and the source \
                    files.",
        note: "",
        files: &["LICENCE.TXT"],
        headers: &[],
    },
    Vendored {
        key: "libjpeg-turbo",
        name: "libjpeg-turbo",
        licence: "IJG AND BSD-3-Clause AND Zlib",
        copyright: "Copyright Thomas G. Lane, Guido Vollbeding, D. R. Commander and others, as \
                    listed in LICENSE.md and README.ijg.",
        note: "This software is based in part on the work of the Independent JPEG Group.",
        files: &["LICENSE.md", "README.ijg"],
        headers: &[],
    },
    Vendored {
        key: "pixman",
        name: "pixman",
        licence: "MIT",
        copyright: "Copyright: as listed in COPYING.",
        note: "",
        files: &["COPYING"],
        headers: &[],
    },
    Vendored {
        key: "freerdp",
        name: "FreeRDP (with WinPR)",
        licence: "Apache-2.0",
        copyright: "Copyright Marc-Andre Moreau, Thincast Technologies GmbH, and the FreeRDP \
                    contributors, as listed in the source files.",
        note: "",
        files: &["LICENSE"],
        headers: &[],
    },
    Vendored {
        key: "openssl",
        name: "OpenSSL",
        licence: "Apache-2.0",
        copyright: "Copyright (c) 1998-2026 The OpenSSL Project Authors. \
                    Copyright (c) 1995-1998 Eric A. Young, Tim J. Hudson.",
        note: "Built without its tools, engines, loadable modules and legacy algorithms.",
        files: &["LICENSE.txt", "AUTHORS.md"],
        headers: &[],
    },
    Vendored {
        key: "gnutls",
        name: "GnuTLS",
        licence: "LGPL-2.1-or-later",
        copyright: "Copyright Free Software Foundation, Inc., and the authors listed in AUTHORS, \
                    as the source files state.",
        note: "The library alone, as released, with the copies of libtasn1 and parts of \
               gnulib it includes (LGPL-2.1-or-later); its tools (GPL-3.0-or-later) are not built.",
        files: &["COPYING.LESSERv2", "AUTHORS"],
        headers: &[],
    },
    Vendored {
        key: "nettle",
        name: "Nettle",
        licence: "LGPL-3.0-or-later OR GPL-2.0-or-later",
        copyright: "Copyright Niels Möller and the authors listed in AUTHORS.",
        note: "With mini-gmp, from GMP, under the same licences, in place of GMP.",
        files: &["COPYING.LESSERv3", "COPYINGv3", "COPYINGv2", "AUTHORS"],
        headers: &[],
    },
    Vendored {
        key: "logger",
        name: "minecraft-linux logger",
        licence: "Unlicense",
        copyright: "",
        note: "",
        files: &["LICENSE"],
        headers: &[],
    },
    Vendored {
        key: "libc-shim",
        name: "minecraft-linux libc-shim",
        licence: "none stated",
        copyright: "Copyright the libc-shim authors (github.com/minecraft-linux/libc-shim).",
        note: "Upstream states no licence: the repository has no licence file. Two of its files \
               come from Android's C library and carry their own notices, below.",
        files: &[],
        headers: &["src/ctype_data.cpp", "src/bionic/strlcpy.cpp"],
    },
    Vendored {
        key: "mcpelauncher-linker",
        name: "mcpelauncher-linker",
        licence: "MIT AND BSD-2-Clause AND Apache-2.0 AND ISC",
        copyright: "Copyright (c) 2024 ChristopherHX and MCMrARM (its own files); \
                    Copyright The Android Open Source Project (bionic and core).",
        note: "Android's dynamic linker and parts of its C library and core libraries, \
               adapted to run in a glibc process.",
        files: &[
            "LICENSE",
            "bionic/linker/NOTICE",
            "bionic/libdl/NOTICE",
            "core/NOTICE",
            "core/liblog/NOTICE",
        ],
        headers: &[
            "bionic/libc/bionic/bionic_call_ifunc_resolver.cpp",
            "bionic/libc/async_safe/async_safe_log.cpp",
            "bionic/libc/upstream-openbsd/lib/libc/string/strlcpy.c",
            "bionic/libc/upstream-openbsd/lib/libc/string/strlcat.c",
        ],
    },
    Vendored {
        key: "libjnivm",
        name: "libjnivm",
        licence: "MIT",
        copyright: "",
        note: "",
        files: &["LICENSE"],
        headers: &[],
    },
];

/// Licence files in a crate below its root, by crate name and path in the
/// package, and the licence each one is the text of: the third-party code a
/// crate vendors, and fonts.
const CRATE_FILES: &[(&str, &str, &str)] = &[
    ("epaint_default_fonts", "fonts/Hack-Regular.txt", "MIT"),
    ("epaint_default_fonts", "fonts/OFL.txt", "OFL-1.1"),
    ("epaint_default_fonts", "fonts/UFL.txt", "Ubuntu-font-1.0"),
    ("epaint_default_fonts", "fonts/emoji-icon-font-mit-license.txt", "MIT"),
    ("ring", "third_party/fiat/LICENSE", "MIT"),
    ("ring", "src/polyfill/once_cell/LICENSE-APACHE", "Apache-2.0"),
    ("ring", "src/polyfill/once_cell/LICENSE-MIT", "MIT"),
    ("tracing-core", "src/spin/LICENSE", "MIT"),
    ("wayland-protocols", "protocols/COPYING", "MIT"),
    (
        "wayland-protocols-plasma",
        "plasma-wayland-protocols/COPYING.LIB",
        "LGPL-2.1-or-later",
    ),
    ("zstd-sys", "zstd/LICENSE", "BSD-3-Clause"),
];

/// Licence files in a crate below its root that are not part of the build.
const CRATE_FILES_SKIPPED: &[(&str, &str)] = &[
    // zlib and zlib-ng's sources, built only where there is no system zlib;
    // the C backends' Meson build requires one, and this links it.
    ("libz-sys", "src/zlib/LICENSE"),
    ("libz-sys", "src/zlib/contrib/minizip/LICENSE.Info-Zip"),
    ("libz-sys", "src/zlib/contrib/dotzlib/LICENSE_1_0.txt"),
    ("libz-sys", "src/zlib-ng/LICENSE.md"),
    // zstd's other licence (it is BSD-3-Clause OR GPL-2.0-only), not the one
    // taken.
    ("zstd-sys", "zstd/COPYING"),
];

/// What the generator reads, as `build.rs` found it.
pub struct Inputs<'a> {
    /// The repository root.
    pub root: &'a Path,
    /// `linux/`.
    pub manifest: &'a Path,
    /// The Meson build tree of the C backends.
    pub tree: &'a Path,
    /// The backends' libraries this build made, by name (`libvnc`, …), as
    /// Meson's tree has them.
    pub libraries: &'a [(&'a str, PathBuf)],
    /// The FFmpeg prefix (`DEP_VENTO_VIDEO_PREFIX`).
    pub ffmpeg_prefix: &'a Path,
    /// FFmpeg's `config.h` (`DEP_VENTO_VIDEO_CONFIG_H`).
    pub ffmpeg_config: &'a Path,
}

/// One entry of the file.
#[derive(Default)]
struct Component {
    name: String,
    version: String,
    /// The files of the tree it is in: `vento`, `lib/libvento_<name>.so`.
    files: BTreeSet<String>,
    licence: String,
    lines: Vec<String>,
    /// Each text's label and number.
    texts: Vec<(String, usize)>,
}

/// The distinct texts, in the order they were first used.
#[derive(Default)]
struct Texts {
    list: Vec<Text>,
    by_key: HashMap<String, usize>,
}

struct Text {
    first: String,
    body: String,
    uses: usize,
}

impl Texts {
    /// The number of `body`, added if no text with the same words was. `first`
    /// says where it was first seen.
    fn add(&mut self, first: String, body: &str) -> usize {
        let body = tidy(body);
        assert!(!body.is_empty(), "{first} is empty");
        let key = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let n = *self.by_key.entry(key).or_insert_with(|| {
            self.list.push(Text { first, body, uses: 0 });
            self.list.len()
        });
        self.list[n - 1].uses += 1;
        n
    }

    /// The labelled numbers of a file's texts, added as `add` does: the file,
    /// or for a copy of the Apache License 2.0, its terms, which are the same
    /// in every copy, and what is around them where that is more than the
    /// unfilled template of the appendix (a copyright line, a filled-in
    /// appendix, another licence).
    fn file(&mut self, label: &str, first: String, body: &str) -> Vec<(String, usize)> {
        let Some((before, terms, after)) = split_apache(body) else {
            return vec![(label.to_string(), self.add(first, body))];
        };
        let mut out = vec![(label.to_string(), self.add(format!("Apache License 2.0, from {first}"), terms))];
        let template = ["[name of copyright owner]", "{name of copyright owner}"];
        let after = if template.iter().any(|t| after.contains(t)) && after.len() < 2000 {
            ""
        } else {
            after
        };
        let rest = tidy(&format!("{before}\n\n{after}"));
        if !rest.is_empty() {
            out.push((
                format!("{label} (around the terms)"),
                self.add(format!("{first}, around the terms"), &rest),
            ));
        }
        out
    }
}

/// A copy of the Apache License 2.0 split into what is before its title,
/// the title and terms, and what is after them.
fn split_apache(text: &str) -> Option<(&str, &str, &str)> {
    // The title: a line that is "Apache License" alone, and the version on
    // the next one that is not blank.
    let mut start = None;
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n').peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "Apache License" {
            let next = lines.clone().map(str::trim).find(|l| !l.is_empty());
            if next == Some("Version 2.0, January 2004") {
                start = Some(offset);
                break;
            }
        }
        offset += line.len();
    }
    let start = start?;
    let end = start + text[start..].find("END OF TERMS AND CONDITIONS")?;
    let end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    Some((&text[..start], &text[start..end], &text[end..]))
}

/// Writes the file to `dest`.
pub fn generate(inputs: &Inputs, dest: &Path) {
    for path in ["LICENSE", "linux/Cargo.lock", "linux/assets/material-symbols/LICENSE"] {
        println!("cargo:rerun-if-changed={}", inputs.root.join(path).display());
    }
    let sysroot = rust_sysroot();
    let canonical = sysroot.join("share/doc/rust/licenses");
    let mut texts = Texts::default();
    // vento's own licence, the first text.
    texts.add("vento's LICENSE".into(), &read(&inputs.root.join("LICENSE")));

    let mut libraries = Vec::new();
    let (vendored, found) = vendored(inputs, &mut texts, &mut libraries);
    let mut others = binary_components(inputs, &canonical, &mut texts, &found);
    others.extend(vendored);
    let std = standard_library(&sysroot, &canonical, &mut texts);
    let crates = crates(inputs, &canonical, &mut texts);

    let mut out = String::new();
    // The version as `vento --version` shows it, which is also the release's
    // tag: `v` and the package's version, without the `0.0.` of `0.0.N`.
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let tag = format!("v{}", version.strip_prefix("0.0.").unwrap_or(&version));
    out.push_str(&format!(
        "vento {tag}\n\n\
         Copyright 2026 Patrick Gaskin.\n\n\
         vento is free software licensed under the GPLv3 or later.\n\n\
         The complete source of this build, including the third-party sources and\n\
         patches, is available at {SOURCE}, tag {tag}.\n\n\
         This file lists the third-party components compiled into vento and\n\
         libvento_*.so, with each one's licence, copyright and notices. Each unique\n\
         licence/notice is numbered and included once, at the end of this file.\n\n\
         The RealVNC backend requires proprietary libraries which are not included in\n\
         this build and must be downloaded or provided by the user.\n\n"
    ));

    out.push_str("Contents\n========\n\n");
    let mut bin: Vec<String> = others
        .iter()
        .filter(|c| c.files.contains("vento"))
        .map(|c| c.name.clone())
        .collect();
    bin.push(format!(
        "the Rust standard library and {} crates (\"Rust crates\")",
        crates.len()
    ));
    out.push_str(&wrap(&format!("vento: {}.", bin.join(", ")), "", "  "));
    for file in &libraries {
        let names: Vec<String> = others
            .iter()
            .filter(|c| c.files.contains(file))
            .map(|c| c.name.clone())
            .collect();
        out.push_str(&wrap(&format!("{file}: {}.", names.join(", ")), "", "  "));
    }
    out.push('\n');

    out.push_str("Components\n==========\n\n");
    for c in &others {
        render(&mut out, c);
    }
    out.push_str("The Rust standard library\n=========================\n\n");
    for c in &std {
        render(&mut out, c);
    }
    out.push_str("Rust crates\n===========\n\n");
    for c in &crates {
        render(&mut out, c);
    }

    out.push_str("Texts\n=====\n");
    for (i, text) in texts.list.iter().enumerate() {
        let also = match text.uses {
            1 => String::new(),
            2 => ", also given for 1 other".into(),
            n => format!(", also given for {} others", n - 1),
        };
        out.push_str(&format!("\n[{}] {}{also}\n", i + 1, text.first));
        out.push_str(&"-".repeat(72));
        out.push_str("\n\n");
        out.push_str(&text.body);
        out.push('\n');
    }

    let tmp = dest.with_extension("tmp");
    fs::write(&tmp, &out).unwrap_or_else(|e| panic!("{}: {e}", tmp.display()));
    fs::rename(&tmp, dest).unwrap_or_else(|e| panic!("{}: {e}", dest.display()));
}

/// `text` wrapped at 80 columns, the first line after `first`, the rest
/// after `rest`.
fn wrap(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    let mut line = first.to_string();
    let mut empty = true;
    for word in text.split(' ').filter(|w| !w.is_empty()) {
        if !empty && line.chars().count() + 1 + word.chars().count() > 80 {
            out.push_str(&line);
            out.push('\n');
            line = rest.to_string();
            empty = true;
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    out.push_str(&line);
    out.push('\n');
    out
}

fn render(out: &mut String, c: &Component) {
    let title = if c.version.is_empty() {
        c.name.clone()
    } else {
        format!("{} {}", c.name, c.version)
    };
    out.push_str(&title);
    out.push('\n');
    if !c.files.is_empty() {
        // The binary first, then the libraries.
        let mut files: Vec<&str> = c.files.iter().map(String::as_str).collect();
        files.sort_by_key(|f| (*f != "vento", *f));
        let files = files.join(", ");
        out.push_str(&wrap(&format!("In: {files}"), "  ", "    "));
    }
    out.push_str(&wrap(&format!("Licence: {}", c.licence), "  ", "    "));
    for line in &c.lines {
        out.push_str(&wrap(line, "  ", "    "));
    }
    let refs: Vec<String> = c.texts.iter().map(|(label, n)| format!("[{n}] {label}")).collect();
    if !refs.is_empty() {
        out.push_str(&wrap(&format!("Texts: {}", refs.join(", ")), "  ", "    "));
    }
    out.push('\n');
}

/// The vendored libraries in the backends' libraries, from the link lines
/// and the Meson targets. `libraries` gets the files, in order.
fn vendored(
    inputs: &Inputs,
    texts: &mut Texts,
    libraries: &mut Vec<String>,
) -> (Vec<Component>, BTreeMap<String, BTreeSet<String>>) {
    let targets = meson_targets(inputs.tree);
    let ninja = read(&inputs.tree.join("build.ninja"));
    let tp = inputs.root.join("third_party");
    let backends = inputs.root.join("backends");
    let ffmpeg_lib = canonical(&inputs.ffmpeg_prefix.join("lib"));

    // Component key -> the files it is in.
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, path) in inputs.libraries {
        let file = format!("lib/libvento_{name}.so");
        libraries.push(file.clone());
        let rel = path
            .strip_prefix(inputs.tree)
            .expect("the library is in the Meson tree");
        for archive in link_archives(&ninja, &rel.to_string_lossy()) {
            let archive = if Path::new(&archive).is_absolute() {
                PathBuf::from(&archive)
            } else {
                inputs.tree.join(&archive)
            };
            let archive = canonical(&archive);
            let mut add = |key: &str| {
                found.entry(key.to_string()).or_default().insert(file.clone());
            };
            if let Some(target) = targets.get(&archive) {
                if target.custom {
                    add(&target.name);
                    continue;
                }
                for source in &target.sources {
                    if let Ok(rel) = source.strip_prefix(&tp) {
                        let dir = rel.components().next().unwrap().as_os_str().to_string_lossy();
                        add(&dir);
                    } else if source.starts_with(&backends)
                        || source.starts_with(inputs.tree)
                        || !source.starts_with(inputs.root)
                    {
                        // Ours, or generated in the build tree from the
                        // target's other sources.
                    } else {
                        panic!(
                            "{}: source {} of {} is neither vendored nor ours",
                            FILE,
                            source.display(),
                            target.name
                        );
                    }
                }
            } else if archive.parent() == Some(ffmpeg_lib.as_path()) {
                let base = archive.file_name().unwrap().to_string_lossy();
                match base.as_ref() {
                    "libavcodec.a" | "libavutil.a" | "libswscale.a" | "libswresample.a" => {
                        add("ffmpeg");
                        // libavutil's VAAPI code is compiled against libva's
                        // headers.
                        add("libva");
                    }
                    "libdav1d.a" => add("dav1d"),
                    "libvento_va.a" => {}
                    _ => panic!("{FILE}: {} is not known to be FFmpeg's", archive.display()),
                }
            } else if archive.file_name().is_some_and(|n| n == "libatomic.a") {
                add("libatomic");
            } else if fs::read(&archive).is_ok_and(|bytes| bytes == b"!<arch>\n") {
                // An empty archive, as the C library ships for libdl, librt
                // and libpthread since their functions moved into libc.
            } else {
                panic!(
                    "{FILE}: {} is linked into {file} and is not known; list it in linux/licenses.rs",
                    archive.display()
                );
            }
        }
    }

    let mut out = Vec::new();
    for v in VENDORED {
        let Some(files) = found.remove(v.key) else { continue };
        let dir = tp.join(v.key);
        let mut c = Component {
            name: v.name.into(),
            version: vendored_version(inputs.root, v.key).unwrap_or_default(),
            files,
            licence: v.licence.into(),
            ..Default::default()
        };
        if !v.copyright.is_empty() {
            c.lines.push(v.copyright.into());
        }
        if !v.note.is_empty() {
            c.lines.push(v.note.into());
        }
        for file in v.files {
            c.texts
                .extend(texts.file(file, format!("{file} of {}", v.name), &read(&dir.join(file))));
        }
        for file in v.headers {
            c.texts
                .extend(texts.file(file, format!("{file} of {}", v.name), &leading_comment(&dir.join(file))));
        }
        out.push(c);
    }
    // FFmpeg, dav1d, libva and libatomic are listed with the binary's
    // components, which the first three are also in.
    let elsewhere: BTreeMap<_, _> = ["ffmpeg", "dav1d", "libva", "libatomic"]
        .iter()
        .filter_map(|k| found.remove_entry(*k))
        .collect();
    assert!(
        found.is_empty(),
        "{FILE}: no entry in linux/licenses.rs's VENDORED for {:?}, compiled into {:?}",
        found.keys().collect::<Vec<_>>(),
        found.values().collect::<Vec<_>>()
    );
    (out, elsewhere)
}

/// The components of the binary that are not crates, with the backends'
/// libraries `vendored` found FFmpeg, dav1d, libva's headers and libatomic
/// in.
fn binary_components(
    inputs: &Inputs,
    canonical_dir: &Path,
    texts: &mut Texts,
    found: &BTreeMap<String, BTreeSet<String>>,
) -> Vec<Component> {
    let tp = inputs.root.join("third_party");
    let elsewhere = |key: &str| found.get(key).cloned().unwrap_or_default();
    let mut out = Vec::new();

    // FFmpeg, as its configure reports its licence.
    let config = read(inputs.ffmpeg_config);
    let licence = config
        .lines()
        .find_map(|l| l.strip_prefix("#define FFMPEG_LICENSE "))
        .map(|v| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("{}: no FFMPEG_LICENSE", inputs.ffmpeg_config.display()));
    assert_eq!(
        licence, "LGPL version 2.1 or later",
        "{FILE}: FFmpeg is configured as {licence}, and linux/licenses.rs lists it as LGPL-2.1-or-later"
    );
    let mut files = elsewhere("ffmpeg");
    files.insert("vento".into());
    let mut c = Component {
        name: "FFmpeg".into(),
        version: fs::read_to_string(tp.join("ffmpeg/RELEASE"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default(),
        files,
        licence: "LGPL-2.1-or-later".into(),
        lines: vec![
            "Copyright (c) 2000-2026 the FFmpeg developers.".into(),
            format!("Configured as \"{licence}\": without --enable-gpl, --enable-version3 or --enable-nonfree."),
            "libavcodec, libavutil and libswresample in vento; libswscale and libavutil in the FreeRDP library.".into(),
            "Linked statically: to relink with a modified FFmpeg, build from the source named at the top of this file."
                .into(),
        ],
        ..Default::default()
    };
    for file in ["COPYING.LGPLv2.1", "LICENSE.md"] {
        c.texts
            .extend(texts.file(file, format!("{file} of FFmpeg"), &read(&tp.join("ffmpeg").join(file))));
    }
    out.push(c);

    let mut files = elsewhere("dav1d");
    files.insert("vento".into());
    let dav1d = texts.file("COPYING", "COPYING of dav1d".into(), &read(&tp.join("dav1d/COPYING")));
    out.push(Component {
        name: "dav1d".into(),
        version: between(&read(&tp.join("dav1d/meson.build")), "version: '", "'").unwrap_or_default(),
        files,
        licence: "BSD-2-Clause".into(),
        lines: vec!["Inside FFmpeg's libavcodec, for AV1.".into()],
        texts: dav1d,
    });

    let mut files = elsewhere("libva");
    files.insert("vento".into());
    let libva = texts.file("COPYING", "COPYING of libva".into(), &read(&tp.join("libva/COPYING")));
    out.push(Component {
        name: "libva (headers)".into(),
        version: String::new(),
        files,
        licence: "MIT".into(),
        lines: vec![
            "Copyright (c) 2007-2009 Intel Corporation, as its headers state.".into(),
            "FFmpeg's VAAPI code is compiled against its headers; libva itself is loaded from the system \
             when installed."
                .into(),
        ],
        texts: libva,
    });

    let rustdesk = texts.add("LICENCE of RustDesk".into(), &read(&tp.join("rustdesk/LICENCE")));
    out.push(Component {
        name: "RustDesk (message definitions)".into(),
        version: String::new(),
        files: ["vento".to_string()].into(),
        licence: "AGPL-3.0-only".into(),
        lines: vec![
            "Copyright Purslane Tech Pte. Ltd. and the RustDesk contributors.".into(),
            "libs/base/protos/message.proto of RustDesk's client, compiled into the RustDesk backend.".into(),
        ],
        texts: vec![("LICENCE".into(), rustdesk)],
    });
    out.push(Component {
        name: "RustDesk hbb_common (rendezvous definitions)".into(),
        version: String::new(),
        files: ["vento".to_string()].into(),
        licence: "AGPL-3.0-only, as part of RustDesk: the repository states no licence of its own".into(),
        lines: vec![
            "Copyright Purslane Tech Pte. Ltd. and the RustDesk contributors.".into(),
            "protos/rendezvous.proto, compiled into the RustDesk backend.".into(),
        ],
        texts: vec![("RustDesk's LICENCE".into(), rustdesk)],
    });

    let symbols = texts.file(
        "LICENSE",
        "LICENSE of Material Symbols".into(),
        &read(&inputs.manifest.join("assets/material-symbols/LICENSE")),
    );
    out.push(Component {
        name: "Material Symbols Outlined (font)".into(),
        version: "2.975".into(),
        files: ["vento".to_string()].into(),
        licence: "Apache-2.0".into(),
        lines: vec![
            "Copyright Google LLC.".into(),
            "The toolbar's icons: a static instance of the variable font (unfilled, weight 400, grade 0, \
             optical size 24)."
                .into(),
        ],
        texts: symbols,
    });

    let atomic = elsewhere("libatomic");
    if !atomic.is_empty() {
        // GPL 3, whose copy in this repository is vento's.
        let gpl = texts.add("vento's LICENSE".into(), &read(&inputs.root.join("LICENSE")));
        let exception = texts.add(
            "GCC-exception-3.1.txt of the Rust toolchain".into(),
            &read(&canonical_dir.join("GCC-exception-3.1.txt")),
        );
        out.push(Component {
            name: "GCC's libatomic".into(),
            version: String::new(),
            files: atomic,
            licence: "GPL-3.0-or-later WITH GCC-exception-3.1".into(),
            lines: vec![
                "Copyright (C) Free Software Foundation, Inc.".into(),
                "The compiler's runtime library for atomic operations, linked statically under the GCC \
                 Runtime Library Exception."
                    .into(),
            ],
            texts: vec![("GPL-3.0".into(), gpl), ("GCC Runtime Library Exception 3.1".into(), exception)],
        });
    }
    out
}

/// The Rust standard library, and the crates it is built with, from the
/// toolchain's `COPYRIGHT-library.html`.
fn standard_library(sysroot: &Path, canonical_dir: &Path, texts: &mut Texts) -> Vec<Component> {
    let path = sysroot.join("share/doc/rust/COPYRIGHT-library.html");
    let html = read(&path);
    let in_tree = between(
        &html,
        "<h2 id=\"in-tree-files\">",
        "<h2 id=\"out-of-tree-dependencies\">",
    )
    .unwrap_or_else(|| unreadable(&path));
    let mut std = Component {
        name: "The Rust standard library".into(),
        version: rustc_version(),
        files: ["vento".to_string()].into(),
        licence: "Apache-2.0 OR MIT".into(),
        ..Default::default()
    };
    std.lines.extend(
        strip_tags(&in_tree)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("In-tree files"))
            .map(String::from),
    );
    for id in ["Apache-2.0", "MIT", "Unicode-3.0", "BSD-2-Clause"] {
        let body = read(&canonical_dir.join(format!("{id}.txt")));
        std.texts
            .extend(texts.file(id, format!("{id}.txt of the Rust toolchain"), &body));
    }
    let mut out = vec![std];

    let deps = html
        .split("<h2 id=\"out-of-tree-dependencies\">")
        .nth(1)
        .unwrap_or_else(|| unreadable(&path));
    for section in deps.split("<h3>").skip(1) {
        let title = section.split("</h3>").next().unwrap_or_else(|| unreadable(&path));
        let title = title.trim_start_matches('\u{1F4E6}').trim();
        // `name-version`, where the name can have a `-` and the version can
        // have one after its build metadata (`wasi-0.11.1+wasi-snapshot-preview1`).
        let dash = title
            .match_indices('-')
            .find(|(i, _)| title[i + 1..].starts_with(|c: char| c.is_ascii_digit()))
            .map(|(i, _)| i)
            .unwrap_or_else(|| unreadable(&path));
        let (name, version) = (&title[..dash], &title[dash + 1..]);
        let licence = between(section, "<b>License:</b>", "</p>").unwrap_or_else(|| unreadable(&path));
        let mut c = Component {
            name: name.into(),
            version: version.into(),
            licence: unescape(licence.trim()),
            ..Default::default()
        };
        for notice in section.split("<summary><code>").skip(1) {
            let file = notice.split("</code>").next().unwrap_or_else(|| unreadable(&path));
            let body = between(notice, "<pre>", "</pre>").unwrap_or_else(|| unreadable(&path));
            let file = unescape(file);
            c.texts
                .extend(texts.file(&file, format!("{file} of {name} {version}"), &unescape(&body)));
        }
        out.push(c);
    }
    assert!(out.len() > 1, "{}: no crates", path.display());
    out
}

fn unreadable<T>(path: &Path) -> T {
    panic!("{}: not in the format linux/licenses.rs reads", path.display())
}

/// The crates compiled into the binary.
fn crates(inputs: &Inputs, canonical_dir: &Path, texts: &mut Texts) -> Vec<Component> {
    let target = env::var("TARGET").unwrap();
    let manifest_path = inputs.manifest.join("Cargo.toml");
    let metadata: serde_json::Value = serde_json::from_slice(&cargo(&[
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--filter-platform",
        &target,
        "--manifest-path",
        &manifest_path.to_string_lossy(),
    ]))
    .expect("cargo metadata's output");

    // This build's features, as cargo names them in the environment of a
    // build script (`CARGO_FEATURE_REALVNC_DOWNLOAD` for `realvnc-download`).
    let declared = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == metadata["resolve"]["root"])
        .and_then(|p| p["features"].as_object())
        .expect("the root package's features")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let features: Vec<String> = declared
        .into_iter()
        .filter(|f| env::var_os(format!("CARGO_FEATURE_{}", f.to_uppercase().replace('-', "_"))).is_some())
        .collect();

    let tree = cargo(&[
        "tree",
        "--locked",
        "--edges",
        "normal",
        "--target",
        &target,
        "--no-default-features",
        "--features",
        &features.join(","),
        "--prefix",
        "depth",
        "--format",
        "{p}",
        "--color",
        "never",
        "--charset",
        "ascii",
        "--manifest-path",
        &manifest_path.to_string_lossy(),
    ]);
    let compiled = compiled_into(&String::from_utf8(tree).unwrap());

    let mut packages: HashMap<(String, String), Vec<&serde_json::Value>> = HashMap::new();
    for p in metadata["packages"].as_array().unwrap() {
        let key = (
            p["name"].as_str().unwrap().to_string(),
            p["version"].as_str().unwrap().to_string(),
        );
        packages.entry(key).or_default().push(p);
    }
    let members: BTreeSet<&str> = metadata["workspace_members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    let tp = inputs.root.join("third_party");

    let mut out = Vec::new();
    for key in compiled {
        let found = packages.get(&key).map(Vec::as_slice).unwrap_or_default();
        let [p] = found else {
            panic!(
                "cargo tree's {} {} is {} packages of cargo metadata's",
                key.0,
                key.1,
                found.len()
            )
        };
        if members.contains(p["id"].as_str().unwrap()) {
            continue;
        }
        let dir = PathBuf::from(p["manifest_path"].as_str().unwrap())
            .parent()
            .unwrap()
            .to_path_buf();
        let licence = p["license"].as_str().unwrap_or("").to_string();
        let path_dep = p["source"].is_null();
        if path_dep && dir.starts_with(inputs.root) && !dir.starts_with(&tp) {
            // This repository's own crates.
            assert_eq!(
                licence, "GPL-3.0-or-later",
                "{FILE}: {} is ours and not GPL-3.0-or-later",
                key.0
            );
            continue;
        }
        let mut c = Component {
            name: key.0.clone(),
            version: key.1.clone(),
            licence: if licence.is_empty() {
                "NOASSERTION".into()
            } else {
                licence.clone()
            },
            ..Default::default()
        };
        if path_dep {
            c.lines.push(format!(
                "From {}, in place of the crates.io release.",
                dir.strip_prefix(inputs.root).unwrap().display()
            ));
        }

        let mut files = licence_files(&dir);
        if let Some(file) = p["license_file"].as_str() {
            let path = dir.join(file);
            if path.is_file() && !files.iter().any(|f| f == &path) {
                files.push(path);
            }
        }
        // A crate of a submodule's workspace has its licence at the
        // repository's root.
        let mut up = dir.clone();
        while files.is_empty() && path_dep && up.starts_with(&tp) && up.parent() != Some(tp.as_path()) {
            up = up.parent().unwrap().to_path_buf();
            files = licence_files(&up);
        }
        let mut covered = Vec::new();
        for (krate, rel, id) in CRATE_FILES {
            if *krate == key.0 {
                let path = dir.join(rel);
                assert!(path.is_file(), "{FILE}: {} {} has no {rel}", key.0, key.1);
                files.push(path);
                covered.push(*id);
            }
        }
        check_nested_licence_files(&key, &dir);
        let has_root_file = files
            .iter()
            .any(|f| f.parent() == Some(dir.as_path()) || !f.starts_with(&dir));
        for file in &files {
            // A file of the crate by its path there; one from the root of a
            // submodule by its path in the repository.
            let label = file
                .strip_prefix(&dir)
                .or_else(|_| file.strip_prefix(inputs.root))
                .unwrap_or(file);
            let label = label.display().to_string();
            c.texts
                .extend(texts.file(&label, format!("{label} of {} {}", key.0, key.1), &read(file)));
        }
        if !has_root_file {
            // No licence file of its own: the standard text of the licences
            // its manifest names, choosing among alternatives.
            let ids = choose(&licence, &covered, canonical_dir).unwrap_or_else(|| {
                panic!(
                    "{FILE}: {} {} has no licence file, and no standard text is at hand for its licence \
                     ({licence:?}); add its files to CRATE_FILES in linux/licenses.rs",
                    key.0, key.1
                )
            });
            for id in ids {
                if covered.contains(&id.as_str()) {
                    continue;
                }
                c.texts.extend(texts.file(
                    &format!("{id} (the package has no licence file)"),
                    format!("{id}.txt of the Rust toolchain"),
                    &read(&canonical_dir.join(format!("{id}.txt"))),
                ));
            }
        }
        out.push(c);
    }
    out.sort_by(|a, b| (a.name.to_lowercase(), &a.version).cmp(&(b.name.to_lowercase(), &b.version)));
    out
}

/// The `[name, version]` of each package in a `cargo tree --prefix depth`
/// that is compiled for the target: everything but what is below a
/// proc-macro crate, which is compiled for the host to run in the compiler.
/// A package already shown is shown again as `(*)` without its dependencies,
/// which loses nothing, since each one's dependencies are shown at its first
/// appearance.
fn compiled_into(tree: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut host_below: Option<usize> = None;
    for line in tree.lines().filter(|l| !l.is_empty()) {
        let digits = line.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
        let depth: usize = line[..digits].parse().unwrap_or_else(|_| panic!("cargo tree: {line}"));
        let mut words = line[digits..].split(' ');
        let name = words.next().unwrap();
        let version = words
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .unwrap_or_else(|| panic!("cargo tree: {line}"));
        if host_below.is_some_and(|d| depth <= d) {
            host_below = None;
        }
        if host_below.is_some() {
            continue;
        }
        out.insert((name.to_string(), version.to_string()));
        if line.contains(" (proc-macro)") {
            host_below = Some(depth);
        }
    }
    assert!(!out.is_empty(), "cargo tree printed no packages");
    out
}

/// Whether `name` is a licence file's: LICENSE, COPYING, COPYRIGHT, NOTICE
/// or UNLICENSE, alone or followed by `-`, `.` or `_` and anything, in any
/// case (`GenerateLicensesTask.LICENSE_FILE`).
fn is_licence_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "license",
        "licence",
        "copying",
        "copyright",
        "notice",
        "unlicense",
        "unlicence",
    ]
    .iter()
    .any(|base| {
        lower
            .strip_prefix(base)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.', '_']))
    })
}

/// The licence files at the root of `dir`, sorted.
fn licence_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_licence_file(&p.file_name().unwrap().to_string_lossy()))
        .collect();
    out.sort();
    out
}

/// Fails on a licence file below a crate's root that is in neither
/// `CRATE_FILES` nor `CRATE_FILES_SKIPPED`: vendored code, whose terms the
/// file would otherwise leave out. Source files and nested packages are not
/// looked at.
fn check_nested_licence_files(key: &(String, String), root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !["target", ".git"].contains(&name.as_str()) && !path.join("Cargo.toml").is_file() {
                    stack.push(path);
                }
                continue;
            }
            if dir == root || !is_licence_file(&name) || name.ends_with(".rs") {
                continue;
            }
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let listed = CRATE_FILES.iter().any(|(k, r, _)| *k == key.0 && *r == rel)
                || CRATE_FILES_SKIPPED.iter().any(|(k, r)| *k == key.0 && *r == rel);
            assert!(
                listed,
                "{FILE}: {} {} has {rel}, which is in neither CRATE_FILES nor CRATE_FILES_SKIPPED in \
                 linux/licenses.rs",
                key.0, key.1
            );
        }
    }
}

/// The licences of an SPDX expression (`/` read as `OR`) for which a text
/// is at hand: from the crate's own files (`covered`) or the toolchain's
/// standard texts. Where there is a choice, Apache-2.0, whose terms need no
/// copyright line the package does not have, then the first.
fn choose(expr: &str, covered: &[&str], canonical_dir: &Path) -> Option<Vec<String>> {
    let tokens: Vec<String> = expr
        .replace('(', " ( ")
        .replace(')', " ) ")
        .replace('/', " OR ")
        .split_whitespace()
        .map(String::from)
        .collect();
    let have = |id: &str| covered.contains(&id) || canonical_dir.join(format!("{id}.txt")).is_file();
    let mut pos = 0;
    let ids = parse_or(&tokens, &mut pos, &have)?;
    (pos == tokens.len()).then_some(ids)
}

fn parse_or(t: &[String], pos: &mut usize, have: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let mut options = vec![parse_and(t, pos, have)];
    while t.get(*pos).is_some_and(|w| w == "OR") {
        *pos += 1;
        options.push(parse_and(t, pos, have));
    }
    let options: Vec<Vec<String>> = options.into_iter().flatten().collect();
    options
        .iter()
        .find(|o| o.len() == 1 && o[0] == "Apache-2.0")
        .or_else(|| options.first())
        .cloned()
}

fn parse_and(t: &[String], pos: &mut usize, have: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let mut ids = Vec::new();
    let mut ok = true;
    loop {
        match parse_one(t, pos, have) {
            Some(more) => ids.extend(more),
            None => ok = false,
        }
        if t.get(*pos).is_some_and(|w| w == "AND") {
            *pos += 1;
        } else {
            break;
        }
    }
    ok.then_some(ids)
}

fn parse_one(t: &[String], pos: &mut usize, have: &dyn Fn(&str) -> bool) -> Option<Vec<String>> {
    let word = t.get(*pos)?.clone();
    *pos += 1;
    if word == "(" {
        let inner = parse_or(t, pos, have);
        if t.get(*pos).is_some_and(|w| w == ")") {
            *pos += 1;
        }
        return inner;
    }
    let mut ids = vec![word];
    if t.get(*pos).is_some_and(|w| w == "WITH") {
        ids.push(t.get(*pos + 1)?.clone());
        *pos += 2;
    }
    ids.iter().all(|id| have(id)).then_some(ids)
}

/// A Meson target: its name, whether it is a custom target, and its sources.
struct MesonTarget {
    name: String,
    custom: bool,
    sources: Vec<PathBuf>,
}

/// The tree's targets by each file they make, from `meson-info`.
fn meson_targets(tree: &Path) -> HashMap<PathBuf, MesonTarget> {
    let path = tree.join("meson-info/intro-targets.json");
    let json: serde_json::Value =
        serde_json::from_str(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut out = HashMap::new();
    for t in json.as_array().unwrap() {
        let sources: Vec<PathBuf> = t["target_sources"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["sources"].as_array().into_iter().flatten())
            .map(|s| canonical(Path::new(s.as_str().unwrap())))
            .collect();
        for file in t["filename"].as_array().unwrap() {
            out.insert(
                canonical(Path::new(file.as_str().unwrap())),
                MesonTarget {
                    name: t["name"].as_str().unwrap().into(),
                    custom: t["type"] == "custom",
                    sources: sources.clone(),
                },
            );
        }
    }
    out
}

/// The static archives on the link line of `output` in `build.ninja`.
fn link_archives(ninja: &str, output: &str) -> Vec<String> {
    let head = format!("build {output}: ");
    let mut lines = ninja.lines().skip_while(|l| !l.starts_with(&head));
    assert!(lines.next().is_some(), "build.ninja has no rule for {output}");
    let args = lines
        .take_while(|l| l.starts_with(' '))
        .find_map(|l| l.trim_start().strip_prefix("LINK_ARGS = "))
        .unwrap_or_else(|| panic!("build.ninja has no LINK_ARGS for {output}"));
    args.split_whitespace()
        .filter(|w| w.ends_with(".a"))
        .map(String::from)
        .collect()
}

/// The version of a vendored library, as its own files say it, where they
/// can be read without running its build.
fn vendored_version(root: &Path, key: &str) -> Option<String> {
    let tp = root.join("third_party").join(key);
    let file = |p: &str| fs::read_to_string(tp.join(p)).ok();
    let v = match key {
        "libvncserver" => between(&file("CMakeLists.txt")?, "project(LibVNCServer VERSION ", " "),
        "tigervnc" => between(&file("CMakeLists.txt")?, "set(VERSION ", ")"),
        "libjpeg-turbo" => between(&file("CMakeLists.txt")?, "set(VERSION ", ")"),
        "pixman" => between(&file("meson.build")?, "version : '", "'"),
        "freerdp" => between(
            &file("cmake/GetProjectVersion.cmake")?,
            "set(RAW_VERSION_STRING \"",
            "\"",
        ),
        "openssl" => {
            let dat = file("VERSION.dat")?;
            let part = |k: &str| between(&dat, &format!("{k}="), "\n");
            Some(format!("{}.{}.{}", part("MAJOR")?, part("MINOR")?, part("PATCH")?))
        }
        "gnutls" | "nettle" => {
            let pins = fs::read_to_string(root.join("backends/crypto-sources.txt")).ok()?;
            between(&pins, &format!("{key}.version = "), "\n")
        }
        _ => None,
    };
    v.map(|s| s.trim().to_string())
}

/// The leading comment of a C or C++ source file, without its markers.
fn leading_comment(path: &Path) -> String {
    let text = read(path);
    let text = text.trim_start();
    let body = if let Some(rest) = text.strip_prefix("/*") {
        rest.split("*/").next().unwrap().to_string()
    } else {
        text.lines()
            .map_while(|l| l.trim_start().strip_prefix("//"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(!body.trim().is_empty(), "{}: no leading comment", path.display());
    body.lines()
        .map(|l| {
            let l = l.trim_start();
            l.strip_prefix("* ").or_else(|| l.strip_prefix('*')).unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text with CRLFs, tabs at line ends, trailing spaces and blank lines at
/// either end removed, and runs of blank lines kept to one.
fn tidy(text: &str) -> String {
    let mut out = Vec::new();
    let text = text.replace("\r\n", "\n");
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() && out.last().is_none_or(|l: &&str| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
}

/// HTML's character references, as `COPYRIGHT-library.html` writes them.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest.find(';').filter(|&e| e <= 10);
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }
        });
        match (decoded, end) {
            (Some(c), Some(e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// What is between the first `start` and the next `end` after it.
fn between(s: &str, start: &str, end: &str) -> Option<String> {
    let from = s.find(start)? + start.len();
    let len = s[from..].find(end)?;
    Some(s[from..from + len].to_string())
}

fn read(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("{FILE}: reading {}: {e}", path.display()));
    String::from_utf8_lossy(&bytes).into_owned()
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn cargo(args: &[&str]) -> Vec<u8> {
    let program = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(&program)
        .args(args)
        .env("CARGO_TERM_COLOR", "never")
        .output()
        .unwrap_or_else(|e| panic!("running cargo {}: {e}", args[0]));
    assert!(
        output.status.success(),
        "cargo {} failed ({}): {}",
        args[0],
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn rustc() -> Command {
    Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
}

fn rust_sysroot() -> PathBuf {
    let out = rustc()
        .args(["--print", "sysroot"])
        .output()
        .expect("running rustc --print sysroot");
    assert!(out.status.success(), "rustc --print sysroot failed");
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
}

fn rustc_version() -> String {
    let out = rustc().arg("--version").output().expect("running rustc --version");
    let text = String::from_utf8(out.stdout).unwrap();
    text.split_whitespace().nth(1).unwrap_or_default().to_string()
}
