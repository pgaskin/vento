# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# <winpr/config.h> and its three neighbours, from upstream's templates, with
# what upstream's checks find on glibc and bionic.

winpr_have = ['FCNTL_H', 'UNISTD_H', 'INTTYPES_H', 'STDBOOL_H', 'SYS_SELECT_H', 'SYS_EVENTFD_H',
              'SYS_TIMERFD_H', 'SYS_XATTR_H', 'LINUX_MSDOS_FS_H', 'TM_GMTOFF', 'POLL_H', 'SYSLOG_H',
              'GETLOGIN_R', 'GETPWUID_R', 'STRNDUP', 'PIPE2', 'ACCEPT4', 'UNWIND_H', 'SSIZE_T',
              'STRERROR_R']
if not android
  # bionic has no POSIX AIO and no pthread_mutex_timedlock.
  winpr_have += ['AIO_H', 'PTHREAD_MUTEX_TIMEDLOCK']
endif
winpr_options = ['WITH_EVENTFD_READ_WRITE', 'WITH_INTERNAL_RC4', 'WITH_INTERNAL_MD4',
                 'WITH_INTERNAL_MD5', 'WITHOUT_WINPR_3x_DEPRECATED']

winpr_conf = configuration_data(freerdp_buildflags)
foreach name : winpr_have
  winpr_conf.set('WINPR_HAVE_' + name, true)
endforeach
foreach name : winpr_options
  winpr_conf.set(name, true)
endforeach
winpr_conf.set('WINPR_BUILD_CONFIG', ' '.join(winpr_options))
winpr_conf.set('WINPR_VERSION_MAJOR', freerdp_v[0])
winpr_conf.set('WINPR_VERSION_MINOR', freerdp_v[1])
winpr_conf.set('WINPR_VERSION_REVISION', freerdp_v[2])
winpr_conf.set('WINPR_VERSION_SUFFIX', '')
winpr_conf.set('WINPR_API_VERSION', freerdp_v[0])
winpr_conf.set('WINPR_VERSION', freerdp_version)
winpr_conf.set('WINPR_VERSION_FULL', freerdp_version)
# Where an installed WinPR finds its files. Nothing is installed; these are
# upstream's values for the default prefix.
foreach name : ['WINPR_DATA_PATH', 'WINPR_KEYMAP_PATH', 'WINPR_PLUGIN_PATH', 'WINPR_LIBRARY_PATH',
                'WINPR_ADDIN_PATH', 'WINPR_PROXY_PLUGINDIR']
  winpr_conf.set(name, '')
endforeach
winpr_conf.set('CMAKE_INSTALL_PREFIX', '/usr/local')
winpr_conf.set('CMAKE_INSTALL_FULL_SYSCONFDIR', '/usr/local/etc')

winpr_headers = []
foreach name : ['config.h', 'buildflags.h', 'build-config.h', 'version.h']
  winpr_headers += configure_file(input: freerdp_templates / 'winpr/include/config' / name + '.in',
    output: name, configuration: winpr_conf, format: 'cmake')
endforeach
