// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento-codegen`: writes the Android app's generated Java from the Rust
//! definitions.
//!
//! ```text
//! vento-codegen catalog <backend> <java package> <class> <out dir>
//! vento-codegen natives <java package> <class> <library> <out dir>
//! vento-codegen json
//! ```
//!
//! `catalog` writes one backend's entry in `api::catalog` (its id, name,
//! description, position in a picker and every option it takes) as a Java
//! class of constants and a `List<BackendOption>`, under `<out dir>` at the
//! path its package names. A module's `BackendProvider` answers from it, and
//! its Java refers to an option by the constant rather than by its own string.
//! `natives` writes a module's natives class: the `native` methods of
//! `NativeBackend.Natives`, from `platform_android::surface`, on a class of
//! the module's own that loads `<library>`. The library registers its
//! functions on that class. `json` prints the whole catalog, for checks that
//! read it outside Rust.
//!
//! The output depends only on the catalog (no time, no path, no map
//! ordering), so a build is the same wherever it runs.

use api::catalog::{self, About};
use api::kind::Platform;
use api::options::{Scope, Spec, Type};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["catalog", id, package, class, out] => catalog(id, package, class, out),
        ["natives", package, class, library, out] => {
            write_java(package, class, out, &java_natives(package, class, library))
        }
        ["json"] => {
            print!("{}", json());
            Ok(())
        }
        _ => Err("usage: vento-codegen catalog <backend> <java package> <class> <out dir>\n       \
                  vento-codegen natives <java package> <class> <library> <out dir>\n       \
                  vento-codegen json"
            .to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("vento-codegen: {why}");
            ExitCode::FAILURE
        }
    }
}

fn about(id: &str) -> Result<&'static About, String> {
    catalog::ALL
        .iter()
        .copied()
        .find(|a| a.id == id)
        .ok_or_else(|| format!("no backend {id:?} in the catalog"))
}

/// One backend's entry in the catalog, as a Java class.
fn catalog(id: &str, package: &str, class: &str, out: &str) -> Result<(), String> {
    write_java(package, class, out, &java_catalog(about(id)?, package, class))
}

/// Writes `<out>/<package as a path>/<class>.java`, and only that: the
/// directory is Gradle's, which empties it before a run.
fn write_java(package: &str, class: &str, out: &str, source: &str) -> Result<(), String> {
    let dir = package
        .split('.')
        .fold(PathBuf::from(out), |dir, part| dir.join(part));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{class}.java"));
    std::fs::write(&path, source).map_err(|e| format!("{}: {e}", path.display()))
}

/// A Java string literal: ASCII only, everything else as `\uXXXX`, so the
/// result does not depend on javac's source encoding.
fn java_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for unit in text.encode_utf16() {
        match unit {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x0a => out.push_str("\\n"),
            0x20..=0x7e => out.push(unit as u8 as char),
            _ => {
                let _ = write!(out, "\\u{unit:04x}");
            }
        }
    }
    out.push('"');
    out
}

fn java_nullable(text: Option<&str>) -> String {
    text.map_or_else(|| "null".to_string(), java_string)
}

/// `ViewOnly` as `VIEW_ONLY`, `AnonymousTLS` as `ANONYMOUS_TLS`, `H264` as
/// itself: a word starts at an upper-case letter after a lower-case one or a
/// digit, or at the last of a run of capitals that a lower-case one follows.
pub fn constant(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            let before = chars[i - 1];
            let after = chars.get(i + 1).copied();
            if before.is_ascii_lowercase()
                || before.is_ascii_digit()
                || (before.is_ascii_uppercase() && after.is_some_and(|a| a.is_ascii_lowercase()))
            {
                out.push('_');
            }
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

fn java_type(ty: Type) -> &'static str {
    match ty {
        Type::Bool => "Type.BOOL",
        Type::Choice => "Type.CHOICE",
        Type::Int => "Type.INT",
        Type::Text => "Type.TEXT",
    }
}

fn java_scope(scope: Scope) -> &'static str {
    match scope {
        Scope::Connection => "Scope.CONNECTION",
        Scope::Global => "Scope.GLOBAL",
        Scope::Layered => "Scope.LAYERED",
    }
}

