// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `RealVncNet`'s natives: the hooks in `realvnc_net.c` put under the RealVNC
// core on the phone, where the system's linker has loaded it and there is
// no symbol map to put them in, as there is on Linux. The core is left as
// it is on disk (its pinned hash still holds); its GOT is written in
// memory instead, once, the first time a session needs it, on the core's
// own thread and before that session's `createSession`, so before the core
// opens a socket for it.
//
// The core is BIND_NOW with full RELRO, and every one of its network
// imports is reached through a JUMP_SLOT relocation, one per symbol (read
// off the pinned build). Before anything is written that is checked again
// on the loaded core: one copy loaded; plain RELA relocations; each of the
// imports in `CHECKED` exactly one JUMP_SLOT, referred to by no other
// relocation (a GLOB_DAT or ABS64 would be a call the GOT slot does not
// catch), and its slot holding libc's own function, which also proves it
// is resolved. Symbol numbers are bounded by the symbol count, which the
// GNU hash table (or the SysV one) gives, since DT_SYMTAB has no size of
// its own. Any mismatch writes nothing. Of those imports, the ones in
// `realvnc_net_hooks` are replaced: every page holding one of their slots
// is made writable first (where one cannot be, nothing is written), then
// every slot is written, the pages in RELRO are put back read-only, and
// the slots are read back.
//
// The patch is tried once per process, by the first session whose route is
// not direct; a session on a direct route never touches the core. Where it
// fails, that session and every later one on a route that is not direct
// refuses to start, with the reason (`nativeAttach` throws): the core would
// otherwise dial the server directly, around the route. It leaves either
// every hook in place or none: a slot that does not read back as written
// has every slot put back as it was.
//
// Debug builds (NDEBUG not defined) read the system property
// `debug.vento.realvnc.unhooked`: "1" makes the patch fail without writing
// anything, to test that refusal (`adb shell setprop`, before the first
// session of the process).

#define _GNU_SOURCE // dl_iterate_phdr

#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <jni.h>
#include <link.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/system_properties.h>
#include <unistd.h>

#include <vento/log.h>
#include <vento/net.h>

#include "realvnc_net.h"

#define TAG "RealVnc"

/* `src/android.rs` in this module's crate, linked in as a static library. */
bool vento_realvnc_network(bool force_socketpair, vento_net *out);
bool vento_realvnc_tunnel(bool force_socketpair, char *out, size_t len);

/* The core's imports that touch sockets or names. The hooks' imports are
   among them; the rest are checked so that a build whose imports are not
   what was read off the pinned one is not patched at all. */
static const char *const CHECKED[] = {
        "socket",      "socketpair",  "bind",         "connect",      "getsockname", "getpeername",
        "getsockopt",  "setsockopt",  "sendmsg",      "recvmsg",      "shutdown",    "getaddrinfo",
        "freeaddrinfo", "getnameinfo", "gai_strerror", "if_indextoname", "gethostname", "read",
        "__read_chk",
};
#define CHECKED_COUNT (sizeof CHECKED / sizeof CHECKED[0])

#if defined(__aarch64__)
#define R_JUMP_SLOT R_AARCH64_JUMP_SLOT
#define R_GLOB_DAT R_AARCH64_GLOB_DAT
#define R_ABS64 R_AARCH64_ABS64
#elif defined(__x86_64__)
#define R_JUMP_SLOT R_X86_64_JUMP_SLOT
#define R_GLOB_DAT R_X86_64_GLOB_DAT
#define R_ABS64 R_X86_64_64
#else
#error "the core is built for arm64-v8a and x86_64 only"
#endif

/* Android's packed relocations, which bionic accepts and the pinned build
   does not use. */
#define DT_ANDROID_REL_TAG 0x6000000f
#define DT_ANDROID_RELA_TAG 0x60000011

struct core {
    int count;
    ElfW(Addr) base;
    const ElfW(Phdr) *phdr;
    ElfW(Half) phnum;
};

