//! The listen host shared by the project schema and the command line.
//!
//! Both listeners bind `<host>:<port>`. The host comes either from `server.bindHost` in a
//! project or from `--bind` on the command line, so the syntax check and the address
//! formatting live here instead of being written twice — and a project rejected at load time
//! is rejected for exactly the same reasons as a command line rejected at parse time.
//!
//! Nothing here touches the network. A host name that does not resolve is a bind failure at
//! startup, not a project that will not load.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

/// The host both listeners bind when nothing says otherwise: every IPv4 interface.
pub const DEFAULT_BIND_HOST: &str = "0.0.0.0";

/// Longest host name a bind host may be, in bytes (RFC 1035).
const MAX_HOST_NAME_BYTES: usize = 253;

/// Longest single label in a host name, in bytes (RFC 1035).
const MAX_LABEL_BYTES: usize = 63;

/// Why a bind host was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BindHostError {
    /// Nothing but whitespace.
    Empty,
    /// Bracketed text that is not an IPv6 literal.
    NotAnIpv6Literal(String),
    /// Neither an IP literal nor a usable host name.
    NotAHost(String),
}

impl fmt::Display for BindHostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "must not be empty"),
            Self::NotAnIpv6Literal(value) => {
                write!(f, "'{value}' is bracketed but is not an IPv6 literal")
            }
            Self::NotAHost(value) => {
                write!(f, "'{value}' is neither an IP address nor a host name")
            }
        }
    }
}

impl std::error::Error for BindHostError {}

/// Check a bind host and return it in the form the listeners use.
///
/// An IPv4 or IPv6 literal is accepted in either bare or bracketed form and comes back
/// canonicalised; anything else has to look like a host name, which is resolved when the
/// listener binds.
///
/// # Errors
/// [`BindHostError`] when the text is empty, is bracketed but not an IPv6 literal, or cannot
/// be a host name.
pub fn parse_bind_host(value: &str) -> Result<String, BindHostError> {
    let text = value.trim();
    if text.is_empty() {
        return Err(BindHostError::Empty);
    }

    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return inner
            .parse::<Ipv6Addr>()
            .map(|address| address.to_string())
            .map_err(|_| BindHostError::NotAnIpv6Literal(text.to_owned()));
    }

    if let Ok(address) = text.parse::<IpAddr>() {
        return Ok(address.to_string());
    }

    if is_host_name(text) {
        Ok(text.to_owned())
    } else {
        Err(BindHostError::NotAHost(text.to_owned()))
    }
}

/// Format `host:port` the way `ToSocketAddrs` wants it, bracketing an IPv6 literal.
#[must_use]
pub fn join_host_port(host: &str, port: u16) -> String {
    if host.starts_with('[') || host.parse::<Ipv6Addr>().is_err() {
        format!("{host}:{port}")
    } else {
        format!("[{host}]:{port}")
    }
}

/// RFC 1123 host-name syntax, plus the underscore a few Windows hosts still carry.
fn is_host_name(text: &str) -> bool {
    if text.len() > MAX_HOST_NAME_BYTES {
        return false;
    }
    text.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= MAX_LABEL_BYTES
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_default_is_every_ipv4_interface() {
        assert_eq!(parse_bind_host(DEFAULT_BIND_HOST).unwrap(), "0.0.0.0");
    }

    #[test]
    fn ipv4_and_ipv6_literals_are_accepted() {
        assert_eq!(parse_bind_host("127.0.0.1").unwrap(), "127.0.0.1");
        assert_eq!(parse_bind_host("::1").unwrap(), "::1");
        assert_eq!(parse_bind_host("[::1]").unwrap(), "::1");
        assert_eq!(parse_bind_host("[::]").unwrap(), "::");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        assert_eq!(parse_bind_host("  127.0.0.1 ").unwrap(), "127.0.0.1");
    }

    #[test]
    fn host_names_are_accepted_and_left_alone() {
        for name in ["localhost", "test-cell-3.lab.example.com", "WIN_BENCH"] {
            assert_eq!(parse_bind_host(name).unwrap(), name, "for {name}");
        }
    }

    #[test]
    fn empty_text_is_rejected() {
        assert_eq!(parse_bind_host("   "), Err(BindHostError::Empty));
    }

    #[test]
    fn brackets_without_an_ipv6_literal_are_rejected() {
        assert!(matches!(
            parse_bind_host("[127.0.0.1]"),
            Err(BindHostError::NotAnIpv6Literal(_))
        ));
    }

    #[test]
    fn text_that_is_neither_an_address_nor_a_name_is_rejected() {
        for bad in [
            "127.0.0.1:5025",
            "0.0.0.0/0",
            "two words",
            "-leading-dash",
            "trailing.",
            "::1%eth0",
        ] {
            assert!(
                matches!(parse_bind_host(bad), Err(BindHostError::NotAHost(_))),
                "for {bad}"
            );
        }
    }

    #[test]
    fn an_over_long_name_is_rejected() {
        assert!(parse_bind_host(&"a".repeat(MAX_LABEL_BYTES)).is_ok());
        assert!(parse_bind_host(&"a".repeat(MAX_LABEL_BYTES + 1)).is_err());
    }

    #[test]
    fn only_an_ipv6_literal_is_bracketed_for_the_listener() {
        assert_eq!(join_host_port("0.0.0.0", 5025), "0.0.0.0:5025");
        assert_eq!(join_host_port("localhost", 5025), "localhost:5025");
        assert_eq!(join_host_port("::1", 5025), "[::1]:5025");
        assert_eq!(join_host_port("[::1]", 5025), "[::1]:5025");
    }
}
