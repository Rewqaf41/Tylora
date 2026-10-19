use crate::focus_follows_mouse::FocusFollowsMouseManager;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use tracing::{debug, error, info, warn};
use tylora_config::{compile_hotkey, Config, FocusMode, Hotkey, WindowRule, WindowRuleMode};
use tylora_core::layout::{FocusOrigin, LayoutEnvironment, LayoutKind};
use tylora_core::LayoutPolicy;
use tylora_core::{
    commands::{CommandError, CommandResult, DisplayTransferStatus, StateCommand},
    navigation::neighbor,
    registry::{ManagedWindowId, ProcessInstance, WindowSnapshot},
    workspace::{WorkspaceEngine, WorkspaceVisibilityBackend},
    CommandEffect, Coordinator, CoordinatorEvent, DisplayId,
};
use tylora_layout::{LayoutSettings, TilingLayout};
use tylora_macos::spaces::{NativeSpace, SpaceObserver};
use tylora_macos::{
    accessibility_trusted, prompt_for_accessibility, suppress_pointer_focus, work_areas,
    DisplayEvents, HotkeyEvent, HotkeyManager, ManagedAdapter, ManagedCommand, ManagedEvent,
    MouseButton, MouseInput, MousePhase, MouseTarget, WorkArea, VISIBILITY_ROLLBACK_FAILED,
};

extern "C" {
    fn geteuid() -> u32;
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("tylora-{}.sock", unsafe { geteuid() }))
}
fn log_path() -> PathBuf {
    std::env::temp_dir().join(format!("tylora-{}.log", unsafe { geteuid() }))
}
fn legacy_daemon_running() -> bool {
    let path = std::env::temp_dir().join(format!("fluxwm-{}.sock", unsafe { geteuid() }));
    fs::symlink_metadata(&path)
        .is_ok_and(|meta| meta.file_type().is_socket() && meta.uid() == unsafe { geteuid() })
        && UnixStream::connect(path).is_ok()
}
fn migrate_legacy_file(new_path: &Path, old_path: &Path) {
    if new_path.exists() || !old_path.is_file() {
        return;
    }
    let result = (|| -> std::io::Result<()> {
        let content = fs::read(old_path)?;
        let parent = new_path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = new_path.with_extension(format!("migration-{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&content)?;
        file.sync_all()?;
        let linked = fs::hard_link(&temporary, new_path);
        let _ = fs::remove_file(&temporary);
        linked.or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })
    })();
    match result {
        Ok(()) => {
            info!(old = %old_path.display(), new = %new_path.display(), "legacy data migrated")
        }
        Err(error) => {
            warn!(old = %old_path.display(), new = %new_path.display(), %error, "legacy data migration failed")
        }
    }
}
pub(crate) fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::env::temp_dir().join("tylora-config"));
    let path = base.join("tylora/config.toml");
    migrate_legacy_file(&path, &base.join("fluxwm/config.toml"));
    path
}
fn load_config() -> Config {
    let path = config_path();
    if !path.exists() {
        warn!(path = %path.display(), "config file missing; default workspaces have no native_space_id, so Option+digit is passed to macOS");
        return Config::default();
    }
    match Config::load(&path) {
        Ok(config) => {
            info!(path = %path.display(), "configuration loaded");
            config
        }
        Err(error) => {
            warn!(%error, path = %path.display(), "invalid configuration; using defaults");
            Config::default()
        }
    }
}

struct ConfigWatcher {
    updates: Receiver<Config>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl ConfigWatcher {
    fn start(path: PathBuf) -> Self {
        let (sender, updates) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new().name("tylora-config-watcher".into()).spawn(move || {
            let mut previous = fs::read(&path).ok();
            while !worker_stop.load(Ordering::Acquire) {
                let current = fs::read(&path).ok();
                if current != previous {
                    let parsed = match current.as_ref() {
                        Some(bytes) => std::str::from_utf8(bytes).map_err(|error| error.to_string()).and_then(|source| Config::parse(source).map_err(|error| error.to_string())),
                        None => Ok(Config::default()),
                    };
                    match parsed {
                        Ok(config) => {
                            if sender.try_send(config).is_ok() { previous = current; }
                        }
                        Err(error) => {
                            warn!(%error, path = %path.display(), "config reload rejected; previous bindings retained");
                            previous = current;
                        }
                    }
                }
                for _ in 0..10 {
                    if worker_stop.load(Ordering::Acquire) { return; }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }).ok();
        Self {
            updates,
            stop,
            worker,
        }
    }
}
impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn hotkey_supported(command: &StateCommand, config: &Config) -> bool {
    use tylora_macos::spaces::{ExperimentalSkyLightBackend, NativeSpaceBackend};
    if matches!(
        command,
        StateCommand::MoveWindowToDisplay { .. }
            | StateCommand::MoveWindowToNextDisplay { .. }
            | StateCommand::MoveWindowToPreviousDisplay { .. }
    ) {
        let backend = ExperimentalSkyLightBackend::new();
        return backend.capabilities().can_move_window_between_spaces
            && backend.list_spaces().is_ok_and(|spaces| {
                let displays: HashSet<_> = spaces
                    .iter()
                    .filter(|space| space.active)
                    .map(|space| space.display)
                    .collect();
                displays.len() > 1
                    && match command {
                        StateCommand::MoveWindowToDisplay { display, .. } => {
                            displays.contains(display)
                        }
                        _ => true,
                    }
            });
    }
    if matches!(
        command,
        StateCommand::MoveWindowToWorkspace { .. }
            | StateCommand::MoveWindowToNamedWorkspace { .. }
    ) {
        let backend = ExperimentalSkyLightBackend::new();
        return backend.capabilities().can_move_window_between_spaces
            && backend.list_spaces().is_ok_and(|spaces| {
                let bound = match command {
                    StateCommand::MoveWindowToWorkspace { workspace, .. } => {
                        config.workspaces.iter().find(|ws| ws.id == *workspace)
                    }
                    StateCommand::MoveWindowToNamedWorkspace { name, .. } => config.workspace(name),
                    _ => None,
                };
                bound.is_some_and(|ws| {
                    ws.space_ids()
                        .into_iter()
                        .any(|id| spaces.iter().any(|space| space.id.0 == id))
                })
            });
    }
    if matches!(
        command,
        StateCommand::FocusWorkspace { .. }
            | StateCommand::FocusNamedWorkspace { .. }
            | StateCommand::FocusNextWorkspace
            | StateCommand::FocusPreviousWorkspace
    ) {
        let backend = ExperimentalSkyLightBackend::new();
        return backend.gesture_switch_available()
            && backend.list_spaces().is_ok_and(|spaces| {
                let exists = |id: u64| spaces.iter().any(|space| space.id.0 == id);
                match command {
                    StateCommand::FocusWorkspace { id } => config
                        .workspaces
                        .iter()
                        .find(|ws| ws.id == *id)
                        .is_some_and(|ws| ws.space_ids().into_iter().any(exists)),
                    StateCommand::FocusNamedWorkspace { name } => config
                        .workspace(name)
                        .is_some_and(|ws| ws.space_ids().into_iter().any(exists)),
                    StateCommand::FocusNextWorkspace | StateCommand::FocusPreviousWorkspace => {
                        config
                            .workspaces
                            .iter()
                            .filter(|ws| ws.space_ids().into_iter().any(exists))
                            .count()
                            > 1
                    }
                    _ => false,
                }
            });
    }
    matches!(
        command,
        StateCommand::FocusWindow { .. }
            | StateCommand::MoveWindow { .. }
            | StateCommand::SetLayout { .. }
            | StateCommand::CycleLayout
            | StateCommand::FocusNextWindow
            | StateCommand::FocusPreviousWindow
            | StateCommand::ScrollLayoutLeft
            | StateCommand::ScrollLayoutRight
            | StateCommand::CenterFocusedWindow
            | StateCommand::ResizeScrollingColumn { .. }
            | StateCommand::ResizeWindow { .. }
            | StateCommand::ToggleFloating { .. }
            | StateCommand::ToggleLayoutZoom { .. }
            | StateCommand::ReconcileWindows
            | StateCommand::FocusDisplay { .. }
            | StateCommand::FocusDisplayNext
            | StateCommand::MoveWindowToDisplay { .. }
            | StateCommand::MoveWindowToNextDisplay { .. }
            | StateCommand::MoveWindowToPreviousDisplay { .. }
    )
}

/// When each native Space last held keyboard focus. Picks between the
/// instances of a workspace bound to several Spaces.
type SpaceFocusHistory = HashMap<u64, Instant>;

/// Chooses which bound Space of a workspace to focus. `existing` keeps the
/// configured order; `current` is the Space active on the display under the
/// pointer, which is where the user is (switching moves the pointer to the
/// target display). If it is one of the instances, the next one is chosen,
/// so pressing the key again toggles between displays. Keyboard focus is not
/// used for this: macOS leaves it on the previous display when the target
/// Space has no window. Otherwise the instance focused most recently wins,
/// then one already visible, then the first.
fn choose_workspace_instance(
    existing: &[(u64, bool)],
    history: &SpaceFocusHistory,
    current: Option<u64>,
) -> Option<u64> {
    let ids: Vec<_> = existing.iter().map(|(id, _)| *id).collect();
    if ids.len() <= 1 {
        return ids.first().copied();
    }
    if let Some(index) = current.and_then(|current| ids.iter().position(|id| *id == current)) {
        return Some(ids[(index + 1) % ids.len()]);
    }
    ids.iter()
        .filter_map(|id| history.get(id).map(|at| (*id, *at)))
        .max_by_key(|(_, at)| *at)
        .map(|(id, _)| id)
        .or_else(|| {
            existing
                .iter()
                .find(|(_, active)| *active)
                .map(|(id, _)| *id)
        })
        .or_else(|| ids.first().copied())
}

fn resolve_focus_target(
    config: &Config,
    command: &StateCommand,
    history: &SpaceFocusHistory,
) -> Result<(DisplayId, tylora_macos::spaces::NativeSpaceId), String> {
    use tylora_macos::spaces::{
        cursor_display, ExperimentalSkyLightBackend, NativeSpaceBackend, NativeSpaceId,
    };
    let backend = ExperimentalSkyLightBackend::new();
    let spaces = backend.list_spaces().map_err(|error| error.to_string())?;
    let pointer = cursor_display();
    let current = spaces
        .iter()
        .find(|space| space.active && Some(space.display) == pointer)
        .map(|space| space.id.0);
    let resolve = |ws: &tylora_config::WorkspaceDefinition| -> Result<_, String> {
        let existing: Vec<_> = ws
            .space_ids()
            .into_iter()
            .filter_map(|id| spaces.iter().find(|space| space.id.0 == id))
            .map(|space| (space.id.0, space.active))
            .collect();
        if ws.space_ids().is_empty() {
            return Err("workspace binding is unresolved; set native_space_id".into());
        }
        let id = choose_workspace_instance(&existing, history, current)
            .ok_or("bound native Space no longer exists")?;
        let display = spaces
            .iter()
            .find(|space| space.id.0 == id)
            .map(|space| space.display)
            .ok_or("bound native Space no longer exists")?;
        Ok((display, NativeSpaceId(id)))
    };
    match command {
        StateCommand::FocusWorkspace { id } => resolve(
            config
                .workspaces
                .iter()
                .find(|ws| ws.id == *id)
                .ok_or("workspace is not configured")?,
        ),
        StateCommand::FocusNamedWorkspace { name } => resolve(
            config
                .workspace(name)
                .ok_or("workspace is not configured")?,
        ),
        StateCommand::FocusNextWorkspace | StateCommand::FocusPreviousWorkspace => {
            let display = cursor_display().ok_or("pointer display unavailable")?;
            let current = backend
                .get_active_space(display)
                .map_err(|error| error.to_string())?
                .id;
            // One entry per workspace with a Space on this display.
            let candidates: Vec<u64> = config
                .workspaces
                .iter()
                .filter_map(|ws| {
                    ws.space_ids().into_iter().find(|id| {
                        spaces
                            .iter()
                            .any(|space| space.id.0 == *id && space.display == display)
                    })
                })
                .collect();
            if candidates.is_empty() {
                return Err("no resolved workspaces on pointer display".into());
            }
            let index = candidates.iter().position(|id| *id == current.0);
            let next = match (command, index) {
                (StateCommand::FocusNextWorkspace, Some(index)) => (index + 1) % candidates.len(),
                (StateCommand::FocusPreviousWorkspace, Some(index)) => {
                    (index + candidates.len() - 1) % candidates.len()
                }
                (StateCommand::FocusPreviousWorkspace, None) => candidates.len() - 1,
                _ => 0,
            };
            Ok((display, NativeSpaceId(candidates[next])))
        }
        _ => Err("not a native focus command".into()),
    }
}

/// Switches to a workspace's native Space, first moving the pointer when the
/// Space belongs to another display. Returns whether keyboard focus still has
/// to be moved: a Space that is already active on another display needs no
/// gesture, so macOS leaves focus on the previous display.
fn focus_workspace_space(
    display: DisplayId,
    target: tylora_macos::spaces::NativeSpaceId,
) -> Result<(tylora_macos::spaces::SwitchOutcome, bool), String> {
    use tylora_macos::spaces::{cursor_display, warp_cursor_to_display, NativeSpaceSwitcher};
    let cross_display = cursor_display() != Some(display);
    if cross_display {
        warp_cursor_to_display(display).map_err(|error| error.to_string())?;
    }
    let outcome =
        NativeSpaceSwitcher::focus_id(display, target).map_err(|error| error.to_string())?;
    let needs_focus = cross_display && outcome.source == outcome.target;
    Ok((outcome, needs_focus))
}

/// Records the native Space of a focused managed window.
fn note_space_focus(state: &DisplayState, id: ManagedWindowId, history: &mut SpaceFocusHistory) {
    if let Some(space) = state
        .coordinators
        .values()
        .find_map(|c| c.registry.get(id))
        .and_then(|record| record.observed.native_space_id)
    {
        history.insert(space, Instant::now());
    }
}

/// Request IDs of passive focus probes; disjoint from command request IDs.
const PASSIVE_FOCUS_REQUEST: u64 = 1 << 62;

/// Pointer-focus quiet period after a workspace switch is confirmed, covering
/// the Space observer delay and the first reconciliation.
const WORKSPACE_SWITCH_POINTER_SETTLE: Duration = Duration::from_millis(400);

/// Target Space for moving a window to a workspace. With several bound
/// Spaces the one on the window's current display is used, so the window
/// keeps its monitor; otherwise the most recently focused instance.
fn resolve_move_target(
    config: &Config,
    command: &StateCommand,
    window_display: Option<DisplayId>,
    history: &SpaceFocusHistory,
) -> Result<(DisplayId, tylora_macos::spaces::NativeSpaceId), String> {
    use tylora_macos::spaces::{ExperimentalSkyLightBackend, NativeSpaceBackend, NativeSpaceId};
    let workspace = match command {
        StateCommand::MoveWindowToWorkspace { workspace, .. } => {
            config.workspaces.iter().find(|ws| ws.id == *workspace)
        }
        StateCommand::MoveWindowToNamedWorkspace { name, .. } => config.workspace(name),
        _ => return Err("not a native move command".into()),
    }
    .ok_or("workspace is not configured")?;
    if workspace.space_ids().is_empty() {
        return Err("workspace binding is unresolved".into());
    }
    let spaces = ExperimentalSkyLightBackend::new()
        .list_spaces()
        .map_err(|e| e.to_string())?;
    let existing: Vec<_> = workspace
        .space_ids()
        .into_iter()
        .filter_map(|id| spaces.iter().find(|space| space.id.0 == id))
        .collect();
    let chosen = existing
        .iter()
        .copied()
        .find(|space| Some(space.display) == window_display)
        .or_else(|| {
            existing
                .iter()
                .filter_map(|space| history.get(&space.id.0).map(|at| (*space, *at)))
                .max_by_key(|(_, at)| *at)
                .map(|(space, _)| space)
        })
        .or_else(|| existing.first().copied())
        .ok_or_else(|| {
            format!(
                "bound native Spaces {:?} no longer exist",
                workspace.space_ids()
            )
        })?;
    Ok((chosen.display, NativeSpaceId(chosen.id.0)))
}

enum NativeMoveEvent {
    DisplayMoved {
        prepared: PreparedDisplayMove,
        response: Option<SyncSender<String>>,
        result: Result<tylora_macos::spaces::MoveOutcome, tylora_macos::spaces::SpaceError>,
    },
    Moved {
        request: u64,
        id: ManagedWindowId,
        generation: u64,
        display: DisplayId,
        issued: Instant,
        response: Option<SyncSender<String>>,
        result: Result<tylora_macos::spaces::MoveOutcome, String>,
    },
    Switched {
        request: u64,
        target: tylora_macos::spaces::NativeSpaceId,
        result: Result<(), String>,
    },
    /// A workspace Space switch finished (or failed). `focus` names a display
    /// whose already-active Space still needs keyboard focus.
    WorkspaceSwitched {
        switch: u64,
        focus: Option<DisplayId>,
        /// Space that became current, recorded in the focus history even
        /// when it holds no window to focus.
        space: Option<u64>,
    },
}

#[derive(Clone, Copy)]
struct PreparedDisplayMove {
    issued: Instant,
    id: ManagedWindowId,
    generation: u64,
    cg_window_id: u32,
    source_display: DisplayId,
    target_display: DisplayId,
    source_space: u64,
    target_space: u64,
    mode: tylora_core::registry::ManagementMode,
    target_frame: tylora_core::Rect,
    focus_after_move: bool,
    /// Desktop number to create on the target display, and how many Desktops
    /// must be added to reach it. `target_space` is resolved after creation.
    create: Option<(u8, u8)>,
    /// Workspace that gains the created Space as another instance.
    bind_workspace: Option<u8>,
}

/// Where a window moved to another display lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayTarget {
    Existing(u64),
    /// Desktop `desktop` does not exist yet; `missing` Desktops are needed.
    Create {
        desktop: u8,
        missing: u8,
    },
}

/// At most this many Desktops are created for one move.
const MAX_CREATED_DESKTOPS: u8 = 8;

/// The source Space's workspace instance on the target display if it has
/// one, else the target Desktop with the same number, else that Desktop must
/// be created. Returns the workspace to bind a created Space to.
fn choose_display_target(
    config: &Config,
    spaces: &[NativeSpace],
    source_space: u64,
    target_display: DisplayId,
) -> Option<(DisplayTarget, Option<u8>)> {
    let source = spaces.iter().find(|space| space.id.0 == source_space)?;
    let workspace = config.workspaces.iter().find(|ws| ws.binds(source_space));
    let on_target = |id: u64| {
        spaces
            .iter()
            .any(|space| space.id.0 == id && space.display == target_display)
    };
    if let Some(id) = workspace.and_then(|ws| ws.space_ids().into_iter().find(|id| on_target(*id)))
    {
        return Some((DisplayTarget::Existing(id), None));
    }
    let desktop = source.desktop.0;
    if let Some(space) = spaces
        .iter()
        .find(|space| space.display == target_display && space.desktop.0 == desktop)
    {
        return Some((DisplayTarget::Existing(space.id.0), None));
    }
    let existing = spaces
        .iter()
        .filter(|space| space.display == target_display)
        .map(|space| space.desktop.0)
        .max()
        .unwrap_or(0);
    let missing = desktop.saturating_sub(existing);
    (missing > 0 && missing <= MAX_CREATED_DESKTOPS).then(|| {
        (
            DisplayTarget::Create { desktop, missing },
            workspace.map(|ws| ws.id),
        )
    })
}

/// Adds Desktops through Mission Control and waits for WindowServer to list
/// Desktop `desktop` on `display`. Returns its Space ID.
fn create_target_desktop(display: DisplayId, desktop: u8, missing: u8) -> Result<u64, String> {
    use tylora_macos::spaces::{ExperimentalSkyLightBackend, NativeSpaceBackend};
    let started = Instant::now();
    tylora_macos::add_desktops(display, missing)?;
    let backend = ExperimentalSkyLightBackend::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(spaces) = backend.list_spaces() {
            if let Some(space) = spaces
                .iter()
                .find(|space| space.display == display && space.desktop.0 == desktop)
            {
                let display_id = display.0;
                info!(
                    display_id,
                    desktop,
                    space_id = space.id.0,
                    elapsed_ms = started.elapsed().as_millis(),
                    "Desktop created through Mission Control"
                );
                return Ok(space.id.0);
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "Desktop {desktop} did not appear on display {}",
                display.0
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[derive(Clone, Copy)]
struct PendingDisplayMove {
    prepared: PreparedDisplayMove,
    issued: Instant,
    verified: bool,
}

fn effective_hotkeys(config: &Config) -> Vec<Hotkey> {
    let mut hotkeys = config.hotkeys.clone();
    for default in [
        Hotkey {
            chord: "option+shift+f".into(),
            command: StateCommand::ToggleLayoutZoom { window: None },
            enabled: true,
        },
        Hotkey {
            chord: "option+shift+tab".into(),
            command: StateCommand::MoveWindowToNextDisplay { window: None },
            enabled: true,
        },
    ]
    .into_iter()
    .chain(tylora_config::layout_hotkeys())
    {
        let same_action = hotkeys.iter().any(|entry| {
            std::mem::discriminant(&entry.command) == std::mem::discriminant(&default.command)
        });
        let chord = compile_hotkey(&default).ok();
        let occupied = hotkeys
            .iter()
            .filter_map(|entry| compile_hotkey(entry).ok())
            .any(|entry| {
                chord.as_ref().is_some_and(|candidate| {
                    entry.keycode == candidate.keycode && entry.modifiers == candidate.modifiers
                })
            });
        if !same_action && !occupied {
            hotkeys.push(default);
        }
    }
    hotkeys
}

fn layout_settings(config: &Config) -> LayoutSettings {
    LayoutSettings {
        dwindle: config.dwindle,
        scrolling: config.scrolling.clone(),
    }
}

fn new_layout(kind: LayoutKind, config: &Config) -> TilingLayout {
    let mut layout = TilingLayout::new(kind, &layout_settings(config));
    let _ = layout.set_gaps(config.gaps.inner, config.gaps.outer);
    layout
}

fn active_bindings(config: &Config) -> Vec<tylora_config::CompiledHotkey> {
    effective_hotkeys(config).iter().filter(|binding| binding.enabled)
        .filter(|binding| {
            let available = hotkey_supported(&binding.command, config);
            if !available { warn!(chord = %binding.chord, "hotkey unavailable or workspace unresolved; key remains with macOS"); }
            available
        })
        .filter_map(|binding| compile_hotkey(binding).ok()).collect()
}

/// One writer owns every display. Each display keeps its own registry and BSP
/// tree; reconciliation partitions process snapshots by display.
struct DisplayState {
    active: HashSet<DisplayId>,
    areas: HashMap<DisplayId, WorkArea>,
    coordinators: HashMap<DisplayId, Coordinator<TilingLayout>>,
    workspaces: HashMap<DisplayId, WorkspaceEngine<TilingLayout>>,
    native_spaces: Vec<NativeSpace>,
    layout_zoom: HashMap<(DisplayId, u64), ZoomTarget>,
    zoom_outer_gap: f64,
    zoom_respect_outer_gaps: bool,
    pending_native: HashMap<ManagedWindowId, (u32, tylora_macos::spaces::NativeSpaceId, Instant)>,
    in_flight_display: HashSet<ManagedWindowId>,
    pending_display: HashMap<u32, PendingDisplayMove>,
    display_rehomes: Vec<(ManagedWindowId, tylora_core::Rect)>,
    display_focus: Vec<ManagedWindowId>,
    hidden_by_tylora: HashSet<ManagedWindowId>,
    pending_hide: HashSet<ManagedWindowId>,
    saved_active: HashMap<u128, u8>,
    primary: DisplayId,
    moving: HashMap<ManagedWindowId, (DisplayId, Instant)>,
    dropped_stale: u64,
    window_rules: Vec<WindowRule>,
    rule_applied: HashSet<ManagedWindowId>,
    /// Layouts of native Spaces that no workspace is bound to.
    space_layouts: HashMap<(DisplayId, u64), TilingLayout>,
    /// Layout kind last taken from the configuration per workspace copy, so a
    /// reload only overrides a runtime `layout set` when the config changed.
    applied_layouts: HashMap<(DisplayId, u8), LayoutKind>,
    applied_default_layout: Option<LayoutKind>,
    /// Native Space ID -> configured workspace bound to it.
    space_bindings: HashMap<u64, u8>,
}

#[derive(Debug, Clone, Copy)]
struct ZoomTarget {
    id: ManagedWindowId,
    cg_window_id: u32,
    generation: u64,
}

fn workspace_state_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join("tylora/workspaces.json");
    migrate_legacy_file(&path, &base.join("fluxwm/workspaces.json"));
    path
}
fn load_active_workspaces() -> HashMap<u128, u8> {
    fs::read(workspace_state_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HashMap<String, u8>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, id)| u128::from_str_radix(&key, 16).ok().map(|uuid| (uuid, id)))
        .collect()
}
fn save_active_workspaces(state: &DisplayState) {
    let mut values = state.saved_active.clone();
    for (&display, engine) in &state.workspaces {
        if let (Some(uuid), Some(id)) = (
            state.areas.get(&display).and_then(|area| area.stable_uuid),
            engine.active(display),
        ) {
            values.insert(uuid, id);
        }
    }
    let mapped: HashMap<_, _> = values
        .into_iter()
        .map(|(uuid, id)| (format!("{uuid:032x}"), id))
        .collect();
    let path = workspace_state_path();
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::create_dir_all(parent) {
            warn!(%error, "cannot create workspace state directory");
            return;
        }
    }
    let Ok(bytes) = serde_json::to_vec(&mapped) else {
        return;
    };
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temp);
    match OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temp)
        .and_then(|mut file| {
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&temp, &path))
    {
        Ok(()) => {}
        Err(error) => warn!(%error, "cannot persist active workspaces"),
    }
}

/// Where a bound workspace's native Space last was. macOS sometimes recreates
/// a Space with a new ID (display reconnect, sleep, lock); the slot lets the
/// daemon follow the replacement instead of leaving the workspace unresolved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SpaceSlot {
    space_id: u64,
    display_uuid: String,
    desktop: u8,
}

fn space_slots_path() -> PathBuf {
    workspace_state_path().with_file_name("space-slots.json")
}

fn load_space_slots() -> HashMap<String, SpaceSlot> {
    let mut slots: HashMap<String, SpaceSlot> = fs::read(space_slots_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    // Keys are "workspace:space"; drop entries from the earlier format.
    slots.retain(|key, _| key.contains(':'));
    slots
}

fn save_space_slots(slots: &HashMap<String, SpaceSlot>) {
    let path = space_slots_path();
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec(slots).map_err(std::io::Error::other)?;
        let temp = path.with_extension(format!("tmp-{}", std::process::id()));
        let _ = fs::remove_file(&temp);
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temp, &path)
    })();
    if let Err(error) = result {
        warn!(%error, "cannot persist native Space slots");
    }
}

/// Key of the remembered slot of one bound Space of one workspace.
fn slot_key(workspace: u8, space: u64) -> String {
    format!("{workspace}:{space}")
}

