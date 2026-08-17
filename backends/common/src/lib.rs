//! What a backend's JNI half needs of Android rather than of a protocol: a
//! locked bitmap, the calls back into Java, a slot to park a protocol thread in
//! until somebody answers a dialog.
//!
//! It held the TLS half as well until a protocol arrived that reaches no TLS at
//! all. What decided the split was neither taste nor compile time: cargo
//! resolves a workspace's features as a union, so a crate that depends on this
//! one is a crate whose dependency graph names rustls whether it uses it or
//! not — and an artefact's licence page is generated from that graph.

#[cfg(target_os = "android")]
pub mod android;
