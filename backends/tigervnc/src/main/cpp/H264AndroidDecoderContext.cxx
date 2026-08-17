// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

#include "H264AndroidDecoderContext.h"

#include <android/log.h>
#include <media/NdkMediaFormat.h>

#if defined(__aarch64__)
#include <arm_neon.h>
#endif

#include <stdlib.h>
#include <string.h>

#include <vector>

#include <rfb/PixelBuffer.h>

#define TAG "TigerVnc"
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, TAG, __VA_ARGS__)

using namespace rfb;

namespace {

// MediaCodec's own constants, which the NDK headers do not declare: they are
// Java fields on MediaCodecInfo.CodecCapabilities and MediaFormat. Asking a
// decoder for Flexible is what makes it describe its output in the terms below
// rather than name a vendor layout no portable code could read.
const int32_t COLOR_FormatYUV420Flexible = 0x7F420888;
const int32_t COLOR_STANDARD_BT709 = 1;
const int32_t COLOR_RANGE_FULL = 1;

// `image-data` is a MediaImage2, the same descriptor Java's Image is built
// from: where each component starts, and what a row and a column of it cost in
// bytes. It is the whole reason nothing here has a table of vendor colour
// formats. A decoder asked for Flexible reports Flexible and nothing more, and
// the alternative is a list of the layouts each vendor means by it; this way
// the decoder says where its planes are, and planar and semi-planar differ only
// in a column increment.
const size_t MEDIA_IMAGE2_HEADER = 24; // type, planes, width, height, 2 × depth
const size_t MEDIA_IMAGE2_PLANE = 20;  // offset, colInc, rowInc, 2 × subsampling
const uint32_t MEDIA_IMAGE_TYPE_YUV = 1;

// How long a decode waits for the frame it just fed. A frame that misses is not
// lost — the next update draws it — so this bounds what a stall costs a decoder
// thread rather than being a deadline. The first is longer because it is the
// one the decoder is still working its output format out on.
const int64_t FIRST_FRAME_TIMEOUT_US = 200000;
const int64_t FRAME_TIMEOUT_US = 30000;

/** Y, then the four chroma terms, in 1/256ths, and what Y is offset by. */
struct Coefficients {
    int y, rv, gu, gv, bu, yOffset;
};

Coefficients coefficients(bool bt709, bool full) {
    if (full) {
        return bt709 ? Coefficients{256, 403, -48, -120, 475, 0}
                     : Coefficients{256, 359, -88, -183, 454, 0};
    }
    return bt709 ? Coefficients{298, 459, -55, -136, 541, -16}
                 : Coefficients{298, 409, -100, -208, 516, -16};
}

/** The three increments a row conversion needs, out of the plane descriptors. */
struct H264Layout {
    int32_t yColInc;
    int32_t uvColInc;
    uint32_t horizSubsampling;
};

inline uint8_t clamp(int v) {
    return (uint8_t) (v < 0 ? 0 : v > 255 ? 255 : v);
}

/**
 * One row of 4:2:0 to RGBA, scalar, from `col` to the end of the row.
 *
 * Walked rather than indexed, and the chroma terms computed once per chroma
 * sample rather than once per pixel. Both matter: a division by the subsampling
 * inside this loop, which is the obvious way to write it, cost 29 ms of a
 * decoder thread for one 1920×1200 frame — more than the hardware decoder
 * spends on the whole picture.
 */
void rowScalar(const uint8_t *y, const uint8_t *u, const uint8_t *v,
               const H264Layout &l, uint32_t *out, int col, int w,
               const Coefficients &c) {
    y += (size_t) col * l.yColInc;
    u += (size_t) (col / (int) l.horizSubsampling) * l.uvColInc;
    v += (size_t) (col / (int) l.horizSubsampling) * l.uvColInc;
    int chroma = 0;
    int rv = 0, gu = 0, bu = 0;
    for (; col < w; col++) {
        if (chroma == 0) {
            const int uu = *u - 128;
            const int vv = *v - 128;
            rv = c.rv * vv + 128;
            gu = c.gu * uu + c.gv * vv + 128;
            bu = c.bu * uu + 128;
            u += l.uvColInc;
            v += l.uvColInc;
            chroma = (int) l.horizSubsampling;
        }
        chroma--;
        const int yy = (*y + c.yOffset) * c.y;
        y += l.yColInc;
        // Red at shift 0, which is what the framebuffer's format is, so this is
        // the byte order Android's ARGB_8888 reads.
        out[col] = (uint32_t) clamp((yy + rv) >> 8)
                | ((uint32_t) clamp((yy + gu) >> 8) << 8)
                | ((uint32_t) clamp((yy + bu) >> 8) << 16)
                | 0xff000000u;
    }
}

#if defined(__aarch64__)

/**
 * The same row, sixteen pixels at a time. Returns how many it did, which is
 * always a multiple of sixteen and may be none — the tail and every layout not
 * covered here go back through the scalar loop, because a decoder that
 * describes its planes differently must still produce a picture.
 *
 * Sixteen rather than eight because that is what makes both chroma loads whole:
 * eight chroma samples is one `vld1_u8` where the planes are separate and one
 * `vld2_u8` where they are interleaved, and neither reads a byte the sixteen
 * luma pixels do not account for.
 */
int rowNeon(const uint8_t *y, const uint8_t *u, const uint8_t *v,
            const H264Layout &l, uint32_t *out, int w, const Coefficients &c) {
    if (l.yColInc != 1 || l.horizSubsampling != 2) {
        return 0;
    }
    const bool interleaved = l.uvColInc == 2 && (v == u + 1 || u == v + 1);
    if (l.uvColInc != 1 && !interleaved) {
        return 0;
    }
    const int16x8_t yOff = vdupq_n_s16((int16_t) c.yOffset);
    const int16x4_t cy = vdup_n_s16((int16_t) c.y);
    const int16x4_t crv = vdup_n_s16((int16_t) c.rv);
    const int16x4_t cgu = vdup_n_s16((int16_t) c.gu);
    const int16x4_t cgv = vdup_n_s16((int16_t) c.gv);
    const int16x4_t cbu = vdup_n_s16((int16_t) c.bu);
    const int32x4_t half = vdupq_n_s32(128);
    const uint8x8_t opaque = vdup_n_u8(0xff);

    int col = 0;
    for (; col + 16 <= w; col += 16, y += 16, u += 8 * l.uvColInc, v += 8 * l.uvColInc) {
        uint8x8_t ub, vb;
        if (interleaved) {
            // NV12 has U first and NV21 V first; one load covers both planes
            // either way, and which half is which is the only difference.
            const uint8x8x2_t pair = vld2_u8(v == u + 1 ? u : v);
            ub = v == u + 1 ? pair.val[0] : pair.val[1];
            vb = v == u + 1 ? pair.val[1] : pair.val[0];
        } else {
            ub = vld1_u8(u);
            vb = vld1_u8(v);
        }
        const int16x8_t uu = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(ub)), vdupq_n_s16(128));
        const int16x8_t vv = vsubq_s16(vreinterpretq_s16_u16(vmovl_u8(vb)), vdupq_n_s16(128));

