# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# libjpeg-turbo, for the Tight encoding of both VNC clients: the libjpeg API
# (not TurboJPEG) as a static library, with 8-, 12- and 16-bit samples and
# arithmetic coding, as upstream builds it by default.
#
# SIMD: on aarch64, upstream's Neon intrinsics, which are C. On x86_64 Linux,
# upstream's assembly where nasm is installed, and C without it. On x86_64
# Android, C only, so that the APK does not depend on what the build machine
# has installed.

jpeg_dir = '../../../third_party/libjpeg-turbo'
if not fs.exists(jpeg_dir / 'src/jpeglib.h')
  error('third_party/libjpeg-turbo is empty: run `git submodule update --init`')
endif
jpeg_cmake = fs.read(jpeg_dir / 'CMakeLists.txt')
jpeg_version = jpeg_cmake.split('\nset(VERSION ')[1].split(')')[0]
jpeg_v = jpeg_version.split('.')

if cpu == 'aarch64'
  jpeg_simd = 'ARM64'
elif not android and add_languages('nasm', required: false, native: false)
  jpeg_simd = 'X86_64'
else
  jpeg_simd = 'NONE'
endif

jpeg_conf = configuration_data({
  'JPEG_LIB_VERSION': 62,
  'VERSION': jpeg_version,
  'LIBJPEG_TURBO_VERSION_NUMBER':
    jpeg_v[0].to_int() * 1000000 + jpeg_v[1].to_int() * 1000 + jpeg_v[2].to_int(),
  'COPYRIGHT_YEAR': jpeg_cmake.split('set(COPYRIGHT_YEAR "')[1].split('"')[0],
  'C_ARITH_CODING_SUPPORTED': true,
  'D_ARITH_CODING_SUPPORTED': true,
  'WITH_SIMD': jpeg_simd != 'NONE',
  'SIMD_ARCHITECTURE': jpeg_simd,
  'RIGHT_SHIFT_IS_UNSIGNED': false,
  # Upstream defaults this to the build date, which ends up in a version string.
  'BUILD': '0',
  'CMAKE_PROJECT_NAME': 'libjpeg-turbo',
  'HIDDEN': '__attribute__((visibility("hidden")))',
  'INLINE': '__inline__ __attribute__((always_inline))',
  'THREAD_LOCAL': '__thread',
  'SIZE_T': 8,
  'HAVE_BUILTIN_CTZL': true,
  'HAVE_INTRIN_H': false,
  'WITH_PROFILE': false,
})
jpeg_headers = []
foreach h : ['jconfig.h', 'jconfigint.h', 'jversion.h']
  jpeg_headers += configure_file(input: jpeg_dir / 'src' / h + '.in', output: h,
    configuration: jpeg_conf, format: 'cmake@')
endforeach

jpeg_sources = files(
  jpeg_dir / 'src/jcapimin.c', jpeg_dir / 'src/jchuff.c', jpeg_dir / 'src/jcicc.c',
  jpeg_dir / 'src/jcinit.c', jpeg_dir / 'src/jclhuff.c', jpeg_dir / 'src/jcmarker.c',
  jpeg_dir / 'src/jcmaster.c', jpeg_dir / 'src/jcomapi.c', jpeg_dir / 'src/jcparam.c',
  jpeg_dir / 'src/jcphuff.c', jpeg_dir / 'src/jctrans.c', jpeg_dir / 'src/jdapimin.c',
  jpeg_dir / 'src/jdatadst.c', jpeg_dir / 'src/jdatasrc.c', jpeg_dir / 'src/jdhuff.c',
  jpeg_dir / 'src/jdicc.c', jpeg_dir / 'src/jdinput.c', jpeg_dir / 'src/jdlhuff.c',
  jpeg_dir / 'src/jdmarker.c', jpeg_dir / 'src/jdmaster.c', jpeg_dir / 'src/jdphuff.c',
  jpeg_dir / 'src/jdtrans.c', jpeg_dir / 'src/jerror.c', jpeg_dir / 'src/jfdctflt.c',
  jpeg_dir / 'src/jmemmgr.c', jpeg_dir / 'src/jmemnobs.c', jpeg_dir / 'src/jpeg_nbits.c',
  # Arithmetic coding.
  jpeg_dir / 'src/jaricom.c', jpeg_dir / 'src/jcarith.c', jpeg_dir / 'src/jdarith.c',
)
# Upstream compiles these once per sample size, through one wrapper file each.
foreach name : ['jcapistd', 'jccolor', 'jcdiffct', 'jclossls', 'jcmainct', 'jcprepct',
                'jcsample', 'jdapistd', 'jdcolor', 'jddiffct', 'jdlossls', 'jdmainct',
                'jdpostct', 'jdsample', 'jutils']
  jpeg_sources += files(jpeg_dir / 'src/wrapper' / name + '-8.c',
                        jpeg_dir / 'src/wrapper' / name + '-12.c',
                        jpeg_dir / 'src/wrapper' / name + '-16.c')
