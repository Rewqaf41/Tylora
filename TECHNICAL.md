# Tylora: technical guide

An experimental Rust tiling window manager for macOS. A background daemon tiles ordinary windows on every display with three layouts (BSP, Dwindle and Scrolling), global hotkeys and modifier mouse drag; workspaces bind to native macOS Spaces; application rules, scratchpads and Accessibility-driven animations are built in. The Tylora app (Tauri + React) is the settings window and menu bar icon; it edits the daemon's TOML configuration over a local socket.

Tylora is experimental software. Native Space discovery, switching and window moves use **private** macOS interfaces (SkyLight) that Apple does not document and may change in any macOS update; Tylora checks them at runtime and fails closed, but behaviour on macOS releases other than the tested one is unknown. It never disables SIP, injects code into the Dock or WindowServer, or needs administrator rights.

## Requirements

- Apple Silicon Mac. Only `arm64` builds exist; Intel and Universal builds are not tested.
- macOS 13 or later is the bundle's minimum (`LSMinimumSystemVersion`); development and all measurements in this guide used **macOS 26.3**. Earlier releases have not been tested.
- Accessibility permission (granted by you in System Settings). Some systems also need Input Monitoring for global hotkeys.
- For workspace hotkeys: Desktops created in Mission Control and bound in Settings → Workspaces.

## Install

1. Open `Tylora_<version>_aarch64.dmg` and drag **Tylora** onto **Applications**.
2. Open Tylora from Applications. The current GitHub DMG is ad-hoc signed and not notarized. After trying to open it, macOS may require **System Settings → Privacy & Security → Open Anyway**. See [RELEASE.md](RELEASE.md) for signing details.
3. The first-run setup opens.

### First run

The setup walks through **Welcome → Accessibility → Hotkeys → Workspaces → Start**; every step has Back, optional steps can be skipped, and it can be repeated from General Settings → Permissions → First-run setup.

- **Accessibility**: *Request access* shows the macOS prompt; switch **Tylora** on in System Settings → Privacy & Security → Accessibility. The page shows the state macOS reports, nothing else. The daemon started from the app uses the same grant.
- **Start**: the daemon starts only when you click *Start daemon*. *Start the daemon automatically* and *Open Tylora at login* stay off unless you turn them on.

### Permissions

macOS ties Accessibility to the app's code signature. After installing a build with a different signature (a local build, then a release, or each ad-hoc rebuild) the old entry no longer applies: switch Tylora off and on in the Accessibility list, or remove it with `−` and add it again. If windows tile but hotkeys do nothing, also allow Tylora in Privacy & Security → Input Monitoring and restart the daemon. When you run `tylora start` from a terminal, macOS asks for the terminal app instead, because the terminal is then the responsible process.

### The app and the daemon

- The daemon (`Tylora.app/Contents/MacOS/tylora serve`) runs on its own. Closing the window or choosing **Quit App** in the menu bar never stops it; **Stop Tylora** (menu bar or Dashboard) does, after restoring windows Tylora hid or parked.
- Starting attaches to a running daemon instead of starting a second one; the socket is bound exclusively.
- **Restart daemon** stops the running daemon, waits until it has released its socket, and starts the bundled one. The Dashboard warns when the running daemon's version differs from the app's (for example after an update).
- **Open Tylora at login** registers the app as a macOS login item (`SMAppService`, System Settings → General → Login Items) and removes it when switched off. Login LaunchAgents written by earlier builds (`com.tylora.settings`, `com.fluxwm.settings`) are converted on first launch and removed. No privileged helper is installed.

### Command line

The CLI is the daemon binary inside the app:

```sh
/Applications/Tylora.app/Contents/MacOS/tylora status
```

To call it as `tylora`, link it into a directory you already have on `PATH`, without sudo (Tylora never edits `PATH` or shell files):

```sh
mkdir -p ~/.local/bin && ln -sf /Applications/Tylora.app/Contents/MacOS/tylora ~/.local/bin/tylora
```