static int find_core(struct dl_phdr_info *info, size_t size, void *data) {
    (void) size;
    struct core *c = data;
    const char *name = info->dlpi_name;
    const size_t n = name ? strlen(name) : 0;
    static const char want[] = "/libvncviewer.so";
    if (n >= sizeof want - 1 && strcmp(name + n - (sizeof want - 1), want) == 0) {
        c->count++;
        c->base = info->dlpi_addr;
        c->phdr = info->dlpi_phdr;
        c->phnum = info->dlpi_phnum;
    }
    return 0;
}

/* How many entries the dynamic symbol table has, from the GNU hash table
   (`gnu`) or the SysV one (`sysv`), either of which may be 0; 0 where
   neither says. */
static size_t symbol_count(ElfW(Addr) gnu, ElfW(Addr) sysv) {
    if (gnu) {
        const uint32_t *h = (const uint32_t *) gnu;
        const uint32_t buckets = h[0], offset = h[1], bloom = h[2];
        const uint32_t *bucket = (const uint32_t *) ((const ElfW(Addr) *) (h + 4) + bloom);
        const uint32_t *chain = bucket + buckets;
        uint32_t last = 0;
        for (uint32_t i = 0; i < buckets; i++) {
            last = bucket[i] > last ? bucket[i] : last;
        }
        if (last < offset) {
            return offset;
        }
        // The last bucket's chain ends with the table's last symbol: the
        // entry with the low bit set.
        while (!(chain[last - offset] & 1)) {
            last++;
        }
        return (size_t) last + 1;
    }
    if (sysv) {
        return ((const uint32_t *) sysv)[1]; // nchain: one per symbol
    }
    return 0;
}

/* Gives the pages holding the first `count` slots at `targets` `prot`, or,
   with `relro_only`, only those in [`relro`, `relro_end`); false, with
   `*failed` the slot's index, where one cannot be. */
static bool protect(void **const *targets, size_t count, int prot, bool relro_only, ElfW(Addr) relro,
                    ElfW(Addr) relro_end, size_t *failed) {
    const uintptr_t page = (uintptr_t) sysconf(_SC_PAGESIZE);
    for (size_t h = 0; h < count; h++) {
        const ElfW(Addr) at = (ElfW(Addr)) targets[h];
        if (relro_only && (at < relro || at >= relro_end)) {
            continue;
        }
        if (mprotect((void *) ((uintptr_t) targets[h] & ~(page - 1)), page, prot) != 0) {
            *failed = h;
            return false;
        }
    }
    return true;
}