        // Eight chroma terms, then each duplicated onto the two luma pixels it
        // covers — which is what vzipq_s32 does in one instruction.
        int32x4_t rv[2], gu[2], bu[2];
        for (int half_i = 0; half_i < 2; half_i++) {
            const int16x4_t uh = half_i ? vget_high_s16(uu) : vget_low_s16(uu);
            const int16x4_t vh = half_i ? vget_high_s16(vv) : vget_low_s16(vv);
            rv[half_i] = vaddq_s32(vmull_s16(vh, crv), half);
            gu[half_i] = vaddq_s32(vaddq_s32(vmull_s16(uh, cgu), vmull_s16(vh, cgv)), half);
            bu[half_i] = vaddq_s32(vmull_s16(uh, cbu), half);
        }

        const uint8x16_t yraw = vld1q_u8(y);
        const int16x8_t ylo = vaddq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(yraw))), yOff);
        const int16x8_t yhi = vaddq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_high_u8(yraw))), yOff);
        const int32x4_t yy[4] = {
                vmull_s16(vget_low_s16(ylo), cy), vmull_s16(vget_high_s16(ylo), cy),
                vmull_s16(vget_low_s16(yhi), cy), vmull_s16(vget_high_s16(yhi), cy),
        };

        for (int block = 0; block < 2; block++) {
            // Two quads of luma against one quad of chroma, doubled.
            const int32x4x2_t r = vzipq_s32(rv[block], rv[block]);
            const int32x4x2_t g = vzipq_s32(gu[block], gu[block]);
            const int32x4x2_t b = vzipq_s32(bu[block], bu[block]);
            uint8x8x4_t px;
            // vqshrun saturates the negative side to zero and vqmovn the high
            // side to 255, so the scalar clamp is two instructions here.
            px.val[0] = vqmovn_u16(vcombine_u16(
                    vqshrun_n_s32(vaddq_s32(yy[block * 2], r.val[0]), 8),
                    vqshrun_n_s32(vaddq_s32(yy[block * 2 + 1], r.val[1]), 8)));
            px.val[1] = vqmovn_u16(vcombine_u16(
                    vqshrun_n_s32(vaddq_s32(yy[block * 2], g.val[0]), 8),
                    vqshrun_n_s32(vaddq_s32(yy[block * 2 + 1], g.val[1]), 8)));
            px.val[2] = vqmovn_u16(vcombine_u16(
                    vqshrun_n_s32(vaddq_s32(yy[block * 2], b.val[0]), 8),
                    vqshrun_n_s32(vaddq_s32(yy[block * 2 + 1], b.val[1]), 8)));
            px.val[3] = opaque;
            vst4_u8((uint8_t *) (out + col + block * 8), px);
        }
    }
    return col;
}

