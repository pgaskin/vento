# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# GnuTLS for Linux: upstream's autotools build of the release the Android port
# compiles (crypto-sources.txt), against the nettle beside it, as a static
# position-independent archive that the libvnc and TigerVNC libraries link
# in. An older distribution has no GnuTLS that a newer one's build runs
# against. The configuration is the Android port's (its gen.sh says why each
# switch), with three differences: its x86_64 assembly is built, and it reads
# the CA bundle and the system priority file at the paths Debian, Ubuntu and
# Fedora use for the bundle and Debian uses for the priority file (Fedora's
# crypto-policies file follows Fedora's own GnuTLS). The backends pass every
# certificate to the client anyway: both VNC libraries are patched to ask
# about one that verified, so the known hosts decide.

gnutls_dir = meson.project_source_root() / 'third_party/gnutls'
fetched = run_command(fetch_crypto_sources_py, 'gnutls', check: true).stdout().strip()
if fetched != ''
  message(fetched)
endif
gnutls_pin = fs.read(gnutls_dir / '.pin').strip()

gnutls_prefix = meson.current_build_dir() / 'prefix'
nettle_flags = '-I' + nettle_prefix / 'include'
gnutls_build = custom_target('gnutls',
  output: ['libgnutls.a', 'gnutls.stamp.h'],
  depends: nettle_build,
  command: [build_upstream_py,
    '--build', meson.current_build_dir() / 'build', '--prefix', gnutls_prefix,
    '--env', 'CC=' + upstream_cc, '--env', 'CFLAGS=' + ' '.join(upstream_cflags),
    '--env', 'CPPFLAGS=' + nettle_flags, '--env', 'LDFLAGS=', '--env', 'LIBS=',
    '--env', 'PKG_CONFIG_PATH=', '--env', 'PKG_CONFIG_LIBDIR=' + gnutls_prefix / 'lib/pkgconfig',
    '--env', 'NETTLE_CFLAGS=' + nettle_flags,
    '--env', 'NETTLE_LIBS=-L' + nettle_prefix / 'lib' + ' -lnettle',
    '--env', 'HOGWEED_CFLAGS=' + nettle_flags,
    '--env', 'HOGWEED_LIBS=-L' + nettle_prefix / 'lib' + ' -lhogweed -lnettle',
    '--env', 'VENTO_PIN=' + gnutls_pin + ' ' + nettle_pin,
    '--make', 'all', '--make', 'install',
    '--copy', 'lib/libgnutls.a=@OUTPUT0@', '--stamp', '@OUTPUT1@',
    '--', gnutls_dir / 'configure', '--prefix=' + gnutls_prefix, '--libdir=' + gnutls_prefix / 'lib',
    '--disable-shared', '--enable-static',
    '--with-nettle-mini', '--with-included-libtasn1', '--with-included-unistring',
    '--without-p11-kit', '--without-idn', '--without-tpm', '--without-tpm2',
    '--without-zlib', '--without-brotli', '--without-zstd',
    '--with-default-trust-store-file=/etc/ssl/certs/ca-certificates.crt',
    '--with-system-priority-file=/etc/gnutls/config',
    '--disable-doc', '--disable-tools', '--disable-tests', '--disable-cxx', '--disable-nls',
    '--disable-libdane', '--disable-full-test-suite'] + upstream_host,
  console: true)

# By path, not link_with: see backend_link_depends in platform/linux/meson.build.
gnutls_dep = declare_dependency(link_args: gnutls_build[0].full_path(), sources: gnutls_build[1],
  compile_args: ['-isystem', gnutls_prefix / 'include'],
  dependencies: [hogweed_dep, nettle_dep, threads_dep])
backend_link_depends += gnutls_build[0]
