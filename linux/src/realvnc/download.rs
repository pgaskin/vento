// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Downloading the viewer's APK, as the add-on does
//! (`plugins/src/main/java/net/pgaskin/remotedesktop/plugin/Download.java`):
//! from its sources in turn, or from a URL the user gave, with redirects
//! followed here so that each hop's scheme is checked, a browser's
//! User-Agent, and every source's failure reported. Nothing here checks what
//! arrives: the pins do, once the libraries are extracted.
//!
//! Over HTTPS with the system's trusted certificates, and through the proxy
//! the environment names (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and
//! `NO_PROXY`, HTTP CONNECT or SOCKS), as ureq reads them.
//! `VENTO_REALVNC_DOWNLOAD_INSECURE` turns off the certificate checks and
//! follows redirects from https to http.

use super::{Error, Report, Step};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

/// The add-on's (`Download.AGENT`): some hosts refuse anything that is not
/// a browser.
const AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                     Chrome/131.0.0.0 Safari/537.36";

/// RealVNC Viewer's package name, as the Aptoide API takes it, and the
/// version code of the release the pins are for (`Viewer.java`).
const PACKAGE: &str = "com.realvnc.viewer.android";
const VERSION_CODE: u32 = 2070175;

/// How a source's URL turns into the URL of the file itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The URL is the file.
    Direct,
    /// Aptoide's API, which answers with the path of one exact build.
    Aptoide,
}

/// The add-on's sources, tried in order (`Viewer.sources`): Aptoide's API for
/// the exact version code, then a Wayback Machine snapshot of the file it
/// pointed at.
fn pinned() -> [(Kind, String); 2] {
    [
        (
            Kind::Aptoide,
            format!("https://ws75.aptoide.com/api/7/app/getMeta?package_name={PACKAGE}&vercode={VERSION_CODE}"),
        ),
        (
            Kind::Direct,
            "https://web.archive.org/web/20260811195414id_/https://pool.apk.aptoide.com/appupdater/\
             com-realvnc-viewer-android-2070175-75463342-f98743311290dcd93f886016879cadfb.apk"
                .to_string(),
        ),
    ]
}

/// The largest download accepted: RealVNC's APK is 11.7 MB, so this only
/// stops a server that sends without end.
const MAX_BYTES: u64 = 64 << 20;

/// The add-on's messages (`Download.java`).
const FAILED: &str = "That did not work.";
const INSECURE_REDIRECT: &str = "The download was redirected to an insecure address.";
const REDIRECT_NOWHERE: &str = "The download was redirected to nowhere.";
const TOO_MANY_REDIRECTS: &str = "The download was redirected too many times.";
const TOO_LARGE: &str = "The download is larger than this can accept.";
const NOWHERE: &str = "There is nowhere to fetch it from.";
const NO_PATH: &str = "The download service did not say where the file is.";
const BAD_ANSWER: &str = "The download service did not answer the way it is documented to.";

/// The APK from the add-on's sources, or from `url`.
pub fn apk(url: Option<&str>, report: &dyn Report) -> Result<Vec<u8>, Error> {
    let insecure = insecure();
    if insecure {
        log::warn!(
            "VENTO_REALVNC_DOWNLOAD_INSECURE is set: certificates are not checked, and redirects from https to \
             http are followed"
        );
    }
    let agent = agent(insecure);
    let sources = match url {
        Some(url) => vec![(Kind::Direct, url.to_string())],
        None => pinned().to_vec(),
    };
    // Every failure, not only the last: two sources fail for different
    // reasons, and the first is usually the one that works.
    let mut failures = Vec::new();
    for (kind, url) in &sources {
        if report.cancelled() {
            return Err(Error::Cancelled);
        }
        let result = (|| {
            let file = match kind {
                Kind::Direct => url.clone(),
                Kind::Aptoide => aptoide(&agent, url, insecure, report)?,
            };
            report.step(Step::Downloading(&host(&file)));
            fetch(&agent, &file, insecure, MAX_BYTES, report, true)
        })();
        match result {
            Ok(apk) => return Ok(apk),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(Error::Failed(why)) => {
                log::warn!("fetching from {url}: {why}");
                failures.push(why);
            }
        }
    }
    Err(Error::Failed(if failures.is_empty() { NOWHERE.into() } else { failures.join("\n") }))
}

/// Whether `VENTO_REALVNC_DOWNLOAD_INSECURE` is set: to anything but empty
/// or `0`.
fn insecure() -> bool {
    std::env::var_os("VENTO_REALVNC_DOWNLOAD_INSECURE").is_some_and(|v| !v.is_empty() && v != "0")
}