/// Updates remembered slots for resolved bindings and returns rebinds
/// `(workspace, old Space ID, new Space ID)` for bound Spaces that vanished
/// while an unbound Space now sits on the same display at the same Desktop
/// position. A binding only moves to a Space no workspace uses, and only
/// when exactly one candidate exists; nothing is inferred without a slot.
fn plan_space_rebinds(
    config: &Config,
    spaces: &[NativeSpace],
    display_uuid: impl Fn(DisplayId) -> Option<u128>,
    slots: &mut HashMap<String, SpaceSlot>,
) -> Vec<(u8, u64, u64)> {
    let mut bound: HashSet<u64> = config
        .workspaces
        .iter()
        .flat_map(|ws| ws.space_ids())
        .collect();
    let slot_of = |space: &NativeSpace| {
        display_uuid(space.display).map(|uuid| SpaceSlot {
            space_id: space.id.0,
            display_uuid: format!("{uuid:032x}"),
            desktop: space.desktop.0,
        })
    };
    let mut rebinds = Vec::new();
    for ws in &config.workspaces {
        for configured in ws.space_ids() {
            if let Some(space) = spaces.iter().find(|space| space.id.0 == configured) {
                if let Some(slot) = slot_of(space) {
                    slots.insert(slot_key(ws.id, configured), slot);
                }
                continue;
            }
            let Some(slot) = slots
                .get(&slot_key(ws.id, configured))
                .filter(|slot| slot.space_id == configured)
                .cloned()
            else {
                continue;
            };
            let candidates: Vec<_> = spaces
                .iter()
                .filter(|space| !bound.contains(&space.id.0) && space.desktop.0 == slot.desktop)
                .filter(|space| {
                    slot_of(space).is_some_and(|current| current.display_uuid == slot.display_uuid)
                })
                .collect();
            if let [replacement] = candidates.as_slice() {
                bound.insert(replacement.id.0);
                slots.remove(&slot_key(ws.id, configured));
                if let Some(slot) = slot_of(replacement) {
                    slots.insert(slot_key(ws.id, replacement.id.0), slot);
                }
                rebinds.push((ws.id, configured, replacement.id.0));
            }
        }
    }
    rebinds
}

/// Adds `space` to a workspace's extra native Spaces through the config
/// manager and returns the resulting configuration.
fn bind_space_to_workspace(config: &Config, workspace: u8, space: u64) -> Option<Config> {
    let definition = config.workspaces.iter().find(|ws| ws.id == workspace)?;
    if definition.binds(space) {
        return None;
    }
    let snapshot = crate::config_manager::read(&config_path()).ok()?;
    let mut extras = definition.native_space_ids.clone();
    extras.push(space);
    let request = crate::config_manager::PatchRequest {
        version: 1,
        revision: snapshot.revision,
        patch: tylora_config::mutation::ConfigPatch::SetWorkspace {
            id: workspace,
            name: definition.name.clone(),
            display: definition.display,
            native_space_id: definition.native_space_id,
            native_space_ids: Some(extras),
        },
    };
    match crate::config_manager::patch(&config_path(), request) {
        Ok(snapshot) => {
            info!(workspace = %definition.name, space, "created Desktop bound to workspace");
            Some(snapshot.config)
        }
        Err(error) => {
            warn!(%error, workspace = %definition.name, "cannot bind created Desktop");
            None
        }
    }
}

/// Persists rebinds through the config manager (revision-checked, comment
/// preserving) and returns the resulting configuration.
fn apply_space_rebinds(config: &Config, rebinds: &[(u8, u64, u64)]) -> Option<Config> {
    let mut latest: Option<Config> = None;
    for &(id, old, new) in rebinds {
        let current = latest.as_ref().unwrap_or(config);
        let Some(definition) = current.workspaces.iter().find(|ws| ws.id == id) else {
            continue;
        };
        let (primary, extras) = if definition.native_space_id == Some(old) {
            (Some(new), None)
        } else {
            (
                definition.native_space_id,
                Some(
                    definition
                        .native_space_ids
                        .iter()
                        .map(|space| if *space == old { new } else { *space })
                        .collect(),
                ),
            )
        };
        let Ok(snapshot) = crate::config_manager::read(&config_path()) else {
            continue;
        };
        let request = crate::config_manager::PatchRequest {
            version: 1,
            revision: snapshot.revision,
            patch: tylora_config::mutation::ConfigPatch::SetWorkspace {
                id,
                name: definition.name.clone(),
                display: definition.display,
                native_space_id: primary,
                native_space_ids: extras,
            },
        };
        let name = definition.name.clone();
        match crate::config_manager::patch(&config_path(), request) {
            Ok(snapshot) => {
                warn!(
                    workspace = %name,
                    old,
                    new,
                    "macOS recreated the bound Space; workspace rebound to its replacement"
                );
                latest = Some(snapshot.config);
            }
            Err(error) => warn!(%error, workspace = %name, "cannot rebind workspace"),
        }
    }
    latest
}

struct ConfirmedVisibility;
impl WorkspaceVisibilityBackend for ConfirmedVisibility {
    fn transition(
        &mut self,
        _: &[ManagedWindowId],
        _: &[ManagedWindowId],
    ) -> Result<(), CommandError> {
        Ok(())
    }
}

#[cfg_attr(not(test), allow(dead_code))] // Legacy variants remain only for migration tests and journal recovery.
enum WorkspaceAction {
    Focus {
        display: DisplayId,
        target: u8,
        source_layout: TilingLayout,
        hide: Vec<ManagedWindowId>,
        show: Vec<ManagedWindowId>,
    },
    Move {
        display: DisplayId,
        window: ManagedWindowId,
        target: u8,
        source_layout: TilingLayout,
        hide: Vec<ManagedWindowId>,
    },
    Recover,
}
struct PendingWorkspace {
    request: u64,
    action: WorkspaceAction,
    response: Option<SyncSender<String>>,
    started: Instant,
    timed_out: bool,
}

struct DragSession {
    start: MouseInput,
    initial: tylora_core::Rect,
    original_tree: Option<TilingLayout>,
    last_pointer: (f64, f64),
    latest_target: Option<tylora_core::Rect>,
    sent_target: Option<tylora_core::Rect>,
    ax_pending: bool,
    finished: bool,
    moved: bool,
    last_send: Instant,
}
impl DragSession {
    fn begin(input: MouseInput, state: &DisplayState) -> Option<Self> {
        let coordinator = state
            .coordinators
            .iter()
            .filter(|(display, _)| state.active.contains(display))
            .find_map(|(_, c)| c.registry.get(input.id).map(|r| (c, r)))?;
        let original_tree = (!input.floating && input.button == MouseButton::Right)
            .then(|| coordinator.0.layout.clone());
        Some(Self {
            start: input,
            initial: coordinator.1.observed.frame,
            original_tree,
            last_pointer: input.start,
            latest_target: None,
            sent_target: None,
            ax_pending: false,
            finished: false,
            moved: false,
            last_send: Instant::now() - Duration::from_secs(1),
        })
    }
    fn update(
        &mut self,
        input: MouseInput,
        state: &mut DisplayState,
        adapter: &ManagedAdapter,
        config: &Config,
    ) {
        if input.id != self.start.id || input.button != self.start.button {
            return;
        }
        if input.phase == MousePhase::Cancel {
            if let Some(original) = self.original_tree.take() {
                if let Some(c) = state.coordinator_for_active(input.id) {
                    c.layout = original;
                    c.version = c.version.wrapping_add(1);
                }
            }
            if self.start.floating && self.moved {
                self.latest_target = Some(self.initial);
            }
            self.finished = true;
            return;
        }
        let dx = input.current.0 - input.start.0;
        let dy = input.current.1 - input.start.1;
        let moved = dx * dx + dy * dy >= config.mouse.drag_threshold * config.mouse.drag_threshold;
        if moved {
            self.moved = true;
        }
        if self.start.floating && moved {
            let area = state
                .coordinators
                .iter()
                .filter(|(display, _)| state.active.contains(display))
                .find_map(|(_, c)| c.registry.get(input.id).map(|_| c.work_area));
            if let Some(area) = area {
                let target = if input.button == MouseButton::Left {
                    let x =
                        (self.initial.x + dx).clamp(area.x, area.x + (area.width - 32.0).max(0.0));
                    let y =
                        (self.initial.y + dy).clamp(area.y, area.y + (area.height - 32.0).max(0.0));
                    tylora_core::Rect::new(x, y, self.initial.width, self.initial.height).ok()
                } else {
                    let min_width = 80.0_f64.min(area.width);
                    let min_height = 60.0_f64.min(area.height);
                    let x = self
                        .initial
                        .x
                        .clamp(area.x, area.x + (area.width - min_width).max(0.0));
                    let y = self
                        .initial
                        .y
                        .clamp(area.y, area.y + (area.height - min_height).max(0.0));
                    tylora_core::Rect::new(
                        x,
                        y,
                        (self.initial.width + dx * config.mouse.resize_sensitivity)
                            .clamp(min_width, (area.x + area.width - x).max(min_width)),
                        (self.initial.height + dy * config.mouse.resize_sensitivity)
                            .clamp(min_height, (area.y + area.height - y).max(min_height)),
                    )
                    .ok()
                };
                self.latest_target = target;
            }
        } else if !self.start.floating
            && input.button == MouseButton::Right
            && moved
            && state.coordinator_for_active(input.id).is_none_or(|c| {
                c.layout.kind() != LayoutKind::Scrolling || config.scrolling.mouse_resize
            })
        {
            let step_x = input.current.0 - self.last_pointer.0;
            let step_y = input.current.1 - self.last_pointer.1;
            // A coalesced mouse-up must apply its final delta even inside the
            // throttle interval; otherwise the BSP ratio lags the pointer.
            if input.phase == MousePhase::End
                || self.last_send.elapsed()
                    >= Duration::from_millis(config.mouse.update_interval_ms)
            {
                let (direction, amount) = if step_x.abs() >= step_y.abs() {
                    (
                        if step_x >= 0.0 {
                            tylora_core::commands::Direction::Right
                        } else {
                            tylora_core::commands::Direction::Left
                        },
                        step_x.abs() * config.mouse.resize_sensitivity,
                    )
                } else {
                    (
                        if step_y >= 0.0 {
                            tylora_core::commands::Direction::Down
                        } else {
                            tylora_core::commands::Direction::Up
                        },
                        step_y.abs() * config.mouse.resize_sensitivity,
                    )
                };
                if amount >= 1.0 {
                    if let Some(c) = state.coordinator_for_active(input.id) {
                        let _ = c.execute(
                            &StateCommand::ResizeWindow { direction, amount },
                            Some(input.id),
                        );
                    }
                    self.last_pointer = input.current;
                    self.last_send = Instant::now();
                }
            }
        }
        if input.phase == MousePhase::End {
            if !self.start.floating && input.button == MouseButton::Left && moved {
                if let Some(c) = state.coordinator_for_active(input.id) {
                    let targets: Vec<_> = c
                        .registry
                        .iter()
                        .filter(|(id, _)| **id != input.id)
                        .filter(|(_, r)| {
                            r.observed.mode == tylora_core::registry::ManagementMode::Tiled
                        })
                        .filter(|(_, r)| point_inside(r.observed.frame, input.current))
                        .map(|(id, _)| *id)
                        .collect();
                    if targets.len() == 1 {
                        let _ = c.execute(
                            &StateCommand::SwapWindow {
                                first: input.id,
                                second: targets[0],
                            },
                            Some(input.id),
                        );
                    }
                }
            }
            if !moved {
                let _ = adapter.commands.try_send(ManagedCommand::Focus(input.id));
            }
            self.finished = true;
        }
    }
    fn flush(&mut self, adapter: &ManagedAdapter, config: &Config) -> bool {
        if self.start.floating {
            if !self.ax_pending
                && (self.finished
                    || self.last_send.elapsed()
                        >= Duration::from_millis(config.mouse.update_interval_ms))
            {
                if let Some(target) = self
                    .latest_target
                    .filter(|target| Some(*target) != self.sent_target)
                {
                    if adapter
                        .commands
                        .try_send(ManagedCommand::DragFrame {
                            id: self.start.id,
                            target,
                        })
                        .is_ok()
                    {
                        self.ax_pending = true;
                        self.sent_target = Some(target);
                        self.last_send = Instant::now();
                    }
                }
            }
            if self.ax_pending || self.latest_target != self.sent_target {
                return false;
            }
        }
        if self.finished {
            let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
            return true;
        }
        false
    }
}
fn point_inside(frame: tylora_core::Rect, point: (f64, f64)) -> bool {
    point.0 >= frame.x
        && point.0 < frame.x + frame.width
        && point.1 >= frame.y
        && point.1 < frame.y + frame.height
}
impl DisplayState {
    /// Applies configured layout kinds and settings to every workspace copy,
    /// the live coordinator layouts and unbound Space layouts.
    fn sync_layout_config(&mut self, config: &Config) {
        self.space_bindings = config
            .workspaces
            .iter()
            .flat_map(|ws| ws.space_ids().into_iter().map(|space| (space, ws.id)))
            .collect();
        let settings = layout_settings(config);
        for coordinator in self.coordinators.values_mut() {
            coordinator.layout.set_settings(&settings);
            coordinator.version = coordinator.version.wrapping_add(1);
        }
        for layout in self.space_layouts.values_mut() {
            layout.set_settings(&settings);
            let _ = layout.set_gaps(config.gaps.inner, config.gaps.outer);
        }
        let displays: Vec<_> = self.coordinators.keys().copied().collect();
        for display in displays {
            let active_space = self
                .native_spaces
                .iter()
                .find(|space| space.display == display && space.active)
                .map(|space| space.id.0);
            for definition in &config.workspaces {
                if definition.display.is_some_and(|id| id != display.0) {
                    continue;
                }
                let desired = definition.layout_or(config.default_layout);
                if self.applied_layouts.get(&(display, definition.id)) == Some(&desired) {
                    continue;
                }
                self.applied_layouts
                    .insert((display, definition.id), desired);
                for space in definition.space_ids() {
                    if let Some(saved) = self.space_layouts.get_mut(&(display, space)) {
                        saved.set_kind(desired);
                    }
                }
                if active_space.is_some_and(|space| definition.binds(space)) {
                    if let Some(coordinator) = self.coordinators.get_mut(&display) {
                        if coordinator.layout.set_kind(desired) {
                            coordinator.version = coordinator.version.wrapping_add(1);
                            let display_id = display.0;
                            info!(
                                display_id,
                                workspace = definition.id,
                                layout = desired.name(),
                                "workspace layout applied from configuration"
                            );
                        }
                    }
                }
            }
        }
        if self.applied_default_layout != Some(config.default_layout) {
            let previous = self.applied_default_layout.replace(config.default_layout);
            for ((_, space), layout) in &mut self.space_layouts {
                let unbound = !config.workspaces.iter().any(|ws| ws.binds(*space));
                if unbound && previous.is_some_and(|kind| kind == layout.kind()) {
                    layout.set_kind(config.default_layout);
                }
            }
            for (&display, coordinator) in &mut self.coordinators {
                let unbound = self
                    .native_spaces
                    .iter()
                    .find(|space| space.display == display && space.active)
                    .is_some_and(|space| !config.workspaces.iter().any(|ws| ws.binds(space.id.0)));
                if unbound
                    && previous.is_some_and(|kind| kind == coordinator.layout.kind())
                    && coordinator.layout.set_kind(config.default_layout)
                {
                    coordinator.version = coordinator.version.wrapping_add(1);
                }
            }
        }
    }

    /// Display frames for every coordinator, so Scrolling never parks a
    /// window onto a neighbouring display.
    fn sync_environments(&mut self) {
        let frames: Vec<_> = self
            .active
            .iter()
            .filter_map(|display| self.areas.get(display))
            .map(|area| (area.display, area.full_frame))
            .collect();
        for (&display, coordinator) in &mut self.coordinators {
            let Some(area) = self.areas.get(&display) else {
                continue;
            };
            coordinator.set_environment(LayoutEnvironment {
                display_frame: Some(area.full_frame),
                other_displays: frames
                    .iter()
                    .filter(|(other, _)| *other != display)
                    .map(|(_, frame)| *frame)
                    .collect(),
            });
        }
    }

    fn apply_live_config(&mut self, config: &Config) {
        self.window_rules = config.window_rules.clone();
        self.zoom_outer_gap = config.gaps.outer;
        self.zoom_respect_outer_gaps = config.zoom.respect_outer_gaps;
        self.rule_applied.clear();
        for coordinator in self.coordinators.values_mut() {
            let _ = coordinator
                .layout
                .set_gaps(config.gaps.inner, config.gaps.outer);
        }
        for (&display, engine) in &mut self.workspaces {
            for definition in &config.workspaces {
                if definition.display.is_none_or(|id| id == display.0) {
                    if let Some(workspace) = engine.workspace_mut(definition.id) {
                        workspace.name = definition.name.clone();
                        let _ = workspace
                            .layout
                            .set_gaps(config.gaps.inner, config.gaps.outer);
                    } else {
                        let _ = engine.create(definition.id, definition.name.clone(), display);
                        if let Some(workspace) = engine.workspace_mut(definition.id) {
                            let _ = workspace
                                .layout
                                .set_gaps(config.gaps.inner, config.gaps.outer);
                        }
                    }
                }
            }
        }
        self.sync_layout_config(config);
    }
    fn new(areas: Vec<WorkArea>, primary: DisplayId, config: &Config) -> Self {
        let mut state = Self {
            active: HashSet::new(),
            areas: HashMap::new(),
            coordinators: HashMap::new(),
            workspaces: HashMap::new(),
            native_spaces: Vec::new(),
            layout_zoom: HashMap::new(),
            zoom_outer_gap: config.gaps.outer,
            zoom_respect_outer_gaps: config.zoom.respect_outer_gaps,
            pending_native: HashMap::new(),
            in_flight_display: HashSet::new(),
            pending_display: HashMap::new(),
            display_rehomes: Vec::new(),
            display_focus: Vec::new(),
            hidden_by_tylora: HashSet::new(),
            pending_hide: HashSet::new(),
            saved_active: if cfg!(test) {
                HashMap::new()
            } else {
                load_active_workspaces()
            },
            primary,
            moving: HashMap::new(),
            dropped_stale: 0,
            window_rules: config.window_rules.clone(),
            rule_applied: HashSet::new(),
            space_layouts: HashMap::new(),
            applied_layouts: HashMap::new(),
            applied_default_layout: None,
            space_bindings: HashMap::new(),
        };
        let _ = state.update_areas(areas, config);
        state.sync_layout_config(config);
        state
    }
    fn update_areas(
        &mut self,
        areas: Vec<WorkArea>,
        config: &Config,
    ) -> Vec<(ManagedWindowId, tylora_core::Rect)> {
        if let Some(primary) = areas.iter().find(|a| a.primary) {
            self.primary = primary.display;
        }
        let next: HashSet<_> = areas.iter().map(|a| a.display).collect();
        self.layout_zoom
            .retain(|(display, _), _| next.contains(display));
        self.pending_display
            .retain(|_, pending| next.contains(&pending.prepared.target_display));
        let fallback = areas
            .iter()
            .find(|a| a.primary)
            .or_else(|| areas.first())
            .copied();
        let mut rehome = Vec::new();
        for lost in self.active.difference(&next) {
            warn!(
                display = lost.0,
                "display disconnected; tiling suspended and reconciliation requested"
            );
            if let (Some(target_area), Some(coordinator)) = (fallback, self.coordinators.get(lost))
            {
                for (id, record) in coordinator.registry.iter() {
                    if record.observed.mode == tylora_core::registry::ManagementMode::Unmanaged
                        || !matches!(
                            record.observed.lifecycle,
                            tylora_core::registry::WindowLifecycle::Active
                                | tylora_core::registry::WindowLifecycle::Discovered
                        )
                    {
                        continue;
                    }
                    let old = record.observed.frame;
                    let width = old.width.min((target_area.frame.width - 16.0).max(1.0));
                    let height = old.height.min((target_area.frame.height - 16.0).max(1.0));
                    if let Ok(target) = tylora_core::Rect::new(
                        target_area.frame.x + 8.0,
                        target_area.frame.y + 8.0,
                        width,
                        height,
                    ) {
                        rehome.push((*id, target));
                    }
                }
            }
        }
        for area in areas {
            self.areas.insert(area.display, area);
            if let Some(coordinator) = self.coordinators.get_mut(&area.display) {
                if coordinator.work_area != area.frame {
                    coordinator.set_work_area(area.display, area.frame);
                }
            } else {
                let mut layout = TilingLayout::new(config.default_layout, &layout_settings(config));
                let _ = layout.set_gaps(config.gaps.inner, config.gaps.outer);
                self.coordinators.insert(
                    area.display,
                    Coordinator::new(layout, area.display, area.frame),
                );
                info!(display = area.display.0, "display added");
            }
            self.workspaces.entry(area.display).or_insert_with(|| {
                let mut engine = WorkspaceEngine::new();
                for definition in &config.workspaces {
                    if definition.display.is_none_or(|id| id == area.display.0) {
                        let _ = engine.create(definition.id, definition.name.clone(), area.display);
                    }
                }
                let preferred = area
                    .stable_uuid
                    .and_then(|uuid| self.saved_active.get(&uuid).copied())
                    .or_else(|| {
                        config
                            .displays
                            .iter()
                            .find(|assignment| assignment.display == area.display.0)
                            .map(|assignment| assignment.workspace)
                    });
                if let Some(id) = preferred {
                    let _ = engine.focus(area.display, id, &mut ConfirmedVisibility);
                }
                engine
            });
        }
        self.active = next;
        if !self.active.contains(&self.primary) {
            if let Some(&first) = self.active.iter().min_by_key(|id| id.0) {
                self.primary = first;
            }
        }
        self.sync_environments();
        self.sync_layout_config(config);
        rehome
    }
    /// Saves the outgoing Space's layout and installs the incoming one for
    /// every display whose active native Space changed. Layouts are keyed by
    /// (display, native Space), so a bound workspace and an unbound Space
    /// alike get back exactly the order, ratios, widths and viewport they had.
    fn apply_native_spaces(
        &mut self,
        old_active: &[(DisplayId, tylora_macos::spaces::NativeSpaceId)],
        new_active: &[(DisplayId, tylora_macos::spaces::NativeSpaceId)],
        config: &Config,
    ) {
        let bound_workspace = |space: u64, display: DisplayId| {
            config
                .workspaces
                .iter()
                .find(|ws| ws.binds(space) && ws.display.is_none_or(|id| id == display.0))
        };
        for &(display, native_id) in new_active {
            let previous = old_active
                .iter()
                .find(|(d, _)| *d == display)
                .map(|(_, space)| space.0);
            let switched = previous.is_some_and(|space| space != native_id.0);
            let Some(coordinator) = self.coordinators.get_mut(&display) else {
                continue;
            };
            // Save the outgoing Space's layout before the new Space filter
            // removes its windows from the live tree.
            if let Some(previous) = previous.filter(|_| switched) {
                self.space_layouts
                    .insert((display, previous), coordinator.layout.clone());
            }
            coordinator.set_active_native_space(Some(native_id.0));
            let bound = bound_workspace(native_id.0, display);
            if let (Some(workspace), Some(engine)) = (bound, self.workspaces.get_mut(&display)) {
                if engine.active(display) != Some(workspace.id) {
                    let _ = engine.focus(display, workspace.id, &mut ConfirmedVisibility);
                }
            }
            if switched || previous.is_none() {
                let kind = bound.map_or(config.default_layout, |ws| {
                    ws.layout_or(config.default_layout)
                });
                let layout = self
                    .space_layouts
                    .remove(&(display, native_id.0))
                    .unwrap_or_else(|| new_layout(kind, config));
                coordinator.replace_layout(layout);
            }
        }
    }
    fn coordinator_for_active(
        &mut self,
        window: ManagedWindowId,
    ) -> Option<&mut Coordinator<TilingLayout>> {
        self.coordinators
            .iter_mut()
            .filter(|(id, _)| self.active.contains(id))
            .find_map(|(_, c)| c.registry.get(window).is_some().then_some(c))
    }
    fn handle_reconciled(
        &mut self,
        process: ProcessInstance,
        windows: Vec<WindowSnapshot>,
    ) -> bool {
        let mut windows = windows;
        self.pending_display.retain(|cg, pending| {
            pending.verified
                || pending.prepared.id.process != process
                || pending.issued.elapsed() < Duration::from_secs(30)
                || windows.iter().any(|window| {
                    window.cg_window_id == Some(*cg)
                        && window.native_space_id == Some(pending.prepared.target_space)
                })
        });
        let late_verified: Vec<_> = windows
            .iter()
            .filter_map(|window| {
                let pending = self.pending_display.get(&window.cg_window_id?)?;
                (!pending.verified
                    && window.id.process.pid == pending.prepared.id.process.pid
                    && window.native_space_id == Some(pending.prepared.target_space))
                .then_some(pending.prepared)
            })
            .collect();
        for prepared in late_verified {
            if !self.commit_display_move(prepared) {
                if let Some(pending) = self.pending_display.get_mut(&prepared.cg_window_id) {
                    pending.verified = true;
                }
            }
        }
        for window in &mut windows {
            if let Some(pending) = window
                .cg_window_id
                .and_then(|cg| self.pending_display.get(&cg))
            {
                let move_to = pending.prepared;
                if window.id.process.pid == move_to.id.process.pid
                    && window.native_space_id == Some(move_to.target_space)
                {
                    window.display_id = Some(move_to.target_display);
                    window.mode = move_to.mode;
                }
            }
        }
        // A window observed on a different Space or converted to floating no
        // longer owns the zoom of its original Space.
        self.layout_zoom.retain(|(display, space), zoom| {
            !windows.iter().any(|window| {
                window.id.process.pid == zoom.id.process.pid
                    && window.cg_window_id == Some(zoom.cg_window_id)
                    && (window.display_id != Some(*display)
                        || window.native_space_id.is_some_and(|id| id != *space)
                        || window.mode != tylora_core::registry::ManagementMode::Tiled)
            })
        });
        // AX may report a window after WindowServer moved it away. Its native
        // membership, when known, decides whether it belongs to this Desktop.
        let mut windows: Vec<_> = windows
            .into_iter()
            .filter(|window| {
                if let Some((_, target, committed)) = self
                    .pending_native
                    .values()
                    .find(|(cg, _, _)| window.cg_window_id == Some(*cg))
                {
                    if window.native_space_id != Some(target.0)
                        && committed.elapsed() < Duration::from_secs(10)
                    {
                        return false;
                    }
                }
                window.native_space_id.is_none_or(|id| {
                    self.native_spaces
                        .iter()
                        .find(|space| space.id.0 == id)
                        .is_none_or(|space| space.active)
                })
            })
            .collect();
        self.rule_applied
            .retain(|id| id.process != process || windows.iter().any(|window| window.id == *id));
        for window in &mut windows {
            if self.rule_applied.contains(&window.id) {
                if let Some(mode) = self.coordinators.values().find_map(|coordinator| {
                    coordinator
                        .registry
                        .get(window.id)
                        .map(|record| record.observed.mode)
                }) {
                    window.mode = mode;
                }
                continue;
            }
            let Some(application_id) = window.application_id.as_deref() else {
                continue;
            };
            if let Some(rule) = self
                .window_rules
                .iter()
                .filter(|rule| rule.enabled && rule.application_id == application_id)
                .max_by_key(|rule| rule.priority)
            {
                window.mode = match rule.mode {
                    WindowRuleMode::Floating => tylora_core::registry::ManagementMode::Floating,
                    WindowRuleMode::Tiled => tylora_core::registry::ManagementMode::Tiled,
                };
                self.rule_applied.insert(window.id);
            }
        }
        for window in &windows {
            let resolved: Vec<_> = self
                .pending_native
                .iter()
                .filter(|(_, (cg, _, _))| window.cg_window_id == Some(*cg))
                .map(|(old, (_, target, _))| (*old, *target))
                .collect();
            for (old, target) in resolved {
                self.pending_native.remove(&old);
                if old != window.id || window.native_space_id != Some(target.0) {
                    for engine in self.workspaces.values_mut() {
                        engine.remove(old);
                    }
                }
            }
        }
        let mut changed = false;
        let migrations: Vec<_> = windows
            .iter()
            .filter_map(|window| {
                let target_display = window.display_id?;
                self.workspaces.iter().find_map(|(old_display, engine)| {
                    (*old_display != target_display)
                        .then(|| {
                            engine.owner(window.id).map(|owner| {
                                (
                                    window.id,
                                    *old_display,
                                    target_display,
                                    owner,
                                    window.mode == tylora_core::registry::ManagementMode::Floating
                                        || self.coordinators.get(old_display).is_some_and(
                                            |coordinator| coordinator.is_floating(window.id),
                                        ),
                                )
                            })
                        })
                        .flatten()
                })
            })
            .collect();
        // A coordinator's explicit floating state is authoritative during a
        // cross-display migration. AX snapshots can report the default tiled
        // mode, so carry the source mode onto the target before reconciliation.
        for (id, _, _, _, floating) in &migrations {
            if *floating {
                if let Some(window) = windows.iter_mut().find(|window| window.id == *id) {
                    window.mode = tylora_core::registry::ManagementMode::Floating;
                }
            }
        }
        for (&display, coordinator) in &mut self.coordinators {
            if !self.active.contains(&display) {
                continue;
            }
            let local: Vec<_> = windows
                .iter()
                .filter(|w| w.display_id == Some(display))
                .cloned()
                .collect();
            changed |= coordinator.handle(CoordinatorEvent::Reconciled {
                process,
                windows: local,
            });
            if let Some(engine) = self.workspaces.get_mut(&display) {
                for (_, record) in coordinator.registry.iter() {
                    if record.observed.mode != tylora_core::registry::ManagementMode::Unmanaged
                        && matches!(
                            record.observed.lifecycle,
                            tylora_core::registry::WindowLifecycle::Active
                                | tylora_core::registry::WindowLifecycle::Discovered
                        )
                    {
                        let _ = engine.register(
                            record.observed.id,
                            display,
                            record.observed.mode == tylora_core::registry::ManagementMode::Floating,
                        );
                    }
                }
                for id in engine.owned_windows() {
                    if coordinator.registry.get(id).is_none()
                        && !self.hidden_by_tylora.contains(&id)
                        && !self.pending_hide.contains(&id)
                        && !self.pending_native.contains_key(&id)
                    {
                        engine.remove(id);
                    }
                }
            }
        }
        for (id, old_display, target_display, owner, floating) in migrations {
            if let Some(engine) = self.workspaces.get_mut(&old_display) {
                engine.remove(id);
            }
            if let Some(engine) = self.workspaces.get_mut(&target_display) {
                engine.remove(id);
                let destination = if engine.active(target_display) == Some(owner) {
                    owner
                } else {
                    engine.active(target_display).unwrap_or(owner)
                };
                if engine.register_into(id, destination, floating).is_err() {
                    let _ = engine.register(id, target_display, floating);
                }
            }
        }
        for (id, (source, deadline)) in &self.moving {
            if id.process == process
                && windows
                    .iter()
                    .any(|w| w.id == *id && w.display_id != Some(*source))
            {
                let elapsed = Duration::from_secs(2)
                    .saturating_sub(deadline.saturating_duration_since(Instant::now()));
                debug!(
                    pid = id.process.pid,
                    token = id.token,
                    elapsed_us = elapsed.as_micros(),
                    "cross-display move observed"
                );
            }
        }
        self.moving.retain(|id, (source, _)| {
            id.process != process
                || windows
                    .iter()
                    .any(|w| w.id == *id && w.display_id == Some(*source))
        });
        let completed: Vec<_> = windows
            .iter()
            .filter_map(|window| {
                let cg = window.cg_window_id?;
                let pending = self.pending_display.get(&cg)?;
                let move_to = pending.prepared;
                (window.id.process.pid == move_to.id.process.pid
                    && window.native_space_id == Some(move_to.target_space)
                    && window.display_id == Some(move_to.target_display)
                    && self
                        .coordinators
                        .get(&move_to.target_display)
                        .is_some_and(|coordinator| coordinator.registry.get(window.id).is_some()))
                .then_some((cg, window.id, *pending))
            })
            .collect();
        for (cg, id, pending) in completed {
            self.pending_display.remove(&cg);
            let move_to = pending.prepared;
            if move_to.mode == tylora_core::registry::ManagementMode::Floating {
                self.display_rehomes.push((id, move_to.target_frame));
            }
            if move_to.focus_after_move
                && self.native_spaces.iter().any(|space| {
                    space.display == move_to.target_display
                        && space.id.0 == move_to.target_space
                        && space.active
                })
            {
                self.display_focus.push(id);
            }
            info!(
                cg_window_id = cg,
                elapsed_ms = pending.issued.elapsed().as_millis(),
                "cross-display window reacquired"
            );
        }
        changed
    }

