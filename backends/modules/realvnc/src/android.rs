// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The session's network on the phone, as the table in `vento/net.h`, for
//! `cpp/realvnc_got.c`: the module's native library, which puts it under the
//! RealVNC core's connections as the Linux hosting layer does. The network
//! is the one the other backends get from `platform_android`, so a setting
//! that changes theirs changes this one too. This crate is linked into that
//! library as a static library, and these two functions are all it exports.

use api::c::ffi::vento_net;

/// Fills in `out` with a session's network, relayed through a socketpair
/// where `force_socketpair` (the app's developer setting) says so, and
/// returns true; returns false and leaves `out` alone where the network is
/// direct, which the core dials itself. The caller owns the table from
/// then on (`vento_net.release`).
///
/// # Safety
///
/// `out` is NULL or points to writable memory for a `vento_net`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vento_realvnc_network(force_socketpair: bool, out: *mut vento_net) -> bool {
    platform_android::log::init("RealVnc");
    let network = platform_android::backend::network(force_socketpair);
    if network.direct() || out.is_null() {
        return false;
    }
    unsafe { out.write(api::c::net::table(network)) };
    true
}

/// Writes what carries a session's connections where `force_socketpair`
/// says so (`Network::tunnel`: "dev-socketpair"), NUL-terminated and
/// truncated to `len`, for the panel's Tunnel row; returns false and writes
/// nothing where the network is direct.
///
/// # Safety
///
/// `out` is NULL or points to `len` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vento_realvnc_tunnel(force_socketpair: bool, out: *mut std::ffi::c_char, len: usize) -> bool {
    let Some(text) = platform_android::backend::network(force_socketpair).tunnel() else {
        return false;
    };
    if out.is_null() || len == 0 {
        return false;
    }
    let n = text.len().min(len - 1);
    let mut end = n;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), out.cast::<u8>(), end);
        *out.add(end) = 0;
    }
    true
}
