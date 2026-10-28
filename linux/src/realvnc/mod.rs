// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where RealVNC's viewer libraries come from when they are missing or of
//! another build: the RealVNC backend's `AutoFetchLibraries` option, which
//! only this client offers (the phone's add-on has a screen of its own).
//! The backend itself ignores the key; the client reads it before a session
//! starts.

use crate::options::Options;
use std::path::PathBuf;

/// The option's key.
pub const OPTION: &str = "AutoFetchLibraries";

/// What `AutoFetchLibraries` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Never: the reason a session cannot start says how to install them.
    Off,
    /// Ask, then download the pinned APK. The default.
    Prompt,
    /// Download the pinned APK without asking.
    Auto,
    /// Extract them from this APK.
    Apk(PathBuf),
    /// Download the APK from this `http://` or `https://` URL.
    Url(String),
}

impl Fetch {
    /// The value as the option holds it: `off`, `prompt` (or empty, the
    /// default), `auto`, a URL, or anything else as a path.
    pub fn parse(value: &str) -> Result<Fetch, String> {
        let scheme = |s: &str| value.len() >= s.len() && value[..s.len()].eq_ignore_ascii_case(s);
        Ok(match value {
            "" | "prompt" => Fetch::Prompt,
            "off" => Fetch::Off,
            "auto" => Fetch::Auto,
            _ if scheme("http://") || scheme("https://") => Fetch::Url(value.to_string()),
            _ if value.contains("://") => {
                return Err(format!(
                    "{OPTION}: off, prompt, auto, the path of an APK, or an http:// or https:// URL"
                ));
            }
            _ => Fetch::Apk(PathBuf::from(value)),
        })
    }

    /// The option in a backend's options.
    pub fn of(options: &Options) -> Result<Fetch, String> {
        Fetch::parse(options.get(OPTION))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_values_are_read_as_the_option_says() {
        assert_eq!(Fetch::parse(""), Ok(Fetch::Prompt));
        assert_eq!(Fetch::parse("prompt"), Ok(Fetch::Prompt));
        assert_eq!(Fetch::parse("off"), Ok(Fetch::Off));
        assert_eq!(Fetch::parse("auto"), Ok(Fetch::Auto));
        assert_eq!(Fetch::parse("https://example.com/a.apk"), Ok(Fetch::Url("https://example.com/a.apk".into())));
        assert_eq!(Fetch::parse("HTTP://example.com/a.apk"), Ok(Fetch::Url("HTTP://example.com/a.apk".into())));
        assert_eq!(Fetch::parse("viewer.apk"), Ok(Fetch::Apk("viewer.apk".into())));
        assert_eq!(Fetch::parse("/x/viewer.apk"), Ok(Fetch::Apk("/x/viewer.apk".into())));
        assert_eq!(
            Fetch::parse("ftp://example.com/a.apk"),
            Err("AutoFetchLibraries: off, prompt, auto, the path of an APK, or an http:// or https:// URL".into())
        );
    }
}