    fn commit_display_move(&mut self, prepared: PreparedDisplayMove) -> bool {
        let source_valid = self
            .coordinators
            .get(&prepared.source_display)
            .and_then(|source| source.registry.get(prepared.id))
            .is_some_and(|record| {
                record.generation == prepared.generation
                    && record.observed.cg_window_id == Some(prepared.cg_window_id)
            });
        if !source_valid {
            // Reconciliation can observe the verified WindowServer move before
            // its worker result reaches the writer. Accept that observed state.
            let target = self
                .coordinators
                .get(&prepared.target_display)
                .and_then(|coordinator| {
                    coordinator.registry.iter().find_map(|(id, record)| {
                        (id.process.pid == prepared.id.process.pid
                            && record.observed.cg_window_id == Some(prepared.cg_window_id)
                            && record.observed.native_space_id == Some(prepared.target_space))
                        .then_some((*id, record.observed.clone()))
                    })
                });
            let Some((id, mut snapshot)) = target else {
                return false;
            };
            let stale_source_ids: Vec<_> = self
                .coordinators
                .get(&prepared.source_display)
                .into_iter()
                .flat_map(|coordinator| coordinator.registry.iter())
                .filter(|(source_id, record)| {
                    source_id.process.pid == prepared.id.process.pid
                        && record.observed.cg_window_id == Some(prepared.cg_window_id)
                })
                .map(|(source_id, _)| *source_id)
                .collect();
            for source_id in stale_source_ids {
                if let Some(coordinator) = self.coordinators.get_mut(&prepared.source_display) {
                    coordinator.handle(CoordinatorEvent::Destroyed(source_id));
                }
                if let Some(engine) = self.workspaces.get_mut(&prepared.source_display) {
                    engine.remove(source_id);
                }
            }
            if let (Some(coordinator), Some(engine)) = (
                self.coordinators.get(&prepared.source_display),
                self.workspaces.get_mut(&prepared.source_display),
            ) {
                if let Some(active) = engine.active(prepared.source_display) {
                    if let Some(workspace) = engine.workspace_mut(active) {
                        workspace.layout = coordinator.layout.clone();
                    }
                }
            }
            snapshot.mode = prepared.mode;
            if let Some(coordinator) = self.coordinators.get_mut(&prepared.target_display) {
                coordinator.handle(CoordinatorEvent::Observed(snapshot));
            }
            if let Some(engine) = self.workspaces.get_mut(&prepared.target_display) {
                engine.remove(id);
                let _ = engine.register(
                    id,
                    prepared.target_display,
                    prepared.mode == tylora_core::registry::ManagementMode::Floating,
                );
                if let (Some(active), Some(coordinator)) = (
                    engine.active(prepared.target_display),
                    self.coordinators.get(&prepared.target_display),
                ) {
                    if let Some(workspace) = engine.workspace_mut(active) {
                        workspace.layout = coordinator.layout.clone();
                    }
                }
            }
            if prepared.mode == tylora_core::registry::ManagementMode::Floating {
                self.display_rehomes.push((id, prepared.target_frame));
            }
            if prepared.focus_after_move
                && self.native_spaces.iter().any(|space| {
                    space.display == prepared.target_display
                        && space.id.0 == prepared.target_space
                        && space.active
                })
            {
                self.display_focus.push(id);
            }
            self.layout_zoom.retain(|(display, space), zoom| {
                !(*display == prepared.source_display
                    && *space == prepared.source_space
                    && zoom.cg_window_id == prepared.cg_window_id)
            });
            return true;
        }
        let Some(source) = self.coordinators.get_mut(&prepared.source_display) else {
            return false;
        };
        source.handle(CoordinatorEvent::Destroyed(prepared.id));
        if let Some(engine) = self.workspaces.get_mut(&prepared.source_display) {
            engine.remove(prepared.id);
            if let Some(active) = engine.active(prepared.source_display) {
                if let Some(workspace) = engine.workspace_mut(active) {
                    workspace.layout = source.layout.clone();
                }
            }
        }
        self.layout_zoom.retain(|(display, space), zoom| {
            !(*display == prepared.source_display
                && *space == prepared.source_space
                && zoom.cg_window_id == prepared.cg_window_id)
        });
        self.pending_display.insert(
            prepared.cg_window_id,
            PendingDisplayMove {
                prepared,
                issued: Instant::now(),
                verified: true,
            },
        );
        self.pending_native.insert(
            prepared.id,
            (
                prepared.cg_window_id,
                tylora_macos::spaces::NativeSpaceId(prepared.target_space),
                Instant::now(),
            ),
        );
        true
    }
    fn commit_native_move(
        &mut self,
        id: ManagedWindowId,
        generation: u64,
        outcome: &tylora_macos::spaces::MoveOutcome,
        target_workspace: Option<u8>,
    ) -> bool {
        if outcome.no_op {
            return false;
        }
        self.layout_zoom.retain(|(_, space), zoom| {
            !(*space == outcome.source.0
                && zoom.cg_window_id == outcome.window_id
                && zoom.id.process.pid == id.process.pid)
        });
        for (display, coordinator) in &mut self.coordinators {
            if coordinator.registry.get(id).is_some_and(|record| {
                record.generation == generation
                    && record.observed.cg_window_id == Some(outcome.window_id)
                    && record.observed.native_space_id == Some(outcome.source.0)
            }) {
                coordinator.handle(CoordinatorEvent::Destroyed(id));
                if let Some(engine) = self.workspaces.get_mut(display) {
                    if let Some(target) = target_workspace {
                        if engine
                            .move_window(id, target, &mut ConfirmedVisibility)
                            .is_ok()
                        {
                            self.pending_native
                                .insert(id, (outcome.window_id, outcome.target, Instant::now()));
                        } else {
                            engine.remove(id);
                        }
                    } else {
                        engine.remove(id);
                    }
                    if let Some(source) = engine.active(*display) {
                        if let Some(ws) = engine.workspace_mut(source) {
                            ws.layout = coordinator.layout.clone();
                        }
                    }
                }
                return true;
            }
        }
        false
    }
    fn handle_process_exited(&mut self, process: ProcessInstance) {
        self.in_flight_display.retain(|id| id.process != process);
        self.pending_display
            .retain(|_, pending| pending.prepared.id.process != process);
        self.layout_zoom
            .retain(|_, zoom| zoom.id.process != process);
        self.pending_native.retain(|id, _| id.process != process);
        for coordinator in self.coordinators.values_mut() {
            coordinator.handle(CoordinatorEvent::ProcessExited(process));
        }
        self.moving.retain(|id, _| id.process != process);
        self.hidden_by_tylora.retain(|id| id.process != process);
        for engine in self.workspaces.values_mut() {
            for id in engine
                .owned_windows()
                .into_iter()
                .filter(|id| id.process == process)
            {
                engine.remove(id);
            }
        }
    }
    fn handle_observation_failed(&mut self, process: ProcessInstance, code: i32) {
        for coordinator in self.coordinators.values_mut() {
            coordinator.handle(CoordinatorEvent::ObservationFailed { process, code });
        }
    }
    fn handle_applied(
        &mut self,
        command: tylora_core::coordinator::ApplyCommand,
        result: Result<tylora_core::Rect, i32>,
    ) {
        if let Some(coordinator) = self.coordinators.values_mut().find(|c| {
            c.registry.get(command.id).is_some_and(|r| {
                r.generation == command.generation && r.pending_apply == Some(command.version)
            })
        }) {
            coordinator.handle(CoordinatorEvent::Applied { command, result });
        } else {
            self.dropped_stale = self.dropped_stale.saturating_add(1);
            debug!(
                pid = command.id.process.pid,
                token = command.id.token,
                "stale AX result dropped by display coordinator"
            );
        }
    }
    fn plan(&mut self) -> Vec<tylora_core::coordinator::ApplyCommand> {
        let mut effects = Vec::new();
        for (&display, coordinator) in &mut self.coordinators {
            if !self.active.contains(&display) {
                continue;
            }
            let active_space = self
                .native_spaces
                .iter()
                .find(|s| s.display == display && s.active)
                .map(|s| s.id.0);
            let zoom = active_space.and_then(|space| self.layout_zoom.get_mut(&(display, space)));
            let override_frame = zoom.and_then(|zoom| {
                let (id, record) = coordinator.registry.iter().find(|(_, record)| {
                    record.observed.cg_window_id == Some(zoom.cg_window_id)
                        && record.observed.id.process.pid == zoom.id.process.pid
                        && record.observed.native_space_id == active_space
                        && record.observed.mode == tylora_core::registry::ManagementMode::Tiled
                })?;
                zoom.id = *id;
                zoom.generation = record.generation;
                let area = coordinator.work_area;
                let gap = if self.zoom_respect_outer_gaps {
                    self.zoom_outer_gap
                } else {
                    0.0
                };
                let gap = gap.min((area.width.min(area.height) - 1.0).max(0.0) / 2.0);
                tylora_core::Rect::new(
                    area.x + gap,
                    area.y + gap,
                    area.width - 2.0 * gap,
                    area.height - 2.0 * gap,
                )
                .ok()
                .map(|frame| (*id, frame))
            });
            effects.extend(coordinator.plan_with_override(override_frame));
        }
        effects
    }
    fn len(&self) -> usize {
        self.coordinators
            .iter()
            .filter(|(id, _)| self.active.contains(id))
            .map(|(_, c)| c.registry.len())
            .sum()
    }
    fn tiled(&self) -> usize {
        self.coordinators
            .iter()
            .filter(|(id, _)| self.active.contains(id))
            .map(|(_, c)| c.layout.len())
            .sum()
    }
    fn expire_moves(&mut self) -> bool {
        let now = Instant::now();
        let expired: Vec<_> = self
            .moving
            .iter()
            .filter(|(_, (_, deadline))| *deadline <= now)
            .map(|(id, (display, _))| (*id, *display))
            .collect();
        for (id, display) in &expired {
            self.moving.remove(id);
            if let Some(source) = self.coordinators.get_mut(display) {
                source.resume(*id);
            }
        }
        !expired.is_empty()
    }
}

pub fn start(display: Option<u32>) -> ExitCode {
    if let Ok(response) = query("status") {
        println!("{response}");
        return ExitCode::SUCCESS;
    }
    if legacy_daemon_running() {
        eprintln!("An older FluxWM daemon is still running. Stop it with the old fluxwm binary before starting Tylora; the two daemons must not manage the same windows.");
        return ExitCode::FAILURE;
    }
    if !prompt_for_accessibility() {
        eprintln!(
            "Accessibility access is not available to this launch of {}. Approve the application named in the macOS prompt (the terminal host may be named instead of tylora) in System Settings > Privacy & Security > Accessibility, restart that application, then retry.",
            std::env::current_exe()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "the tylora executable".into())
        );
        return ExitCode::FAILURE;
    }
    let log = match OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path())
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!("cannot open daemon log: {error}");
            return ExitCode::FAILURE;
        }
    };
    let _ = log.set_permissions(fs::Permissions::from_mode(0o600));
    let stderr = match log.try_clone() {
        Ok(file) => file,
        Err(_) => return ExitCode::FAILURE,
    };
    let mut command = Command::new(std::env::current_exe().expect("current executable"));
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    // Own process group: closing the launching terminal, or a tool killing
    // that terminal's group, must not silently take the daemon down.
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    if let Some(id) = display {
        command.args(["--display", &id.to_string()]);
    }
    match command.spawn() {
        Ok(child) => info!(pid = child.id(), "daemon spawned"),
        Err(error) => {
            eprintln!("cannot start daemon: {error}");
            return ExitCode::FAILURE;
        }
    }
    for _ in 0..30 {
        if let Ok(response) = query("status") {
            println!("{response}");
            return ExitCode::SUCCESS;
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "daemon did not become ready; inspect {}",
        log_path().display()
    );
    ExitCode::FAILURE
}

