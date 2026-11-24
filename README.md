# Concord

[![CI](https://github.com/CSharpForge/concord/actions/workflows/ci.yml/badge.svg)](https://github.com/CSharpForge/concord/actions/workflows/ci.yml)

A desktop app to bulk-delete **your own** Discord messages — a safer, more reliable take on [Undiscord](https://github.com/victornpb/undiscord).

> ## ⚠️ Not supported by Discord
> Concord signs in as your user account and calls Discord's API the way its own app does.
> **Discord's rules do not allow automating user accounts** ("self-bots"), and doing so can get an
> account limited or permanently terminated. There is no official bulk-delete for users.
> Use Concord at your own risk, ideally on an account you can afford to lose.
>
> Your login token is a full key to your account. Concord stores it only in Windows Credential
> Manager. It is never logged, never stored in the database, and never sent to the web UI.

**Platform: Windows 10/11 only.** Linux and macOS are not supported.

## Status

**Pre-release.** Concord is **not affiliated with Discord**, and it has **not yet been tested against live
Discord**: it is built from Discord's official API docs and verified against mocked servers (100+ Rust tests,
UI tests and a mocked-backend UI run). Expect surprises, and try it on a throwaway account first.

What works today:

- Embedded Discord sign-in (QR code or password); the token goes to Windows Credential Manager.
- Browse servers, channels and DMs; pick what to clean.
- **Scan, review, delete:** a scan finds your own messages (optionally filtered by date range, words,
  attachments or links); you review the list and untick anything to keep; a delete removes exactly that list.
- **Protect-list:** servers or channels Concord will never scan or delete from.
- Rate-limit-aware requests, pause/resume/cancel, and recovery after a crash or quit.
- **Runs in the background:** a tray icon (open, pause all jobs, quit), closing the window hides it to the tray so
  jobs keep running, only one copy runs at a time, and an opt-in "start when I sign in to Windows" switch that
  opens Concord minimized. Jobs that were running stay paused after a restart until you resume them.

These Windows-only behaviours (tray, hide on close, start at login) use Tauri's official tray API and its
`autostart` / `single-instance` plugins. GitHub Actions builds and tests Concord on Windows for every push
(Rust and UI tests, plus a real Windows Credential Manager check). What no automated test covers yet, and a
person has not yet confirmed on Windows: the tray icon appearing, hide-on-close, start at login, and Discord
sign-in inside WebView2.

Planned: data-package import, archive before delete, scheduled cleanups, a Windows installer and release builds.

## Safety rules the engine follows

- Only messages **you** wrote are ever touched (author id must equal the signed-in user), and only message
  types Discord documents as deletable.
- Pinned messages are kept by default.
- **Scan** jobs never delete. A delete job removes exactly the list you reviewed (minus what you unticked),
  and needs typing `DELETE` (enforced in the backend too).
- Protected servers/channels are refused when jobs are created and again when they run.
- Requests follow Discord's rate-limit headers and `Retry-After`; a 429 slows the job down further.
- Jobs run one at a time. A job interrupted by quitting is left **paused**, never auto-resumed.
- To let you review, a scan keeps a short text preview of each found message in a local SQLite database on
  your computer. The previews are erased when the deletion finishes or you remove the scan. Your token is
  never stored there.

## Security and trust

- **Where your token goes:** [docs/TOKEN-HANDLING.md](docs/TOKEN-HANDLING.md). It is stored only in Windows
  Credential Manager, sent only to `discord.com`, and never given to the app's window. There is no telemetry
  and no logging. Tests fail the build if any of that stops being true.
- **Known limitations** (for example: the token is not wiped from memory, no installer or code signing yet) are
  listed there too.
- **Reporting a vulnerability:** see [SECURITY.md](SECURITY.md).

## Testing against real Discord

Use a **throwaway account** and a private test server, never your main account:

```bash
CONCORD_TEST_TOKEN=<test account token> CONCORD_TEST_CHANNEL=<channel id> \
  cargo test live_job -- --ignored --nocapture        # scan only
# add CONCORD_TEST_DELETE=yes to delete that account's own messages in that channel
```

## Stack

Tauri 2 (Rust) · Svelte 5 + TypeScript · SQLite (`tauri-plugin-sql`) · `reqwest` (rustls) · `keyring`

## Develop (Windows)

Prerequisites, from the official [Tauri guide](https://v2.tauri.app/start/prerequisites/#windows):

1. **Microsoft C++ Build Tools**, with the "Desktop development with C++" workload.
2. **WebView2**, already present on Windows 10 (1803+) and 11.
3. **Rust** via [rustup](https://rustup.rs), with the **MSVC** toolchain (`rustup default stable-msvc`).
4. **Node.js**.

Then:

```bash
npm install
npm run tauri dev      # run the app
npm test               # UI tests (Vitest)
cd src-tauri && cargo test                                 # Rust tests
cd src-tauri && cargo test -- --ignored keyring_roundtrip  # real Credential Manager test
```

The tests are plain Rust and TypeScript and should pass anywhere, but only Windows is a supported target.

## How sign-in works

There is no official way to obtain a user token. Concord opens discord.com in a separate window; a
script observes (never alters) the `Authorization` header on Discord's own API calls and hands the
token to the Rust backend, which validates it with `GET /users/@me` and saves it to the keychain,
then clears that window's browsing data. This depends on Discord's web client and may break when it
changes.

## License

[MIT](LICENSE). Concord is open source so you can read exactly what it does with your login token.
The MIT license covers the code; it does not make using Concord permitted by Discord (see the warning above).
