// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The module's rdpsnd device for FreeRDP: the library's device interface
// (freerdp/client/rdpsnd.h, rdpsndDevicePlugin) over vento's output. See
// rdpsnd_vento.h for why it exists and how the channel finds it.
//
// What the channel asks of a device: whether it plays a format the server
// offers, a stream opened in one of them when the server starts sending,
// each wave's samples as they arrive, the server's volume, and the delay
// until the samples just passed are heard, which goes back to the server in
// the wave's confirm and tells it the client's share of the latency. The
// table (vento/audio.h) does all of it: the queue, opening the device off
// this thread, the drop past a quarter of a second, the session's gain and
// mute, and the decoding when the server sends AAC or Opus, done by the
// platform's decoders (libavcodec on Linux, MediaCodec on Android) rather
// than the library's DSP. The channel calls into a device from one thread at
// a time, and no call waits on the platform's device.
//
// The channel never reports when the sound has stopped: the server's Close
// PDU is only logged, so a device it opened stays open, and an open stream
// keeps the platform's device playing silence and the session reporting that
// it plays. So the device closes its own stream once its last samples have
// been heard and two more seconds have passed, as TigerVNC's does at its
// server's AudioEnd, and the next wave opens another.

#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <freerdp/addin.h>
#include <freerdp/channels/rdpsnd.h>
#include <freerdp/client/channels.h>
#include <freerdp/client/rdpsnd.h>
#include <freerdp/codec/audio.h>
#include <freerdp/codec/dsp.h>
#include <winpr/cast.h>
#include <winpr/wtsapi.h>

#include <vento/log.h>

#include "rdpsnd_vento.h"

#define TAG "FreeRdp"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

/* How long a stream stays open after its last samples have been heard: long
   enough that the gaps between a server's sounds don't each close and reopen
   the platform's device, short enough that the session stops reporting sound
   soon after it ends. The same as TigerVNC's, for the same reasons. */
#define IDLE_GRACE_MS 2000

/* How this device plays a format: as received, widened from 8 bits, or
   decoded by the platform's decoders, which the session's table reports
   (`VentoSound`). Windows picks AAC whenever it is offered, and it is offered
   wherever the platform decodes it, now on both platforms, since vento
   decodes it instead of the library's FFmpeg, which only the Linux client
   had. Other formats the channel decodes before the device sees them if the
   library can (without its FFmpeg, only PCM, so none of them), and otherwise
   does not offer back. This is the only place a format is decided. */
typedef enum {
    KIND_NONE = 0,
    KIND_S16,
    KIND_U8,
    KIND_AAC,
    KIND_OPUS,
} Kind;

static bool decoded(Kind kind) {
    return kind == KIND_AAC || kind == KIND_OPUS;
}

/* The table's limits, which are the platform's: rates from 8 to 384 kHz,
   one to eight channels. A server offers mono and stereo. */
static Kind kindOf(const VentoSound *sound, const AUDIO_FORMAT *f) {
    if (f == NULL || f->nSamplesPerSec < 8000 || f->nSamplesPerSec > 384000 || f->nChannels < 1
            || f->nChannels > 8) {
        return KIND_NONE;
    }
    if (f->wFormatTag == WAVE_FORMAT_PCM && f->wBitsPerSample == 16) {
        return KIND_S16;
    }
    if (f->wFormatTag == WAVE_FORMAT_PCM && f->wBitsPerSample == 8) {
        return KIND_U8;
    }
    if (f->wFormatTag == WAVE_FORMAT_AAC_MS && sound->aac) {
        return KIND_AAC;
    }
    /* Opus's identification header has room for two channels without a
       mapping table, which a server's format does not carry. */
    if (f->wFormatTag == WAVE_FORMAT_OPUS && sound->opus && f->nChannels <= 2) {
        return KIND_OPUS;
    }
    return KIND_NONE;
}