pub fn request(command: &str) -> ExitCode {
    match query(command) {
        Ok(response) => {
            println!("{response}");
            if response.starts_with('{')
                && serde_json::from_str::<CommandResult>(&response).is_ok_and(|result| !result.ok)
            {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) if command == "status" => {
            println!("stopped ({error})");
            ExitCode::SUCCESS
        }
        Err(error)
            if command == "recover-windows"
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
        {
            if !accessibility_trusted() {
                eprintln!("Accessibility permission is required to recover windows");
                return ExitCode::FAILURE;
            }
            // Opens the WindowServer connection on the main thread, as the
            // daemon does; display queries in the AX worker require it.
            let _ = work_areas();
            let adapter = ManagedAdapter::start();
            let _ = adapter
                .commands
                .try_send(ManagedCommand::RestoreVisibility { request: 1 });
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut outcome = ExitCode::FAILURE;
            while Instant::now() < deadline {
                match adapter.events.recv_timeout(Duration::from_millis(100)) {
                    Ok(ManagedEvent::VisibilityDone {
                        request: 1,
                        result: Ok(()),
                    }) => {
                        println!(
                            "recovery attempted; inspect windows and Dock for unmatched entries"
                        );
                        outcome = ExitCode::SUCCESS;
                        break;
                    }
                    Ok(ManagedEvent::VisibilityDone {
                        request: 1,
                        result: Err(code),
                    }) => {
                        eprintln!("recovery incomplete (AX {code})");
                        break;
                    }
                    Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
            }
            let _ = adapter.commands.try_send(ManagedCommand::Stop);
            outcome
        }
        Err(error) => {
            eprintln!("tylora is not running: {error}");
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn query(command: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response.trim_end().to_owned())
}

struct IpcRequest {
    command: String,
    response: SyncSender<String>,
}

fn ipc_loop(listener: UnixListener, requests: SyncSender<IpcRequest>, stopping: Arc<AtomicBool>) {
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut line = String::new();
                let reply = if BufReader::new(&stream).read_line(&mut line).is_ok() {
                    let (tx, rx) = mpsc::sync_channel(1);
                    match requests.try_send(IpcRequest {
                        command: line.trim().to_owned(),
                        response: tx,
                    }) {
                        Ok(()) => rx
                            .recv_timeout(Duration::from_secs(16))
                            .unwrap_or_else(|_| "coordinator busy\n".into()),
                        Err(_) => "request queue full\n".into(),
                    }
                } else {
                    "invalid request\n".into()
                };
                let _ = stream.write_all(reply.as_bytes());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(error) => {
                warn!(%error, "IPC accept failed");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn bind_socket() -> std::io::Result<UnixListener> {
    let path = socket_path();
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { geteuid() } {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "unsafe socket path",
            ));
        }
        if UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                "daemon already running",
            ));
        }
        fs::remove_file(&path)?;
    }
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn serve(requested_display: Option<u32>) -> ExitCode {
    if legacy_daemon_running() {
        error!("older FluxWM daemon still running; stop it before starting Tylora");
        return ExitCode::FAILURE;
    }
    let config = load_config();
    let areas = work_areas();
    let Some(area) = areas.iter().copied().find(|a| {
        requested_display
            .map(|id| id == a.display.0)
            .unwrap_or(true)
    }) else {
        error!("no matching visible work area; run in a logged-in graphical session and check --display");
        return ExitCode::FAILURE;
    };
    if !accessibility_trusted() {
        error!(
            "Accessibility permission is required for tiling; grant it to the tylora executable"
        );
        return ExitCode::FAILURE;
    }
    let listener = match bind_socket() {
        Ok(listener) => listener,
        Err(error) => {
            error!(%error, "cannot bind daemon socket");
            return ExitCode::FAILURE;
        }
    };
    info!(
        display = area.display.0,
        x = area.frame.x,
        y = area.frame.y,
        width = area.frame.width,
        height = area.frame.height,
        "tiling started"
    );
    let primary = areas
        .iter()
        .find(|a| a.primary)
        .map(|a| a.display)
        .unwrap_or(area.display);
    let (area_tx, area_rx) = mpsc::sync_channel(8);
    let worker = thread::Builder::new()
        .name("tylora-coordinator".into())
        .spawn(move || run_coordinator(areas, primary, listener, area_rx, config));
    let Ok(worker) = worker else {
        error!("cannot spawn coordinator");
        let _ = fs::remove_file(socket_path());
        return ExitCode::FAILURE;
    };
    let display_events = DisplayEvents::start();
    let mut next_area_check = Instant::now() + Duration::from_secs(30);
    let mut display_change_due: Option<Instant> = None;
    while !worker.is_finished() {
        if display_events.events.try_recv().is_ok() {
            display_change_due = Some(Instant::now() + Duration::from_millis(250));
        }
        if Instant::now() >= next_area_check
            || display_change_due.is_some_and(|due| Instant::now() >= due)
        {
            let updated = work_areas();
            let _ = area_tx.try_send(updated);
            next_area_check = Instant::now() + Duration::from_secs(30);
            display_change_due = None;
        }
        thread::sleep(Duration::from_millis(50));
    }
    worker.join().unwrap_or(ExitCode::FAILURE)
}

fn run_coordinator(
    areas: Vec<WorkArea>,
    primary: DisplayId,
    listener: UnixListener,
    area_rx: Receiver<Vec<WorkArea>>,
    mut config: Config,
) -> ExitCode {
    let (ipc_tx, ipc_rx) = mpsc::sync_channel(64);
    let ipc_stopping = Arc::new(AtomicBool::new(false));
    let ipc_flag = ipc_stopping.clone();
    if thread::Builder::new()
        .name("tylora-ipc".into())
        .spawn(move || ipc_loop(listener, ipc_tx, ipc_flag))
        .is_err()
    {
        error!("cannot spawn IPC thread");
        return ExitCode::FAILURE;
    }
    let adapter = ManagedAdapter::start();
    let mut state = DisplayState::new(areas, primary, &config);
    let space_observer = SpaceObserver::start();
    let config_watcher = ConfigWatcher::start(config_path());
    let bindings = active_bindings(&config);
    let mut input = HotkeyManager::start(
        bindings,
        config.mouse.clone(),
        config.focus.follows_mouse && config.focus.mode != FocusMode::Disabled,
    );
    suppress_pointer_focus(!config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled);
    let mut follow_mouse = FocusFollowsMouseManager::start();
    let mut drag: Option<DragSession> = None;
    let mut last_mouse_targets = Instant::now() - Duration::from_secs(1);
    let mut drag_focus_probe: Option<u64> = None;
    let mut next_drag_probe = Instant::now() + Duration::from_millis(500);
    let mut deferred = VecDeque::new();
    let mut pending_commands: HashMap<u64, (StateCommand, Option<SyncSender<String>>, Instant)> =
        HashMap::new();
    let (native_move_tx, native_move_rx) = mpsc::sync_channel::<NativeMoveEvent>(32);
    let mut pending_workspace: Option<PendingWorkspace> = None;
    let mut next_command_id = 0u64;
    let mut follow_confirm: Option<(u64, ManagedWindowId, Instant)> = None;
    let mut pending_move_focus: HashMap<ManagedWindowId, Instant> = HashMap::new();
    let mut move_focus_probe: HashMap<u64, (ManagedWindowId, Instant)> = HashMap::new();
    let mut latest_move_request: Option<u64> = None;
    let mut pointer_focus_pause_until: Option<Instant> = None;
    let mut workspace_switch: u64 = 0;
    // Passive focus tracking for Dwindle (insertion anchor) and Scrolling
    // (active column). Driven only by AX notifications, never by a timer.
    let mut space_slots = if cfg!(test) {
        HashMap::new()
    } else {
        load_space_slots()
    };
    let mut space_focus: SpaceFocusHistory = HashMap::new();
    let mut passive_focus: Option<(u64, Instant)> = None;
    let mut passive_focus_dirty = false;
    let mut passive_focus_counter = 0u64;
    let mut last_passive_focus = Instant::now() - Duration::from_secs(1);
    let mut stopping = false;
    let mut pending_config = None;
    let mut bindings_dirty = false;
    while !stopping {
        if passive_focus.is_some_and(|(_, issued)| issued.elapsed() > Duration::from_secs(2)) {
            passive_focus = None;
        }
        if passive_focus_dirty
            && passive_focus.is_none()
            && drag.is_none()
            && last_passive_focus.elapsed() >= Duration::from_millis(120)
        {
            passive_focus_counter = passive_focus_counter.wrapping_add(1);
            let request = PASSIVE_FOCUS_REQUEST | passive_focus_counter;
            if adapter
                .commands
                .try_send(ManagedCommand::QueryFocused {
                    request,
                    include_cg: false,
                })
                .is_ok()
            {
                passive_focus = Some((request, Instant::now()));
                passive_focus_dirty = false;
                last_passive_focus = Instant::now();
            }
        }
        if pointer_focus_pause_until.is_some_and(|until| Instant::now() >= until) {
            pointer_focus_pause_until = None;
            suppress_pointer_focus(
                !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
            );
        }
        for _ in 0..32 {
            let Ok(event) = native_move_rx.try_recv() else {
                break;
            };
            match event {
                NativeMoveEvent::DisplayMoved {
                    prepared,
                    response,
                    result,
                } => {
                    state.in_flight_display.remove(&prepared.id);
                    let mut prepared = prepared;
                    if prepared.create.is_some() {
                        // Opening Mission Control triggers rescans that can
                        // re-register the window; follow it by CGWindowID.
                        if let Some((id, generation)) = state
                            .coordinators
                            .get(&prepared.source_display)
                            .into_iter()
                            .flat_map(|coordinator| coordinator.registry.iter())
                            .find(|(id, record)| {
                                id.process.pid == prepared.id.process.pid
                                    && record.observed.cg_window_id == Some(prepared.cg_window_id)
                            })
                            .map(|(id, record)| (*id, record.generation))
                        {
                            prepared.id = id;
                            prepared.generation = generation;
                        }
                    }
                    let mut reply = match result {
                        Ok(outcome)
                            if outcome.confirmed
                                && !outcome.no_op
                                && outcome.window_id == prepared.cg_window_id
                                && outcome.source.0 == prepared.source_space
                                && outcome.target.0 == prepared.target_space =>
                        {
                            // A Desktop created for this move may not have reached
                            // the Space observer yet; ask WindowServer directly.
                            let target_known = state.native_spaces.iter().any(|space| {
                                space.display == prepared.target_display
                                    && space.id.0 == prepared.target_space
                            }) || {
                                use tylora_macos::spaces::{
                                    ExperimentalSkyLightBackend, NativeSpaceBackend,
                                };
                                ExperimentalSkyLightBackend::new().list_spaces().is_ok_and(
                                    |spaces| {
                                        spaces.iter().any(|space| {
                                            space.display == prepared.target_display
                                                && space.id.0 == prepared.target_space
                                        })
                                    },
                                )
                            };
                            let committed = state.active.contains(&prepared.target_display)
                                && target_known
                                && state.commit_display_move(prepared);
                            // WindowServer verified the move; bookkeeping that
                            // missed it is repaired by reconciliation.
                            if prepared.create.is_some() {
                                if let Some(workspace) = prepared.bind_workspace {
                                    let current = pending_config.as_ref().unwrap_or(&config);
                                    if let Some(updated) = bind_space_to_workspace(
                                        current,
                                        workspace,
                                        prepared.target_space,
                                    ) {
                                        pending_config = Some(updated);
                                    }
                                }
                            }
                            let target_inactive = !state.native_spaces.iter().any(|space| {
                                space.display == prepared.target_display
                                    && space.id.0 == prepared.target_space
                                    && space.active
                            });
                            if target_inactive {
                                // Show the window: switch the target display to its
                                // Desktop, then return the pointer unless focus follows.
                                workspace_switch = workspace_switch.wrapping_add(1).max(1);
                                let switch = workspace_switch;
                                follow_mouse.cancel();
                                suppress_pointer_focus(true);
                                pointer_focus_pause_until =
                                    Some(Instant::now() + Duration::from_secs(13));
                                let reconcile = adapter.commands.clone();
                                let focus_sender = native_move_tx.clone();
                                thread::spawn(move || {
                                    let pointer = tylora_macos::current_pointer();
                                    let target =
                                        tylora_macos::spaces::NativeSpaceId(prepared.target_space);
                                    // Right after Mission Control created the Desktop the
                                    // Dock may still ignore the swipe; let it settle and retry.
                                    if prepared.create.is_some() {
                                        thread::sleep(Duration::from_millis(400));
                                    }
                                    let mut result =
                                        focus_workspace_space(prepared.target_display, target);
                                    for _ in 0..2 {
                                        if result.is_ok() {
                                            break;
                                        }
                                        thread::sleep(Duration::from_millis(400));
                                        result =
                                            focus_workspace_space(prepared.target_display, target);
                                    }
                                    if !prepared.focus_after_move {
                                        if let Some(point) = pointer {
                                            tylora_macos::spaces::warp_cursor_to(point);
                                        }
                                    }
                                    match result {
                                        Ok(_) => {
                                            info!(
                                                space_id = prepared.target_space,
                                                "target Desktop shown after display move"
                                            );
                                            let _ = reconcile.try_send(ManagedCommand::Reconcile);
                                        }
                                        Err(error) => {
                                            warn!(%error, "cannot show target Desktop after display move")
                                        }
                                    }
                                    let _ = focus_sender.send(NativeMoveEvent::WorkspaceSwitched {
                                        switch,
                                        focus: None,
                                        space: prepared
                                            .focus_after_move
                                            .then_some(prepared.target_space),
                                    });
                                });
                            }
                            if committed {
                                let mut focus = "skipped".to_string();
                                if !prepared.focus_after_move
                                    && state.native_spaces.iter().any(|space| {
                                        space.display == prepared.source_display
                                            && space.id.0 == prepared.source_space
                                            && space.active
                                    })
                                {
                                    let remaining = state
                                        .coordinators
                                        .get(&prepared.source_display)
                                        .and_then(|coordinator| {
                                            coordinator.layout.windows().first().copied().or_else(|| {
                                                coordinator.registry.iter()
                                                    .filter(|(_, record)| record.observed.native_space_id == Some(prepared.source_space)
                                                        && record.observed.mode != tylora_core::registry::ManagementMode::Unmanaged
                                                        && matches!(record.observed.lifecycle, tylora_core::registry::WindowLifecycle::Active | tylora_core::registry::WindowLifecycle::Discovered))
                                                    .map(|(id, _)| *id).min()
                                            })
                                        });
                                    if let Some(id) = remaining {
                                        focus = if adapter
                                            .commands
                                            .try_send(ManagedCommand::Focus(id))
                                            .is_ok()
                                        {
                                            pending_move_focus.insert(id, Instant::now());
                                            "source_requested"
                                        } else {
                                            "source_queue_full"
                                        }
                                        .into();
                                    }
                                } else if prepared.focus_after_move {
                                    focus = if state.native_spaces.iter().any(|space| {
                                        space.display == prepared.target_display
                                            && space.id.0 == prepared.target_space
                                            && space.active
                                    }) {
                                        "target_pending_reacquisition"
                                    } else {
                                        "deferred_until_target_space_active"
                                    }
                                    .into();
                                }
                                let target_pending =
                                    state.pending_display.contains_key(&prepared.cg_window_id);
                                let mut result = CommandResult::success(if target_pending {
                                    "native display transfer verified; BSP target reconciliation pending"
                                } else {
                                    "native display transfer and BSP reconciliation verified"
                                });
                                result.window = Some(prepared.id);
                                result.display = Some(prepared.target_display);
                                result.transfer = Some(Box::new(DisplayTransferStatus {
                                    cg_window_id: prepared.cg_window_id,
                                    source_display: prepared.source_display,
                                    target_display: prepared.target_display,
                                    source_space_id: prepared.source_space,
                                    target_space_id: prepared.target_space,
                                    native_transfer: "verified".into(),
                                    bsp_update: if target_pending {
                                        "source_committed_target_pending"
                                    } else {
                                        "reconciled"
                                    }
                                    .into(),
                                    focus,
                                    elapsed_ms: outcome.operation_ms + outcome.verification_ms,
                                }));
                                info!(
                                    cg_window_id = prepared.cg_window_id,
                                    source = prepared.source_display.0,
                                    target = prepared.target_display.0,
                                    elapsed_ms = outcome.operation_ms + outcome.verification_ms,
                                    "native cross-display transfer verified"
                                );
                                result
                            } else {
                                CommandResult::failure(CommandError::DisplayNotFound, "native transfer verified, but display state changed; reconciling")
                            }
                        }
                        Ok(_) => CommandResult::failure(
                            CommandError::Unsupported,
                            "native transfer result did not match the requested Spaces",
                        ),
                        Err(error) => {
                            if matches!(error, tylora_macos::spaces::SpaceError::MoveTimeout { .. })
                            {
                                state.pending_display.insert(
                                    prepared.cg_window_id,
                                    PendingDisplayMove {
                                        prepared,
                                        issued: Instant::now(),
                                        verified: false,
                                    },
                                );
                            }
                            CommandResult::failure(CommandError::Unsupported, error.to_string())
                        }
                    };
                    if reply.transfer.is_none() {
                        reply.window = Some(prepared.id);
                        reply.display = Some(prepared.target_display);
                        reply.transfer = Some(Box::new(DisplayTransferStatus {
                            cg_window_id: prepared.cg_window_id,
                            source_display: prepared.source_display,
                            target_display: prepared.target_display,
                            source_space_id: prepared.source_space,
                            target_space_id: prepared.target_space,
                            native_transfer: if state
                                .pending_display
                                .get(&prepared.cg_window_id)
                                .is_some_and(|pending| !pending.verified)
                            {
                                "indeterminate"
                            } else {
                                "failed_or_uncommitted"
                            }
                            .into(),
                            bsp_update: "reconciliation_requested".into(),
                            focus: "skipped".into(),
                            elapsed_ms: prepared.issued.elapsed().as_millis(),
                        }));
                    }
                    if !reply.ok {
                        warn!(cg_window_id = prepared.cg_window_id, message = %reply.message, "cross-display move failed or indeterminate");
                    }
                    tylora_macos::spaces::record_move_diagnostic(
                        &serde_json::json!({"window_id":prepared.cg_window_id,"source_display":prepared.source_display,"target_display":prepared.target_display,"source_space_id":prepared.source_space,"target_space_id":prepared.target_space,"verified":reply.ok,"stage":reply.transfer.as_ref().map(|status| status.native_transfer.as_str()).unwrap_or("failed_or_indeterminate"),"error":reply.error,"elapsed_ms":reply.transfer.as_ref().map(|status| status.elapsed_ms)}),
                    );
                    let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
                    if let Some(response) = response {
                        let _ = response.try_send(command_json(reply));
                    }
                }
                NativeMoveEvent::Moved {
                    request,
                    id,
                    generation,
                    display,
                    issued,
                    response,
                    result,
                } => match result {
                    Ok(outcome) => {
                        tylora_macos::spaces::record_move_diagnostic(
                            &serde_json::json!({"window_id":outcome.window_id,"pid":id.process.pid,"source_space_id":outcome.source,"target_space_id":outcome.target,"backend":outcome.backend,"stage":outcome.stage,"operation_ms":outcome.operation_ms,"verification_ms":outcome.verification_ms,"verified":outcome.confirmed,"no_op":outcome.no_op,"error":null}),
                        );
                        let target_workspace = config
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.binds(outcome.target.0))
                            .map(|workspace| workspace.id);
                        let committed =
                            state.commit_native_move(id, generation, &outcome, target_workspace);
                        info!(
                            window_id = outcome.window_id,
                            source = outcome.source.0,
                            target = outcome.target.0,
                            operation_ms = outcome.operation_ms,
                            verification_ms = outcome.verification_ms,
                            committed,
                            "native window move confirmed"
                        );
                        if let Some(response) = response {
                            let _ = response.try_send(command_json(CommandResult::success(
                                "native move verified",
                            )));
                        } else if latest_move_request == Some(request)
                            && issued.elapsed() < Duration::from_secs(3)
                            && state.native_spaces.iter().any(|space| {
                                space.id == outcome.source
                                    && space.display == display
                                    && space.active
                            })
                        {
                            pointer_focus_pause_until =
                                Some(Instant::now() + Duration::from_secs(12));
                            let sender = native_move_tx.clone();
                            thread::spawn(move || {
                                let result = tylora_macos::spaces::NativeSpaceSwitcher::focus_id(
                                    display,
                                    outcome.target,
                                )
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                                let _ = sender.send(NativeMoveEvent::Switched {
                                    request,
                                    target: outcome.target,
                                    result,
                                });
                            });
                        } else {
                            info!(
                                request,
                                age_ms = issued.elapsed().as_millis(),
                                source = outcome.source.0,
                                target = outcome.target.0,
                                "window moved; Space follow skipped after newer input or Space change"
                            );
                        }
                    }
                    Err(error) => {
                        tylora_macos::spaces::record_move_diagnostic(
                            &serde_json::json!({"pid":id.process.pid,"backend":"SkyLight bridged move","stage":"failed_or_indeterminate","verified":false,"error":error}),
                        );
                        warn!(%error, pid = id.process.pid, token = id.token,
                        "native window move failed or remains indeterminate; reconciling");
                        if let Some(response) = response {
                            let _ = response.try_send(command_json(CommandResult::failure(
                                CommandError::Unsupported,
                                error,
                            )));
                        }
                    }
                },
                NativeMoveEvent::Switched {
                    request,
                    target,
                    result,
                } => {
                    if latest_move_request == Some(request) {
                        latest_move_request = None;
                        pointer_focus_pause_until = None;
                        suppress_pointer_focus(
                            !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                        );
                        match result {
                            Ok(()) => info!(target = target.0, "moved window Space focused"),
                            Err(error) => {
                                warn!(%error, target = target.0, "moved window Space focus failed")
                            }
                        }
                    }
                }
                NativeMoveEvent::WorkspaceSwitched {
                    switch,
                    focus,
                    space,
                } => {
                    if let Some(space) = space {
                        space_focus.insert(space, Instant::now());
                    }
                    if switch != 0
                        && switch == workspace_switch
                        && pointer_focus_pause_until.is_some()
                    {
                        // Let the Space observer and reconciliation catch up before
                        // pointer focus may raise a window again.
                        follow_mouse.cancel();
                        pointer_focus_pause_until =
                            Some(Instant::now() + WORKSPACE_SWITCH_POINTER_SETTLE);
                    }
                    if let Some(display) = focus {
                        let result = focus_display(&state, &adapter, display);
                        if !result.ok {
                            let display_id = display.0;
                            info!(display_id, message = %result.message, "workspace display focus skipped");
                        }
                    }
                }
            }
            let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
        }
        while let Ok(updated) = config_watcher.updates.try_recv() {
            pending_config = Some(updated);
        }
        while let Ok(spaces) = space_observer.changes.try_recv() {
            let old_ids: Vec<_> = state.native_spaces.iter().map(|space| space.id).collect();
            let new_ids: Vec<_> = spaces.iter().map(|space| space.id).collect();
            bindings_dirty |= old_ids != new_ids;
            let old_active: Vec<_> = state
                .native_spaces
                .iter()
                .filter(|space| space.active)
                .map(|space| (space.display, space.id))
                .collect();
            let new_active: Vec<_> = spaces
                .iter()
                .filter(|space| space.active)
                .map(|space| (space.display, space.id))
                .collect();
            if old_active != new_active || state.native_spaces.len() != spaces.len() {
                follow_mouse.cancel();
                info!(
                    ?new_active,
                    count = spaces.len(),
                    "native Spaces changed; reconciling AX windows"
                );
                let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
            }
            state.apply_native_spaces(&old_active, &new_active, &config);
            state.layout_zoom.retain(|(display, space), _| {
                spaces
                    .iter()
                    .any(|native| native.display == *display && native.id.0 == *space)
            });
            state.space_layouts.retain(|(display, space), _| {
                spaces
                    .iter()
                    .any(|native| native.display == *display && native.id.0 == *space)
            });
            state.native_spaces = spaces;
            let before = space_slots.clone();
            let areas = &state.areas;
            let current = pending_config.as_ref().unwrap_or(&config);
            let rebinds = plan_space_rebinds(
                current,
                &state.native_spaces,
                |display| areas.get(&display).and_then(|area| area.stable_uuid),
                &mut space_slots,
            );
            if space_slots != before {
                save_space_slots(&space_slots);
            }
            if !rebinds.is_empty() {
                if let Some(updated) = apply_space_rebinds(current, &rebinds) {
                    pending_config = Some(updated);
                }
            }
        }
        if drag.is_none() && (pending_config.is_some() || bindings_dirty) {
            if let Some(updated) = pending_config.take() {
                suppress_pointer_focus(true);
                follow_mouse.cancel();
                state.apply_live_config(&updated);
                config = updated;
                suppress_pointer_focus(
                    !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                );
                info!("configuration hot reloaded; gaps, workspaces, hotkeys and mouse settings applied");
                let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
            }
            input.stop();
            input = HotkeyManager::start(
                active_bindings(&config),
                config.mouse.clone(),
                config.focus.follows_mouse && config.focus.mode != FocusMode::Disabled,
            );
            bindings_dirty = false;
        }
        for _ in 0..64 {
            match adapter.events.try_recv() {
                Ok(ManagedEvent::Reconciled { process, windows }) => {
                    // Focus is tracked for every layout: Dwindle/Scrolling use
                    // it, and so does choosing between a workspace's Spaces.
                    passive_focus_dirty = true;
                    let before = state.len();
                    let missing_zoom = state.layout_zoom.iter().any(|((display, space), zoom)| {
                        zoom.id.process == process
                            && state.native_spaces.iter().any(|native| {
                                native.display == *display && native.id.0 == *space && native.active
                            })
                            && !windows
                                .iter()
                                .any(|window| window.cg_window_id == Some(zoom.cg_window_id))
                    });
                    if missing_zoom {
                        if let Ok(all_windows) = tylora_macos::discover_all_cg_windows() {
                            state.layout_zoom.retain(|_, zoom| {
                                zoom.id.process != process
                                    || all_windows.iter().any(|window| {
                                        window.id.0 == zoom.cg_window_id
                                            && window.owner_pid == process.pid
                                    })
                            });
                        }
                    }
                    let changed = state.handle_reconciled(process, windows);
                    for (id, target) in std::mem::take(&mut state.display_rehomes) {
                        let _ = adapter
                            .commands
                            .try_send(ManagedCommand::Rehome { id, target });
                    }
                    for id in std::mem::take(&mut state.display_focus) {
                        if adapter.commands.try_send(ManagedCommand::Focus(id)).is_ok() {
                            pending_move_focus.insert(id, Instant::now());
                        }
                    }
                    if changed {
                        info!(
                            pid = process.pid,
                            before,
                            after = state.len(),
                            "registry reconciled"
                        );
                    }
                }
                Ok(ManagedEvent::ProcessExited(process)) => {
                    state.handle_process_exited(process);
                    info!(pid = process.pid, "process exited; windows removed");
                }
                Ok(ManagedEvent::ObservationFailed { process, code }) => {
                    state.handle_observation_failed(process, code);
                    warn!(
                        pid = process.pid,
                        code, "AX observation failed; registry preserved"
                    );
                }
                Ok(ManagedEvent::Applied { command, result }) => {
                    let zoom_confirmed = result
                        .as_ref()
                        .is_ok_and(|actual| actual.near(command.target, 1.0))
                        && state.layout_zoom.iter().any(|((display, space), zoom)| {
                            state.native_spaces.iter().any(|native| {
                                native.display == *display && native.id.0 == *space && native.active
                            }) && zoom.id == command.id
                                && zoom.generation == command.generation
                                && state
                                    .coordinators
                                    .get(display)
                                    .and_then(|coordinator| coordinator.registry.get(command.id))
                                    .is_some_and(|record| {
                                        record.pending_apply == Some(command.version)
                                    })
                                && state.coordinators.get(display).is_some_and(|coordinator| {
                                    coordinator.version == command.version
                                })
                        });
                    match result {
                        Err(code) => warn!(
                            pid = command.id.process.pid,
                            token = command.id.token,
                            code,
                            "AX geometry apply failed"
                        ),
                        Ok(actual) if !actual.near(command.target, 1.0) => warn!(
                            pid = command.id.process.pid,
                            token = command.id.token,
                            target = ?command.target,
                            actual = ?actual,
                            "AX geometry did not reach target"
                        ),
                        Ok(_) => {}
                    }
                    state.handle_applied(command, result);
                    if zoom_confirmed {
                        let _ = adapter.commands.try_send(ManagedCommand::Focus(command.id));
                    }
                }
                Ok(ManagedEvent::Focused {
                    request,
                    window,
                    cg_window,
                    focused_pid,
                }) => {
                    if passive_focus.is_some_and(|(probe, _)| probe == request) {
                        passive_focus = None;
                        if let Some(id) = window {
                            note_space_focus(&state, id, &mut space_focus);
                            let origin = if follow_mouse
                                .requested
                                .is_some_and(|(requested, _)| requested == id)
                            {
                                FocusOrigin::Pointer
                            } else {
                                FocusOrigin::System
                            };
                            for engine in state.workspaces.values_mut() {
                                engine.set_focused(id);
                            }
                            if let Some(coordinator) = state.coordinator_for_active(id) {
                                if coordinator.focus_hint(id, origin) {
                                    debug!(
                                        pid = id.process.pid,
                                        token = id.token,
                                        ?origin,
                                        "layout followed passive focus"
                                    );
                                }
                            }
                        }
                        continue;
                    }
                    if let Some((expected, issued)) = move_focus_probe.remove(&request) {
                        if window == Some(expected) {
                            info!(
                                pid = expected.process.pid,
                                token = expected.token,
                                elapsed_ms = issued.elapsed().as_millis(),
                                "display move focus confirmed by AX"
                            );
                        } else {
                            warn!(
                                pid = expected.process.pid,
                                token = expected.token,
                                ?window,
                                "display move focus was not confirmed"
                            );
                        }
                        continue;
                    }
                    if let Some((confirm_request, expected, issued)) = follow_confirm {
                        if request == confirm_request {
                            follow_confirm = None;
                            if window == Some(expected) {
                                follow_mouse.confirmations += 1;
                                follow_mouse.request_to_confirm_total_us +=
                                    issued.elapsed().as_micros();
                                debug!(
                                    request_to_confirm_us = issued.elapsed().as_micros(),
                                    "pointer focus confirmed"
                                );
                            } else {
                                follow_mouse.failures += 1;
                                follow_mouse.requested = None;
                                follow_mouse.applied_for = None;
                            }
                            continue;
                        }
                    }
                    if drag_focus_probe == Some(request) {
                        drag_focus_probe = None;
                        if let Some(session) = drag.as_mut() {
                            if window != Some(session.start.id) {
                                let mut cancelled = session.start;
                                cancelled.phase = MousePhase::Cancel;
                                session.update(cancelled, &mut state, &adapter, &config);
                            }
                        }
                        continue;
                    }
                    if let Some((command, response, issued)) = pending_commands.remove(&request) {
                        if let Some(id) = window {
                            note_space_focus(&state, id, &mut space_focus);
                            for engine in state.workspaces.values_mut() {
                                engine.set_focused(id);
                            }
                        }
                        if matches!(
                            command,
                            StateCommand::MoveWindowToDisplay { .. }
                                | StateCommand::MoveWindowToNextDisplay { .. }
                                | StateCommand::MoveWindowToPreviousDisplay { .. }
                        ) {
                            let resolved = window.or_else(|| {
                                let (cg, pid) = cg_window?;
                                state
                                    .coordinators
                                    .values()
                                    .flat_map(|coordinator| coordinator.registry.iter())
                                    .find_map(|(id, record)| {
                                        (id.process.pid == pid
                                            && record.observed.cg_window_id == Some(cg))
                                        .then_some(*id)
                                    })
                            });
                            match prepare_display_move(&state, &config, &command, resolved) {
                                Ok(prepared) => {
                                    follow_mouse.cancel();
                                    suppress_pointer_focus(true);
                                    pointer_focus_pause_until =
                                        Some(Instant::now() + Duration::from_secs(5));
                                    state.in_flight_display.insert(prepared.id);
                                    let sender = native_move_tx.clone();
                                    thread::spawn(move || {
                                        let mut prepared = prepared;
                                        if let Some((desktop, missing)) = prepared.create {
                                            match create_target_desktop(
                                                prepared.target_display,
                                                desktop,
                                                missing,
                                            ) {
                                                Ok(space) => prepared.target_space = space,
                                                Err(error) => {
                                                    // Fall back to the active Space chosen at prepare time.
                                                    warn!(%error, "cannot create Desktop; moving to the active Space");
                                                    prepared.create = None;
                                                    prepared.bind_workspace = None;
                                                }
                                            }
                                        }
                                        let result = tylora_macos::spaces::NativeWindowSpaceMover::move_window_to_space(
                                            prepared.cg_window_id,
                                            tylora_macos::spaces::NativeSpaceId(prepared.target_space),
                                        );
                                        let _ = sender.send(NativeMoveEvent::DisplayMoved {
                                            prepared,
                                            response,
                                            result,
                                        });
                                    });
                                }
                                Err(result) => {
                                    if let Some(response) = response {
                                        let _ = response.try_send(command_json(result));
                                    }
                                }
                            }
                            continue;
                        }
                        if matches!(
                            command,
                            StateCommand::MoveWindowToWorkspace { .. }
                                | StateCommand::MoveWindowToNamedWorkspace { .. }
                        ) {
                            let window_display = window
                                .and_then(|id| {
                                    state
                                        .coordinators
                                        .iter()
                                        .find(|(_, c)| c.registry.get(id).is_some())
                                        .map(|(display, _)| *display)
                                })
                                .or_else(|| {
                                    let (cg, pid) = cg_window?;
                                    state
                                        .coordinators
                                        .iter()
                                        .find(|(_, c)| {
                                            c.registry.iter().any(|(id, record)| {
                                                id.process.pid == pid
                                                    && record.observed.cg_window_id == Some(cg)
                                            })
                                        })
                                        .map(|(display, _)| *display)
                                });
                            let target = resolve_move_target(
                                &config,
                                &command,
                                window_display,
                                &space_focus,
                            );
                            let identity = if let Some((cg, pid)) = cg_window {
                                state.coordinators.values().find_map(|coordinator| {
                                    coordinator.registry.iter().find_map(|(id, record)| {
                                        (id.process.pid == pid
                                            && (record.observed.cg_window_id == Some(cg)
                                                || (window == Some(*id)
                                                    && record.observed.cg_window_id.is_none())))
                                        .then_some((*id, record.generation, cg))
                                    })
                                })
                            } else {
                                window.and_then(|id| {
                                    state.coordinators.values().find_map(|coordinator| {
                                        coordinator.registry.get(id).and_then(|record| {
                                            record
                                                .observed
                                                .cg_window_id
                                                .map(|cg| (id, record.generation, cg))
                                        })
                                    })
                                })
                            }
                            .or_else(|| {
                                if cg_window.is_some() {
                                    return None;
                                }
                                let pid = focused_pid?;
                                let candidates: Vec<_> =
                                    state
                                        .coordinators
                                        .values()
                                        .flat_map(|coordinator| coordinator.registry.iter())
                                        .filter_map(|(id, record)| {
                                            let cg = record.observed.cg_window_id?;
                                            (id.process.pid == pid
                                                && record.observed.native_space_id.is_some_and(
                                                    |native| {
                                                        state.native_spaces.iter().any(|space| {
                                                            space.id.0 == native && space.active
                                                        })
                                                    },
                                                ))
                                            .then_some((*id, record.generation, cg))
                                        })
                                        .collect();
                                (candidates.len() == 1).then(|| candidates[0])
                            });
                            match (target, identity) {
                                (Ok((display, target)), Some((id, generation, cg))) => {
                                    let sender = native_move_tx.clone();
                                    thread::spawn(move || {
                                        let result = tylora_macos::spaces::NativeWindowSpaceMover::move_window_to_space(cg, target).map_err(|e| e.to_string());
                                        let _ = sender.try_send(NativeMoveEvent::Moved {
                                            request,
                                            id,
                                            generation,
                                            display,
                                            issued,
                                            response,
                                            result,
                                        });
                                    });
                                }
                                (Err(error), _) => {
                                    warn!(%error, "native move target unresolved");
                                    if let Some(response) = response {
                                        let _ = response.try_send(command_json(
                                            CommandResult::failure(
                                                CommandError::WorkspaceNotFound,
                                                error,
                                            ),
                                        ));
                                    }
                                }
                                (_, None) => {
                                    warn!(
                                        ?window,
                                        ?cg_window,
                                        ?focused_pid,
                                        "focused window could not be matched uniquely to a managed CGWindowID"
                                    );
                                    if let Some(response) = response {
                                        let _ = response.try_send(command_json(
                                            CommandResult::failure(
                                                CommandError::NoFocusedWindow,
                                                "focused window could not be matched uniquely to a managed CGWindowID",
                                            ),
                                        ));
                                    }
                                }
                            }
                            continue;
                        }
                        if matches!(
                            command,
                            StateCommand::FocusWorkspace { .. }
                                | StateCommand::MoveWindowToWorkspace { .. }
                        ) {
                            if let Some(response) = response {
                                let _ = response.try_send(command_json(CommandResult::failure(
                                    CommandError::VisibilityUnsupported,
                                    "virtual workspaces were retired; use `tylora space` for native macOS Desktops",
                                )));
                            }
                            continue;
                        }
                        let result =
                            execute_state_command(&mut state, &adapter, &config, &command, window);
                        debug!(
                            elapsed_us = issued.elapsed().as_micros(),
                            ?command,
                            ok = result.ok,
                            "command handled"
                        );
                        if let Some(response) = response {
                            let _ = response.try_send(command_json(result));
                        }
                    }
                }
                Ok(ManagedEvent::FocusFailed { window, code }) => {
                    pending_move_focus.remove(&window);
                    if follow_mouse.requested.is_some_and(|(id, _)| id == window) {
                        follow_mouse.requested = None;
                        follow_mouse.applied_for = None;
                        follow_mouse.failures += 1;
                    }
                    warn!(
                        pid = window.process.pid,
                        token = window.token,
                        code,
                        "AX focus failed"
                    );
                }
                Ok(ManagedEvent::FocusApplied(window)) => {
                    note_space_focus(&state, window, &mut space_focus);
                    if let Some(issued) = pending_move_focus.remove(&window) {
                        next_command_id = next_command_id.wrapping_add(1);
                        if adapter
                            .commands
                            .try_send(ManagedCommand::QueryFocused {
                                request: next_command_id,
                                include_cg: false,
                            })
                            .is_ok()
                        {
                            move_focus_probe.insert(next_command_id, (window, issued));
                        } else {
                            warn!(
                                pid = window.process.pid,
                                token = window.token,
                                "display move focus confirmation queue full"
                            );
                        }
                    }
                    if let Some((id, issued)) = follow_mouse.requested.filter(|(id, issued)| {
                        *id == window && follow_mouse.applied_for != Some(*issued)
                    }) {
                        follow_mouse.applied_for = Some(issued);
                        follow_mouse.applied += 1;
                        follow_mouse.request_to_applied_total_us += issued.elapsed().as_micros();
                        debug!(
                            pid = id.process.pid,
                            token = id.token,
                            requested_to_applied_us = issued.elapsed().as_micros(),
                            "pointer focus applied"
                        );
                        next_command_id = next_command_id.wrapping_add(1);
                        if adapter
                            .commands
                            .try_send(ManagedCommand::QueryFocused {
                                request: next_command_id,
                                include_cg: false,
                            })
                            .is_ok()
                        {
                            follow_confirm = Some((next_command_id, id, issued));
                        } else {
                            follow_mouse.failures += 1;
                            follow_mouse.requested = None;
                            follow_mouse.applied_for = None;
                        }
                    }
                }
                Ok(ManagedEvent::DragApplied { id, result }) => {
                    if let Some(session) = drag.as_mut().filter(|session| session.start.id == id) {
                        session.ax_pending = false;
                        if let Err(code) = result {
                            warn!(
                                pid = id.process.pid,
                                token = id.token,
                                code,
                                "AX drag frame failed"
                            );
                            session.finished = true;
                        }
                    }
                }
                Ok(ManagedEvent::VisibilityDone { request, result }) => {
                    if let Some(pending) = pending_workspace.take() {
                        if pending.request == request {
                            state.pending_hide.clear();
                            if pending.timed_out
                                && !matches!(pending.action, WorkspaceAction::Recover)
                            {
                                warn!("late workspace result; restoring windows before accepting another transition");
                                let _ = adapter
                                    .commands
                                    .try_send(ManagedCommand::RestoreVisibility { request: 0 });
                                pending_workspace = Some(PendingWorkspace {
                                    request: 0,
                                    action: WorkspaceAction::Recover,
                                    response: None,
                                    started: Instant::now(),
                                    timed_out: false,
                                });
                                continue;
                            }
                            let result = match result {
                                Ok(()) => {
                                    let focus = apply_workspace_action(&mut state, pending.action);
                                    if let Some(id) = focus.window {
                                        let _ =
                                            adapter.commands.try_send(ManagedCommand::Focus(id));
                                    }
                                    let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
                                    debug!(
                                        elapsed_us = pending.started.elapsed().as_micros(),
                                        "workspace transition finished"
                                    );
                                    focus
                                }
                                Err(code) => {
                                    if matches!(pending.action, WorkspaceAction::Recover) {
                                        warn!(
                                            code,
                                            "some Tylora-minimized windows could not be restored"
                                        );
                                        CommandResult::failure(CommandError::AccessibilityUnavailable, format!("recovery incomplete (AX {code}); retry recover-windows"))
                                    } else {
                                        warn!(code, "workspace visibility transaction rejected");
                                        if code == VISIBILITY_ROLLBACK_FAILED {
                                            let _ = adapter.commands.try_send(
                                                ManagedCommand::RestoreVisibility { request: 0 },
                                            );
                                            pending_workspace = Some(PendingWorkspace {
                                                request: 0,
                                                action: WorkspaceAction::Recover,
                                                response: None,
                                                started: Instant::now(),
                                                timed_out: false,
                                            });
                                        }
                                        CommandResult::failure(CommandError::VisibilityUnsupported, format!("workspace visibility failed (AX {code}); previous workspace preserved"))
                                    }
                                }
                            };
                            if let Some(response) = pending.response {
                                let _ = response.try_send(command_json(result));
                            }
                        } else {
                            pending_workspace = Some(pending);
                        }
                    } else if request == 0 && result.is_ok() {
                        state.hidden_by_tylora.clear();
                    }
                }
                Ok(ManagedEvent::VisibilityGone(id)) => {
                    state.hidden_by_tylora.remove(&id);
                    state.pending_hide.remove(&id);
                    for engine in state.workspaces.values_mut() {
                        engine.remove(id);
                    }
                }
                Ok(ManagedEvent::QueueOverflow) => {
                    warn!("observer queue overflow; full reconciliation requested")
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    error!("AX worker stopped");
                    stopping = true;
                    break;
                }
            }
        }
        for _ in 0..32 {
            match input.events.try_recv() {
                Ok(HotkeyEvent::Command {
                    command,
                    pressed_at,
                }) => {
                    tylora_macos::spaces::NativeSpaceSwitcher::cancel_pending();
                    latest_move_request = None;
                    pointer_focus_pause_until = None;
                    suppress_pointer_focus(
                        !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                    );
                    if matches!(
                        command,
                        StateCommand::FocusWorkspace { .. }
                            | StateCommand::FocusNamedWorkspace { .. }
                            | StateCommand::FocusNextWorkspace
                            | StateCommand::FocusPreviousWorkspace
                    ) {
                        // Pointer focus must not raise a window from the old Space while
                        // the switch animates; WindowServer would switch straight back.
                        workspace_switch = workspace_switch.wrapping_add(1).max(1);
                        let switch = workspace_switch;
                        follow_mouse.cancel();
                        suppress_pointer_focus(true);
                        pointer_focus_pause_until = Some(Instant::now() + Duration::from_secs(13));
                        let reconcile = adapter.commands.clone();
                        let focus_sender = native_move_tx.clone();
                        let configured = config.clone();
                        let history = space_focus.clone();
                        thread::spawn(move || {
                            let mut focus = None;
                            let mut space = None;
                            match resolve_focus_target(&configured, &command, &history) {
                                Ok((display, target)) => {
                                    match focus_workspace_space(display, target) {
                                        Ok((outcome, needs_focus)) => {
                                            info!(
                                                space_id = target.0,
                                                display_id = outcome.display.0,
                                                elapsed_ms = outcome.elapsed_ms,
                                                "native workspace focus confirmed"
                                            );
                                            let _ = reconcile.try_send(ManagedCommand::Reconcile);
                                            focus = needs_focus.then_some(display);
                                            space = Some(target.0);
                                        }
                                        Err(error) => {
                                            warn!(%error, target = target.0, "native workspace focus failed")
                                        }
                                    }
                                }
                                Err(error) => warn!(%error, "native workspace binding unresolved"),
                            }
                            let _ = focus_sender.send(NativeMoveEvent::WorkspaceSwitched {
                                switch,
                                focus,
                                space,
                            });
                        });
                        continue;
                    }
                    next_command_id = next_command_id.wrapping_add(1);
                    let moving_window = matches!(
                        command,
                        StateCommand::MoveWindowToWorkspace { .. }
                            | StateCommand::MoveWindowToNamedWorkspace { .. }
                            | StateCommand::MoveWindowToDisplay { .. }
                            | StateCommand::MoveWindowToNextDisplay { .. }
                            | StateCommand::MoveWindowToPreviousDisplay { .. }
                    );
                    if moving_window {
                        follow_mouse.cancel();
                        suppress_pointer_focus(true);
                        pointer_focus_pause_until = Some(Instant::now() + Duration::from_secs(1));
                    }
                    if pending_commands.len() < 64
                        && adapter
                            .commands
                            .try_send(ManagedCommand::QueryFocused {
                                request: next_command_id,
                                include_cg: moving_window,
                            })
                            .is_ok()
                    {
                        pending_commands.insert(next_command_id, (command, None, pressed_at));
                        if moving_window {
                            latest_move_request = Some(next_command_id);
                        }
                    } else {
                        warn!("hotkey dropped: coordinator or AX queue full");
                    }
                }
                Ok(HotkeyEvent::MouseStart(start)) => {
                    tylora_macos::spaces::NativeSpaceSwitcher::cancel_pending();
                    latest_move_request = None;
                    pointer_focus_pause_until = None;
                    suppress_pointer_focus(
                        !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                    );
                    follow_mouse.cancel();
                    if drag.is_none() {
                        drag = DragSession::begin(start, &state);
                        if drag.is_some() {
                            suppress_pointer_focus(true);
                            let _ = adapter.commands.try_send(ManagedCommand::Focus(start.id));
                            next_drag_probe = Instant::now() + Duration::from_millis(500);
                        }
                    }
                }
                Ok(HotkeyEvent::PointerButton) => {
                    tylora_macos::spaces::NativeSpaceSwitcher::cancel_pending();
                    latest_move_request = None;
                    pointer_focus_pause_until = None;
                    suppress_pointer_focus(
                        !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                    );
                    follow_mouse.cancel();
                    follow_mouse.requested = None;
                }
                Ok(HotkeyEvent::TapDisabled) => warn!("event tap disabled; attempting recovery"),
                Ok(HotkeyEvent::QueueOverflow) => warn!("hotkey queue overflow"),
                Ok(HotkeyEvent::Stopped) => warn!("hotkey event tap stopped"),
                Err(_) => break,
            }
        }
        if let Some(update) = input.take_mouse() {
            if let Some(session) = drag.as_mut() {
                session.update(update, &mut state, &adapter, &config);
            }
        }
        if let Some((point, at)) = input.take_pointer() {
            if drag.is_none() && pending_workspace.is_none() && pointer_focus_pause_until.is_none()
            {
                follow_mouse.moved(point, at, &config.focus);
            } else {
                follow_mouse.cancel();
            }
        }
        follow_mouse.poll(&config.focus);
        if let Some(hit) = follow_mouse.take_hit() {
            let candidate = hit.window.and_then(|(cg_id, pid)| {
                state
                    .coordinators
                    .iter()
                    .find_map(|(display, coordinator)| {
                        if !state.active.contains(display) {
                            return None;
                        }
                        coordinator.registry.iter().find_map(|(id, record)| {
                            let window = &record.observed;
                            if state
                                .native_spaces
                                .iter()
                                .find(|space| space.display == *display && space.active)
                                .and_then(|space| state.layout_zoom.get(&(*display, space.id.0)))
                                .is_some_and(|zoom| {
                                    zoom.cg_window_id != cg_id || zoom.id.process.pid != pid
                                })
                            {
                                return None;
                            }
                            if id.process.pid != pid
                                || window.cg_window_id != Some(cg_id)
                                || !follow_mouse.point.is_some_and(|pointer| {
                                    pointer.0 >= window.frame.x
                                        && pointer.0 < window.frame.x + window.frame.width
                                        && pointer.1 >= window.frame.y
                                        && pointer.1 < window.frame.y + window.frame.height
                                })
                                || window.mode == tylora_core::registry::ManagementMode::Unmanaged
                                || !matches!(
                                    window.lifecycle,
                                    tylora_core::registry::WindowLifecycle::Active
                                        | tylora_core::registry::WindowLifecycle::Discovered
                                )
                                || window.native_space_id.is_none_or(|space| {
                                    !state.native_spaces.iter().any(|s| {
                                        s.id.0 == space && s.display == *display && s.active
                                    })
                                })
                                // A parked Scrolling column only shows a sliver;
                                // hovering it must not focus (and scroll to) it.
                                || coordinator.is_parked(*id)
                                || window.application_id.as_ref().is_none_or(|bundle| {
                                    bundle == "com.tylora.settings"
                                        || config.focus.ignore_apps.iter().any(|app| {
                                            app.enabled
                                                && app.bundle_id.eq_ignore_ascii_case(bundle)
                                        })
                                })
                            {
                                return None;
                            }
                            Some((*id, cg_id))
                        })
                    })
            });
            if let Some((id, cg_id)) = candidate {
                follow_mouse.candidate(id, cg_id, hit.moved_at, &config.focus);
            } else {
                if follow_mouse.pending.take().is_some() {
                    follow_mouse.cancelled += 1;
                }
                follow_mouse.requested = None;
                follow_mouse.applied_for = None;
            }
        }
        if drag.is_none() && pending_workspace.is_none() && pointer_focus_pause_until.is_none() {
            if let Some((id, cg_id, due, moved_at)) = follow_mouse.pending {
                if Instant::now() >= due {
                    follow_mouse.pending = None;
                    let blocked_by_zoom =
                        state.coordinators.iter().any(|(display, coordinator)| {
                            coordinator.registry.get(id).is_some()
                                && state
                                    .native_spaces
                                    .iter()
                                    .find(|space| space.display == *display && space.active)
                                    .and_then(|space| {
                                        state.layout_zoom.get(&(*display, space.id.0))
                                    })
                                    .is_some_and(|zoom| {
                                        zoom.cg_window_id != cg_id
                                            || zoom.id.process.pid != id.process.pid
                                    })
                        });
                    if blocked_by_zoom {
                        follow_mouse.cancel();
                    }
                    if !blocked_by_zoom && follow_mouse.point.is_some() {
                        if adapter.request_pointer_focus(id, cg_id).is_ok() {
                            let requested = Instant::now();
                            debug!(
                                mouse_to_request_us =
                                    requested.saturating_duration_since(moved_at).as_micros(),
                                pid = id.process.pid,
                                token = id.token,
                                "pointer focus requested"
                            );
                            follow_mouse.requested = Some((id, requested));
                            follow_mouse.applied_for = None;
                            follow_mouse.requests_sent += 1;
                            follow_mouse.mouse_to_request_total_us +=
                                requested.saturating_duration_since(moved_at).as_micros();
                        } else {
                            follow_mouse.failures += 1;
                        }
                    }
                }
            }
        } else {
            follow_mouse.cancel();
        }
        if drag.is_some() && drag_focus_probe.is_none() && Instant::now() >= next_drag_probe {
            next_command_id = next_command_id.wrapping_add(1);
            if adapter
                .commands
                .try_send(ManagedCommand::QueryFocused {
                    request: next_command_id,
                    include_cg: false,
                })
                .is_ok()
            {
                drag_focus_probe = Some(next_command_id);
                next_drag_probe = Instant::now() + Duration::from_millis(250);
            }
        }
        if let Some(session) = drag.as_mut() {
            if state
                .coordinators
                .values()
                .all(|c| c.registry.get(session.start.id).is_none())
            {
                let mut cancelled = session.start;
                cancelled.phase = MousePhase::Cancel;
                session.update(cancelled, &mut state, &adapter, &config);
            }
            if session.flush(&adapter, &config) {
                debug!(elapsed_us = session.start.at.elapsed().as_micros(), floating = session.start.floating, button = ?session.start.button, "mouse drag completed");
                drag = None;
                suppress_pointer_focus(
                    !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                );
                drag_focus_probe = None;
            }
        }
        if last_mouse_targets.elapsed() >= Duration::from_millis(100) {
            let targets = state
                .coordinators
                .iter()
                .filter(|(display, _)| state.active.contains(display))
                .flat_map(|(_, c)| c.registry.iter())
                .filter(|(_, record)| {
                    matches!(
                        record.observed.lifecycle,
                        tylora_core::registry::WindowLifecycle::Active
                            | tylora_core::registry::WindowLifecycle::Discovered
                    )
                })
                .map(|(id, record)| MouseTarget {
                    id: *id,
                    frame: record.observed.frame,
                    floating: record.observed.mode
                        == tylora_core::registry::ManagementMode::Floating,
                    enabled: record.observed.mode
                        != tylora_core::registry::ManagementMode::Unmanaged,
                })
                .collect();
            input.update_targets(targets);
            last_mouse_targets = Instant::now();
        }
        pending_commands.retain(|_, (_, response, issued)| {
            let live = issued.elapsed() < Duration::from_secs(2);
            if !live {
                if let Some(response) = response {
                    let _ = response.try_send(command_json(CommandResult::failure(
                        CommandError::AccessibilityUnavailable,
                        "focus query timed out",
                    )));
                }
            }
            live
        });
        pending_move_focus.retain(|_, issued| issued.elapsed() < Duration::from_secs(5));
        move_focus_probe.retain(|_, (_, issued)| issued.elapsed() < Duration::from_secs(5));
        if pending_workspace.as_ref().is_some_and(|pending| {
            pending.started.elapsed() > Duration::from_secs(8) && !pending.timed_out
        }) {
            warn!("workspace transition timed out; requesting visibility recovery");
            if let Some(pending) = pending_workspace.as_mut() {
                state.pending_hide.clear();
                pending.timed_out = true;
                if let Some(response) = pending.response.take() {
                    let _ = response.try_send(command_json(CommandResult::failure(
                        CommandError::AccessibilityUnavailable,
                        "workspace transition timed out; recovery requested",
                    )));
                }
            }
        }
        while let Ok(updated) = area_rx.try_recv() {
            let previous_display_count = state.active.len();
            for (id, target) in state.update_areas(updated, &config) {
                if adapter
                    .commands
                    .try_send(ManagedCommand::Rehome { id, target })
                    .is_err()
                {
                    warn!(
                        pid = id.process.pid,
                        token = id.token,
                        "rehome queue full; macOS relocation will be reconciled"
                    );
                }
            }
            bindings_dirty |= previous_display_count != state.active.len();
            let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
        }
        if stopping {
            break;
        }
        if state.expire_moves() {
            let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
        }
        let start = Instant::now();
        for command in state.plan() {
            if deferred.len() < 1024 {
                deferred.push_back(command);
            } else {
                state.handle_applied(command, Err(-1));
                warn!("apply queue capacity reached");
            }
        }
        debug!(
            elapsed_us = start.elapsed().as_micros(),
            "layout calculated"
        );
        for _ in 0..64 {
            let Some(command) = deferred.pop_front() else {
                break;
            };
            let current = state
                .coordinator_for_active(command.id)
                .and_then(|c| c.registry.get(command.id))
                .is_some_and(|r| {
                    r.generation == command.generation
                        && r.pending_apply == Some(command.version)
                        && r.desired == Some(command.target)
                });
            if !current {
                continue;
            }
            match adapter.commands.try_send(ManagedCommand::Apply(command)) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    deferred.push_front(command);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    stopping = true;
                    break;
                }
            }
        }
        for _ in 0..8 {
            let Ok(request) = ipc_rx.try_recv() else {
                break;
            };
            if request.command == "config get --json" {
                let reply = match crate::config_manager::read(&config_path()) {
                    Ok(snapshot) => serde_json::json!({"version":1,"ok":true,"data":snapshot}),
                    Err(message) => {
                        serde_json::json!({"version":1,"ok":false,"error":{"code":"config_read_failed","message":message}})
                    }
                };
                let _ = request.response.try_send(reply.to_string());
                continue;
            }
            if let Some(payload) = request.command.strip_prefix("layout preview ") {
                let reply = layout_preview(payload, &config).unwrap_or_else(|message| {
                    serde_json::json!({"version":1,"error":message}).to_string()
                });
                let _ = request.response.try_send(reply);
                continue;
            }
            if let Some(payload) = request.command.strip_prefix("config patch ") {
                let result = serde_json::from_str::<crate::config_manager::PatchRequest>(payload)
                    .map_err(|error| error.to_string())
                    .and_then(|patch| {
                        if drag.is_some() { return Err("mouse drag in progress; retry shortly".into()); }
                        if let tylora_config::mutation::ConfigPatch::SetWorkspace { id, display, .. } = &patch.patch {
                            if config.workspaces.iter().find(|workspace| workspace.id == *id)
                                .is_some_and(|workspace| workspace.display != *display) {
                                return Err("changing a workspace display assignment while running is unsupported".into());
                            }
                        }
                        crate::config_manager::patch(&config_path(), patch)
                    });
                let reply = match result {
                    Ok(snapshot) => {
                        suppress_pointer_focus(true);
                        state.apply_live_config(&snapshot.config);
                        config = snapshot.config.clone();
                        input.stop();
                        follow_mouse.cancel();
                        input = HotkeyManager::start(
                            active_bindings(&config),
                            config.mouse.clone(),
                            config.focus.follows_mouse && config.focus.mode != FocusMode::Disabled,
                        );
                        suppress_pointer_focus(
                            !config.focus.follows_mouse || config.focus.mode == FocusMode::Disabled,
                        );
                        let _ = adapter.commands.try_send(ManagedCommand::Reconcile);
                        info!(revision = %snapshot.revision, "GUI config patch applied");
                        serde_json::json!({"version":1,"ok":true,"data":snapshot})
                    }
                    Err(message) => {
                        let code = if message.contains("changed") {
                            "config_conflict"
                        } else if message.contains("unsupported") {
                            "unsupported"
                        } else {
                            "config_validation_failed"
                        };
                        serde_json::json!({"version":1,"ok":false,"error":{"code":code,"message":message}})
                    }
                };
                let _ = request.response.try_send(reply.to_string());
                continue;
            }
            if request.command == "recover-windows" {
                if pending_workspace.is_some() {
                    let _ = request
                        .response
                        .try_send(command_json(CommandResult::failure(
                            CommandError::Unsupported,
                            "workspace transition in progress",
                        )));
                } else {
                    next_command_id = next_command_id.wrapping_add(1);
                    // Columns a Scrolling layout parks on purpose stay reachable
                    // through navigation; only stranded windows are moved.
                    let parked: Vec<_> = state
                        .coordinators
                        .iter()
                        .filter(|(display, _)| state.active.contains(display))
                        .flat_map(|(_, c)| {
                            c.layout
                                .windows()
                                .into_iter()
                                .filter(|id| c.is_parked(*id))
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    let _ = adapter.commands.try_send(ManagedCommand::SetParked(parked));
                    if adapter
                        .commands
                        .try_send(ManagedCommand::RestoreVisibility {
                            request: next_command_id,
                        })
                        .is_ok()
                    {
                        pending_workspace = Some(PendingWorkspace {
                            request: next_command_id,
                            action: WorkspaceAction::Recover,
                            response: Some(request.response),
                            started: Instant::now(),
                            timed_out: false,
                        });
                    } else {
                        let _ = request
                            .response
                            .try_send(command_json(CommandResult::failure(
                                CommandError::AccessibilityUnavailable,
                                "AX worker busy",
                            )));
                    }
                }
                continue;
            }
            if let Some(payload) = request.command.strip_prefix("v1 ") {
                match serde_json::from_str::<StateCommand>(payload) {
                    Ok(
                        command @ (StateCommand::FocusWorkspace { .. }
                        | StateCommand::FocusNamedWorkspace { .. }
                        | StateCommand::FocusNextWorkspace
                        | StateCommand::FocusPreviousWorkspace),
                    ) => {
                        let configured = config.clone();
                        let response = request.response;
                        let reconcile = adapter.commands.clone();
                        let focus_sender = native_move_tx.clone();
                        let history = space_focus.clone();
                        thread::spawn(move || {
                            let result = resolve_focus_target(&configured, &command, &history)
                                .and_then(|(display, target)| {
                                    focus_workspace_space(display, target)
                                        .map(|(_, needs_focus)| (display, target, needs_focus))
                                });
                            let reply = match result {
                                Ok((display, target, needs_focus)) => {
                                    let _ = reconcile.try_send(ManagedCommand::Reconcile);
                                    let _ = focus_sender.send(NativeMoveEvent::WorkspaceSwitched {
                                        switch: 0,
                                        focus: needs_focus.then_some(display),
                                        space: Some(target.0),
                                    });
                                    CommandResult::success("native Space focused")
                                }
                                Err(message) => {
                                    CommandResult::failure(CommandError::Unsupported, message)
                                }
                            };
                            let _ = response.try_send(command_json(reply));
                        });
                    }
                    Ok(command) if pending_commands.len() < 64 => {
                        next_command_id = next_command_id.wrapping_add(1);
                        if adapter
                            .commands
                            .try_send(ManagedCommand::QueryFocused {
                                request: next_command_id,
                                include_cg: matches!(
                                    command,
                                    StateCommand::MoveWindowToWorkspace { .. }
                                        | StateCommand::MoveWindowToNamedWorkspace { .. }
                                        | StateCommand::MoveWindowToDisplay { .. }
                                        | StateCommand::MoveWindowToNextDisplay { .. }
                                        | StateCommand::MoveWindowToPreviousDisplay { .. }
                                ),
                            })
                            .is_ok()
                        {
                            pending_commands.insert(
                                next_command_id,
                                (command, Some(request.response), Instant::now()),
                            );
                        } else {
                            let _ =
                                request
                                    .response
                                    .try_send(command_json(CommandResult::failure(
                                        CommandError::AccessibilityUnavailable,
                                        "AX worker busy",
                                    )));
                        }
                    }
                    Ok(_) => {
                        let _ = request
                            .response
                            .try_send(command_json(CommandResult::failure(
                                CommandError::Unsupported,
                                "command queue full",
                            )));
                    }
                    Err(error) => {
                        let _ = request
                            .response
                            .try_send(command_json(CommandResult::failure(
                                CommandError::InvalidArgument,
                                error.to_string(),
                            )));
                    }
                }
                continue;
            }
            let reply = if request.command == "hotkeys list" {
                let mut out = String::new();
                for hotkey in effective_hotkeys(&config) {
                    out.push_str(&format!(
                        "{}\t{}\t{:?}\n",
                        hotkey.chord,
                        if !hotkey.enabled {
                            "disabled"
                        } else if hotkey_supported(&hotkey.command, &config) {
                            "active"
                        } else {
                            "unavailable"
                        },
                        hotkey.command
                    ));
                }
                out
            } else if request.command == "hotkeys list --json" {
                let hotkeys: Vec<_> = effective_hotkeys(&config)
                    .into_iter()
                    .map(|entry| {
                        serde_json::json!({
                            "chord": entry.chord, "command": entry.command,
                            "enabled": entry.enabled,
                            "active": entry.enabled && hotkey_supported(&entry.command, &config)
                        })
                    })
                    .collect();
                serde_json::to_string(&serde_json::json!({"version":1,"hotkeys":hotkeys}))
                    .unwrap_or_default()
            } else if request.command == "workspace list" {
                let mut out = String::new();
                let mut entries: Vec<_> = state
                    .workspaces
                    .iter()
                    .flat_map(|(display, engine)| {
                        engine
                            .workspaces()
                            .map(move |workspace| (*display, engine, workspace))
                    })
                    .collect();
                entries.sort_by_key(|(display, _, workspace)| (display.0, workspace.id));
                for (display, engine, workspace) in entries {
                    out.push_str(&format!(
                        "{}\t{}\tdisplay={}\tactive={}\twindows={}\n",
                        workspace.id,
                        workspace.name,
                        display.0,
                        engine.active(display) == Some(workspace.id),
                        workspace.windows.len()
                    ));
                }
                out
            } else if request.command == "workspace list --json" {
                let mut entries: Vec<_> = state
                    .workspaces
                    .iter()
                    .flat_map(|(display, engine)| {
                        engine
                            .workspaces()
                            .map(move |workspace| (*display, engine, workspace))
                    })
                    .collect();
                entries.sort_by_key(|(display, _, workspace)| (display.0, workspace.id));
                let workspaces: Vec<_> = entries.into_iter().map(|(display, engine, ws)| serde_json::json!({"id":ws.id,"name":ws.name,"display":display,"active":engine.active(display)==Some(ws.id),"windows":ws.windows.len()})).collect();
                serde_json::to_string(&serde_json::json!({"version":1,"visibility_backend":"ax_minimized","workspaces":workspaces})).unwrap_or_default()
            } else if request.command == "diagnostics --json" {
                let mut value: serde_json::Value = serde_json::from_str(&handle_request(
                    &request.command,
                    &state,
                    &adapter,
                    &mut stopping,
                ))
                .unwrap_or_default();
                value["focus_follows_mouse"] = serde_json::json!({
                    "requests": follow_mouse.requests_sent,
                    "confirmed": follow_mouse.confirmations,
                    "applied": follow_mouse.applied,
                    "cancelled_delayed": follow_mouse.cancelled,
                    "failures": follow_mouse.failures,
                    "hit_tests": follow_mouse.hit_count,
                    "hit_test_total_us": follow_mouse.hit_total_us,
                    "mouse_to_request_total_us": follow_mouse.mouse_to_request_total_us,
                    "request_to_confirm_total_us": follow_mouse.request_to_confirm_total_us,
                    "request_to_applied_total_us": follow_mouse.request_to_applied_total_us,
                });
                value.to_string()
            } else {
                handle_request(&request.command, &state, &adapter, &mut stopping)
            };
            let _ = request.response.try_send(reply);
        }
        thread::sleep(Duration::from_millis(10));
    }
    input.stop();
    suppress_pointer_focus(false);
    if adapter
        .commands
        .try_send(ManagedCommand::RestoreVisibility { request: u64::MAX })
        .is_ok()
    {
        let until = Instant::now() + Duration::from_secs(8);
        while Instant::now() < until {
            match adapter.events.recv_timeout(Duration::from_millis(100)) {
                Ok(ManagedEvent::VisibilityDone { request, result }) if request == u64::MAX => {
                    if let Err(code) = result {
                        warn!(code, "visibility restore failed during stop");
                    }
                    break;
                }
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
        }
    }
    let _ = adapter.commands.try_send(ManagedCommand::Stop);
    ipc_stopping.store(true, Ordering::Release);
    let _ = fs::remove_file(socket_path());
    info!("tiling stopped");
    ExitCode::SUCCESS
}

fn command_json(result: CommandResult) -> String {
    serde_json::to_string(&result)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"unsupported\"}".into())
}

