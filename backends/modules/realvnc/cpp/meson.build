# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The RealVNC backend's native code. On Android, libremotedesktop_realvnc.so
# is the estimator read (realvnc_traffic.c) and the hooks that put the
# session's network under the core's connections (realvnc_net.c,
# realvnc_got.c). It links against nothing of the core's: the viewer core is
# already in the process, the read goes through a pointer from the core's own
# Java, and the hooks find the core through the dynamic linker. It ships in
# the RealVNC add-on. On Linux,
# libvento_realvnc.so is the session in realvnc_session.cxx over the hosting
# layer in host/, which runs the core's Android build in a glibc process
# (backends/platform/linux/ports/realvnc-host), with a probe and the tests.
# It needs the two libraries from RealVNC's viewer at run time, and nothing of
# RealVNC's to build.

if android
  # The session's network the hooks dial through is this module's crate as a
  # static library (nativeLibrary in backends/build.gradle). Only the JNI
  # entry points are exported (exports.map); the NDK libraries the crate's
  # archive brings that the library does not use are dropped.
  shared_library('remotedesktop_realvnc', 'realvnc_traffic.c', 'realvnc_net.c', 'realvnc_got.c',
    android_log_c,
    include_directories: vento_inc,
    c_args: warning_args,
    link_args: ['-Wl,--version-script=' + meson.current_source_dir() / 'exports.map',
                '-Wl,--gc-sections', '-Wl,--as-needed'],
    link_depends: files('exports.map'),
    dependencies: [cc.find_library('remotedesktop_realvnc', dirs: crate_library_dir, static: true)]
                  + backend_deps)
else
  # The pins of the core build the declarations were read from, from the one
  # place they are stated: Library.java. The session refuses any other build.
  library_java = fs.read('../../../../realvnc/src/main/java/com/realvnc/vncviewer/jni/Library.java')
  pins = configuration_data()
  pins.set_quoted('VENTO_REALVNC_VERSION', library_java.split('VERSION = "')[1].split('"')[0])
  foreach abi : ['arm64-v8a', 'x86_64']
    foreach lib : ['libvncviewer', 'libc++_shared']
      key = '"@0@/@1@.so",'.format(abi, lib)
      if not library_java.contains(key)
        error('no SHA-256 for @0@/@1@.so in Library.java'.format(abi, lib))
      endif
      # The first quoted string after the key: 64 hexadecimal digits.
      hash = library_java.split(key)[1].split('"')[1]
      if hash.strip('0123456789abcdef') != '' or hash.substring(63) == '' or hash.substring(64) != ''
        error('no SHA-256 for @0@/@1@.so in Library.java'.format(abi, lib))
      endif
      name = '@0@_@1@'.format(abi, lib).to_upper().replace('-', '_').replace('++', 'XX')
      pins.set_quoted('VENTO_REALVNC_SHA256_' + name, hash)
    endforeach
  endforeach
  realvnc_pins_h = configure_file(output: 'realvnc_pins.h', configuration: pins)

  realvnc_inc = [vento_inc, include_directories('.', 'include', 'host')]
  # Linked without -Bsymbolic: the client opens this library in the global
  # scope, for the core it loads.
  realvnc_lib = shared_library('vento_realvnc', 'realvnc_session.cxx', 'realvnc_net.c', 'host/DesktopCore.cxx',
    'host/Loader.cxx', 'host/Looper.cxx', 'host/Classes.cxx', 'host/Files.cxx', realvnc_pins_h,
    linux_log_c,
    include_directories: realvnc_inc,
    c_args: warning_args,
    cpp_args: warning_args,
    link_args: ['-Wl,--exclude-libs,ALL'],
    dependencies: realvnc_host_dep)
  if get_option('probes')
    executable('vento_realvnc_probe', 'realvnc_probe.cxx', probe_c,
      include_directories: [realvnc_inc, probe_inc],
      c_args: warning_args,
      cpp_args: warning_args,
      link_with: realvnc_lib)
    # The hosting layer's resolver wrappers, over libc-shim's resolver.
    test('vento_realvnc_test', executable('vento_realvnc_test', 'realvnc_test.cxx',
      include_directories: include_directories('host'),
      cpp_args: warning_args,
      dependencies: libc_shim_dep))
    # The hooks under the core's connections (realvnc_net.c), driven as the
    # core calls them, over a fake network.
    test('vento_realvnc_net_test', executable('vento_realvnc_net_test', 'realvnc_net_test.c',
      'realvnc_net.c',
      include_directories: [vento_inc, include_directories('.')],
      c_args: warning_args,
      dependencies: threads_dep))
  endif
endif