typedef struct {
    rdpsndDevicePlugin device; /* first: the channel holds a pointer to this */
    VentoSound *sound;

    /* The format the channel opened, and how it is played. */
    AUDIO_FORMAT format;
    Kind kind;
    bool opened;
    bool announced; /* the first wave since `Open` has been logged */
    bool failed;    /* the platform failed to open a stream in it */
    /* The decoder of a decoded kind, opened with the format, and whether a
       wave has failed to decode since (logged once). */
    vento_audio_decoder *decoder;
    bool undecodable;
    /* The stream's rate and channels, which for a decoded kind come from the
       decoder rather than the server's format (Opus is always 48 kHz). */
    int streamRate, streamChannels;
    /* The server's volume, as a gain, applied to every stream opened. */
    float gain;
    /* Whether this device's server listed a format it plays, so the device
       counts in the session's `offering` (`stopOffering`). */
    bool offering;

    /* The stream and its idle deadline, under `lock`, which the closing
       thread also takes. */
    pthread_mutex_t lock;
    pthread_cond_t cond;
    pthread_t closer;
    bool closerRunning;
    bool quit;
    vento_audio_stream *stream;
    uint64_t idleAt; /* monotonic ms after which the stream is closed */

    int16_t *wide; /* 8-bit samples widened, grown as needed */
    size_t wideLen;
} Device;

static uint64_t nowMs(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t) t.tv_sec * 1000u + (uint64_t) t.tv_nsec / 1000000u;
}

/* "PCM 44100 Hz 2 ch 16-bit", the tag without the library's prefix. */
static void describe(const AUDIO_FORMAT *f, char *out, size_t len) {
    const char *tag = audio_format_get_tag_string(f->wFormatTag);
    if (tag && strncmp(tag, "WAVE_FORMAT_", 12) == 0) {
        tag += 12;
    }
    snprintf(out, len, "%s %u Hz %u ch %u-bit", tag ? tag : "?", (unsigned) f->nSamplesPerSec,
             (unsigned) f->nChannels, (unsigned) f->wBitsPerSample);
}

/* ---- the stream -------------------------------------------------------- */

static void closeLocked(Device *d) {
    if (d->stream != NULL) {
        d->sound->table.close(d->stream);
        d->stream = NULL;
    }
}

static void closeDecoderLocked(Device *d) {
    if (d->decoder != NULL) {
        d->sound->table.decoder_close(d->decoder);
        d->decoder = NULL;
    }
}

/* Opens the stream in `rate` and `channels` unless it is already open in
   them: on the first wave since `Open`, after the stream was closed for being
   idle, or when a decoder's output changed. */
static bool openLocked(Device *d, int rate, int channels) {
    if (d->stream != NULL && d->streamRate == rate && d->streamChannels == channels) {
        return true;
    }
    closeLocked(d);
    if (!d->opened || d->failed || d->kind == KIND_NONE) {
        return false;
    }
    d->streamRate = rate;
    d->streamChannels = channels;
    d->stream = d->sound->table.open(d->sound->table.user, VENTO_AUDIO_S16, rate, channels);
    if (d->stream == NULL) {
        d->failed = true;
        LOGW("sound: the platform's output would not open a stream; the server's sound is dropped");
        return false;
    }
    if (d->sound->table.set_gain != NULL) {
        d->sound->table.set_gain(d->stream, d->gain);
    }
    pthread_cond_signal(&d->cond);
    return true;
}

/* Closes the stream once it has been idle past its deadline, which every
   write extends. A thread per device rather than a library timer: the channel
   calls the device only when a wave arrives, and this is for when no wave
   comes. */
static void *closer(void *arg) {
    Device *d = arg;
    pthread_mutex_lock(&d->lock);
    while (!d->quit) {
        if (d->stream == NULL) {
            pthread_cond_wait(&d->cond, &d->lock);
            continue;
        }
        const uint64_t now = nowMs();
        if (now >= d->idleAt) {
            closeLocked(d);
            continue;
        }
        struct timespec until;
        clock_gettime(CLOCK_MONOTONIC, &until);
        const uint64_t wait = d->idleAt - now;
        until.tv_sec += (time_t) (wait / 1000u);
        until.tv_nsec += (long) (wait % 1000u) * 1000000L;
        if (until.tv_nsec >= 1000000000L) {
            until.tv_sec += 1;
            until.tv_nsec -= 1000000000L;
        }
        pthread_cond_timedwait(&d->cond, &d->lock, &until);
    }
    pthread_mutex_unlock(&d->lock);
    return NULL;
}

