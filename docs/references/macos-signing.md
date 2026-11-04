# macOS signing and notarization

The `desktop-macos` job in `.github/workflows/release.yml` signs `RigSpark.app`
(and the bundled `llama-server`) with a Developer ID Application certificate,
enables the Hardened Runtime, notarizes and staples the app, then signs,
notarizes and staples the `.dmg`. Before upload it checks the result with
`codesign`, `stapler validate` and `spctl` (Gatekeeper).

Tagged releases (`v*`) fail when the secrets below are missing, so an unsigned
dmg is never published. Manual `workflow_dispatch` runs without them build an
ad-hoc signed app for testing only.

## Repository secrets

Signing (required):

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | Base64 of the exported **Developer ID Application** `.p12` (certificate + private key): `base64 -i DeveloperID.p12 \| pbcopy` |
| `APPLE_CERTIFICATE_PASSWORD` | Password chosen when exporting the `.p12` |

The signing identity is read from the certificate, so no identity secret is
needed.

Notarization (one of the two sets is required):

App Store Connect API key (recommended; create it under Users and Access →
Integrations → App Store Connect API, role Developer):

| Secret | Value |
| --- | --- |
| `APPLE_API_KEY_ID` | Key ID, for example `ABCD123456` |
| `APPLE_API_ISSUER` | Issuer ID (UUID shown above the keys list) |
| `APPLE_API_KEY_P8` | Base64 of `AuthKey_<KEY_ID>.p8`: `base64 -i AuthKey_ABCD123456.p8 \| pbcopy` |

Or Apple ID:

| Secret | Value |
| --- | --- |
| `APPLE_ID` | Apple Developer account email |
| `APPLE_APP_SPECIFIC_PASSWORD` | App-specific password from account.apple.com |
| `APPLE_TEAM_ID` | 10-character Team ID from developer.apple.com → Membership |

## Exporting the certificate

1. In developer.apple.com → Certificates, create a **Developer ID
   Application** certificate (a CSR from Keychain Access → Certificate
   Assistant).
2. Install it, then in Keychain Access → My Certificates, right-click
   "Developer ID Application: …" → Export as `.p12` with a password.
3. Add the secrets with `gh secret set APPLE_CERTIFICATE < <(base64 -i DeveloperID.p12)`
   and so on, or in Settings → Secrets and variables → Actions.

## Checking a downloaded dmg

```bash
spctl --assess --type open --context context:primary-signature -v RigSpark_aarch64.dmg
xcrun stapler validate RigSpark_aarch64.dmg
```

Local builds keep `signingIdentity: "-"` (ad-hoc) in `tauri.conf.json`; CI
overrides it only when a Developer ID certificate is present.
