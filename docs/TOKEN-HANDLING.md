# Where your Discord token goes

Concord signs in as you, so it needs your Discord **token**: a secret that works like a password for your
account. This page says exactly where Concord gets it, where it keeps it, where it sends it, and what it
cannot protect against. Everything here can be checked in the source; the files are linked.

> **Not supported by Discord.** Using a user token in a third-party tool breaks Discord's rules and can get an
> account limited or terminated, whatever Concord does with the token. See the [README](../README.md).

## In short

- The token is stored **only in Windows Credential Manager**, never in a file or in Concord's database.
- Concord sends it **only to `discord.com`** (Discord's API), in the `Authorization` header.
- The app's window (the web page) **never receives the token**. No command returns it.
- The app code **has no logging**, so the token cannot end up in a log.
- There is **no telemetry, analytics, auto-update or crash reporting**.
- Tests fail the build if any of the above stops being true (see [Enforced by tests](#enforced-by-tests)).

## The journey of the token

1. **Sign-in window.** Concord opens `https://discord.com/login` in a separate window
   ([`auth/mod.rs`](../src-tauri/src/auth/mod.rs)). That window may only navigate to `discord.com` and
   `discordapp.com` pages over `https` ([`is_allowed_navigation`](../src-tauri/src/auth/capture.rs)). You sign
   in on Discord's own page, by QR code or password; **Concord never sees your password**.
2. **Capture.** A small script runs only on Discord pages inside that window
   ([`capture.rs`](../src-tauri/src/auth/capture.rs)). It watches the `Authorization` header of Discord's own
   requests to `discord.com/api`; it does not change any request. When it sees a token-shaped value it
   navigates the window to `concord-token://capture/#<token>`. Concord's Rust code cancels that navigation and
   reads the token from the address. The Discord page is never given access to Concord's commands.
   *There is no official way to obtain a user token, so this depends on how Discord's web client works and may
   break when it changes.*
3. **Check and store.** Concord calls `GET https://discord.com/api/v9/users/@me` with the token. Only if
   Discord accepts it, the token is saved to Windows Credential Manager
   ([`token_store.rs`](../src-tauri/src/auth/token_store.rs), via the [`keyring`](https://crates.io/crates/keyring)
   crate). The sign-in window's browsing data is then cleared and the window closed.
4. **Use.** When Concord needs Discord (listing your servers, scanning, deleting) the Rust backend reads the
   token from Credential Manager and sends it to `https://discord.com/api/v9` as the `Authorization` header
   ([`discord/mod.rs`](../src-tauri/src/discord/mod.rs)). That is the only place it is sent. While a job runs,
   the token is held in the process's memory as an ordinary string.
5. **Sign out.** "Sign out" deletes the Credential Manager entry. It does **not** tell Discord to invalidate
   the token (see [Revoking a token](#revoking-a-token)).

## What the app window can and cannot see

The window is a web page (Svelte) talking to Rust through a fixed list of commands
([`lib.rs`](../src-tauri/src/lib.rs)). None of them returns the token: they return things like your username,
your server and channel names, and job counters. The window is also granted **no plugin permissions**
([`capabilities/default.json`](../src-tauri/capabilities/default.json) grants only `core:default`), and its code
makes no network requests of its own except loading server icons from Discord's image CDN
(`cdn.discordapp.com`; images only, no token).

## What is stored on disk

| What | Where | Contains the token? |
|---|---|---|
| Login token | Windows Credential Manager | Yes (that is its job) |
| Job history, counters, protect-list | `concord.db` (SQLite) in the app's data folder, typically `%APPDATA%\app.concord.desktop` | **No** |
| Short previews of messages a scan found (at most 140 characters each) | the same `concord.db` | No, but it is your own message text |

The previews exist so you can review what will be deleted. They are erased when a deletion finishes or when you
remove the scan. **`concord.db` is not encrypted**: anyone who can read your Windows user's files can read any
previews that are still stored.

## Who the app talks to

From the app's own code: **`discord.com` only** (the login page and the API). Nothing else: no analytics, no
update server, no crash reports. Concord uses Microsoft's WebView2 component to show windows; WebView2 is a
separate Microsoft product with its own behaviour that Concord does not control.

## Enforced by tests

These run on every push, on Windows, in [CI](../.github/workflows/ci.yml):

- [`src-tauri/tests/trust.rs`](../src-tauri/tests/trust.rs): the app's Rust code contains no web address except
  `https://discord.com`; contains no logging or printing; no command deals in tokens; and the window is granted
  only `core:default`.
- [`src/lib/trust.test.ts`](../src/lib/trust.test.ts): the UI references only `cdn.discordapp.com`, never calls
  `fetch` or opens a socket itself, uses no Tauri plugin APIs, and never touches `localStorage` or cookies.

## Honest limitations

- **The token is not wiped from memory.** While Concord runs, the token sits in ordinary process memory.
  Anything that can read the process (running as your Windows user, or a debugger) could read it.
- **No Content Security Policy is set yet** on the app window. It is planned. Concord's window only shows
  Concord's own pages, but a CSP would add a second layer.
- **The sign-in window shares the app's WebView2 profile.** Concord clears it after a *successful* sign-in. If
  you close the window mid-way, Discord's cookies can stay in that profile until the next sign-in or until you
  delete the app's data.
- **Anyone who can run code as you can use the token.** Credential Manager protects the token from other
  Windows users, not from programs running as you.
- **No installer or code signing yet.** Today you build from source. Check what you run.
- **Windows behaviour is only partly verified.** CI builds and tests on Windows, but the tray icon, start at
  login, and the sign-in capture inside WebView2 have not been confirmed by a person yet.

## Revoking a token

Signing out of Concord removes the stored copy only. To make the token itself useless, **change your Discord
password**: this logs every device out and invalidates old tokens (it is also what
[Undiscord's README](https://github.com/victornpb/undiscord#-do-not-share-your-auth-token-) advises). Then, to
remove everything Concord kept, delete its Credential Manager entry (look for one mentioning
`app.concord.desktop`) and the app's data folder.

If you think a token leaked, change your password first, then tell us privately: see
[SECURITY.md](../SECURITY.md).

## Verify it yourself

- Read the files linked above; the token code is a few hundred lines.
- Run `cargo test --test trust` (in `src-tauri`) and `npm test`.
- Search the Rust code for every web address: `grep -rn "https\?://" src-tauri/src` (ignore comments and the
  `#[cfg(test)]` sections at the bottom of files).
- Build from source (see the README) rather than trusting a download.