/* ---- the device -------------------------------------------------------- */

/* This device's channel will send no more sound: it closed (its server
   closed it, or the session is ending), or its server lists nothing this end
   plays. The switch is withdrawn when no channel of the session has sound
   left to send, so not while the other rdpsnd channel (static or dynamic)
   still offers sound. */
static void stopOffering(Device *d) {
    if (!d->offering) {
        return;
    }
    d->offering = false;
    if (atomic_fetch_sub(&d->sound->offering, 1) == 1 && d->sound->table.withdraw != NULL) {
        d->sound->table.withdraw(d->sound->table.user);
    }
}

static BOOL formatSupported(rdpsndDevicePlugin *device, const AUDIO_FORMAT *format) {
    const Device *d = (const Device *) device;
    return kindOf(d->sound, format) != KIND_NONE;
}

/* Logs the server's formats as the channel receives them, with how each is
   handled: played here, decoded here first ("vento"), decoded by the library
   first, or not offered back. The first wave shows which one the server
   picked (`playEx`). */
static UINT serverFormatAnnounce(rdpsndDevicePlugin *device, const AUDIO_FORMAT *formats,
                                 size_t count) {
    Device *d = (Device *) device;
    char line[1536];
    size_t used = 0;
    bool playable = false;
    line[0] = '\0';
    for (size_t i = 0; i < count; i++) {
        playable = playable || kindOf(d->sound, &formats[i]) != KIND_NONE;
    }
    for (size_t i = 0; i < count && used + 64 < sizeof(line); i++) {
        char one[64];
        describe(&formats[i], one, sizeof(one));
        const Kind kind = kindOf(d->sound, &formats[i]);
        const char *how = decoded(kind)                                    ? " (vento)"
                          : kind != KIND_NONE                               ? ""
                          : freerdp_dsp_supports_format(&formats[i], FALSE) ? " (library)"
                                                                            : " (no)";
        const int n = snprintf(line + used, sizeof(line) - used, "%s%s%s", i ? ", " : "", one, how);
        if (n < 0) {
            break;
        }
        used += (size_t) n < sizeof(line) - used ? (size_t) n : sizeof(line) - used - 1;
    }
    /* FNV-1a over the line: the whole list the first time, a short line
       when it is the same as the last one logged. */
    uint64_t hash = 1469598103934665603ull;
    for (const char *c = line; *c != '\0'; c++) {
        hash = (hash ^ (uint8_t) *c) * 1099511628211ull;
    }
    if (atomic_exchange(&d->sound->formats, hash) == hash) {
        LOGI("sound: the server offers the same %zu formats again", count);
    } else {
        LOGI("sound: the server offers %zu formats: %s", count, line);
    }
    /* A server that lists a format this end plays may send sound from now on,
       and nothing in rdpsnd asks it to stop during a session (the client's
       formats are a single reply to this list), so the session's switch is a
       mute, offered already when the channel connected (`entry`) and kept
       here; a list with nothing this end plays takes it away. */
    if (playable && d->sound->table.offer != NULL) {
        if (!d->offering) {
            d->offering = true;
            atomic_fetch_add(&d->sound->offering, 1);
        }
        d->sound->table.offer(d->sound->table.user, VENTO_AUDIO_SWITCH_MUTE, true, NULL, NULL);
    } else {
        stopOffering(d);
    }
    return CHANNEL_RC_OK;
}

/* The configuration a decoded kind's decoder is opened with: an Opus
   identification header when the server sent one as the format's extra
   bytes, and otherwise nothing, so the format's fields describe AAC LC or
   plain Opus. That matches Windows' AAC (raw frames, no extra bytes) and
   what the library's own decoding assumed of it too. */
static void decoderConfig(const AUDIO_FORMAT *f, const uint8_t **config, size_t *len) {
    *config = NULL;
    *len = 0;
    if (f->wFormatTag == WAVE_FORMAT_OPUS && f->data != NULL && f->cbSize >= 19
            && memcmp(f->data, "OpusHead", 8) == 0) {
        *config = f->data;
        *len = f->cbSize;
    }
}

