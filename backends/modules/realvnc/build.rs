// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Generates Rust bindings for `vento/realvnc.h`, this backend's header
//! (`cpp/include`), for the `ffi` module in `src/lib.rs`.
//!
//! Only the header's own `vento_realvnc_` items are generated, on top of the
//! `api` crate's shared declarations, as in each C backend's crate. The
//! functions are generated as a loader rather than as `extern` declarations:
//! a `Library` that opens the shared library and looks up each function, so
//! nothing here links against a library the client's build may not have
//! made. Every function is required: a library missing one of them is not
//! the one this header describes.
//!
//! Also reads the pins of the viewer's build from `Library.java`, as the
//! Meson build does for the session: the version, and the SHA-256 of each
//! of the two libraries for the target's ABI, as `VENTO_REALVNC_VERSION`,
//! `VENTO_REALVNC_ABI`, `VENTO_REALVNC_SHA256_LIBCXX_SHARED` and
//! `VENTO_REALVNC_SHA256_LIBVNCVIEWER`. The ABI and the hashes are empty for
//! an architecture `Library.java` has no build for.
//!
//! Does nothing on targets other than Linux, where the crate is empty.

use std::env;
use std::path::{Path, PathBuf};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    pins(&manifest.join("../../../realvnc/src/main/java/com/realvnc/vncviewer/jni/Library.java"));
    let own = manifest.join("cpp/include");
    let shared = manifest.join("../../api/include");
    println!("cargo:rerun-if-changed={}", own.display());
    println!("cargo:rerun-if-changed={}", shared.display());
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindgen::Builder::default()
        .header_contents("realvnc.h", "#include <vento/realvnc.h>\n")
        .clang_arg(format!("-I{}", own.display()))
        .clang_arg(format!("-I{}", shared.display()))
        .allowlist_item("vento_realvnc_.*")
        .allowlist_item("VENTO_REALVNC_.*")
        .allowlist_recursively(false)
        .derive_default(true)
        .layout_tests(false)
        .dynamic_library_name("Library")
        .dynamic_link_require_all(true)
        // Wraps the loader's calls in their own `unsafe` blocks, as edition
        // 2024 expects in the body of an `unsafe fn`.
        .wrap_unsafe_ops(true)
        .generate()
        .expect("bindgen over vento/realvnc.h")
        .write_to_file(out.join("bindings.rs"))
        .expect("writing bindings.rs");
}

/// The pins in `Library.java`, as `cargo:rustc-env` variables. The parse is
/// the Meson build's: the version after `VERSION = "`, and each hash as the
/// first quoted string after its `"<abi>/<name>",` key.
fn pins(library_java: &Path) {
    println!("cargo:rerun-if-changed={}", library_java.display());
    let text = std::fs::read_to_string(library_java)
        .unwrap_or_else(|e| panic!("{}: {e}", library_java.display()));
    let version = text
        .split_once("VERSION = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(version, _)| version)
        .expect("no VERSION in Library.java");
    println!("cargo:rustc-env=VENTO_REALVNC_VERSION={version}");
    // The APK's directory for the target's ABI.
    let abi = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "x86_64",
        Ok("aarch64") => "arm64-v8a",
        _ => "",
    };
    println!("cargo:rustc-env=VENTO_REALVNC_ABI={abi}");
    for (name, var) in [("libc++_shared.so", "LIBCXX_SHARED"), ("libvncviewer.so", "LIBVNCVIEWER")] {
        let hash = if abi.is_empty() { "" } else { hash(&text, abi, name) };
        println!("cargo:rustc-env=VENTO_REALVNC_SHA256_{var}={hash}");
    }
}

fn hash<'a>(text: &'a str, abi: &str, name: &str) -> &'a str {
    let key = format!("\"{abi}/{name}\",");
    text.split_once(&key)
        .and_then(|(_, rest)| rest.split('"').nth(1))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .unwrap_or_else(|| panic!("no SHA-256 for {abi}/{name} in Library.java"))
}