/**
 * Whether the two agree on this decoder's first row, which is what decides
 * that the vectorised one may be used at all.
 *
 * One row of one frame, once per layout: the arithmetic is the same for every
 * row, and what is actually in doubt is whether the planes are where the
 * descriptor says — which a whole row of real pixels answers and an assertion
 * about colInc does not.
 */
bool neonAgrees(const uint8_t *y, const uint8_t *u, const uint8_t *v,
                const H264Layout &l, int w, const Coefficients &c) {
    std::vector<uint32_t> scalar((size_t) w), vector((size_t) w);
    rowScalar(y, u, v, l, scalar.data(), 0, w, c);
    const int done = rowNeon(y, u, v, l, vector.data(), w, c);
    if (done == 0) {
        return false;
    }
    rowScalar(y, u, v, l, vector.data(), done, w, c);
    return memcmp(scalar.data(), vector.data(), (size_t) w * sizeof(uint32_t)) == 0;
}

#endif

int32_t readInt(const uint8_t *p) {
    int32_t v;
    memcpy(&v, p, sizeof(v));
    return v;
}

} // namespace

H264AndroidDecoderContext::H264AndroidDecoderContext(const core::Rect &r)
        : H264DecoderContext(r) {
    codec_ = AMediaCodec_createDecoderByType("video/avc");
    if (codec_ == nullptr) {
        LOGW("H264: no AVC decoder on this device");
        failed_ = true;
        return;
    }

    AMediaFormat *format = AMediaFormat_new();
    AMediaFormat_setString(format, AMEDIAFORMAT_KEY_MIME, "video/avc");
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_WIDTH, r.width());
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_HEIGHT, r.height());
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_COLOR_FORMAT, COLOR_FormatYUV420Flexible);
    // A remote desktop is the case this flag exists for: the decoder must not
    // hold frames back to reorder them, because there is nothing to reorder —
    // and a frame held is a desktop that lags behind the finger on it.
    AMediaFormat_setInt32(format, AMEDIAFORMAT_KEY_LOW_LATENCY, 1);

    // No surface: the pixels have to come back to the CPU, because what the
    // seam promises is a region read out of a framebuffer.
    const media_status_t status =
            AMediaCodec_configure(codec_, format, nullptr, nullptr, 0);
    AMediaFormat_delete(format);
    if (status != AMEDIA_OK || AMediaCodec_start(codec_) != AMEDIA_OK) {
        LOGW("H264: could not start the decoder for %dx%d (%d)",
             r.width(), r.height(), status);
        AMediaCodec_delete(codec_);
        codec_ = nullptr;
        failed_ = true;
        return;
    }
}

H264AndroidDecoderContext::~H264AndroidDecoderContext() {
    if (codec_ != nullptr) {
        AMediaCodec_stop(codec_);
        AMediaCodec_delete(codec_);
    }
}

