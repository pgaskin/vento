// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// FreeRDP's sound (the rdpsnd channel) played on vento's output
// (vento/audio.h) instead of one of the library's devices; shared by the
// session and the module's rdpsnd device (rdpsnd_vento.c).
//
// The channel's protocol half is the library's: the formats offered and
// confirmed, the wave PDUs and their confirms, the volume. It leaves the sound
// itself to a device, which it picks by name from the channel's arguments
// ("sys:<name>"), looked up through the addin provider every client of the
// library registers. That is public API, so the module adds its device
// without a patch: the session registers a provider that handles "vento"
// itself and passes everything else to the library's own, and names "vento"
// in the arguments of both rdpsnd channels, static and dynamic.
//
// A device is created with only the channel, which knows the library's
// context but not the session; the session maps a context to its sound
// (vento_sound_of), as it does for the H.264 contexts.

#ifndef VENTO_FREERDP_RDPSND_VENTO_H
#define VENTO_FREERDP_RDPSND_VENTO_H

#include <stdatomic.h>
#include <stdbool.h>
#include <stdint.h>

#include <freerdp/freerdp.h>

#include <vento/audio.h>

/* The device name the session puts in the channel's arguments. */
#define VENTO_RDPSND_SUBSYSTEM "vento"

/* The library's channel drops a wave by itself when it decides the device is
   too far behind, by wall-clock time since the stream began: more than two
   waves plus the configured latency, which is zero unless the arguments set
   it. The table's queue already buffers what the server's timing needs and
   drops the oldest audio past a quarter of a second (vento/audio.h), so the
   channel is given that quarter second: its own check then never fires
   before the queue's, and a burst arriving all at once after a network stall
   is trimmed by the queue instead of losing a wave in the channel. */
#define VENTO_RDPSND_LATENCY_MS 250

/* One session's sound: the platform's output, on which every rdpsnd device
   created by the session's channels opens its streams. */
typedef struct VentoSound {
    vento_audio table;
    bool available;
    /* The codecs the platform's decoders handle, which the server is
       offered. */
    bool aac, opus;
    /* The number of devices whose channel is connected and whose server
       has not listed only formats this end can't play; the session's sound
       switch is offered while there is one. */
    atomic_int offering;
    /* A hash of the last list of formats logged in full: a server that
       lists the same ones again (Windows, at every sound it starts) gets a
       short line. */
    _Atomic uint64_t formats;
} VentoSound;

/* Internal to the library, not part of the backend's interface: only
   vento/freerdp.h's symbols are exported. */
#pragma GCC visibility push(hidden)

/* Takes ownership of `table` whatever it returns; false, with the table
   released, if it is NULL or lacks a required call. */
bool vento_sound_init(VentoSound *v, const vento_audio *table);

/* Called after the last device is freed, which is after the channels are. */
void vento_sound_release(VentoSound *v);

/* Registers the module's device where the channel looks for one. The
   library restores its own provider whenever a client context is created, so
   this is called after every freerdp_client_context_new. */
bool vento_sound_register(void);

/* Defined by the session: the sound of the session that owns `context`. */
VentoSound *vento_sound_of(rdpContext *context);

#pragma GCC visibility pop

#endif
