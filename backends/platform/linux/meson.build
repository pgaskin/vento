# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Linux half of a C backend: `vento_log` onto stderr (cpp/vento_log.c),
# compiled into each backend's library; how a backend becomes that library;
# the probe body; and the FFmpeg the backends link.

linux_log_c = files('cpp/vento_log.c')

# OpenSSL, nettle and GnuTLS are built by their own build systems, run by
# ports/build-upstream.py at build time with the compiler and flags of this
# build (ports/<name>/meson.build). GnuTLS and nettle are release tarballs,
# which fetch-crypto-sources.py unpacks into third_party/ when they are
# missing or not the pinned release. Both scripts are not executable, so that
# find_program runs them with the Python that runs Meson.
build_upstream_py = find_program('ports/build-upstream.py')
fetch_crypto_sources_py = find_program('../../fetch-crypto-sources.py')
upstream_cflags = get_option('c_args') + ['-O' + get_option('optimization')]
if get_option('debug')
  upstream_cflags += '-g'
endif
if get_option('b_ndebug') == 'true'
  upstream_cflags += '-DNDEBUG'
endif
upstream_cflags += '-fPIC'
upstream_cc = ' '.join(cc.cmd_array())
# A cross build names the target to each upstream's configure (its GNU triple
# for nettle and GnuTLS, a target name for OpenSSL), and nettle compiles the
# table generator it runs during its build with the build machine's compiler.
upstream_cc_for_build = upstream_cc
upstream_host = []
if meson.is_cross_build()
  upstream_cc_for_build = ' '.join(meson.get_compiler('c', native: true).cmd_array())
  upstream_host = ['--host=@0@-linux-gnu'.format(cpu)]
endif

# A backend is libvento_<name>.so, which the Rust client under linux/ opens at
# run time. It carries its vendored static dependencies with their symbols
# hidden (--exclude-libs), so that it exports only the C API in vento/<name>.h
# and vento_log: several of these in one process must not interpose on each
# other's copies of a library. References to its own exports bind to its own
# definitions (-Bsymbolic), since the client opens RealVNC's library in the
# global scope and that one exports some of the same names; RealVNC's module
# leaves -Bsymbolic out.
backend_link_args = ['-Wl,--exclude-libs,ALL', '-Wl,-Bsymbolic']
# The archives of OpenSSL, nettle and GnuTLS (ports/<name>/meson.build), which
# their dependencies link by path, so that ninja relinks a library after one
# is rebuilt. Through link_with, a custom target's archive also puts its
# directory into the RUNPATH of every library linking it (Meson's build rpath,
# which only `meson install` removes), and the client ships the libraries out
# of the build tree: a tree unpacked in /tmp/a/vento would search
# /tmp/platform/linux/ports/... for libz.so.1 and the rest first.
backend_link_depends = []

# A probe connects, prints every callback and writes the framebuffer out:
# probe/probe.c with a module's own main in front of it.
probe_c = files('probe/probe.c')
probe_inc = include_directories('probe')

# FFmpeg, of which the backends link libswscale alone: FreeRDP's graphics
# pipeline scales with it (H.264 and sound are decoded on the Rust side).
# With ffmpeg_prefix, the static libraries backends/platform/linux/build.rs
# built from third_party/ffmpeg, so that the Linux client has one FFmpeg and it
# is not the system's; each is followed by what a static link of it needs
# (the libva shim after libavutil, whose VAAPI code calls libva through it:
# cpp/va_shim.c). Without it, the system's.
ffmpeg_prefix = get_option('ffmpeg_prefix')
if ffmpeg_prefix != ''
  ffmpeg_lib = ffmpeg_prefix / 'lib'
  # Not include_directories(), which refuses a directory inside the source
  # tree, as linux/target is.
  ffmpeg_args = ['-isystem', ffmpeg_prefix / 'include']
  avutil_dep = declare_dependency(compile_args: ffmpeg_args,
    dependencies: [cc.find_library('avutil', dirs: ffmpeg_lib, static: true),
                   cc.find_library('vento_va', dirs: ffmpeg_lib, static: true),
                   dl_dep, threads_dep, m_dep])
  swscale_dep = declare_dependency(compile_args: ffmpeg_args,
    dependencies: [cc.find_library('swscale', dirs: ffmpeg_lib, static: true), avutil_dep])
else
  swscale_dep = dependency('libswscale')
endif
