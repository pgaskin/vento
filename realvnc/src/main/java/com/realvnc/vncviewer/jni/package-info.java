// SPDX-License-Identifier: CC0-1.0

/**
 * The Java side of the JNI surface of RealVNC Viewer's {@code libvncviewer.so}.
 *
 * <p>Two things in here are linkage rather than design, and neither can be
 * changed without unbinding the library: the <b>native methods</b>, which bind
 * by mangled name, so the package, the class name, the method name and the shape
 * of the argument list are all part of the address; and the <b>callbacks</b>,
 * which the core resolves by class name and caches a method id per member, so an
 * interface's name, its nesting, its method names and their descriptors are
 * equally fixed — as are the constructor signatures of the classes it builds.
 *
 * <p>Everything else is ours, and each class says at the top what requires the
 * name it carries. {@code check-jni-abi.sh} is the last word on all of it,
 * because it asks the binary rather than a decompiler.
 *
 * <p>This is the subset one client needs rather than a reproduction of the
 * surface; the module's README lists what is left out and why, and says why the
 * library itself is not here.
 */
package com.realvnc.vncviewer.jni;
