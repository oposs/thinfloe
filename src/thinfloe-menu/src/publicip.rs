//! The public IP address: the settings the Network tab keeps (`public_ip`
//! and `public_ip_url` in `[network]`) and the rule for a lookup address.
//! The lookup is off unless the person switches it on.

/// The lookup address used while the field is empty.
pub const DEFAULT_URL: &str = "https://api.ipify.org";
/// Most bytes of a lookup address.
const URL_MAX: usize = 256;
const SCHEME: &str = "https://";

/// A lookup address the menu may ask: `https://` only, as for the add-on
/// sources fetched without asking, since the answer is fetched in the
/// background and shown as a fact. A non-empty host, no whitespace or
/// control character (the address becomes a program argument), at most
/// `URL_MAX` bytes.
pub fn url_ok(url: &str) -> bool {
    let Some(scheme) = url.get(..SCHEME.len()) else {
        return false;
    };
    let rest = &url[SCHEME.len()..];
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    scheme.eq_ignore_ascii_case(SCHEME)
        && !host.is_empty()
        && url.len() <= URL_MAX
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// The address a lookup asks: the one set, or `DEFAULT_URL` when none is.
/// It is not checked here; the lookup checks it before each request.
pub fn effective_url(cfg_url: &str) -> &str {
    if cfg_url.is_empty() { DEFAULT_URL } else { cfg_url }
}

/// What the Network tab shows and saves: the switch and the lookup address
/// as `config.toml` holds them (`""` for the default).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PublicIpPrefs {
    pub on: bool,
    pub url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_lookup_addresses_are_accepted() {
        for good in ["https://api.ipify.org", "HTTPS://ip.example.net/plain"] {
            assert!(url_ok(good), "{good}");
        }
        let long = format!("https://{}", "a".repeat(257 - "https://".len()));
        assert_eq!(long.len(), 257);
        for bad in [
            "http://api.ipify.org",
            "https://",
            "file:///etc/passwd",
            "https://a b",
            "https://a\n",
            "-K/etc/x",
            long.as_str(),
        ] {
            assert!(!url_ok(bad), "{bad:?}");
        }
        assert!(url_ok(&long[..256]));
    }

    #[test]
    fn an_empty_lookup_address_is_the_default() {
        assert_eq!(effective_url(""), DEFAULT_URL);
        assert_eq!(DEFAULT_URL, "https://api.ipify.org");
        assert_eq!(effective_url("https://ip.example.net"), "https://ip.example.net");
    }
}