### Configuration and migration

Configuration: `~/.config/tylora/config.toml` (or `$XDG_CONFIG_HOME/tylora/config.toml`); the app and daemon resolve it through the same code. State: `~/.local/state/tylora/`. Logs: `$TMPDIR/tylora-<uid>.log` (open with `open $TMPDIR`); socket: `$TMPDIR/tylora-<uid>.sock`.

The former command `fluxwm` is now `tylora`. On first access, Tylora copies `~/.config/fluxwm/config.toml` into `~/.config/tylora/config.toml` byte for byte (hotkeys, Spaces, rules, scratchpads, layouts and animations included) when the Tylora file does not exist yet, then validates the copy and logs any problem; an invalid copy is kept for you to fix and defaults apply meanwhile. The FluxWM file is never changed or deleted; an existing Tylora file is never overwritten, and the two are never merged. Workspace state and the recovery journal are imported the same way. **Stop the older FluxWM daemon before starting Tylora**: Tylora refuses to start alongside it so that two coordinators cannot manage the same windows.

### Troubleshooting

| Symptom | What to do |
| --- | --- |
| Start fails with an Accessibility message | Grant Accessibility to Tylora (see Permissions); after a rebuild, toggle the entry off and on. |
| Windows tile, hotkeys do nothing | Allow Input Monitoring for Tylora, then Restart daemon. Check Settings → Hotkeys for conflicts. |
| Option+digit does nothing | Bind the workspace to a native Space (Settings → Workspaces, or `tylora space diagnostics --json`). |
| Windows left minimized, parked or off screen after a crash | `tylora recover-windows` (works while the daemon is stopped), or Dashboard → Recover windows; restore anything left from the Dock. |
| "Daemon offline" in the app | Start it from the Dashboard; read `$TMPDIR/tylora-<uid>.log`. |
| Dashboard reports a version mismatch | Dashboard → Restart daemon. |
| "An older FluxWM daemon is still running" | Stop it with the old `fluxwm stop`. |

### Uninstall

1. Menu bar → **Stop Tylora** (restores hidden and parked windows), then **Quit App**.
2. If enabled, turn off *Open Tylora at login* in General Settings first (or remove Tylora in System Settings → General → Login Items).
3. Delete `/Applications/Tylora.app` and, if you made it, `~/.local/bin/tylora`.
4. Optional, removes your settings: `rm -rf ~/.config/tylora ~/.local/state/tylora`.
5. Remove Tylora from Privacy & Security → Accessibility (and Input Monitoring) with `−`.

Desktops that Tylora added for scratchpad parking stay in Mission Control; remove them there if you no longer want them.

## Build from source

Requires Rust stable, Xcode Command Line Tools and Bun. `cd apps/gui && bun install && bun run desktop:build` produces `target/release/bundle/macos/Tylora.app` and `target/release/bundle/dmg/Tylora_<version>_aarch64.dmg` (ad-hoc signed); `bun run verify:bundle` checks them. Packaging, signing, notarization and updates are described in [RELEASE.md](RELEASE.md); app development in [apps/gui/README.md](apps/gui/README.md).

For the daemon alone:

```sh
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build -p tylora-daemon
```

Run from a logged-in graphical session. On the first `start`, macOS should prompt for Accessibility access. Approve the application **named in the prompt** in System Settings → Privacy & Security → Accessibility, restart that application, then run `start` again. When launching from an integrated terminal, macOS may name its host (for example, Visual Studio Code) even if `tylora` is already enabled:

