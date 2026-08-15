// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: MIT

package net.pgaskin.remotedesktop.control.input;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.List;

public class ConfigTest {

    /**
     * {@link Config#copyFrom} is a hand-written list of assignments, and it has
     * to be: the input stack holds one {@code Config} by reference so that
     * flipping the {@code PRESET} square swaps every setting live, without
     * rebuilding anything. The failure mode is adding a tunable and forgetting
     * the copy — silently, since the new setting would simply never follow a
     * preset change. So walk the fields reflectively and check every one moves.
     */
    @Test
    public void copyFromCopiesEveryField() throws IllegalAccessException {
        final Config src = Config.faithful(2.625f);
        final Config dst = Config.improved(2.625f);

        // Give every field in the source a value the destination cannot already
        // have, so "was copied" cannot be confused with "happened to match".
        final List<Field> fields = mutableFields();
        assertTrue("Config has no fields — has it been rewritten?", fields.size() > 20);
        for (Field f : fields) {
            f.setAccessible(true);
            setDistinctively(f, src, dst);
        }

        dst.copyFrom(src);

        final List<String> missed = new ArrayList<>();
        for (Field f : fields) {
            if (!f.get(src).equals(f.get(dst))) {
                missed.add(f.getName() + ": " + f.get(src) + " != " + f.get(dst));
            }
        }
        assertEquals("Config.copyFrom does not copy " + missed, 0, missed.size());
    }

    /** Every instance field; {@code density} is final and set at construction. */
    private static List<Field> mutableFields() {
        final List<Field> out = new ArrayList<>();
        for (Field f : Config.class.getDeclaredFields()) {
            final int m = f.getModifiers();
            if (!Modifier.isStatic(m) && !Modifier.isFinal(m) && !f.isSynthetic()) {
                out.add(f);
            }
        }
        return out;
    }

    private static void setDistinctively(Field f, Config src, Config dst)
            throws IllegalAccessException {
        final Class<?> t = f.getType();
        if (t == boolean.class) {
            f.setBoolean(src, !f.getBoolean(dst));
        } else if (t == int.class) {
            f.setInt(src, f.getInt(dst) + 7);
        } else if (t == long.class) {
            f.setLong(src, f.getLong(dst) + 7);
        } else if (t == float.class) {
            f.setFloat(src, f.getFloat(dst) + 7);
        } else if (t == double.class) {
            f.setDouble(src, f.getDouble(dst) + 7);
        } else {
            throw new AssertionError("Config." + f.getName() + " is a " + t
                    + "; teach this test how to vary it");
        }
    }
}
