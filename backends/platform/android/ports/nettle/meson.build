# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# nettle and hogweed for Android as static libraries, from the release tarball
# in third_party/nettle and the configure and generator output in gen/
# (gen.sh writes it), so that an app build runs no configure and needs no m4.
# Both libraries are built whole: an archive contributes only the members
# something references.

nettle_dir = '../../../../../third_party/nettle'
if not fs.exists(nettle_dir / 'nettle-types.h')
  error('third_party/nettle is empty: run `./gradlew :backends:fetchCryptoSources`')
endif

subdir('gen')            # nettle_sources, hogweed_sources, nettle_headers
subdir('include/nettle') # nettle_public_headers: <nettle/...>, as upstream installs them

nettle_private_inc = include_directories('gen', nettle_dir)
nettle_public_inc = include_directories('include', is_system: true)
nettle_args = vendored_args + ['-DHAVE_CONFIG_H']

nettle_files = []
foreach f : nettle_sources
  nettle_files += files(nettle_dir / f)
endforeach
hogweed_files = []
foreach f : hogweed_sources
  hogweed_files += files(nettle_dir / f)
endforeach

nettle_lib = static_library('nettle', nettle_files, nettle_public_headers,
  include_directories: nettle_private_inc,
  c_args: nettle_args)
nettle_dep = declare_dependency(link_with: nettle_lib, sources: nettle_public_headers,
  include_directories: nettle_public_inc)

# Under --enable-mini-gmp configure leaves mini-gmp's conversions between an
# mpz and a double out through CFLAGS, which gen/ cannot carry.
hogweed_lib = static_library('hogweed', hogweed_files, nettle_public_headers,
  include_directories: nettle_private_inc,
  c_args: nettle_args + ['-DMINI_GMP_ENABLE_FLOAT=0'])
hogweed_dep = declare_dependency(link_with: hogweed_lib, dependencies: nettle_dep)