void H264AndroidDecoderContext::decode(const uint8_t *buffer, uint32_t len,
                                       ModifiablePixelBuffer *pb) {
    if (failed_ || len == 0) {
        return;
    }
    if (!feed(buffer, len)) {
        return;
    }
    if (drawNext(pb, sawFrame_ ? FRAME_TIMEOUT_US : FIRST_FRAME_TIMEOUT_US)) {
        sawFrame_ = true;
        // Anything else already decoded, in order, so a decode that fell behind
        // catches up inside this update rather than over the next several.
        while (drawNext(pb, 0)) {
        }
    }
}

bool H264AndroidDecoderContext::feed(const uint8_t *buffer, uint32_t len) {
    while (len > 0) {
        const ssize_t index = AMediaCodec_dequeueInputBuffer(codec_, FRAME_TIMEOUT_US);
        if (index < 0) {
            LOGW("H264: no input buffer, dropping %u bytes", len);
            return false;
        }
        size_t capacity = 0;
        uint8_t *in = AMediaCodec_getInputBuffer(codec_, (size_t) index, &capacity);
        if (in == nullptr || capacity == 0) {
            return false;
        }

        // An access unit larger than one input buffer is split at a NAL
        // boundary rather than anywhere: a decoder reassembles by start code,
        // and a buffer cut through the middle of one is a corrupt frame.
        uint32_t take = len;
        if (take > capacity) {
            take = 0;
            for (uint32_t i = 3; i + 3 < capacity; i++) {
                if (buffer[i] == 0 && buffer[i + 1] == 0 && buffer[i + 2] == 1) {
                    take = buffer[i - 1] == 0 ? i - 1 : i;
                }
            }
            if (take == 0) {
                LOGW("H264: %u bytes with no NAL boundary under %zu", len, capacity);
                return false;
            }
        }
        memcpy(in, buffer, take);
        AMediaCodec_queueInputBuffer(codec_, (size_t) index, 0, take, pts_, 0);
        buffer += take;
        len -= take;
        pts_ += 1000;
    }
    return true;
}

bool H264AndroidDecoderContext::drawNext(ModifiablePixelBuffer *pb, int64_t timeoutUs) {
    for (;;) {
        AMediaCodecBufferInfo info;
        const ssize_t index = AMediaCodec_dequeueOutputBuffer(codec_, &info, timeoutUs);
        if (index == AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED
                || index == AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED) {
            continue;
        }
        if (index < 0) {
            return false; // TRY_AGAIN_LATER, or an error this cannot act on
        }

        // The buffer's own format rather than the stream's: a layout is a
        // property of the buffer about to be read, and the first buffer of a
        // session does not have the same one as the rest.
        AMediaFormat *format = AMediaCodec_getBufferFormat(codec_, (size_t) index);
        if (format != nullptr) {
            readFormat(format);
            AMediaFormat_delete(format);
        }

        size_t size = 0;
        const uint8_t *out = AMediaCodec_getOutputBuffer(codec_, (size_t) index, &size);
        if (out != nullptr && info.size > 0) {
            draw(out + info.offset, (size_t) info.size, pb);
        }
        AMediaCodec_releaseOutputBuffer(codec_, (size_t) index, false);
        return true;
    }
}

void H264AndroidDecoderContext::readFormat(AMediaFormat *format) {
    AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_WIDTH, &frameWidth_);
    AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_HEIGHT, &frameHeight_);
    // The visible rectangle, where the coded one is padded up to a macroblock.
    int32_t left, top, right, bottom;
    if (AMediaFormat_getRect(format, AMEDIAFORMAT_KEY_DISPLAY_CROP,
                             &left, &top, &right, &bottom)) {
        frameWidth_ = right - left + 1;
        frameHeight_ = bottom - top + 1;
    }

    // Both of these are the stream's own, out of the bitstream's VUI. A stream
    // that says nothing is taken for what the encoders this has met produce.
    int32_t standard = 0, range = 0;
    bt709_ = !AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_COLOR_STANDARD, &standard)
            || standard == COLOR_STANDARD_BT709 || standard == 0;
    fullRange_ = AMediaFormat_getInt32(format, AMEDIAFORMAT_KEY_COLOR_RANGE, &range)
            && range == COLOR_RANGE_FULL;

    void *data = nullptr;
    size_t size = 0;
    haveLayout_ = false;
    if (!AMediaFormat_getBuffer(format, "image-data", &data, &size) || data == nullptr
            || size < MEDIA_IMAGE2_HEADER + 3 * MEDIA_IMAGE2_PLANE) {
        LOGW("H264: the decoder did not describe its output planes");
        failed_ = true;
        return;
    }
    const uint8_t *p = (const uint8_t *) data;
    if ((uint32_t) readInt(p) != MEDIA_IMAGE_TYPE_YUV || (uint32_t) readInt(p + 4) < 3) {
        LOGW("H264: the decoder's output is not three-plane YUV");
        failed_ = true;
        return;
    }
    for (int i = 0; i < 3; i++) {
        const uint8_t *q = p + MEDIA_IMAGE2_HEADER + (size_t) i * MEDIA_IMAGE2_PLANE;
        planes_[i].offset = (uint32_t) readInt(q);
        planes_[i].colInc = readInt(q + 4);
        planes_[i].rowInc = readInt(q + 8);
        planes_[i].horizSubsampling = (uint32_t) readInt(q + 12);
        planes_[i].vertSubsampling = (uint32_t) readInt(q + 16);
        if (planes_[i].horizSubsampling == 0 || planes_[i].vertSubsampling == 0
                || planes_[i].colInc <= 0 || planes_[i].rowInc <= 0) {
            LOGW("H264: plane %d has a layout this cannot read", i);
            failed_ = true;
            return;
        }
    }
    haveLayout_ = true;
}