fn agent(insecure: bool) -> ureq::Agent {
    let roots = if insecure {
        Vec::new()
    } else {
        let found = rustls_native_certs::load_native_certs();
        for e in &found.errors {
            log::warn!("the system's trusted certificates: {e}");
        }
        if found.certs.is_empty() {
            log::warn!("no trusted certificates were found on this system");
        }
        found
            .certs
            .iter()
            .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
            .collect()
    };
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::Specific(Arc::new(roots)))
        .disable_verification(insecure)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls)
        .user_agent(AGENT)
        // Followed here, each hop checked (`fetch`).
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .timeout_recv_body(Some(Duration::from_secs(15 * 60)))
        .build()
        .new_agent()
}

/// The host in a URL, or the URL itself where it has none.
fn host(url: &str) -> String {
    url.parse::<ureq::http::Uri>()
        .ok()
        .and_then(|u| u.host().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// Asks Aptoide's API at `url` which file it has, and returns its URL.
fn aptoide(agent: &ureq::Agent, url: &str, insecure: bool, report: &dyn Report) -> Result<String, Error> {
    report.step(Step::Asking(&host(url)));
    log::info!("looking for APK on {}", host(url));
    let meta = fetch(agent, url, insecure, 1 << 20, report, false)?;
    let bad = |why: &dyn std::fmt::Display| {
        log::warn!("{url}: {why}");
        Error::Failed(BAD_ANSWER.into())
    };
    let json: serde_json::Value = serde_json::from_slice(&meta).map_err(|e| bad(&e))?;
    let path = json["data"]["file"]["path"]
        .as_str()
        .ok_or_else(|| bad(&"no data.file.path"))?;
    if path.is_empty() {
        return Err(Error::Failed(NO_PATH.into()));
    }
    Ok(path.to_string())
}

/// The proxy ureq takes from the environment for `url`, and the variable it
/// came from, for the log; `None` where there is none or `NO_PROXY` names
/// the host.
fn proxy(url: &str) -> Option<(String, &'static str)> {
    let proxy = ureq::Proxy::try_from_env()?;
    let uri: ureq::http::Uri = url.parse().ok()?;
    if proxy.is_no_proxy(&uri) {
        return None;
    }
    // The order `Proxy::try_from_env` reads them in.
    let var = ["ALL_PROXY", "all_proxy", "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
        .into_iter()
        .find(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
        .unwrap_or("the environment");
    Some((format!("{}:{}", proxy.host(), proxy.port()), var))
}

/// The file at `url`, following redirects (at most five) and checking each
/// hop's scheme: once on https, never back to http unless `insecure`. The
/// add-on's sources are https, and a URL the user gave may start on http.
/// The body is read in pieces, with the progress reported where `download`
/// says it is the APK.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    insecure: bool,
    max: u64,
    report: &dyn Report,
    download: bool,
) -> Result<Vec<u8>, Error> {
    let https = |u: &str| u.len() >= 8 && u[..8].eq_ignore_ascii_case("https://");
    let mut location = url.to_string();
    let mut was_https = https(url);
    for _ in 0..=5 {
        if !https(&location) && was_https && !insecure {
            return Err(Error::Failed(INSECURE_REDIRECT.into()));
        }
        was_https |= https(&location);
        let host = host(&location);
        let via = proxy(&location);
        let mut response = agent.get(&location).call().map_err(|e| failed(&e, &host, via.as_ref()))?;
        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let next = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or(Error::Failed(REDIRECT_NOWHERE.into()))?;
            location = resolve(&location, next).ok_or(Error::Failed(REDIRECT_NOWHERE.into()))?;
            continue;
        }
        if status != 200 {
            return Err(Error::Failed(format!("The download failed with HTTP {status}.")));
        }
        // The size the server declares, only for the progress bar: the
        // limit counts the bytes that arrive.
        let total = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if download {
            let size = total.map_or(String::new(), |t| format!(" ({:.1} MB)", t as f64 / 1e6));
            match &via {
                Some((proxy, var)) => {
                    log::info!("downloading the APK from {host}{size} through the proxy {proxy} ({var})")
                }
                None => log::info!("downloading the APK from {host}{size}"),
            }
        }
        let mut reader = response.body_mut().with_config().limit(max + 1).reader();
        let mut out = Vec::new();
        let mut buf = vec![0; 64 * 1024];
        if download {
            report.progress(0, total);
        }
        loop {
            if report.cancelled() {
                return Err(Error::Cancelled);
            }
            let n = match reader.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("{location}: {e}");
                    return Err(Error::Failed(format!("{host} could not be reached.")));
                }
            };
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() as u64 > max {
                return Err(Error::Failed(TOO_LARGE.into()));
            }
            if download {
                report.progress(out.len() as u64, total);
            }
        }
        return Ok(out);
    }
    Err(Error::Failed(TOO_MANY_REDIRECTS.into()))
}

