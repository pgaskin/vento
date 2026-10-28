// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The arguments `build.rs` gives FFmpeg's configure and dav1d's Meson when
//! cross-compiling: pure functions of the target and its tools, in a file of
//! their own so that the library's tests can run them (a build script's own
//! tests never run).
//!
//! FFmpeg's configure and Meson both take the build machine for the target
//! unless told: FFmpeg would build for the arch its compiler's defaults
//! say (which, with a cross gcc, is the target's, but whose checks would
//! then run what they compiled), and Meson would take `CC` for a compiler of
//! the build machine, whose sanity check runs a program it compiled. So for
//! a cross build FFmpeg is given `--enable-cross-compile`, the target's
//! arch and OS, the compiler cc-rs chose for the target (with the flags
//! that make it the target's, such as clang's `--target=`), its archiver,
//! the prefix of its tools and the build machine's compiler; and dav1d is
//! given a cross file. A native build is given none of this, so its
//! configure line and the stamp made of it have no cross arguments.

use std::path::{Path, PathBuf};

/// The machine the libraries are built for, as Cargo describes it to a
/// build script.
pub struct Target<'a> {
    /// `HOST` and `TARGET`, the triples.
    pub host: &'a str,
    pub target: &'a str,
    /// `CARGO_CFG_TARGET_ARCH`, `_OS` and `_ENDIAN`.
    pub arch: &'a str,
    pub os: &'a str,
    pub endian: &'a str,
}

impl Target<'_> {
    pub fn is_cross(&self) -> bool {
        self.host != self.target
    }
}

/// The target's tools, as cc-rs found them (`CC_<target>`, `AR_<target>`
/// and the rest of what it reads).
pub struct Tools {
    /// The compiler itself, whose name may carry its toolchain's prefix.
    pub cc: PathBuf,
    /// How it is run, as `CC` said: a launcher such as ccache before it and
    /// `CC`'s own arguments after it (see [`compiler_command`]), or the
    /// compiler alone.
    pub command: Vec<String>,
    /// The C++ compiler as `CXX` runs it, the same way, for the cross file
    /// the C backends' Meson build is given as well (dav1d has no C++).
    pub cxx_command: Vec<String>,
    /// The compiler's arguments that choose what it compiles for; not the
    /// optimisation and debug flags cc-rs adds, which FFmpeg chooses itself.
    pub target_flags: Vec<String>,
    pub ar: PathBuf,
    /// The build machine's C compiler, as cc-rs found it for `HOST`, for
    /// what FFmpeg's configure compiles for the build machine (`--host-cc`),
    /// whose default is `gcc` whether or not there is one.
    pub host_cc: PathBuf,
}

/// The compiler as `CC` runs it, out of the command cc-rs makes for it
/// (`Tool::to_command`, its program then its arguments): that command less
/// the `own` flags cc-rs adds at its end (`Tool::args`, which are its
/// optimisation and debug choices and the target's, passed on separately as
/// [`target_flags`]). What is left is `[ccache, gcc]` for `CC="ccache gcc"`,
/// `[gcc, -m32]` for `CC="gcc -m32"` and `[cc]` for no `CC` at all, where the
/// compiler's path alone loses the launcher and the arguments.
pub fn compiler_command(command: &[String], own: usize) -> Vec<String> {
    let keep = command.len().saturating_sub(own).max(1);
    command[..keep.min(command.len())].to_vec()
}

/// The `-m` flags that choose an ABI, an instruction set or a CPU, as cc-rs
/// adds them for a target (`-m32`, `-march=armv7-a`, `-mfloat-abi=hard`):
/// not the others, such as its `-mno-omit-leaf-frame-pointer`, which are
/// code generation, FFmpeg's choice.
const TARGET_M: &[&str] = &[
    "-m32", "-m64", "-mx32", "-march=", "-mcpu=", "-mtune=", "-mabi=", "-mfloat-abi=", "-mfpu=", "-mthumb",
    "-marm", "-mlittle-endian", "-mbig-endian",
];

