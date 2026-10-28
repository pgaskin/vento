# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The FreeRDP backend: freerdp_session.c, the shim over FreeRDP's client, with
# h264_vento.c, the H.264 subsystem that hands FreeRDP's pictures to vento's
# decoders, and rdpsnd_vento.c, the sound device that plays through vento's
# output and decoders (backends/ports/freerdp says how both are wired in). On
# Android it is libremotedesktop_freerdp.so, with the module's Rust crate
# linked in; on Linux it is libvento_freerdp.so, with a probe and the tests.

freerdp_module_sources = files('freerdp_session.c', 'h264_vento.c', 'rdpsnd_vento.c')
freerdp_module_inc = [vento_inc, include_directories('include'), freerdp_codec_inc]

if android
  shared_library('remotedesktop_freerdp', freerdp_module_sources, android_log_c,
    include_directories: freerdp_module_inc,
    c_args: warning_args,
    link_args: backend_link_args,
    link_depends: backend_link_depends,
    dependencies: [freerdp_dep,
                   cc.find_library('remotedesktop_freerdp', dirs: crate_library_dir, static: true)]
                  + backend_deps)
else
  freerdp_lib = shared_library('vento_freerdp', freerdp_module_sources, linux_log_c,
    include_directories: freerdp_module_inc,
    # WLog's lines through vento_log, rather than its console appender.
    c_args: warning_args + ['-DVENTO_FREERDP_WLOG'],
    link_args: backend_link_args,
    link_depends: backend_link_depends,
    dependencies: freerdp_dep)
  if get_option('probes')
    executable('vento_freerdp_probe', 'freerdp_probe.c', probe_c,
      include_directories: [freerdp_module_inc, probe_inc],
      c_args: warning_args,
      link_with: freerdp_lib)
    # The session's pure parts, which the test includes rather than links,
    # with the two input calls the keys go through wrapped.
    test('vento_freerdp_test', executable('vento_freerdp_test', 'freerdp_test.c', linux_log_c,
      include_directories: freerdp_module_inc,
      c_args: warning_args,
      link_args: ['-Wl,--wrap=freerdp_input_send_keyboard_event',
                  '-Wl,--wrap=freerdp_input_send_unicode_keyboard_event'],
      link_depends: backend_link_depends,
      dependencies: freerdp_dep))
  endif
endif