/* Writes the hooks into the core's GOT, or says why it did not. */
static bool patch(char *why, size_t why_len) {
#ifndef NDEBUG
    char test[PROP_VALUE_MAX] = "";
    if (__system_property_get("debug.vento.realvnc.unhooked", test) > 0 && strcmp(test, "1") == 0) {
        snprintf(why, why_len, "turned off for a test (debug.vento.realvnc.unhooked)");
        return false;
    }
#endif
    struct core c = {0};
    dl_iterate_phdr(find_core, &c);
    if (c.count != 1) {
        snprintf(why, why_len, "libvncviewer.so is loaded %d times", c.count);
        return false;
    }
    const ElfW(Dyn) *dyn = NULL;
    ElfW(Addr) relro = 0, relro_end = 0;
    for (ElfW(Half) i = 0; i < c.phnum; i++) {
        if (c.phdr[i].p_type == PT_DYNAMIC) {
            dyn = (const ElfW(Dyn) *) (c.base + c.phdr[i].p_vaddr);
        } else if (c.phdr[i].p_type == PT_GNU_RELRO) {
            relro = c.base + c.phdr[i].p_vaddr;
            relro_end = relro + c.phdr[i].p_memsz;
        }
    }
    if (!dyn) {
        snprintf(why, why_len, "no dynamic section");
        return false;
    }
    // Bionic leaves the dynamic section as linked: addresses in it are the
    // file's, relative to the load address.
    ElfW(Addr) symtab = 0, strtab = 0, jmprel = 0, rela = 0, gnu_hash = 0, sysv_hash = 0;
    size_t strsz = 0, pltrelsz = 0, relasz = 0, syment = 0, relaent = sizeof(ElfW(Rela));
    long pltrel = 0;
    bool now = false;
    for (const ElfW(Dyn) *d = dyn; d->d_tag != DT_NULL; d++) {
        switch (d->d_tag) {
            case DT_SYMTAB: symtab = c.base + d->d_un.d_ptr; break;
            case DT_GNU_HASH: gnu_hash = c.base + d->d_un.d_ptr; break;
            case DT_HASH: sysv_hash = c.base + d->d_un.d_ptr; break;
            case DT_STRTAB: strtab = c.base + d->d_un.d_ptr; break;
            case DT_STRSZ: strsz = d->d_un.d_val; break;
            case DT_SYMENT: syment = d->d_un.d_val; break;
            case DT_JMPREL: jmprel = c.base + d->d_un.d_ptr; break;
            case DT_PLTRELSZ: pltrelsz = d->d_un.d_val; break;
            case DT_PLTREL: pltrel = (long) d->d_un.d_val; break;
            case DT_RELA: rela = c.base + d->d_un.d_ptr; break;
            case DT_RELASZ: relasz = d->d_un.d_val; break;
            case DT_RELAENT: relaent = d->d_un.d_val; break;
            case DT_FLAGS: now |= (d->d_un.d_val & DF_BIND_NOW) != 0; break;
            case DT_FLAGS_1: now |= (d->d_un.d_val & DF_1_NOW) != 0; break;
            case DT_REL:
            case DT_ANDROID_REL_TAG:
            case DT_ANDROID_RELA_TAG:
                snprintf(why, why_len, "relocations of a kind not read here (tag %#lx)",
                         (unsigned long) d->d_tag);
                return false;
            default: break;
        }
    }
    if (!symtab || !strtab || !jmprel || pltrel != DT_RELA || syment != sizeof(ElfW(Sym))
        || relaent != sizeof(ElfW(Rela)) || !now) {
        snprintf(why, why_len, "not the dynamic section expected (BIND_NOW with RELA PLT relocations)");
        return false;
    }
    const ElfW(Sym) *syms = (const ElfW(Sym) *) symtab;
    const char *strs = (const char *) strtab;
    const size_t nsyms = symbol_count(gnu_hash, sysv_hash);
    if (!nsyms) {
        snprintf(why, why_len, "no hash table to count the symbols by");
        return false;
    }

    // Each checked import: its one JUMP_SLOT, and no other reference.
    void **slots[CHECKED_COUNT] = {0};
    const ElfW(Rela) *plt = (const ElfW(Rela) *) jmprel;
    for (size_t i = 0; i < pltrelsz / sizeof *plt; i++) {
        const uint32_t sym = (uint32_t) ELF64_R_SYM(plt[i].r_info);
        if (ELF64_R_TYPE(plt[i].r_info) != R_JUMP_SLOT || !sym || sym >= nsyms
            || syms[sym].st_name >= strsz) {
            continue;
        }
        const char *name = strs + syms[sym].st_name;
        for (size_t k = 0; k < CHECKED_COUNT; k++) {
            if (strcmp(name, CHECKED[k]) == 0) {
                if (slots[k]) {
                    snprintf(why, why_len, "%s has two JUMP_SLOTs", name);
                    return false;
                }
                slots[k] = (void **) (c.base + plt[i].r_offset);
            }
        }
    }
    const ElfW(Rela) *other = (const ElfW(Rela) *) rela;
    for (size_t i = 0; rela && i < relasz / sizeof *other; i++) {
        const uint32_t sym = (uint32_t) ELF64_R_SYM(other[i].r_info);
        const uint32_t type = (uint32_t) ELF64_R_TYPE(other[i].r_info);
        if (!sym || sym >= nsyms || syms[sym].st_name >= strsz
            || (type != R_GLOB_DAT && type != R_ABS64)) {
            continue;
        }
        const char *name = strs + syms[sym].st_name;
        for (size_t k = 0; k < CHECKED_COUNT; k++) {
            if (strcmp(name, CHECKED[k]) == 0) {
                snprintf(why, why_len, "%s is also referred to by a relocation of type %u", name, type);
                return false;
            }
        }
    }
    void *libc = dlopen("libc.so", RTLD_NOW | RTLD_NOLOAD);
    if (!libc) {
        snprintf(why, why_len, "libc.so: %s", dlerror());
        return false;
    }
    for (size_t k = 0; k < CHECKED_COUNT; k++) {
        void *real = dlsym(libc, CHECKED[k]);
        if (!slots[k] || !real || *slots[k] != real) {
            snprintf(why, why_len, "%s: %s", CHECKED[k],
                     !slots[k] ? "no JUMP_SLOT" : "its slot does not hold libc's function");
            dlclose(libc);
            return false;
        }
    }
    dlclose(libc);

    // The hooks' slots: every page made writable first, then every slot
    // written (each hook's `next` before any slot, so that a call through a
    // slot finds it), then the pages in RELRO put back read-only, as the
    // linker left them; a slot outside RELRO is on a page that was
    // writable already.
    void **targets[REALVNC_NET_HOOKS];
    void *was[REALVNC_NET_HOOKS];
    for (size_t h = 0; h < REALVNC_NET_HOOKS; h++) {
        targets[h] = NULL;
        for (size_t k = 0; k < CHECKED_COUNT; k++) {
            if (strcmp(CHECKED[k], realvnc_net_hooks[h].name) == 0) {
                targets[h] = slots[k];
            }
        }
        if (!targets[h]) {
            snprintf(why, why_len, "%s is not among the checked imports", realvnc_net_hooks[h].name);
            return false;
        }
        was[h] = *targets[h];
    }
    size_t failed = 0, ignored = 0;
    if (!protect(targets, REALVNC_NET_HOOKS, PROT_READ | PROT_WRITE, false, relro, relro_end, &failed)) {
        snprintf(why, why_len, "making %s's slot writable: %s", realvnc_net_hooks[failed].name,
                 strerror(errno));
        protect(targets, failed, PROT_READ, true, relro, relro_end, &ignored);
        return false;
    }
    for (size_t h = 0; h < REALVNC_NET_HOOKS; h++) {
        *realvnc_net_hooks[h].next = was[h];
    }
    for (size_t h = 0; h < REALVNC_NET_HOOKS; h++) {
        __atomic_store_n(targets[h], realvnc_net_hooks[h].hook, __ATOMIC_RELEASE);
    }
    bool written = true;
    for (size_t h = 0; h < REALVNC_NET_HOOKS && written; h++) {
        if (*targets[h] != realvnc_net_hooks[h].hook) {
            snprintf(why, why_len, "%s's slot did not take the hook", realvnc_net_hooks[h].name);
            written = false;
        }
    }
    if (!written) {
        for (size_t h = 0; h < REALVNC_NET_HOOKS; h++) {
            __atomic_store_n(targets[h], was[h], __ATOMIC_RELEASE);
        }
    }
    protect(targets, REALVNC_NET_HOOKS, PROT_READ, true, relro, relro_end, &ignored);
    return written;
}

