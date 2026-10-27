# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# FreeRDP 3 as an RDP client: WinPR, libfreerdp with its codecs and
# primitives, client/common, and the client half of each channel listed in
# freerdp_channels below.
#
# The options follow upstream's Android build
# (client/Android/cmake/ExternalFreeRDP.cmake), less what a phone or the Linux
# client has no use for. WinPR's own RC4, MD4 and MD5 are compiled, since RDP
# needs them and a current OpenSSL has them in its legacy provider. JSON is
# off (json-stub.c): it is read only for Azure sign-in and for keyboard
# layouts and time zones from files. The library decodes nothing itself:
# H.264 goes to vento's decoders through the FreeRDP module's h264_vento.c, in
# the slot patches/freerdp adds for an external subsystem (none of upstream's
# subsystems is built), and sound goes to vento's output and decoders through
# its rdpsnd_vento.c, which the channel finds by name at run time, so the only
# sound device built in is the one that discards every sample. On Linux,
# libswscale scales a surface the server maps onto a scaled output, and with
# it the library offers EGFX 10.5 and 10.6; Android has no scaler and is 10.4.

freerdp_dir = '../../../third_party/freerdp'
if not fs.exists(freerdp_dir / 'include/freerdp/freerdp.h')
  error('third_party/freerdp is empty: run `git submodule update --init`')
endif
# The release's version string, not `git describe`, whose commit hash
# depends on the clone.
freerdp_version = fs.read(freerdp_dir / 'cmake/GetProjectVersion.cmake').split(
  'set(RAW_VERSION_STRING "')[1].split('"')[0]
freerdp_v = freerdp_version.split('.')
freerdp_debug = get_option('variant') == 'debug'
# Not executable, so that find_program runs it with the Python that runs Meson.
generate_py = find_program('generate.py')

# The channels: for each, the entry points it registers (the static channel
# table's columns), its sources, its subsystems, and the include directory
# upstream gives it (its own directory, its common/, or none). Serial and
# parallel ports are desktop only, as upstream has them. Upstream's channel
# options also decide config.h's CHANNEL_* lines (include/freerdp).
freerdp_channels = {
  'ainput': {'entries': ['DVCPluginEntry'], 'sources': ['client/ainput_main.c']},
  'audin': {'entries': ['DVCPluginEntry'], 'sources': ['client/audin_main.c']},
  'cliprdr': {'entries': ['VirtualChannelEntryEx'], 'include': 'none',
    'sources': ['client/cliprdr_format.c', 'client/cliprdr_main.c', 'cliprdr_common.c']},
  'disp': {'entries': ['DVCPluginEntry'], 'sources': ['client/disp_main.c', 'disp_common.c']},
  'drdynvc': {'entries': ['VirtualChannelEntryEx'], 'include': 'none',
    'sources': ['client/drdynvc_main.c']},
  'drive': {'entries': ['DeviceServiceEntry'], 'include': 'none',
    'sources': ['client/drive_file.c', 'client/drive_main.c']},
  'echo': {'entries': ['DVCPluginEntry'], 'sources': ['client/echo_main.c']},
  'encomsp': {'entries': ['VirtualChannelEntryEx'], 'sources': ['client/encomsp_main.c']},
  'geometry': {'entries': ['DVCPluginEntry'], 'sources': ['client/geometry_main.c']},
  'location': {'entries': ['DVCPluginEntry'], 'sources': ['client/location_main.c']},
  'rail': {'entries': ['VirtualChannelEntryEx'], 'include': 'none',
    'sources': ['client/client_rails.c', 'client/rail_main.c', 'client/rail_orders.c',
                'rail_common.c']},
  # Both C libraries have getmntent_r, which the drive list uses.
  'rdpdr': {'entries': ['VirtualChannelEntryEx'], 'include': 'none',
    'sources': ['client/devman.c', 'client/irp.c', 'client/rdpdr_capabilities.c',
                'client/rdpdr_main.c'],
    'args': ['-DFREERDP_HAVE_GETMNTENT_R']},
  'rdpei': {'entries': ['DVCPluginEntry'], 'sources': ['client/rdpei_main.c', 'rdpei_common.c']},
  'rdpgfx': {'entries': ['DVCPluginEntry'],
    'sources': ['client/rdpgfx_codec.c', 'client/rdpgfx_main.c', 'rdpgfx_common.c']},
  'rdpsnd': {'entries': ['VirtualChannelEntryEx', 'DVCPluginEntry'], 'include': 'common',
    'sources': ['client/rdpsnd_main.c', 'common/rdpsnd_common.c'],
    'subsystems': {'fake': ['client/fake/rdpsnd_fake.c']}},
  'remdesk': {'entries': ['VirtualChannelEntryEx'], 'include': 'common',
    'sources': ['client/remdesk_main.c', 'common/remdesk_common.c']},
  'smartcard': {'entries': ['DeviceServiceEntry'], 'include': 'none',
    'sources': ['client/smartcard_main.c']},
  'video': {'entries': ['DVCPluginEntry'], 'sources': ['client/video_main.c']},
}
if not android
  freerdp_channels += {
    'parallel': {'entries': ['DeviceServiceEntry'], 'include': 'none',
      'sources': ['client/parallel_main.c']},
    'serial': {'entries': ['DeviceServiceEntry'], 'include': 'none',
      'sources': ['client/serial_main.c']},
  }