#[cfg_attr(not(test), allow(dead_code))]
fn prepare_workspace_action(
    state: &DisplayState,
    command: &StateCommand,
    focused: Option<ManagedWindowId>,
) -> Result<Option<WorkspaceAction>, CommandResult> {
    let display = focused
        .and_then(|id| {
            state
                .coordinators
                .iter()
                .find(|(d, c)| state.active.contains(d) && c.registry.get(id).is_some())
                .map(|(d, _)| *d)
        })
        .unwrap_or(state.primary);
    let Some(engine) = state.workspaces.get(&display) else {
        return Err(CommandResult::failure(
            CommandError::DisplayNotFound,
            "workspace display unavailable",
        ));
    };
    let Some(coordinator) = state.coordinators.get(&display) else {
        return Err(CommandResult::failure(
            CommandError::DisplayNotFound,
            "display coordinator unavailable",
        ));
    };
    match command {
        StateCommand::FocusWorkspace { id } => {
            if engine.workspace(*id).is_none() {
                return Err(CommandResult::failure(
                    CommandError::WorkspaceNotFound,
                    "workspace is not configured on this display",
                ));
            }
            if engine.active(display) == Some(*id) {
                return Ok(None);
            }
            let old = engine.active(display).ok_or_else(|| {
                CommandResult::failure(CommandError::WorkspaceNotFound, "no active workspace")
            })?;
            let hide = engine
                .workspace(old)
                .into_iter()
                .flat_map(|ws| ws.windows.iter())
                .filter(|id| {
                    coordinator.registry.get(**id).is_some_and(|r| {
                        matches!(
                            r.observed.lifecycle,
                            tylora_core::registry::WindowLifecycle::Active
                                | tylora_core::registry::WindowLifecycle::Discovered
                        )
                    })
                })
                .copied()
                .collect();
            let show = engine
                .workspace(*id)
                .into_iter()
                .flat_map(|ws| ws.windows.iter())
                .filter(|id| state.hidden_by_tylora.contains(id))
                .copied()
                .collect();
            Ok(Some(WorkspaceAction::Focus {
                display,
                target: *id,
                source_layout: coordinator.layout.clone(),
                hide,
                show,
            }))
        }
        StateCommand::MoveWindowToWorkspace { window, workspace } => {
            let id = (*window).or(focused).ok_or_else(|| {
                CommandResult::failure(CommandError::NoFocusedWindow, "no focused window")
            })?;
            if coordinator.registry.get(id).is_none() || engine.owner(id).is_none() {
                return Err(CommandResult::failure(
                    CommandError::WindowNotFound,
                    "window is not managed on this display",
                ));
            }
            if engine.workspace(*workspace).is_none() {
                return Err(CommandResult::failure(
                    CommandError::WorkspaceNotFound,
                    "workspace is not configured on this display",
                ));
            }
            if engine.owner(id) == Some(*workspace) {
                return Ok(None);
            }
            let hide = if coordinator.registry.get(id).is_some_and(|r| {
                matches!(
                    r.observed.lifecycle,
                    tylora_core::registry::WindowLifecycle::Active
                        | tylora_core::registry::WindowLifecycle::Discovered
                )
            }) {
                vec![id]
            } else {
                Vec::new()
            };
            Ok(Some(WorkspaceAction::Move {
                display,
                window: id,
                target: *workspace,
                source_layout: coordinator.layout.clone(),
                hide,
            }))
        }
        _ => Err(CommandResult::failure(
            CommandError::Unsupported,
            "not a workspace command",
        )),
    }
}

