// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The static description of each of the eight backends: its name, why to
//! pick it, its place in a picker, and its options. One list for every
//! client (the Linux client's picker reads it, and each of the phone's
//! `BackendProvider`s answers from a class `vento-codegen` generates from
//! it), so that a backend is described once.
//!
//! Each entry is its backend's own file, `backends/modules/<name>/catalog.rs`
//! (its [`About`], its options table and their tests), compiled here by path
//! rather than by the backend's crate: `vento-codegen` builds on the host
//! from this crate alone, without the backends' libraries, bindgen or async
//! runtimes, and Gradle reruns it when any of those files changes
//! (`codegenSources` in `backends/build.gradle`). This file holds the shape
//! of an entry and the registry; a rule shared by every backend reads an
//! entry's data (its pinning, whether its user name carries a domain) rather
//! than checking which backend it is.

use crate::options::Spec;
use crate::trust::Pinning;

/// One backend, described.
#[derive(Debug, PartialEq, Eq)]
pub struct About {
    /// Stored in connection records and used on the command line, so it must
    /// not change once shipped.
    pub id: &'static str,
    /// For a picker and a title.
    pub name: &'static str,
    /// A word or two, for a small badge. Two backends may speak one
    /// protocol, so this sometimes names the implementation.
    pub short_name: &'static str,
    /// Why to choose this backend, in a sentence or two: what it does that
    /// the others do not, and at what cost.
    pub description: &'static str,
    /// Position in a picker, lowest first.
    pub order: i32,
    /// What a far end's identity is pinned under, and whether the first one
    /// is accepted without asking ([`trust::pinned_as`](crate::trust::pinned_as)).
    pub pinning: Pinning,
    /// Whether a user name carries a domain, `DOMAIN\user` or `user@domain`
    /// (as in RDP): a credentials prompt has no domain field, so it starts
    /// with the domain in the name ([`credentials::prompt_user`](crate::credentials::prompt_user)).
    /// The Linux client's connection dialog has a Domain field only for such
    /// a backend, and sends no domain to the others.
    pub user_has_domain: bool,
    /// Whether the backend's library has its own identity store: it returns
    /// an identity to be remembered ([`Backend::saved_identity`]) and expects
    /// it back the next time ([`Connect::identity`]). Only RealVNC's core
    /// does. A client reads its stored identities only for such a backend.
    ///
    /// [`Backend::saved_identity`]: crate::backend::Backend::saved_identity
    /// [`Connect::identity`]: crate::kind::Connect::identity
    pub saves_identity: bool,
    pub options: &'static [Spec],
}

// One file per backend, in that backend's module; the paths are relative to
// this file.
#[path = "../../modules/rfb/catalog.rs"]
mod rfb;
#[path = "../../modules/libvnc/catalog.rs"]
mod libvnc;
#[path = "../../modules/tigervnc/catalog.rs"]
mod tigervnc;
#[path = "../../modules/freerdp/catalog.rs"]
mod freerdp;
#[path = "../../modules/ironrdp/catalog.rs"]
mod ironrdp;
#[path = "../../modules/realvnc/catalog.rs"]
mod realvnc;
#[path = "../../modules/spice/catalog.rs"]
mod spice;
#[path = "../../modules/rustdesk/catalog.rs"]
mod rustdesk;

pub use freerdp::FREERDP;
pub use ironrdp::IRONRDP;
pub use libvnc::LIBVNC;
pub use realvnc::REALVNC;
pub use rfb::RFB;
pub use rustdesk::RUSTDESK;
pub use spice::SPICE;
pub use tigervnc::TIGERVNC;

/// All eight, in picker order.
pub static ALL: [&About; 8] = [&RFB, &LIBVNC, &TIGERVNC, &FREERDP, &IRONRDP, &REALVNC, &SPICE, &RUSTDESK];

/// A placeholder entry for testing the rules that read one: pinned under
/// the address, no domain, no options.
#[cfg(test)]
pub(crate) const FIXTURE: About = About {
    id: "fixture",
    name: "Fixture",
    short_name: "Fixture",
    description: "",
    order: 0,
    pinning: Pinning::ADDRESS,
    user_has_domain: false,
    saves_identity: false,
    options: &[],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::Options;

    #[test]
    fn every_backend_has_a_distinct_id_and_place_and_defaults_that_pass_its_own_checks() {
        for (i, about) in ALL.iter().enumerate() {
            for other in &ALL[i + 1..] {
                assert_ne!(about.id, other.id);
                assert!(about.order < other.order, "{} before {}", about.id, other.id);
            }
            let options = Options::defaults(about.options);
            for spec in about.options {
                assert!(spec.check(options.get(spec.key)).is_ok(), "{}/{}", about.id, spec.key);
                // Nothing in any list has a default that needs a switch.
                assert!(spec.stranded(spec.default, |_| false).is_none(), "{}/{}", about.id, spec.key);
            }
        }
    }
}
