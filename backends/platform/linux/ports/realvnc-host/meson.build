# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The four libraries RealVNC's hosting layer (backends/modules/realvnc/cpp/host)
# runs the viewer core's Android build on, in a glibc process. Linux only, and
# clang only: libc-shim does not compile with GCC (its vector::insert
# overloads trip -Wchanges-meaning, as of GCC 16). They are in no APK and on no
# licence page, and each is pinned at the commit the hosting layer was tested
# with:
#
#   logger               minecraft-linux/logger          The Unlicense (public domain)
#   libc-shim            minecraft-linux/libc-shim       no licence file; GitHub lists none.
#                                                        Two bionic-derived headers in it carry
#                                                        AOSP's Apache-2.0 and BSD notices.
#   mcpelauncher-linker  minecraft-linux/mcpelauncher-linker  MIT (its own files); its nested
#                                                        `bionic` and `core` are AOSP, Apache-2.0
#                                                        and BSD
#   libjnivm             ChristopherHX/libjnivm          MIT
#
# Their CMakeLists are a source list and a few definitions each, written down
# here with upstream's defaults for every option but the trace.

tp = '../../../../../third_party'
foreach name : ['logger', 'libc-shim', 'mcpelauncher-linker', 'libjnivm']
  if not fs.exists(tp / name / 'CMakeLists.txt')
    error('third_party/@0@ is empty: run `git submodule update --init --recursive`'.format(name))
  endif
endforeach
if not fs.exists(tp / 'mcpelauncher-linker/bionic/linker/linker.cpp')
  error('third_party/mcpelauncher-linker\'s own submodules are empty: '
    + 'run `git submodule update --init --recursive`')
endif

logger_inc = include_directories(tp / 'logger/include')
logger_lib = static_library('logger', tp / 'logger/src/log.cpp',
  include_directories: logger_inc,
  cpp_args: vendored_args)

# libc-shim's 24-byte atomics need libatomic with some compilers, which
# upstream checks for. Its static archive where the compiler has one, so that
# the library runs where libatomic.so.1 is not installed.
libc_shim_deps = []
atomics_code = '''#include <atomic>
struct A { long long a; void *wrapper; long long b; };
int main() {
    std::atomic<A> a;
    auto a2 = a.load();
    auto a3 = a2;
    a3.wrapper = &a;
    return a.compare_exchange_strong(a2, a3, std::memory_order_relaxed, std::memory_order_relaxed) ? 0 : 1;
}'''
if not cxx.links(atomics_code, name: '24-byte atomics without libatomic')
  libc_shim_deps += cxx.find_library('atomic', static: true, required: false)
  if not libc_shim_deps[0].found()
    libc_shim_deps = [cxx.find_library('atomic')]
  endif
endif
shim = tp / 'libc-shim/src'
libc_shim_lib = static_library('libc-shim',
  shim / 'common.cpp', shim / 'pthreads.cpp', shim / 'semaphore.cpp', shim / 'network.cpp',
  shim / 'dirent.cpp', shim / 'cstdio.cpp', shim / 'errno.cpp', shim / 'ctype_data.cpp',
  shim / 'bionic/strlcpy.cpp', shim / 'stat.cpp', shim / 'file_misc.cpp', shim / 'sysconf.cpp',
  shim / 'system_properties.cpp', shim / 'iorewrite.cpp', shim / 'statvfs.cpp', shim / 'sched.cpp',
  include_directories: [include_directories(tp / 'libc-shim/include'), logger_inc],
  cpp_args: vendored_args,
  link_with: logger_lib,
  dependencies: libc_shim_deps)
libc_shim_dep = declare_dependency(link_with: libc_shim_lib,
  include_directories: include_directories(tp / 'libc-shim/include', is_system: true))

