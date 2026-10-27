# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# TigerVNC's client library: the core, rdr, network, rfb and rfbclient
# libraries of its common/, as one archive, with the config.h they include.
# Upstream's own CMakeLists builds a viewer and a server and requires X11.
#
# H.264: upstream's H264DecoderContext.cxx, the factory that picks one of its
# two decoder contexts (FFmpeg's, Windows'), is not compiled. The TigerVNC
# module's H264VentoDecoderContext.cxx defines that factory and the context
# that hands each rectangle to vento's video decoders, and goes into the
# module's own library.

tigervnc_dir = '../../../third_party/tigervnc'
if not fs.exists(tigervnc_dir / 'common/rfb/CConnection.cxx')
  error('third_party/tigervnc is empty: run `git submodule update --init`')
endif

# Not set: ENABLE_NLS (bionic has no gettext, and the Linux build matches it),
# HAVE_AUDIO (their viewer's own sound output; the session plays sound through
# vento/audio.h) and HAVE_LIBAV (their FFmpeg decoder context). BUILD_TIMESTAMP
# would otherwise be the time of the build.
tigervnc_config_h = configure_file(input: tigervnc_dir / 'config.h.in', output: 'config.h',
  format: 'cmake@', configuration: configuration_data({
    'CMAKE_PROJECT_NAME': 'tigervnc',
    'VERSION': fs.read(tigervnc_dir / 'CMakeLists.txt').split('set(VERSION ')[1].split(')')[0],
    'BUILD_TIMESTAMP': '0',
    'HAVE_H264': true,
    'HAVE_GNUTLS': true,
    'HAVE_NETTLE': true,
  }))

common = tigervnc_dir / 'common'
tigervnc_sources = files(
  # core
  common / 'core/Configuration.cxx', common / 'core/Exception.cxx', common / 'core/Logger.cxx',
  common / 'core/Logger_file.cxx', common / 'core/Logger_stdio.cxx',
  common / 'core/Logger_syslog.cxx', common / 'core/LogWriter.cxx', common / 'core/Region.cxx',
  common / 'core/Timer.cxx', common / 'core/string.cxx', common / 'core/time.cxx',
  common / 'core/xdgdirs.cxx',
  # rdr
  common / 'rdr/AESInStream.cxx', common / 'rdr/AESOutStream.cxx',
  common / 'rdr/BufferedInStream.cxx', common / 'rdr/BufferedOutStream.cxx',
  common / 'rdr/FdInStream.cxx', common / 'rdr/FdOutStream.cxx', common / 'rdr/FileInStream.cxx',
  common / 'rdr/HexInStream.cxx', common / 'rdr/HexOutStream.cxx', common / 'rdr/RandomStream.cxx',
  common / 'rdr/TLSException.cxx', common / 'rdr/TLSInStream.cxx', common / 'rdr/TLSOutStream.cxx',
  common / 'rdr/TLSSocket.cxx', common / 'rdr/ZlibInStream.cxx', common / 'rdr/ZlibOutStream.cxx',
  # network
  common / 'network/Socket.cxx', common / 'network/TcpSocket.cxx', common / 'network/UnixSocket.cxx',
  # rfb
  common / 'rfb/AccessRights.cxx', common / 'rfb/Blacklist.cxx', common / 'rfb/Congestion.cxx',
  common / 'rfb/ComparingUpdateTracker.cxx', common / 'rfb/Cursor.cxx', common / 'rfb/d3des.c',
  common / 'rfb/JpegCompressor.cxx', common / 'rfb/JpegDecompressor.cxx',
  common / 'rfb/KeyRemapper.cxx', common / 'rfb/KeysymStr.c', common / 'rfb/PixelBuffer.cxx',
  common / 'rfb/PixelFormat.cxx', common / 'rfb/ScreenSet.cxx', common / 'rfb/Security.cxx',
  common / 'rfb/UpdateTracker.cxx', common / 'rfb/encodings.cxx', common / 'rfb/obfuscate.cxx',
  # rfbclient, with H.264, GnuTLS and nettle
  common / 'rfb/CConnection.cxx', common / 'rfb/CMsgReader.cxx', common / 'rfb/CMsgWriter.cxx',
  common / 'rfb/CSecurityPlain.cxx', common / 'rfb/CSecurityStack.cxx',
  common / 'rfb/CSecurityVeNCrypt.cxx', common / 'rfb/CSecurityVncAuth.cxx',
  common / 'rfb/CopyRectDecoder.cxx', common / 'rfb/DecodeManager.cxx', common / 'rfb/Decoder.cxx',
  common / 'rfb/HextileDecoder.cxx', common / 'rfb/JPEGDecoder.cxx', common / 'rfb/RREDecoder.cxx',
  common / 'rfb/RawDecoder.cxx', common / 'rfb/SecurityClient.cxx', common / 'rfb/ServerParams.cxx',
  common / 'rfb/TightDecoder.cxx', common / 'rfb/ZRLEDecoder.cxx', common / 'rfb/H264Decoder.cxx',
  common / 'rfb/CSecurityTLS.cxx', common / 'rfb/CSecurityDH.cxx',
  common / 'rfb/CSecurityMSLogonII.cxx', common / 'rfb/CSecurityRSAAES.cxx',
)

tigervnc_deps = [pixman_dep, jpeg_dep, gnutls_dep, hogweed_dep, nettle_dep, zlib_dep, threads_dep]
tigervnc_lib = static_library('tigervnc', tigervnc_sources, tigervnc_config_h,
  include_directories: [include_directories('.'), include_directories(common, is_system: true)],
  c_args: vendored_args + ['-DHAVE_CONFIG_H'],
  cpp_args: vendored_args + ['-DHAVE_CONFIG_H'],
  dependencies: tigervnc_deps)
tigervnc_dep = declare_dependency(link_with: tigervnc_lib, sources: tigervnc_config_h,
  compile_args: ['-DHAVE_CONFIG_H'],
  include_directories: [include_directories('.'), include_directories(common, is_system: true)],
  dependencies: tigervnc_deps)