endif

# Upstream's directory-wide definitions and C flags, which every FreeRDP and
# WinPR file is compiled with: every symbol hidden but the FREERDP_API and
# WINPR_API ones, frame pointers for its backtraces, and a signed char on
# aarch64 too.
freerdp_cflags = ['-fvisibility=hidden', '-fno-omit-frame-pointer', '-fsigned-char']
freerdp_args = vendored_args + freerdp_cflags + [
  '-DFREERDP_EXPORTS', '-DNONAMELESSUNION', '-DWINPR_DEFINE_ATTR_NODISCARD', '-DWITH_OPENSSL',
  '-DWITH_SMARTCARD_EMULATE',
  '-DWINPR_TIMEZONE_FILE="@0@"'.format(android ? '/var/db/zoneinfo' : '/etc/timezone'),
]
if freerdp_debug
  # Upstream's debug build: asserts that print what failed before aborting.
  freerdp_args += '-DWITH_VERBOSE_WINPR_ASSERT'
  if android
    freerdp_args += '-DNDK_DEBUG=1'
  endif
endif

# The generated headers, <winpr/config.h> and <freerdp/config.h> and their
# neighbours, from upstream's templates. What buildflags.h reports in both
# libraries is the flags above and the build type's, as upstream's
# CMAKE_C_FLAGS would have them.
freerdp_templates = meson.project_source_root() / 'third_party/freerdp'
freerdp_buildflags = {
  'CURRENT_C_FLAGS': ' '.join(freerdp_cflags + ['-O' + get_option('optimization'), '-g', '-DNDEBUG']),
  'CMAKE_C_COMPILER_ID': cc.get_id() == 'clang' ? 'Clang' : 'GNU',
  'CMAKE_C_COMPILER_VERSION': cc.version(),
  'TARGET_ARCH': cpu == 'x86_64' ? 'x64' : '',
  'CURRENT_BUILD_CONFIG': freerdp_debug ? 'Debug' : 'RelWithDebInfo',
  'CMAKE_SHARED_LIBRARY_PREFIX': 'lib',
  'CMAKE_SHARED_LIBRARY_SUFFIX': '.so',
  'VENDOR': 'FreeRDP',
  'PRODUCT': 'FreeRDP',
  'GIT_REVISION': 'n/a',
}
subdir('include/winpr')
subdir('include/freerdp')
freerdp_headers = winpr_headers + freerdp_config_headers
freerdp_inc = include_directories('include', freerdp_dir / 'winpr/include', freerdp_dir / 'include')

# WinPR