void H264AndroidDecoderContext::draw(const uint8_t *base, size_t size,
                                     ModifiablePixelBuffer *pb) {
    const int w = rect.width() < frameWidth_ ? rect.width() : frameWidth_;
    const int h = rect.height() < frameHeight_ ? rect.height() : frameHeight_;
    if (!haveLayout_ || w <= 0 || h <= 0) {
        return;
    }
    // Every read below is inside the buffer if the last one of each plane is,
    // and a decoder that has just changed resolution is where that matters.
    for (const Plane &plane : planes_) {
        const size_t last = plane.offset
                + (size_t) ((h - 1) / plane.vertSubsampling) * plane.rowInc
                + (size_t) ((w - 1) / plane.horizSubsampling) * plane.colInc;
        if (last >= size) {
            LOGW("H264: a %dx%d frame does not fit %zu bytes", w, h, size);
            return;
        }
    }

    core::Rect drawn = rect;
    drawn.br.x = drawn.tl.x + w;
    drawn.br.y = drawn.tl.y + h;
    // Straight into the framebuffer rather than through a buffer of our own.
    // Converting into one and handing that to imageRect is the same picture and
    // three times the memory traffic — 9 MiB written, then read and written
    // again — which for a frame this size is most of what the conversion costs.
    int dstStride = 0;
    uint32_t *dst = (uint32_t *) pb->getBufferRW(drawn, &dstStride);
    if (dst == nullptr) {
        return;
    }

    const Plane &yp = planes_[0];
    const Plane &up = planes_[1];
    const Plane &vp = planes_[2];
    const Coefficients c = coefficients(bt709_, fullRange_);
    const H264Layout layout{yp.colInc, up.colInc, up.horizSubsampling};
#if defined(__aarch64__)
    if (checkedYColInc_ != layout.yColInc || checkedUvColInc_ != layout.uvColInc
            || checkedSubsampling_ != layout.horizSubsampling) {
        checkedYColInc_ = layout.yColInc;
        checkedUvColInc_ = layout.uvColInc;
        checkedSubsampling_ = layout.horizSubsampling;
        vectorise_ = neonAgrees(base + yp.offset, base + up.offset, base + vp.offset,
                                layout, w, c);
        LOGW("H264: the vectorised conversion is %s for this decoder's layout",
             vectorise_ ? "in use" : "off");
    }
#endif
    for (int row = 0; row < h; row++) {
        const uint8_t *y = base + yp.offset + (size_t) row * yp.rowInc;
        const uint8_t *u = base + up.offset
                + (size_t) (row / up.vertSubsampling) * up.rowInc;
        const uint8_t *v = base + vp.offset
                + (size_t) (row / vp.vertSubsampling) * vp.rowInc;
        uint32_t *out = dst + (size_t) row * dstStride;

        int col = 0;
#if defined(__aarch64__)
        if (vectorise_) {
            col = rowNeon(y, u, v, layout, out, w, c);
        }
#endif
        rowScalar(y, u, v, layout, out, col, w, c);
    }

    pb->commitBufferRW(drawn);
}
