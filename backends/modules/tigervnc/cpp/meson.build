# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The TigerVNC backend: tigervnc_session.cxx, the shim over TigerVNC's client,
# and H264VentoDecoderContext.cxx, which TigerVNC's H.264 decoder creates its
# context through (backends/ports/tigervnc says why it is here). H.264 and
# sound go to the tables the session is given (VentoVideo.h, VentoAudio.h):
# MediaCodec and AAudio on Android through the crate, the Linux client's
# FFmpeg and its PipeWire or ALSA output on Linux. The probe passes neither.
# On Android it is libremotedesktop_tigervnc.so, with the module's Rust crate
# linked in; on Linux it is libvento_tigervnc.so, with a probe.

tigervnc_module_inc = [vento_inc, include_directories('.', 'include')]
# gnutls as well as TigerVNC: the session checks the certificate itself.
tigervnc_module_deps = [tigervnc_dep, gnutls_dep]
# An archive linked whole, so that its symbols are hidden with the vendored
# libraries' on Linux (--exclude-libs) whatever the link order.
tigervnc_h264_lib = static_library('vento_tigervnc_h264', 'H264VentoDecoderContext.cxx',
  include_directories: tigervnc_module_inc,
  cpp_args: warning_args,
  dependencies: tigervnc_dep)

if android
  shared_library('remotedesktop_tigervnc', 'tigervnc_session.cxx', android_log_c,
    include_directories: tigervnc_module_inc,
    c_args: warning_args,
    cpp_args: warning_args,
    link_args: backend_link_args,
    link_depends: backend_link_depends,
    link_whole: tigervnc_h264_lib,
    dependencies: tigervnc_module_deps
                  + [cc.find_library('remotedesktop_tigervnc', dirs: crate_library_dir, static: true)]
                  + backend_deps)
else
  tigervnc_lib = shared_library('vento_tigervnc', 'tigervnc_session.cxx', linux_log_c,
    include_directories: tigervnc_module_inc,
    c_args: warning_args,
    cpp_args: warning_args,
    link_args: backend_link_args,
    link_whole: tigervnc_h264_lib,
    dependencies: tigervnc_module_deps)
  if get_option('probes')
    executable('vento_tigervnc_probe', 'tigervnc_probe.cxx', probe_c,
      include_directories: [tigervnc_module_inc, probe_inc],
      c_args: warning_args,
      cpp_args: warning_args,
      link_with: tigervnc_lib)
  endif
endif
