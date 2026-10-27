# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# OpenSSL's libcrypto and libssl for Android as static libraries, from the
# submodule in third_party/openssl and the Configure output in gen/ (gen.sh
# writes it). One archive per archive upstream builds (the two libraries and
# two provider archives), because the definitions are per archive: the
# providers are compiled without most of the assembly's macros, and a file
# that reads one it was not compiled for calls into a kernel never selected.

openssl_dir = '../../../../../third_party/openssl'
if not fs.exists(openssl_dir / 'Configure')
  error('third_party/openssl is empty: run `git submodule update --init`')
endif

# Configure decides the word sizes and the assembly, and with them which C
# files are compiled, so most of gen/ is per ABI. A file both ABIs generated
# the same is in gen/ once.
gen_abi = 'gen' / {'aarch64': 'arm64-v8a', 'x86_64': 'x86_64'}[cpu]
if not fs.exists(gen_abi / 'meson.build')
  error('no OpenSSL configuration in @0@; add the ABI to gen.sh'.format(gen_abi))
endif
subdir(gen_abi) # openssl_archives: archive -> definitions, sources, generated sources

# Upstream's include path: a generated header shadows the tree's template.
openssl_inc_dirs = []
foreach dir : [gen_abi, 'gen', gen_abi / 'include', 'gen/include', gen_abi / 'crypto', 'gen/crypto',
               gen_abi / 'providers/common/include', 'gen/providers/common/include',
               openssl_dir, openssl_dir / 'include', openssl_dir / 'crypto',
               openssl_dir / 'crypto/modes', openssl_dir / 'providers/common/include',
               openssl_dir / 'providers/implementations/include',
               openssl_dir / 'providers/fips/include']
  if fs.is_dir(dir)
    openssl_inc_dirs += dir
  endif
endforeach

openssl_args = vendored_args + [
  '-DOPENSSL_BUILDING_OPENSSL', '-DOPENSSL_PIC', '-DOPENSSL_USE_NODELETE',
  # Where providers are loaded from and the trust store lives: paths that do
  # not exist on Android. No module is loadable in an app's sandbox, and the
  # app verifies certificates itself.
  '-DOPENSSLDIR="/etc/ssl"', '-DENGINESDIR="/nonexistent"', '-DMODULESDIR="/nonexistent"',
  # FreeRDP's library exports its own API, not OpenSSL's.
  '-fvisibility=hidden',
]

openssl_libs = []
foreach archive, lists : openssl_archives
  sources = []
  foreach f : lists['sources']
    sources += files(openssl_dir / f)
  endforeach
  foreach f : lists['generated']
    sources += files(fs.exists(gen_abi / f) ? gen_abi / f : 'gen' / f)
  endforeach
  defines = []
  foreach name : lists['defines']
    defines += '-D' + name
  endforeach
  openssl_libs += static_library('openssl_' + archive, sources,
    include_directories: include_directories(openssl_inc_dirs),
    c_args: openssl_args + defines)
endforeach

# The default provider is reached only through a table in libcrypto; lld
# resolves symbols across archives whatever their order.
openssl_public_inc = []
foreach dir : [gen_abi / 'include', 'gen/include', openssl_dir / 'include']
  if fs.is_dir(dir)
    openssl_public_inc += dir
  endif
endforeach
openssl_dep = declare_dependency(link_with: openssl_libs,
  include_directories: include_directories(openssl_public_inc, is_system: true))
