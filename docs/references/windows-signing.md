# Windows code signing (SignPath)

Windows releases are signed with free code signing provided by
[SignPath.io](https://about.signpath.io/), with a certificate by
[SignPath Foundation](https://signpath.org/). The application is under review,
so until it is approved `RigSpark_x64-setup.exe` ships unsigned and the release
notes say so.

## How the release signs the installer

Signing is off until the repository variable `SIGNPATH_PROJECT_SLUG` is set.
With it set, `.github/workflows/release.yml` does this:

1. `desktop-windows` builds and silent-installs the NSIS installer as before,
   then uploads the unsigned `RigSpark_x64-setup.exe` as a workflow artifact
   instead of attaching it to the release.
2. `sign-windows` submits that artifact to SignPath with origin verification,
   waits for the signed file, and checks it with `Get-AuthenticodeSignature`:
   the status must be `Valid`, the signer must be SignPath Foundation, and the
   signature must be timestamped.
3. It installs the signed installer silently and checks the bundled
   `llama-server.exe` runs, the same check the unsigned build gets.
4. Authenticode changes the file, so it signs the in-app update again
   (`cargo tauri signer sign`) over the signed bytes and rewrites the `.sha256`.
   Only then does it attach the installer, `.sig` and `.sha256` to the draft
   release.
5. `publish` waits for `sign-windows`. If SignPath is configured and signing
   fails or is skipped, nothing is published. The release notes say the
   installer is signed only when `sign-windows` succeeded.

Without `SIGNPATH_PROJECT_SLUG`, `sign-windows` is skipped and the unsigned
installer is attached directly, exactly as before.

### Why this workflow uses two actions

The rest of the release runs without an action runtime. SignPath Foundation
signs only with origin verification, and for GitHub that accepts nothing but a
workflow artifact uploaded with `actions/upload-artifact`, submitted with
`signpath/github-action-submit-signing-request`. Those two, pinned to full
commit SHAs, are the only actions allowed, and only in `release.yml`
(`RELEASE_SIGNING_ACTIONS` in `crates/rigspark-cli/src/retirement.rs`; the
policy tests fail on any other action or revision). To update them, change the
SHA in both places after reviewing the new release.

## Setting it up once SignPath approves the project

In SignPath:

1. Link the project to the **GitHub.com** trusted build system.
2. Add an artifact configuration (make it the default, or set
   `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`):

   ```xml
   <?xml version="1.0" encoding="utf-8"?>
   <artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
     <zip-file>
       <pe-file path="RigSpark_x64-setup.exe">
         <authenticode-sign />
       </pe-file>
     </zip-file>
   </artifact-configuration>
   ```

3. Use the `release-signing` policy (or set `SIGNPATH_SIGNING_POLICY_SLUG`) and
   create an API token for a user who is a submitter on it.

In GitHub (Settings → Secrets and variables → Actions):

| Kind | Name | Value |
| --- | --- | --- |
| Secret | `SIGNPATH_API_TOKEN` | The submitter's API token |
| Variable | `SIGNPATH_ORGANIZATION_ID` | SignPath organization ID |
| Variable | `SIGNPATH_PROJECT_SLUG` | SignPath project slug; setting this turns signing on |
| Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | Optional, defaults to `release-signing` |
| Variable | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | Optional, defaults to the project's default |

Then remove "it isn't code-signed yet" from the README's Get started steps.

## What is still unsigned

Only the installer is signed. `rigspark-desktop.exe` and `llama-server.exe`
inside it, and the `rigspark-x86_64-pc-windows-msvc.tar.gz` CLI archive, are
not. SmartScreen checks the downloaded installer, so the warning goes away
once it builds reputation; Smart App Control also checks each program as it
starts, so signing the inner binaries (signing them before NSIS packs them) is
the next step.

## Checking a downloaded installer

```powershell
Get-AuthenticodeSignature .\RigSpark_x64-setup.exe | Format-List Status, SignerCertificate
(Get-FileHash .\RigSpark_x64-setup.exe).Hash.ToLower()   # compare with RigSpark_x64-setup.exe.sha256
```
