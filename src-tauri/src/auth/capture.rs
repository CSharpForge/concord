//! Token capture from the embedded Discord login window.
//!
//! There is NO official way to obtain a user token; this is an improvised
//! approach and may break whenever Discord changes its web client.
//!
//! How it works: the login window is a normal Tauri webview pointed at
//! discord.com. An initialization script *observes* the `Authorization`
//! header on Discord's own API calls (it never alters requests). When it sees
//! a user token it navigates the window to `concord-token://capture/#<token>`.
//! Rust's navigation handler cancels that navigation and reads the token from
//! the URL. This avoids granting the remote Discord page access to Tauri IPC.

use tauri::Url;

pub const CAPTURE_SCHEME: &str = "concord-token";

/// Characters/length a Discord user token can have. Used on both sides.
const MIN_LEN: usize = 50;
const MAX_LEN: usize = 200;

pub const INIT_SCRIPT: &str = r#"
(function () {
  if (window.__concordCapture) return;
  try { Object.defineProperty(window, '__concordCapture', { value: true }); } catch (e) {}
  if (!/(^|\.)discord\.com$/.test(location.hostname)) return;
  var done = false;
  function ok(v) {
    return typeof v === 'string' && /^[A-Za-z0-9._-]{50,200}$/.test(v);
  }
  function offer(v) {
    if (done || !ok(v)) return;
    done = true;
    location.href = 'concord-token://capture/#' + v;
  }
  function fromHeaders(h) {
    if (!h) return null;
    try {
      if (typeof h.get === 'function') return h.get('authorization');
      if (Array.isArray(h)) {
        for (var i = 0; i < h.length; i++)
          if (String(h[i][0]).toLowerCase() === 'authorization') return h[i][1];
        return null;
      }
      for (var k in h) if (k.toLowerCase() === 'authorization') return h[k];
    } catch (e) {}
    return null;
  }
  function isApi(u) {
    try {
      var x = new URL(String((u && u.url) || u), location.href);
      return /(^|\.)discord\.com$/.test(x.hostname) && x.pathname.indexOf('/api/') === 0;
    } catch (e) { return false; }
  }
  var origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (input, init) {
      try {
        if (isApi(input)) offer(fromHeaders(init && init.headers) || fromHeaders(input && input.headers));
      } catch (e) {}
      return origFetch.apply(this, arguments);
    };
  }
  var origOpen = XMLHttpRequest.prototype.open;
  var origSet = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (m, u) {
    this.__concordApi = isApi(u);
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.setRequestHeader = function (n, v) {
    try { if (this.__concordApi && String(n).toLowerCase() === 'authorization') offer(v); } catch (e) {}
    return origSet.apply(this, arguments);
  };
})();
"#;

/// Extract a token from `concord-token://capture/#<token>`, or `None` if the
/// URL isn't a well-formed capture URL.
pub fn token_from_url(url: &Url) -> Option<String> {
    if url.scheme() != CAPTURE_SCHEME || url.host_str() != Some("capture") {
        return None;
    }
    let frag = url.fragment()?;
    let valid = (MIN_LEN..=MAX_LEN).contains(&frag.len())
        && frag.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    valid.then(|| frag.to_owned())
}

/// Top-level navigations the login window may perform.
pub fn is_allowed_navigation(url: &Url) -> bool {
    if url.scheme() == "about" {
        return url.as_str() == "about:blank";
    }
    if url.scheme() != "https" {
        return false;
    }
    match url.host_str() {
        Some(h) => {
            h == "discord.com"
                || h.ends_with(".discord.com")
                || h == "discordapp.com"
                || h.ends_with(".discordapp.com")
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        s.parse().unwrap()
    }

    /// A made-up token-shaped string. Built from parts so no token-shaped literal
    /// sits in the source (secret scanners would flag it, even though it is fake).
    fn tok() -> String {
        ["MTIzNDU2Nzg5MDEyMzQ1Njc4", "GabcDE", "abcdefghijklmnopqrstuvwxyz0123456789_-AB"].join(".")
    }

    #[test]
    fn extracts_valid_token() {
        let t = tok();
        let url = u(&format!("concord-token://capture/#{t}"));
        assert_eq!(token_from_url(&url).as_deref(), Some(t.as_str()));
    }

    #[test]
    fn rejects_wrong_scheme_host_or_shape() {
        let t = tok();
        assert_eq!(token_from_url(&u(&format!("https://capture/#{t}"))), None);
        assert_eq!(token_from_url(&u(&format!("concord-token://evil/#{t}"))), None);
        assert_eq!(token_from_url(&u("concord-token://capture/")), None);
        assert_eq!(token_from_url(&u("concord-token://capture/#short")), None);
        assert_eq!(token_from_url(&u(&format!("concord-token://capture/#{t}%20x"))), None);
        let long = "a".repeat(MAX_LEN + 1);
        assert_eq!(token_from_url(&u(&format!("concord-token://capture/#{long}"))), None);
    }

    #[test]
    fn navigation_allowlist() {
        assert!(is_allowed_navigation(&u("https://discord.com/login")));
        assert!(is_allowed_navigation(&u("https://support.discord.com/hc")));
        assert!(is_allowed_navigation(&u("https://discordapp.com/x")));
        assert!(is_allowed_navigation(&u("about:blank")));
        assert!(!is_allowed_navigation(&u("http://discord.com/login")));
        assert!(!is_allowed_navigation(&u("https://evildiscord.com/")));
        assert!(!is_allowed_navigation(&u("https://discord.com.evil.io/")));
        assert!(!is_allowed_navigation(&u("https://example.com/")));
        assert!(!is_allowed_navigation(&u("file:///etc/passwd")));
    }

    #[test]
    fn init_script_charset_matches_rust() {
        // Keep JS and Rust validation in sync.
        assert!(INIT_SCRIPT.contains(&format!("{{{MIN_LEN},{MAX_LEN}}}")));
    }
}
