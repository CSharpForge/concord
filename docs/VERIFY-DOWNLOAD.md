# Verifying a Concord download

Every release has three things you can use to check that the installer you downloaded is the one this
repository built:

| File | What it is |
|---|---|
| `Concord_<version>_x64-setup.exe` | The Windows installer (installs for your user only, no administrator rights). |
| `SHA256SUMS.txt` | The SHA-256 checksum of the installer. |
| A signed **attestation** (stored by GitHub, not a file) | A signed record that the installer and `SHA256SUMS.txt` were built by this repository's [release workflow](../.github/workflows/release.yml) from a specific commit. |

Releases start as **drafts** that the maintainer reviews and publishes by hand, and are marked as
pre-releases until Concord is tested on live Discord.

## 1. Check the checksum

In PowerShell, in the folder you downloaded to:

```powershell
(Get-FileHash .\Concord_0.1.0_x64-setup.exe -Algorithm SHA256).Hash.ToLower()
```

(or `certutil -hashfile .\Concord_0.1.0_x64-setup.exe SHA256`). The result must equal the entry for that file
in `SHA256SUMS.txt`. This proves the file was not damaged or swapped on the way to you, **if** you trust
`SHA256SUMS.txt`, which is why step 2 exists.

## 2. Check where it was built

Install the [GitHub CLI](https://cli.github.com/), then:

```powershell
gh attestation verify .\Concord_0.1.0_x64-setup.exe --repo CSharpForge/concord `
  --signer-workflow CSharpForge/concord/.github/workflows/release.yml
gh attestation verify .\SHA256SUMS.txt --repo CSharpForge/concord `
  --signer-workflow CSharpForge/concord/.github/workflows/release.yml
```

`Verification succeeded` means GitHub holds a signed statement, made with a short-lived
[Sigstore](https://www.sigstore.dev/) certificate, that exactly these bytes were produced by that workflow in
this repository. The output names the commit it was built from, and you can read that commit's source.

## What this does and does not prove

- It **does** prove the download is byte-for-byte what the release workflow produced from a named commit, and
  that nobody altered it afterwards.
- It does **not** prove the source code is safe. Read it: [docs/TOKEN-HANDLING.md](TOKEN-HANDLING.md) explains
  the sensitive parts and links to the code.
- It does **not** make the build reproducible. Building from source yourself gives a working app but not an
  identical file, so you cannot compare your own build's checksum to the release.

## Windows SmartScreen

The installer is **not code-signed yet**, so Windows SmartScreen shows "Windows protected your PC / unknown
publisher". That warning is expected and is about the missing signature, not a verdict on the file. Do the two
checks above first; only then choose **More info, Run anyway**.

Signing is planned through the [SignPath Foundation](https://signpath.org/), which signs open-source projects
for free. This is **not guaranteed**: it has conditions, including that the project is already released and is
not malware or a potentially unwanted program, and a tool that goes against Discord's rules may not qualify.
Until then, releases stay unsigned and the checks above are the way to trust them.

## Building it yourself instead

You can skip downloads entirely: see "Develop (Windows)" in the [README](../README.md) and run
`npm run tauri build`.

## Something looks wrong

If a checksum or attestation does not verify, do not run the installer. Report it privately:
[SECURITY.md](../SECURITY.md).