static BOOL open_(rdpsndDevicePlugin *device, const AUDIO_FORMAT *format, UINT32 latency) {
    Device *d = (Device *) device;
    (void) latency;
    const Kind kind = kindOf(d->sound, format);
    if (kind == KIND_NONE) {
        char one[64];
        describe(format, one, sizeof(one));
        LOGW("sound: asked to open %s, which this end does not play", one);
        return FALSE;
    }
    pthread_mutex_lock(&d->lock);
    closeLocked(d);
    closeDecoderLocked(d);
    d->format = *format;
    d->format.data = NULL;
    d->format.cbSize = 0;
    d->kind = kind;
    d->opened = true;
    d->announced = false;
    d->failed = false;
    d->undecodable = false;
    int rate = (int) format->nSamplesPerSec;
    if (decoded(kind)) {
        const uint8_t *config;
        size_t len;
        decoderConfig(format, &config, &len);
        d->decoder = d->sound->table.decoder_open(
                d->sound->table.user, kind == KIND_AAC ? VENTO_AUDIO_CODEC_AAC : VENTO_AUDIO_CODEC_OPUS,
                rate, (int) format->nChannels, config, len);
        if (d->decoder == NULL) {
            d->failed = true;
            LOGW("sound: the platform would not decode what the server sends; its sound is dropped");
        }
        /* Opus decodes at 48 kHz whatever the server's format says. */
        if (kind == KIND_OPUS) {
            rate = 48000;
        }
    }
    /* Opened now rather than at the first wave, which follows immediately,
       in the format the decoder will most likely produce, and opened again at
       the first wave if it produces something else. If the platform fails to
       open a stream, that is logged once and the waves are dropped, but the
       device still reports that it opened: the channel treats a failed open
       as an error and ends itself, and the session with it. */
    openLocked(d, rate, (int) format->nChannels);
    d->idleAt = nowMs() + IDLE_GRACE_MS;
    pthread_mutex_unlock(&d->lock);
    return TRUE;
}

/* The server's volume: the left channel in the low word, the right in the
   high one, each out of 0xFFFF. The stream gets one gain, the louder of the
   two, since the table has no balance: a server that pans is heard at the
   level of its louder side rather than not at all. */
static BOOL setVolume(rdpsndDevicePlugin *device, UINT32 value) {
    Device *d = (Device *) device;
    const UINT32 left = value & 0xFFFFu;
    const UINT32 right = value >> 16;
    const float gain = (float) (left > right ? left : right) / 65535.0f;
    pthread_mutex_lock(&d->lock);
    if (gain != d->gain) {
        LOGI("sound: the server sets its volume to %.0f%%", (double) gain * 100.0);
    }
    d->gain = gain;
    if (d->stream != NULL && d->sound->table.set_gain != NULL) {
        d->sound->table.set_gain(d->stream, gain);
    }
    pthread_mutex_unlock(&d->lock);
    return TRUE;
}

/* The volume the client reports to the server in the formats it confirms:
   the server's own, as last set, and full initially, since a server given
   zero here would start the session muted. */
static UINT32 getVolume(rdpsndDevicePlugin *device) {
    Device *d = (Device *) device;
    pthread_mutex_lock(&d->lock);
    const UINT32 level = (UINT32) (d->gain * 65535.0f + 0.5f);
    pthread_mutex_unlock(&d->lock);
    return level | (level << 16);
}

static const int16_t *widen(Device *d, const BYTE *data, size_t size) {
    if (d->wideLen < size) {
        int16_t *grown = realloc(d->wide, size * sizeof(int16_t));
        if (grown == NULL) {
            return NULL;
        }
        d->wide = grown;
        d->wideLen = size;
    }
    for (size_t i = 0; i < size; i++) {
        d->wide[i] = (int16_t) (((int) data[i] - 128) * 256);
    }
    return d->wide;
}

/* Decodes one wave of a decoded kind through the platform's decoder: its
   samples, in the format the decoder reports, or NULL if it produced none. */
