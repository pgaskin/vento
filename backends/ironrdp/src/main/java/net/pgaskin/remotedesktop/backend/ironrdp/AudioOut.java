// SPDX-FileCopyrightText: 2026 Nick Haghiri
// SPDX-License-Identifier: GPL-3.0-or-later

package net.pgaskin.remotedesktop.backend.ironrdp;

import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.util.Log;

import java.util.concurrent.LinkedBlockingQueue;

/**
 * Plays the remote machine's sound on this phone.
 *
 * <p>Blocks arrive on the protocol thread and are written to an
 * {@link AudioTrack} on a thread of their own, so the protocol thread never
 * waits on the audio hardware. The queue is bounded: when it is full the new
 * block is dropped, since a late sound is worse than a gap in one.
 *
 * <p>Only 16-bit stereo is played, which is the only PCM the client advertises.
 */
final class AudioOut {

    private static final String TAG = "IronRdpAudio";
    private static final int QUEUE_BLOCKS = 32;

    private record Block(int sampleRate, byte[] pcm) {
    }

    private final LinkedBlockingQueue<Block> queue = new LinkedBlockingQueue<>(QUEUE_BLOCKS);
    private final Thread thread;
    private volatile boolean closed;

    AudioOut() {
        thread = new Thread(this::run, "rdp-audio");
        thread.setDaemon(true);
        thread.start();
    }

    /** Any thread. Drops the block rather than wait when the queue is full. */
    void write(int sampleRate, int channels, byte[] pcm) {
        if (closed) {
            return;
        }
        if (channels != 2) {
            Log.w(TAG, "dropping a block with " + channels + " channels");
            return;
        }
        if (!queue.offer(new Block(sampleRate, pcm))) {
            Log.d(TAG, "queue full, dropping a block");
        }
    }

    /** Any thread. The track is released on the playback thread once it has stopped. */
    void close() {
        closed = true;
        thread.interrupt();
    }

    private void run() {
        AudioTrack track = null;
        int trackRate = 0;
        try {
            while (!closed) {
                final Block block;
                try {
                    block = queue.take();
                } catch (InterruptedException e) {
                    break;
                }
                if (track == null || trackRate != block.sampleRate()) {
                    if (track != null) {
                        track.release();
                    }
                    track = create(block.sampleRate());
                    trackRate = block.sampleRate();
                }
                // Blocking: the track's own buffer is the only one needed, and it
                // paces the thread to the hardware.
                track.write(block.pcm(), 0, block.pcm().length);
            }
        } catch (Throwable t) {
            Log.e(TAG, "playback stopped", t);
        } finally {
            if (track != null) {
                track.stop();
                track.release();
            }
        }
    }

    private static AudioTrack create(int sampleRate) {
        final int mask = AudioFormat.CHANNEL_OUT_STEREO;
        final int minimum = AudioTrack.getMinBufferSize(sampleRate, mask,
                AudioFormat.ENCODING_PCM_16BIT);
        // About a fifth of a second: enough to ride out a late block, not so
        // much that a sound lags the picture by eye.
        final int buffer = Math.max(minimum, sampleRate * 4 / 5);
        final AudioTrack track = new AudioTrack.Builder()
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                        .build())
                .setAudioFormat(new AudioFormat.Builder()
                        .setSampleRate(sampleRate)
                        .setChannelMask(mask)
                        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                        .build())
                .setBufferSizeInBytes(buffer)
                .setTransferMode(AudioTrack.MODE_STREAM)
                .build();
        track.play();
        return track;
    }
}
