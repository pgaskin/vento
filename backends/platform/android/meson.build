# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Android half of a C backend: the compile and link flags of the NDK and of
# a reproducible build, `vento_log` onto logcat (cpp/vento_log.c), and how a
# backend becomes the module's libremotedesktop_<name>.so. MesonBuildTask
# writes the cross file (the NDK's clang for the ABI and API level).

# What the NDK's CMake toolchain (build/cmake/android-legacy.toolchain.cmake in
# r28) adds to every compile and link.
add_project_arguments('-DANDROID', '-fdata-sections', '-ffunction-sections', '-funwind-tables',
  '-fstack-protector-strong', '-no-canonical-prefixes', '-D_FORTIFY_SOURCE=2', '-Wformat',
  '-Werror=format-security',
  language: ['c', 'cpp'])
add_project_link_arguments('-static-libstdc++', '-Wl,--no-undefined-version',
  '-Wl,--fatal-warnings', '-Wl,--no-undefined', '-Qunused-arguments', '-latomic', '-lm',
  language: ['c', 'cpp'])
if get_option('variant') == 'debug'
  add_project_arguments('-fno-limit-debug-info', language: ['c', 'cpp'])
else
  add_project_link_arguments('-Wl,--gc-sections', language: ['c', 'cpp'])
endif

# Nothing about where the build ran is recorded. assert() and __FILE__ put a
# source's path in .rodata, which stripping keeps: the checkout's root is
# mapped to `.`, and so is the build tree's relative path to it, which is how
# Meson names a source. lld hashes the unstripped image, NDK paths in its
# debug info included, so the build ID note is left out.
add_project_arguments(
  '-ffile-prefix-map=@0@=.'.format(meson.project_source_root()),
  '-ffile-prefix-map=@0@/=./'.format(fs.relative_to(meson.project_source_root(),
                                                    meson.project_build_root())),
  language: ['c', 'cpp'])
add_project_link_arguments('-Wl,--build-id=none', language: ['c', 'cpp'])

android_log_c = files('cpp/vento_log.c')

# A backend's library carries its module's Rust crate (the backend over the C
# session, and the JNI layer in this directory's crate), which cargo-ndk made
# as a static library before Meson runs (nativeLibrary in
# backends/build.gradle). Nothing in the C calls into it, so JNI_OnLoad is
# named as undefined to pull it in, and it is the only symbol exported
# (cpp/exports.map says why). The NDK libraries are the crate's
# native-static-libs. RealVNC's crate is the session's network under its core's
# connections, which its hooks call, and its library has exports of its own
# (modules/realvnc/cpp/meson.build).
crate_library_dir = get_option('crate_library_dir')
if crate_library_dir == ''
  error('crate_library_dir is not set: Gradle passes the directory cargo-ndk built the crates into')
endif
crate_library_dir = crate_library_dir / {'aarch64': 'arm64-v8a', 'x86_64': 'x86_64'}[cpu]
backend_link_args = ['-Wl,--undefined=JNI_OnLoad',
  '-Wl,--version-script=' + meson.current_source_dir() / 'cpp/exports.map']
backend_link_depends = files('cpp/exports.map')
backend_deps = []
foreach name : ['jnigraphics', 'mediandk', 'aaudio', 'log', 'dl', 'm']
  backend_deps += cc.find_library(name)
endforeach
