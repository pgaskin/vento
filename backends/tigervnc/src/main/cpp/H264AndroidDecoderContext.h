// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// TigerVNC's H.264 decoder context, backed by the phone's own decoder.
//
// Their two are FFmpeg's libavcodec and Windows' Media Foundation. This is the
// third: MediaCodec, through the NDK's C API, which is why H.264 costs this
// build no vendored library at all and decodes in hardware.
//
// What it costs, on one phone against one server, an idle 1920×1200 desktop
// over 20 seconds — which is why the encoding is off until somebody asks for
// it, and why the loop below is written the way it is:
//
//   H.264   6.7% of the process, 0.04 MiB over the wire
//   Tight   2.5%,                0.17 MiB
//   ZRLE    2.2%,                0.13 MiB
//
// So a quarter of the bytes for two and a half times the processor, and a cost
// that does not know how much changed — the opposite of every other encoding
// here, and the reason this is the cheapest workload for it in bytes and the
// worst in CPU. Almost none of that is the decode, which is the hardware's: the
// YUV to RGB conversion is 19.6 ms of it per frame, down from 29.4 for the
// obvious version of the same loop, and 13.6 once the two layouts that actually
// occur — planar and semi-planar 4:2:0 — got a vectorised row. A third rather
// than the three quarters sixteen-wide arithmetic would suggest, because what
// is left is memory traffic.

#ifndef REMOTEDESKTOP_H264ANDROIDDECODERCONTEXT_H
#define REMOTEDESKTOP_H264ANDROIDDECODERCONTEXT_H

#include <media/NdkMediaCodec.h>

#include <stdint.h>

#include <rfb/H264DecoderContext.h>

namespace rfb {

class H264AndroidDecoderContext : public H264DecoderContext {
public:
    explicit H264AndroidDecoderContext(const core::Rect &r);

    ~H264AndroidDecoderContext() override;

    void decode(const uint8_t *buffer, uint32_t len, ModifiablePixelBuffer *pb) override;

private:
    /** Where one component lives in an output buffer: bytes, not pixels. */
    struct Plane {
        uint32_t offset;
        int32_t rowInc;
        int32_t colInc;
        uint32_t horizSubsampling;
        uint32_t vertSubsampling;
    };

    bool feed(const uint8_t *buffer, uint32_t len);

    /** The next frame the decoder has ready, drawn into pb. True if one was. */
    bool drawNext(ModifiablePixelBuffer *pb, int64_t timeoutUs);

    void readFormat(AMediaFormat *format);

    void draw(const uint8_t *base, size_t size, ModifiablePixelBuffer *pb);

    AMediaCodec *codec_ = nullptr;
    bool failed_ = false;
    bool sawFrame_ = false;
    int64_t pts_ = 0;

    // What the decoder has just said about its output. Re-read per buffer,
    // because it is per buffer that it is true.
    Plane planes_[3] = {};
    bool haveLayout_ = false;
    int32_t frameWidth_ = 0;
    int32_t frameHeight_ = 0;
    bool fullRange_ = false;
    bool bt709_ = false;

    // Whether the vectorised row conversion may be used, and the layout that
    // answer was reached for. Checked against the scalar one on a real row of a
    // real frame rather than asserted: the fallback exists for a decoder that
    // describes its planes in some way this does not cover, and a wrong picture
    // is a worse failure than a slow one. A decoder reports its layout per
    // buffer, so the answer is keyed on the three fields the conversion reads.
#if defined(__aarch64__)
    bool vectorise_ = false;
    int32_t checkedYColInc_ = -1;
    int32_t checkedUvColInc_ = -1;
    uint32_t checkedSubsampling_ = 0;
#endif
};

} // namespace rfb

#endif
