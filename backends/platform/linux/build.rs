// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's FFmpeg: dav1d and FFmpeg built from their submodules
//! (`third_party/dav1d`, `third_party/ffmpeg`) into `OUT_DIR` as static
//! libraries, and `cpp/vento_video.c` compiled against them, so that the Linux
//! client links no system FFmpeg and needs no FFmpeg development files.
//!
//! **One FFmpeg for every desktop consumer.** The Rust decoders here (video,
//! and AAC and Opus sound) are one; FreeRDP's libswscale, built by
//! `linux/build.rs` through Meson, is the other. That build gets the location
//! of these libraries through the `links` metadata (`DEP_VENTO_VIDEO_PREFIX`),
//! so both link the same configuration of the same source. FFmpeg is
//! configured with `--disable-everything` and then exactly what those
//! consumers use (`DECODERS`, `HWACCELS`, libswscale and libswresample), with
//! `--disable-autodetect` so that nothing installed on the build machine is
//! linked in, and without `--enable-gpl`: the result is LGPL 2.1 or later, and
//! dav1d is BSD-2-Clause.
//!
//! **Built once.** The two builds take about a minute and a half together
//! (dav1d with Meson and Ninja, FFmpeg with its configure and make), and are
//! redone only when their inputs change: any of the three submodules' commits,
//! the configure line, the compiler (as `CC` runs it, and its version, its file
//! watched) or whether NASM is present, all recorded in a stamp beside them.
//! Any other rerun of this script (a change to the wrapper, a new `PATH`) finds
//! the stamp unchanged and only recompiles the wrapper. A debug and a release
//! build each have their own `OUT_DIR` and so their own copy; FFmpeg is
//! optimised in both, as its configure does by default.
//!
//! **Hardware decoders, from headers alone.** FFmpeg's VAAPI decoders
//! (`HWACCELS`) are built against `third_party/libva`'s headers, installed into
//! the prefix with the one libva generates, through our own pkg-config files.
//! libva itself is not linked: its functions are `cpp/va_shim.c`'s, which
//! loads the real ones when hardware decoding is first requested. So the binary
//! runs where libva is not installed, and does not load it while hardware
//! decoding is off.
//!
//! **Cross-compiling.** Where the target is not the build machine, FFmpeg's
//! configure is told so and dav1d gets a Meson cross file in `OUT_DIR`,
//! both from the compilers and archiver cc-rs chose for the target
//! (`CC_<target>`, `CXX_<target>`, `AR_<target>`, ...): see `cross.rs`, whose
//! functions the crate's tests run. Both are part of the stamp. The cross
//! file is also `linux/build.rs`'s, for the C backends (`links` metadata
//! again, `DEP_VENTO_VIDEO_CROSS_FILE`). A native build's configure line has
//! no cross arguments.
//!
//! **Assembly** is NASM's on x86: without it dav1d and libavcodec fall back to
//! their C code, which works at a fraction of the speed, so its absence is a
//! warning rather than an error. Installing it later rebuilds them with it at
//! the next build, without a `cargo clean`.
//!
//! Empty on Android, as the crate is: the phone's decoder is MediaCodec.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod cross;

/// The decoders, and their users. Each is checked by name in libavcodec's
/// configure, so a misspelling is an error rather than a missing decoder.
const DECODERS: &[&str] = &[
    // RustDesk (the Rust decoder), TigerVNC's H.264 encoding, FreeRDP's
    // graphics pipeline (AVC420 and AVC444).
    "h264",
    // RustDesk.
    "hevc",
    "vp8",
    "vp9",
    // RustDesk's AV1, through dav1d in software; libavcodec's own `av1`
    // decodes only with hardware acceleration, and is there for that alone.
    "libdav1d",
    "av1",
    // Sound (`cpp/vento_ffaudio.c`): AAC from an RDP server (Windows sends it
    // whenever it is offered), and Opus from RDP and RustDesk. libavcodec's
    // Opus decoder resamples its SILK half with libswresample, which is why
    // that is built.
    "aac",
    "opus",
];