/// The compiler arguments that select what it compiles for:
/// clang's `--target=`/`-target`, `--sysroot`, `--gcc-toolchain`, the
/// linker (`-fuse-ld=`), and the `-m` flags of `TARGET_M`.
pub fn target_flags(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "-target" || arg == "--sysroot" || arg == "--gcc-toolchain" || arg == "-isysroot" {
            kept.push(arg.clone());
            if let Some(value) = args.next() {
                kept.push(value.clone());
            }
        } else if arg.starts_with("--target=")
            || arg.starts_with("--sysroot=")
            || arg.starts_with("--gcc-toolchain=")
            || arg.starts_with("-fuse-ld=")
            || TARGET_M.iter().any(|m| arg == m || (m.ends_with('=') && arg.starts_with(m)))
        {
            kept.push(arg.clone());
        }
    }
    kept
}

/// The prefix a cross toolchain's tools share, which FFmpeg's
/// `--cross-prefix` puts before `nm`, `ranlib` and `strip`: everything up to
/// the last `-` of a compiler named `<prefix>gcc`, `<prefix>cc` or
/// `<prefix>clang`, its directory included. `None` for a compiler with no
/// prefix (`cc`, `clang`), whose own name says nothing of the target.
pub fn cross_prefix(cc: &Path) -> Option<String> {
    let name = cc.file_name()?.to_str()?;
    let stem = ["gcc", "cc", "clang"].iter().find_map(|tool| {
        name.strip_suffix(tool).filter(|p| p.ends_with('-'))
    })?;
    let dir = cc.parent().filter(|d| !d.as_os_str().is_empty());
    Some(match dir {
        Some(dir) => format!("{}/{stem}", dir.display()),
        None => stem.to_string(),
    })
}

/// The configure arguments FFmpeg gets for a cross build; empty for a native
/// build.
pub fn ffmpeg_args(target: &Target, tools: &Tools) -> Vec<String> {
    if !target.is_cross() {
        return Vec::new();
    }
    let mut args = vec![
        "--enable-cross-compile".to_string(),
        format!("--arch={}", target.arch),
        format!("--target-os={}", target.os),
        format!("--ar={}", tools.ar.display()),
        // Not `<prefix>pkg-config`, which FFmpeg would look for with a cross
        // prefix: it reads only the prefix's own pkg-config files, which
        // belong to no particular machine.
        "--pkg-config=pkg-config".to_string(),
        format!("--host-cc={}", tools.host_cc.display()),
    ];
    if let Some(prefix) = cross_prefix(&tools.cc) {
        args.push(format!("--cross-prefix={prefix}"));
    }
    if !tools.target_flags.is_empty() {
        let flags = tools.target_flags.join(" ");
        args.push(format!("--extra-cflags={flags}"));
        args.push(format!("--extra-ldflags={flags}"));
    }
    args
}

/// Meson's name for the CPU family of a Rust target arch.
fn cpu_family(arch: &str) -> &str {
    match arch {
        "powerpc" => "ppc",
        "powerpc64" => "ppc64",
        "loongarch64" => "loongarch64",
        other => other,
    }
}

