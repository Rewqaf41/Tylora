# Tylora Settings

The React UI talks to a Tauri Rust bridge. The bridge uses the same-user `tylora-<uid>.sock` Unix socket; the daemon remains the only owner of window, workspace and layout state. It never invokes a shell for GUI actions. The bundled `tylora` binary is used only to start a stopped daemon.

## Development

Requires macOS, Rust stable and Bun. From this directory:

```sh
bun install
bun run desktop:dev
```

`desktop:dev` builds and copies the daemon sidecar before Tauri starts Vite. If the old FluxWM daemon is still running, stop it using the previous `fluxwm` executable first. macOS Accessibility and Input Monitoring permissions may need to be granted to the binary or launcher shown by the system prompt.

Focus & Mouse edits the daemon's `[focus]` TOML through a revision-checked IPC patch. Pointer focus is off by default. When enabled, Instant or Delayed mode uses the existing event tap and AX focus worker. The daemon checks the frontmost on-screen CG window before activating a managed window; system overlays and windows on inactive Spaces are skipped. `focus_only` is rejected because the current macOS activation path raises the selected window. The Diagnostics page exposes request, confirmation, cancellation and hit-test counters.

## Local `.app`

```sh
bun run desktop:build
open "../../target/release/bundle/macos/Tylora.app"
```

Copy the `.app` to `/Applications` for a stable location before enabling Launch at login. This locally built app is not notarized; macOS may require an explicit first launch from Finder. The CLI remains available as `../../target/release/tylora`.

## Checks

```sh
bun run typecheck
bun run lint
bun run test
bun run build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Current backend limits

Native Space bindings, hotkeys, BSP gaps, and application rules for initial floating or tiled mode are editable. The layout preview is computed by the daemon's Rust BSP engine. Title patterns, preferred workspace and display in rules, a global split setting, floating defaults, and editable logging level are unavailable. Recent errors are available in the daemon log, but the IPC diagnostics endpoint has no bounded recent-error history yet. Physical hotkeys, Accessibility prompts, mouse resize, tray behavior, and native Space movement require hands-on macOS checks.