static pthread_mutex_t patch_lock = PTHREAD_MUTEX_INITIALIZER;
static int patched; // 0: not tried, 1: in place, -1: failed

/* The hooks' notice (`realvnc_net_set_notice`) as `RealVncNet.notice`,
   which shows a toast: the hooks call it from the core's threads, which
   are not attached to the VM, so the call attaches and detaches. */
static JavaVM *vm;
static jclass net_class; // a global reference to RealVncNet
static jmethodID notice_method;

static void jni_notice(const char *text) {
    JNIEnv *env = NULL;
    if (!vm || !net_class || !notice_method) {
        return;
    }
    const jint got = (*vm)->GetEnv(vm, (void **) &env, JNI_VERSION_1_6);
    const bool attached_here = got == JNI_EDETACHED;
    if (attached_here && (*vm)->AttachCurrentThread(vm, &env, NULL) != JNI_OK) {
        return;
    }
    if (env) {
        jstring s = (*env)->NewStringUTF(env, text);
        if (s) {
            (*env)->CallStaticVoidMethod(env, net_class, notice_method, s);
            if ((*env)->ExceptionCheck(env)) {
                (*env)->ExceptionClear(env);
            }
            (*env)->DeleteLocalRef(env, s);
        }
    }
    if (attached_here) {
        (*vm)->DetachCurrentThread(vm);
    }
}