```sh
target/debug/tylora list-windows --ax  # read-only CG and AX diagnostics
target/debug/tylora start              # all connected displays
target/debug/tylora status --json
target/debug/tylora windows
target/debug/tylora layout
target/debug/tylora focus left
target/debug/tylora move right
target/debug/tylora resize left --amount 50
target/debug/tylora floating toggle
target/debug/tylora zoom toggle
target/debug/tylora display list --json
target/debug/tylora display focus next
target/debug/tylora display move-window next
target/debug/tylora window move-to-display next --json
target/debug/tylora window move-to-display previous --json
target/debug/tylora window move-to-display 12345 --json  # use an ID from display list
target/debug/tylora hotkeys list
target/debug/tylora space capabilities --json
target/debug/tylora space list --json
target/debug/tylora space current --json
target/debug/tylora space windows 2 --json
target/debug/tylora space diagnostics --json
target/debug/tylora space focus Code --json  # after binding Code to a native Space ID
target/debug/tylora space focus-id 4 --json  # diagnostic: use an ID from `space list`
target/debug/tylora space move-diagnostics --json
target/debug/tylora space move-window Browser --json  # moves the focused window after native verification
target/debug/tylora space move-window Browser --window-id 12345 --json  # explicit CGWindowID for diagnostics
target/debug/tylora recover-windows
target/debug/tylora reconcile
target/debug/tylora stop
```

`start` changes positions and sizes of eligible ordinary windows on all connected displays. `--display ID` validates a visible display but does not restrict management to it. Use `stop` to end management promptly and restore windows hidden by Tylora; it does not restore previous positions. If a crash leaves windows minimized, restart Tylora or run `recover-windows` even while the daemon is stopped. Recovery identifies windows conservatively; if a window cannot be matched, restore it from Dock. Logs are at `$TMPDIR/tylora-<uid>.log`; set `RUST_LOG=debug` before `start` for latency diagnostics. Global input may additionally require Input Monitoring permission. After rebuilding, macOS may require permissions to be renewed for the executable.

`status` reports `degraded=N` when N tiled windows have not reached their requested geometry and `stale=N` for discarded AX results. Tylora checks AX geometry after a short settling delay and retries mismatches with bounded backoff (250 ms, 750 ms, 5 s, 15 s, then once per minute). After three mismatches it learns a window's observed minimum size for future BSP calculations. If the combined minimum sizes cannot fit, some windows may still overlap. `windows` shows each window's target, actual geometry, and retry count.

## Input, workspaces and configuration

Default tiling hotkeys: Option+H/J/K/L focuses geometrically, Option+Shift+H/J/K/L swaps tiles, Option+F toggles floating, Option+Shift+F toggles Layout Zoom, and Option+R reconciles. Option+1…9 is bound to _logical workspace IDs_. Each logical workspace needs an explicit `native_space_id` from `tylora space list --json`; unresolved bindings are shown as unavailable and left with macOS. The experimental switcher synthesizes Dock swipe gestures in an instant form (no sliding animation; a jump over several Desktops never stops on the ones in between) and verifies the resulting native Space ID. Option+Shift+1…9 moves the focused window to the configured native Space, follows it there after WindowServer confirms the move, and focuses the reacquired window through Accessibility. The digit maps to a logical workspace, not the current Desktop order. Neither operation uses Mission Control's numeric keyboard shortcuts, disables SIP, or minimizes windows. Mouse drag and resize code was not changed.

Layout Zoom keeps the BSP tree and split ratios intact. It expands one tiled window per Native Space to the display work area, or leaves the configured outer gap when `[zoom] respect_outer_gaps = true`. Other windows remain present behind it. macOS may raise another window when that app is explicitly activated; Layout Zoom does not make the Space exclusively fullscreen. Floating windows return `unsupported` for this command. `status --json` reports each zoom target and whether AX confirmed its geometry.

Option+Shift+Tab moves the focused window to the next physical display in left-to-right order. The command targets that display's currently active native Space through the existing SkyLight move bridge and returns a verified native transfer result with source and target IDs; target BSP insertion follows AX reconciliation. The hotkey is available only while at least two displays and the native mover are available. `[window_movement]` configures `focus_after_move` (default `false`), `wrap_around` (default `true`), and `preserve_floating_position` (default `true`). This top-level table is used because `[[displays]]` already names the display assignment array in the TOML schema. Floating windows retain their mode and relative position, clamped to the target work area. Explicit native transfer does not use AX offscreen placement or minimize windows.

