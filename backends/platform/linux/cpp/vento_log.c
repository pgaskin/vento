// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `vento_log` for a desktop process: one line on stderr, with the time, the
// level and the tag in front, where the Android adapter has logcat. This is
// all a C backend's session file needs from the Linux platform to link
// (`linux_log_c`, in `meson.build`); the rest of `cpp/` is the C
// called by the Rust decoders, and the probe's body is in `probe/`.
//
// `vento_log_line` (`vento_log.h`) writes the line. The Linux client's Rust
// logger and its FFmpeg log callback call it too (`platform_linux::stderr`),
// so every line on the client's stderr has the same layout:
//
//     14:03:27.512 I tag: text
//
// The time is local, to the millisecond, without the date.

#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>

#include <vento/log.h>

#include "vento_log.h"

void vento_log_line(char level, const char *tag, size_t tag_len, const char *text,
                    size_t text_len) {
    struct timespec now;
    struct tm local;
    char prefix[128];
    clock_gettime(CLOCK_REALTIME, &now);
    int n;
    if (localtime_r(&now.tv_sec, &local)) {
        n = snprintf(prefix, sizeof(prefix), "%02d:%02d:%02d.%03ld %c %.*s: ", local.tm_hour,
                     local.tm_min, local.tm_sec, now.tv_nsec / 1000000, level, (int) tag_len,
                     tag);
    } else {
        n = snprintf(prefix, sizeof(prefix), "--:--:--.--- %c %.*s: ", level, (int) tag_len, tag);
    }
    if (n < 0) {
        return;
    }
    if (n >= (int) sizeof(prefix)) {
        n = (int) sizeof(prefix) - 1;
    }
    /* One write per line, so that two threads logging at once do not
       interleave halfway through a line. */
    struct iovec parts[3] = {
            {prefix, (size_t) n},
            {(void *) text, text_len},
            {"\n", 1},
    };
    (void) !writev(STDERR_FILENO, parts, 3);
}

void vento_log(int level, const char *tag, const char *fmt, ...) {
    char line[1024];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(line, sizeof(line), fmt, ap);
    va_end(ap);
    if (n < 0) {
        return;
    }
    const bool cut = n >= (int) sizeof(line);
    if (cut) {
        /* Room for the "..." that says so. */
        n = (int) sizeof(line) - 4;
    }
    /* libvncclient's lines arrive with a trailing newline, which logcat
       drops; it is dropped here too, so each message stays one line. */
    while (n > 0 && line[n - 1] == '\n') {
        n--;
    }
    if (cut) {
        line[n++] = '.';
        line[n++] = '.';
        line[n++] = '.';
    }
    vento_log_line(level == VENTO_LOG_WARN ? 'W' : 'I', tag, strlen(tag), line, (size_t) n);
}