/* Keeps what `jni_notice` needs from the first attach. Under `patch_lock`. */
static void install_notice(JNIEnv *env, jclass clazz) {
    if (vm) {
        return;
    }
    jclass global = (*env)->NewGlobalRef(env, clazz);
    jmethodID method = (*env)->GetStaticMethodID(env, clazz, "notice", "(Ljava/lang/String;)V");
    if (!global || !method || (*env)->GetJavaVM(env, &vm) != JNI_OK) {
        if ((*env)->ExceptionCheck(env)) {
            (*env)->ExceptionClear(env);
        }
        if (global) {
            (*env)->DeleteGlobalRef(env, global);
        }
        vm = NULL;
        return;
    }
    net_class = global;
    notice_method = method;
    realvnc_net_set_notice(jni_notice);
}

/* A session's route: positive where its connections go through the
   session's network, negative where the core dials them itself. Throws
   java.io.IOException, with the reason, where the session must not start,
   direct or not: its route is not direct and cannot be put under the core,
   another session to the same server is open on another route, or there
   are too many sessions. A direct session refused here and started anyway
   would have its connections taken for another session's once the hooks
   are in place. */
JNIEXPORT jint JNICALL Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncNet_nativeAttach(
        JNIEnv *env, jclass clazz, jstring address, jint timeout_ms, jboolean force_socketpair) {
    vento_net net;
    const bool relayed = vento_realvnc_network(force_socketpair == JNI_TRUE, &net);
    pthread_mutex_lock(&patch_lock);
    install_notice(env, clazz);
    if (relayed && patched == 0) {
        char why[256] = "";
        patched = patch(why, sizeof why) ? 1 : -1;
        realvnc_net_installed(patched > 0, why);
        if (patched > 0) {
            vento_log(VENTO_LOG_INFO, TAG, "the core's socket calls are hooked");
        } else {
            vento_log(VENTO_LOG_WARN, TAG, "the core's socket calls could not be hooked: %s", why);
        }
    }
    pthread_mutex_unlock(&patch_lock);
    // Every session attaches, hooked or not: a direct one so that a later
    // session to its server on another route is refused, and so that the
    // hooks, once a later session puts them in place, know its server is
    // its own; one that is not direct is refused where the hooks are not
    // in place, with the reason.
    const char *a = address ? (*env)->GetStringUTFChars(env, address, NULL) : NULL;
    char why[600] = "";
    char tunnel[128] = "";
    if (relayed) {
        vento_realvnc_tunnel(true, tunnel, sizeof tunnel);
    }
    const int route = realvnc_net_attach(relayed ? &net : NULL, tunnel, a ? a : "", timeout_ms, why,
                                         sizeof why);
    if (a) {
        (*env)->ReleaseStringUTFChars(env, address, a);
    }
    if (route == 0) {
        jclass io = (*env)->FindClass(env, "java/io/IOException");
        if (io) {
            (*env)->ThrowNew(env, io, why);
        }
        return 0;
    }
    return relayed ? route : -route;
}

JNIEXPORT void JNICALL Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncNet_nativeClosed(
        JNIEnv *env, jclass clazz, jint route) {
    (void) env;
    (void) clazz;
    realvnc_net_closed(route < 0 ? -route : route);
}

/* What carries the connections of a session made with `force_socketpair`,
   for the panel's Tunnel row; NULL where they are the core's own. */
JNIEXPORT jstring JNICALL Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncNet_nativeTunnel(
        JNIEnv *env, jclass clazz, jboolean force_socketpair) {
    (void) clazz;
    char text[256];
    if (!vento_realvnc_tunnel(force_socketpair == JNI_TRUE, text, sizeof text)) {
        return NULL;
    }
    return (*env)->NewStringUTF(env, text);
}

JNIEXPORT void JNICALL Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncNet_nativeCancel(
        JNIEnv *env, jclass clazz, jint route) {
    (void) env;
    (void) clazz;
    realvnc_net_cancel(route < 0 ? -route : route);
}

JNIEXPORT void JNICALL Java_net_pgaskin_remotedesktop_backend_realvnc_RealVncNet_nativeDetach(
        JNIEnv *env, jclass clazz, jint route) {
    (void) env;
    (void) clazz;
    realvnc_net_detach(route < 0 ? -route : route);
}
