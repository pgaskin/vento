# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# LibVNCServer's client library, libvncclient, with the common/ files it
# uses: GnuTLS for VeNCrypt, libjpeg-turbo for Tight, zlib. Upstream's
# configure checks for some fifty headers and functions; glibc and bionic
# give the same answers but two, which are written down here.

vnc_dir = '../../../third_party/libvncserver'
if not fs.exists(vnc_dir / 'include/rfb/rfbclient.h')
  error('third_party/libvncserver is empty: run `git submodule update --init`')
endif
vnc_version = fs.read(vnc_dir / 'CMakeLists.txt').split('project(LibVNCServer VERSION ')[1].split(' ')[0]
vnc_v = vnc_version.split('.')

vnc_conf = configuration_data({
  'LIBVNCSERVER_ALLOW24BPP': true,
  'LIBVNCSERVER_IPv6': true,
  'LIBVNCSERVER_HAVE_LIBZ': true,
  'LIBVNCSERVER_HAVE_LIBJPEG': true,
  'LIBVNCSERVER_HAVE_LIBPTHREAD': true,
  'LIBVNCSERVER_HAVE_GNUTLS': true,
  # bionic has no ftime(), and has <sys/endian.h>.
  'LIBVNCSERVER_HAVE_FTIME': not android,
  'LIBVNCSERVER_HAVE_SYS_ENDIAN_H': android,
  'HAVE_LIBVNCSERVER_PID_T': true,
  'HAVE_LIBVNCSERVER_SIZE_T': true,
  'HAVE_LIBVNCSERVER_SOCKLEN_T': true,
  'FULL_PACKAGE_NAME': 'LibVNCServer',
  'LibVNCServer_VERSION': vnc_version,
  'LibVNCServer_VERSION_MAJOR': vnc_v[0],
  'LibVNCServer_VERSION_MINOR': vnc_v[1],
  'LibVNCServer_VERSION_PATCH': vnc_v[2],
  # What `const` and `inline` would be defined as, were they missing; the
  # lines are commented out in the template.
  'CMAKE_CONST': '',
  'CMAKE_INLINE': '',
})
foreach name : ['DIRENT_H', 'ENDIAN_H', 'FCNTL_H', 'GETTIMEOFDAY', 'GETHOSTBYNAME', 'GETHOSTNAME',
                'INET_NTOA', 'MEMMOVE', 'MEMSET', 'MKFIFO', 'SELECT', 'SOCKET', 'STRCHR',
                'STRCSPN', 'STRDUP', 'STRERROR', 'STRSTR', 'NETINET_IN_H', 'SYS_SOCKET_H',
                'SYS_STAT_H', 'SYS_TIME_H', 'SYS_TYPES_H', 'SYS_WAIT_H', 'SYS_RESOURCE_H',
                'UNISTD_H', 'VFORK', 'VPRINTF', 'MMAP', 'FORK']
  vnc_conf.set('LIBVNCSERVER_HAVE_' + name, true)
endforeach
# <rfb/rfbconfig.h>: written in rfb/ of the build tree, since a configured
# file goes into the directory of the meson.build that writes it.
subdir('rfb')

vnc_sources = files(
  vnc_dir / 'src/libvncclient/cursor.c',
  vnc_dir / 'src/libvncclient/listen.c',
  vnc_dir / 'src/libvncclient/rfbclient.c',
  vnc_dir / 'src/libvncclient/sockets.c',
  vnc_dir / 'src/libvncclient/tls_gnutls.c',
  vnc_dir / 'src/libvncclient/vncviewer.c',
  vnc_dir / 'src/common/crypto_included.c',
  vnc_dir / 'src/common/d3des.c',
  vnc_dir / 'src/common/minilzo.c',
  vnc_dir / 'src/common/sha1.c',
  vnc_dir / 'src/common/sockets.c',
  vnc_dir / 'src/common/turbojpeg.c',
)

vncclient_lib = static_library('vncclient', vnc_sources, rfbconfig_h,
  include_directories: include_directories('.', vnc_dir / 'include', vnc_dir / 'src/libvncserver',
    vnc_dir / 'src/libvncclient', vnc_dir / 'src/common'),
  # The two definitions its target carries for the common/ files that test
  # them before including rfbconfig.h, and upstream's C90 (with GNU
  # extensions, which Meson calls gnu89).
  c_args: vendored_args + ['-DLIBVNCSERVER_HAVE_LIBJPEG', '-DLIBVNCSERVER_HAVE_LIBZ'],
  override_options: ['c_std=gnu89'],
  dependencies: [jpeg_dep, gnutls_dep, zlib_dep, threads_dep])
vncclient_dep = declare_dependency(link_with: vncclient_lib, sources: rfbconfig_h,
  include_directories: include_directories('.', vnc_dir / 'include', is_system: true),
  dependencies: [jpeg_dep, gnutls_dep, zlib_dep, threads_dep])
