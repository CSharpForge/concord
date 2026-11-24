//! Trust guarantees that `docs/TOKEN-HANDLING.md` promises, enforced as tests.
//!
//! These read the source files, so a change that breaks a promise fails CI
//! instead of silently shipping. They only look at production code: each file's
//! `#[cfg(test)]` section (and everything after it) and comment lines are ignored.

use std::fs;
use std::path::{Path, PathBuf};

/// Hosts the app's Rust code may contact: Discord's API and login page, nothing else.
const ALLOWED_HOSTS: &[&str] = &["discord.com"];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// (file, 1-based line, text) for every production, non-comment line.
fn production_lines() -> Vec<(String, usize, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() >= 8, "expected to find the app's source files, found {}", files.len());
    let mut lines = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).unwrap();
        let prod = text.split("#[cfg(test)]").next().unwrap();
        let name = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        for (i, line) in prod.lines().enumerate() {
            if !line.trim_start().starts_with("//") {
                lines.push((name.clone(), i + 1, line.to_owned()));
            }
        }
    }
    lines
}

fn hosts_in(line: &str) -> Vec<(bool, String)> {
    let mut found = Vec::new();
    for scheme in ["https://", "http://"] {
        let mut rest = line;
        while let Some(pos) = rest.find(scheme) {
            let after = &rest[pos + scheme.len()..];
            let host: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').collect();
            // A bare scheme ("https://" with no host) is text being matched, not a destination.
            if !host.is_empty() {
                found.push((scheme == "http://", host));
            }
            rest = after;
        }
    }
    found
}

#[test]
fn the_app_backend_only_contacts_discord_over_https() {
    let mut bad = Vec::new();
    let mut seen_discord = false;
    for (file, n, line) in production_lines() {
        for (plain_http, host) in hosts_in(&line) {
            if ALLOWED_HOSTS.contains(&host.as_str()) && !plain_http {
                seen_discord = true;
            } else {
                bad.push(format!("{file}:{n}: {}{host}", if plain_http { "http://" } else { "https://" }));
            }
        }
    }
    assert!(seen_discord, "the scan should find the discord.com API base");
    assert!(bad.is_empty(), "unexpected network destinations in app code:\n{}", bad.join("\n"));
}

#[test]
fn app_code_has_no_logging_or_printing() {
    let banned = ["println!", "eprintln!", "print!(", "eprint!(", "dbg!(", "log::", "tracing::", "env_logger"];
    let mut bad = Vec::new();
    for (file, n, line) in production_lines() {
        for b in banned {
            if line.contains(b) {
                bad.push(format!("{file}:{n}: {}", line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "logging could leak the token; remove:\n{}", bad.join("\n"));
}

#[test]
fn the_web_view_is_granted_only_core_permissions() {
    // No plugin (sql, opener, autostart, ...) is reachable from the web page; the
    // app's own commands are allowed by Tauri by default.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities/default.json");
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let perms: Vec<&str> = json["permissions"].as_array().unwrap().iter().map(|p| p.as_str().unwrap()).collect();
    assert_eq!(perms, ["core:default"]);
}

#[test]
fn no_command_deals_in_tokens() {
    // The token's only readers are the keychain store and the HTTP client. A command
    // that could hand it to the web page would show "token" in its name or signature.
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs")).unwrap();
    let handler = lib.split("generate_handler![").nth(1).unwrap().split(']').next().unwrap();
    let names: Vec<&str> = handler.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    assert!(names.len() >= 15, "found the command list");
    for name in &names {
        assert!(!name.to_lowercase().contains("token"), "command {name} must not deal in tokens");
    }

    // Every `#[tauri::command]` signature (attribute line up to the opening brace).
    let lines = production_lines();
    let mut checked = 0;
    for (i, (file, n, line)) in lines.iter().enumerate() {
        if !line.contains("#[tauri::command]") {
            continue;
        }
        let mut sig = String::new();
        for (f2, _, l2) in lines.iter().skip(i + 1) {
            if f2 != file {
                break;
            }
            sig.push_str(l2);
            if l2.contains('{') {
                break;
            }
        }
        assert!(!sig.to_lowercase().contains("token"), "{file}:{n}: a command signature mentions a token: {sig}");
        checked += 1;
    }
    assert_eq!(checked, names.len(), "every registered command was checked");
}

#[test]
fn the_content_security_policy_is_strict() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
    let conf: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let csp = conf["app"]["security"]["csp"].as_object().expect("a CSP must be set (it is null/absent)");
    let get = |k: &str| csp.get(k).and_then(|v| v.as_str()).unwrap_or_else(|| panic!("CSP is missing {k}"));

    // Scripts and styles: this app's own files only. Tauri adds hashes/nonces for its bundled code.
    assert_eq!(get("script-src"), "'self'");
    assert_eq!(get("style-src"), "'self'");
    assert_eq!(get("default-src"), "'self'");
    // Locked down: no plugins, no <base>, no forms, never framed.
    assert_eq!(get("object-src"), "'none'");
    assert_eq!(get("base-uri"), "'none'");
    assert_eq!(get("form-action"), "'none'");
    assert_eq!(get("frame-ancestors"), "'none'");
    // Images: Discord's CDN for server icons, nothing else remote.
    let img: Vec<&str> = get("img-src").split_whitespace().collect();
    assert_eq!(img, ["'self'", "data:", "https://cdn.discordapp.com"]);
    // The page may only talk to Tauri's own IPC; no web hosts at all.
    let connect: Vec<&str> = get("connect-src").split_whitespace().collect();
    assert_eq!(connect, ["'self'", "ipc:", "http://ipc.localhost"]);

    // SvelteKit's hidden screen-reader announcer ships one inline style attribute. It is allowed
    // by hash only (never 'unsafe-inline'); the UI test checks the hash matches SvelteKit's output.
    let attr: Vec<&str> = get("style-src-attr").split_whitespace().collect();
    assert_eq!(attr.len(), 2, "style-src-attr must be exactly 'unsafe-hashes' plus one hash");
    assert_eq!(attr[0], "'unsafe-hashes'");
    assert!(attr[1].starts_with("'sha256-") && attr[1].ends_with("='"), "style-src-attr hash looks wrong: {}", attr[1]);

    // Nothing may weaken it.
    for (k, v) in csp {
        let v = v.as_str().unwrap();
        for bad in ["'unsafe-inline'", "'unsafe-eval'", "'wasm-unsafe-eval'", "*", "blob:", "http:", "https:", "ws:", "wss:"] {
            assert!(!v.split_whitespace().any(|t| t == bad), "{k} contains {bad}");
        }
    }
}