/// The Meson cross file for the target, dav1d's and the C backends' (see
/// `linux/build.rs`), or `None` for a native build.
pub fn meson_cross_file(target: &Target, tools: &Tools) -> Option<String> {
    if !target.is_cross() {
        return None;
    }
    let quote = |s: &str| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"));
    let compiler = |command: &[String]| -> String {
        command
            .iter()
            .chain(tools.target_flags.iter())
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // The CPU is the triple's first part, which can be more specific than
    // the family (`armv7`, `i686`).
    let cpu = target.target.split('-').next().unwrap_or(target.arch);
    Some(format!(
        "[binaries]\nc = [{}]\ncpp = [{}]\nar = {}\n\n[host_machine]\nsystem = {}\ncpu_family = {}\ncpu = {}\nendian = {}\n",
        compiler(&tools.command),
        compiler(&tools.cxx_command),
        quote(&tools.ar.display().to_string()),
        quote(target.os),
        quote(cpu_family(target.arch)),
        quote(cpu),
        quote(target.endian),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const NATIVE: Target = Target {
        host: "x86_64-unknown-linux-gnu",
        target: "x86_64-unknown-linux-gnu",
        arch: "x86_64",
        os: "linux",
        endian: "little",
    };
    const ARM64: Target = Target {
        host: "x86_64-unknown-linux-gnu",
        target: "aarch64-unknown-linux-gnu",
        arch: "aarch64",
        os: "linux",
        endian: "little",
    };

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_native_build_is_told_nothing_more() {
        let tools = Tools {
            cc: "cc".into(),
            command: strings(&["cc"]),
            cxx_command: strings(&["c++"]),
            target_flags: strings(&["-m64"]),
            ar: "ar".into(),
            host_cc: "cc".into(),
        };
        assert!(ffmpeg_args(&NATIVE, &tools).is_empty());
        assert_eq!(meson_cross_file(&NATIVE, &tools), None);
    }

    #[test]
    fn a_cross_gcc_gives_its_prefix_and_arch() {
        let tools = Tools {
            cc: "aarch64-linux-gnu-gcc".into(),
            command: strings(&["aarch64-linux-gnu-gcc"]),
            cxx_command: strings(&["aarch64-linux-gnu-g++"]),
            target_flags: Vec::new(),
            ar: "aarch64-linux-gnu-ar".into(),
            host_cc: "cc".into(),
        };
        assert_eq!(
            ffmpeg_args(&ARM64, &tools),
            strings(&[
                "--enable-cross-compile",
                "--arch=aarch64",
                "--target-os=linux",
                "--ar=aarch64-linux-gnu-ar",
                "--pkg-config=pkg-config",
                "--host-cc=cc",
                "--cross-prefix=aarch64-linux-gnu-",
            ])
        );
        assert_eq!(
            meson_cross_file(&ARM64, &tools).unwrap(),
            "[binaries]\nc = ['aarch64-linux-gnu-gcc']\ncpp = ['aarch64-linux-gnu-g++']\nar = 'aarch64-linux-gnu-ar'\n\n\
             [host_machine]\nsystem = 'linux'\ncpu_family = 'aarch64'\ncpu = 'aarch64'\nendian = 'little'\n"
        );
    }

    #[test]
    fn clang_is_made_the_targets_by_its_flags() {
        // cc-rs's clang for aarch64: its target flag kept, its optimisation
        // and debug flags left to FFmpeg.
        let args = strings(&[
            "-O0", "-ffunction-sections", "-fdata-sections", "-fPIC", "-gdwarf-4",
            "-fno-omit-frame-pointer", "-mno-omit-leaf-frame-pointer",
            "--target=aarch64-unknown-linux-gnu", "--sysroot", "/sysroots/arm64",
        ]);
        let tools = Tools {
            cc: "/usr/bin/clang".into(),
            command: strings(&["/usr/bin/clang"]),
            cxx_command: strings(&["/usr/bin/clang++"]),
            target_flags: target_flags(&args),
            ar: "llvm-ar".into(),
            host_cc: "/usr/bin/clang".into(),
        };
        assert_eq!(tools.target_flags, strings(&["--target=aarch64-unknown-linux-gnu", "--sysroot", "/sysroots/arm64"]));
        let ffmpeg = ffmpeg_args(&ARM64, &tools);
        assert!(!ffmpeg.iter().any(|a| a.starts_with("--cross-prefix")), "clang has no prefix: {ffmpeg:?}");
        let flags = "--target=aarch64-unknown-linux-gnu --sysroot /sysroots/arm64";
        assert!(ffmpeg.contains(&format!("--extra-cflags={flags}")));
        assert!(ffmpeg.contains(&format!("--extra-ldflags={flags}")));
        // An armv7 gcc's ABI flags are its target's too.
        let armv7 = strings(&["-O3", "-march=armv7-a", "-mfloat-abi=hard", "-mfpu=vfpv3-d16", "-mthumb", "-Wall"]);
        assert_eq!(target_flags(&armv7), armv7[1..5]);
        assert!(meson_cross_file(&ARM64, &tools).unwrap().starts_with(
            "[binaries]\nc = ['/usr/bin/clang', '--target=aarch64-unknown-linux-gnu', '--sysroot', '/sysroots/arm64']\n\
             cpp = ['/usr/bin/clang++', '--target=aarch64-unknown-linux-gnu', '--sysroot', '/sysroots/arm64']\n"
        ));
    }

    /// cc-rs's command for `CC="ccache aarch64-linux-gnu-gcc"`, for
    /// `CC="gcc -m32"` and for no `CC`: the launcher and `CC`'s arguments
    /// kept, cc-rs's own flags left off, and the launcher before the compiler
    /// in the cross file.
    #[test]
    fn a_launcher_and_ccs_arguments_are_kept() {
        let own = strings(&["-O0", "-ffunction-sections", "-fPIC", "-m64"]);
        let made = |cc: &[&str]| [strings(cc), own.clone()].concat();
        assert_eq!(
            compiler_command(&made(&["ccache", "aarch64-linux-gnu-gcc"]), own.len()),
            strings(&["ccache", "aarch64-linux-gnu-gcc"])
        );
        assert_eq!(compiler_command(&made(&["gcc", "-m32"]), own.len()), strings(&["gcc", "-m32"]));
        assert_eq!(compiler_command(&made(&["cc"]), own.len()), strings(&["cc"]));
        assert_eq!(compiler_command(&strings(&["cc"]), 3), strings(&["cc"]));
        let tools = Tools {
            cc: "aarch64-linux-gnu-gcc".into(),
            command: strings(&["ccache", "aarch64-linux-gnu-gcc"]),
            cxx_command: strings(&["ccache", "aarch64-linux-gnu-g++"]),
            target_flags: Vec::new(),
            ar: "aarch64-linux-gnu-ar".into(),
            host_cc: "cc".into(),
        };
        assert!(meson_cross_file(&ARM64, &tools).unwrap().starts_with("[binaries]\nc = ['ccache', 'aarch64-linux-gnu-gcc']\n"));
        assert!(ffmpeg_args(&ARM64, &tools).contains(&"--cross-prefix=aarch64-linux-gnu-".to_string()));
    }

    #[test]
    fn the_prefix_keeps_the_compilers_directory() {
        assert_eq!(cross_prefix(Path::new("/opt/x/bin/arm-linux-gnueabihf-gcc")).as_deref(), Some("/opt/x/bin/arm-linux-gnueabihf-"));
        assert_eq!(cross_prefix(Path::new("riscv64-linux-gnu-cc")).as_deref(), Some("riscv64-linux-gnu-"));
        assert_eq!(cross_prefix(Path::new("aarch64-linux-gnu-clang")).as_deref(), Some("aarch64-linux-gnu-"));
        for plain in ["cc", "gcc", "clang", "/usr/bin/gcc", "x86_64-linux-gnu-gcc-14"] {
            assert_eq!(cross_prefix(Path::new(plain)), None, "{plain}");
        }
    }

    #[test]
    fn mesons_cpu_is_the_triples() {
        let armv7 = Target {
            host: "x86_64-unknown-linux-gnu",
            target: "armv7-unknown-linux-gnueabihf",
            arch: "arm",
            os: "linux",
            endian: "little",
        };
        let tools = Tools {
            cc: "arm-linux-gnueabihf-gcc".into(),
            command: strings(&["arm-linux-gnueabihf-gcc"]),
            cxx_command: strings(&["arm-linux-gnueabihf-g++"]),
            target_flags: Vec::new(),
            ar: "arm-linux-gnueabihf-ar".into(),
            host_cc: "cc".into(),
        };
        let file = meson_cross_file(&armv7, &tools).unwrap();
        assert!(file.contains("cpu_family = 'arm'\ncpu = 'armv7'\n"), "{file}");
        assert!(ffmpeg_args(&armv7, &tools).contains(&"--arch=arm".to_string()));
        assert_eq!(cpu_family("powerpc64"), "ppc64");
    }
}
