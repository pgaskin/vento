# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The C backends selected by the `backends` option, and the libraries each one
# needs, in dependency order: the platform half (platform/<os>/meson.build),
# the ports, then the modules (modules/<name>/cpp/meson.build), each of which
# makes its own library.

backends = get_option('backends')
if not android and 'realvnc' in backends and not (cc.get_id() == 'clang' and cxx.get_id() == 'clang')
  # libc-shim, under RealVNC's hosting layer, does not build with GCC.
  message('RealVNC: skipped, since its hosting layer needs clang and this is @0@ '.format(cc.get_id())
    + '(CC=clang CXX=clang++ meson setup ... to build it)')
  backends = []
  foreach name : get_option('backends')
    if name != 'realvnc'
      backends += name
    endif
  endforeach
endif

vento_inc = include_directories('api/include')

if android
  subdir('platform/android')
else
  subdir('platform/linux')
endif

vnc = 'libvnc' in backends or 'tigervnc' in backends

# TLS: GnuTLS for both VNC clients (and nettle under it, which TigerVNC also
# calls for its RSA-AES security types), OpenSSL for FreeRDP. Android compiles
# the sources with configure's answers committed; Linux runs upstream's own
# build at build time.
if android
  crypto_ports = 'platform/android/ports/'
else
  crypto_ports = 'platform/linux/ports/'
endif
if vnc
  subdir(crypto_ports / 'nettle')
  subdir(crypto_ports / 'gnutls')
endif
if 'freerdp' in backends
  subdir(crypto_ports / 'openssl')
endif

# libjpeg-turbo, for both VNC clients' Tight encoding.
if vnc
  subdir('ports/libjpeg-turbo')
endif

if 'libvnc' in backends
  subdir('ports/libvncserver')
  subdir('modules/libvnc/cpp')
endif
if 'tigervnc' in backends
  subdir('ports/pixman')
  subdir('ports/tigervnc')
  subdir('modules/tigervnc/cpp')
endif
if 'freerdp' in backends
  subdir('ports/freerdp')
  subdir('modules/freerdp/cpp')
endif
if 'realvnc' in backends
  if not android
    subdir('platform/linux/ports/realvnc-host')
  endif
  subdir('modules/realvnc/cpp')
endif