winpr_files = {
  'bcrypt': ['bcrypt'],
  'clipboard': ['clipboard', 'synthetic', 'synthetic_file'],
  'comm': ['comm', 'comm_io', 'comm_ioctl', 'comm_sercx2_sys', 'comm_sercx_sys', 'comm_serial_sys'],
  'credentials': ['credentials'],
  # UTF-16 through Java's String on Android, and WinPR's own code on Linux.
  'crt': ['alignment', 'assert', 'buffer', 'conversion', 'memory', 'string', 'unicode',
          android ? 'unicode_android' : 'unicode_builtin'],
  'crypto': ['cert', 'cipher', 'crypto', 'hash', 'hmac_md5', 'md4', 'md5', 'rand', 'rc4'],
  'dsparse': ['dsparse'],
  'environment': ['environment'],
  'error': ['error'],
  'file': ['file', 'generic', 'namedPipeClient', 'pattern'],
  'handle': ['handle', 'nonehandle'],
  'input': ['keycode', 'scancode', 'virtualkey'],
  'interlocked': ['interlocked'],
  'io': ['device', 'io'],
  'library': ['library'],
  'memory': ['memory'],
  'ncrypt': ['ncrypt'],
  'nt': ['nt', 'ntstatus'],
  'path': ['path', 'shell'],
  'pool': ['callback', 'callback_cleanup', 'cleanup_group', 'io', 'pool', 'synch', 'timer', 'work'],
  'registry': ['registry', 'registry_reg'],
  'rpc': ['rpc'],
  'security': ['security'],
  'shell': android ? [] : ['shell'],
  'smartcard': ['smartcard', 'smartcard_pcsc'],
  'sspicli': ['sspicli'],
  'sspi': ['CredSSP/credssp', 'Kerberos/kerberos', 'NTLM/ntlm', 'NTLM/ntlm_av_pairs',
           'NTLM/ntlm_compute', 'NTLM/ntlm_message', 'Negotiate/negotiate', 'Schannel/schannel',
           'Schannel/schannel_openssl', 'sspi', 'sspi_export', 'sspi_gss', 'sspi_winpr'],
  'synch': ['address', 'barrier', 'critical', 'event', 'init', 'mutex', 'pollset', 'sleep',
            'timer', 'wait'],
  # CPU features through the NDK's helper on Android.
  'sysinfo': ['sysinfo'] + (android ? ['cpufeatures/cpu-features'] : []),
  'thread': ['apc', 'argv', 'process', 'processor', 'procthreadattr', 'thread', 'tls'],
  'timezone': ['TimeZoneIanaAbbrevMap', 'TimeZoneNameMapUtils', 'WindowsZones', 'timezone'],
  'utils': ['asn1/asn1', 'atexit', 'cmdline', 'debug', 'image', 'ini', 'json/json-stub', 'json/json',
            'ntlm', 'print', 'sam', 'ssl', 'stream', 'strlst', 'unwind/debug', 'winpr'],
  'utils/collections': ['ArrayList', 'BitStream', 'BufferPool', 'CountdownEvent', 'HashTable',
                        'LinkedList', 'ListDictionary', 'MessagePipe', 'MessageQueue', 'Object',
                        'ObjectPool', 'PubSub', 'Queue', 'Stack', 'StreamPool'],
  'utils/wlog': ['Appender', 'BinaryAppender', 'CallbackAppender', 'ConsoleAppender', 'DataMessage',
                 'FileAppender', 'ImageMessage', 'Layout', 'Message', 'PacketMessage',
                 'SyslogAppender', 'UdpAppender', 'wlog'],
  'winsock': ['winsock'],
  'wtsapi': ['wtsapi'],
}
winpr_sources = []
foreach dir, names : winpr_files
  foreach name : names
    winpr_sources += files(freerdp_dir / 'winpr/libwinpr' / dir / name + '.c')
  endforeach
endforeach

winpr_args = freerdp_args + [
  '-DWINPR_EXPORTS', '-DUSE_UNWIND', '-DWINPR_CRITICAL_SECTION_DISABLE_SPINCOUNT',
  '-DWINPR_HAVE_SERIAL_SUPPORT', '-DWITHOUT_WINPR_3x_DEPRECATED', '-DWITH_SMARTCARD_PCSC',
  '-DWITH_TIMEZONE_COMPILED', '-DWINPR_THREADPOOL_DEFAULT_MIN_COUNT=4',
  '-DWINPR_THREADPOOL_DEFAULT_MAX_COUNT=16', '-DCLIPBOARD_FILE_MAX_RECURSION=512',
  '-DSTREAMPOOL_SIZE_LIMIT=0x1000000', '-DWINPR_RESOURCE_ROOT="/usr/local/share/WinPR"',
]
if not android
  # What glibc has and bionic does not: scheduling priorities, POSIX regular
  # expressions and POSIX timers.
  winpr_args += ['-DPTHREAD_SETSCHEDPRIO', '-DWINPR_HAVE_REGCOMP', '-DWITH_POSIX_TIMER']
