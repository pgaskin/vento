# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# pixman, which TigerVNC's core::Region wraps: the generic C only. The SIMD
# sources are compositing fast paths, which a region does not use.

pixman_dir = '../../../third_party/pixman'
if not fs.exists(pixman_dir / 'pixman/pixman.h')
  error('third_party/pixman is empty: run `git submodule update --init`')
endif
pixman_v = fs.read(pixman_dir / 'meson.build').split('version : \'')[1].split('\'')[0].split('.')

pixman_headers = [
  configure_file(input: pixman_dir / 'pixman/pixman-version.h.in', output: 'pixman-version.h',
    configuration: configuration_data({
      'PIXMAN_VERSION_MAJOR': pixman_v[0],
      'PIXMAN_VERSION_MINOR': pixman_v[1],
      'PIXMAN_VERSION_MICRO': pixman_v[2],
    })),
  # Of upstream's configure answers, the two the sources need: a package name,
  # and how the compiler spells thread-local storage, without which
  # pixman-compiler.h stops the build. Not the constructor attribute: with it,
  # pixman picks its implementation when the library is loaded, which keeps
  # every compositing path from being garbage-collected (some 350 kB on
  # aarch64), where a region needs none of them.
  configure_file(output: 'pixman-config.h', configuration: configuration_data({
    'PACKAGE': '"pixman"',
    'TLS': '__thread',
  })),
]

pixman_sources = []
foreach name : ['pixman', 'pixman-access', 'pixman-access-accessors', 'pixman-arm',
                'pixman-bits-image', 'pixman-combine32', 'pixman-combine-float',
                'pixman-conical-gradient', 'pixman-edge', 'pixman-edge-accessors',
                'pixman-fast-path', 'pixman-filter', 'pixman-glyph', 'pixman-general',
                'pixman-gradient-walker', 'pixman-image', 'pixman-implementation',
                'pixman-linear-gradient', 'pixman-matrix', 'pixman-mips', 'pixman-noop',
                'pixman-ppc', 'pixman-radial-gradient', 'pixman-region16', 'pixman-region32',
                'pixman-region64f', 'pixman-riscv', 'pixman-solid-fill', 'pixman-timer',
                'pixman-trap', 'pixman-utils', 'pixman-x86']
  pixman_sources += files(pixman_dir / 'pixman' / name + '.c')
endforeach

pixman_lib = static_library('pixman', pixman_sources, pixman_headers,
  include_directories: include_directories('.', pixman_dir / 'pixman'),
  c_args: vendored_args + ['-DHAVE_CONFIG_H'])
pixman_dep = declare_dependency(link_with: pixman_lib, sources: pixman_headers,
  include_directories: include_directories('.', pixman_dir / 'pixman', is_system: true))
