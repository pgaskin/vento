// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The far end's sound, played on the phone: the sink for TigerVNC's QEMU audio
// extension, which is the protocol half of it in `common/` and a PulseAudio or
// waveOut sink in their viewer. This is the third sink, on AAudio.
//
// The policy is theirs, numbers included — at most a second of audio queued
// ahead, and 20 ms of silence in front of every stream so that a packet arriving
// a little late does not leave the device with nothing to play. What differs is
// where the queue lives. Theirs is a ring of their own that PulseAudio's thread
// drains; this one is AAudio's own buffer, sized to that second and written with
// a timeout of zero. A write puts in what fits and the rest is dropped on the
// floor, which is the jitter bound doing its job — the samples that did not fit
// are ones that could never have been played in time — and it never waits,
// which matters because the caller is the protocol thread: a sink that blocked
// it would stall the picture behind the sound. So there is no callback, no
// second buffer and no lock, and everything here happens on one thread.
//
// Opened on the first stream the server begins rather than with the session,
// since most servers never send any, and the phone's output is not held for a
// session that has nothing to play. A stream that will not open is said once in
// the log, and everything after it is swallowed: sound is not worth ending a
// session over.

#ifndef REMOTEDESKTOP_ANDROIDAUDIOOUTPUT_H
#define REMOTEDESKTOP_ANDROIDAUDIOOUTPUT_H

#include <aaudio/AAudio.h>

#include <stddef.h>
#include <stdint.h>

class AndroidAudioOutput {
public:
    AndroidAudioOutput() = default;

    ~AndroidAudioOutput();

    AndroidAudioOutput(const AndroidAudioOutput &) = delete;

    AndroidAudioOutput &operator=(const AndroidAudioOutput &) = delete;

    /** The server has started sending: open if need be, start, pre-roll. */
    void start();

    /** The server has stopped: let what is queued play out, then stop. */
    void stop();

    /** A chunk of samples in the agreed format, aligned to nothing at all. */
    void play(const uint8_t *data, size_t length);

private:
    bool open();

    void close();

    /** Whatever the output is now, once; false, and silence from then on. */
    bool reopen(aaudio_result_t why);

    void preroll();

    /** Frames, not bytes; what does not fit is dropped. */
    void write(const void *frames, int32_t count);

    AAudioStream *stream_ = nullptr;
    bool failed_ = false;
    bool started_ = false;

    // A chunk can end partway through a frame, and the next one then starts
    // partway through it too. Dropping the stub would shift every channel
    // after it, so it is carried over instead.
    uint8_t partial_[4] = {};
    size_t partialLength_ = 0;
};

#endif