endif
winpr_deps = [openssl_dep, threads_dep, dl_dep, zlib_dep]
if android
  winpr_deps += cc.find_library('log')
else
  winpr_deps += [cc.find_library('rt'), m_dep]
endif
winpr_inc = [freerdp_inc, include_directories(freerdp_dir / 'winpr/libwinpr/utils')]

# Upstream sets a few definitions on single files; each such file is an
# archive of its own here.
winpr_libs = [
  # pipe2() and accept4(), which glibc declares only with _GNU_SOURCE.
  static_library('winpr_gnu_source',
    files(freerdp_dir / 'winpr/libwinpr/pipe/pipe.c', freerdp_dir / 'winpr/libwinpr/synch/semaphore.c'),
    winpr_headers,
    include_directories: winpr_inc,
    c_args: winpr_args + ['-D_GNU_SOURCE'],
    dependencies: winpr_deps),
]
if android
  # WinPR's JNI_OnLoad, which keeps the JavaVM it reads the time zone through,
  # under another name: a library has only one JNI_OnLoad, the crate's, which
  # calls this one (`natives!`'s `on_load` in the FreeRDP module's src/lib.rs).
  # jni.h's declaration is renamed with it.
  winpr_libs += static_library('winpr_android', files(freerdp_dir / 'winpr/libwinpr/utils/android.c'),
    winpr_headers,
    include_directories: winpr_inc,
    c_args: winpr_args + ['-DJNI_OnLoad=winpr_JNI_OnLoad'],
    dependencies: winpr_deps)
endif
winpr_lib = static_library('winpr', winpr_sources, winpr_headers,
  include_directories: winpr_inc,
  c_args: winpr_args,
  link_with: winpr_libs,
  dependencies: winpr_deps)
winpr_dep = declare_dependency(link_with: winpr_lib, sources: winpr_headers,
  include_directories: freerdp_inc, dependencies: winpr_deps)

# libfreerdp

libfreerdp_files = {
  'cache': ['bitmap', 'brush', 'cache', 'glyph', 'nine_grid', 'offscreen', 'palette', 'persistent',
            'pointer'],
  'common': ['addin', 'assistance', 'settings', 'settings_getters', 'settings_str'],
  'core': ['aad', 'activation', 'autodetect', 'capabilities', 'channels', 'childsession', 'client',
           'codecs', 'connection', 'credssp_auth', 'display', 'errbase', 'errconnect', 'errinfo',
           'fastpath', 'freerdp', 'gateway/arm', 'gateway/http', 'gateway/ncacn_http',
           'gateway/rdg', 'gateway/rpc', 'gateway/rpc_bind', 'gateway/rpc_client',
           'gateway/rpc_fault', 'gateway/rts', 'gateway/rts_signature', 'gateway/tsg',
           'gateway/websocket', 'gateway/wst', 'gcc', 'graphics', 'heartbeat', 'info', 'input',
           'license', 'listener', 'mcs', 'message', 'metrics', 'multitransport', 'nego', 'nla',
           'orders', 'peer', 'proxy', 'rdp', 'rdstls', 'redirection', 'security', 'server',
           'settings', 'smartcardlogon', 'state', 'streamdump', 'surface', 'tcp', 'timer',
           'timezone', 'tpdu', 'tpkt', 'transport', 'update', 'utils', 'window'],
  'crypto': ['base64', 'ber', 'cert_common', 'certificate', 'certificate_data', 'certificate_store',
             'crypto', 'opensslcompat', 'per', 'privatekey', 'tls', 'x509_utils'],
  'emu/scard': ['FreeRDP.ico', 'smartcard_emulate', 'smartcard_virtual_gids'],
  'gdi': ['bitmap', 'brush', 'clipping', 'dc', 'drawing', 'gdi', 'gfx', 'graphics', 'line', 'pen',
          'region', 'shape', 'video'],
  'locale': ['keyboard', 'keyboard_layout', 'locale'],
  'utils': ['channel_pdu_tracker', 'cliprdr_utils', 'drdynvc', 'encoded_types', 'gfx', 'helpers',
            'http', 'passphrase', 'pcap', 'profiler', 'rdpdr_utils', 'ringbuffer', 'signal',
            'signal_posix', 'smartcard_call', 'smartcard_operations', 'smartcard_pack',
            'stopwatch', 'string', 'warnings'],
}
libfreerdp_sources = []
foreach dir, names : libfreerdp_files
  foreach name : names
    libfreerdp_sources += files(freerdp_dir / 'libfreerdp' / dir / name + '.c')
  endforeach
