# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# nettle and hogweed for Linux: upstream's autotools build of the release
# the Android port compiles (crypto-sources.txt), with mini-gmp as there, as
# static position-independent archives that the libvnc and TigerVNC
# libraries link in. A distribution's nettle would tie a built tree to its
# soname, which changes with nearly every release. The assembly is m4 input:
# built where m4 is installed, left out where it is not.

nettle_dir = meson.project_source_root() / 'third_party/nettle'
fetched = run_command(fetch_crypto_sources_py, 'nettle', check: true).stdout().strip()
if fetched != ''
  message(fetched)
endif
# Read so that a new pin, or a tree unpacked again, reconfigures.
fs.read('../../../../crypto-sources.txt')
nettle_pin = fs.read(nettle_dir / '.pin').strip()

nettle_prefix = meson.current_build_dir() / 'prefix'
nettle_options = ['--enable-mini-gmp']
if not find_program('m4', required: false).found()
  message('nettle: no m4, so built without its assembly (install m4 for it)')
  nettle_options += ['--disable-assembler', '--disable-fat']
endif
nettle_build = custom_target('nettle',
  output: ['libnettle.a', 'libhogweed.a', 'nettle.stamp.h'],
  command: [build_upstream_py,
    '--build', meson.current_build_dir() / 'build', '--prefix', nettle_prefix,
    # Neither configure should see the environment of whatever runs the
    # build. The pin is here so that a new tarball rebuilds.
    '--env', 'CC=' + upstream_cc, '--env', 'CC_FOR_BUILD=' + upstream_cc_for_build,
    '--env', 'CFLAGS=' + ' '.join(upstream_cflags), '--env', 'CPPFLAGS=', '--env', 'LDFLAGS=',
    '--env', 'LIBS=', '--env', 'PKG_CONFIG_PATH=', '--env', 'PKG_CONFIG_LIBDIR=' + nettle_prefix / 'lib/pkgconfig',
    '--env', 'VENTO_PIN=' + nettle_pin,
    '--make', 'all', '--make', 'install',
    '--copy', 'lib/libnettle.a=@OUTPUT0@', '--copy', 'lib/libhogweed.a=@OUTPUT1@',
    '--stamp', '@OUTPUT2@',
    '--', nettle_dir / 'configure', '--prefix=' + nettle_prefix, '--libdir=' + nettle_prefix / 'lib',
    '--disable-shared', '--enable-static', '--disable-documentation', '--disable-openssl']
    + upstream_host + nettle_options,
  console: true)

# The stamp is a header so that Meson compiles a consumer after the install.
nettle_inc_args = ['-isystem', nettle_prefix / 'include']
# The archives by path, not link_with: see backend_link_depends in
# platform/linux/meson.build.
nettle_dep = declare_dependency(link_args: nettle_build[0].full_path(), sources: nettle_build[2],
  compile_args: nettle_inc_args)
hogweed_dep = declare_dependency(link_args: nettle_build[1].full_path(), sources: nettle_build[2],
  compile_args: nettle_inc_args, dependencies: nettle_dep)
backend_link_depends += [nettle_build[0], nettle_build[1]]
