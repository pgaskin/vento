// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "AndroidAudioOutput.h"

#include <android/log.h>

#include <string.h>

#include <rfb/qemuTypes.h>

#define TAG "TigerVnc"
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, TAG, __VA_ARGS__)

namespace {

// What CConnection asks every server for. It keeps the three to itself, in
// CConnection.cxx, and their viewer restates them in its own header the same
// way; if theirs ever change, these have to follow.
constexpr uint8_t SAMPLE_FORMAT = rfb::qemuAudioFormatS16;
constexpr int32_t CHANNELS = 2;
constexpr int32_t FREQUENCY = 48000;

// A frame is one sample for each channel. Native-endian on both ends, which is
// what AAudio's I16 is.
constexpr size_t FRAME_BYTES = (size_t) CHANNELS << (SAMPLE_FORMAT >> 1);
static_assert(SAMPLE_FORMAT == rfb::qemuAudioFormatS16 && FRAME_BYTES == 4,
              "the stream below is opened as 16-bit stereo");

// Their viewer's two numbers.
constexpr int32_t MAX_JITTER_MS = 1000;
constexpr int32_t MIN_STREAM_DELAY_MS = 20;

constexpr int32_t MAX_JITTER_FRAMES = MAX_JITTER_MS * FREQUENCY / 1000;
constexpr int32_t PREROLL_FRAMES = MIN_STREAM_DELAY_MS * FREQUENCY / 1000;

} // namespace

AndroidAudioOutput::~AndroidAudioOutput() {
    // The session is ending, so whatever is still queued goes with it.
    close();
}

bool AndroidAudioOutput::open() {
    if (stream_ != nullptr) {
        return true;
    }
    if (failed_) {
        return false;
    }
    AAudioStreamBuilder *builder = nullptr;
    aaudio_result_t result = AAudio_createStreamBuilder(&builder);
    if (result == AAUDIO_OK) {
        AAudioStreamBuilder_setDirection(builder, AAUDIO_DIRECTION_OUTPUT);
        AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_I16);
        AAudioStreamBuilder_setChannelCount(builder, CHANNELS);
        AAudioStreamBuilder_setSampleRate(builder, FREQUENCY);
        AAudioStreamBuilder_setUsage(builder, AAUDIO_USAGE_MEDIA);
        AAudioStreamBuilder_setContentType(builder, AAUDIO_CONTENT_TYPE_MUSIC);
        // Room for the whole second, which is a request: what the device
        // grants is read back below and may be less.
        AAudioStreamBuilder_setBufferCapacityInFrames(builder, MAX_JITTER_FRAMES);
        result = AAudioStreamBuilder_openStream(builder, &stream_);
        AAudioStreamBuilder_delete(builder);
    }
    if (result != AAUDIO_OK) {
        stream_ = nullptr;
        failed_ = true;
        LOGW("audio: could not open an output stream (%s); the server's sound is dropped",
             AAudio_convertResultToText(result));
        return false;
    }
    // The buffer size, not the capacity, is what a write fills up to — so
    // this is the line the jitter bound is drawn at.
    int32_t size = AAudioStream_getBufferCapacityInFrames(stream_);
    if (size > MAX_JITTER_FRAMES) {
        size = MAX_JITTER_FRAMES;
    }
    AAudioStream_setBufferSizeInFrames(stream_, size);
    return true;
}

void AndroidAudioOutput::close() {
    if (stream_ != nullptr) {
        AAudioStream_requestStop(stream_);
        AAudioStream_close(stream_);
        stream_ = nullptr;
    }
    started_ = false;
}

void AndroidAudioOutput::start() {
    partialLength_ = 0;
    if (!open()) {
        return;
    }
    // From stopping as well as stopped: a stream the server begins again
    // before the last one has drained carries straight on. Already started
    // is the one refusal that is not a failure.
    const aaudio_result_t result = AAudioStream_requestStart(stream_);
    if (result != AAUDIO_OK && result != AAUDIO_ERROR_INVALID_STATE && !reopen(result)) {
        return;
    }
    started_ = true;
    preroll();
}

bool AndroidAudioOutput::reopen(aaudio_result_t why) {
    // Most likely the output moved — headphones, a car — which leaves a
    // stream disconnected for good. One more try on whatever the output is
    // now, and then silence for the rest of the session: once per change of
    // output is a price the protocol thread can pay, once per chunk is not.
    close();
    if (open()) {
        const aaudio_result_t result = AAudioStream_requestStart(stream_);
        if (result == AAUDIO_OK) {
            return true;
        }
        why = result;
    }
    if (!failed_) {
        LOGW("audio: the output stream stopped working (%s); the server's sound is dropped",
             AAudio_convertResultToText(why));
    }
    close();
    failed_ = true;
    return false;
}

// Straight to the stream rather than through write(): a stream that has
// just been reopened is not one to reopen again over its silence.
void AndroidAudioOutput::preroll() {
    static const int16_t silence[PREROLL_FRAMES * CHANNELS] = {};
    AAudioStream_write(stream_, silence, PREROLL_FRAMES, 0);
}

void AndroidAudioOutput::stop() {
    // A stop drains what is queued rather than dropping it, so the tail of
    // the stream is still heard, and the call itself does not wait for that.
    if (stream_ != nullptr && started_) {
        AAudioStream_requestStop(stream_);
    }
    started_ = false;
}

void AndroidAudioOutput::play(const uint8_t *data, size_t length) {
    if (stream_ == nullptr || !started_ || length == 0) {
        return;
    }
    if (partialLength_ > 0) {
        const size_t take = FRAME_BYTES - partialLength_ < length
                ? FRAME_BYTES - partialLength_ : length;
        memcpy(partial_ + partialLength_, data, take);
        partialLength_ += take;
        data += take;
        length -= take;
        if (partialLength_ < FRAME_BYTES) {
            return;
        }
        write(partial_, 1);
        partialLength_ = 0;
    }
    const size_t whole = length / FRAME_BYTES;
    // Theirs are at most a mebibyte, far inside an int32_t of frames.
    if (whole > 0) {
        write(data, (int32_t) whole);
    }
    partialLength_ = length - whole * FRAME_BYTES;
    memcpy(partial_, data + whole * FRAME_BYTES, partialLength_);
}

void AndroidAudioOutput::write(const void *frames, int32_t count) {
    if (stream_ == nullptr) {
        return;
    }
    // A timeout of zero is what makes this safe on the protocol thread: it
    // takes what fits and returns.
    const aaudio_result_t result = AAudioStream_write(stream_, frames, count, 0);
    if (result == AAUDIO_ERROR_DISCONNECTED && reopen(result)) {
        // A new stream is an empty one, and what was lost with the old one
        // is gone either way; the pre-roll is what keeps this chunk from
        // being played straight into an underrun.
        started_ = true;
        preroll();
        AAudioStream_write(stream_, frames, count, 0);
    }
}