endforeach

# Codecs and primitives. codec/h264.c is the decoder's frame; its subsystem
# is the FreeRDP module's (WITH_H264_EXTERNAL in config.h). The Neon kernels
# need no flag on aarch64 and are #if'd out elsewhere. Each x86 kernel is
# compiled with its own -m flag, which on aarch64 is left out (the file is
# #if'd out there); one archive per flag.
codec_dir = freerdp_dir / 'libfreerdp/codec'
prim_dir = freerdp_dir / 'libfreerdp/primitives'
codec_sources = []
foreach name : ['audio', 'bitmap', 'bulk', 'clear', 'color', 'dsp', 'h264', 'interleaved', 'jpeg',
                'mppc', 'ncrush', 'neon/nsc_neon', 'neon/rfx_neon', 'nsc', 'nsc_encode', 'planar',
                'progressive', 'region', 'rfx', 'rfx_decode', 'rfx_dwt', 'rfx_encode',
                'rfx_quantization', 'rfx_rlgr', 'video', 'xcrush', 'yuv', 'zgfx']
  codec_sources += files(codec_dir / name + '.c')
endforeach
prim_sources = []
foreach name : ['neon/prim_YCoCg_neon', 'neon/prim_YUV_neon', 'neon/prim_colors_neon',
                'opencl/prim_YUV_opencl', 'prim_YCoCg', 'prim_YUV', 'prim_add', 'prim_alphaComp',
                'prim_andor', 'prim_colors', 'prim_copy', 'prim_set', 'prim_shift', 'prim_sign',
                'primitives']
  prim_sources += files(prim_dir / name + '.c')
endforeach
codec_kernels = {
  '-msse3': files(codec_dir / 'sse/nsc_sse2.c', codec_dir / 'sse/rfx_sse2.c'),
}
prim_kernels = {
  '-msse3': files(prim_dir / 'sse/prim_add_sse3.c', prim_dir / 'sse/prim_alphaComp_sse3.c',
                  prim_dir / 'sse/prim_andor_sse3.c', prim_dir / 'sse/prim_colors_sse2.c',
                  prim_dir / 'sse/prim_set_sse2.c', prim_dir / 'sse/prim_shift_sse3.c'),
  '-mssse3': files(prim_dir / 'sse/prim_YCoCg_ssse3.c', prim_dir / 'sse/prim_sign_ssse3.c'),
  '-msse4.1': files(prim_dir / 'sse/prim_YUV_sse4.1.c', prim_dir / 'sse/prim_copy_sse4_1.c'),
  '-mavx2': files(prim_dir / 'sse/prim_copy_avx2.c'),
}

libfreerdp_deps = [winpr_dep, openssl_dep, zlib_dep, threads_dep, dl_dep, m_dep]
if not android
  libfreerdp_deps += swscale_dep
endif
codec_args = freerdp_args + ['-DYUV_TILE_SIZE=512']
prim_inc = [freerdp_inc, include_directories(prim_dir)]

kernel_libs = []
foreach flag, sources : codec_kernels
  kernel_libs += static_library('freerdp_codecs' + flag.underscorify(), sources, freerdp_headers,
    include_directories: freerdp_inc,
    c_args: codec_args + (cpu == 'x86_64' ? [flag] : []),
    dependencies: libfreerdp_deps)