A workspace may own one Space per display: `native_space_ids = [4665]` next to `native_space_id = 7` binds Browser to Desktop 1 of both displays. Its focus key opens the instance with the most recent focus; pressing it again from one instance moves to the next. Its move key uses the instance on the window's current display, else the most recently focused one. When macOS recreates a bound Space with a new ID (display reconnect, sleep, lock), the daemon rebinds the workspace to the unbound Space now at the same display and Desktop position, remembered in `~/.local/state/tylora/space-slots.json`, and logs the change; it never guesses without a remembered slot or between several candidates.

Copy [examples/config.toml](examples/config.toml) to `~/.config/tylora/config.toml`, add each workspace's `native_space_id`, and run `tylora space diagnostics --json` to confirm the bindings resolve. For example, `name = "Code"` with `native_space_id = 4` binds Code to macOS Space ID 4 **only on a machine where ID 4 exists**. Reordering Desktops does not change this binding; deleting that Space makes it unresolved. The TOML schema supports `focus_named_workspace`, `move_window_to_named_workspace`, `focus_next_workspace`, and `focus_previous_workspace` commands with arbitrary supported modifier/key combinations. The daemon hot reloads valid TOML edits within about one second, reinstalls active hotkeys after any mouse drag finishes, and rejects conflicts while preserving the last valid configuration. Other layout settings may require restarting the daemon. Digit chords use physical macOS keycodes, so Option+Shift+1 is recognized by key position even if the keyboard layout produces a symbol.

The old virtual workspace switching path is disabled; recovery for its old minimized-window journal remains. The coordinator keeps BSP layouts for configured workspaces and selects the corresponding tree after observing a native Space change. It does not apply geometry to inactive Spaces. A confirmed move removes the window from the source tree and records it as pending in the target tree until AX rediscovers it there. A window may get a new managed AX token after the move; CGWindowID connects the two observations. The move hotkey follows and focuses the window; the diagnostic `space move-window` CLI command only moves it. Native Space behavior remains experimental, particularly during app closure or display changes while the target Space is inactive.

Space discovery and CGWindowID membership use runtime-resolved private SkyLight symbols. The move bridge resolves a local Mach-O symbol and checks the Objective-C class and initializer ABI at runtime; this was tested on macOS 26.3 Apple Silicon with SIP enabled. The asynchronous native call is followed by a three-second membership check. A timeout means the result is indeterminate; reconciliation checks the actual state and Tylora never claims success from submission alone. The switching gesture uses public Core Graphics. Both paths may change across macOS releases. Another process without Accessibility permission may report a different capability result, even for the same binary. Cross-display switching currently requires moving the pointer to the target display.

The previous FluxWM build may have left a recovery journal for windows it minimized. Tylora copies that journal on first recovery access and retains the original; `tylora recover-windows` still matches windows conservatively. Restore unmatched windows manually from Dock. Legacy active workspace state from `~/.local/state/fluxwm/workspaces.json` is imported, but is not an authority for native Space IDs and must not be interpreted as a Desktop mapping.

Core Graphics display callbacks trigger a debounced AppKit work-area refresh, with a 30-second safety refresh after missed events or sleep. On display disconnect, the old layout is suspended and managed windows are requested to move into the remaining work area. Apps may refuse AX geometry; macOS relocation is then the fallback. Automatic restoration to the original display is not implemented. `display list` includes a public ColorSync UUID, full bounds, work area, scale and primary flag.

