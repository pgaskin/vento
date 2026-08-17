// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// What this session has received, which the viewer core's JNI surface does not
// offer: 320 exported functions and not one byte count among them. The number
// is in the core's own bandwidth estimator — the class its logger calls
// `LineSpeedEstimator` — and this file is the walk to it.
//
// From the session pointer, all of it read out of `getLineSpeed`:
//
//     A   = *(token + 0x1c8)        token is the AndroidSession*
//     C   = *(A     + 0x1c8)        null until there is a connection
//     W   = C + 0x2e8               the estimator's owner
//     Est = W + 8                   the estimator, six words and two flags
//
//     Est+0x00  s64  elapsed in the current window, 100 ns units
//     Est+0x08  s64  kilobits accumulated in the current window
//     Est+0x20  s64  round-trip time, 100 ns units
//     Est+0x28  u32  the stream position the current window started at
//
// The window is the framebuffer-update request/complete pair, so the mark moves
// several times a second in a live session; it is cumulative and monotonic, and
// it is 32 bits, so a total has to be accumulated from deltas by the caller.
//
// **Nothing here dereferences an address it has not proved is mapped.** A walk
// through somebody else's stripped binary is a dependency `check-jni-abi.sh`
// cannot see — the library is pinned by SHA-256, but a pin does not say what is
// at +0x1c8 — and the failure mode of a wrong offset is a SIGSEGV in a
// stranger's session rather than an error anybody can report. write(2) answers
// EFAULT for an unreadable source instead of raising a signal, so every hop is
// copied through a pipe: the bytes that come back out are the bytes the kernel
// validated going in, which leaves no window between the check and the read.

#define _GNU_SOURCE // pipe2

#include <fcntl.h>
#include <jni.h>
#include <pthread.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#define HOP_A 0x1c8
#define HOP_C 0x1c8
#define OWNER 0x2e8
#define EST 8

#define EST_ELAPSED 0x00
#define EST_KILOBITS 0x08
#define EST_RTT 0x20
#define EST_MARK 0x28
#define EST_BYTES 0x30

static pthread_mutex_t probe_lock = PTHREAD_MUTEX_INITIALIZER;
static int probe_pipe[2] = {-1, -1};

// Both ends non-blocking: a full pipe or an empty one must fail the read rather
// than park the session thread in the kernel.
static int probe_open(void) {
    if (probe_pipe[0] < 0 && pipe2(probe_pipe, O_CLOEXEC | O_NONBLOCK) != 0) {
        return 0;
    }
    return probe_pipe[0] >= 0;
}

/** Copy len bytes from addr, or fail if any of them is unmapped. */
static int probe_read(void *dst, uintptr_t addr, size_t len) {
    int ok = 0;
    char sink[EST_BYTES];
    pthread_mutex_lock(&probe_lock);
    if (probe_open()) {
        const ssize_t n = write(probe_pipe[1], (const void *) addr, len);
        if (n == (ssize_t) len) {
            ok = read(probe_pipe[0], dst, len) == (ssize_t) len;
        } else if (n > 0) {
            read(probe_pipe[0], sink, (size_t) n); // a short write leaves the pipe dirty
        }
    }
    pthread_mutex_unlock(&probe_lock);
    return ok;
}

// A pointer that is misaligned is not one of theirs, whether or not it happens
// to be mapped: LP64 puts every member of this chain on eight.
static int hop(uintptr_t base, size_t offset, uintptr_t *out) {
    uintptr_t value;
    if (!probe_read(&value, base + offset, sizeof value)) {
        return 0;
    }
    *out = value;
    return value != 0 && (value & 7) == 0;
}

JNIEXPORT jboolean JNICALL
Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncTraffic_nativeEstimator(
        JNIEnv *env, jclass clazz, jlong token, jlongArray out) {
    (void) clazz;
    if (token == 0 || out == NULL || (*env)->GetArrayLength(env, out) < 4) {
        return JNI_FALSE;
    }
    uintptr_t a, c;
    if (!hop((uintptr_t) token, HOP_A, &a) || !hop(a, HOP_C, &c)) {
        return JNI_FALSE;
    }
    unsigned char est[EST_BYTES];
    if (!probe_read(est, c + OWNER + EST, sizeof est)) {
        return JNI_FALSE;
    }

    int64_t elapsed, kilobits, rtt;
    uint32_t mark;
    memcpy(&elapsed, est + EST_ELAPSED, sizeof elapsed);
    memcpy(&kilobits, est + EST_KILOBITS, sizeof kilobits);
    memcpy(&rtt, est + EST_RTT, sizeof rtt);
    memcpy(&mark, est + EST_MARK, sizeof mark);

    const jlong values[] = {elapsed, kilobits, rtt, (jlong) mark};
    (*env)->SetLongArrayRegion(env, out, 0, 4, values);
    return JNI_TRUE;
}
