// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "H264VentoDecoderContext.h"

#include "VentoVideo.h"

#include <rfb/PixelBuffer.h>

#include <vento/log.h>
#include <vento/video.h>

#define TAG "TigerVnc"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

using namespace rfb;

namespace {

// What the decoder writes: four bytes a pixel, R, G, B, A in memory, which
// is a little-endian word with red at shift 0 on any host. It is also the
// session's framebuffer format (`fullColourPF` in `tigervnc_session.cxx`),
// which is what lets a picture go straight in.
const PixelFormat pfRGBA(32, 24, false, true, 255, 255, 255, 0, 8, 16);

// How often a context asks its decoder for the overlay's line: it is a
// string built per call, and an overlay redraws a few times a second.
constexpr std::chrono::milliseconds STATUS_EVERY{500};

} // namespace

// Their factory and the base class's destructor, in place of their
// `H264DecoderContext.cxx`, which this build does not compile: every context
// is ours.
H264DecoderContext *H264DecoderContext::createContext(const core::Rect &r) {
    return new H264VentoDecoderContext(r);
}

H264DecoderContext::~H264DecoderContext() {}

H264VentoDecoderContext::H264VentoDecoderContext(const core::Rect &r)
        : H264DecoderContext(r) {}

H264VentoDecoderContext::~H264VentoDecoderContext() {
    close();
}

void H264VentoDecoderContext::open(vento::Video *video) {
    const vento_video_decoders &t = video->table();
    mode_ = video->mode();
    // With hardware decoding off nothing is asked of the GPU, so no hardware
    // slot is taken either.
    hardware_ = mode_ != 0 && video->claimHardware();
    if (mode_ != 0 && !hardware_) {
        LOGI("H264: %d contexts already decode in hardware, so the one for %dx%d+%d+%d is "
             "software", vento::Video::MAX_HARDWARE, rect.width(), rect.height(), rect.tl.x,
             rect.tl.y);
    }
    decoder_ = t.open(t.user, VENTO_VIDEO_H264, rect.width(), rect.height(), hardware_);
    if (decoder_ == nullptr) {
        LOGW("H264: no decoder for %dx%d", rect.width(), rect.height());
        failed_ = true;
        if (hardware_) {
            video->releaseHardware();
            hardware_ = false;
        }
    }
    // A new decoder has drawn nothing yet, so whatever the framebuffer holds
    // is not its picture.
    fbId_ = 0;
}

void H264VentoDecoderContext::close() {
    if (decoder_ != nullptr) {
        video_->table().close(decoder_);
        decoder_ = nullptr;
    }
    if (hardware_) {
        video_->releaseHardware();
        hardware_ = false;
    }
}

void H264VentoDecoderContext::decode(const uint8_t *buffer, uint32_t len,
                                     ModifiablePixelBuffer *pb) {
    // Always the session's framebuffer, which is the only buffer their
    // DecodeManager decodes into; checked rather than assumed, since a
    // wrong cast here would write to unrelated memory.
    auto *fb = dynamic_cast<vento::VideoPixelBuffer *>(pb);
    vento::Video *video = fb != nullptr ? fb->video() : nullptr;
    if (video == nullptr || !video->available()) {
        if (!failed_) {
            LOGW("H264: nothing to decode with in this session");
            failed_ = true;
        }
        return;
    }
    if (failed_ || len == 0) {
        return;
    }
    // The context belongs to one session for its whole life: their
    // H264Decoder belongs to one connection, which has one session.
    video_ = video;

    if (decoder_ == nullptr) {
        open(video);
    } else if (video->mode() != mode_) {
        // The user changed how video is decoded: open a decoder in the new
        // mode, and ask for a whole picture, since the new decoder has none
        // of the pictures the stream refers back to. With a server that
        // answers a non-incremental request with a key frame, as wayvnc does,
        // the switch happens at the next picture.
        LOGI("H264: hardware decoding changed, asking for a whole picture");
        close();
        open(video);
        video->askRefresh();
    }
    if (decoder_ == nullptr) {
        return;
    }

    const vento_video_decoders &t = video->table();
    const int w = rect.width();
    const int h = rect.height();

    // Straight into the framebuffer where the rectangle is all of it and the
    // format is the decoder's: a frame is then only the rows that moved,
    // converted once, where through a buffer of our own it would be those
    // rows and then a copy of them.
    uint32_t *direct = nullptr;
    if (rect == fb->getRect() && fb->getPF() == pfRGBA) {
        int stride = 0;
        uint32_t *pixels = reinterpret_cast<uint32_t *>(fb->pixels(&stride));
        if (stride == w) {
            direct = pixels;
        }
    }

    // Whether the framebuffer still holds this decoder's last picture: only
    // then is writing what moved enough. Anything else that drew over the
    // rectangle since (another encoding, a CopyRect) means the whole picture
    // is drawn again, which is what upstream's context does with every frame.
    const uint64_t before = fb->writes();
    const bool inStep = fb->id() == fbId_ && before == fbWrites_;

    int32_t changed[4] = {};
    bool committed = false;
    bool picture;
    if (direct != nullptr) {
        picture = t.decode(decoder_, buffer, len, direct, w, h, !inStep, changed);
        if (picture && changed[2] > 0 && changed[3] > 0) {
            fb->commitBufferRW(core::Rect(changed[0], changed[1], changed[0] + changed[2],
                                          changed[1] + changed[3]));
            committed = true;
        }
    } else {
        buffer_.resize((size_t) w * (size_t) h);
        picture = t.decode(decoder_, buffer, len, buffer_.data(), w, h, false, changed);
        if (picture) {
            core::Rect drawn = rect;
            const uint32_t *from = buffer_.data();
            if (inStep) {
                drawn = core::Rect(rect.tl.x + changed[0], rect.tl.y + changed[1],
                                   rect.tl.x + changed[0] + changed[2],
                                   rect.tl.y + changed[1] + changed[3]);
                from += (size_t) changed[1] * (size_t) w + (size_t) changed[0];
            }
            if (!drawn.is_empty()) {
                // Converted where the framebuffer's format is not the
                // decoder's, and a copy where it is.
                fb->imageRect(pfRGBA, drawn, from, w);
                committed = true;
            }
        }
    }

    // In step from here if it was, or if this picture was drawn whole, but
    // only if nothing else was committed while it was being decoded, which
    // another decoder thread may have done.
    const bool nowInStep = inStep || picture;
    if (nowInStep && fb->writes() == before + (committed ? 1 : 0)) {
        fbId_ = fb->id();
        fbWrites_ = before + (committed ? 1 : 0);
    } else {
        fbId_ = 0;
    }

    if (picture) {
        const int hardware = t.hardware != nullptr ? t.hardware(decoder_) : -1;
        char line[160];
        const char *status = nullptr;
        const auto now = std::chrono::steady_clock::now();
        if (t.status != nullptr && now - lastStatus_ >= STATUS_EVERY) {
            lastStatus_ = now;
            if (t.status(decoder_, line, sizeof(line))) {
                status = line;
            }
        }
        video->report(hardware, status);
    }
    if (t.take_key_frame_request != nullptr && t.take_key_frame_request(decoder_)) {
        LOGI("H264: the decoder lost its reference pictures, asking for a whole picture");
        video->askRefresh();
    }
}
