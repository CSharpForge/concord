Concord {{TAG}}: Windows 10/11 installer (pre-release).

**Not supported by Discord.** Concord signs in as your user account, which Discord's rules do not allow. Accounts that automate can be limited or terminated. Read the [README](https://github.com/CSharpForge/concord#readme) before using it, and try it on a throwaway account first. It has not been tested against live Discord yet.

## Install

Download `{{INSTALLER}}` and run it. It installs for **your user only** and needs no administrator rights. Uninstall from Settings, then Apps.

**Windows will warn about an "unknown publisher".** This installer is not code-signed yet, so Windows SmartScreen shows "Windows protected your PC". That is expected for now. Do not skip the checks below: they are how you know the file is the one built from this repository.

## Verify your download

1. **Checksum.** In PowerShell, in the folder you downloaded to:
   ```powershell
   (Get-FileHash .\{{INSTALLER}} -Algorithm SHA256).Hash.ToLower()
   ```
   It must equal the hash for this file in `SHA256SUMS.txt`.
2. **Where it was built.** With the [GitHub CLI](https://cli.github.com/):
   ```powershell
   gh attestation verify .\{{INSTALLER}} --repo CSharpForge/concord
   ```
   This checks a signed record that the file was produced by this repository's release workflow, from the tagged commit.

More detail, and what these checks do and do not prove: [docs/VERIFY-DOWNLOAD.md](https://github.com/CSharpForge/concord/blob/main/docs/VERIFY-DOWNLOAD.md).
