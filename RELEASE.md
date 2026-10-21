# Releasing Tylora

This document covers packaging, signing, notarization, updates and the
release checklist. User-facing install and uninstall steps are in
[README.md](README.md#get-started).

## What ships

`Tylora.dmg` contains `Tylora.app` and a link to `/Applications`.

```text
Tylora.app/Contents/
  Info.plist            CFBundleIdentifier com.tylora.settings, LSMinimumSystemVersion 13.0
  MacOS/tylora-gui      the Settings app and menu bar icon (Tauri)
  MacOS/tylora          the daemon and CLI (Tauri sidecar, same version)
  Resources/icon.icns   generated from apps/gui/src-tauri/icons/icon.svg
```

The app starts the daemon from `Contents/MacOS/tylora` next to its own
executable; it never looks at `PATH` or a development path. The Objective-C
move bridge (`crates/tylora-macos/src/spaces/window_move_bridge.m`) is
compiled into the daemon binary, so there is no other nested executable.

Only `aarch64-apple-darwin` (Apple Silicon) is built and tested. An Intel or
Universal build needs `rustup target add x86_64-apple-darwin`, a sidecar named
`tylora-universal-apple-darwin` (`lipo -create` of both daemons) and
`tauri build --target universal-apple-darwin`; it has not been tested and must
not be published until it has been run on an Intel Mac.

## Versioning

One version for everything, kept equal by hand and checked by CI on a tag:
`apps/gui/package.json`, `apps/gui/src-tauri/tauri.conf.json`,
the Cargo packages (`tylora-daemon` is what `tylora --version` prints) and
the tag `vX.Y.Z`. `scripts/verify-bundle.sh` fails when the bundled daemon
and the app disagree.

## Build locally

```sh
cd apps/gui
bun install
bun run desktop:build        # target/release/bundle/macos/Tylora.app and dmg/Tylora_<v>_aarch64.dmg
bun run verify:bundle        # layout, versions, architecture, signature, dmg checksum
```

Without an identity the bundle is **ad-hoc signed** (`signingIdentity: "-"`)
with the hardened runtime and the empty
[`Entitlements.plist`](apps/gui/src-tauri/Entitlements.plist). The tag workflow
publishes this DMG as a GitHub Release asset, alongside `SHA256SUMS.txt`, but
macOS may block its first launch because it is not notarized. After attempting
to open Tylora, the user can choose **Open Anyway** in System Settings →
Privacy & Security. The release notes and README state this limitation.

The workflow also uploads the DMG as an Actions artifact for build inspection.
Users should download the Release asset, which has a stable filename:
`Tylora_aarch64.dmg`. The versioned DMG remains attached to the same release.
The README button points to GitHub's `releases/latest/download/` URL; it works
once a non-draft, non-prerelease release has been published.

## Developer ID signing and notarization

Status: **BLOCKED** until an Apple Developer Program membership is available.
Nothing in this repository contains certificates, keys or passwords; do not
commit any.

1. Create a *Developer ID Application* certificate, export it with its key as
   `.p12`.
2. Create an App Store Connect API key (Users and Access → Integrations, role
   Developer) and download `AuthKey_<KEY_ID>.p8`.
3. Add repository secrets:

   | Secret | Value |
   | --- | --- |
   | `APPLE_CERTIFICATE` | `openssl base64 -A -in DeveloperID.p12` |
   | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
   | `APPLE_SIGNING_IDENTITY` | `Developer ID Application: Name (TEAMID)` |
   | `APPLE_API_ISSUER` | the issuer ID shown above the keys table |
   | `APPLE_API_KEY` | the key ID |
   | `APPLE_API_KEY_P8` | the contents of the `.p8` file |

Tauri signs every executable in the bundle (app and sidecar) with the
hardened runtime and the entitlements, notarizes through `notarytool` and
staples the ticket when these variables are set. Locally the same works with
the variables exported in the shell, or with the certificate in the login
keychain and `APPLE_SIGNING_IDENTITY` set.

The hardened runtime needs no exceptions: Accessibility and the Event Tap are
TCC permissions, SkyLight is an Apple-signed system framework resolved with
`dlsym`, and Mission Control is driven through Accessibility, not Apple Events.
Tylora is not sandboxed (a window manager must reach other apps' windows), so
it cannot go to the Mac App Store.

Verify a signed build:

```sh
sh apps/gui/scripts/verify-bundle.sh --release
# codesign --verify --deep --strict, Developer ID authority,
# stapler validate (app and dmg), spctl --assess (app and dmg)
```

## Permissions across builds

macOS keys Accessibility (and Input Monitoring) to the code signature. An
ad-hoc signature changes with every build, so after each local rebuild the
entry in System Settings → Privacy & Security → Accessibility must be switched
off and on again (or removed and re-added). A Developer ID build keeps its
grant across updates signed by the same team. The daemon started by
Tylora.app runs with Tylora.app as its responsible process, so one grant to
"Tylora" covers it. Started from a terminal, the terminal app needs the grant
instead.

## Updates

There is no automatic updater. The Tauri updater stays off until releases
are signed and an update signing key exists; turning it on later needs
`tauri-plugin-updater`, a `TAURI_SIGNING_PRIVATE_KEY` secret and a published
`latest.json`.

Manual update with the DMG:

1. Choose **Stop Tylora** in the menu bar (windows hidden or parked by Tylora
   are restored), then **Quit App**.
2. Drag the new Tylora.app onto `/Applications`, replacing the old one.
3. Open Tylora and start the daemon.

Compatibility rules for every release:

- **Configuration**: `~/.config/tylora/config.toml` carries `version = 1`.
  New keys must have defaults so older files keep loading; a breaking change
  needs a new version number and a migration that, like the FluxWM import,
  never deletes or overwrites the user's file.
- **State** (`~/.local/state/tylora/*.json`) must tolerate unknown and missing
  fields.
- **IPC**: every JSON reply carries `"version": 1`; the app rejects any other
  protocol version. `status --json` reports `daemon_version`, and the app
  shows a warning with a restart action when the running daemon differs from
  its own version (for example a daemon from the previous release still
  running after an update).
- **Daemon handover**: the daemon is not replaced while running. Restart
  stops the old daemon, waits until it releases the socket (up to 20 s while
  it restores windows), then starts the bundled one. Two daemons never run
  at once: the socket is bound exclusively and `start` attaches to a live one.
- **Login item**: `SMAppService.mainApp` is keyed to the bundle identifier,
  so replacing the app keeps it.

## Release checklist

1. Bump the version in the four places above; update README and this file.
2. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`; in `apps/gui`: `bun run typecheck`, `lint`, `test`,
   `build`.
3. `bun run desktop:build` and `bun run verify:bundle`.
4. Manual checks on a disposable user account or clean Mac: [Technical guide](TECHNICAL.md#manual-macos-validation),
   first-run setup, Accessibility grant, launch at login on
   and off, update over the previous version, uninstall.
5. Tag `vX.Y.Z` and push. The workflow publishes a release with a direct DMG
   download. Without Apple credentials it is ad-hoc signed and unnotarized;
   with credentials it must pass the Developer ID, notarization and Gatekeeper
   gate before publication.
6. Install the published DMG on a second Mac. For an unnotarized build, check
   that the release notes' **Open Anyway** instructions match the actual
   macOS prompt.