static const int16_t *decodeLocked(Device *d, const BYTE *data, size_t size, size_t *bytes,
                                   int *rate, int *channels) {
    *bytes = 0;
    if (d->decoder == NULL) {
        return NULL;
    }
    const int16_t *samples = NULL;
    const int frames = d->sound->table.decode(d->decoder, data, size, &samples, rate, channels);
    if (frames < 0 && !d->undecodable) {
        LOGW("sound: a wave of %zu bytes would not decode; it is dropped, as is any other", size);
        d->undecodable = true;
    }
    if (frames <= 0 || samples == NULL || *channels <= 0) {
        return NULL;
    }
    *bytes = (size_t) frames * (size_t) *channels * sizeof(int16_t);
    return samples;
}

/* A wave: as the server sent it when this device plays or decodes its
   format, and otherwise already decoded by the channel (`format` is then the
   server's, the samples 16-bit PCM in the opened stream's format). Returns
   the milliseconds until these samples are heard, which the channel adds to
   the wave's confirm. */
static UINT playEx(rdpsndDevicePlugin *device, const AUDIO_FORMAT *format, const BYTE *data,
                   size_t size) {
    Device *d = (Device *) device;
    const Kind kind = kindOf(d->sound, format);
    if (!d->announced && format != NULL) {
        char one[64];
        describe(format, one, sizeof(one));
        LOGI("sound: the server sends %s%s", one,
             decoded(kind)        ? ", which the platform decodes"
             : kind == KIND_NONE ? ", which the library decodes"
                                 : "");
        d->announced = true;
    }
    pthread_mutex_lock(&d->lock);
    int latency = 0;
    const void *samples = data;
    size_t bytes = size;
    int rate = (int) d->format.nSamplesPerSec;
    int channels = (int) d->format.nChannels;
    if (decoded(d->kind) && kind == d->kind) {
        samples = decodeLocked(d, data, size, &bytes, &rate, &channels);
    } else if (d->kind == KIND_U8 && kind == KIND_U8) {
        samples = widen(d, data, size);
        bytes = size * sizeof(int16_t);
    }
    /* A decoder that produced nothing for this wave (yet) leaves the stream
       as it is, open or closed. */
    if (samples != NULL && bytes > 0 && openLocked(d, rate, channels)) {
        d->sound->table.write(d->stream, samples, bytes);
    }
    if (d->stream != NULL) {
        latency = d->sound->table.latency != NULL ? d->sound->table.latency(d->stream) : 0;
        d->idleAt = nowMs() + (uint64_t) (latency > 0 ? latency : 0) + IDLE_GRACE_MS;
    }
    pthread_mutex_unlock(&d->lock);
    return (UINT) (latency > 0 ? latency : 0);
}

static void close_(rdpsndDevicePlugin *device) {
    Device *d = (Device *) device;
    pthread_mutex_lock(&d->lock);
    closeLocked(d);
    closeDecoderLocked(d);
    d->opened = false;
    pthread_mutex_unlock(&d->lock);
}

static void free_(rdpsndDevicePlugin *device) {
    Device *d = (Device *) device;
    if (d == NULL) {
        return;
    }
    stopOffering(d);
    pthread_mutex_lock(&d->lock);
    d->quit = true;
    closeLocked(d);
    closeDecoderLocked(d);
    pthread_cond_signal(&d->cond);
    pthread_mutex_unlock(&d->lock);
    if (d->closerRunning) {
        pthread_join(d->closer, NULL);
    }
    pthread_mutex_destroy(&d->lock);
    pthread_cond_destroy(&d->cond);
    free(d->wide);
    free(d);
}

/* The channel's entry point for a device, called when the channel connects
   (static) or opens (dynamic), each creating its own device. */
