# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# GnuTLS for Android as static libraries, from the release tarball in
# third_party/gnutls and the configure output in gen/ (gen.sh writes it). One
# archive per upstream Makefile, since the include path is what differs
# between them.

gnutls_dir = '../../../../../third_party/gnutls'
if not fs.exists(gnutls_dir / 'lib/global.c')
  error('third_party/gnutls is empty: run `./gradlew :backends:fetchCryptoSources`')
endif
# A few answers depend on the ABI (the width of a wchar_t, the layout of a long
# double), so the files holding them are per ABI, first on the path.
gen_abi = 'gen' / {'aarch64': 'arm64-v8a', 'x86_64': 'x86_64'}[cpu]
if not fs.exists(gen_abi / 'config.h')
  error('no GnuTLS configuration in @0@; add the ABI to gen.sh'.format(gen_abi))
endif
subdir('gen') # gnutls_sources: Makefile directory -> sources

# The include path of each Makefile, as upstream's has it.
gnutls_lib_inc = [gen_abi, 'gen', 'gen/gl', gnutls_dir / 'gl', 'gen/lib/includes',
                  gnutls_dir / 'lib/includes', gnutls_dir / 'lib', gnutls_dir / 'lib/x509',
                  gnutls_dir / 'lib/minitasn1', 'gen/lib/unistring', gnutls_dir / 'lib/unistring']
gnutls_parts = {
  # gnulib and the bundled unistring are compiled against themselves alone,
  # which keeps gnulib's replacement headers (a stdint.h, a string.h) from
  # reaching anything else.
  'gl': {'inc': [gen_abi, 'gen', gen_abi / 'gl', 'gen/gl', gnutls_dir / 'gl']},
  'lib/unistring': {'inc': [gen_abi, 'gen', gen_abi / 'lib/unistring', 'gen/lib/unistring',
                            gnutls_dir / 'lib/unistring']},
  # The bundled libtasn1, which parses certificates.
  'lib/minitasn1': {'inc': [gen_abi, 'gen', 'gen/gl', gnutls_dir / 'gl', gnutls_dir / 'lib',
                            gnutls_dir / 'lib/minitasn1'],
                    'args': ['-DASN1_BUILDING=1']},
}
foreach part : ['lib', 'lib/accelerated', 'lib/algorithms', 'lib/auth', 'lib/ext', 'lib/extras',
                'lib/hpke', 'lib/nettle', 'lib/x509']
  # Each Makefile's own directory first, then the shared set.
  inc = [gnutls_dir / part] + gnutls_lib_inc
  args = ['-DGNUTLS_BUILDING_LIB=1']
  if part == 'lib/nettle'
    # The integer helpers, and the backports of what a newer nettle has.
    inc += [gnutls_dir / 'lib/nettle/int', gnutls_dir / 'lib/nettle/backport']
  elif part == 'lib'
    # The administrator's TLS defaults, at a path that does not exist on
    # Android.
    args += ['-DSYSTEM_PRIORITY_FILE="/etc/gnutls/config"', '-DLOCALEDIR="/usr/share/locale"']
  endif
  gnutls_parts += {part: {'inc': inc, 'args': args, 'deps': [nettle_dep, hogweed_dep]}}
endforeach

gnutls_libs = []
foreach part, sources : gnutls_sources
  config = gnutls_parts[part]
  inc = []
  foreach dir : config['inc']
    if fs.is_dir(dir)
      inc += dir
    endif
  endforeach
  part_sources = []
  foreach f : sources
    part_sources += files(gnutls_dir / f)
  endforeach
  gnutls_libs += static_library('gnutls_' + part.replace('/', '_'), part_sources,
    include_directories: include_directories(inc),
    c_args: vendored_args + ['-DHAVE_CONFIG_H'] + config.get('args', []),
    dependencies: config.get('deps', []))
endforeach

gnutls_dep = declare_dependency(link_with: gnutls_libs,
  include_directories: include_directories('gen/lib/includes', gnutls_dir / 'lib/includes',
                                           is_system: true),
  dependencies: [hogweed_dep, nettle_dep])