fn java_option(spec: &Spec) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "            new BackendOption({}, {}, {},\n                    {}, ",
        constant(spec.key),
        java_string(spec.label),
        java_nullable(spec.summary_on(Platform::Android)),
        java_type(spec.ty),
    );
    if spec.choices.is_empty() {
        out.push_str("List.of(),");
    } else {
        out.push_str("List.of(");
        for (i, c) in spec.choices.iter().enumerate() {
            let _ = write!(
                out,
                "\n                            new Choice({}, {}, {}, {}){}",
                java_string(c.value),
                java_string(c.label_on(Platform::Android)),
                java_nullable(c.summary),
                java_nullable(c.requires),
                if i + 1 < spec.choices.len() { "," } else { ")," },
            );
        }
    }
    let gate = match spec.gate {
        None => "null".to_string(),
        Some(gate) => format!(
            "new Gate({}, List.of({}))",
            java_string(gate.key),
            gate.values.iter().map(|v| java_string(v)).collect::<Vec<_>>().join(", ")
        ),
    };
    let _ = write!(
        out,
        "\n                    {}, {}, {}, {})",
        java_string(spec.default),
        java_scope(spec.scope),
        spec.live,
        gate
    );
    out
}

fn java_catalog(about: &About, package: &str, class: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Generated by vento-codegen from remotedesktop-api's catalog: this\n\
         // backend's entry is its module's catalog.rs (backends/modules/<name>/),\n\
         // registered by backends/api/src/catalog.rs. Not kept in the tree:\n\
         // change the Rust and build again.\n\
         \n\
         package {package};\n\
         \n\
         import net.pgaskin.remotedesktop.backend.BackendOption;\n\
         import net.pgaskin.remotedesktop.backend.BackendOption.Choice;\n\
         import net.pgaskin.remotedesktop.backend.BackendOption.Gate;\n\
         import net.pgaskin.remotedesktop.backend.BackendOption.Scope;\n\
         import net.pgaskin.remotedesktop.backend.BackendOption.Type;\n\
         \n\
         import java.util.List;\n\
         \n\
         /**\n \
          * The {} backend's entry in the catalog shared by both clients: its\n \
          * {{@code BackendProvider}} answers from here, and its options are stored\n \
          * and set under the keys below.\n \
          */\n\
         public final class {class} {{\n\
         \n    private {class}() {{\n    }}\n",
        about.name.replace('&', "&amp;").replace('<', "&lt;"),
    );
    let _ = writeln!(out, "    public static final String ID = {};", java_string(about.id));
    let _ = writeln!(out, "    public static final String NAME = {};", java_string(about.name));
    let _ = writeln!(out, "    public static final String SHORT_NAME = {};", java_string(about.short_name));
    let _ = writeln!(out, "    public static final String DESCRIPTION = {};", java_string(about.description));
    let _ = writeln!(out, "    public static final int ORDER = {};\n", about.order);
    for spec in phone_options(about) {
        let _ = writeln!(
            out,
            "    public static final String {} = {};",
            constant(spec.key),
            java_string(spec.key)
        );
    }
    let options: Vec<String> = phone_options(about).map(java_option).collect();
    let _ = writeln!(
        out,
        "\n    public static final List<BackendOption> OPTIONS = List.of(\n{});\n}}",
        options.join(",\n")
    );
    out
}

/// A module's natives class. Every backend's has the same methods on a
/// different class, because each library registers its functions on its own
/// module's class, so two libraries loaded into one app (the RustDesk
/// add-on's and the app's own) never register over each other.
fn java_natives(package: &str, class: &str, library: &str) -> String {
    use platform_android::surface::{NATIVES, java_types};
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// Generated by vento-codegen from remotedesktop-platform-android's JNI table\n\
         // (backends/platform/android/src/surface.rs). Not kept in the tree: change\n\
         // the Rust and build again.\n\
         \n\
         package {package};\n\
         \n\
         /**\n \
          * The natives of {{@code lib{library}.so}}, registered by its\n \
          * {{@code JNI_OnLoad}} on this class only. {{@code NativeBackend.Natives}}\n \
          * documents each one.\n \
          */\n\
         final class {class} implements net.pgaskin.remotedesktop.backend.NativeBackend.Natives {{\n\
         \n    static {{\n        System.loadLibrary({});\n    }}\n",
        java_string(library),
    );
    for native in NATIVES {
        let (params, ret) = java_types(native.descriptor)
            .unwrap_or_else(|| panic!("{}: not a descriptor this writes", native.name));
        let params: Vec<String> = params
            .iter()
            .zip(native.params)
            .map(|(ty, name)| format!("{ty} {name}"))
            .collect();
        let _ = writeln!(
            out,
            "    @Override\n    public native {ret} {}({});\n",
            native.name,
            params.join(", ")
        );
    }
    out.push_str("}\n");
    out
}

fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The options the phone offers: every one of the backend's but those for
/// the command line alone ([`Spec::command_line_only`]).
fn phone_options(about: &About) -> impl Iterator<Item = &'static Spec> {
    about.options.iter().filter(|s| !s.command_line_only)
}

fn json_nullable(text: Option<&str>) -> String {
    text.map_or_else(|| "null".to_string(), json_string)
}

/// The catalog as JSON, for checks outside Rust to compare against.
fn json() -> String {
    let mut out = String::from("[\n");
    for (i, about) in catalog::ALL.iter().enumerate() {
        let options: Vec<String> = phone_options(about)
            .map(|s| {
                let choices: Vec<String> = s
                    .choices
                    .iter()
                    .map(|c| {
                        format!(
                            "{{\"value\": {}, \"label\": {}, \"summary\": {}, \"requires\": {}}}",
                            json_string(c.value),
                            json_string(c.label_on(Platform::Android)),
                            json_nullable(c.summary),
                            json_nullable(c.requires)
                        )
                    })
                    .collect();
                let gate = s.gate.map_or_else(
                    || "null".to_string(),
                    |g| {
                        format!(
                            "{{\"key\": {}, \"values\": [{}]}}",
                            json_string(g.key),
                            g.values.iter().map(|v| json_string(v)).collect::<Vec<_>>().join(", ")
                        )
                    },
                );
                format!(
                    "    {{\"key\": {}, \"label\": {}, \"summary\": {}, \"type\": {}, \"choices\": [{}], \
                     \"default\": {}, \"scope\": {}, \"live\": {}, \"gate\": {}}}",
                    json_string(s.key),
                    json_string(s.label),
                    json_nullable(s.summary_on(Platform::Android)),
                    json_string(java_type(s.ty).trim_start_matches("Type.")),
                    choices.join(", "),
                    json_string(s.default),
                    json_string(java_scope(s.scope).trim_start_matches("Scope.")),
                    s.live,
                    gate
                )
            })
            .collect();
        let _ = write!(
            out,
            "  {{\"id\": {}, \"name\": {}, \"shortName\": {}, \"description\": {}, \"order\": {}, \"options\": [\n{}\n  ]}}{}\n",
            json_string(about.id),
            json_string(about.name),
            json_string(about.short_name),
            json_string(about.description),
            about.order,
            options.join(",\n"),
            if i + 1 < catalog::ALL.len() { "," } else { "" }
        );
    }
    out.push_str("]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_become_constants_the_way_the_providers_named_them() {
        assert_eq!(constant("ViewOnly"), "VIEW_ONLY");
        assert_eq!(constant("AnonymousTLS"), "ANONYMOUS_TLS");
        assert_eq!(constant("H264"), "H264");
        assert_eq!(constant("LegacyTls"), "LEGACY_TLS");
        assert_eq!(constant("EnableUdpRfb"), "ENABLE_UDP_RFB");
        assert_eq!(constant("Nla"), "NLA");
        assert_eq!(constant("KeepAliveResponseTimeout"), "KEEP_ALIVE_RESPONSE_TIMEOUT");
    }

    #[test]
    fn a_java_string_is_ascii_and_escaped() {
        assert_eq!(java_string("1920 × 1200"), "\"1920 \\u00d7 1200\"");
        assert_eq!(java_string("a \"b\" \\ c"), "\"a \\\"b\\\" \\\\ c\"");
        assert_eq!(java_string("😀"), "\"\\ud83d\\ude00\"");
    }

    /// An option for the desktop alone is not in the phone's class or in
    /// the catalog's JSON.
    #[test]
    fn the_desktops_options_are_left_out() {
        let realvnc = about("realvnc").unwrap();
        assert!(realvnc.options.iter().any(|s| s.key == "AutoFetchLibraries"));
        let java = java_catalog(realvnc, "p", "C");
        assert!(!java.contains("AutoFetchLibraries") && !java.contains("AUTO_FETCH_LIBRARIES"), "{java}");
        assert!(java.contains("KEEP_ALIVE_RESPONSE_TIMEOUT"));
        assert!(!json().contains("AutoFetchLibraries"));
    }

    /// Every option of every backend has a constant of its own, or the
    /// generated class would not compile.
    #[test]
    fn no_two_keys_of_a_backend_make_one_constant() {
        for about in catalog::ALL {
            let mut names: Vec<String> = about.options.iter().map(|s| constant(s.key)).collect();
            names.extend(["ID", "NAME", "SHORT_NAME", "DESCRIPTION", "ORDER", "OPTIONS"].map(String::from));
            let count = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), count, "{}", about.id);
        }
    }
}
