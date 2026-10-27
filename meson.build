# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The native build of the C backends for Android and Linux: each backend's
# session file and every C and C++ library under it. No upstream build file is
# used. What an upstream's own build would configure is written down in its
# port:
#
#   backends/ports/<name>                  the vendored trees both platforms compile
#   backends/platform/android/ports/<name> OpenSSL, GnuTLS and nettle for Android
#                                          (configure's output committed in gen/)
#   backends/platform/linux/ports/<name>   OpenSSL, GnuTLS and nettle for Linux
#                                          (upstream's own build, run at build
#                                          time), and RealVNC's hosting libraries
#
# On Android, Gradle builds each C module's libremotedesktop_<name>.so in a
# tree of its own per variant and ABI (MesonBuildTask in backends/buildSrc).
# On Linux, every backend's libvento_<name>.so is built in one tree. A
# development build with the probes and tests:
#
#   CC=clang CXX=clang++ meson setup build/host -Dprobes=true
#   meson compile -C build/host
#   build/host/backends/modules/libvnc/cpp/vento_libvnc_probe HOST:1 --password ... --out shot.ppm
#   meson test -C build/host
#
# The options are in meson.options. Gradle runs the Meson pinned in
# backends/build.gradle (mesonVersion); this file sets the oldest it accepts.
project('vento', 'c', 'cpp',
  meson_version: '>=1.11.0',
  default_options: [
    # RelWithDebInfo's flags, -O2 -g -DNDEBUG, on every platform and variant:
    # the decoders are several times slower at -O0, and upstream's asserts are
    # invariant checks that abort the process.
    'buildtype=debugoptimized',
    'b_ndebug=true',
    'default_library=static',
    # Every archive is linked into a shared library.
    'b_staticpic=true',
    # Each library a target names is one of its NEEDED entries, as with CMake.
    'b_asneeded=false',
    # No warning flags by default: each target says what it wants (below).
    'warning_level=0',
    # C11 as FreeRDP requires, and C++17 as TigerVNC's and RealVNC's code do
    # (an older compiler's default is C++14).
    'c_std=gnu11',
    'cpp_std=gnu++17',
  ])

android = host_machine.system() == 'android'
cpu = host_machine.cpu_family()
if host_machine.system() not in ['linux', 'android'] or cpu not in ['aarch64', 'x86_64']
  error('this builds for Linux and Android on aarch64 and x86_64, not @0@ on @1@'.format(
    host_machine.system(), cpu))
endif

cc = meson.get_compiler('c')
cxx = meson.get_compiler('cpp')
fs = import('fs')

# Warnings in our own sources are findings; in a vendored tree they are not,
# since a fix would be a patch carried indefinitely. Ours get -Wall -Wextra,
# fatal only with -Dstrict_warnings=true (scripts/check-warnings.sh), so that a
# newer toolchain's new warning cannot break a release build. Vendored targets
# get -w, and vendored headers that our sources include are system includes.
warning_args = ['-Wall', '-Wextra']
if get_option('strict_warnings')
  warning_args += '-Werror'
endif
vendored_args = ['-w']

# No link-time optimisation anywhere (b_lto stays off): with it, the bytes of
# a library depended on how its tree had been configured before.

zlib_dep = dependency('zlib')
threads_dep = dependency('threads')
dl_dep = cc.find_library('dl')
m_dep = cc.find_library('m')

subdir('backends')
