# Windows distribution

mightty uses a per-user MSIX package. Windows installs and removes the package
without administrator rights. The package registers `mightty.exe` and the
`mightty:` protocol.

The protocol accepts these links:

- `mightty://activate`
- `mightty://quick`
- `mightty://profile?id=PROFILE_ID`
- `mightty://quick?profile=PROFILE_ID`

Settings remain under `%APPDATA%\mightty\settings.json`. Workspaces remain
under `%APPDATA%\mightty\workspaces`. Package upgrades do not own these paths.

## Build a signed package

Install the Windows SDK, Rust MSVC target, Zig, and `mise`. Import a code-signing
certificate into the current user's personal certificate store.

The certificate subject must match the package publisher. The certificate must
contain a private key and permit code signing.

```powershell
.\tools\package-windows.ps1 `
  -Publisher "CN=Your Publisher" `
  -CertificateThumbprint "0123456789ABCDEF0123456789ABCDEF01234567"
```

The script performs these actions:

1. It downloads and verifies the pinned terminal fonts when required.
2. It builds the locked release target.
3. It creates the MSIX package with `MakeAppx`.
4. It signs and timestamps the package with `SignTool`.
5. It verifies the signature and writes `mightty.appinstaller`.

The updater manifest checks for a package update every four hours. Windows can
also update the package in the background.

## Publish a release

The `release-windows.yml` workflow publishes tags that match the Cargo version.
Configure these repository secrets:

- `WINDOWS_PACKAGE_PUBLISHER`
- `WINDOWS_SIGNING_PFX_BASE64`
- `WINDOWS_SIGNING_PFX_PASSWORD`

`WINDOWS_PACKAGE_PUBLISHER` must equal the certificate subject. Encode the PFX
file as one base64 string before you store it in the repository secret.

## Test an upgrade

Use a clean local or virtual Windows user. The test stops if mightty user data
already exists.

```powershell
.\tools\test-windows-package-upgrade.ps1 `
  -PreviousPackage .\mightty-0.1.0-x64.msix `
  -CurrentPackage .\mightty-0.2.0-x64.msix
```

The test installs both signed versions. It verifies that settings and workspace
files keep the same hashes.