endforeach
foreach flag, sources : prim_kernels
  kernel_libs += static_library('freerdp_primitives' + flag.underscorify(), sources, freerdp_headers,
    include_directories: prim_inc,
    c_args: freerdp_args + (cpu == 'x86_64' ? [flag] : []),
    dependencies: libfreerdp_deps)
endforeach
codec_lib = static_library('freerdp_codecs', codec_sources, freerdp_headers,
  include_directories: freerdp_inc,
  c_args: codec_args,
  dependencies: libfreerdp_deps)
prim_lib = static_library('freerdp_primitives', prim_sources, freerdp_headers,
  include_directories: prim_inc,
  c_args: freerdp_args,
  dependencies: libfreerdp_deps)
# EXT_PATH is where an installed FreeRDP looks for extensions; none is loaded.
libfreerdp_lib = static_library('freerdp', libfreerdp_sources, freerdp_headers,
  include_directories: freerdp_inc,
  c_args: freerdp_args + ['-DEXT_PATH="/usr/local/lib/freerdp@0@/extensions"'.format(freerdp_v[0])],
  link_with: [codec_lib, prim_lib] + kernel_libs,
  dependencies: libfreerdp_deps)
libfreerdp_dep = declare_dependency(link_with: libfreerdp_lib, sources: freerdp_headers,
  include_directories: freerdp_inc, dependencies: libfreerdp_deps)

# The channels and client/common

channel_libs = []
channel_specs = []
foreach name, channel : freerdp_channels
  dir = freerdp_dir / 'channels' / name
  include = channel.get('include', 'dir')
  inc = [freerdp_inc]
  if include == 'dir'
    inc += include_directories(dir)
  elif include == 'common'
    inc += include_directories(dir / 'common')
  endif
  sources = []
  foreach f : channel['sources']
    sources += files(dir / f)
  endforeach
  # A subsystem goes into its channel's library, with client/ on the path.
  subsystems = channel.get('subsystems', {})
  foreach subsystem, subsystem_sources : subsystems
    foreach f : subsystem_sources
      sources += files(dir / f)
    endforeach
  endforeach
  if subsystems.keys().length() > 0
    inc += include_directories(dir / 'client')
  endif
  channel_libs += static_library('freerdp_channel_' + name, sources, freerdp_headers,
    include_directories: inc,
    c_args: freerdp_args + channel.get('args', []),
    dependencies: libfreerdp_dep)
  channel_specs += '@0@:@1@:@2@'.format(name, '+'.join(channel['entries']),
                                        ','.join(subsystems.keys()))
endforeach

# channels/client/tables.c, the static registry of the channels above.
tables_c = custom_target('freerdp_tables_c',
  input: freerdp_dir / 'channels/client/tables.c.in',
  output: 'tables.c',
  command: [generate_py, 'tables', '@INPUT@', '@OUTPUT@', channel_specs])

client_sources = files(freerdp_dir / 'channels/client/addin.c',
                       freerdp_dir / 'channels/client/generic_dynvc.c')
foreach name : ['aad_helper', 'client', 'client_cliprdr_file', 'cmdline', 'file', 'geometry',
                'oauth2', 'smartcard_cli']
  client_sources += files(freerdp_dir / 'client/common' / name + '.c')
endforeach
freerdp_client_lib = static_library('freerdp_client', client_sources, tables_c, freerdp_headers,
  include_directories: [freerdp_inc, include_directories(freerdp_dir / 'channels/client')],
  # addin.c names a channel's shared library by file name, with the
  # executable suffix upstream passes in; none is ever loaded here.
  c_args: freerdp_args + ['-DCMAKE_EXECUTABLE_SUFFIX=""'],
  link_with: channel_libs,
  dependencies: libfreerdp_dep)

# What the FreeRDP module links: its headers as system ones, with OpenSSL's
# for the TLS version constants.
freerdp_dep = declare_dependency(link_with: [freerdp_client_lib] + channel_libs,
  sources: freerdp_headers,
  include_directories: include_directories('include', freerdp_dir / 'winpr/include',
    freerdp_dir / 'include', is_system: true),
  dependencies: libfreerdp_dep)
# The H.264 subsystem interface, which upstream keeps beside its subsystems
# rather than in its public headers.
freerdp_codec_inc = include_directories(codec_dir, is_system: true)
