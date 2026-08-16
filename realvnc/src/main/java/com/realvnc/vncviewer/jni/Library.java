// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

import android.os.Build;

import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/**
 * Which build of {@code libvncviewer.so} the declarations in this package were
 * read from, and how to know a copy is that build.
 *
 * <p>It is here rather than wherever a copy happens to be fetched because it is
 * a fact about <i>these declarations</i>: every native's symbol, every
 * callback's name and every descriptor came out of one build, and
 * {@code check-jni-abi.sh} exists to say they still match it. A hash of that
 * build is the same statement in another form. Nothing here says where a copy
 * can be got — that belongs to whoever acquires one.
 *
 * <p>The library is not in this repository and is in no artefact it produces.
 * Whoever loads it verifies it against this, because a hash of somebody else's
 * binary is the whole of what makes loading it safe.
 */
public final class Library {

    /** The viewer release these declarations were read from. */
    public static final String VERSION = "4.9.3.60175";

    /**
     * In load order: {@code libvncviewer.so}'s {@code DT_NEEDED} on the first is
     * satisfied by soname out of the namespace it is already in, so the two go
     * in this order and no search path is needed for either.
     */
    private static final List<String> NAMES = List.of("libc++_shared.so", "libvncviewer.so");

    /**
     * SHA-256 by {@code <abi>/<name>}, which is also the path each lives at
     * inside the viewer's APK. arm64 is every device that can run minSdk 34 and
     * x86_64 is the emulator.
     */
    private static final Map<String, String> SHA256 = Map.of(
            "arm64-v8a/libvncviewer.so",
            "fdd67f78312bfbe4fb7861eaa42cfe5d6091668c057157d66e94aa33e41fe8ba",
            "arm64-v8a/libc++_shared.so",
            "4397241b4bd20a8e579bfb41d21107857e12985f6a01ca0c2a5f83380d1270b4",
            "x86_64/libvncviewer.so",
            "ee3acfbe81cdd8f9dbd9542e2a0c9bd31634d78334c5e22c27414a8b73ac364a",
            "x86_64/libc++_shared.so",
            "db9609240e60ca18b816a5e95a6bbc784e799261efbd5b30c8e2e03b540b3fa8");

    private Library() {
    }

    /** The libraries to load, in the order they must be loaded. */
    public static List<String> names() {
        return NAMES;
    }

    /** The ABIs pinned here, which is what a device must be one of. */
    public static Set<String> abis() {
        return Set.of("arm64-v8a", "x86_64");
    }

    /** Null for an ABI or a name this pin says nothing about. */
    public static String sha256(String abi, String name) {
        return SHA256.get(abi + "/" + name);
    }

    /**
     * This device's ABI, of the ones pinned here, and null for a device that is
     * none of them. {@code SUPPORTED_ABIS} is in preference order, so a 64-bit
     * device gets its 64-bit answer.
     */
    public static String abi() {
        for (String abi : Build.SUPPORTED_ABIS) {
            if (abis().contains(abi)) {
                return abi;
            }
        }
        return null;
    }

    /**
     * What this device needs: each library's name to its SHA-256, in the order
     * they must be loaded. One statement, used by whoever acquires a copy and
     * again by whoever loads one.
     *
     * <p><b>Empty means this device can never have them</b>, which is a
     * different answer from "has not got them yet" and has to be read as one:
     * an empty set of requirements is satisfied by an empty store.
     */
    public static Map<String, String> wanted() {
        final String abi = abi();
        if (abi == null) {
            return Map.of();
        }
        final Map<String, String> wanted = new LinkedHashMap<>();
        for (String name : NAMES) {
            wanted.put(name, sha256(abi, name));
        }
        return wanted;
    }
}
