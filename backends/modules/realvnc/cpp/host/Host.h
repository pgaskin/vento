// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Shared by the files in this directory: the logging macros, the Java
// stand-ins every translation unit has to agree on, and the way back from a
// `jobject` to the C++ object behind it. Internal to the hosting layer; the
// session sees only `HostedCore.h`, one directory up.
//
// How the hosting layer works: RealVNC's `libvncviewer.so` is a bionic
// library that talks to Android only through JNI, `liblog`, `libandroid`'s
// looper and `libjnigraphics`. mcpelauncher-linker loads it into a glibc
// process with libc-shim standing in for bionic's libc, and libjnivm stands
// in for the JVM: every Java class the core resolves by name is a C++ struct
// registered under that name, with the methods the core calls hooked one by
// one. The set of classes is what the core used on its way to a connected
// session, found by running it and reading the trace (the Meson option
// `realvnc_trace` turns that trace on); the Android side of the
// backend never sees any of this, since the real classes exist there.

#ifndef VENTO_REALVNC_HOST_H
#define VENTO_REALVNC_HOST_H

#include <stdint.h>

#include <memory>
#include <string>
#include <vector>

#include <jnivm.h>

#include <vento/log.h>

#define TAG "RealVnc"
#define LOGI(...) vento_log(VENTO_LOG_INFO, TAG, __VA_ARGS__)
#define LOGW(...) vento_log(VENTO_LOG_WARN, TAG, __VA_ARGS__)

namespace vento_realvnc {

/* `android.content.Context`, which the core keeps a reference to and asks for
   resources, assets, a content resolver and itself. One instance, made at
   init. Here rather than in the file that hooks it because jnivm keys a
   hook's derived JNI signature on the C++ type, so every file that names
   this class in a parameter must name the same type. */
struct Context final : jnivm::Object {};

/* `SessionBindings$Session`: the opaque handle, whose `token` is the native
   `AndroidSession*` the core reads back on every call. Same rule. */
struct SessionHandle final : jnivm::Object {
    jlong token = 0;
};

/* `android.graphics.Bitmap`, `ARGB_8888` only, which in memory is `R, G, B,
   A`, the order `HostedCore.h` uses. The core makes one per cursor through
   `createBitmap` and draws into it through `libjnigraphics`. */
struct Bitmap final : jnivm::Object {
    jint width = 0, height = 0;
    std::vector<uint32_t> pixels;
};

/* jnivm hands out raw `Object*` as `jobject`, and a global or weak reference
   is a wrapper around the real object; this follows the wrappers down to the
   object and returns it if it is a `T`, or nullptr. */
template <class T>
T *unwrap(jobject o) {
    auto *obj = (jnivm::Object *) o;
    while (obj) {
        if (auto *g = dynamic_cast<jnivm::Global *>(obj)) {
            obj = g->wrapped.get();
            continue;
        }
        if (auto *w = dynamic_cast<jnivm::Weak *>(obj)) {
            obj = w->wrapped.lock().get();
            continue;
        }
        break;
    }
    return dynamic_cast<T *>(obj);
}

/* A jnivm object as the `jobject` a native takes. */
template <class T>
jobject toJni(jnivm::ENV *env, const std::shared_ptr<T> &p) {
    return jnivm::JNITypes<std::shared_ptr<T>>::ToJNIType(env, p);
}

inline std::shared_ptr<jnivm::String> jstr(const std::string &s) {
    return std::make_shared<jnivm::String>(s);
}

inline std::string fromJstr(const std::shared_ptr<jnivm::String> &s) {
    return s ? std::string(*s) : std::string();
}

/* The stand-ins, each file's own: `Classes.cxx` has the Android and
   `java.*` classes, `Files.cxx` has `java.io` and the encrypted-file API.
   Both are called once, before `JNI_OnLoad`. */
void registerClasses(jnivm::VM &vm, const std::shared_ptr<Context> &context);
void registerFiles(jnivm::VM &vm);

} // namespace vento_realvnc

#endif