/// The VAAPI decoders of the codecs above, for the Rust decoder when hardware
/// decoding is requested (`cpp/vento_video.c`); TigerVNC's and FreeRDP's H.264
/// go through it as well. libva is only headers at build time
/// (`third_party/libva`); its functions are reached through `cpp/va_shim.c`.
const HWACCELS: &[&str] = &[
    "h264_vaapi",
    "hevc_vaapi",
    "vp8_vaapi",
    "vp9_vaapi",
    "av1_vaapi",
];

fn main() {
    println!("cargo:rerun-if-changed=cpp/vento_video.c");
    println!("cargo:rerun-if-changed=cpp/vento_video.h");
    println!("cargo:rerun-if-changed=cpp/vento_ffaudio.c");
    println!("cargo:rerun-if-changed=cpp/vento_ffaudio.h");
    println!("cargo:rerun-if-changed=cpp/va_shim.c");
    println!("cargo:rerun-if-changed=cpp/va_shim.h");
    println!("cargo:rerun-if-changed=cpp/vento_log.c");
    println!("cargo:rerun-if-changed=cpp/vento_log.h");
    println!("cargo:rerun-if-changed=../../api/include/vento/log.h");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        return;
    }
    for var in ["CC", "PATH"] {
        println!("cargo:rerun-if-env-changed={var}");
    }

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let third_party = manifest
        .join("../../../third_party")
        .canonicalize()
        .expect("third_party/ at the repository root");
    let dav1d = third_party.join("dav1d");
    let ffmpeg = third_party.join("ffmpeg");
    let libva = third_party.join("libva");
    for (source, marker) in [
        (&dav1d, "meson.build"),
        (&ffmpeg, "configure"),
        (&libva, "va/va.h"),
    ] {
        assert!(
            source.join(marker).is_file(),
            "{} is empty; run: git submodule update --init third_party/dav1d third_party/ffmpeg \
             third_party/libva",
            source.display()
        );
        if let Some(head) = git_head(source) {
            println!("cargo:rerun-if-changed={}", head.display());
        }
    }

    let build = cc::Build::new();
    let compiler = build.get_compiler();
    let cc = compiler.path().to_path_buf();
    let command = command_of(&compiler);
    // The stamp holds the compiler's version, so that an upgrade rebuilds
    // FFmpeg; but a compiler changed under the same name does not rerun this
    // script, so its file is watched as well.
    if let Some(file) = executable(&cc) {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    let var = |name: &str| env::var(name).unwrap_or_default();
    let (host, target) = (var("HOST"), var("TARGET"));
    let (arch, os, endian) = (
        var("CARGO_CFG_TARGET_ARCH"),
        var("CARGO_CFG_TARGET_OS"),
        var("CARGO_CFG_TARGET_ENDIAN"),
    );
    let target = cross::Target {
        host: &host,
        target: &target,
        arch: &arch,
        os: &os,
        endian: &endian,
    };
    let tools = cross::Tools {
        cc: cc.clone(),
        command: command.clone(),
        cxx_command: command_of(&cc::Build::new().cpp(true).get_compiler()),
        target_flags: cross::target_flags(
            &compiler
                .args()
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        ),
        ar: build.get_archiver().get_program().into(),
        host_cc: cc::Build::new()
            .target(&host)
            .get_compiler()
            .path()
            .to_path_buf(),
    };
    let cross_file = cross::meson_cross_file(&target, &tools);
    let x86 = matches!(
        env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
        Ok("x86" | "x86_64")
    );
    let asm = !x86 || runs("nasm", &["-v"]);
    if !asm {
        println!(
            "cargo:warning=FFmpeg: no nasm on the path, so dav1d and libavcodec are built without \
             their x86 assembly and decode several times slower"
        );
        // Installing NASM changes no variable: it puts a file into a
        // directory already on the path, which `rerun-if-env-changed=PATH`
        // does not see, and the build would keep its slow FFmpeg until a
        // `cargo clean`. So while it is missing, each directory on the path
        // is watched as well. Its arrival changes one of them, this script
        // runs again, finds NASM and so the stamp's `asm false` stale, and
        // rebuilds FFmpeg with the assembly. Anything else installed there in
        // the meantime reruns it too, which costs only the wrapper's
        // recompile; with NASM present nothing is watched.
        if let Some(path) = env::var_os("PATH") {
            for dir in env::split_paths(&path).filter(|dir| dir.is_dir()) {
                println!("cargo:rerun-if-changed={}", dir.display());
            }
        }
    }

    let root = PathBuf::from(env::var("OUT_DIR").unwrap()).join("ffmpeg");
    let prefix = root.join("prefix");
    let cc_line = command.join(" ");
    let mut configure = configure_args(&prefix, &cc_line, asm);
    configure.extend(cross::ffmpeg_args(&target, &tools));
    let mut stamp = format!(
        "dav1d {}\nffmpeg {}\nlibva {}\ncc {cc_line} {}\nasm {asm}\n{}\n",
        revision(&dav1d),
        revision(&ffmpeg),
        revision(&libva),
        version(&cc),
        configure.join(" ")
    );
    if let Some(file) = &cross_file {
        stamp.push_str(&format!("meson cross file:\n{file}"));
    }
    let stamp_path = root.join("stamp");
    if fs::read_to_string(&stamp_path).ok().as_deref() != Some(stamp.as_str()) {
        println!("cargo:warning=FFmpeg: building dav1d and FFmpeg from third_party/ (once)");
        if root.exists() {
            fs::remove_dir_all(&root).expect("deleting the stale FFmpeg build");
        }
        fs::create_dir_all(&root).unwrap();
        build_dav1d(&dav1d, &root, &prefix, &cc_line, asm, cross_file.as_deref());
        install_libva_headers(&libva, &prefix);
        write_pkg_config(&prefix);
        build_va_shim(&manifest, &root, &prefix);
        build_ffmpeg(&ffmpeg, &root, &prefix, &configure);
        fs::write(&stamp_path, &stamp).unwrap();
    } else {
        // The shim is not part of the stamp: FFmpeg's libraries only link
        // against it, so a change to it recompiles the shim alone.
        build_va_shim(&manifest, &root, &prefix);
    }

    // With `vento_log.c`, the stderr logger the C backends link, for the
    // client's own log lines (`src/stderr.rs`) and FFmpeg's.
    cc::Build::new()
        .file(manifest.join("cpp/vento_video.c"))
        .file(manifest.join("cpp/vento_ffaudio.c"))
        .file(manifest.join("cpp/vento_log.c"))
        .include(prefix.join("include"))
        .include(manifest.join("../../api/include"))
        .warnings(true)
        .compile("vento_video");
    // libavcodec and its dependencies, in static link order, from its own
    // pkg-config file: dav1d and libavutil after it, the libva shim after
    // that, and the system's libm, libdl and threads. Not libatomic: FFmpeg's
    // configure names it wherever it is installed, needed or not, and on
    // the targets built here (native atomics) nothing is taken from it. The
    // C backends' Meson link of the same libraries does not name it either.
    for lib in static_libs(&prefix, "libavcodec") {
        if lib == "atomic" {
            continue;
        }
        if prefix.join("lib").join(format!("lib{lib}.a")).is_file() {
            println!("cargo:rustc-link-lib=static={lib}");
        } else {
            println!("cargo:rustc-link-lib={lib}");
        }
    }
    println!(
        "cargo:rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    // For `linux/build.rs`, which hands it to the C backends' Meson build,
    // with the cross file in a cross build, so that they are built with the
    // same compilers.
    println!("cargo:prefix={}", prefix.display());
    if let Some(file) = &cross_file {
        let path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("meson-cross.ini");
        if fs::read_to_string(&path).ok().as_deref() != Some(file.as_str()) {
            fs::write(&path, file).unwrap();
        }
        println!("cargo:cross_file={}", path.display());
    }
}

/// The compiler command for FFmpeg's configure and Meson: the path alone
/// would drop a launcher (`CC="ccache gcc"`) and `CC`'s own arguments
/// (`CC="gcc -m32"`).
fn command_of(compiler: &cc::Tool) -> Vec<String> {
    let made = compiler.to_command();
    cross::compiler_command(
        &std::iter::once(made.get_program())
            .chain(made.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        compiler.args().len(),
    )
}

/// FFmpeg's configure line, which is also part of the stamp. `cc` is the
/// compiler as `CC` runs it, words and all, which configure splits as a
/// shell would.
fn configure_args(prefix: &Path, cc: &str, asm: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "--disable-everything",
        "--disable-autodetect",
        "--disable-iconv",
        "--disable-programs",
        "--disable-doc",
        "--disable-network",
        "--disable-debug",
        "--disable-avformat",
        "--disable-avfilter",
        "--disable-avdevice",
        "--enable-swscale",
        "--enable-swresample",
        "--enable-static",
        "--disable-shared",
        // Into the C backends' shared libraries as well as the binary.
        "--enable-pic",
        "--enable-pthreads",
        "--enable-libdav1d",
        // Hardware decoding (`HWACCELS`), against the headers of
        // `third_party/` through the pkg-config files `write_pkg_config`
        // puts in the prefix. Named, since `--disable-autodetect` turns off
        // everything not named; nothing else on the machine is looked for,
        // so no libdrm and no X11.
        "--enable-vaapi",
        // It reads only the prefix's pkg-config files (dav1d's and libva's),
        // and a static link needs their static half.
        "--pkg-config-flags=--static",
    ]
    .map(String::from)
    .into();
    args.push(format!("--prefix={}", prefix.display()));
    args.push(format!("--cc={cc}"));
    args.push(format!("--enable-decoder={}", DECODERS.join(",")));
    args.push(format!("--enable-hwaccel={}", HWACCELS.join(",")));
    if !asm {
        args.push("--disable-x86asm".into());
    }
    args
}

/// Builds dav1d with `cc` (natively, as `CC`, which Meson splits into a
/// launcher and a compiler) or with the cross file's compiler (`cross`).
/// Meson is `MESON`, or `meson` on the path, as for the C backends.
fn build_dav1d(source: &Path, root: &Path, prefix: &Path, cc: &str, asm: bool, cross: Option<&str>) {
    let build = root.join("dav1d-build");
    let mut setup = Command::new(env::var_os("MESON").unwrap_or_else(|| "meson".into()));
    setup
        .arg("setup")
        .arg(&build)
        .arg(source)
        .arg(format!("--prefix={}", prefix.display()))
        .args([
            "--libdir=lib",
            "--buildtype=release",
            "--default-library=static",
            "-Db_staticpic=true",
            "-Denable_tools=false",
            "-Denable_tests=false",
            "-Denable_examples=false",
            // Nothing is fetched: dav1d needs no subproject without its tools.
            "--wrap-mode=nodownload",
        ])
        .arg(format!("-Denable_asm={asm}"));
    match cross {
        // The target's compiler is the cross file's. Meson would take `CC` for
        // the build machine's compiler, whose sanity check runs what it
        // built.
        Some(file) => {
            let path = root.join("dav1d-cross.ini");
            fs::write(&path, file).unwrap();
            setup.arg("--cross-file").arg(path);
        }
        None => {
            setup.env("CC", cc);
        }
    }
    run(setup, &root.join("dav1d-setup.log"));
    let mut install = Command::new("ninja");
    install.arg("-C").arg(&build).arg("install").arg(jobs());
    run(install, &root.join("dav1d-build.log"));
}

/// Installs libva's headers into the prefix, where its own install would put
/// them (`include/va/`), with the one it generates: `va_version.h`, from its
/// template and the version `meson.build` declares. No libva code is compiled;
/// its functions are `cpp/va_shim.c`'s.
fn install_libva_headers(source: &Path, prefix: &Path) {
    let into = prefix.join("include/va");
    fs::create_dir_all(&into).unwrap();
    let meson = fs::read_to_string(source.join("meson.build")).expect("libva's meson.build");
    let part = |name: &str| -> String {
        meson
            .lines()
            .find_map(|l| {
                let (key, value) = l.split_once('=')?;
                (key.trim() == name).then(|| value.trim().to_string())
            })
            .unwrap_or_else(|| panic!("libva's meson.build declares no {name}"))
    };
    let (major, minor, micro) = (
        part("va_api_major_version"),
        part("va_api_minor_version"),
        part("va_api_micro_version"),
    );
    let template =
        fs::read_to_string(source.join("va/va_version.h.in")).expect("libva's va_version.h.in");
    let version = template
        .replace("@VA_API_MAJOR_VERSION@", &major)
        .replace("@VA_API_MINOR_VERSION@", &minor)
        .replace("@VA_API_MICRO_VERSION@", &micro)
        .replace("@VA_API_VERSION@", &format!("{major}.{minor}.{micro}"));
    assert!(
        !version.contains("@VA_"),
        "va_version.h.in has a placeholder this does not fill"
    );
    fs::write(into.join("va_version.h"), version).unwrap();
    // The headers libva's install puts there, as listed in `va/meson.build`:
    // the core ones and the DRM one, which has its own directory in the tree
    // and sits beside the rest once installed.
    let list = fs::read_to_string(source.join("va/meson.build")).expect("libva's va/meson.build");
    let listed = |name: &str| -> Vec<String> {
        let start = list
            .find(&format!("{name} = ["))
            .unwrap_or_else(|| panic!("va/meson.build lists no {name}"));
        let body = &list[start..start + list[start..].find(']').unwrap()];
        body.split('\'')
            .skip(1)
            .step_by(2)
            .map(String::from)
            .collect()
    };
    let headers = [listed("libva_headers"), listed("libva_drm_headers")].concat();
    assert!(
        headers.iter().any(|h| h == "va.h") && headers.iter().any(|h| h == "drm/va_drm.h"),
        "va/meson.build's header lists are not what this reads them as"
    );
    for header in headers {
        let from = source.join("va").join(&header);
        fs::copy(&from, into.join(from.file_name().unwrap())).unwrap();
    }
}

/// Writes the pkg-config files describing libva to FFmpeg's configure (which
/// reads only the prefix's, as for dav1d): libva and libva-drm are the headers
/// just installed and the shim, with the shim's own dependencies: libdl for
/// `dlopen`, and libpthread for `pthread_once`, which is in libc itself only
/// from glibc 2.34. Before that, configure's libva test program does not link
/// without it, and configure stops ("vaapi requested but not found").
fn write_pkg_config(prefix: &Path) {
    let dir = prefix.join("lib/pkgconfig");
    fs::create_dir_all(&dir).unwrap();
    let head = format!(
        "prefix={}\nlibdir=${{prefix}}/lib\nincludedir=${{prefix}}/include\n\n",
        prefix.display()
    );
    // The version libva would say it is: 2.x for the 1.x of its API.
    let api = fs::read_to_string(prefix.join("include/va/va_version.h")).unwrap();
    let api = api
        .lines()
        .find_map(|l| l.strip_prefix("#define VA_VERSION_S"))
        .map(|v| v.trim().trim_matches('"').to_string())
        .expect("va_version.h has VA_VERSION_S");
    let libva_version = match api.split_once('.') {
        Some(("1", rest)) => format!("2.{rest}"),
        _ => api.clone(),
    };
    fs::write(
        dir.join("libva.pc"),
        format!(
            "{head}Name: libva\nDescription: libva's headers over vento's dlopen shim\n\
             Version: {libva_version}\nCflags: -I${{includedir}}\n\
             Libs: -L${{libdir}} -lvento_va -ldl -lpthread\n"
        ),
    )
    .unwrap();
    fs::write(
        dir.join("libva-drm.pc"),
        format!(
            "{head}Name: libva-drm\nDescription: libva-drm's header over vento's dlopen shim\n\
             Version: {libva_version}\nRequires: libva\nCflags: -I${{includedir}}\n\
             Libs: -L${{libdir}} -lvento_va\n"
        ),
    )
    .unwrap();
}

/// Builds `cpp/va_shim.c` as `libvento_va.a` in the prefix, named by the
/// pkg-config files above and linked with FFmpeg's libraries from then on.
/// Copied in only when it differs, so that the C backends' build does not
/// relink for nothing.
fn build_va_shim(manifest: &Path, root: &Path, prefix: &Path) {
    let out = root.join("va-shim");
    fs::create_dir_all(&out).unwrap();
    cc::Build::new()
        .file(manifest.join("cpp/va_shim.c"))
        .include(prefix.join("include"))
        .warnings(true)
        .pic(true)
        .out_dir(&out)
        .cargo_metadata(false)
        .compile("vento_va");
    let built = out.join("libvento_va.a");
    let installed = prefix.join("lib/libvento_va.a");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    if fs::read(&built).ok() != fs::read(&installed).ok() {
        fs::copy(&built, &installed).unwrap();
    }
}

fn build_ffmpeg(source: &Path, root: &Path, prefix: &Path, configure: &[String]) {
    let build = root.join("ffmpeg-build");
    fs::create_dir_all(&build).unwrap();
    let mut cmd = Command::new(source.join("configure"));
    cmd.current_dir(&build)
        .args(configure)
        // The prefix's pkg-config files alone, not the system's.
        .env("PKG_CONFIG_LIBDIR", prefix.join("lib/pkgconfig"))
        .env_remove("PKG_CONFIG_PATH");
    run(cmd, &root.join("ffmpeg-configure.log"));
    let mut make = Command::new("make");
    make.current_dir(&build).arg(jobs()).arg("install");
    run(make, &root.join("ffmpeg-build.log"));
}

/// `-lfoo` for each library `pkg-config --static` names for `package` in the
/// built prefix, each once, at its last place, which is where a static link
/// needs it.
fn static_libs(prefix: &Path, package: &str) -> Vec<String> {
    let out = Command::new("pkg-config")
        .args(["--static", "--libs", package])
        .env("PKG_CONFIG_LIBDIR", prefix.join("lib/pkgconfig"))
        .env_remove("PKG_CONFIG_PATH")
        .output()
        .expect("running pkg-config");
    assert!(
        out.status.success(),
        "pkg-config does not know the {package} just built"
    );
    let words: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|w| match w {
            "-pthread" => Some("pthread".to_string()),
            w => w.strip_prefix("-l").map(String::from),
        })
        .collect();
    let mut libs = Vec::new();
    for (i, lib) in words.iter().enumerate() {
        if !words[i + 1..].contains(lib) {
            libs.push(lib.clone());
        }
    }
    libs
}