`disconnect_policy = "recreate_desktops"` (default `"move_to_primary"`, the macOS behavior) keeps an external display's Desktops off the remaining display. The daemon remembers each connected external display's Desktops by UUID (from WindowServer, which drops a display at once). Two seconds after that display disconnects, it moves the windows from the Desktops macOS appended to the remaining display: a workspace's windows go to the same workspace's Space there, parked scratchpads go to that display's parking Desktop, and other windows follow their application rule (bundle ID rules without a title) or land on the active Desktop. It then removes those Desktops through Mission Control. When the same display returns, it recreates as many Desktops, rebinds the workspaces and the parking Desktop to them by position, and returns the windows that still sit where they were moved. The built-in display is never handled this way, so clamshell mode keeps the Mac's Desktops. State is kept in `~/.local/state/tylora/display-desktops.json`; a failed attempt is retried after the display next changes.

## Layouts

Every native Space keeps its own layout state: the BSP or Dwindle tree with its split ratios, or the Scrolling strip with its column order, widths and viewport. Switching Spaces restores exactly that state, including for Spaces no workspace is bound to.

- **BSP** — the original layout: new windows split the most recent region.
- **Dwindle** — Hyprland-style: a new window splits the _focused_ window; the first split follows `[dwindle] initial_orientation` (`vertical` = side by side) and later splits alternate. `split_ratio` (0.1–0.9) applies to new splits only. Removing a window collapses its parent split.
- **Scrolling** — niri-style: each window is a column in a horizontal strip that may be wider than the display. Columns keep their own width (a fraction of the usable width, default 65%). Keyboard focus scrolls the viewport only as far as needed (`viewport_alignment = "nearest"`) or keeps the column centred (`"center"`). New windows open right of the focused column and are revealed.

```sh
tylora layout list [--json]          # kinds and where they are active
tylora layout current [--json]       # per display: Space, workspace, layout, Scrolling viewport and columns
tylora layout set bsp|dwindle|scrolling [--json]
tylora layout cycle [--json]
tylora focus next|previous           # layout order; strip order in Scrolling
tylora scroll left|right             # move the viewport without changing focus
tylora center                        # centre the focused column
tylora column cycle-width            # 33% → 50% → 65% → 100% (configurable presets)
tylora column width 0.1              # grow (or shrink with a negative value) by a fraction
```

`layout set`/`cycle` apply to the current workspace (the focused window's display, else the pointer's) and are not saved. Persistent choices live in the configuration: `default_layout` and an optional `layout = "scrolling"` per `[[workspaces]]` entry; the Settings app edits both. Default bindings are added only when you have not bound the same action or the chord yourself: Option+Control+H/L scrolls, Option+Control+C centres, Option+Control+W cycles column width, Option+Control+T cycles the layout. Option+H/L and Option+Shift+H/L focus and move between neighbouring columns in Scrolling.

```toml
default_layout = "bsp"

[dwindle]
initial_orientation = "vertical"   # or "horizontal"
split_ratio = 0.5

[scrolling]
default_column_width = 0.65
min_column_width = 0.2
max_column_width = 1.0
column_width_presets = [0.3333, 0.5, 0.65, 1.0]
viewport_alignment = "nearest"     # or "center"
focus_auto_scroll = true
mouse_resize = true                # modifier + right drag changes column width
scroll_sensitivity = 0.25          # fraction of the viewport per scroll command
```

Mouse resize changes split ratios in BSP/Dwindle and the focused column width in Scrolling (following columns shift; the viewport stays). Layout Zoom works in every layout and leaves ratios, widths and the viewport untouched.

**Scrolling and macOS limits.** WindowServer never lets a window leave every display: `AXPosition` reports success but silently keeps at least 40 points of the window visible horizontally (about 32 points at the bottom edge). This was measured with `cargo run -p tylora-macos --example offscreen_probe -- <pid>` on macOS 26.3. Tylora therefore parks columns that are outside the viewport in a bottom corner of their own display, leaving a 40 × 32 point sliver visible; partly visible columns are shown at their real position. A park position whose off-display part would touch another display is rejected; if neither corner is safe the strip falls back to a _bounded_ mode that fits every column on screen (`layout current` reports `mode`). Windows are never minimized or moved to other Spaces to emulate scrolling. Focus-follows-mouse ignores parked slivers, and passive focus (a click or Cmd-Tab) scrolls only when the newly focused column was parked, so focus cannot start a scroll loop.

