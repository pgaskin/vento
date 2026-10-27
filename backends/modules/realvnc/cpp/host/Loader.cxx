// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "Loader.h"

#include <dlfcn.h>
#include <stdarg.h>
#include <stdio.h>
#include <net/if.h>
#include <sys/ioctl.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <wchar.h>

#include <unordered_map>

#include <libc_shim.h>
#include <mcpelauncher/linker.h>

#include "Gai.h"
#include "Host.h"
#include "Looper.h"

namespace vento_realvnc {

namespace {

// ---- libc ---------------------------------------------------------------------

/* libc-shim's own `syscall` knows gettid and getrandom and returns ENOSYS for
   the rest; the core uses raw timerfd_create and timerfd_settime. The numbers
   are the kernel's, the same under bionic and glibc. */
long realSyscall(long n, ...) {
    va_list ap;
    va_start(ap, n);
    long a[6];
    for (long &x : a) {
        x = va_arg(ap, long);
    }
    va_end(ap);
    return ::syscall(n, a[0], a[1], a[2], a[3], a[4], a[5]);
}

/* libc-shim's `ioctl` knows three requests and answers 0 to every other
   without passing it on, which leaves the answer unwritten. The core's UDP
   transport asks SIOCGIFMTU of the interface its route to the server goes
   through, and so took whatever was on its stack as the MTU: the payload
   estimate came out anywhere from 16 bytes (the transport then failed
   with "OutStream full" and the session fell back to TCP) to the 1200
   bytes `SctpMaxMtu` allows, depending on what ran before it. The request
   goes to the kernel instead: its `ifreq` is the same under bionic and
   glibc. */
using IoctlFn = int (*)(int, unsigned long, void *);
IoctlFn shimIoctl;

int interfaceIoctl(int fd, unsigned long request, void *arg) {
    if (request == SIOCGIFMTU) {
        return ::ioctl(fd, request, arg);
    }
    return shimIoctl(fd, request, arg);
}

ssize_t readlinkChk(const char *path, char *buf, size_t len, size_t /*bufLen*/) {
    return readlink(path, buf, len);
}

/* An Android process is killed rather than torn down by running its static
   destructors, and the core's, run at `exit`, throw against each other and
   against jnivm's. The core registers them through this, so they are not
   registered at all. */
int noAtexit(void (*)(void *), void *, void *) {
    return 0;
}

/* The bionic libc, as libc-shim provides it, with the gaps the core hits
   filled from glibc. The ABI is the same on Linux x86_64 and aarch64
   for everything routed here; `getaddrinfo` is deliberately not, since
   bionic's `addrinfo` has `ai_canonname` before `ai_addr` and glibc the
   reverse. */
std::unordered_map<std::string, void *> libcSymbols() {
    std::unordered_map<std::string, void *> libc;
    for (const auto &s : shim::get_shimmed_symbols()) {
        libc[s.name] = s.value;
    }
    libc["syscall"] = (void *) &realSyscall;
    // The shim translates clock ids for macOS and rejects CLOCK_MONOTONIC_COARSE.
    libc["clock_gettime"] = dlsym(RTLD_DEFAULT, "clock_gettime");
    libc["clock_getres"] = dlsym(RTLD_DEFAULT, "clock_getres");
    // The shim translates socket options and throws on one it does not know
    // (IP_PKTINFO), and its sockaddr conversion rejects AF_NETLINK, which the
    // core's interface discovery uses.
    for (const char *n : {"setsockopt", "getsockopt", "sendmsg", "recvmsg", "__cmsg_nxthdr",
                          "socket", "bind", "connect", "getsockname", "getpeername", "sendto",
                          "recvfrom", "accept", "shutdown", "strcasestr",
                          "__cxa_thread_atexit_impl"}) {
        libc[n] = dlsym(RTLD_DEFAULT, n);
    }
    shimIoctl = (IoctlFn) libc.at("ioctl");
    libc["ioctl"] = (void *) &interfaceIoctl;
    libc["socketpair"] = (void *) &socketpair;
    libc["wcwidth"] = (void *) &wcwidth;
    libc["setrlimit"] = (void *) &setrlimit;
    libc["__readlink_chk"] = (void *) &readlinkChk;
    libc["__cxa_atexit"] = (void *) &noAtexit;
    // The shim's resolver returns glibc's error codes (see Gai.h).
    wrapResolver(libc);
    return libc;
}

// ---- libm -------------------------------------------------------------------------

/* Only what the core imports from libm (three functions, read off its
   dynamic symbol table), not all of it. The library is pinned by hash,
   so the list is a fact about the pinned build. */
std::unordered_map<std::string, void *> libmSymbols() {
    std::unordered_map<std::string, void *> libm;
    for (const char *n : {"cos", "exp", "pow"}) {
        libm[n] = dlsym(RTLD_DEFAULT, n);
    }
    return libm;
}

// ---- liblog ----------------------------------------------------------------------

/* Android's priorities: VERBOSE 2, DEBUG 3, INFO 4, WARN 5, ERROR 6, FATAL 7.
   Everything lands in `vento_log` under this backend's tag with the core's
   own tag in front, as `adb logcat RealVnc:V` would show it; WARN and above
   become warnings here. */
int logVprint(int prio, const char *tag, const char *fmt, va_list ap) {
    char line[1024];
    vsnprintf(line, sizeof line, fmt, ap);
    vento_log(prio >= 5 ? VENTO_LOG_WARN : VENTO_LOG_INFO, TAG, "%s: %s", tag ? tag : "", line);
    return 0;
}

int logPrint(int prio, const char *tag, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    logVprint(prio, tag, fmt, ap);
    va_end(ap);
    return 0;
}

int logWrite(int prio, const char *tag, const char *text) {
    vento_log(prio >= 5 ? VENTO_LOG_WARN : VENTO_LOG_INFO, TAG, "%s: %s", tag ? tag : "", text);
    return 0;
}

// ---- libjnigraphics ------------------------------------------------------------------

/* The core draws its cursor into an `android.graphics.Bitmap` through the
   NDK's bitmap API; the bitmap is `Classes.cxx`'s stand-in, and these are
   the three calls over it. */
struct AndroidBitmapInfo {
    uint32_t width, height;
    int32_t stride, format;
    uint32_t flags;
};

int bitmapGetInfo(JNIEnv *, jobject bm, AndroidBitmapInfo *info) {
    auto *b = unwrap<Bitmap>(bm);
    if (!b) {
        return -2; // ANDROID_BITMAP_RESULT_BAD_PARAMETER
    }
    info->width = (uint32_t) b->width;
    info->height = (uint32_t) b->height;
    info->stride = b->width * 4;
    info->format = 1; // ANDROID_BITMAP_FORMAT_RGBA_8888
    info->flags = 0;
    return 0;
}

int bitmapLockPixels(JNIEnv *, jobject bm, void **addr) {
    auto *b = unwrap<Bitmap>(bm);
    if (!b) {
        return -2;
    }
    if (b->pixels.empty()) {
        return -3; // ANDROID_BITMAP_RESULT_ALLOCATION_FAILED: too big to allocate
    }
    *addr = b->pixels.data();
    return 0;
}

int bitmapUnlockPixels(JNIEnv *, jobject) {
    return 0;
}

} // namespace

void *loadCore(const std::string &dir, std::string *why) {
    linker::init();
    linker::load_library("libc.so", libcSymbols());
    linker::load_library("libm.so", libmSymbols());
    linker::load_library("liblog.so", {{"__android_log_print", (void *) &logPrint},
                                       {"__android_log_vprint", (void *) &logVprint},
                                       {"__android_log_write", (void *) &logWrite}});
    linker::load_library("libandroid.so",
                         {{"ALooper_forThread", (void *) &looperForThread},
                          {"ALooper_prepare", (void *) &looperPrepare},
                          {"ALooper_addFd", (void *) &looperAddFd},
                          {"ALooper_removeFd", (void *) &looperRemoveFd},
                          {"ALooper_pollOnce", (void *) &looperPollOnce},
                          {"ALooper_pollAll", (void *) &looperPollAll}});
    linker::load_library("libjnigraphics.so",
                         {{"AndroidBitmap_getInfo", (void *) &bitmapGetInfo},
                          {"AndroidBitmap_lockPixels", (void *) &bitmapLockPixels},
                          {"AndroidBitmap_unlockPixels", (void *) &bitmapUnlockPixels}});

    // The core's DT_NEEDED on libc++_shared.so is resolved by soname from the
    // libraries already in the namespace, so libc++_shared.so is loaded first.
    linker::update_LD_LIBRARY_PATH(dir.c_str());
    if (!linker::dlopen((dir + "/libc++_shared.so").c_str(), RTLD_NOW)) {
        *why = std::string("libc++_shared.so: ") + linker::dlerror();
        return nullptr;
    }
    void *core = linker::dlopen((dir + "/libvncviewer.so").c_str(), RTLD_NOW);
    if (!core) {
        *why = std::string("libvncviewer.so: ") + linker::dlerror();
        return nullptr;
    }
    return core;
}

void *coreSymbol(void *core, const char *name) {
    return linker::dlsym(core, name);
}

} // namespace vento_realvnc
