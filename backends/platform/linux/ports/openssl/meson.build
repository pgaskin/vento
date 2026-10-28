# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# OpenSSL for Linux: upstream's own build of the submodule the Android port
# compiles, as two static position-independent archives that the FreeRDP
# library links in. A distribution's libssl.so.1.1 or .so.3 would tie a built
# tree to it. The submodule is only read: the build is out of tree.
#
# The configuration is the Android port's (its gen.sh says why each switch),
# with the assembly on as there, and three differences:
# - The legacy provider is built (no `no-legacy`). FreeRDP uses none of it,
#   but WinPR loads it by name when it initialises OpenSSL and, failing that,
#   warns on stderr that MD4 is unavailable. With no-dso it is compiled in as
#   a built-in provider, so loading it reads no file. About 70 kB.
# - OPENSSLDIR is /etc/ssl, where Debian and Ubuntu keep the CA certificates
#   (certs/) and Fedora links cert.pem to its bundle. It decides nothing:
#   FreeRDP is told the application checks certificates, so every one goes
#   to the client's known hosts.
# - no-autoload-config: the distribution's openssl.cnf, written for its own
#   OpenSSL build, is not read.

openssl_dir = meson.project_source_root() / 'third_party/openssl'
if not fs.exists(openssl_dir / 'Configure')
  error('third_party/openssl is empty: run `git submodule update --init`')
endif
# Configure would build the in-place configuration instead.
if fs.exists(openssl_dir / 'configdata.pm')
  error('third_party/openssl has been configured in place (configdata.pm); '
    + '`git -C third_party/openssl clean -fdx` restores it')
endif
openssl_version_dat = fs.read(openssl_dir / 'VERSION.dat')
openssl_version = '.'.join([
  openssl_version_dat.split('MAJOR=')[1].split('\n')[0],
  openssl_version_dat.split('MINOR=')[1].split('\n')[0],
  openssl_version_dat.split('PATCH=')[1].split('\n')[0],
])

perl = find_program('perl')
if run_command(perl, '-e', 'use FindBin; use IPC::Cmd; use File::Compare; use Time::Piece;',
               check: false).returncode() != 0
  error('OpenSSL\'s Configure needs perl with its core modules (Fedora: perl-core; Debian and Ubuntu: perl)')
endif

openssl_prefix = meson.current_build_dir() / 'prefix'
# Configure takes the build machine for the target unless it is named.
openssl_target = []
if meson.is_cross_build()
  openssl_target = ['linux-' + cpu]
endif
openssl_build = custom_target('openssl',
  output: ['libssl.a', 'libcrypto.a', 'openssl.stamp.h'],
  command: [build_upstream_py,
    # The version is here so that a new pin rebuilds.
    '--build', meson.current_build_dir() / 'build-' + openssl_version, '--prefix', openssl_prefix,
    # Configure takes its own optimisation flags unless CFLAGS is set.
    '--unset', 'CFLAGS', '--unset', 'CPPFLAGS', '--unset', 'LDFLAGS', '--unset', 'LIBS',
    '--env', 'CC=' + upstream_cc,
    '--make', 'build_libs', '--make', 'install_dev',
    '--copy', 'lib/libssl.a=@OUTPUT0@', '--copy', 'lib/libcrypto.a=@OUTPUT1@',
    '--stamp', '@OUTPUT2@',
    '--', perl, openssl_dir / 'Configure', '--prefix=' + openssl_prefix, '--libdir=lib',
    '--openssldir=/etc/ssl',
    'no-shared', 'no-tests', 'no-apps', 'no-docs', 'no-engine', 'no-dso', 'no-comp', 'no-dtls',
    'no-quic', 'no-srp', 'no-psk', 'no-ssl3', 'no-weak-ssl-ciphers', 'no-autoload-config', '-fPIC']
    + openssl_target,
  console: true)

# By path, not link_with: see backend_link_depends in platform/linux/meson.build.
openssl_dep = declare_dependency(link_args: [openssl_build[0].full_path(), openssl_build[1].full_path()],
  sources: openssl_build[2],
  compile_args: ['-isystem', openssl_prefix / 'include'],
  dependencies: [threads_dep, dl_dep])
backend_link_depends += [openssl_build[0], openssl_build[1]]