linker_dir = tp / 'mcpelauncher-linker'
linker_sources = []
foreach f : ['bionic/linker/rt.cpp', 'bionic/linker/linker_gdb_support.cpp',
             'bionic/libc/bionic/bionic_call_ifunc_resolver.cpp',
             'bionic/linker/linker_dlwarning.cpp', 'bionic/linker/dlfcn.cpp',
             'bionic/linker/linker_phdr.cpp', 'bionic/linker/linker_soinfo.cpp',
             'bionic/linker/linker.cpp', 'bionic/linker/linker_config.cpp',
             'bionic/linker/linker_utils.cpp', 'bionic/linker/linker_debug.cpp',
             'bionic/linker/linker_block_allocator.cpp',
             'bionic/linker/linker_mapped_file_fragment.cpp', 'bionic/linker/linker_relocate.cpp',
             'bionic/linker/linker_namespaces.cpp', 'core/base/mapped_file.cpp',
             'bionic/linker/linker_globals.cpp', 'bionic/linker/linker_main.cpp',
             'bionic/linker/linker_cfi.cpp', 'bionic/linker/linker_sdk_versions.cpp',
             'bionic/linker/linker_logger.cpp', 'core/base/file.cpp', 'core/base/logging.cpp',
             'core/base/liblog_symbols.cpp', 'bionic/libc/async_safe/async_safe_log.cpp',
             'core/base/stringprintf.cpp', 'core/base/strings.cpp', 'core/liblog/logger_write.cpp',
             'core/liblog/properties.cpp', 'core/base/threads.cpp', 'core/base/properties.cpp',
             'core/base/parsebool.cpp', 'src/zip_archive_stream_entry.cc',
             'core/libziparchive/zip_archive.cc', 'src/linker.cpp', 'bionic/libdl/libdl.cpp',
             'bionic/libc/upstream-openbsd/lib/libc/string/strlcpy.c',
             'bionic/libc/upstream-openbsd/lib/libc/string/strlcat.c']
  linker_sources += files(linker_dir / f)
endforeach
if cpu == 'aarch64'
  linker_sources += files(linker_dir / 'bionic/linker/arch/arm_neon/linker_gnu_hash_neon.cpp')
endif
linker_args = vendored_args + ['-DPATH_MAX=256', '-D_GNU_SOURCE', '-include', 'compat.h']
linker_lib = static_library('linker', linker_sources,
  include_directories: include_directories(linker_dir / 'include', linker_dir / 'core/base/include',
    linker_dir / 'core/liblog/include', linker_dir / 'core/libcutils/include',
    linker_dir / 'public_include'),
  c_args: linker_args,
  cpp_args: linker_args,
  dependencies: [zlib_dep, threads_dep])

# jnivm's garbage collection and debug code generation stay at upstream's
# defaults, which the hosting layer was tested with. Both are public
# definitions upstream, so they are its consumers' too. The debug code
# generation prints the `Call Unknown ... Signature=...` line that says what
# a stand-in in host/Classes.cxx lacks; the trace (realvnc_trace) prints every
# JNI call the core makes, several thousand lines a session.
jnivm_dir = tp / 'libjnivm'
jnivm_public_args = ['-DEnableJNIVMGC', '-DJNI_DEBUG']
jnivm_args = vendored_args + jnivm_public_args + ['-DHAVE_LOGGER', '-DJNIVM_FAKE_JNI_SYNTAX=0']
if get_option('realvnc_trace')
  jnivm_args += '-DJNI_TRACE'
endif
jnivm_sources = []
foreach name : ['internal/array', 'internal/bytebuffer', 'internal/field', 'internal/method',
                'internal/string', 'internal/stringUtil', 'internal/findclass',
                'internal/jValuesfromValist', 'internal/skipJNIType', 'env', 'method', 'vm',
                'object', 'internal/codegen/class', 'internal/codegen/field',
                'internal/codegen/method', 'internal/codegen/namespace',
                'internal/codegen/parseJNIType', 'internal/codegen/vm']
  jnivm_sources += files(jnivm_dir / 'src/jnivm' / name + '.cpp')
endforeach
jnivm_lib = static_library('jnivm', jnivm_sources,
  include_directories: [include_directories(jnivm_dir / 'include'), logger_inc],
  cpp_args: jnivm_args,
  override_options: ['cpp_std=gnu++14'],
  link_with: logger_lib,
  dependencies: dl_dep)

realvnc_host_dep = declare_dependency(
  link_with: [linker_lib, libc_shim_lib, logger_lib, jnivm_lib],
  compile_args: jnivm_public_args,
  include_directories: include_directories(jnivm_dir / 'include', tp / 'libc-shim/include',
    linker_dir / 'public_include', is_system: true),
  dependencies: [dl_dep, threads_dep, zlib_dep])