/// A redirect's `Location`, which may be relative, against `base`.
fn resolve(base: &str, next: &str) -> Option<String> {
    if next.contains("://") {
        return Some(next.to_string());
    }
    let uri: ureq::http::Uri = base.parse().ok()?;
    let origin = format!("{}://{}", uri.scheme_str()?, uri.authority()?);
    Some(match next {
        _ if next.starts_with("//") => format!("{}:{next}", uri.scheme_str()?),
        _ if next.starts_with('/') => format!("{origin}{next}"),
        _ => {
            let path = uri.path();
            format!("{origin}{}{next}", &path[..path.rfind('/').map_or(0, |i| i + 1)])
        }
    })
}

/// A failed request in the words the add-on's screen would use, with the
/// details in the log.
fn failed(e: &ureq::Error, host: &str, via: Option<&(String, &str)>) -> Error {
    log::warn!("{host}: {e}");
    // A handshake that failed on the certificate comes as I/O from rustls.
    let tls = matches!(e, ureq::Error::Io(io) if io.get_ref().is_some_and(|i| i.is::<rustls::Error>()));
    Error::Failed(match e {
        _ if tls => format!("The certificate of {host} could not be verified."),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => {
            format!("The certificate of {host} could not be verified.")
        }
        ureq::Error::ConnectProxyFailed(_) => match via {
            Some((proxy, _)) => format!("The proxy {proxy} could not be reached."),
            None => FAILED.into(),
        },
        ureq::Error::Io(_) | ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Timeout(_) => {
            match via {
                Some((proxy, _)) => format!("The proxy {proxy} could not be reached."),
                None => format!("{host} could not be reached."),
            }
        }
        ureq::Error::BodyExceedsLimit(_) => TOO_LARGE.into(),
        ureq::Error::TooManyRedirects => TOO_MANY_REDIRECTS.into(),
        _ => FAILED.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sources are the add-on's: `Viewer.java` builds the same URLs out
    /// of the same parts.
    #[test]
    fn the_sources_are_the_add_ons() {
        let java = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../plugins/realvnc/src/main/java/net/pgaskin/remotedesktop/realvnc/Viewer.java"
        ))
        .unwrap();
        // Its string concatenations joined, and its constants put in.
        let joined: String = java
            .split('\n')
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("")
            .replace("\" + \"", "")
            .replace("\"+ \"", "")
            .replace("\" + PACKAGE + \"", PACKAGE)
            .replace("\" + VERSION_CODE),", &format!("{VERSION_CODE}\"),"));
        assert!(java.contains(&format!("PACKAGE = \"{PACKAGE}\"")));
        assert!(java.contains(&format!("VERSION_CODE = {VERSION_CODE};")));
        for (kind, url) in pinned() {
            let kind = match kind {
                Kind::Aptoide => "APTOIDE",
                Kind::Direct => "DIRECT",
            };
            assert!(joined.contains(&format!("Download.Kind.{kind},\"{url}")), "{url}\n{joined}");
        }
    }

    #[test]
    fn redirects_are_resolved_against_the_url() {
        assert_eq!(resolve("https://a.example/x/y?q", "https://b.example/z").unwrap(), "https://b.example/z");
        assert_eq!(resolve("https://a.example/x/y?q", "/z").unwrap(), "https://a.example/z");
        assert_eq!(resolve("https://a.example:8443/x/y", "z").unwrap(), "https://a.example:8443/x/z");
        assert_eq!(resolve("http://a.example/x", "//b.example/z").unwrap(), "http://b.example/z");
    }

    #[test]
    fn the_insecure_switch_is_anything_but_empty_or_zero() {
        // One test, since the variable is the process's.
        for (value, on) in [(None, false), (Some(""), false), (Some("0"), false), (Some("1"), true), (Some("yes"), true)] {
            match value {
                // SAFETY: no other test reads or writes this variable.
                Some(v) => unsafe { std::env::set_var("VENTO_REALVNC_DOWNLOAD_INSECURE", v) },
                None => unsafe { std::env::remove_var("VENTO_REALVNC_DOWNLOAD_INSECURE") },
            }
            assert_eq!(insecure(), on, "{value:?}");
        }
        unsafe { std::env::remove_var("VENTO_REALVNC_DOWNLOAD_INSECURE") };
    }
}
