# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The libvncclient backend: libvnc_session.c, the shim over libvncclient. On
# Android it is libremotedesktop_libvnc.so, with the module's Rust crate
# linked in; on Linux it is libvento_libvnc.so, with a probe.

libvnc_inc = [vento_inc, include_directories('include')]

if android
  shared_library('remotedesktop_libvnc', 'libvnc_session.c', android_log_c,
    include_directories: libvnc_inc,
    c_args: warning_args,
    link_args: backend_link_args,
    link_depends: backend_link_depends,
    dependencies: [vncclient_dep,
                   cc.find_library('remotedesktop_libvnc', dirs: crate_library_dir, static: true)]
                  + backend_deps)
else
  libvnc_lib = shared_library('vento_libvnc', 'libvnc_session.c', linux_log_c,
    include_directories: libvnc_inc,
    c_args: warning_args,
    link_args: backend_link_args,
    dependencies: vncclient_dep)
  if get_option('probes')
    executable('vento_libvnc_probe', 'libvnc_probe.c', probe_c,
      include_directories: [libvnc_inc, probe_inc],
      c_args: warning_args,
      link_with: libvnc_lib)
  endif
endif
