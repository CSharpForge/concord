# Security policy

Concord handles a secret (your Discord login token) and deletes data, so security reports are welcome.
How it protects the token: [docs/TOKEN-HANDLING.md](docs/TOKEN-HANDLING.md), including its
[honest limitations](docs/TOKEN-HANDLING.md#honest-limitations).

## Supported versions

Concord is **pre-release**. Only the latest commit on `main` is supported; there are no stable releases yet.

## Reporting a vulnerability

**Please do not open a public issue with details, and never post a real Discord token anywhere.**

Use GitHub's private reporting: on the repository's **Security** tab choose **Report a vulnerability**
(or go to <https://github.com/CSharpForge/concord/security/advisories/new>).

Please include what you found, the steps to reproduce it (using a throwaway account, see below), the version or
commit, and what you think the impact is.

What to expect: this is a small open-source project run on a best-effort basis. There is no guaranteed response
time and no bug bounty. Reports are read, and a confirmed problem gets a fix and, if you like, credit. Please
give a reasonable time to fix before sharing details publicly.

## What counts as a vulnerability

Examples we want to hear about:

- Any way to read, leak, log or send the token somewhere other than `discord.com`.
- Any way the web page (UI) or a Discord page can obtain the token or call Concord's commands it should not.
- Anything that makes Concord delete or touch messages that are **not** the signed-in user's, ignore the
  protect-list, or skip the review/confirmation steps.
- A bypass of the checks that keep a delete job limited to its reviewed list.
- Stored data readable by other Windows users, or a flaw in how previews are erased.

## Not vulnerabilities

- Discord limiting or banning an account because a user token is used by a tool. This is a known, documented
  risk (see the [README](README.md)).
- Problems in Discord, Windows, WebView2, Tauri or other dependencies themselves: please report those upstream.
- Issues that need an attacker who already runs code as the same Windows user (for example reading process
  memory); these are listed as known limitations.
- Running a modified or unofficial build.

## Testing safely

- Use a **throwaway Discord account** and your own private server. Never test with someone else's account or
  with your main account.
- Do not run automated tests against Discord's servers beyond what is needed to show the problem.
