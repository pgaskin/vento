// SPDX-License-Identifier: CC0-1.0

package com.realvnc.vncviewer.jni;

/**
 * The core's global parameter registry — every parameter it has, including the
 * ones no viewer UI exposes.
 *
 * <p>Required by {@code Java_com_realvnc_vncviewer_jni_ConfigurationBindings_set}.
 * The registry's getters are declared by the library too and are not here: a
 * parameter this side sets is a parameter this side already knows.
 */
public final class ConfigurationBindings {

    private ConfigurationBindings() {
    }

    /**
     * @param level   a priority; the viewer applies its own expert options at 10
     * @param persist whether to write the value to the config layer's data
     *                directory as well as apply it
     */
    public static native void set(String name, String value, int level, boolean persist);
}