static UINT VCAPITYPE entry(PFREERDP_RDPSND_DEVICE_ENTRY_POINTS points) {
    rdpContext *context = freerdp_rdpsnd_get_context(points->rdpsnd);
    VentoSound *sound = context != NULL ? vento_sound_of(context) : NULL;
    if (sound == NULL || !sound->available) {
        /* The session names this device only when it has an output. */
        return CHANNEL_RC_INITIALIZATION_ERROR;
    }
    Device *d = calloc(1, sizeof(Device));
    if (d == NULL) {
        return CHANNEL_RC_NO_MEMORY;
    }
    d->sound = sound;
    d->gain = 1.0f;
    pthread_mutex_init(&d->lock, NULL);
    pthread_condattr_t attr;
    pthread_condattr_init(&attr);
    pthread_condattr_setclock(&attr, CLOCK_MONOTONIC);
    pthread_cond_init(&d->cond, &attr);
    pthread_condattr_destroy(&attr);
    if (pthread_create(&d->closer, NULL, closer, d) != 0) {
        free_(&d->device);
        return CHANNEL_RC_NO_MEMORY;
    }
    d->closerRunning = true;

    d->device.FormatSupported = formatSupported;
    d->device.ServerFormatAnnounce = serverFormatAnnounce;
    d->device.Open = open_;
    d->device.GetVolume = getVolume;
    d->device.SetVolume = setVolume;
    d->device.PlayEx = playEx;
    d->device.Close = close_;
    d->device.Free = free_;
    points->pRegisterRdpsndDevice(points->rdpsnd, &d->device);
    /* The channel is connected: the static one once the server has joined
       it, which is during the connection, and a dynamic one once the server
       has opened it. Either is the server saying it may send sound, so the
       session's switch is offered from here. Windows lists its formats only
       when it starts a sound, on a dynamic channel it opens for it, and the
       switch came with the first sound when it waited for that list. A list
       with nothing this end plays takes it away again
       (`serverFormatAnnounce`). */
    if (sound->table.offer != NULL) {
        d->offering = true;
        atomic_fetch_add(&sound->offering, 1);
        sound->table.offer(sound->table.user, VENTO_AUDIO_SWITCH_MUTE, true, NULL, NULL);
    }
    return CHANNEL_RC_OK;
}

/* ---- registration ----------------------------------------------------- */

/* Every addin lookup goes through here once registered: rdpsnd's device
   "vento" is this module's, and everything else (every channel and every
   other device) comes from the library's static table, as for any client of
   the library. */
static PVIRTUALCHANNELENTRY provider(LPCSTR name, LPCSTR subsystem, LPCSTR type, DWORD flags) {
    if (name != NULL && subsystem != NULL && strcmp(name, RDPSND_CHANNEL_NAME) == 0
            && strcmp(subsystem, VENTO_RDPSND_SUBSYSTEM) == 0) {
        PFREERDP_RDPSND_DEVICE_ENTRY device = entry;
        return WINPR_FUNC_PTR_CAST(device, PVIRTUALCHANNELENTRY);
    }
    return freerdp_channels_load_static_addin_entry(name, subsystem, type, flags);
}

bool vento_sound_register(void) {
    /* Process-wide, as the library's provider is: every session registers
       the same function, so a second session's registration undoes nothing
       of the first's. It can't cover the moment between a context being
       created, which restores the library's provider, and this call: another
       session's channel that looks up its device in that moment doesn't find
       it, and that session plays nothing. */
    return freerdp_register_addin_provider(provider, 0) == 0;
}

bool vento_sound_init(VentoSound *v, const vento_audio *table) {
    memset(v, 0, sizeof(*v));
    if (table == NULL) {
        return false;
    }
    if (table->open == NULL || table->write == NULL || table->close == NULL) {
        if (table->release != NULL) {
            table->release(table->user);
        }
        return false;
    }
    v->table = *table;
    v->available = true;
    atomic_init(&v->offering, 0);
    atomic_init(&v->formats, 0);
    /* Queried once: this decides what the server is offered for the whole
       session. */
    if (table->decodable != NULL && table->decoder_open != NULL && table->decode != NULL
            && table->decoder_close != NULL) {
        v->aac = table->decodable(table->user, VENTO_AUDIO_CODEC_AAC);
        v->opus = table->decodable(table->user, VENTO_AUDIO_CODEC_OPUS);
    }
    return true;
}

void vento_sound_release(VentoSound *v) {
    if (v->available && v->table.release != NULL) {
        v->table.release(v->table.user);
    }
    v->available = false;
}
