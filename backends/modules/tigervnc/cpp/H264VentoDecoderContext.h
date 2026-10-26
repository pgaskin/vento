// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// TigerVNC's H.264 decoder context, backed by vento's own video decoders.
//
// Their two are FFmpeg's libavcodec and Windows' Media Foundation, each a
// decoder of the library's own. This one hands the rectangle to the platform's
// `runtime::media::video::Decoders` through `vento/video.h` (the Linux
// client's FFmpeg, on VAAPI where the user chose it, and MediaCodec on
// Android), so that TigerVNC's H.264 is decoded by the same code as RustDesk's
// video on both platforms: one colour rule, one hardware path with one
// fallback, and the same panel rows about it. Made by their factory
// (`H264DecoderContext::createContext`), which this build defines in place of
// theirs, beside this class.
//
// Upstream's semantics are kept: a context per rectangle, made and reset by
// their H264Decoder exactly as before (up to 64, the oldest dropped, a reset
// flag a fresh decoder), and every picture drawn over the whole rectangle
// whatever else drew there in between. What differs is what a picture
// costs: the decoder writes only the rows that moved, straight into the
// framebuffer where the rectangle is the whole screen, and only those rows
// are damaged.

#ifndef VENTO_H264VENTODECODERCONTEXT_H
#define VENTO_H264VENTODECODERCONTEXT_H

#include <stdint.h>

#include <chrono>
#include <vector>

#include <rfb/H264DecoderContext.h>

struct vento_video_decoder;

namespace vento {
class Video;
class VideoPixelBuffer;
} // namespace vento

namespace rfb {

class H264VentoDecoderContext : public H264DecoderContext {
public:
    explicit H264VentoDecoderContext(const core::Rect &r);

    ~H264VentoDecoderContext() override;

    void decode(const uint8_t *buffer, uint32_t len, ModifiablePixelBuffer *pb) override;

private:
    /** A decoder in the mode the user has chosen now, or none. */
    void open(vento::Video *video);

    void close();

    vento::Video *video_ = nullptr;
    vento_video_decoder *decoder_ = nullptr;
    /** The mode the decoder was opened in, and whether it holds one of the
        session's hardware slots (`Video::claimHardware`). */
    int mode_ = 0;
    bool hardware_ = false;
    bool failed_ = false;

    /** Where the decoder writes when it cannot write into the framebuffer: a
        rectangle that is not the whole screen, or a framebuffer whose format
        is not the decoder's. Only the decoder writes it, so it always holds
        the decoder's last picture. */
    std::vector<uint32_t> buffer_;

    /** The framebuffer and its count of writes as of this context's last
        picture: where they still match, nothing else has drawn since, and
        only what moved need be written. */
    uint64_t fbId_ = 0;
    uint64_t fbWrites_ = 0;

    std::chrono::steady_clock::time_point lastStatus_{};
};

} // namespace rfb

#endif
