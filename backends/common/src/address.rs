// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What somebody typed into the address field, split into a host and a port.
//!
//! One implementation for the four clients written here, because the four had
//! four and they did not agree: the same literal reached one of them as an
//! address, another as a host with a port after it, and a third as nothing it
//! could resolve. The three shims over somebody else's client have the same
//! rule written again in their own language, which is as close to sharing as C
//! and Rust get.

/// What a number after the colon means, which is a fact about the protocol
/// rather than about the address.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Ports {
    /// A number is a port.
    Plain,
    /// Under 100 is a display number, the way `:1` has meant since the 1990s,
    /// and `host::port` is how somebody says they meant the port.
    Display,
}

/// Split `host`, `host:port`, `[literal]` or `[literal]:port`, where a host is
/// a name or an IPv4 literal.
///
/// **An IPv6 literal is bracketed, always.** Unbracketed it cannot be told from
/// a host with a port after it — `fdb7::1` is either an address or `fdb7` on
/// port 1, and under [`Ports::Display`] `::1:1` would have to be an address and
/// a display at once. Every available guess lands somewhere real: on a
/// different machine, or on the wildcard address, which is where two of these
/// clients used to end up. So more than one colon outside brackets is refused,
/// and the only unbracketed address with two of them that is not is
/// `host::port`, which is older than the ambiguity and is the escape from the
/// display rule.
///
/// The error is a sentence rather than a code because it is shown as the reason
/// a session closed.
pub fn split(address: &str, default_port: u16, ports: Ports) -> Result<(String, u16), String> {
    let address = address.trim();
    if let Some(rest) = address.strip_prefix('[') {
        let (host, rest) = rest
            .split_once(']')
            .ok_or_else(|| format!("{address} missing closing bracket"))?;
        let rest = rest.trim();
        let port = match rest.strip_prefix(':') {
            Some(tail) => number(tail, address, ports)?,
            None if rest.is_empty() => default_port,
            None => return Err(format!("{address} has junk after closing bracket")),
        };
        return host_and(host.trim(), port);
    }
    // `host::port` before the split below, since it is the one form where the
    // last colon is not where the port starts.
    if ports == Ports::Display
        && let Some((host, tail)) = address.split_once("::")
        && !host.contains(':')
        && !tail.contains(':')
    {
        return host_and(host, number(tail, address, Ports::Plain)?);
    }
    match address.rsplit_once(':') {
        Some((host, _)) if host.contains(':') => Err(bracketed(address, default_port)),
        Some((host, tail)) => host_and(host, number(tail, address, ports)?),
        None => host_and(address, default_port),
    }
}

fn bracketed(address: &str, default_port: u16) -> String {
    format!("IPv6 addresses must be bracketed, like [::1]:{default_port}, not {address}")
}

fn host_and(host: &str, port: u16) -> Result<(String, u16), String> {
    match host.is_empty() {
        true => Err("No host in address".into()),
        false => Ok((host.to_string(), port)),
    }
}

fn number(tail: &str, address: &str, ports: Ports) -> Result<u16, String> {
    let n: u32 = tail
        .trim()
        .parse()
        .map_err(|_| format!("{address} does not end in a port number"))?;
    // A display number is mapped before the range check, so that `:0` is
    // display 0 rather than a port nothing listens on.
    let n = match ports {
        Ports::Display if n < 100 => 5900 + n,
        _ => n,
    };
    match n == 0 || n > u16::MAX as u32 {
        true => Err(format!("{address} does not end in a port number")),
        false => Ok(n as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vnc(address: &str) -> Result<(String, u16), String> {
        split(address, 5900, Ports::Display)
    }

    fn rdp(address: &str) -> Result<(String, u16), String> {
        split(address, 3389, Ports::Plain)
    }

    #[test]
    fn a_name_or_a_v4_literal_splits_at_the_only_colon() {
        assert_eq!(vnc("10.0.0.5").unwrap(), ("10.0.0.5".into(), 5900));
        assert_eq!(vnc("10.0.0.5:1").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(vnc("10.0.0.5:0").unwrap(), ("10.0.0.5".into(), 5900));
        assert_eq!(vnc("10.0.0.5:99").unwrap(), ("10.0.0.5".into(), 5999));
        assert_eq!(vnc("10.0.0.5:100").unwrap(), ("10.0.0.5".into(), 100));
        assert_eq!(vnc(" desktop:5901 ").unwrap(), ("desktop".into(), 5901));
        assert_eq!(rdp("desktop").unwrap(), ("desktop".into(), 3389));
        assert_eq!(rdp("desktop:3390").unwrap(), ("desktop".into(), 3390));
    }

    /// The escape from the display rule, and it is VNC's alone.
    #[test]
    fn a_doubled_colon_means_the_port_was_meant() {
        assert_eq!(vnc("10.0.0.5::1").unwrap(), ("10.0.0.5".into(), 1));
        assert_eq!(vnc("10.0.0.5::5901").unwrap(), ("10.0.0.5".into(), 5901));
        assert_eq!(vnc("fdb7::1").unwrap(), ("fdb7".into(), 1));
        assert!(rdp("10.0.0.5::1").is_err());
    }

    #[test]
    fn a_literal_is_bracketed_and_may_carry_a_port() {
        assert_eq!(vnc("[::1]").unwrap(), ("::1".into(), 5900));
        assert_eq!(vnc("[::1]:5902").unwrap(), ("::1".into(), 5902));
        assert_eq!(vnc("[::1]:2").unwrap(), ("::1".into(), 5902));
        assert_eq!(vnc(" [fdb7:aebd::1] ").unwrap(), ("fdb7:aebd::1".into(), 5900));
        assert_eq!(rdp("[::1]").unwrap(), ("::1".into(), 3389));
        assert_eq!(rdp("[::1]:3390").unwrap(), ("::1".into(), 3390));
    }

    /// The whole of the reason the brackets are not optional.
    #[test]
    fn a_bare_literal_is_refused_in_words() {
        for address in ["2001:db8::1", "fdb7:aebd:7160:0:cfb9:d78:95a7:251e", "::1:1"] {
            assert!(vnc(address).unwrap_err().contains("bracketed"), "{address}");
            assert!(rdp(address).unwrap_err().contains("bracketed"), "{address}");
        }
        assert!(rdp("fdb7::1").unwrap_err().contains("bracketed"));
    }

    #[test]
    fn what_is_not_an_address_at_all() {
        assert!(vnc("host:nonsense").is_err());
        assert!(vnc("host:99999").is_err());
        assert!(vnc("[::1").is_err());
        assert!(vnc("[::1]junk").is_err());
        assert!(vnc("[::1]:").is_err());
        assert!(vnc("").is_err());
        assert!(vnc(":5901").is_err());
        assert!(rdp("desktop:0").is_err());
    }
}