fn jobs() -> String {
    format!("-j{}", env::var("NUM_JOBS").unwrap_or_else(|_| "4".into()))
}

/// Runs `cmd` with its output into `log`, and fails the build with the log's
/// end when it fails.
fn run(mut cmd: Command, log: &Path) {
    let file = fs::File::create(log).unwrap();
    let status = cmd
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .status()
        .unwrap_or_else(|e| panic!("could not run {:?}: {e}", cmd.get_program()));
    if !status.success() {
        let text = fs::read_to_string(log).unwrap_or_default();
        let tail: Vec<&str> = text.lines().rev().take(30).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        panic!(
            "{:?} failed ({status}); {}:\n{}",
            cmd.get_program(),
            log.display(),
            tail.join("\n")
        );
    }
}

fn runs(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// The submodule's checked-out commit, or "unknown" outside a git checkout.
fn revision(source: &Path) -> String {
    Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// The file that changes when the submodule checks out another commit: `HEAD`
/// in its git directory, which a submodule's `.git` file points to.
fn git_head(source: &Path) -> Option<PathBuf> {
    let dot_git = source.join(".git");
    let dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = fs::read_to_string(&dot_git).ok()?;
        source.join(text.trim().strip_prefix("gitdir:")?.trim())
    };
    Some(dir.join("HEAD")).filter(|p| p.is_file())
}

/// The file `cc` runs, found on `PATH` as a shell would for a bare name, with
/// symlinks followed: the file a compiler upgrade replaces.
fn executable(cc: &Path) -> Option<PathBuf> {
    let found = if cc.components().count() > 1 {
        Some(cc.to_path_buf())
    } else {
        env::split_paths(&env::var_os("PATH")?).map(|dir| dir.join(cc)).find(|p| p.is_file())
    };
    found?.canonicalize().ok()
}

/// The compiler's first line of `--version`, so that an upgrade rebuilds.
fn version(cc: &Path) -> String {
    Command::new(cc)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .map(String::from)
        })
        .unwrap_or_default()
}
