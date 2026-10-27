# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# <freerdp/config.h> and its neighbours, from upstream's templates. config.h
# is mostly the channel list; the rest is what this build has.

freerdp_options = ['WITH_OPAQUE_SETTINGS', 'WITH_SIMD', 'WITH_AVX2', 'WITH_GFX_H264',
                   'WITH_H264_EXTERNAL', 'WITH_CHANNELS', 'WITH_CLIENT_CHANNELS',
                   'WITHOUT_FREERDP_3x_DEPRECATED']
if not android
  freerdp_options += 'WITH_SWSCALE'
endif
# Checked as upstream does, since it depends on the kernel headers' age: older
# ones (Ubuntu 18.04's, the Linux release build's) lack VMADDR_FLAG_TO_HOST.
if cc.has_header_symbol('linux/vm_sockets.h', 'VMADDR_FLAG_TO_HOST',
                        prefix: '#include <ctype.h>\n#include <sys/socket.h>')
  freerdp_options += 'HAVE_AF_VSOCK_H'
endif

# Upstream's channel options as they default for this configuration: c for a
# client half (each of which is in freerdp_channels), s for a server half.
# A channel with a server half has its _SERVER line even though no server is
# built.
channel_halves = {
  'ainput': 'cs', 'audin': 'cs', 'cliprdr': 'cs', 'disp': 'cs', 'drdynvc': 'cs', 'drive': 'c',
  'echo': 'cs', 'encomsp': 'cs', 'geometry': 'c', 'location': 'cs', 'parallel': 'c',
  'printer': '', 'rail': 'cs', 'rdpdr': 'cs', 'rdpecam': 's', 'rdpei': 'cs', 'rdpgfx': 'cs',
  'rdpemsc': 's', 'rdpsnd': 'cs', 'remdesk': 'cs', 'serial': 'c', 'smartcard': 'c',
  'telemetry': 's', 'video': 'c',
}
if android
  channel_halves += {'audin': 'c', 'parallel': '', 'serial': ''}
endif
foreach name, halves : channel_halves
  freerdp_options += 'CHANNEL_' + name.to_upper()
  if halves.contains('c')
    freerdp_options += 'CHANNEL_' + name.to_upper() + '_CLIENT'
  endif
  if halves.contains('s')
    freerdp_options += 'CHANNEL_' + name.to_upper() + '_SERVER'
  endif
  if halves.contains('c') != freerdp_channels.has_key(name)
    error('config.h and freerdp_channels disagree on @0@\'s client half'.format(name))
  endif
endforeach

freerdp_conf = configuration_data(freerdp_buildflags)
foreach name : freerdp_options
  freerdp_conf.set(name, true)
endforeach
freerdp_conf.set('FREERDP_BUILD_CONFIG', ' '.join(freerdp_options))
freerdp_conf.set('FREERDP_VERSION_MAJOR', freerdp_v[0])
freerdp_conf.set('FREERDP_VERSION_MINOR', freerdp_v[1])
freerdp_conf.set('FREERDP_VERSION_REVISION', freerdp_v[2])
freerdp_conf.set('FREERDP_VERSION_SUFFIX', '')
freerdp_conf.set('FREERDP_API_VERSION', freerdp_v[0])
freerdp_conf.set('FREERDP_VERSION', freerdp_version)
freerdp_conf.set('FREERDP_VERSION_FULL', freerdp_version)
# Upstream's install layout: relative to the library on Android, under the
# default prefix on Linux. Nothing is installed or loaded from these: every
# channel is built in.
if android
  freerdp_paths = {'DATA_PATH': 'share', 'KEYMAP_PATH': '', 'PLUGIN_PATH': '.',
                   'INSTALL_PREFIX': '.', 'LIBRARY_PATH': '.', 'LIBEXEC_REL_PATH': '.',
                   'LIBEXEC_PATH': '.', 'ADDIN_PATH': '.', 'PROXY_PLUGINDIR': './proxy/'}
else
  plugins = 'lib/freerdp' + freerdp_v[0]
  freerdp_paths = {'DATA_PATH': '/usr/local/share/freerdp' + freerdp_v[0], 'KEYMAP_PATH': '',
                   'PLUGIN_PATH': plugins, 'INSTALL_PREFIX': '/usr/local', 'LIBRARY_PATH': 'lib',
                   'LIBEXEC_REL_PATH': '../libexec', 'LIBEXEC_PATH': 'libexec',
                   'ADDIN_PATH': plugins, 'PROXY_PLUGINDIR': plugins / 'proxy/'}
endif
foreach name, value : freerdp_paths
  freerdp_conf.set('NATIVE_FREERDP_' + name, value)
endforeach

freerdp_config_headers = []
foreach name : ['config.h', 'buildflags.h', 'build-config.h', 'version.h']
  freerdp_config_headers += configure_file(input: freerdp_templates / 'include/config' / name + '.in',
    output: name, configuration: freerdp_conf, format: 'cmake')
endforeach
# The FreeRDP_Settings_Keys_* enums, read out of the settings struct.
freerdp_config_headers += custom_target('freerdp_settings_keys_h',
  input: [freerdp_templates / 'include/freerdp/settings_types_private.h',
          freerdp_templates / 'include/config/settings_keys.h.in'],
  output: 'settings_keys.h',
  command: [generate_py, 'settings-keys', '@INPUT0@', '@INPUT1@', '@OUTPUT@'])