Windows left mostly off screen are journaled in `$TMPDIR/tylora-<uid>.offscreen.json` (mode 0600). `tylora stop` brings them all back on screen; after a crash the next `tylora start` or an offline `tylora recover-windows` restores them, matching by process, frame and title. With the daemon running, `recover-windows` leaves columns the Scrolling layout parks on purpose — reach them by navigation or switch the workspace to BSP.

## Animations

Windows glide to their new frames when the layout changes: a window opens or closes, a layout command (swap, move, resize, layout switch), a gap change, the end of a mouse drag, Layout Zoom, Scrolling navigation, and scratchpads appearing or leaving. Space switches, display changes and mouse drags themselves stay instant, and so does any geometry without such a cause (reconciliation). The only way to move other apps' windows is Accessibility (`AXPosition`/`AXSize`), so there is no fading or scaling; how smooth it looks depends on the app (moves run near 60 fps, heavy resizes such as Electron's lower). See [architecture.md](architecture.md#animation-engine) for measurements and limits.

```toml
[animations]
enabled = true
backend = "auto"            # auto (Accessibility when granted) | accessibility | instant; native_experimental is unsupported
duration_ms = 180           # 0–500; 0 is instant
easing = "ease_out_cubic"   # linear | ease_out_cubic | ease_in_out_cubic | ease_out_quart | ease_out_expo
preferred_fps = 60          # 15–240: how often frames are written, not a promise of what apps show
respect_reduce_motion = true  # macOS Reduce Motion makes every move instant (the setting is only read)
scratchpad_slide = "down"   # down (from above) | up | left | right

[animations.events]
window_open = true
window_close = true
layout_change = true
layout_zoom = true
scrolling = true
scratchpad = true
```

Changes apply without a restart (also from Settings → Animations, whose preview animates demo rectangles only). Apps too slow to keep up (two frame writes over 40 ms, or one over 120 ms) move instantly for ten minutes. `tylora animations status` shows the settings, the backend in use, Reduce Motion, instant reasons and counters. On `stop`, running animations end at their targets.

## Application rules

`[[rules]]` place windows of an application in a workspace or on a display and set their mode. Matching is deterministic: rules are visited by `priority` (higher first, then file order); each action (move, mode) comes from the first matching rule that sets it, and `stop = true` (default) ends the walk. Workspace and display always come from the same rule, so a window never gets two conflicting moves.

```toml
[[rules]]
id = "vscode-code"
name = "VS Code to Code"
bundle_id = "com.microsoft.VSCode"
workspace = "code"          # workspace name or ID; uses its bound native Space
mode = "tiled"              # unchanged | tiled | floating | unmanaged
priority = 100
apply = "on_create"         # on_create | on_first_seen | on_title_change | manual

[[rules]]
id = "calculator-floating"
bundle_id = "com.apple.calculator"
mode = "floating"

[[rules]]
id = "quick-notes"
bundle_id = "com.apple.Notes"
title = "^Quick"            # optional; title_match = exact | contains | regex
title_match = "regex"
display = 2                 # with workspace: that workspace's Space on display 2, else an error
mode = "floating"
```

- **Moves** use the native Space bound to the workspace (`native_space_id`/`native_space_ids`) and the existing verified mover; Tylora updates its state only after WindowServer confirms the move. Nothing is minimized, placed off screen or chosen by Desktop order.
- **When rules run**: `on_create` for windows opened while the daemon runs (windows open at startup are excluded), `on_first_seen` also for those, `on_title_change` again when the title changes, `manual` only through `rules apply`. A configuration reload never re-applies rules to open windows. A rule with a title condition waits briefly (a few observations, at most two seconds) for the title.
- **Manual override**: once a rule moved a window, any later move to another Space (by you, a hotkey or Mission Control) marks an override and rules leave the window alone until it closes or you apply the rule again. Each window gets at most two automatic attempts.
- Dialogs, sheets and other non-standard windows are never tiled or moved. Legacy `[[window_rules]]` keep working as mode-only rules applied on first sight.

```sh
tylora rules list [--json]
tylora rules validate [--json]                 # config file + bound Spaces/displays
tylora rules test --window <CGWindowID> [--json] # preview: matches, winner, reasons, plan; moves nothing
tylora rules apply --window <CGWindowID> [--json] # apply now, clearing a manual override
tylora rules reload [--json]
tylora rules diagnostics [--json]              # timings, moves, modes, errors, overrides, recent actions
tylora apps list [--json]                      # running apps with real bundle IDs
```

CGWindowIDs are shown by `tylora windows --json`. The Settings app's Window Rules page edits rules (create, edit, delete, enable, drag to reorder), picks applications from the running ones, and has a Rule Preview with an explicitly confirmed **Apply rule now**.

## Scratchpads

A scratchpad is a named window kept one hotkey away (like Hyprland special workspaces). It never takes part in BSP, Dwindle or Scrolling: assigning a tiled window floats it and the rest reflows; unassigning tiles it again.

```toml
[[scratchpads]]
id = "term"                 # 1-32 letters, digits, '-' or '_'
name = "Terminal"
hotkey = "option+grave"     # toggles; checked against other hotkeys
scope = "global"            # global: shown on the current Space | local: only on its own Space
width = 0.7                 # fractions of the work area (0.1-1)
height = 0.65
alignment = "center"        # center | top | bottom | left | right | top_left | ... | bottom_right
offset_x = 0
offset_y = 0
target = "pointer"          # pointer | focused; `display = ID` pins it to one display
remember_position = true
remember_size = true
hide = "park"               # park (default) | minimize (explicit opt-in)
bundle_id = "com.apple.Terminal"   # optional; filters the window list in Settings

[[rules]]
id = "terminal-scratchpad"
bundle_id = "com.apple.Terminal"
scratchpad = "term"         # cannot be combined with workspace, display or mode
```

- **Hide** moves the window with the verified native mover to a *parking* Desktop on its display. Tylora adds that Desktop through Mission Control the first time it needs one. There is no off-screen placement, and no window gets "all Spaces" behaviour (SIP stays enabled; nothing is injected into the Dock or WindowServer). `hide = "minimize"` uses AXMinimized instead, and only when you choose it.
- **Show** moves the window to the active Space of the target display, sets the frame, raises and focuses it, then checks that it is on screen. Hiding returns focus to the previously focused window. Focus follows mouse is paused while a show or hide runs.
- **Assignment**: manual assignment (CLI or Settings) always wins. A rule only fills an empty scratchpad, with the first matching new window, once.
- **Recovery**: `tylora stop`, `tylora recover-windows` (also when the daemon is not running) and `tylora scratchpad recover` move parked windows back to their home Space, or to the active Space of their display. State is kept in `~/.local/state/tylora/scratchpads.json`. Closed windows become `unavailable`; removed parking Desktops are recreated on the next hide.

```sh
tylora scratchpad list [--json]
tylora scratchpad assign <id> [--window <CGWindowID>] [--force] [--json]  # default: focused window
tylora scratchpad unassign <id> [--json]
tylora scratchpad toggle|show|hide <id> [--json]
tylora scratchpad recover [--json]
tylora scratchpad capabilities [--json]
```

The Settings app's Scratchpads page creates, edits and deletes scratchpads, records the hotkey, shows state, size and position with a geometry preview, and has **Assign Focused Window** (the window focused before Settings), toggle/show/hide and the backend capabilities.

## Manual macOS validation

1. In a graphical session, run `list-windows --ax`; confirm CG geometry and separate AX geometry are plausible. Verify the CLI warns when Accessibility access is absent.
2. Grant Accessibility access, run `start`, and inspect `status`, `windows` and `layout`. Confirm only normal windows move into each display's visible work area. Check that the log reports global hotkeys active; if not, check Input Monitoring and restart the terminal.
3. Open two or more windows in an observed app; verify automatic BSP insertion. Close one; verify the other expands. Run `reconcile` and confirm no destroyed window remains in `windows` or `layout`.
4. Minimize and restore a window; verify it leaves and rejoins tiling. Move or resize a window manually; confirm there is no repeated resize loop in the log.
5. Test Option+H/J/K/L and Option+Shift+H/J/K/L with two tiles. Option+left drag one tile onto another; Option+right drag to change a split ratio. Toggle floating with Option+F, then drag and resize it. Release Option mid-drag and confirm cancellation. Secure Event Input must not be intercepted.
6. Repeat with a second display, a Retina display, negative display coordinates and a window spanning displays. Check `display focus next` and `display move-window next` with a disposable window. Confirm spanning windows and fullscreen/system panels are not tiled.
7. Disconnect/reconnect a monitor; change resolution, arrangement and primary display; sleep/wake. Confirm no managed window becomes inaccessible. Inspect `display list`, `windows` and `status` after each change.
8. Run `space capabilities --json`, `space list --json`, and `space current --json`; compare the IDs with Mission Control. Put the desired IDs in `config.toml`, check `space diagnostics --json`, then run `space focus Code` and `space focus Browser`. With the daemon running, physically press Option+1/2 and verify the native Desktop changes without minimizing windows. For a disposable ordinary window, physically press Option+Shift+1/2: Tylora should follow it to the target Desktop and focus that same window. Check its destination in Mission Control and `space windows N --json`, and verify the source tile reflows. Reorder those Desktops in Mission Control and repeat; the names must still target the same Space IDs. Edit a hotkey chord in TOML and check `hotkeys list --json` without restarting. Synthetic key input from automation may bypass the Event Tap, so test on the keyboard.
9. Test an app that denies or delays AX operations. Confirm other apps remain responsive, `status` works, and failures are logged.
10. Stop with `tylora stop`; verify any windows from the old recovery journal return and the process and socket disappear. Restart the daemon and recheck native Space discovery.
11. Layouts: open four disposable TextEdit documents on one Space and run `layout set scrolling`. `layout current` must show `visible`/`partial`/`parked` columns and `windows` must show observed frames equal to desired ones (parked frames at the bottom-right or bottom-left corner). Use Option+H/L, Option+Control+H/L, Option+Control+C and Option+Control+W and recheck. Click a parked sliver or Cmd-Tab to its app: the strip scrolls to it. Switch to `dwindle`, focus one window and open another document: it must split the focused window. Option+right-drag in each layout. Switch Spaces and back: order, ratios and widths return. Finally `tylora stop` with parked columns and confirm every window is back on screen (`list-windows --ax`).
12. Rules: add a rule for TextEdit with `workspace` set to an inactive bound workspace, open a new document and check `rules diagnostics` and `space windows N`. Move the window elsewhere with a hotkey: it must stay (manual override). `rules apply --window` brings it back. Edit the rule in Settings and confirm `rules list` changes without restarting.

Automated tests and CLI smoke checks do not replace the modifier-drag and display hotplug checks above.

The ignored native integration test moves one explicitly chosen disposable window and returns it to its source Space. Run it only from a graphical terminal with Accessibility permission, substituting IDs from `space windows N --json` and `space list --json`:

```sh
TYLORA_TEST_WINDOW_ID=12345 TYLORA_TEST_SOURCE_SPACE_ID=7 TYLORA_TEST_TARGET_SPACE_ID=4 \
  cargo test -p tylora-macos --test native_move -- --ignored --nocapture
```

The test was run on a blank TextEdit window with CGWindowID 163539, source Space 7 and target Space 4 on macOS 26.3 Apple Silicon with SIP enabled. It passed and restored the window to Space 7. The IDs are machine-specific and should not be reused elsewhere.

See [architecture.md](architecture.md) for state ownership, reconciliation rules and known limits.