fn apply_workspace_action(state: &mut DisplayState, action: WorkspaceAction) -> CommandResult {
    match action {
        WorkspaceAction::Focus {
            display,
            target,
            source_layout,
            hide,
            show,
        } => {
            let Some(engine) = state.workspaces.get_mut(&display) else {
                return CommandResult::failure(
                    CommandError::DisplayNotFound,
                    "workspace display vanished",
                );
            };
            let Some(source) = engine.active(display) else {
                return CommandResult::failure(
                    CommandError::WorkspaceNotFound,
                    "active workspace vanished",
                );
            };
            if let Some(ws) = engine.workspace_mut(source) {
                ws.layout = source_layout;
            }
            let source_ids = engine
                .workspace(source)
                .map(|ws| ws.windows.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            let target_ids = engine
                .workspace(target)
                .map(|ws| ws.windows.iter().copied().collect::<Vec<_>>())
                .unwrap_or_default();
            let focus = match engine.focus(display, target, &mut ConfirmedVisibility) {
                Ok(focus) => focus,
                Err(error) => {
                    return CommandResult::failure(error, "workspace model transition failed")
                }
            };
            let layout = engine
                .workspace(target)
                .map(|ws| ws.layout.clone())
                .unwrap_or_default();
            if let Some(coordinator) = state.coordinators.get_mut(&display) {
                for id in source_ids {
                    let _ = coordinator.suspend(id);
                }
                for id in target_ids {
                    coordinator.resume(id);
                }
                coordinator.replace_layout(layout);
            }
            state.hidden_by_tylora.extend(hide);
            for id in show {
                state.hidden_by_tylora.remove(&id);
            }
            if !cfg!(test) {
                save_active_workspaces(state);
            }
            let mut result = CommandResult::success("workspace switched");
            result.display = Some(display);
            result.window = focus;
            result
        }
        WorkspaceAction::Move {
            display,
            window,
            target,
            source_layout,
            hide,
        } => {
            let Some(engine) = state.workspaces.get_mut(&display) else {
                return CommandResult::failure(
                    CommandError::DisplayNotFound,
                    "workspace display vanished",
                );
            };
            let Some(source) = engine.owner(window) else {
                return CommandResult::failure(
                    CommandError::WindowNotFound,
                    "workspace owner vanished",
                );
            };
            if let Some(ws) = engine.workspace_mut(source) {
                ws.layout = source_layout;
            }
            if let Err(error) = engine.move_window(window, target, &mut ConfirmedVisibility) {
                return CommandResult::failure(error, "workspace model move failed");
            }
            let layout = engine
                .workspace(source)
                .map(|ws| ws.layout.clone())
                .unwrap_or_default();
            if let Some(coordinator) = state.coordinators.get_mut(&display) {
                let _ = coordinator.suspend(window);
                coordinator.replace_layout(layout);
            }
            state.hidden_by_tylora.extend(hide);
            let mut result = CommandResult::success("window moved to workspace");
            result.display = Some(display);
            result.window = Some(window);
            result
        }
        WorkspaceAction::Recover => {
            state.hidden_by_tylora.clear();
            for (&display, engine) in &mut state.workspaces {
                let Some(active) = engine.active(display) else {
                    continue;
                };
                if let Some(coordinator) = state.coordinators.get(&display) {
                    if let Some(ws) = engine.workspace_mut(active) {
                        ws.layout = coordinator.layout.clone();
                    }
                }
                let misplaced: Vec<_> = engine
                    .owned_windows()
                    .into_iter()
                    .filter(|id| engine.owner(*id) != Some(active))
                    .collect();
                for id in misplaced {
                    let _ = engine.move_window(id, active, &mut ConfirmedVisibility);
                    if let Some(coordinator) = state.coordinators.get_mut(&display) {
                        coordinator.resume(id);
                    }
                }
                if let (Some(coordinator), Some(ws)) = (
                    state.coordinators.get_mut(&display),
                    engine.workspace(active),
                ) {
                    coordinator.replace_layout(ws.layout.clone());
                }
            }
            CommandResult::success("visible windows restored into active workspaces")
        }
    }
}

fn execute_state_command(
    state: &mut DisplayState,
    adapter: &ManagedAdapter,
    config: &Config,
    command: &StateCommand,
    focused: Option<tylora_core::ManagedWindowId>,
) -> CommandResult {
    if let StateCommand::ToggleLayoutZoom { window } = command {
        let Some(id) = (*window).or(focused) else {
            return CommandResult::failure(CommandError::NoFocusedWindow, "no focused window");
        };
        let Some((&display, coordinator)) = state
            .coordinators
            .iter()
            .find(|(display, c)| state.active.contains(display) && c.registry.get(id).is_some())
        else {
            return CommandResult::failure(CommandError::WindowNotFound, "window is not managed");
        };
        let Some(space) = state
            .native_spaces
            .iter()
            .find(|s| s.display == display && s.active)
            .map(|s| s.id.0)
        else {
            return CommandResult::failure(
                CommandError::Unsupported,
                "active native Space is unavailable",
            );
        };
        let Some(record) = coordinator.registry.get(id) else {
            unreachable!()
        };
        if record.observed.mode != tylora_core::registry::ManagementMode::Tiled
            || !coordinator.layout.windows().contains(&id)
        {
            return CommandResult::failure(
                CommandError::Unsupported,
                "layout Zoom requires a tiled window",
            );
        }
        if record.observed.native_space_id != Some(space) {
            return CommandResult::failure(
                CommandError::Unsupported,
                "window's native Space could not be verified",
            );
        }
        let Some(cg_window_id) = record.observed.cg_window_id else {
            return CommandResult::failure(
                CommandError::Unsupported,
                "window has no verified CGWindowID",
            );
        };
        let key = (display, space);
        let disabling = state.layout_zoom.get(&key).is_some_and(|zoom| {
            zoom.cg_window_id == cg_window_id && zoom.id.process.pid == id.process.pid
        });
        if disabling {
            state.layout_zoom.remove(&key);
        } else {
            state.layout_zoom.insert(
                key,
                ZoomTarget {
                    id,
                    cg_window_id,
                    generation: record.generation,
                },
            );
        }
        if let Some(coordinator) = state.coordinators.get_mut(&display) {
            coordinator.version = coordinator.version.wrapping_add(1);
        }
        if !disabling {
            let _ = adapter.commands.try_send(ManagedCommand::Focus(id));
        }
        let mut result = CommandResult::success(if disabling {
            "layout Zoom disabled; BSP restoration scheduled"
        } else {
            "layout Zoom scheduled"
        });
        result.window = Some(id);
        result.display = Some(display);
        return result;
    }
    if matches!(command, StateCommand::ReconcileWindows) {
        return if adapter.commands.try_send(ManagedCommand::Reconcile).is_ok() {
            CommandResult::success("reconciliation scheduled")
        } else {
            CommandResult::failure(CommandError::AccessibilityUnavailable, "AX worker busy")
        };
    }
    if matches!(
        command,
        StateCommand::MoveWindowToDisplay { .. }
            | StateCommand::MoveWindowToNextDisplay { .. }
            | StateCommand::MoveWindowToPreviousDisplay { .. }
    ) {
        return CommandResult::failure(
            CommandError::Unsupported,
            "display move requires verified native transfer",
        );
    }
    if matches!(command, StateCommand::FocusDisplayNext) {
        let origin = focused
            .and_then(|id| {
                state
                    .coordinators
                    .iter()
                    .find(|(display, c)| {
                        state.active.contains(display) && c.registry.get(id).is_some()
                    })
                    .map(|(display, _)| *display)
            })
            .unwrap_or(state.primary);
        let Some(target) = adjacent_display(&ordered_displays(state), origin, false, true) else {
            return CommandResult::failure(CommandError::NoNeighbor, "no other display");
        };
        return focus_display(state, adapter, target);
    }
    if let StateCommand::FocusDisplay { direction } = command {
        let origin = focused
            .and_then(|id| {
                state
                    .coordinators
                    .values()
                    .find(|c| c.registry.get(id).is_some())
                    .map(|c| c.display)
            })
            .unwrap_or(state.primary);
        let Some(source) = state.areas.get(&origin) else {
            return CommandResult::failure(
                CommandError::DisplayNotFound,
                "current display unavailable",
            );
        };
        let (sx, sy) = (
            source.frame.x + source.frame.width / 2.0,
            source.frame.y + source.frame.height / 2.0,
        );
        let target = state
            .active
            .iter()
            .filter(|&&id| id != origin)
            .filter_map(|id| {
                let area = state.areas.get(id)?;
                let (dx, dy) = (
                    area.frame.x + area.frame.width / 2.0 - sx,
                    area.frame.y + area.frame.height / 2.0 - sy,
                );
                let forward = match direction {
                    tylora_core::commands::Direction::Left => -dx,
                    tylora_core::commands::Direction::Right => dx,
                    tylora_core::commands::Direction::Up => -dy,
                    tylora_core::commands::Direction::Down => dy,
                };
                (forward > 0.0).then_some((*id, dx * dx + dy * dy))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((display, _)) = target else {
            return CommandResult::failure(
                CommandError::NoNeighbor,
                "no display in that direction",
            );
        };
        return focus_display(state, adapter, display);
    }
    if matches!(
        command,
        StateCommand::SetLayout { .. }
            | StateCommand::CycleLayout
            | StateCommand::ScrollLayoutLeft
            | StateCommand::ScrollLayoutRight
            | StateCommand::CenterFocusedWindow
            | StateCommand::ResizeScrollingColumn { .. }
            | StateCommand::FocusNextWindow
            | StateCommand::FocusPreviousWindow
    ) {
        let Some(display) = layout_target_display(state, focused) else {
            return CommandResult::failure(CommandError::DisplayNotFound, "no active display");
        };
        let Some(coordinator) = state.coordinators.get_mut(&display) else {
            return CommandResult::failure(CommandError::DisplayNotFound, "display unavailable");
        };
        let before = coordinator.layout.kind();
        let outcome = coordinator.execute(command, focused);
        let after = coordinator.layout.kind();
        if before != after {
            // Zoom is tied to the previous layout's geometry.
            state
                .layout_zoom
                .retain(|(zoom_display, _), _| *zoom_display != display);
            let display_id = display.0;
            info!(
                display_id,
                from = before.name(),
                to = after.name(),
                windows = coordinator.layout.len(),
                "workspace layout switched"
            );
        }
        let mut result = match outcome {
            Ok(CommandEffect::Focus(window)) => {
                if adapter
                    .commands
                    .try_send(ManagedCommand::Focus(window))
                    .is_err()
                {
                    return CommandResult::failure(
                        CommandError::AccessibilityUnavailable,
                        "AX worker busy",
                    );
                }
                let mut result = CommandResult::success("focus scheduled");
                result.window = Some(window);
                result
            }
            Ok(_) if before != after => {
                CommandResult::success(format!("layout set to {}", after.name()))
            }
            Ok(_) => CommandResult::success("layout update scheduled"),
            Err(CommandError::InvalidArgument)
                if matches!(command, StateCommand::SetLayout { .. }) =>
            {
                CommandResult::success(format!("layout already {}", after.name()))
            }
            Err(CommandError::Unsupported) => CommandResult::failure(
                CommandError::Unsupported,
                format!(
                    "command requires the scrolling layout (current: {})",
                    after.name()
                ),
            ),
            Err(error) => {
                let message = format!("command failed: {error:?}");
                CommandResult::failure(error, message)
            }
        };
        result.display = Some(display);
        return result;
    }
    let command_window = match command {
        StateCommand::ToggleFloating { window } => (*window).or(focused),
        StateCommand::SwapWindow { first, .. } => Some(*first),
        _ => focused,
    };
    let Some(coordinator) = command_window.and_then(|id| state.coordinator_for_active(id)) else {
        return CommandResult::failure(
            CommandError::NoFocusedWindow,
            "focused window is not managed",
        );
    };
    let outcome = coordinator.execute(command, focused);
    if let (StateCommand::ToggleFloating { .. }, Some(id)) = (command, command_window) {
        if outcome.is_ok() {
            let is_floating = state.coordinators.values().any(|coordinator| {
                coordinator.registry.get(id).is_some_and(|record| {
                    record.observed.mode == tylora_core::registry::ManagementMode::Floating
                })
            });
            if is_floating {
                state.layout_zoom.retain(|_, zoom| zoom.id != id);
            }
        }
    }
    if let (StateCommand::FocusWindow { direction }, Err(CommandError::NoNeighbor), Some(source)) =
        (command, &outcome, focused)
    {
        if config.cross_monitor_navigation {
            let placements = state
                .coordinators
                .iter()
                .filter(|(display, _)| state.active.contains(display))
                .flat_map(|(_, c)| {
                    c.layout
                        .calculate_layout(c.work_area)
                        .into_iter()
                        .filter(|(id, _)| !c.is_parked(*id))
                });
            if let Some(target) = neighbor(source, *direction, placements) {
                if adapter
                    .commands
                    .try_send(ManagedCommand::Focus(target))
                    .is_ok()
                {
                    let mut result = CommandResult::success("cross-display focus scheduled");
                    result.window = Some(target);
                    return result;
                }
                return CommandResult::failure(
                    CommandError::AccessibilityUnavailable,
                    "AX worker busy",
                );
            }
        }
    }
    match outcome {
        Ok(CommandEffect::None) => CommandResult::success("layout update scheduled"),
        Ok(CommandEffect::Focus(window)) => {
            if adapter
                .commands
                .try_send(ManagedCommand::Focus(window))
                .is_err()
            {
                CommandResult::failure(CommandError::AccessibilityUnavailable, "AX worker busy")
            } else {
                let mut result = CommandResult::success("focus scheduled");
                result.window = Some(window);
                result
            }
        }
        Ok(CommandEffect::Reconcile) => unreachable!(),
        Err(error) => {
            let message = format!("command failed: {error:?}");
            CommandResult::failure(error, message)
        }
    }
}

/// Display whose current workspace a layout command applies to: the focused
/// managed window's display, else the display under the pointer, else primary.
fn layout_target_display(
    state: &DisplayState,
    focused: Option<ManagedWindowId>,
) -> Option<DisplayId> {
    focused
        .and_then(|id| {
            state
                .coordinators
                .iter()
                .find(|(display, c)| state.active.contains(display) && c.registry.get(id).is_some())
                .map(|(display, _)| *display)
        })
        .or_else(|| {
            tylora_macos::spaces::cursor_display().filter(|display| state.active.contains(display))
        })
        .or_else(|| {
            state
                .active
                .contains(&state.primary)
                .then_some(state.primary)
        })
}

fn ordered_displays(state: &DisplayState) -> Vec<DisplayId> {
    let mut ordered: Vec<_> = state
        .active
        .iter()
        .filter_map(|id| state.areas.get(id))
        .collect();
    ordered.sort_by(|a, b| {
        a.full_frame
            .x
            .total_cmp(&b.full_frame.x)
            .then_with(|| a.full_frame.y.total_cmp(&b.full_frame.y))
            .then_with(|| a.display.0.cmp(&b.display.0))
    });
    ordered.into_iter().map(|area| area.display).collect()
}

fn adjacent_display(
    order: &[DisplayId],
    origin: DisplayId,
    previous: bool,
    wrap: bool,
) -> Option<DisplayId> {
    if order.len() < 2 {
        return None;
    }
    let index = order.iter().position(|id| *id == origin)?;
    let target = if previous {
        if index == 0 {
            if wrap {
                order.len() - 1
            } else {
                return None;
            }
        } else {
            index - 1
        }
    } else if index + 1 == order.len() {
        if wrap {
            0
        } else {
            return None;
        }
    } else {
        index + 1
    };
    Some(order[target])
}

fn floating_destination(
    source: tylora_core::Rect,
    target: tylora_core::Rect,
    frame: tylora_core::Rect,
    preserve: bool,
) -> tylora_core::Rect {
    let width = frame.width.min(target.width).max(1.0);
    let height = frame.height.min(target.height).max(1.0);
    let (x, y) = if preserve {
        let x_fraction = if source.width > frame.width {
            ((frame.x - source.x) / (source.width - frame.width)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let y_fraction = if source.height > frame.height {
            ((frame.y - source.y) / (source.height - frame.height)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        (
            target.x + x_fraction * (target.width - width),
            target.y + y_fraction * (target.height - height),
        )
    } else {
        (
            target.x + (target.width - width) / 2.0,
            target.y + (target.height - height) / 2.0,
        )
    };
    tylora_core::Rect::new(x, y, width, height).expect("finite work areas")
}

fn prepare_display_move(
    state: &DisplayState,
    config: &Config,
    command: &StateCommand,
    focused: Option<ManagedWindowId>,
) -> Result<PreparedDisplayMove, CommandResult> {
    let selected = match command {
        StateCommand::MoveWindowToDisplay { window, .. }
        | StateCommand::MoveWindowToNextDisplay { window }
        | StateCommand::MoveWindowToPreviousDisplay { window } => (*window).or(focused),
        _ => {
            return Err(CommandResult::failure(
                CommandError::Unsupported,
                "not a display move",
            ))
        }
    };
    let id = selected.ok_or_else(|| {
        CommandResult::failure(CommandError::NoFocusedWindow, "no focused window")
    })?;
    if state.in_flight_display.contains(&id) {
        return Err(CommandResult::failure(
            CommandError::Unsupported,
            "window transfer already in progress",
        ));
    }
    let (source_display, record) = state
        .coordinators
        .iter()
        .filter(|(display, _)| state.active.contains(display))
        .find_map(|(display, coordinator)| {
            coordinator
                .registry
                .get(id)
                .map(|record| (*display, record))
        })
        .ok_or_else(|| {
            CommandResult::failure(CommandError::WindowNotFound, "window is not managed")
        })?;
    let order = ordered_displays(state);
    let target_display = match command {
        StateCommand::MoveWindowToDisplay { display, .. } => *display,
        StateCommand::MoveWindowToNextDisplay { .. } => adjacent_display(
            &order,
            source_display,
            false,
            config.window_movement.wrap_around,
        )
        .ok_or_else(|| CommandResult::failure(CommandError::NoNeighbor, "no next display"))?,
        StateCommand::MoveWindowToPreviousDisplay { .. } => adjacent_display(
            &order,
            source_display,
            true,
            config.window_movement.wrap_around,
        )
        .ok_or_else(|| CommandResult::failure(CommandError::NoNeighbor, "no previous display"))?,
        _ => unreachable!(),
    };
    if target_display == source_display {
        return Err(CommandResult::failure(
            CommandError::NoNeighbor,
            "window is already on that display",
        ));
    }
    let target_area = state
        .areas
        .get(&target_display)
        .filter(|_| state.active.contains(&target_display))
        .ok_or_else(|| {
            CommandResult::failure(CommandError::DisplayNotFound, "target display unavailable")
        })?;
    let source_area = state.areas.get(&source_display).ok_or_else(|| {
        CommandResult::failure(CommandError::DisplayNotFound, "source display unavailable")
    })?;
    let source_space = record.observed.native_space_id.ok_or_else(|| {
        CommandResult::failure(CommandError::Unsupported, "source native Space is unknown")
    })?;
    if !state
        .native_spaces
        .iter()
        .any(|space| space.display == source_display && space.id.0 == source_space && space.active)
    {
        return Err(CommandResult::failure(
            CommandError::Unsupported,
            "source Space is no longer active",
        ));
    }
    let active_target = state
        .native_spaces
        .iter()
        .find(|space| space.display == target_display && space.active)
        .map(|space| space.id.0)
        .ok_or_else(|| {
            CommandResult::failure(
                CommandError::Unsupported,
                "target display has no active native Space",
            )
        })?;
    // Keep the Desktop: the same workspace or Desktop number on the target
    // display, created when missing; otherwise the active Space as before.
    let (target_space, create, bind_workspace) =
        match choose_display_target(config, &state.native_spaces, source_space, target_display) {
            Some((DisplayTarget::Existing(id), _)) => (id, None, None),
            Some((DisplayTarget::Create { desktop, missing }, workspace)) => {
                (active_target, Some((desktop, missing)), workspace)
            }
            None => (active_target, None, None),
        };
    let cg_window_id = record.observed.cg_window_id.ok_or_else(|| {
        CommandResult::failure(
            CommandError::Unsupported,
            "window has no verified CGWindowID",
        )
    })?;
    if state.pending_display.contains_key(&cg_window_id) {
        return Err(CommandResult::failure(
            CommandError::Unsupported,
            "previous native transfer is awaiting reconciliation",
        ));
    }
    if record.observed.mode == tylora_core::registry::ManagementMode::Unmanaged {
        return Err(CommandResult::failure(
            CommandError::Unsupported,
            "window is unmanaged",
        ));
    }
    let target_frame = floating_destination(
        source_area.frame,
        target_area.frame,
        record.observed.frame,
        config.window_movement.preserve_floating_position,
    );
    Ok(PreparedDisplayMove {
        issued: Instant::now(),
        id,
        generation: record.generation,
        cg_window_id,
        source_display,
        target_display,
        source_space,
        target_space,
        mode: record.observed.mode,
        target_frame,
        focus_after_move: config.window_movement.focus_after_move,
        create,
        bind_workspace,
    })
}
fn focus_display(
    state: &DisplayState,
    adapter: &ManagedAdapter,
    display: DisplayId,
) -> CommandResult {
    let window = state
        .coordinators
        .get(&display)
        .and_then(|c| c.layout.windows().first().copied());
    match window {
        Some(window)
            if adapter
                .commands
                .try_send(ManagedCommand::Focus(window))
                .is_ok() =>
        {
            let mut result = CommandResult::success("display focus scheduled");
            result.display = Some(display);
            result.window = Some(window);
            result
        }
        Some(_) => CommandResult::failure(CommandError::AccessibilityUnavailable, "AX worker busy"),
        None => CommandResult::failure(
            CommandError::NoFocusedWindow,
            "target display has no managed window",
        ),
    }
}
/// Frames from the real layout engines for the GUI preview. Scrolling
/// returns virtual strip frames (the GUI clips them to the viewport) plus the
/// placement each column would get on a display with no neighbours.
fn layout_preview(payload: &str, config: &Config) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Preview {
        inner: f64,
        outer: f64,
        #[serde(default)]
        layout: Option<LayoutKind>,
        #[serde(default)]
        dwindle: Option<tylora_config::DwindleSettings>,
        #[serde(default)]
        scrolling: Option<tylora_config::ScrollingSettings>,
        #[serde(default)]
        windows: Option<u64>,
        #[serde(default)]
        focus: Option<u64>,
    }
    let input = serde_json::from_str::<Preview>(payload).map_err(|error| error.to_string())?;
    let mut settings = layout_settings(config);
    if let Some(dwindle) = input.dwindle {
        dwindle.validate()?;
        settings.dwindle = dwindle;
    }
    if let Some(scrolling) = input.scrolling {
        scrolling.validate()?;
        settings.scrolling = scrolling;
    }
    let kind = input.layout.unwrap_or(LayoutKind::Bsp);
    let mut layout = TilingLayout::new(kind, &settings);
    layout
        .set_gaps(input.inner, input.outer)
        .map_err(|_| "invalid gaps".to_string())?;
    let area = tylora_core::Rect::new(0.0, 0.0, 640.0, 360.0).map_err(|error| error.to_string())?;
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(area),
        other_displays: Vec::new(),
    });
    let slot = |token: u64| ManagedWindowId {
        process: ProcessInstance { pid: 1, epoch: 1 },
        token,
    };
    let count = input.windows.unwrap_or(3).clamp(1, 6);
    for token in 1..=count {
        layout.insert_window(slot(token));
    }
    if let Some(focus) = input.focus.filter(|focus| (1..=count).contains(focus)) {
        layout.focus_changed(slot(focus), FocusOrigin::Command, area);
    }
    let summary = layout.summary(area);
    let frames: Vec<_> = match &summary.scrolling {
        Some(scrolling) => {
            let view = layout.scrolling().view(area);
            scrolling
                .columns
                .iter()
                .map(|column| {
                    let rect = tylora_core::Rect::new(
                        view.usable.x + column.strip_x - view.offset,
                        view.usable.y,
                        column.width,
                        view.usable.height,
                    )
                    .expect("finite preview frame");
                    serde_json::json!({"slot": column.window.token, "rect": rect, "placement": column.placement})
                })
                .collect()
        }
        None => layout
            .calculate_layout(area)
            .into_iter()
            .map(|(id, rect)| serde_json::json!({"slot":id.token,"rect":rect}))
            .collect(),
    };
    Ok(serde_json::json!({
        "version": 1,
        "layout": kind.name(),
        "frames": frames,
        "scrolling": summary.scrolling,
    })
    .to_string())
}

/// Current layout per active display, with the bound workspace and, for
/// Scrolling, the viewport and column placements.
fn layout_current_json(state: &DisplayState) -> Vec<serde_json::Value> {
    let mut displays: Vec<_> = state.active.iter().copied().collect();
    displays.sort();
    displays
        .into_iter()
        .filter_map(|display| {
            let coordinator = state.coordinators.get(&display)?;
            let space = state
                .native_spaces
                .iter()
                .find(|space| space.display == display && space.active)
                .map(|space| space.id.0);
            let workspace = space.and_then(|space| state.space_bindings.get(&space).copied());
            let summary = coordinator.layout.summary(coordinator.work_area);
            Some(serde_json::json!({
                "display": display,
                "native_space_id": space,
                "workspace": workspace,
                "layout": summary.kind.name(),
                "windows": summary.windows,
                "order": summary.order,
                "scrolling": summary.scrolling,
            }))
        })
        .collect()
}

fn handle_request(
    command: &str,
    state: &DisplayState,
    adapter: &ManagedAdapter,
    stopping: &mut bool,
) -> String {
    match command {
        "stop" => {
            *stopping = true;
            "stopping\n".into()
        }
        "status" => format!(
            "running displays={} primary={} windows={} tiled={} degraded={} stale={}\n",
            state.active.len(),
            state.primary.0,
            state.len(),
            state.tiled(),
            state
                .coordinators
                .values()
                .flat_map(|c| c.registry.iter())
                .filter(|(_, record)| record.desired.is_some()
                    && record.failed_target == record.desired)
                .count(),
            state.dropped_stale + state.coordinators.values().map(|c| c.stale_results).sum::<u64>()
        ),
        "status --json" => serde_json::to_string(&serde_json::json!({
            "version": 1,
            "state": "running",
            "displays": state.active.len(),
            "primary_display": state.primary,
            "windows": state.len(),
            "tiled": state.tiled(),
            "floating": state.coordinators.values().flat_map(|c| c.registry.iter())
                .filter(|(_, record)| record.observed.mode == tylora_core::registry::ManagementMode::Floating).count(),
            "active_workspaces": state.workspaces.iter().filter_map(|(display, engine)| engine.active(*display)
                .map(|id| serde_json::json!({"display":display,"id":id}))).collect::<Vec<_>>(),
            "layout": state.coordinators.get(&state.primary).map_or("bsp", |c| c.layout.kind().name()),
            "layouts": layout_current_json(state),
            "layout_metrics": state.coordinators.iter().filter(|(display, _)| state.active.contains(display)).map(|(display, c)| {
                let (viewport_calculations, viewport_total_us) = c.layout.viewport_metrics();
                serde_json::json!({
                    "display": display,
                    "layout_calculations": c.metrics.calculations,
                    "layout_calculation_total_us": c.metrics.calculation_total_us,
                    "layout_calculation_max_us": c.metrics.calculation_max_us,
                    "viewport_calculations": viewport_calculations,
                    "viewport_total_us": viewport_total_us,
                    "geometry_applies": c.metrics.applies,
                    "geometry_apply_total_us": c.metrics.apply_total_us,
                    "geometry_apply_max_us": c.metrics.apply_max_us,
                    "ax_failures": c.metrics.ax_failures,
                    "geometry_mismatches": c.metrics.geometry_mismatches,
                })
            }).collect::<Vec<_>>(),
            "layout_zoom": state.layout_zoom.iter().map(|((display, space), zoom)| {
                let record = state.coordinators.get(display).and_then(|coordinator| coordinator.registry.get(zoom.id));
                let confirmed = record.is_some_and(|record| record.generation == zoom.generation && record.desired.is_some_and(|desired| record.applied.is_some_and(|actual| actual.near(desired, 1.0))));
                serde_json::json!({"display": display, "native_space_id": space, "window": zoom.id, "cg_window_id": zoom.cg_window_id, "confirmed": confirmed})
            }).collect::<Vec<_>>(),
            "pending_display_moves": state.pending_display.values().map(|pending| serde_json::json!({
                "cg_window_id": pending.prepared.cg_window_id,
                "source_display": pending.prepared.source_display,
                "target_display": pending.prepared.target_display,
                "target_space_id": pending.prepared.target_space,
                "native_verified": pending.verified,
                "elapsed_ms": pending.issued.elapsed().as_millis(),
            })).collect::<Vec<_>>(),
            "accessibility_trusted": accessibility_trusted(),
            "native_spaces": state.native_spaces,
            "native_space_layout": true,
            "degraded": state.coordinators.values().flat_map(|c| c.registry.iter()).filter(|(_, record)| record.desired.is_some() && record.failed_target == record.desired).count(),
            "stale_results": state.dropped_stale + state.coordinators.values().map(|c| c.stale_results).sum::<u64>()
        })).unwrap_or_default(),
        "space list --json" => serde_json::to_string(&serde_json::json!({"version":1,"spaces":state.native_spaces})).unwrap_or_default(),
        "space capabilities --json" => {
            use tylora_macos::spaces::{ExperimentalSkyLightBackend, NativeSpaceBackend};
            serde_json::to_string(&serde_json::json!({"version":1,"capabilities":ExperimentalSkyLightBackend::new().capabilities()})).unwrap_or_default()
        }
        "diagnostics --json" => serde_json::to_string(&serde_json::json!({
            "version":1,
            "daemon":"running",
            "accessibility_trusted":accessibility_trusted(),
            "native_spaces":state.native_spaces.len(),
            "managed_windows":state.len(),
            "pending_display_moves":state.pending_display.len(),
            "stale_results":state.dropped_stale + state.coordinators.values().map(|c| c.stale_results).sum::<u64>(),
            "degraded":state.coordinators.values().flat_map(|c| c.registry.iter())
                .filter(|(_, record)| record.desired.is_some() && record.failed_target == record.desired).count(),
        })).unwrap_or_default(),
        "display list" => {
            use std::fmt::Write as _;
            let mut entries: Vec<_> = state
                .active
                .iter()
                .filter_map(|id| state.areas.get(id))
                .collect();
            entries.sort_by_key(|area| area.display.0);
            let mut out = String::new();
            for area in entries {
                let _ = writeln!(
                    out,
                    "{}\tuuid={:?}\tfull={:?}\twork={:?}\tscale={:.2}\t{}",
                    area.display.0,
                    area.stable_uuid.map(|value| format!("{value:032x}")),
                    area.full_frame,
                    area.frame,
                    area.scale,
                    if area.primary { "primary" } else { "secondary" }
                );
            }
            out
        }
        "display list --json" => {
            let mut entries: Vec<_> = state.active.iter().filter_map(|id| state.areas.get(id)).collect();
            entries.sort_by_key(|area| area.display.0);
            let displays: Vec<_> = entries.into_iter().map(|area| serde_json::json!({
                "id": area.display,
                "uuid": area.stable_uuid.map(|value| format!("{value:032x}")),
                "full_frame": area.full_frame,
                "work_area": area.frame,
                "scale": area.scale,
                "primary": area.primary,
            })).collect();
            serde_json::to_string(&serde_json::json!({"version":1,"displays":displays})).unwrap_or_default()
        }
        "reconcile" => match adapter.commands.try_send(ManagedCommand::Reconcile) {
            Ok(()) => "reconciliation scheduled\n".into(),
            Err(_) => "worker busy; retry reconcile\n".into(),
        },
        "windows --json" => {
            let windows: Vec<_> = state.coordinators.iter().flat_map(|(display, coordinator)| {
                coordinator.registry.iter().map(move |(id, record)| {
                    let workspace = state.workspaces.get(display).and_then(|engine| engine.owner(*id));
                    serde_json::json!({
                        "pid":id.process.pid,"token":id.token,"cg_window_id":record.observed.cg_window_id,
                        "application_id":record.observed.application_id,"native_space_id":record.observed.native_space_id,
                        "display":display,"workspace":workspace,
                        "floating":record.observed.mode == tylora_core::registry::ManagementMode::Floating,
                    })
                })
            }).collect();
            serde_json::to_string(&serde_json::json!({"version":1,"windows":windows})).unwrap_or_default()
        }
        "windows" => {
            let mut records: Vec<_> = state
                .coordinators
                .values()
                .flat_map(|c| c.registry.iter())
                .collect();
            records.sort_by_key(|(id, _)| **id);
            let in_tree: std::collections::HashSet<_> = state
                .coordinators
                .values()
                .flat_map(|c| c.layout.windows())
                .collect();
            let mut out = String::from(
                "PID EPOCH TOKEN GENERATION STATE MODE DISPLAY OBSERVED DESIRED APPLIED LAST_OK_MS PENDING FAILED_TARGET ATTEMPTS IN_TREE\n",
            );
            for (id, record) in records {
                out.push_str(&format!(
                    "{} {} {} {} {:?} {:?} {} {:?} {:?} {:?} {} {:?} {:?} {} {}\n",
                    id.process.pid,
                    id.process.epoch,
                    id.token,
                    record.generation,
                    record.observed.lifecycle,
                    record.observed.mode,
                    record
                        .observed
                        .display_id
                        .map(|d| d.0.to_string())
                        .unwrap_or_else(|| "?".into()),
                    record.observed.frame,
                    record.desired,
                    record.applied,
                    record.last_successful_update.elapsed().as_millis(),
                    record.pending_apply,
                    record.failed_target,
                    record.failed_attempts,
                    in_tree.contains(id)
                ));
            }
            out
        }
        "layout list" | "layout list --json" => {
            let kinds: Vec<_> = LayoutKind::ALL
                .iter()
                .map(|kind| {
                    serde_json::json!({
                        "layout": kind.name(),
                        "description": match kind {
                            LayoutKind::Bsp => "binary space partitioning; new windows split the last region",
                            LayoutKind::Dwindle => "Hyprland-style dwindle; new windows split the focused window",
                            LayoutKind::Scrolling => "niri-style horizontal strip of columns with a scrolling viewport",
                        },
                    })
                })
                .collect();
            if command.ends_with("--json") {
                format!(
                    "{}\n",
                    serde_json::json!({"version": 1, "layouts": kinds, "current": layout_current_json(state)})
                )
            } else {
                let mut out = String::new();
                for kind in LayoutKind::ALL {
                    let used: Vec<_> = state
                        .coordinators
                        .values()
                        .filter(|c| state.active.contains(&c.display) && c.layout.kind() == kind)
                        .map(|c| c.display.0.to_string())
                        .collect();
                    out.push_str(&format!(
                        "{}{}\n",
                        kind.name(),
                        if used.is_empty() {
                            String::new()
                        } else {
                            format!(" (active on display {})", used.join(", "))
                        }
                    ));
                }
                out
            }
        }
        "layout current" | "layout current --json" => {
            let current = layout_current_json(state);
            if command.ends_with("--json") {
                format!("{}\n", serde_json::json!({"version": 1, "displays": current}))
            } else {
                let mut out = String::new();
                for entry in current {
                    out.push_str(&format!(
                        "display={} space={} workspace={} layout={} windows={}\n",
                        entry["display"],
                        entry["native_space_id"],
                        entry["workspace"],
                        entry["layout"].as_str().unwrap_or("?"),
                        entry["windows"]
                    ));
                    if let Some(scrolling) = entry.get("scrolling").filter(|v| !v.is_null()) {
                        out.push_str(&format!(
                            "  mode={} viewport={:.0} strip={:.0}\n",
                            scrolling["mode"].as_str().unwrap_or("?"),
                            scrolling["viewport_offset"].as_f64().unwrap_or(0.0),
                            scrolling["strip_width"].as_f64().unwrap_or(0.0)
                        ));
                        for column in scrolling["columns"].as_array().into_iter().flatten() {
                            out.push_str(&format!(
                                "  pid={} token={} width={:.0}% placement={}\n",
                                column["window"]["process"]["pid"],
                                column["window"]["token"],
                                column["width_fraction"].as_f64().unwrap_or(0.0) * 100.0,
                                column["placement"]["state"].as_str().unwrap_or("?")
                            ));
                        }
                    }
                }
                out
            }
        }
        "layout" => {
            let mut out = String::new();
            for coordinator in state
                .coordinators
                .values()
                .filter(|c| state.active.contains(&c.display))
            {
                out.push_str(&format!(
                    "display={} work_area={:?}\n",
                    coordinator.display.0, coordinator.work_area
                ));
                for (node, axis, ratio) in coordinator.layout.splits() {
                    out.push_str(&format!(
                        "split={} axis={axis:?} ratio={ratio:.3}\n",
                        node.0
                    ));
                }
                for (id, frame) in coordinator.layout.calculate_layout(coordinator.work_area) {
                    out.push_str(&format!(
                        "pid={} epoch={} token={} target={:?}\n",
                        id.process.pid, id.process.epoch, id.token, frame
                    ));
                }
            }
            out
        }
        _ => "unknown command\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tylora_core::{
        registry::{ManagementMode, WindowLifecycle},
        Rect,
    };
    #[test]
    fn legacy_config_import_is_non_destructive() {
        let root = std::env::temp_dir().join(format!(
            "tylora-migration-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("fluxwm/config.toml");
        let new = root.join("tylora/config.toml");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "# old workspaces\nversion = 1\n").unwrap();
        migrate_legacy_file(&new, &old);
        assert_eq!(
            fs::read_to_string(&new).unwrap(),
            fs::read_to_string(&old).unwrap()
        );
        fs::write(&new, "version = 1\n# newer\n").unwrap();
        migrate_legacy_file(&new, &old);
        assert!(fs::read_to_string(&new).unwrap().contains("newer"));
        fs::remove_dir_all(root).unwrap();
    }
    fn area(id: u32, width: f64) -> WorkArea {
        let frame = Rect::new((id - 1) as f64 * 1000.0, 0.0, width, 600.0).unwrap();
        WorkArea {
            display: DisplayId(id),
            frame,
            full_frame: frame,
            scale: 1.0,
            stable_uuid: Some(id as u128),
            primary: id == 1,
        }
    }
    fn snapshot(token: u64, display: u32) -> WindowSnapshot {
        WindowSnapshot {
            id: ManagedWindowId {
                process: ProcessInstance { pid: 100, epoch: 1 },
                token,
            },
            cg_window_id: None,
            native_space_id: None,
            application_id: Some("test.app".into()),
            frame: Rect::new((display - 1) as f64 * 1000.0 + 10.0, 10.0, 300.0, 200.0).unwrap(),
            display_id: Some(DisplayId(display)),
            lifecycle: WindowLifecycle::Active,
            mode: ManagementMode::Tiled,
        }
    }
    #[test]
    fn window_rule_applies_to_new_window_and_survives_reconciliation() {
        let mut config = Config::default();
        config.window_rules.push(WindowRule {
            application_id: "test.app".into(),
            mode: WindowRuleMode::Floating,
            priority: 10,
            enabled: true,
        });
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let window = snapshot(1, 1);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 0);
        assert_eq!(
            state.coordinators[&DisplayId(1)]
                .registry
                .get(window.id)
                .unwrap()
                .observed
                .mode,
            tylora_core::registry::ManagementMode::Floating
        );
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        assert_eq!(
            state.coordinators[&DisplayId(1)]
                .registry
                .get(window.id)
                .unwrap()
                .observed
                .mode,
            tylora_core::registry::ManagementMode::Floating
        );
        state.handle_reconciled(window.id.process, Vec::new());
        assert!(!state.rule_applied.contains(&window.id));
    }
    #[test]
    fn confirmed_native_move_reflows_source_and_rebinds_target_without_ghost() {
        use tylora_macos::spaces::{DesktopNumber, MoveOutcome, NativeSpaceId};
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &Config::default());
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(2),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: false,
            },
        ];
        state
            .coordinators
            .get_mut(&DisplayId(1))
            .unwrap()
            .set_active_native_space(Some(7));
        let mut moving = snapshot(1, 1);
        moving.cg_window_id = Some(123);
        moving.native_space_id = Some(7);
        let mut sibling = snapshot(2, 1);
        sibling.cg_window_id = Some(124);
        sibling.native_space_id = Some(7);
        state.handle_reconciled(moving.id.process, vec![moving.clone(), sibling.clone()]);
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 2);
        let generation = state.coordinators[&DisplayId(1)]
            .registry
            .get(moving.id)
            .unwrap()
            .generation;
        let outcome = MoveOutcome {
            window_id: 123,
            source: NativeSpaceId(7),
            target: NativeSpaceId(4),
            backend: "mock",
            stage: "confirmed",
            operation_ms: 1,
            verification_ms: 1,
            confirmed: true,
            no_op: false,
        };
        assert!(state.commit_native_move(moving.id, generation, &outcome, Some(2)));
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 1);
        assert!(state.coordinators[&DisplayId(1)]
            .registry
            .get(moving.id)
            .is_none());
        assert_eq!(state.workspaces[&DisplayId(1)].owner(moving.id), Some(2));
        // A pre-move AX scan can arrive after native confirmation. It must
        // not put the moved window back into the source BSP.
        state.handle_reconciled(moving.id.process, vec![moving.clone(), sibling.clone()]);
        assert!(state.coordinators[&DisplayId(1)]
            .registry
            .get(moving.id)
            .is_none());
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 1);
        assert!(!state.commit_native_move(moving.id, generation, &outcome, Some(2)));
        state.native_spaces[0].active = false;
        state.native_spaces[1].active = true;
        let coordinator = state.coordinators.get_mut(&DisplayId(1)).unwrap();
        coordinator.set_active_native_space(Some(4));
        state
            .workspaces
            .get_mut(&DisplayId(1))
            .unwrap()
            .focus(DisplayId(1), 2, &mut ConfirmedVisibility)
            .unwrap();
        let mut reacquired = snapshot(3, 1);
        reacquired.cg_window_id = Some(123);
        reacquired.native_space_id = Some(4);
        state.handle_reconciled(reacquired.id.process, vec![reacquired.clone()]);
        assert!(!state.pending_native.contains_key(&moving.id));
        assert_eq!(
            state.workspaces[&DisplayId(1)].owner(reacquired.id),
            Some(2)
        );
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 1);
    }
    #[test]
    fn process_reconcile_partitions_displays_without_ghosts() {
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &Config::default(),
        );
        let process = ProcessInstance { pid: 100, epoch: 1 };
        state.handle_reconciled(process, vec![snapshot(1, 1), snapshot(2, 2)]);
        assert_eq!(state.coordinators[&DisplayId(1)].registry.len(), 1);
        assert_eq!(state.coordinators[&DisplayId(2)].registry.len(), 1);
        state.handle_reconciled(process, vec![snapshot(1, 2)]);
        assert_eq!(state.coordinators[&DisplayId(1)].registry.len(), 0);
        assert_eq!(state.coordinators[&DisplayId(2)].registry.len(), 1);
        assert_eq!(state.tiled(), 1);
    }
    #[test]
    fn disconnect_suspends_layout_and_reconnect_updates_resolution() {
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        state.handle_reconciled(ProcessInstance { pid: 100, epoch: 1 }, vec![snapshot(1, 2)]);
        let rehome = state.update_areas(vec![area(1, 1000.0)], &config);
        assert_eq!(rehome.len(), 1);
        assert_eq!(rehome[0].0, snapshot(1, 2).id);
        assert!(rehome[0].1.x >= 0.0 && rehome[0].1.y >= 0.0);
        assert!(state.plan().is_empty());
        assert!(state.coordinators[&DisplayId(2)]
            .registry
            .get(snapshot(1, 2).id)
            .is_some());
        state.update_areas(vec![area(1, 1000.0), area(2, 1200.0)], &config);
        assert_eq!(state.coordinators[&DisplayId(2)].work_area.width, 1200.0);
        assert_eq!(state.plan().len(), 1);
    }
    #[test]
    fn verified_display_move_reflows_source_and_reacquires_target_without_ghost() {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: true,
            },
        ];
        let mut window = snapshot(1, 1);
        window.cg_window_id = Some(321);
        window.native_space_id = Some(7);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let prepared = prepare_display_move(
            &state,
            &config,
            &StateCommand::MoveWindowToNextDisplay {
                window: Some(window.id),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            (
                prepared.source_display,
                prepared.target_display,
                prepared.source_space,
                prepared.target_space
            ),
            (DisplayId(1), DisplayId(2), 7, 4)
        );
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 1);
        state.layout_zoom.insert(
            (DisplayId(1), 7),
            ZoomTarget {
                id: window.id,
                cg_window_id: 321,
                generation: prepared.generation,
            },
        );
        assert!(state.commit_display_move(prepared));
        assert!(state.layout_zoom.is_empty());
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 0);
        let mut reacquired = window.clone();
        reacquired.id.token = 2;
        reacquired.native_space_id = Some(4);
        // The AX frame may still be on the old display until BSP applies.
        state.handle_reconciled(window.id.process, vec![reacquired.clone()]);
        assert_eq!(state.coordinators[&DisplayId(1)].registry.len(), 0);
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 1);
        assert!(state.coordinators[&DisplayId(2)]
            .registry
            .get(reacquired.id)
            .is_some());
        assert!(state.pending_display.is_empty());
    }

    #[test]
    fn display_order_and_wrap_follow_physical_arrangement() {
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(3, 1000.0), area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        state.areas.get_mut(&DisplayId(1)).unwrap().full_frame.x = 2000.0;
        state.areas.get_mut(&DisplayId(2)).unwrap().full_frame.x = -1000.0;
        state.areas.get_mut(&DisplayId(3)).unwrap().full_frame.x = 0.0;
        let order = ordered_displays(&state);
        assert_eq!(order, vec![DisplayId(2), DisplayId(3), DisplayId(1)]);
        assert_eq!(
            adjacent_display(&order, DisplayId(1), false, true),
            Some(DisplayId(2))
        );
        assert_eq!(
            adjacent_display(&order, DisplayId(2), true, true),
            Some(DisplayId(1))
        );
        assert_eq!(adjacent_display(&order, DisplayId(1), false, false), None);
        state.areas.get_mut(&DisplayId(1)).unwrap().full_frame.x = 0.0;
        state.areas.get_mut(&DisplayId(1)).unwrap().full_frame.y = 500.0;
        assert_eq!(
            ordered_displays(&state),
            vec![DisplayId(2), DisplayId(3), DisplayId(1)]
        );
    }

    #[test]
    fn floating_destination_preserves_fraction_and_clamps_size() {
        let source = Rect::new(-1200.0, 0.0, 1200.0, 800.0).unwrap();
        let target = Rect::new(0.0, -900.0, 800.0, 600.0).unwrap();
        let frame = Rect::new(-900.0, 200.0, 600.0, 400.0).unwrap();
        let placed = floating_destination(source, target, frame, true);
        assert_eq!(placed, Rect::new(100.0, -800.0, 600.0, 400.0).unwrap());
        let huge = floating_destination(
            source,
            target,
            Rect::new(-1200.0, 0.0, 1600.0, 1000.0).unwrap(),
            true,
        );
        assert_eq!(huge, target);
    }

    #[test]
    fn floating_transfer_keeps_mode_and_requests_target_work_area_frame() {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        let config = Config::default();
        let mut state =
            DisplayState::new(vec![area(1, 1000.0), area(2, 600.0)], DisplayId(1), &config);
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: true,
            },
        ];
        let mut window = snapshot(1, 1);
        window.cg_window_id = Some(400);
        window.native_space_id = Some(7);
        window.mode = ManagementMode::Floating;
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let prepared = prepare_display_move(
            &state,
            &config,
            &StateCommand::MoveWindowToNextDisplay {
                window: Some(window.id),
            },
            None,
        )
        .unwrap();
        assert_eq!(prepared.mode, ManagementMode::Floating);
        assert!(state.commit_display_move(prepared));
        let mut reacquired = window.clone();
        reacquired.id.token = 2;
        reacquired.native_space_id = Some(4);
        state.handle_reconciled(window.id.process, vec![reacquired.clone()]);
        assert_eq!(
            state.coordinators[&DisplayId(2)]
                .registry
                .get(reacquired.id)
                .unwrap()
                .observed
                .mode,
            ManagementMode::Floating
        );
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 0);
        assert_eq!(
            state.display_rehomes,
            vec![(reacquired.id, prepared.target_frame)]
        );
        assert!(prepared.target_frame.x >= state.areas[&DisplayId(2)].frame.x);
    }

    #[test]
    fn move_rejects_single_display_and_inactive_target_without_state_change() {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let mut window = snapshot(1, 1);
        window.cg_window_id = Some(401);
        window.native_space_id = Some(7);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let command = StateCommand::MoveWindowToNextDisplay {
            window: Some(window.id),
        };
        assert_eq!(
            prepare_display_move(&state, &config, &command, None)
                .err()
                .unwrap()
                .error,
            Some(CommandError::NoNeighbor)
        );
        state.update_areas(vec![area(1, 1000.0), area(2, 1000.0)], &config);
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: false,
            },
        ];
        assert_eq!(
            prepare_display_move(&state, &config, &command, None)
                .err()
                .unwrap()
                .error,
            Some(CommandError::Unsupported)
        );
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 1);
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 0);
    }

    #[test]
    fn duplicate_or_late_transfer_cannot_commit_a_replaced_window() {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: true,
            },
        ];
        let mut window = snapshot(1, 1);
        window.cg_window_id = Some(402);
        window.native_space_id = Some(7);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let command = StateCommand::MoveWindowToNextDisplay {
            window: Some(window.id),
        };
        let prepared = prepare_display_move(&state, &config, &command, None).unwrap();
        state.in_flight_display.insert(window.id);
        assert_eq!(
            prepare_display_move(&state, &config, &command, None)
                .err()
                .unwrap()
                .error,
            Some(CommandError::Unsupported)
        );
        state.in_flight_display.remove(&window.id);
        state.handle_reconciled(window.id.process, Vec::new());
        assert!(!state.commit_display_move(prepared));
        assert!(state.pending_display.is_empty());
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 0);
    }

    #[test]
    fn timeout_reconciliation_accepts_late_native_membership_without_ghost() {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        state.native_spaces = vec![
            NativeSpace {
                id: NativeSpaceId(7),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(4),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: true,
            },
        ];
        let mut window = snapshot(1, 1);
        window.cg_window_id = Some(403);
        window.native_space_id = Some(7);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let command = StateCommand::MoveWindowToNextDisplay {
            window: Some(window.id),
        };
        let prepared = prepare_display_move(&state, &config, &command, None).unwrap();
        state.pending_display.insert(
            403,
            PendingDisplayMove {
                prepared,
                issued: Instant::now(),
                verified: false,
            },
        );
        assert!(prepare_display_move(&state, &config, &command, None).is_err());
        let mut late = window.clone();
        late.id.token = 2;
        late.native_space_id = Some(4);
        state.handle_reconciled(window.id.process, vec![late.clone()]);
        assert_eq!(state.coordinators[&DisplayId(1)].registry.len(), 0);
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 1);
        assert!(state.pending_display.is_empty());
    }
    #[test]
    fn explicit_floating_window_works_without_system_focus() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let window = snapshot(1, 1);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        let (commands, _received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let result = execute_state_command(
            &mut state,
            &adapter,
            &config,
            &StateCommand::ToggleFloating {
                window: Some(window.id),
            },
            None,
        );
        assert!(result.ok, "{result:?}");
        assert_eq!(state.coordinators[&DisplayId(1)].layout.len(), 0);
    }

    #[test]
    fn floating_mode_follows_external_cross_display_migration() {
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        let window = snapshot(1, 1);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        state
            .coordinators
            .get_mut(&DisplayId(1))
            .unwrap()
            .execute(
                &StateCommand::ToggleFloating {
                    window: Some(window.id),
                },
                None,
            )
            .unwrap();
        // Synchronize the workspace engine with the explicit mode change.
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        assert!(state.workspaces[&DisplayId(1)]
            .workspace(1)
            .unwrap()
            .floating
            .contains(&window.id));

        // AX can still report Tiled when the window appears on its new display.
        let mut migrated = window.clone();
        migrated.display_id = Some(DisplayId(2));
        state.handle_reconciled(window.id.process, vec![migrated]);

        assert!(state.coordinators[&DisplayId(1)]
            .registry
            .get(window.id)
            .is_none());
        assert_eq!(
            state.coordinators[&DisplayId(2)]
                .registry
                .get(window.id)
                .unwrap()
                .observed
                .mode,
            ManagementMode::Floating
        );
        assert!(state.workspaces[&DisplayId(2)]
            .workspace(1)
            .unwrap()
            .floating
            .contains(&window.id));
        assert_eq!(state.coordinators[&DisplayId(2)].layout.len(), 0);
    }

    #[test]
    fn workspace_switch_and_move_update_real_coordinator_tree() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let first = snapshot(1, 1);
        let second = snapshot(2, 1);
        state.handle_reconciled(first.id.process, vec![first.clone(), second.clone()]);
        let command = StateCommand::MoveWindowToWorkspace {
            window: Some(first.id),
            workspace: 2,
        };
        let action = prepare_workspace_action(&state, &command, Some(first.id))
            .unwrap()
            .unwrap();
        assert_eq!(state.workspaces[&DisplayId(1)].owner(first.id), Some(1));
        assert!(apply_workspace_action(&mut state, action).ok);
        assert_eq!(state.workspaces[&DisplayId(1)].owner(first.id), Some(2));
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![second.id]
        );
        assert!(state.hidden_by_tylora.contains(&first.id));

        let action = prepare_workspace_action(
            &state,
            &StateCommand::FocusWorkspace { id: 2 },
            Some(second.id),
        )
        .unwrap()
        .unwrap();
        assert!(apply_workspace_action(&mut state, action).ok);
        assert_eq!(
            state.workspaces[&DisplayId(1)].active(DisplayId(1)),
            Some(2)
        );
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![first.id]
        );
        assert!(!state.hidden_by_tylora.contains(&first.id));
        assert!(state.hidden_by_tylora.contains(&second.id));

        let action =
            prepare_workspace_action(&state, &StateCommand::FocusWorkspace { id: 1 }, None)
                .unwrap()
                .unwrap();
        assert!(apply_workspace_action(&mut state, action).ok);
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![second.id]
        );
        assert_eq!(state.workspaces[&DisplayId(1)].owner(first.id), Some(2));
    }
    #[test]
    fn workspace_switch_is_independent_per_display_and_empty_repeat_is_noop() {
        let config = Config::default();
        let mut state = DisplayState::new(
            vec![area(1, 1000.0), area(2, 1000.0)],
            DisplayId(1),
            &config,
        );
        assert!(
            prepare_workspace_action(&state, &StateCommand::FocusWorkspace { id: 1 }, None)
                .unwrap()
                .is_none()
        );
        let action =
            prepare_workspace_action(&state, &StateCommand::FocusWorkspace { id: 4 }, None)
                .unwrap()
                .unwrap();
        assert!(apply_workspace_action(&mut state, action).ok);
        assert_eq!(
            state.workspaces[&DisplayId(1)].active(DisplayId(1)),
            Some(4)
        );
        assert_eq!(
            state.workspaces[&DisplayId(2)].active(DisplayId(2)),
            Some(1)
        );
    }
    #[test]
    fn tiled_mouse_up_applies_final_delta_inside_throttle_interval() {
        let mut config = Config::default();
        config.mouse.update_interval_ms = 1000;
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let first = snapshot(1, 1);
        let second = snapshot(2, 1);
        state.handle_reconciled(first.id.process, vec![first.clone(), second]);
        let (commands, _received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let start = MouseInput {
            id: first.id,
            button: MouseButton::Right,
            phase: MousePhase::Start,
            start: (100.0, 100.0),
            current: (100.0, 100.0),
            floating: false,
            at: Instant::now(),
        };
        let mut drag = DragSession::begin(start, &state).unwrap();
        drag.last_send = Instant::now();
        let before = state.coordinators[&DisplayId(1)].layout.splits()[0].2;
        drag.update(
            MouseInput {
                phase: MousePhase::End,
                current: (170.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert!(state.coordinators[&DisplayId(1)].layout.splits()[0].2 > before);
    }
    #[test]
    fn floating_right_drag_obeys_minimum_and_work_area() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let mut window = snapshot(1, 1);
        window.frame.x = 940.0;
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        state
            .coordinators
            .get_mut(&DisplayId(1))
            .unwrap()
            .execute(
                &StateCommand::ToggleFloating {
                    window: Some(window.id),
                },
                None,
            )
            .unwrap();
        let (commands, _received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let start = MouseInput {
            id: window.id,
            button: MouseButton::Right,
            phase: MousePhase::Start,
            start: (950.0, 100.0),
            current: (950.0, 100.0),
            floating: true,
            at: Instant::now(),
        };
        let mut drag = DragSession::begin(start, &state).unwrap();
        drag.update(
            MouseInput {
                phase: MousePhase::End,
                current: (1500.0, -500.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        let target = drag.latest_target.unwrap();
        assert!(target.width >= 80.0 && target.height >= 60.0);
        assert!(target.x >= 0.0 && target.y >= 0.0);
        assert!(target.x + target.width <= 1000.0);
        assert!(target.y + target.height <= 600.0);
    }
    #[test]
    fn tiled_mouse_resize_cancel_restores_split_ratio() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let first = snapshot(1, 1);
        let mut second = snapshot(2, 1);
        second.frame.x = 600.0;
        state.handle_reconciled(first.id.process, vec![first.clone(), second]);
        let (commands, _received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let start = MouseInput {
            id: first.id,
            button: MouseButton::Right,
            phase: MousePhase::Start,
            start: (100.0, 100.0),
            current: (100.0, 100.0),
            floating: false,
            at: Instant::now(),
        };
        let mut drag = DragSession::begin(start, &state).unwrap();
        let original = state.coordinators[&DisplayId(1)].layout.splits()[0].2;
        drag.update(
            MouseInput {
                phase: MousePhase::Update,
                current: (150.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert!(state.coordinators[&DisplayId(1)].layout.splits()[0].2 > original);
        drag.update(
            MouseInput {
                phase: MousePhase::Cancel,
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.splits()[0].2,
            original
        );
    }
    #[test]
    fn tiled_mouse_drop_swaps_tree_only_at_end() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let first = snapshot(1, 1);
        let mut second = snapshot(2, 1);
        second.frame.x = 600.0;
        state.handle_reconciled(first.id.process, vec![first.clone(), second.clone()]);
        let (commands, _received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let start = MouseInput {
            id: first.id,
            button: MouseButton::Left,
            phase: MousePhase::Start,
            start: (100.0, 100.0),
            current: (100.0, 100.0),
            floating: false,
            at: Instant::now(),
        };
        let mut drag = DragSession::begin(start, &state).unwrap();
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![first.id, second.id]
        );
        drag.update(
            MouseInput {
                phase: MousePhase::Update,
                current: (650.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![first.id, second.id]
        );
        drag.update(
            MouseInput {
                phase: MousePhase::End,
                current: (650.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert_eq!(
            state.coordinators[&DisplayId(1)].layout.windows(),
            vec![second.id, first.id]
        );
    }
    #[test]
    fn floating_drag_sends_final_target_after_intermediate_frame() {
        let config = Config::default();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let window = snapshot(1, 1);
        state.handle_reconciled(window.id.process, vec![window.clone()]);
        state
            .coordinators
            .get_mut(&DisplayId(1))
            .unwrap()
            .execute(
                &StateCommand::ToggleFloating {
                    window: Some(window.id),
                },
                None,
            )
            .unwrap();
        let (commands, received) = mpsc::sync_channel(8);
        let (_events, events) = mpsc::sync_channel(8);
        let adapter = ManagedAdapter { commands, events };
        let start = MouseInput {
            id: window.id,
            button: MouseButton::Left,
            phase: MousePhase::Start,
            start: (100.0, 100.0),
            current: (100.0, 100.0),
            floating: true,
            at: Instant::now(),
        };
        let mut drag = DragSession::begin(start, &state).unwrap();
        drag.update(
            MouseInput {
                phase: MousePhase::Update,
                current: (120.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert!(!drag.flush(&adapter, &config));
        let first = match received.try_recv().unwrap() {
            ManagedCommand::DragFrame { target, .. } => target,
            other => panic!("unexpected {other:?}"),
        };
        drag.update(
            MouseInput {
                phase: MousePhase::End,
                current: (160.0, 100.0),
                ..start
            },
            &mut state,
            &adapter,
            &config,
        );
        assert!(!drag.flush(&adapter, &config));
        drag.ax_pending = false; // mock AX acknowledgement
        assert!(!drag.flush(&adapter, &config));
        let final_target = match received.try_recv().unwrap() {
            ManagedCommand::DragFrame { target, .. } => target,
            other => panic!("unexpected {other:?}"),
        };
        assert!(final_target.x > first.x);
        drag.ax_pending = false;
        assert!(drag.flush(&adapter, &config));
    }

    // ------------------------------------------------ multi-layout (stage 6)

    fn space(id: u64, display: u32, active: bool) -> NativeSpace {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        NativeSpace {
            id: NativeSpaceId(id),
            desktop: DesktopNumber(id as u8),
            display: DisplayId(display),
            active,
        }
    }
    fn active_pairs(
        spaces: &[NativeSpace],
    ) -> Vec<(DisplayId, tylora_macos::spaces::NativeSpaceId)> {
        spaces
            .iter()
            .filter(|space| space.active)
            .map(|space| (space.display, space.id))
            .collect()
    }
    /// Activates `target` on display 1, as the Space observer would.
    fn switch_space(state: &mut DisplayState, config: &Config, ids: &[u64], target: u64) {
        let next: Vec<_> = ids.iter().map(|id| space(*id, 1, *id == target)).collect();
        let old = active_pairs(&state.native_spaces);
        let new = active_pairs(&next);
        state.apply_native_spaces(&old, &new, config);
        state.native_spaces = next;
    }
    /// Space switch followed by the full AX scan it triggers.
    fn switch_and_scan(
        state: &mut DisplayState,
        config: &Config,
        ids: &[u64],
        target: u64,
        windows: &[WindowSnapshot],
    ) {
        switch_space(state, config, ids, target);
        state.handle_reconciled(windows[0].id.process, windows.to_vec());
    }
    fn on_space(token: u64, space: u64) -> WindowSnapshot {
        let mut window = snapshot(token, 1);
        window.native_space_id = Some(space);
        window.cg_window_id = Some(1000 + token as u32);
        window
    }
    fn layout_config() -> Config {
        let mut config = Config::default();
        config.workspaces[0].name = "Code".into();
        config.workspaces[0].native_space_id = Some(7);
        config.workspaces[0].layout = Some(LayoutKind::Scrolling);
        config.workspaces[1].name = "Browser".into();
        config.workspaces[1].native_space_id = Some(4);
        config.workspaces[2].name = "Chat".into();
        config.workspaces[2].native_space_id = Some(5);
        config.workspaces[2].layout = Some(LayoutKind::Dwindle);
        config
    }
    fn mock_adapter() -> (ManagedAdapter, Receiver<ManagedCommand>) {
        let (commands, received) = mpsc::sync_channel(64);
        let (_events, events) = mpsc::sync_channel(8);
        (ManagedAdapter { commands, events }, received)
    }

    #[test]
    fn each_native_space_restores_its_own_layout() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let ids = [7, 4, 5, 9];
        switch_space(&mut state, &config, &ids, 7);
        let windows = [
            on_space(1, 7),
            on_space(2, 7),
            on_space(3, 4),
            on_space(4, 5),
        ];
        state.handle_reconciled(windows[0].id.process, windows.to_vec());
        let display = DisplayId(1);
        assert_eq!(
            state.coordinators[&display].layout.kind(),
            LayoutKind::Scrolling
        );
        assert_eq!(state.coordinators[&display].layout.len(), 2);
        switch_and_scan(&mut state, &config, &ids, 4, &windows);
        assert_eq!(state.coordinators[&display].layout.kind(), LayoutKind::Bsp);
        assert_eq!(
            state.coordinators[&display].layout.windows(),
            vec![windows[2].id]
        );
        switch_and_scan(&mut state, &config, &ids, 5, &windows);
        assert_eq!(
            state.coordinators[&display].layout.kind(),
            LayoutKind::Dwindle
        );
        // An unbound Space gets the default layout and remembers a runtime change.
        switch_and_scan(&mut state, &config, &ids, 9, &windows);
        assert_eq!(state.coordinators[&display].layout.kind(), LayoutKind::Bsp);
        state
            .coordinators
            .get_mut(&display)
            .unwrap()
            .execute(
                &StateCommand::SetLayout {
                    layout: LayoutKind::Dwindle,
                },
                None,
            )
            .unwrap();
        switch_and_scan(&mut state, &config, &ids, 7, &windows);
        let coordinator = &state.coordinators[&display];
        assert_eq!(coordinator.layout.kind(), LayoutKind::Scrolling);
        assert_eq!(
            coordinator.layout.windows(),
            vec![windows[0].id, windows[1].id]
        );
        switch_and_scan(&mut state, &config, &ids, 9, &windows);
        assert_eq!(
            state.coordinators[&display].layout.kind(),
            LayoutKind::Dwindle
        );
    }

    #[test]
    fn leaving_and_returning_keeps_scrolling_widths_and_viewport() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let ids = [7, 4];
        switch_space(&mut state, &config, &ids, 7);
        let windows: Vec<_> = (1..=4).map(|token| on_space(token, 7)).collect();
        state.handle_reconciled(windows[0].id.process, windows.clone());
        let display = DisplayId(1);
        let coordinator = state.coordinators.get_mut(&display).unwrap();
        coordinator
            .execute(
                &StateCommand::ResizeScrollingColumn { delta: Some(0.2) },
                Some(windows[1].id),
            )
            .unwrap();
        coordinator
            .execute(
                &StateCommand::FocusWindow {
                    direction: tylora_core::commands::Direction::Left,
                },
                Some(windows[3].id),
            )
            .unwrap();
        let before = coordinator.layout.summary(coordinator.work_area);
        // While away, the scan drops the Code windows from the registry.
        switch_and_scan(&mut state, &config, &ids, 4, &windows);
        assert_eq!(state.coordinators[&display].registry.len(), 0);
        // Back on Code the restored strip waits for the scan instead of
        // discarding the not-yet-observed columns.
        switch_space(&mut state, &config, &ids, 7);
        assert_eq!(state.coordinators[&display].awaiting(), 4);
        assert!(
            state.plan().is_empty(),
            "no AX writes for unobserved windows"
        );
        state.handle_reconciled(windows[0].id.process, windows.clone());
        assert_eq!(state.coordinators[&display].awaiting(), 0);
        let coordinator = &state.coordinators[&display];
        let after = coordinator.layout.summary(coordinator.work_area);
        assert_eq!(before.order, after.order);
        let (b, a) = (before.scrolling.unwrap(), after.scrolling.unwrap());
        assert_eq!(b.viewport_offset, a.viewport_offset);
        assert_eq!(
            b.columns
                .iter()
                .map(|c| c.width_fraction)
                .collect::<Vec<_>>(),
            a.columns
                .iter()
                .map(|c| c.width_fraction)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn moving_window_to_other_space_uses_target_layout_without_ghosts() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        let ids = [7, 4];
        switch_space(&mut state, &config, &ids, 7);
        let mut windows: Vec<_> = (1..=3).map(|token| on_space(token, 7)).collect();
        state.handle_reconciled(windows[0].id.process, windows.clone());
        // macOS moved window 3 to the Browser Space.
        windows[2].native_space_id = Some(4);
        state.handle_reconciled(windows[0].id.process, windows.clone());
        let display = DisplayId(1);
        assert_eq!(state.coordinators[&display].layout.len(), 2);
        switch_and_scan(&mut state, &config, &ids, 4, &windows);
        let coordinator = &state.coordinators[&display];
        assert_eq!(coordinator.layout.kind(), LayoutKind::Bsp);
        assert_eq!(coordinator.layout.windows(), vec![windows[2].id]);
        let frames = coordinator.layout.calculate_layout(coordinator.work_area);
        assert_eq!(frames.len(), 1, "no ghost of the Scrolling columns");
        assert!(frames[0].1.x >= 0.0 && frames[0].1.x + frames[0].1.width <= 1000.0);
    }

    #[test]
    fn config_layout_changes_apply_but_runtime_override_survives_unrelated_reload() {
        let mut config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        switch_space(&mut state, &config, &[7, 4], 4);
        let display = DisplayId(1);
        state
            .coordinators
            .get_mut(&display)
            .unwrap()
            .execute(
                &StateCommand::SetLayout {
                    layout: LayoutKind::Scrolling,
                },
                None,
            )
            .unwrap();
        config.gaps.inner = 12.0;
        state.apply_live_config(&config);
        assert_eq!(
            state.coordinators[&display].layout.kind(),
            LayoutKind::Scrolling,
            "unrelated reload keeps runtime layout"
        );
        config.workspaces[1].layout = Some(LayoutKind::Dwindle);
        state.apply_live_config(&config);
        assert_eq!(
            state.coordinators[&display].layout.kind(),
            LayoutKind::Dwindle
        );
    }

    #[test]
    fn scrolling_environment_lists_other_displays() {
        let config = Config::default();
        let state = DisplayState::new(vec![area(1, 1000.0), area(2, 800.0)], DisplayId(1), &config);
        let environment = state.coordinators[&DisplayId(1)].environment();
        assert_eq!(environment.display_frame, Some(area(1, 1000.0).full_frame));
        assert_eq!(environment.other_displays, vec![area(2, 800.0).full_frame]);
    }

    #[test]
    fn scrolling_commands_route_to_pointer_or_focused_display_and_report_layout() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        switch_space(&mut state, &config, &[7, 4], 4);
        let windows: Vec<_> = (1..=2).map(|token| on_space(token, 4)).collect();
        state.handle_reconciled(windows[0].id.process, windows.clone());
        let (adapter, _received) = mock_adapter();
        let result = execute_state_command(
            &mut state,
            &adapter,
            &config,
            &StateCommand::ScrollLayoutRight,
            Some(windows[0].id),
        );
        assert!(!result.ok);
        assert_eq!(result.error, Some(CommandError::Unsupported));
        let result = execute_state_command(
            &mut state,
            &adapter,
            &config,
            &StateCommand::SetLayout {
                layout: LayoutKind::Scrolling,
            },
            Some(windows[0].id),
        );
        assert!(result.ok, "{}", result.message);
        assert_eq!(result.message, "layout set to scrolling");
        let again = execute_state_command(
            &mut state,
            &adapter,
            &config,
            &StateCommand::SetLayout {
                layout: LayoutKind::Scrolling,
            },
            Some(windows[0].id),
        );
        assert!(again.ok);
        let current = layout_current_json(&state);
        assert_eq!(current[0]["layout"], "scrolling");
        assert_eq!(current[0]["workspace"], 2);
        assert_eq!(current[0]["windows"], 2);
    }

    #[test]
    fn layout_zoom_in_scrolling_keeps_widths_and_viewport() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        switch_space(&mut state, &config, &[7, 4], 7);
        let windows: Vec<_> = (1..=3).map(|token| on_space(token, 7)).collect();
        state.handle_reconciled(windows[0].id.process, windows.clone());
        let display = DisplayId(1);
        let before = {
            let c = &state.coordinators[&display];
            c.layout.summary(c.work_area).scrolling.unwrap()
        };
        let plain: HashMap<_, _> = state.plan().into_iter().map(|c| (c.id, c.target)).collect();
        state.layout_zoom.insert(
            (display, 7),
            ZoomTarget {
                id: windows[1].id,
                cg_window_id: 1002,
                generation: 0,
            },
        );
        state.coordinators.get_mut(&display).unwrap().version += 1;
        let zoomed: HashMap<_, _> = state
            .coordinators
            .get_mut(&display)
            .unwrap()
            .registry
            .iter()
            .map(|(id, r)| (*id, r.desired))
            .collect();
        let _ = state.plan();
        let target = state.coordinators[&display]
            .registry
            .get(windows[1].id)
            .unwrap()
            .desired
            .unwrap();
        assert_eq!(
            (target.x, target.width),
            (0.0, 1000.0),
            "zoomed fills the work area"
        );
        assert_eq!(
            state.coordinators[&display]
                .registry
                .get(windows[0].id)
                .unwrap()
                .desired,
            zoomed[&windows[0].id],
            "other columns keep their frames"
        );
        let after = {
            let c = &state.coordinators[&display];
            c.layout.summary(c.work_area).scrolling.unwrap()
        };
        assert_eq!(before.viewport_offset, after.viewport_offset);
        state.layout_zoom.clear();
        let _ = state.plan();
        assert_eq!(
            state.coordinators[&display]
                .registry
                .get(windows[1].id)
                .unwrap()
                .desired,
            plain
                .get(&windows[1].id)
                .copied()
                .or(zoomed[&windows[1].id]),
            "unzoom returns to the strip frame"
        );
    }

    #[test]
    fn passive_focus_on_parked_column_scrolls_but_pointer_focus_never_does() {
        let config = layout_config();
        let mut state = DisplayState::new(vec![area(1, 1000.0)], DisplayId(1), &config);
        switch_space(&mut state, &config, &[7, 4], 7);
        let windows: Vec<_> = (1..=4).map(|token| on_space(token, 7)).collect();
        state.handle_reconciled(windows[0].id.process, windows.clone());
        let coordinator = state.coordinators.get_mut(&DisplayId(1)).unwrap();
        coordinator
            .execute(&StateCommand::FocusNextWindow, Some(windows[3].id))
            .unwrap();
        assert!(coordinator.is_parked(windows[3].id));
        let offset = |c: &Coordinator<TilingLayout>| {
            c.layout
                .summary(c.work_area)
                .scrolling
                .unwrap()
                .viewport_offset
        };
        let before = offset(coordinator);
        // Pointer focus updates the active column but never moves the strip.
        coordinator.focus_hint(windows[3].id, FocusOrigin::Pointer);
        assert_eq!(offset(coordinator), before);
        assert!(coordinator.is_parked(windows[3].id));
        assert!(coordinator.focus_hint(windows[3].id, FocusOrigin::System));
        assert!(!coordinator.is_parked(windows[3].id));
    }

    // ------------------------------------------------ Space ID replacement

    fn spaces_on(entries: &[(u64, u32, u8)]) -> Vec<NativeSpace> {
        use tylora_macos::spaces::{DesktopNumber, NativeSpaceId};
        entries
            .iter()
            .map(|&(id, display, desktop)| NativeSpace {
                id: NativeSpaceId(id),
                desktop: DesktopNumber(desktop),
                display: DisplayId(display),
                active: desktop == 1,
            })
            .collect()
    }
    fn uuid(display: DisplayId) -> Option<u128> {
        Some(0xabc0 + u128::from(display.0))
    }

    #[test]
    fn recreated_space_is_rebound_to_its_replacement() {
        let mut config = Config::default();
        config.workspaces[0].native_space_id = Some(7);
        config.workspaces[1].native_space_id = Some(4607);
        let mut slots = HashMap::new();
        let before = spaces_on(&[(7, 1, 1), (4, 1, 2), (4607, 2, 1)]);
        assert!(plan_space_rebinds(&config, &before, uuid, &mut slots).is_empty());
        assert_eq!(slots[&slot_key(2, 4607)].space_id, 4607);
        // The second display's Space came back as 4665.
        let after = spaces_on(&[(7, 1, 1), (4, 1, 2), (4665, 2, 1)]);
        let rebinds = plan_space_rebinds(&config, &after, uuid, &mut slots);
        assert_eq!(rebinds, vec![(2, 4607, 4665)]);
        assert_eq!(slots[&slot_key(2, 4665)].space_id, 4665);
        assert!(!slots.contains_key(&slot_key(2, 4607)));
    }

    #[test]
    fn rebind_refuses_ambiguous_or_taken_or_unknown_slots() {
        let mut config = Config::default();
        config.workspaces[0].native_space_id = Some(10);
        config.workspaces[1].native_space_id = Some(20);
        let mut slots = HashMap::new();
        plan_space_rebinds(
            &config,
            &spaces_on(&[(10, 1, 1), (20, 1, 2)]),
            uuid,
            &mut slots,
        );
        // Space 20 vanished and its slot (display 1, Desktop 2) is now held by
        // Space 10, which another workspace owns: no rebind.
        let taken = spaces_on(&[(10, 1, 2)]);
        assert!(plan_space_rebinds(&config, &taken, uuid, &mut slots).is_empty());
        // Different display: not the same slot.
        let elsewhere = spaces_on(&[(10, 1, 1), (30, 2, 2)]);
        assert!(plan_space_rebinds(&config, &elsewhere, uuid, &mut slots).is_empty());
        // Without a remembered slot nothing is inferred from Desktop order.
        let mut empty = HashMap::new();
        config.workspaces[2].native_space_id = Some(99);
        let fresh = spaces_on(&[(10, 1, 1), (20, 1, 2), (31, 1, 3)]);
        assert!(plan_space_rebinds(&config, &fresh, uuid, &mut empty).is_empty());
        assert!(!empty.contains_key(&slot_key(3, 99)));
    }

    #[test]
    fn extra_space_of_a_workspace_is_rebound_too() {
        let mut config = Config::default();
        config.workspaces[0].native_space_id = Some(7);
        config.workspaces[0].native_space_ids = vec![4607];
        let mut slots = HashMap::new();
        plan_space_rebinds(
            &config,
            &spaces_on(&[(7, 1, 1), (4607, 2, 1)]),
            uuid,
            &mut slots,
        );
        let rebinds = plan_space_rebinds(
            &config,
            &spaces_on(&[(7, 1, 1), (4665, 2, 1)]),
            uuid,
            &mut slots,
        );
        assert_eq!(rebinds, vec![(1, 4607, 4665)]);
    }

    #[test]
    fn workspace_instance_follows_recent_focus_and_toggles() {
        let mut history = SpaceFocusHistory::new();
        let now = Instant::now();
        let both = [(7, true), (4665, true)];
        // Nothing known: an already visible instance, else the first.
        assert_eq!(choose_workspace_instance(&both, &history, None), Some(7));
        assert_eq!(
            choose_workspace_instance(&[(7, false), (4665, true)], &history, None),
            Some(4665)
        );
        // On Chat (Space 5): the instance focused most recently wins.
        history.insert(7, now);
        history.insert(4665, now + Duration::from_secs(1));
        assert_eq!(
            choose_workspace_instance(&both, &history, Some(5)),
            Some(4665)
        );
        // On one instance, pressing again goes to the other one and back,
        // even when keyboard focus stayed on the other display.
        history.insert(4665, now + Duration::from_secs(5));
        assert_eq!(
            choose_workspace_instance(&both, &history, Some(4665)),
            Some(7)
        );
        assert_eq!(
            choose_workspace_instance(&both, &history, Some(7)),
            Some(4665)
        );
        // A single bound Space is always chosen.
        assert_eq!(
            choose_workspace_instance(&[(5, false)], &history, Some(5)),
            Some(5)
        );
        assert_eq!(choose_workspace_instance(&[], &history, None), None);
    }
    // ------------------------------------------- display move keeps the Desktop

    #[test]
    fn display_move_keeps_desktop_number_and_workspace() {
        let mut config = Config::default();
        config.workspaces[0].native_space_id = Some(7);
        config.workspaces[0].native_space_ids = vec![4678];
        config.workspaces[1].native_space_id = Some(4);
        // Mac: Desktops 1..3 (7, 4, 5); second display: Desktop 1 only.
        let spaces = spaces_on(&[(7, 1, 1), (4, 1, 2), (5, 1, 3), (4678, 2, 1)]);
        // Browser has its own instance on the second display.
        assert_eq!(
            choose_display_target(&config, &spaces, 7, DisplayId(2)),
            Some((DisplayTarget::Existing(4678), None))
        );
        // Code (Desktop 2) has none and display 2 lacks Desktop 2: create it
        // and bind it to Code.
        assert_eq!(
            choose_display_target(&config, &spaces, 4, DisplayId(2)),
            Some((
                DisplayTarget::Create {
                    desktop: 2,
                    missing: 1
                },
                Some(2)
            ))
        );
        // Desktop 3 needs two more Desktops; an unbound Space binds nothing.
        assert_eq!(
            choose_display_target(&config, &spaces, 5, DisplayId(2)),
            Some((
                DisplayTarget::Create {
                    desktop: 3,
                    missing: 2
                },
                None
            ))
        );
        // Once display 2 has Desktop 2 it is reused, also in reverse.
        let more = spaces_on(&[(7, 1, 1), (4, 1, 2), (4678, 2, 1), (4700, 2, 2)]);
        assert_eq!(
            choose_display_target(&config, &more, 4, DisplayId(2)),
            Some((DisplayTarget::Existing(4700), None))
        );
        assert_eq!(
            choose_display_target(&config, &more, 4700, DisplayId(1)),
            Some((DisplayTarget::Existing(4), None))
        );
        // Unknown source Space: fall back to the caller's active Space.
        assert_eq!(
            choose_display_target(&config, &more, 999, DisplayId(2)),
            None
        );
    }

    #[test]
    fn display_move_never_creates_unbounded_desktops() {
        let config = Config::default();
        let spaces = spaces_on(&[(1, 1, 12), (2, 2, 1)]);
        assert_eq!(
            choose_display_target(&config, &spaces, 1, DisplayId(2)),
            None
        );
    }

}