endforeach
foreach name : ['jccoefct', 'jcdctmgr', 'jdcoefct', 'jddctmgr', 'jdmerge', 'jfdctfst',
                'jfdctint', 'jidctflt', 'jidctfst', 'jidctint', 'jidctred', 'jquant1',
                'jquant2']
  jpeg_sources += files(jpeg_dir / 'src/wrapper' / name + '-8.c',
                        jpeg_dir / 'src/wrapper' / name + '-12.c')
endforeach

jpeg_simd_inc = []
if jpeg_simd == 'ARM64'
  jpeg_sources += files(jpeg_dir / 'simd/jsimd.c',
    jpeg_dir / 'simd/arm/jccolor-neon.c', jpeg_dir / 'simd/arm/jcgray-neon.c',
    jpeg_dir / 'simd/arm/aarch64/jchuff-neon.c', jpeg_dir / 'simd/arm/jcphuff-neon.c',
    jpeg_dir / 'simd/arm/jcsample-neon.c', jpeg_dir / 'simd/arm/jdcolor-neon.c',
    jpeg_dir / 'simd/arm/jdmerge-neon.c', jpeg_dir / 'simd/arm/jdsample-neon.c',
    jpeg_dir / 'simd/arm/jfdctfst-neon.c', jpeg_dir / 'simd/arm/jfdctint-neon.c',
    jpeg_dir / 'simd/arm/jidctfst-neon.c', jpeg_dir / 'simd/arm/jidctint-neon.c',
    jpeg_dir / 'simd/arm/jidctred-neon.c', jpeg_dir / 'simd/arm/jquanti-neon.c',
    jpeg_dir / 'simd/arm/aarch64/jsimdcpu.c')
  # Which Neon load intrinsics the compiler has: all three, in every clang
  # and GCC this builds with.
  jpeg_headers += configure_file(input: jpeg_dir / 'simd/arm/neon-compat.h.in',
    output: 'neon-compat.h', format: 'cmake@', configuration: configuration_data({
      'HAVE_VLD1_S16_X3': true, 'HAVE_VLD1_U16_X2': true, 'HAVE_VLD1Q_U8_X4': true,
      'WITH_SIMDE': false}))
elif jpeg_simd == 'X86_64'
  jpeg_sources += files(jpeg_dir / 'simd/jsimd.c')
  foreach name : ['jsimdcpu', 'jfdctflt-sse', 'jccolor-sse2', 'jcgray-sse2', 'jchuff-sse2',
                  'jcphuff-sse2', 'jcsample-sse2', 'jdcolor-sse2', 'jdmerge-sse2',
                  'jdsample-sse2', 'jfdctfst-sse2', 'jfdctint-sse2', 'jidctflt-sse2',
                  'jidctfst-sse2', 'jidctint-sse2', 'jidctred-sse2', 'jquantf-sse2',
                  'jquanti-sse2', 'jccolor-avx2', 'jcgray-avx2', 'jcsample-avx2',
                  'jdcolor-avx2', 'jdmerge-avx2', 'jdsample-avx2', 'jfdctint-avx2',
                  'jidctint-avx2', 'jquanti-avx2']
    jpeg_sources += files(jpeg_dir / 'simd/x86_64' / name + '.asm')
  endforeach
  # Include directories apply to nasm too: its macros are in these two.
  jpeg_simd_inc = include_directories(jpeg_dir / 'simd/nasm', jpeg_dir / 'simd/x86_64')
endif

jpeg_lib = static_library('jpeg', jpeg_sources, jpeg_headers,
  include_directories: [include_directories('.', jpeg_dir / 'src'), jpeg_simd_inc],
  c_args: vendored_args,
  nasm_args: ['-DPIC'])
jpeg_dep = declare_dependency(link_with: jpeg_lib, sources: jpeg_headers,
  include_directories: include_directories('.', jpeg_dir / 'src', is_system: true))
