//! AX ownership, observers, and geometry application live on one dedicated thread.

use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    mem::size_of,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use tracing::{debug, info, warn};
use tylora_core::animation::{
    AnimationConfig, AnimationEvent, AnimationMetrics, AnimationPolicy, AnimationScheduler,
    Decision, Frame, SlideDirection, StartRequest, Started, Transition, WindowConstraints,
};
use tylora_core::coordinator::ApplyCommand;
use tylora_core::registry::{
    ManagedWindowId, ManagementMode, ProcessInstance, WindowLifecycle, WindowSnapshot,
};
pub const VISIBILITY_ROLLBACK_FAILED: i32 = -32001;
/// `Applied` error: an animation was stopped before reaching its target
/// (window dragged by the user). The coordinator retries after its backoff.
pub const ANIMATION_CANCELLED: i32 = -32002;
/// Frame writes this slow, twice in a row, mean the app cannot keep up
/// with an animation; its windows then move instantly for a while. One
/// write slower than `STALLED_FRAME_WRITE` is enough. A first resize of an
/// Electron window often takes ~50 ms once and is no reason to give up.
const SLOW_FRAME_WRITE: Duration = Duration::from_millis(40);
const STALLED_FRAME_WRITE: Duration = Duration::from_millis(120);
const SLOW_APPLICATION_COOLDOWN: Duration = Duration::from_secs(600);
static ANIMATION_METRICS: Mutex<Option<AnimationMetrics>> = Mutex::new(None);
static ANIMATION_STATE: Mutex<Option<AnimationState>> = Mutex::new(None);

/// What the AX worker currently does with animations, for diagnostics.
#[derive(Debug, Clone, Serialize)]
pub struct AnimationState {
    pub policy_enabled: bool,
    pub backend: tylora_core::animation::ResolvedBackend,
    pub reduce_motion: bool,
    pub accessibility: bool,
    pub slow_applications: Vec<i32>,
    pub instant_reasons: HashMap<String, u64>,
}

/// Counters of the AX worker's animation scheduler (zero before it ran).
pub fn animation_metrics() -> AnimationMetrics {
    ANIMATION_METRICS
        .lock()
        .ok()
        .and_then(|metrics| *metrics)
        .unwrap_or_default()
}

pub fn animation_state() -> Option<AnimationState> {
    ANIMATION_STATE.lock().ok().and_then(|state| state.clone())
}
static POINTER_FOCUS_SUPPRESSED: AtomicBool = AtomicBool::new(false);
static AX_RUN_LOOP: Mutex<Option<usize>> = Mutex::new(None);
pub fn suppress_pointer_focus(value: bool) {
    POINTER_FOCUS_SUPPRESSED.store(value, Ordering::Release);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct RecoveryRecord {
    process: ProcessInstance,
    frame: Rect,
    title: Option<String>,
}
/// A window Tylora left mostly outside every display, with a frame that
/// brings it back. Persisted so a crash cannot strand windows off screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct OffscreenRecord {
    process: ProcessInstance,
    frame: Rect,
    title: Option<String>,
    safe: Rect,
}

fn offscreen_path() -> PathBuf {
    std::env::temp_dir().join(format!("tylora-{}.offscreen.json", unsafe { geteuid() }))
}

/// Bounds of every active display in global top-left coordinates.
fn display_bounds() -> Vec<Rect> {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count) } != 0 {
        return Vec::new();
    }
    ids[..count as usize]
        .iter()
        .filter_map(|id| {
            let bounds = unsafe { CGDisplayBounds(*id) };
            Rect::new(
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            )
            .ok()
        })
        .collect()
}

fn overlap_area(a: Rect, b: Rect) -> f64 {
    let width = (a.x + a.width).min(b.x + b.width) - a.x.max(b.x);
    let height = (a.y + a.height).min(b.y + b.height) - a.y.max(b.y);
    width.max(0.0) * height.max(0.0)
}

/// True when less than a quarter of the window is on any display.
fn mostly_offscreen(frame: Rect, displays: &[Rect]) -> bool {
    let area = frame.width * frame.height;
    if area <= 0.0 || displays.is_empty() {
        return false;
    }
    let visible: f64 = displays
        .iter()
        .map(|display| overlap_area(frame, *display))
        .sum();
    visible < area * 0.25
}

/// The window's size, moved fully inside the display it overlaps most.
fn safe_frame(frame: Rect, displays: &[Rect]) -> Option<Rect> {
    let display = displays
        .iter()
        .copied()
        .max_by(|a, b| overlap_area(frame, *a).total_cmp(&overlap_area(frame, *b)))?;
    let width = frame.width.min((display.width - 16.0).max(1.0));
    let height = frame.height.min((display.height - 48.0).max(1.0));
    let x = frame
        .x
        .clamp(display.x + 8.0, display.x + display.width - width - 8.0);
    let y = frame
        .y
        .clamp(display.y + 40.0, display.y + display.height - height - 8.0);
    Rect::new(x, y, width, height).ok()
}

fn recovery_path() -> PathBuf {
    let directory = std::env::temp_dir();
    let path = directory.join(format!("tylora-{}.recovery.json", unsafe { geteuid() }));
    let old = directory.join(format!("fluxwm-{}.recovery.json", unsafe { geteuid() }));
    if !path.exists() && old.is_file() {
        let result = (|| -> std::io::Result<()> {
            let bytes = fs::read(&old)?;
            let temporary = path.with_extension(format!("migration-{}.tmp", std::process::id()));
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            let linked = fs::hard_link(&temporary, &path);
            let _ = fs::remove_file(&temporary);
            linked.or_else(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Ok(())
                } else {
                    Err(error)
                }
            })
        })();
        if let Err(error) = result {
            warn!(%error, "cannot import legacy recovery journal");
        }
    }
    path
}
extern "C" {
    fn geteuid() -> u32;
}

fn application_details(pid: i32) -> (Option<String>, bool, bool) {
    let Some(app) = (unsafe {
        objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
    }) else {
        return (None, false, false);
    };
    let identifier = unsafe { app.bundleIdentifier() }.map(|value| value.to_string());
    let regular =
        unsafe { app.activationPolicy() } == objc2_app_kit::NSApplicationActivationPolicy::Regular;
    let hidden = unsafe { app.isHidden() };
    (identifier, regular, hidden)
}

/// Bundle identifier of a running process, if it has one.
pub fn bundle_identifier(pid: i32) -> Option<String> {
    application_details(pid).0
}

/// `Applied` error: the window is not on any display's active Space, so its
/// geometry was left alone (kAXErrorCannotComplete).
pub const INACTIVE_SPACE: i32 = -25204;

/// Raises `window_id` of `pid` through Accessibility without scanning the
/// app. Right after WindowServer made the window key it is the app's
/// focused window, so one attribute read finds it; otherwise the app's
/// window list is searched. A short timeout keeps a busy app from stalling;
/// an app still busy with the move it just got (kAXErrorCannotComplete) is
/// asked again a few times rather than given up on.
pub fn raise_window(pid: i32, window_id: u32) -> Result<(), i32> {
    let mut result = raise_window_once(pid, window_id);
    for _ in 0..4 {
        if result != Err(INACTIVE_SPACE) {
            break;
        }
        thread::sleep(Duration::from_millis(60));
        result = raise_window_once(pid, window_id);
    }
    result
}

fn raise_window_once(pid: i32, window_id: u32) -> Result<(), i32> {
    let app = OwnedCF(unsafe { AXUIElementCreateApplication(pid) });
    if app.0.is_null() {
        return Err(-1);
    }
    unsafe { AXUIElementSetMessagingTimeout(app.0, 0.15) };
    let raise = |element: AXRef| {
        let action = cf_string(c"AXRaise");
        match unsafe { AXUIElementPerformAction(element, action.0) } {
            0 => Ok(()),
            code => Err(code),
        }
    };
    if let Ok(focused) = ax_attribute(app.0, c"AXFocusedWindow") {
        if crate::spaces::ax_cg_window(focused.0) == Some(window_id) {
            return raise(focused.0);
        }
    }
    let windows = ax_attribute(app.0, c"AXWindows")?;
    let count = unsafe { CFArrayGetCount(windows.0) };
    (0..count)
        .map(|index| unsafe { CFArrayGetValueAtIndex(windows.0, index) })
        .find(|element| crate::spaces::ax_cg_window(*element) == Some(window_id))
        .map_or(Err(-25202), raise)
}

/// A regular application that currently owns at least one window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunningApplication {
    pub pid: i32,
    pub bundle_id: String,
    pub name: Option<String>,
    pub windows: usize,
}

/// Regular applications with windows on any Space, with their real bundle
/// IDs. Process IDs come from WindowServer; metadata from NSRunningApplication.
pub fn running_applications() -> Vec<RunningApplication> {
    let Ok(windows) = crate::discover_all_cg_windows() else {
        return Vec::new();
    };
    let mut counts: HashMap<i32, usize> = HashMap::new();
    for window in &windows {
        *counts.entry(window.owner_pid).or_default() += 1;
    }
    let mut apps: Vec<_> = counts
        .into_iter()
        .filter_map(|(pid, windows)| {
            let app = unsafe {
                objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            }?;
            if unsafe { app.activationPolicy() }
                != objc2_app_kit::NSApplicationActivationPolicy::Regular
            {
                return None;
            }
            Some(RunningApplication {
                pid,
                bundle_id: unsafe { app.bundleIdentifier() }?.to_string(),
                name: unsafe { app.localizedName() }.map(|name| name.to_string()),
                windows,
            })
        })
        .collect();
    apps.sort_by(|a, b| {
        a.name
            .as_deref()
            .unwrap_or(&a.bundle_id)
            .to_lowercase()
            .cmp(&b.name.as_deref().unwrap_or(&b.bundle_id).to_lowercase())
    });
    apps
}

#[derive(Debug, Clone, Copy)]
pub struct WorkArea {
    pub display: DisplayId,
    pub frame: Rect,
    pub full_frame: Rect,
    pub scale: f64,
    pub stable_uuid: Option<u128>,
    pub primary: bool,
    /// The Mac's own panel; it disconnects only in clamshell mode.
    pub builtin: bool,
}

/// Must be called on the process main thread. AppKit uses bottom-left coordinates;
/// the result is converted to the top-left global coordinates used by AX and CG.
pub fn work_areas() -> Vec<WorkArea> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let screens = NSScreen::screens(mtm);
    let Some(primary) = screens.get(0) else {
        return Vec::new();
    };
    let primary_top = primary.frame().origin.y + primary.frame().size.height;
    let mut areas = Vec::new();
    for index in 0..screens.len() {
        let Some(screen) = screens.get(index) else {
            continue;
        };
        let full = screen.frame();
        let center_probe = CGRect {
            origin: CGPoint {
                x: full.origin.x + full.size.width / 2.0,
                y: primary_top - full.origin.y - full.size.height / 2.0,
            },
            size: CGSize {
                width: 1.0,
                height: 1.0,
            },
        };
        let mut count = 0u32;
        if unsafe { CGGetDisplaysWithRect(center_probe, 0, ptr::null_mut(), &mut count) } != 0
            || count != 1
        {
            continue;
        }
        let mut id = 0u32;
        if unsafe { CGGetDisplaysWithRect(center_probe, 1, &mut id, &mut count) } != 0 || count != 1
        {
            continue;
        }
        let visible = screen.visibleFrame();
        if let Ok(frame) = Rect::new(
            visible.origin.x,
            primary_top - visible.origin.y - visible.size.height,
            visible.size.width,
            visible.size.height,
        ) {
            let full_frame = Rect::new(
                full.origin.x,
                primary_top - full.origin.y - full.size.height,
                full.size.width,
                full.size.height,
            )
            .unwrap_or(frame);
            let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
            let stable_uuid = if uuid.is_null() {
                None
            } else {
                let bytes = unsafe { CFUUIDGetUUIDBytes(uuid) };
                unsafe { CFRelease(uuid) };
                Some(u128::from_be_bytes(bytes.bytes))
            };
            areas.push(WorkArea {
                display: DisplayId(id),
                frame,
                full_frame,
                scale: screen.backingScaleFactor(),
                stable_uuid,
                primary: index == 0,
                builtin: unsafe { CGDisplayIsBuiltin(id) } != 0,
            });
        }
    }
    areas
}
#[repr(C)]
struct CFUUIDBytes {
    bytes: [u8; 16],
}
#[link(name = "ColorSync", kind = "framework")]
extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFRef;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayIsBuiltin(display: u32) -> u32;
}

/// Public Core Graphics display reconfiguration notifications. The callback
/// only signals the main thread, which re-reads NSScreen after a short debounce.
pub struct DisplayEvents {
    pub events: Receiver<()>,
    sender: Box<SyncSender<()>>,
    registered: bool,
}
impl DisplayEvents {
    pub fn start() -> Self {
        let (sender, events) = mpsc::sync_channel(1);
        let mut sender = Box::new(sender);
        let context = (&mut *sender as *mut SyncSender<()>).cast();
        let registered =
            unsafe { CGDisplayRegisterReconfigurationCallback(display_callback, context) } == 0;
        if !registered {
            warn!("display callback registration failed; periodic refresh remains active");
        }
        Self {
            events,
            sender,
            registered,
        }
    }
}
impl Drop for DisplayEvents {
    fn drop(&mut self) {
        if self.registered {
            let context = (&mut *self.sender as *mut SyncSender<()>).cast();
            unsafe {
                CGDisplayRemoveReconfigurationCallback(display_callback, context);
            }
        }
    }
}
extern "C" fn display_callback(_display: u32, _flags: u32, user: *mut c_void) {
    if !user.is_null() {
        // SAFETY: registered context outlives the Core Graphics callback.
        let sender = unsafe { &*(user as *const SyncSender<()>) };
        let _ = sender.try_send(());
    }
}

#[derive(Debug, Clone)]
pub enum ManagedCommand {
    Apply(ApplyCommand),
    /// Like `Apply`, but `event` caused it: the worker animates the change
    /// when the animation policy allows, and applies it at once otherwise.
    /// `Applied` is reported when the target is reached (or abandoned).
    Animate {
        command: ApplyCommand,
        event: AnimationEvent,
    },
    /// Animation settings and the system Reduce Motion state.
    Animations {
        config: AnimationConfig,
        reduce_motion: bool,
    },
    /// Ends every running animation at its target now (Space switch,
    /// display change); windows never stay at intermediate frames.
    FinishAnimations,
    Rehome {
        id: ManagedWindowId,
        target: Rect,
    },
    DragFrame {
        id: ManagedWindowId,
        target: Rect,
    },
    QueryFocused {
        request: u64,
        include_cg: bool,
    },
    Focus(ManagedWindowId),
    FocusAt {
        id: ManagedWindowId,
        cg_id: u32,
    },
    Visibility {
        request: u64,
        hide: Vec<ManagedWindowId>,
        show: Vec<ManagedWindowId>,
    },
    RestoreVisibility {
        request: u64,
    },
    /// Finds a window by CGWindowID (rescanning its process), optionally sets
    /// its frame, raises and focuses it, and reports the frame read back.
    Present {
        request: u64,
        pid: i32,
        cg_window_id: u32,
        frame: Option<Rect>,
        focus: bool,
        /// Glide into `frame` (scratchpad show) when animations allow it;
        /// `Presented` then comes when the window arrives.
        animate: bool,
    },
    /// Moves a visible scratchpad window a short way towards `direction`
    /// before it is parked, when animations allow it; answered by `Slid`
    /// (at once, without motion, when they do not).
    Slide {
        request: u64,
        pid: i32,
        cg_window_id: u32,
        direction: SlideDirection,
    },
    /// Windows a Scrolling layout deliberately keeps outside the viewport.
    /// `RestoreVisibility` leaves them parked; `Stop` restores everything.
    SetParked(Vec<ManagedWindowId>),
    Reconcile,
    Stop,
}

#[derive(Debug, Clone)]
pub enum ManagedEvent {
    Reconciled {
        process: ProcessInstance,
        windows: Vec<WindowSnapshot>,
    },
    ProcessExited(ProcessInstance),
    ObservationFailed {
        process: ProcessInstance,
        code: i32,
    },
    Applied {
        command: ApplyCommand,
        result: Result<Rect, i32>,
    },
    Focused {
        request: u64,
        window: Option<ManagedWindowId>,
        cg_window: Option<(u32, i32)>,
        focused_pid: Option<i32>,
    },
    FocusFailed {
        window: ManagedWindowId,
        code: i32,
    },
    FocusApplied(ManagedWindowId),
    DragApplied {
        id: ManagedWindowId,
        result: Result<(), i32>,
    },
    VisibilityDone {
        request: u64,
        result: Result<(), i32>,
    },
    VisibilityGone(ManagedWindowId),
    Presented {
        request: u64,
        result: Result<Rect, i32>,
    },
    Slid {
        request: u64,
        result: Result<(), i32>,
    },
    QueueOverflow,
}

pub struct ManagedAdapter {
    pub commands: SyncSender<ManagedCommand>,
    pub events: Receiver<ManagedEvent>,
}

impl ManagedAdapter {
    pub fn start() -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(256);
        let (event_tx, event_rx) = mpsc::sync_channel(2048);
        thread::Builder::new()
            .name("tylora-ax-worker".into())
            .spawn(move || {
                let mut worker = Worker::new(command_rx, event_tx);
                worker.run();
            })
            .expect("AX worker thread");
        Self {
            commands: command_tx,
            events: event_rx,
        }
    }

    /// Sends a command and wakes the AX worker at once instead of on its
    /// next 50 ms poll: geometry changes and animations start immediately.
    pub fn send_now(&self, command: ManagedCommand) -> Result<(), TrySendError<ManagedCommand>> {
        self.commands.try_send(command)?;
        if let Ok(guard) = AX_RUN_LOOP.lock() {
            if let Some(pointer) = *guard {
                unsafe { CFRunLoopWakeUp(pointer as CFRef) };
            }
        }
        Ok(())
    }

    pub fn request_pointer_focus(
        &self,
        id: ManagedWindowId,
        cg_id: u32,
    ) -> Result<(), TrySendError<ManagedCommand>> {
        self.commands
            .try_send(ManagedCommand::FocusAt { id, cg_id })?;
        if let Ok(guard) = AX_RUN_LOOP.lock() {
            if let Some(pointer) = *guard {
                unsafe { CFRunLoopWakeUp(pointer as CFRef) };
            }
        }
        Ok(())
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFUUIDGetUUIDBytes(uuid: CFRef) -> CFUUIDBytes;
    fn CFRetain(value: CFRef) -> CFRef;
    fn CFEqual(a: CFRef, b: CFRef) -> u8;
    fn CFBooleanGetValue(value: CFRef) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    static kCFBooleanTrue: CFRef;
    static kCFBooleanFalse: CFRef;
    fn CFRunLoopGetCurrent() -> CFRef;
    fn CFRunLoopWakeUp(run_loop: CFRef);
    fn CFRunLoopAddSource(run_loop: CFRef, source: CFRef, mode: CFRef);
    fn CFRunLoopRunInMode(mode: CFRef, seconds: f64, return_after_source_handled: u8) -> i32;
    static kCFRunLoopDefaultMode: CFRef;
}
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(
        pid: i32,
        callback: extern "C" fn(CFRef, AXRef, CFRef, *mut c_void),
        observer: *mut CFRef,
    ) -> i32;
    fn AXObserverAddNotification(
        observer: CFRef,
        element: AXRef,
        notification: CFRef,
        refcon: *mut c_void,
    ) -> i32;
    fn AXObserverGetRunLoopSource(observer: CFRef) -> CFRef;
    fn AXUIElementIsAttributeSettable(element: AXRef, attribute: CFRef, settable: *mut u8) -> i32;
    fn AXUIElementSetAttributeValue(element: AXRef, attribute: CFRef, value: CFRef) -> i32;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFRef;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayBounds(display: u32) -> CGRect;
    fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayRegisterReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        user: *mut c_void,
    ) -> i32;
    fn CGDisplayRemoveReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        user: *mut c_void,
    ) -> i32;
}
#[link(name = "proc")]
extern "C" {
    fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, size: i32) -> i32;
}
extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
    fn kqueue() -> i32;
    fn kevent(
        queue: i32,
        changes: *const KEvent,
        change_count: i32,
        events: *mut KEvent,
        event_count: i32,
        timeout: *const Timespec,
    ) -> i32;
    fn close(fd: i32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KEvent {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: isize,
    udata: *mut c_void,
}
impl Default for KEvent {
    fn default() -> Self {
        Self {
            ident: 0,
            filter: 0,
            flags: 0,
            fflags: 0,
            data: 0,
            udata: ptr::null_mut(),
        }
    }
}
#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[repr(C)]
#[derive(Default)]
struct ProcBsdInfo {
    fields: [u32; 12],
    comm: [u8; 16],
    name: [u8; 32],
    more: [u32; 6],
    start_sec: u64,
    start_usec: u64,
}

fn process_instance(pid: i32) -> Option<ProcessInstance> {
    let mut info = ProcBsdInfo::default();
    let size = size_of::<ProcBsdInfo>() as i32;
    // PROC_PIDTBSDINFO is a public libproc flavor. Require a complete struct.
    if unsafe { proc_pidinfo(pid, 3, 0, (&mut info as *mut ProcBsdInfo).cast(), size) } != size {
        return None;
    }
    Some(ProcessInstance {
        pid,
        epoch: info
            .start_sec
            .saturating_mul(1_000_000)
            .saturating_add(info.start_usec),
    })
}

fn cf_text(value: CFRef) -> Option<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return None;
    }
    let mut bytes = [0u8; 512];
    if unsafe {
        CFStringGetCString(
            value,
            bytes.as_mut_ptr().cast(),
            bytes.len() as isize,
            0x0800_0100,
        )
    } == 0
    {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(bytes.as_ptr().cast()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn ax_bool(element: AXRef, attribute: &'static CStr) -> Option<bool> {
    let value = ax_attribute(element, attribute).ok()?;
    if unsafe { CFGetTypeID(value.0) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(value.0) } != 0)
}

fn settable(element: AXRef, attribute: &'static CStr) -> bool {
    let key = cf_string(attribute);
    let mut flag = 0u8;
    !key.0.is_null()
        && unsafe { AXUIElementIsAttributeSettable(element, key.0, &mut flag) } == 0
        && flag != 0
}

fn add_notification(observer: CFRef, element: AXRef, name: &'static CStr, context: *mut c_void) {
    let key = cf_string(name);
    if key.0.is_null() {
        return;
    }
    let code = unsafe { AXObserverAddNotification(observer, element, key.0, context) };
    if code != 0 && code != -25209 && code != -25207 {
        debug!(?name, code, "AX notification unavailable");
    }
}

struct CallbackContext {
    pid: i32,
    sender: SyncSender<i32>,
    overflow: Arc<AtomicBool>,
}
extern "C" fn callback(
    _observer: CFRef,
    _element: AXRef,
    _notification: CFRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: refcon points to a Box retained longer than its AXObserver.
    let context = unsafe { &*(refcon as *const CallbackContext) };
    if let Err(TrySendError::Full(_)) = context.sender.try_send(context.pid) {
        context.overflow.store(true, Ordering::Release);
    }
}

struct AxHandle {
    id: ManagedWindowId,
    element: OwnedCF,
}
struct AppWatch {
    process: ProcessInstance,
    app: OwnedCF,
    observer: OwnedCF,
    // Field order matters: observer drops before context in `Drop` below.
    context: Box<CallbackContext>,
    handles: Vec<AxHandle>,
    registered: HashSet<u64>,
}
impl Drop for AppWatch {
    fn drop(&mut self) {
        // Observer released while callback context remains alive.
        self.handles.clear();
        self.observer = OwnedCF(ptr::null());
    }
}

struct Worker {
    commands: Receiver<ManagedCommand>,
    events: SyncSender<ManagedEvent>,
    raw_rx: Receiver<i32>,
    raw_tx: SyncSender<i32>,
    overflow: Arc<AtomicBool>,
    apps: HashMap<i32, AppWatch>,
    cg_known: HashMap<i32, HashSet<u32>>,
    pending_absences: HashMap<i32, HashSet<ManagedWindowId>>,
    verify_due: HashMap<i32, Instant>,
    pending_applies: Vec<(ApplyCommand, Instant)>,
    process_queue: i32,
    next_token: u64,
    minimized_by_tylora: HashSet<ManagedWindowId>,
    recovery: Vec<RecoveryRecord>,
    live_recovery: HashMap<ManagedWindowId, RecoveryRecord>,
    /// Off-screen windows from a previous daemon session, not yet matched.
    offscreen_orphans: Vec<OffscreenRecord>,
    live_offscreen: HashMap<ManagedWindowId, OffscreenRecord>,
    parked: HashSet<ManagedWindowId>,
    last_snapshots: HashMap<ManagedWindowId, WindowSnapshot>,
    next_discovery: Instant,
    next_reconcile: Instant,
    last_wall_tick: SystemTime,
    last_gap_scan: SystemTime,
    /// The only animation scheduler: every geometry write stays on this
    /// thread, so frames of one window never race.
    animations: AnimationScheduler<Completion>,
    animation_policy: AnimationPolicy,
    /// Apps whose frame writes were too slow to animate, until when.
    slow_pids: HashMap<i32, Instant>,
    /// AX notifications caused by our own frame writes; scanned once the
    /// app's animations end instead of on every frame.
    deferred_scans: HashSet<i32>,
    instant_reasons: HashMap<String, u64>,
    /// Consecutive slow frame writes per app.
    slow_strikes: HashMap<i32, u8>,
}

/// What a finished animation fulfils.
#[derive(Debug, Clone, Copy)]
enum Completion {
    /// A layout target: reported as `Applied` like an instant apply.
    Apply(ApplyCommand),
    /// A scratchpad show: reported as `Presented` with the final frame.
    Present { request: u64 },
    /// A scratchpad leaving before it is parked: reported as `Slid`.
    Slide { request: u64 },
}

impl Worker {
    fn new(commands: Receiver<ManagedCommand>, events: SyncSender<ManagedEvent>) -> Self {
        let (raw_tx, raw_rx) = mpsc::sync_channel(1024);
        Self {
            commands,
            events,
            raw_rx,
            raw_tx,
            overflow: Arc::new(AtomicBool::new(false)),
            apps: HashMap::new(),
            cg_known: HashMap::new(),
            pending_absences: HashMap::new(),
            verify_due: HashMap::new(),
            pending_applies: Vec::new(),
            process_queue: unsafe { kqueue() },
            next_token: 0,
            minimized_by_tylora: HashSet::new(),
            recovery: fs::read(recovery_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default(),
            live_recovery: HashMap::new(),
            offscreen_orphans: fs::read(offscreen_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default(),
            live_offscreen: HashMap::new(),
            parked: HashSet::new(),
            last_snapshots: HashMap::new(),
            next_discovery: Instant::now(),
            next_reconcile: Instant::now(),
            last_wall_tick: SystemTime::now(),
            last_gap_scan: SystemTime::UNIX_EPOCH,
            animations: AnimationScheduler::new(AnimationConfig::default().frame_interval()),
            animation_policy: AnimationPolicy::default(),
            slow_pids: HashMap::new(),
            deferred_scans: HashSet::new(),
            instant_reasons: HashMap::new(),
            slow_strikes: HashMap::new(),
        }
    }

    fn run(&mut self) {
        let loop_ref = unsafe { CFRetain(CFRunLoopGetCurrent()) };
        if let Ok(mut slot) = AX_RUN_LOOP.lock() {
            *slot = Some(loop_ref as usize);
        }
        self.recover_after_crash();
        let _ = self.restore_offscreen_orphans();
        loop {
            let wall_now = SystemTime::now();
            let gap = wall_now
                .duration_since(self.last_wall_tick)
                .unwrap_or_default();
            self.last_wall_tick = wall_now;
            if gap > Duration::from_secs(10)
                && wall_now
                    .duration_since(self.last_gap_scan)
                    .unwrap_or_default()
                    > Duration::from_secs(30)
            {
                info!(
                    gap_seconds = gap.as_secs(),
                    "long scheduler gap or wake detected; reconciling"
                );
                self.last_gap_scan = wall_now;
                self.scan_all();
            }
            let mut stop = false;
            for _ in 0..64 {
                match self.commands.try_recv() {
                    Ok(ManagedCommand::Apply(command)) => {
                        if !self.apply_now(command) {
                            return;
                        }
                    }
                    Ok(ManagedCommand::Animate { command, event }) => {
                        if !self.animate(command, event) {
                            return;
                        }
                    }
                    Ok(ManagedCommand::Animations {
                        config,
                        reduce_motion,
                    }) => self.configure_animations(config, reduce_motion),
                    Ok(ManagedCommand::FinishAnimations) => {
                        if !self.finish_all_animations() {
                            return;
                        }
                    }
                    Ok(ManagedCommand::Rehome { id, target }) => {
                        let command = ApplyCommand {
                            id,
                            generation: 0,
                            version: 0,
                            target,
                        };
                        let started = Instant::now();
                        match self.apply(command) {
                            Ok(_) => {
                                if self.live_offscreen.remove(&id).is_some() {
                                    self.persist_offscreen();
                                }
                                self.verify_due.insert(
                                    id.process.pid,
                                    Instant::now() + Duration::from_millis(250),
                                );
                                info!(
                                    pid = id.process.pid,
                                    token = id.token,
                                    elapsed_us = started.elapsed().as_micros(),
                                    "window rehome scheduled after display disconnect"
                                );
                            }
                            Err(code) => warn!(
                                pid = id.process.pid,
                                token = id.token,
                                code,
                                "cannot rehome window through AX; macOS may relocate it"
                            ),
                        }
                    }
                    Ok(ManagedCommand::DragFrame { id, target }) => {
                        // The user's hand wins over any running transition.
                        self.cancel_animation(id);
                        let command = ApplyCommand {
                            id,
                            generation: 0,
                            version: 0,
                            target,
                        };
                        let started = Instant::now();
                        let result = self.apply(command).map(|_| ());
                        debug!(
                            pid = id.process.pid,
                            token = id.token,
                            elapsed_us = started.elapsed().as_micros(),
                            "AX drag frame completed"
                        );
                        let _ = self.events.send(ManagedEvent::DragApplied { id, result });
                    }
                    Ok(ManagedCommand::QueryFocused {
                        request,
                        include_cg,
                    }) => {
                        let window = self.focused_window();
                        let cg_window = include_cg
                            .then(crate::spaces::focused_cg_window)
                            .and_then(Result::ok)
                            .map(|focused| (focused.window_id, focused.pid));
                        let focused_pid =
                            include_cg.then(|| self.focused_application_pid()).flatten();
                        if self
                            .events
                            .send(ManagedEvent::Focused {
                                request,
                                window,
                                cg_window,
                                focused_pid,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Ok(ManagedCommand::Focus(window)) => match self.timed_focus(window) {
                        Ok(()) => {
                            let _ = self.events.send(ManagedEvent::FocusApplied(window));
                        }
                        Err(code) => {
                            let _ = self.events.send(ManagedEvent::FocusFailed { window, code });
                        }
                    },
                    Ok(ManagedCommand::FocusAt { id, cg_id }) => {
                        // Check at execution time: a queued request must not focus a
                        // window which the pointer has already left or which was hidden.
                        // Windows moving under a still pointer must not steal
                        // focus: pointer requests wait until motion ends.
                        let result = if self.animations.is_idle()
                            && !POINTER_FOCUS_SUPPRESSED.load(Ordering::Acquire)
                            && crate::current_pointer().is_some_and(|current| {
                                crate::topmost_window_at(current) == Some((cg_id, id.process.pid))
                            }) {
                            if self.focused_window() == Some(id) {
                                Ok(())
                            } else {
                                self.focus(id)
                            }
                        } else {
                            Err(-25202)
                        };
                        match result {
                            Ok(()) => {
                                let _ = self.events.send(ManagedEvent::FocusApplied(id));
                            }
                            Err(code) => {
                                let _ = self
                                    .events
                                    .send(ManagedEvent::FocusFailed { window: id, code });
                            }
                        }
                    }
                    Ok(ManagedCommand::Visibility {
                        request,
                        hide,
                        show,
                    }) => {
                        let started = Instant::now();
                        let result = self.visibility_transition(&hide, &show);
                        debug!(
                            elapsed_us = started.elapsed().as_micros(),
                            hidden = hide.len(),
                            shown = show.len(),
                            "workspace visibility transaction completed"
                        );
                        let _ = self
                            .events
                            .send(ManagedEvent::VisibilityDone { request, result });
                    }
                    Ok(ManagedCommand::Present {
                        request,
                        pid,
                        cg_window_id,
                        frame,
                        focus,
                        animate,
                    }) => {
                        let animated = animate
                            && frame.is_some_and(|frame| {
                                self.present_animated(request, pid, cg_window_id, frame, focus)
                            });
                        if !animated {
                            let result = self.present(pid, cg_window_id, frame, focus);
                            let _ = self
                                .events
                                .send(ManagedEvent::Presented { request, result });
                        }
                    }
                    Ok(ManagedCommand::Slide {
                        request,
                        pid,
                        cg_window_id,
                        direction,
                    }) => {
                        if !self.slide(request, pid, cg_window_id, direction) {
                            return;
                        }
                    }
                    Ok(ManagedCommand::SetParked(ids)) => {
                        self.parked = ids.into_iter().collect();
                    }
                    Ok(ManagedCommand::RestoreVisibility { request }) => {
                        let keep = std::mem::take(&mut self.parked);
                        let offscreen = self.restore_offscreen(&keep);
                        let result = self.restore_all_visibility().and(offscreen);
                        let _ = self
                            .events
                            .send(ManagedEvent::VisibilityDone { request, result });
                    }
                    Ok(ManagedCommand::Reconcile) => self.scan_all(),
                    Ok(ManagedCommand::Stop) | Err(TryRecvError::Disconnected) => {
                        stop = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            if stop {
                // No window may stay at an intermediate frame after stop.
                let _ = self.finish_all_animations();
                if let Err(code) = self.restore_offscreen(&HashSet::new()) {
                    warn!(
                        code,
                        remaining = self.live_offscreen.len() + self.offscreen_orphans.len(),
                        "could not bring every off-screen window back on stop"
                    );
                }
                if let Err(code) = self.restore_all_visibility() {
                    warn!(
                        code,
                        remaining = self.minimized_by_tylora.len(),
                        "could not restore all Tylora-minimized windows on stop"
                    );
                }
                break;
            }
            let mut index = 0;
            while index < self.pending_applies.len() {
                if self.pending_applies[index].1 > Instant::now() {
                    index += 1;
                    continue;
                }
                let (command, _) = self.pending_applies.swap_remove(index);
                let result = self.read_geometry(command);
                if let Ok(actual) = result {
                    self.track_offscreen(command.id, actual);
                }
                let failed = result.is_err();
                if self
                    .events
                    .send(ManagedEvent::Applied { command, result })
                    .is_err()
                {
                    return;
                }
                if failed {
                    self.scan_pid(command.id.process.pid);
                }
            }
            if !self.tick_animations() {
                return;
            }
            let mut dirty = std::mem::take(&mut self.deferred_scans);
            while let Ok(pid) = self.raw_rx.try_recv() {
                dirty.insert(pid);
            }
            // Our own frame writes notify AXMoved/AXResized: scan those apps
            // once their animations end, not on every frame.
            let animating: HashSet<i32> = self.animations.ids().map(|id| id.process.pid).collect();
            for pid in dirty {
                if animating.contains(&pid) {
                    self.deferred_scans.insert(pid);
                    continue;
                }
                debug!(pid, "AX notification received; scanning process");
                self.scan_pid(pid);
            }
            self.poll_process_exits();
            let due: Vec<_> = self
                .verify_due
                .iter()
                .filter(|(_, when)| **when <= Instant::now())
                .map(|(pid, _)| *pid)
                .collect();
            for pid in due {
                self.verify_due.remove(&pid);
                self.scan_pid(pid);
            }
            if self.overflow.swap(false, Ordering::AcqRel) {
                warn!("AX callback queue overflow; scheduling authoritative reconciliation");
                let _ = self.events.send(ManagedEvent::QueueOverflow);
                self.scan_all();
            }
            if Instant::now() >= self.next_discovery {
                self.discover_new_processes();
                self.next_discovery = Instant::now() + Duration::from_secs(2);
            }
            if Instant::now() >= self.next_reconcile {
                self.scan_all();
                self.next_reconcile = Instant::now() + Duration::from_secs(30);
            }
            // Pumps AXObserver sources. Callback only enqueues PID. While
            // animating, wake for the next frame; idle, poll as before.
            let wait = self.animations.next_deadline().map_or(0.05, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    .min(0.05)
            });
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, wait, 1);
            }
        }
    }

    fn discover_new_processes(&mut self) {
        match cg_windows() {
            Ok(windows) => {
                let mut current: HashMap<i32, HashSet<u32>> = HashMap::new();
                for window in windows {
                    current
                        .entry(window.owner_pid)
                        .or_default()
                        .insert(window.id.0);
                }
                let changed: HashSet<_> = current
                    .iter()
                    .filter(|(pid, ids)| {
                        self.cg_known.get(pid) != Some(ids) || !self.apps.contains_key(pid)
                    })
                    .map(|(pid, _)| *pid)
                    .chain(
                        self.cg_known
                            .keys()
                            .filter(|pid| !current.contains_key(pid))
                            .copied(),
                    )
                    .collect();
                self.cg_known = current;
                for pid in changed {
                    debug!(pid, "CG window set changed; requesting AX scan");
                    self.scan_pid(pid);
                }
            }
            Err(error) => warn!(%error, "CG process discovery failed"),
        }
    }

    fn scan_all(&mut self) {
        info!("reconciliation started");
        self.discover_new_processes();
        for pid in self.apps.keys().copied().collect::<Vec<_>>() {
            self.scan_pid(pid);
        }
    }

    fn ensure_watch(&mut self, process: ProcessInstance) -> Result<(), i32> {
        if let Some(old) = self.apps.get(&process.pid) {
            if old.process == process {
                return Ok(());
            }
            let old_process = old.process;
            self.apps.remove(&process.pid);
            let _ = self.events.send(ManagedEvent::ProcessExited(old_process));
        }
        let app = OwnedCF(unsafe { AXUIElementCreateApplication(process.pid) });
        if app.0.is_null() {
            return Err(-1);
        }
        let code = unsafe { AXUIElementSetMessagingTimeout(app.0, 0.25) };
        if code != 0 {
            return Err(code);
        }
        let context = Box::new(CallbackContext {
            pid: process.pid,
            sender: self.raw_tx.clone(),
            overflow: self.overflow.clone(),
        });
        let mut observer_ref = ptr::null();
        let code = unsafe { AXObserverCreate(process.pid, callback, &mut observer_ref) };
        if code != 0 {
            return Err(code);
        }
        let observer = OwnedCF(observer_ref);
        let refcon = (&*context as *const CallbackContext).cast_mut().cast();
        for name in [
            c"AXWindowCreated",
            c"AXFocusedWindowChanged",
            // Cmd-Tab and Dock activation: lets layouts follow app switches.
            c"AXApplicationActivated",
        ] {
            add_notification(observer.0, app.0, name, refcon);
        }
        let source = unsafe { AXObserverGetRunLoopSource(observer.0) };
        if source.is_null() {
            return Err(-1);
        }
        unsafe {
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
        }
        self.apps.insert(
            process.pid,
            AppWatch {
                process,
                app,
                observer,
                context,
                handles: Vec::new(),
                registered: HashSet::new(),
            },
        );
        if self.process_queue >= 0 {
            let change = KEvent {
                ident: process.pid as usize,
                filter: -5,
                flags: 0x0001 | 0x0004 | 0x0020,
                fflags: 0x8000_0000,
                data: 0,
                udata: ptr::null_mut(),
            };
            if unsafe {
                kevent(
                    self.process_queue,
                    &change,
                    1,
                    ptr::null_mut(),
                    0,
                    ptr::null(),
                )
            } < 0
            {
                debug!(pid=process.pid, error=%std::io::Error::last_os_error(), "process exit watcher unavailable");
            }
        }
        Ok(())
    }

    fn poll_process_exits(&mut self) {
        if self.process_queue < 0 {
            return;
        }
        let mut events = [KEvent::default(); 16];
        let timeout = Timespec { sec: 0, nsec: 0 };
        let count = unsafe {
            kevent(
                self.process_queue,
                ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as i32,
                &timeout,
            )
        };
        if count <= 0 {
            return;
        }
        for event in events.iter().take(count as usize) {
            if event.filter != -5 || event.fflags & 0x8000_0000 == 0 {
                continue;
            }
            let pid = event.ident as i32;
            if let Some(old) = self.apps.remove(&pid) {
                self.pending_absences.remove(&pid);
                self.verify_due.remove(&pid);
                let _ = self.events.send(ManagedEvent::ProcessExited(old.process));
                self.minimized_by_tylora
                    .retain(|id| id.process != old.process);
                self.last_snapshots
                    .retain(|id, _| id.process != old.process);
                self.recovery.retain(|record| record.process != old.process);
                self.live_recovery.retain(|id, _| id.process != old.process);
                let _ = self.persist_recovery();
            }
        }
    }

    fn scan_pid(&mut self, pid: i32) {
        let Some(process) = process_instance(pid) else {
            // proc_pidinfo can fail transiently. Only ESRCH proves process exit.
            let gone = unsafe { kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(3);
            if gone {
                if let Some(old) = self.apps.remove(&pid) {
                    let _ = self.events.send(ManagedEvent::ProcessExited(old.process));
                }
            }
            return;
        };
        if let Err(code) = self.ensure_watch(process) {
            let _ = self
                .events
                .send(ManagedEvent::ObservationFailed { process, code });
            return;
        }
        let watch = self.apps.get_mut(&pid).expect("watch installed");
        let list = match ax_attribute(watch.app.0, c"AXWindows") {
            Ok(value) => value,
            Err(code) => {
                let _ = self
                    .events
                    .send(ManagedEvent::ObservationFailed { process, code });
                return;
            }
        };
        if unsafe { CFGetTypeID(list.0) != CFArrayGetTypeID() } {
            let _ = self
                .events
                .send(ManagedEvent::ObservationFailed { process, code: -1 });
            return;
        }
        let (app_id, regular_app, app_hidden) = application_details(pid);
        let hidden = ax_bool(watch.app.0, c"AXHidden") == Some(true) || app_hidden;
        let focused = ax_attribute(watch.app.0, c"AXFocusedWindow").ok();
        let mut windows = Vec::new();
        let mut handles = Vec::new();
        let mut incomplete = false;
        let count = unsafe { CFArrayGetCount(list.0) };
        for index in 0..count {
            let element = unsafe { CFArrayGetValueAtIndex(list.0, index) };
            if element.is_null() {
                continue;
            }
            let id = watch
                .handles
                .iter()
                .find(|h| unsafe { CFEqual(h.element.0, element) } != 0)
                .map(|h| h.id)
                .unwrap_or_else(|| {
                    self.next_token = self.next_token.wrapping_add(1).max(1);
                    ManagedWindowId {
                        process,
                        token: self.next_token,
                    }
                });
            let retained = OwnedCF(unsafe { CFRetain(element) });
            if retained.0.is_null() {
                incomplete = true;
                continue;
            }
            let timeout_code = unsafe { AXUIElementSetMessagingTimeout(element, 0.25) };
            handles.push(AxHandle {
                id,
                element: retained,
            });
            if timeout_code != 0 {
                incomplete = true;
                let _ = self.events.send(ManagedEvent::ObservationFailed {
                    process,
                    code: timeout_code,
                });
                continue;
            }
            if watch.registered.insert(id.token) {
                let refcon = (&*watch.context as *const CallbackContext)
                    .cast_mut()
                    .cast();
                for name in [
                    c"AXUIElementDestroyed",
                    c"AXMoved",
                    c"AXResized",
                    c"AXWindowMiniaturized",
                    c"AXWindowDeminiaturized",
                    // Rules may match titles that are set after creation.
                    c"AXTitleChanged",
                ] {
                    add_notification(watch.observer.0, element, name, refcon);
                }
            }
            let frame = match ax_rect(element) {
                Ok(frame) => frame,
                Err((_, code)) => {
                    let _ = self
                        .events
                        .send(ManagedEvent::ObservationFailed { process, code });
                    continue;
                }
            };
            let role = ax_attribute(element, c"AXRole")
                .ok()
                .and_then(|v| cf_text(v.0));
            let subrole = ax_attribute(element, c"AXSubrole")
                .ok()
                .and_then(|v| cf_text(v.0));
            let normal = role.as_deref() == Some("AXWindow")
                && subrole.as_deref() == Some("AXStandardWindow");
            let writable = settable(element, c"AXPosition") && settable(element, c"AXSize");
            let lifecycle = if hidden {
                WindowLifecycle::Hidden
            } else if ax_bool(element, c"AXMinimized") == Some(true) {
                WindowLifecycle::Minimized
            } else if focused
                .as_ref()
                .is_some_and(|value| unsafe { CFEqual(value.0, element) } != 0)
            {
                WindowLifecycle::Active
            } else {
                WindowLifecycle::Discovered
            };
            let display_id = display_for(frame);
            let covers_display = display_id.is_some_and(|display| {
                let bounds = unsafe { CGDisplayBounds(display.0) };
                Rect::new(
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.size.width,
                    bounds.size.height,
                )
                .is_ok_and(|full| frame.near(full, 1.0))
            });
            let cg_window_id = crate::spaces::ax_cg_window(element);
            let native_space_id = cg_window_id.and_then(|wid| {
                use crate::spaces::{ExperimentalSkyLightBackend, NativeSpaceBackend};
                // Native fullscreen windows can briefly belong to no Space while
                // macOS animates them into their own Space.
                match ExperimentalSkyLightBackend::new()
                    .get_window_spaces(wid)
                    .ok()?
                    .as_slice()
                {
                    [only] => Some(only.0),
                    _ => None,
                }
            });
            let title = ax_attribute(element, c"AXTitle")
                .ok()
                .and_then(|value| cf_text(value.0));
            windows.push(WindowSnapshot {
                id,
                cg_window_id,
                native_space_id,
                application_id: app_id.clone(),
                title,
                frame,
                display_id,
                lifecycle,
                mode: if normal && regular_app && writable && !covers_display {
                    ManagementMode::Tiled
                } else {
                    ManagementMode::Unmanaged
                },
            });
        }
        // Some applications omit minimized windows from AXWindows. Retain an
        // AX handle and its last snapshot only for windows Tylora minimized.
        let listed: HashSet<_> = handles.iter().map(|handle| handle.id).collect();
        for old in &watch.handles {
            if listed.contains(&old.id) || !self.minimized_by_tylora.contains(&old.id) {
                continue;
            }
            if ax_bool(old.element.0, c"AXMinimized") != Some(true) {
                continue;
            }
            if let Some(previous) = self.last_snapshots.get(&old.id) {
                let retained = OwnedCF(unsafe { CFRetain(old.element.0) });
                if !retained.0.is_null() {
                    handles.push(AxHandle {
                        id: old.id,
                        element: retained,
                    });
                    let mut snapshot = previous.clone();
                    snapshot.lifecycle = WindowLifecycle::Minimized;
                    windows.push(snapshot);
                }
            }
        }
        // If any AX window read failed, this is a partial scan: never remove absent windows.
        // A full scan will be retried on the next notification/reconciliation.
        if incomplete || windows.len() != handles.len() {
            return;
        }
        let current_ids: HashSet<_> = handles.iter().map(|h| h.id).collect();
        let absent: HashSet<_> = watch
            .handles
            .iter()
            .map(|h| h.id)
            .filter(|id| !current_ids.contains(id))
            .collect();
        if !absent.is_empty() && self.pending_absences.get(&pid) != Some(&absent) {
            debug!(
                pid,
                count = absent.len(),
                "possible window removal; awaiting second complete scan"
            );
            self.pending_absences.insert(pid, absent);
            self.verify_due
                .insert(pid, Instant::now() + Duration::from_millis(150));
            return;
        }
        self.pending_absences.remove(&pid);
        self.verify_due.remove(&pid);
        watch.handles = handles;
        for id in &absent {
            if self.minimized_by_tylora.remove(id) {
                if let Some(record) = self.live_recovery.remove(id) {
                    self.recovery.retain(|saved| saved != &record);
                }
                let _ = self.events.send(ManagedEvent::VisibilityGone(*id));
            }
        }
        for snapshot in &windows {
            self.last_snapshots.insert(snapshot.id, snapshot.clone());
        }
        self.last_snapshots.retain(|id, _| {
            id.process != process
                || current_ids.contains(id)
                || self.minimized_by_tylora.contains(id)
        });
        watch
            .registered
            .retain(|token| watch.handles.iter().any(|h| h.id.token == *token));
        if !absent.is_empty() {
            let _ = self.persist_recovery();
        }
        let _ = self
            .events
            .send(ManagedEvent::Reconciled { process, windows });
    }

    fn read_geometry(&self, command: ApplyCommand) -> Result<Rect, i32> {
        let watch = self.apps.get(&command.id.process.pid).ok_or(-1)?;
        if watch.process != command.id.process {
            return Err(-1);
        }
        let handle = watch
            .handles
            .iter()
            .find(|h| h.id == command.id)
            .ok_or(-1)?;
        ax_rect(handle.element.0).map_err(|(_, code)| code)
    }

    fn focused_window(&self) -> Option<ManagedWindowId> {
        let pid = super::frontmost_application_pid()?;
        // Ask the application itself: its element keeps working when the
        // system-wide focus element is stuck.
        let app = OwnedCF(unsafe { AXUIElementCreateApplication(pid) });
        if app.0.is_null() {
            return None;
        }
        let focused = ax_attribute(app.0, c"AXFocusedWindow")
            .or_else(|_| ax_attribute(app.0, c"AXMainWindow"))
            .ok()?;
        self.apps
            .get(&pid)?
            .handles
            .iter()
            .find(|handle| unsafe { CFEqual(handle.element.0, focused.0) } != 0)
            .map(|handle| handle.id)
    }

    fn focused_application_pid(&self) -> Option<i32> {
        super::frontmost_application_pid()
    }

    fn present(
        &mut self,
        pid: i32,
        cg_window_id: u32,
        frame: Option<Rect>,
        focus: bool,
    ) -> Result<Rect, i32> {
        // A window that just arrived from another Space has no handle yet.
        self.scan_pid(pid);
        let id = self
            .apps
            .get(&pid)
            .and_then(|watch| {
                watch.handles.iter().find(|handle| {
                    crate::spaces::ax_cg_window(handle.element.0) == Some(cg_window_id)
                })
            })
            .map(|handle| handle.id)
            .ok_or(-25202)?;
        if let Some(target) = frame {
            self.apply(ApplyCommand {
                id,
                generation: 0,
                version: 0,
                target,
            })?;
        }
        if focus {
            self.focus(id)?;
        }
        // Applications may settle the frame asynchronously; read it back.
        thread::sleep(Duration::from_millis(120));
        self.read_geometry(ApplyCommand {
            id,
            generation: 0,
            version: 0,
            target: frame.unwrap_or_else(|| Rect::new(0.0, 0.0, 1.0, 1.0).expect("unit rect")),
        })
    }

    fn timed_focus(&self, id: ManagedWindowId) -> Result<(), i32> {
        let started = Instant::now();
        let result = self.focus(id);
        debug!(
            pid = id.process.pid,
            elapsed_us = started.elapsed().as_micros(),
            "AX focus completed"
        );
        result
    }

    fn focus(&self, id: ManagedWindowId) -> Result<(), i32> {
        let watch = self.apps.get(&id.process.pid).ok_or(-1)?;
        if watch.process != id.process {
            return Err(-1);
        }
        let handle = watch
            .handles
            .iter()
            .find(|handle| handle.id == id)
            .ok_or(-1)?;
        let action = cf_string(c"AXRaise");
        if action.0.is_null() {
            return Err(-1);
        }
        let code = unsafe { AXUIElementPerformAction(handle.element.0, action.0) };
        if code != 0 {
            return Err(code);
        }
        let key = cf_string(c"AXFocusedWindow");
        if key.0.is_null() {
            return Err(-1);
        }
        let code = unsafe { AXUIElementSetAttributeValue(watch.app.0, key.0, handle.element.0) };
        if code != 0 {
            return Err(code);
        }
        // Activating through NSRunningApplication from this thread blocks
        // for about a second waiting on the Dock, stalling every geometry
        // write and animation queued behind it. WindowServer makes the same
        // window key and its app frontmost in milliseconds.
        if crate::spaces::ax_cg_window(handle.element.0)
            .is_some_and(|window| crate::spaces::focus_window(id.process.pid, window).is_ok())
        {
            return Ok(());
        }
        if let Some(app) = unsafe {
            objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(
                id.process.pid,
            )
        } {
            let _ = unsafe {
                app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty())
            };
        }
        Ok(())
    }

    fn visibility_element(&self, id: ManagedWindowId) -> Result<AXRef, i32> {
        let watch = self.apps.get(&id.process.pid).ok_or(-25202)?;
        if watch.process != id.process {
            return Err(-25202);
        }
        watch
            .handles
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.element.0)
            .ok_or(-25202)
    }

    fn recovery_record(&self, id: ManagedWindowId) -> Result<RecoveryRecord, i32> {
        let element = self.visibility_element(id)?;
        Ok(RecoveryRecord {
            process: id.process,
            frame: ax_rect(element).map_err(|(_, code)| code)?,
            title: ax_attribute(element, c"AXTitle")
                .ok()
                .and_then(|value| cf_text(value.0)),
        })
    }

    fn persist_recovery(&self) -> Result<(), i32> {
        let path = recovery_path();
        if self.recovery.is_empty() {
            if let Err(error) = fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Err(-1);
                }
            }
            return Ok(());
        }
        let bytes = serde_json::to_vec(&self.recovery).map_err(|_| -1)?;
        let temp = path.with_extension(format!("tmp-{}", std::process::id()));
        let _ = fs::remove_file(&temp);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|_| -1)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| -1)?;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600)).map_err(|_| -1)?;
        fs::rename(&temp, &path).map_err(|_| -1)?;
        Ok(())
    }

    fn recover_after_crash(&mut self) {
        if self.recovery.is_empty() {
            return;
        }
        warn!(
            records = self.recovery.len(),
            "recovering windows from previous daemon session"
        );
        let records = self.recovery.clone();
        let mut unresolved = Vec::new();
        for record in records {
            if process_instance(record.process.pid) != Some(record.process) {
                continue;
            }
            self.scan_pid(record.process.pid);
            let matches: Vec<_> = self
                .apps
                .get(&record.process.pid)
                .into_iter()
                .flat_map(|watch| watch.handles.iter())
                .filter(|handle| {
                    ax_rect(handle.element.0).is_ok_and(|frame| frame.near(record.frame, 1.0))
                })
                .filter(|handle| {
                    ax_attribute(handle.element.0, c"AXTitle")
                        .ok()
                        .and_then(|value| cf_text(value.0))
                        == record.title
                })
                .map(|handle| handle.id)
                .collect();
            if matches.len() != 1 {
                warn!(
                    pid = record.process.pid,
                    matches = matches.len(),
                    "recovery window could not be identified uniquely"
                );
                unresolved.push(record);
                continue;
            }
            let id = matches[0];
            if self
                .visibility_element(id)
                .ok()
                .and_then(|element| ax_bool(element, c"AXMinimized"))
                == Some(true)
            {
                self.minimized_by_tylora.insert(id);
                if let Err(code) = self.set_minimized(id, false) {
                    warn!(
                        pid = id.process.pid,
                        token = id.token,
                        code,
                        "crash recovery failed"
                    );
                    self.live_recovery.insert(id, record.clone());
                    unresolved.push(record);
                } else {
                    self.minimized_by_tylora.remove(&id);
                }
            }
        }
        self.recovery = unresolved;
        if let Err(code) = self.persist_recovery() {
            warn!(code, "cannot update recovery journal");
        }
    }

    /// Journals windows that Tylora placed mostly outside every display and
    /// forgets them once a later frame is back on screen.
    fn track_offscreen(&mut self, id: ManagedWindowId, actual: Rect) {
        let displays = display_bounds();
        if !mostly_offscreen(actual, &displays) {
            if self.live_offscreen.remove(&id).is_some() {
                self.persist_offscreen();
            }
            return;
        }
        if self
            .live_offscreen
            .get(&id)
            .is_some_and(|record| record.frame.near(actual, 1.0))
        {
            return;
        }
        let Some(safe) = safe_frame(actual, &displays) else {
            return;
        };
        let title = self
            .visibility_element(id)
            .ok()
            .and_then(|element| ax_attribute(element, c"AXTitle").ok())
            .and_then(|value| cf_text(value.0));
        self.live_offscreen.insert(
            id,
            OffscreenRecord {
                process: id.process,
                frame: actual,
                title,
                safe,
            },
        );
        self.persist_offscreen();
    }

    fn persist_offscreen(&self) {
        let path = offscreen_path();
        let records: Vec<_> = self
            .live_offscreen
            .values()
            .chain(self.offscreen_orphans.iter())
            .collect();
        if records.is_empty() {
            if let Err(error) = fs::remove_file(&path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    warn!(%error, "cannot clear off-screen window journal");
                }
            }
            return;
        }
        let result = (|| -> std::io::Result<()> {
            let bytes = serde_json::to_vec(&records).map_err(std::io::Error::other)?;
            let temp = path.with_extension(format!("tmp-{}", std::process::id()));
            let _ = fs::remove_file(&temp);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &path)
        })();
        if let Err(error) = result {
            warn!(%error, "cannot persist off-screen window journal");
        }
    }

    /// Moves journaled off-screen windows back on screen, except `keep`.
    fn restore_offscreen(&mut self, keep: &HashSet<ManagedWindowId>) -> Result<(), i32> {
        let mut first_error = None;
        let ids: Vec<_> = self
            .live_offscreen
            .keys()
            .copied()
            .filter(|id| !keep.contains(id))
            .collect();
        for id in ids {
            let Some(record) = self.live_offscreen.get(&id).cloned() else {
                continue;
            };
            let command = ApplyCommand {
                id,
                generation: 0,
                version: 0,
                target: record.safe,
            };
            match self.apply(command) {
                Ok(_) => {
                    self.live_offscreen.remove(&id);
                    info!(
                        pid = id.process.pid,
                        token = id.token,
                        "off-screen window restored"
                    );
                }
                Err(code) if self.read_geometry(command).is_err() => {
                    // The window is gone; nothing left to restore.
                    self.live_offscreen.remove(&id);
                    debug!(pid = id.process.pid, code, "off-screen window vanished");
                }
                Err(code) => {
                    first_error.get_or_insert(code);
                    warn!(
                        pid = id.process.pid,
                        token = id.token,
                        code,
                        "cannot restore off-screen window"
                    );
                }
            }
        }
        if let Err(code) = self.restore_offscreen_orphans() {
            first_error.get_or_insert(code);
        }
        self.persist_offscreen();
        first_error.map_or(Ok(()), Err)
    }

    /// Matches records from an earlier session by process, frame and title.
    fn restore_offscreen_orphans(&mut self) -> Result<(), i32> {
        if self.offscreen_orphans.is_empty() {
            return Ok(());
        }
        warn!(
            records = self.offscreen_orphans.len(),
            "restoring off-screen windows from previous daemon session"
        );
        let records = std::mem::take(&mut self.offscreen_orphans);
        let mut unresolved = Vec::new();
        for record in records {
            if process_instance(record.process.pid) != Some(record.process) {
                continue;
            }
            self.scan_pid(record.process.pid);
            let matches: Vec<_> = self
                .apps
                .get(&record.process.pid)
                .into_iter()
                .flat_map(|watch| watch.handles.iter())
                .filter(|handle| {
                    ax_rect(handle.element.0).is_ok_and(|frame| frame.near(record.frame, 1.0))
                })
                .filter(|handle| {
                    ax_attribute(handle.element.0, c"AXTitle")
                        .ok()
                        .and_then(|value| cf_text(value.0))
                        == record.title
                })
                .map(|handle| handle.id)
                .collect();
            if matches.len() != 1 {
                warn!(
                    pid = record.process.pid,
                    matches = matches.len(),
                    "off-screen window could not be identified uniquely"
                );
                unresolved.push(record);
                continue;
            }
            let command = ApplyCommand {
                id: matches[0],
                generation: 0,
                version: 0,
                target: record.safe,
            };
            if let Err(code) = self.apply(command) {
                warn!(
                    pid = record.process.pid,
                    code, "cannot restore off-screen window"
                );
                unresolved.push(record);
            }
        }
        let failed = !unresolved.is_empty();
        self.offscreen_orphans = unresolved;
        self.persist_offscreen();
        if failed {
            Err(-25202)
        } else {
            Ok(())
        }
    }

    fn restore_all_visibility(&mut self) -> Result<(), i32> {
        let ids: Vec<_> = self.minimized_by_tylora.iter().copied().collect();
        let mut first_error = None;
        for id in ids {
            if self
                .visibility_element(id)
                .ok()
                .and_then(|element| ax_bool(element, c"AXMinimized"))
                == Some(false)
            {
                self.minimized_by_tylora.remove(&id);
                if let Some(record) = self.live_recovery.remove(&id) {
                    self.recovery.retain(|saved| saved != &record);
                }
                let _ = self.persist_recovery();
                continue;
            }
            if let Err(code) = self.visibility_transition(&[], &[id]) {
                first_error.get_or_insert(code);
                warn!(
                    pid = id.process.pid,
                    token = id.token,
                    code,
                    "could not restore Tylora-minimized window"
                );
            }
        }
        if !self.recovery.is_empty() {
            first_error.get_or_insert(-25202);
        }
        first_error.map_or(Ok(()), Err)
    }

    fn set_minimized(&self, id: ManagedWindowId, value: bool) -> Result<(), i32> {
        let element = self.visibility_element(id)?;
        // Minimize/restore may involve Dock animation. Give this explicit
        // worker operation more time than the ordinary geometry scan.
        let timeout_code = unsafe { AXUIElementSetMessagingTimeout(element, 1.0) };
        if timeout_code != 0 {
            return Err(timeout_code);
        }
        let result = self.set_minimized_with_timeout(element, value);
        let _ = unsafe { AXUIElementSetMessagingTimeout(element, 0.25) };
        result
    }
    fn set_minimized_with_timeout(&self, element: AXRef, value: bool) -> Result<(), i32> {
        let key = cf_string(c"AXMinimized");
        if key.0.is_null() {
            return Err(-1);
        }
        let desired = if value {
            unsafe { kCFBooleanTrue }
        } else {
            unsafe { kCFBooleanFalse }
        };
        let code = unsafe { AXUIElementSetAttributeValue(element, key.0, desired) };
        if code != 0 {
            warn!(code, value, "AXMinimized setter failed");
            return Err(code);
        }
        for _ in 0..4 {
            if ax_bool(element, c"AXMinimized") == Some(value) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        if ax_bool(element, c"AXMinimized") != Some(value) {
            warn!(
                value,
                "AXMinimized verification did not reach requested state"
            );
            return Err(-25204);
        }
        Ok(())
    }

    /// Preflight every window, then apply the smallest possible per-window AX
    /// transaction. On failure, restore completed writes in reverse order.
    /// A failed rollback remains in `minimized_by_tylora` for stop/recovery.
    fn visibility_transition(
        &mut self,
        hide: &[ManagedWindowId],
        show: &[ManagedWindowId],
    ) -> Result<(), i32> {
        for &id in show {
            let element = self.visibility_element(id)?;
            if !self.minimized_by_tylora.contains(&id)
                || !settable(element, c"AXMinimized")
                || ax_bool(element, c"AXMinimized") != Some(true)
            {
                return Err(-25205);
            }
        }
        for &id in hide {
            let element = self.visibility_element(id)?;
            if !settable(element, c"AXMinimized") || ax_bool(element, c"AXMinimized") != Some(false)
            {
                return Err(-25205);
            }
        }
        let new_records: Vec<_> = hide
            .iter()
            .map(|id| self.recovery_record(*id))
            .collect::<Result<_, _>>()?;
        let shown_records: Vec<_> = show
            .iter()
            .filter_map(|id| {
                self.live_recovery
                    .get(id)
                    .cloned()
                    .map(|record| (*id, record))
            })
            .collect();
        self.recovery.extend(new_records.iter().cloned());
        if let Err(code) = self.persist_recovery() {
            self.recovery
                .truncate(self.recovery.len().saturating_sub(new_records.len()));
            return Err(code);
        }
        let mut changed = Vec::new();
        for (&id, value) in show
            .iter()
            .map(|id| (id, false))
            .chain(hide.iter().map(|id| (id, true)))
        {
            match self.set_minimized(id, value) {
                Ok(()) => {
                    if value {
                        self.minimized_by_tylora.insert(id);
                    } else {
                        self.minimized_by_tylora.remove(&id);
                    }
                    if value {
                        if let Some(record) = hide
                            .iter()
                            .position(|candidate| *candidate == id)
                            .and_then(|index| new_records.get(index))
                        {
                            self.live_recovery.insert(id, record.clone());
                        }
                    } else {
                        self.live_recovery.remove(&id);
                    }
                    changed.push((id, !value));
                }
                Err(code) => {
                    let mut rollback_failed = false;
                    // The setter may have succeeded even if its verification
                    // timed out. Try to restore this window as well.
                    if self
                        .visibility_element(id)
                        .ok()
                        .and_then(|element| ax_bool(element, c"AXMinimized"))
                        == Some(value)
                        && self.set_minimized(id, !value).is_err()
                    {
                        rollback_failed = true;
                        if value {
                            self.minimized_by_tylora.insert(id);
                            if let Some(record) = hide
                                .iter()
                                .position(|candidate| *candidate == id)
                                .and_then(|index| new_records.get(index))
                            {
                                self.live_recovery.insert(id, record.clone());
                            }
                        }
                    }
                    for (previous, original) in changed.into_iter().rev() {
                        if let Err(rollback_code) = self.set_minimized(previous, original) {
                            rollback_failed = true;
                            warn!(
                                pid = previous.process.pid,
                                token = previous.token,
                                rollback_code,
                                "visibility rollback failed; recovery needed"
                            );
                        } else if original {
                            self.minimized_by_tylora.insert(previous);
                            if let Some((_, record)) =
                                shown_records.iter().find(|(id, _)| *id == previous)
                            {
                                self.live_recovery.insert(previous, record.clone());
                            }
                        } else {
                            self.minimized_by_tylora.remove(&previous);
                            self.live_recovery.remove(&previous);
                        }
                    }
                    self.recovery.retain(|record| {
                        if !new_records.iter().any(|new| {
                            new.process == record.process
                                && new.frame == record.frame
                                && new.title == record.title
                        }) {
                            return true;
                        }
                        self.apps
                            .get(&record.process.pid)
                            .into_iter()
                            .flat_map(|watch| watch.handles.iter())
                            .any(|handle| {
                                ax_rect(handle.element.0)
                                    .is_ok_and(|frame| frame.near(record.frame, 1.0))
                                    && ax_bool(handle.element.0, c"AXMinimized") == Some(true)
                            })
                    });
                    if self.persist_recovery().is_err() {
                        rollback_failed = true;
                    }
                    return Err(if rollback_failed {
                        VISIBILITY_ROLLBACK_FAILED
                    } else {
                        code
                    });
                }
            }
        }
        self.recovery
            .retain(|saved| !shown_records.iter().any(|(_, record)| record == saved));
        self.persist_recovery()
            .map_err(|_| VISIBILITY_ROLLBACK_FAILED)?;
        for pid in hide
            .iter()
            .chain(show)
            .map(|id| id.process.pid)
            .collect::<HashSet<_>>()
        {
            self.scan_pid(pid);
        }
        Ok(())
    }

    /// AX setters can return before the target application updates its geometry.
    /// The caller checks the result on a later worker loop iteration.
    fn apply(&mut self, command: ApplyCommand) -> Result<Option<Rect>, i32> {
        let watch = self.apps.get(&command.id.process.pid).ok_or(-1)?;
        if watch.process != command.id.process {
            return Err(-1);
        }
        let handle = watch
            .handles
            .iter()
            .find(|h| h.id == command.id)
            .ok_or(-1)?;
        let current = ax_rect(handle.element.0).map_err(|(_, code)| code)?;
        if current.near(command.target, 1.0) {
            return Ok(Some(current));
        }
        // Checked at write time: the user may have just left this window's
        // Space, and WindowServer can switch straight back to a window whose
        // geometry changes. The coordinator retries after its backoff.
        if crate::spaces::ax_cg_window(handle.element.0)
            .is_some_and(|window| !crate::spaces::window_on_active_space(window))
        {
            return Err(INACTIVE_SPACE);
        }
        if !settable(handle.element.0, c"AXSize") || !settable(handle.element.0, c"AXPosition") {
            return Err(-25205);
        }
        let size = CGSize {
            width: command.target.width,
            height: command.target.height,
        };
        let position = CGPoint {
            x: command.target.x,
            y: command.target.y,
        };
        let size_value = OwnedCF(unsafe { AXValueCreate(2, (&size as *const CGSize).cast()) });
        let pos_value = OwnedCF(unsafe { AXValueCreate(1, (&position as *const CGPoint).cast()) });
        if size_value.0.is_null() || pos_value.0.is_null() {
            return Err(-1);
        }
        let size_key = cf_string(c"AXSize");
        let pos_key = cf_string(c"AXPosition");
        if size_key.0.is_null() || pos_key.0.is_null() {
            return Err(-1);
        }
        let pos_code =
            unsafe { AXUIElementSetAttributeValue(handle.element.0, pos_key.0, pos_value.0) };
        if pos_code != 0 {
            return Err(pos_code);
        }
        let size_code =
            unsafe { AXUIElementSetAttributeValue(handle.element.0, size_key.0, size_value.0) };
        if size_code != 0 {
            return Err(size_code);
        }
        Ok(None)
    }
}

/// Animation Engine integration: the scheduler decides the frames, these
/// methods write them. Intermediate frames are never read back; the final
/// one goes through `apply` and is verified like any instant change.
impl Worker {
    /// Reports an apply result to the coordinator. False once the event
    /// channel is gone (daemon stopping).
    fn report_apply(&mut self, command: ApplyCommand, result: Result<Option<Rect>, i32>) -> bool {
        match result {
            Ok(None) => {
                self.pending_applies
                    .push((command, Instant::now() + Duration::from_millis(200)));
                true
            }
            Ok(Some(actual)) => {
                self.track_offscreen(command.id, actual);
                self.events
                    .send(ManagedEvent::Applied {
                        command,
                        result: Ok(actual),
                    })
                    .is_ok()
            }
            Err(code) => {
                let sent = self
                    .events
                    .send(ManagedEvent::Applied {
                        command,
                        result: Err(code),
                    })
                    .is_ok();
                if code != INACTIVE_SPACE && code != ANIMATION_CANCELLED {
                    self.scan_pid(command.id.process.pid);
                }
                sent
            }
        }
    }

    /// Instant apply. A running transition of the window is superseded by
    /// this newer target and dropped without a report of its own.
    fn apply_now(&mut self, command: ApplyCommand) -> bool {
        let _ = self.animations.cancel(command.id);
        let start = Instant::now();
        let result = self.apply(command);
        debug!(
            pid = command.id.process.pid,
            token = command.id.token,
            elapsed_us = start.elapsed().as_micros(),
            "AX geometry write completed"
        );
        self.publish_animation_metrics();
        self.report_apply(command, result)
    }

    fn handle_for(&self, id: ManagedWindowId) -> Option<&AxHandle> {
        let watch = self.apps.get(&id.process.pid)?;
        (watch.process == id.process)
            .then(|| watch.handles.iter().find(|handle| handle.id == id))
            .flatten()
    }

    /// Animates `command` when the policy allows it, else applies it now.
    fn animate(&mut self, command: ApplyCommand, event: AnimationEvent) -> bool {
        let id = command.id;
        // A running transition continues from where it is drawn now.
        let from = match self.animations.get(id) {
            Some(running) => Some(running.current),
            None => self
                .handle_for(id)
                .and_then(|handle| ax_rect(handle.element.0).ok()),
        };
        let Some(from) = from else {
            return self.apply_now(command);
        };
        let checked = Instant::now();
        let constraints = self.window_constraints(id, from, command.target);
        debug!(
            elapsed_us = checked.elapsed().as_micros(),
            ?constraints,
            "animation constraints checked"
        );
        match self
            .animation_policy
            .decide(Some(event), from, command.target, constraints)
        {
            Decision::Instant(reason) => {
                debug!(
                    pid = id.process.pid,
                    token = id.token,
                    ?event,
                    ?reason,
                    "geometry applied instantly"
                );
                *self
                    .instant_reasons
                    .entry(format!("{reason:?}"))
                    .or_default() += 1;
                self.animations.metrics_mut().instant += 1;
                self.apply_now(command)
            }
            Decision::Animate(curve) => {
                debug!(pid = id.process.pid, token = id.token, ?event, ?from, to = ?command.target, "animation started");
                let started = self.animations.start(
                    StartRequest {
                        id,
                        generation: command.generation,
                        version: command.version,
                        from,
                        to: command.target,
                        curve,
                        completion: Completion::Apply(command),
                    },
                    Instant::now(),
                );
                // A superseded layout target needs no report: the
                // coordinator already waits for the newer version.
                if let Started::Retargeted { superseded } = started {
                    debug!(
                        pid = id.process.pid,
                        token = id.token,
                        "animation retargeted"
                    );
                    self.report_superseded(superseded, from);
                }
                // The first frame goes out in this iteration.
                self.tick_animations()
            }
        }
    }

    fn window_constraints(&self, id: ManagedWindowId, from: Rect, to: Rect) -> WindowConstraints {
        let displays = display_bounds();
        let window = self
            .handle_for(id)
            .and_then(|handle| crate::spaces::ax_cg_window(handle.element.0));
        WindowConstraints {
            slow_application: self
                .slow_pids
                .get(&id.process.pid)
                .is_some_and(|until| Instant::now() < *until),
            offscreen: self.parked.contains(&id)
                || mostly_offscreen(from, &displays)
                || mostly_offscreen(to, &displays),
            cross_display: display_for(from) != display_for(to),
            inactive_space: window
                .is_some_and(|window| !crate::spaces::window_on_active_space(window)),
            dragging: false,
        }
    }

    fn configure_animations(&mut self, config: AnimationConfig, reduce_motion: bool) {
        self.animation_policy = AnimationPolicy {
            config,
            reduce_motion,
            accessibility: super::trusted(),
        };
        self.animations.set_interval(config.frame_interval());
        // Settings that rule animations out end the running ones at once.
        let still = self.animation_policy.decide(
            Some(AnimationEvent::LayoutChange),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            Rect {
                x: 100.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            WindowConstraints::default(),
        );
        if matches!(still, Decision::Instant(_)) {
            let _ = self.finish_all_animations();
        }
        self.publish_animation_metrics();
    }

    /// Writes the due frames. False once the event channel is gone.
    fn tick_animations(&mut self) -> bool {
        if self.animations.next_deadline().is_none() {
            return true;
        }
        let frames = self.animations.frames(Instant::now());
        let mut ok = true;
        for frame in frames {
            match frame {
                Frame::Step {
                    id,
                    frame,
                    previous,
                } => match self.write_step(id, frame, previous) {
                    Ok(latency) => {
                        self.animations.metrics_mut().record_write(latency);
                        let pid = id.process.pid;
                        let strikes = if latency > SLOW_FRAME_WRITE {
                            let strikes = self.slow_strikes.entry(pid).or_default();
                            *strikes = strikes.saturating_add(1);
                            *strikes
                        } else {
                            self.slow_strikes.remove(&pid);
                            0
                        };
                        if strikes >= 2 || latency > STALLED_FRAME_WRITE {
                            // The app cannot keep up: finish this window now
                            // and move its windows instantly for a while.
                            warn!(
                                pid,
                                latency_ms = latency.as_millis(),
                                "animation frame writes too slow; this app moves instantly for now"
                            );
                            self.slow_strikes.remove(&pid);
                            self.slow_pids
                                .insert(pid, Instant::now() + SLOW_APPLICATION_COOLDOWN);
                            if let Some(transition) = self.animations.cancel(id) {
                                ok &= self.finish_transition(transition);
                            }
                        }
                    }
                    Err(code) => {
                        debug!(
                            pid = id.process.pid,
                            token = id.token,
                            code,
                            "animation frame write failed"
                        );
                        // The final apply reports what really happened.
                        if let Some(transition) = self.animations.fail(id) {
                            ok &= self.finish_transition(transition);
                        }
                    }
                },
                Frame::Final(transition) => ok &= self.finish_transition(transition),
            }
        }
        self.publish_animation_metrics();
        ok
    }

    /// Writes the target of a transition and reports it like an apply.
    fn finish_transition(&mut self, transition: Transition<Completion>) -> bool {
        debug!(
            pid = transition.id.process.pid,
            token = transition.id.token,
            status = ?transition.status,
            elapsed_ms = transition.started.elapsed().as_millis(),
            "animation finished"
        );
        let command = ApplyCommand {
            id: transition.id,
            generation: transition.generation,
            version: transition.version,
            target: transition.to,
        };
        match transition.completion {
            Completion::Apply(command) => {
                let result = self.apply(command);
                if let Ok(Some(actual)) = result {
                    self.animations
                        .metrics_mut()
                        .record_final_error(command.target, actual);
                }
                self.report_apply(command, result)
            }
            Completion::Present { request } => {
                let result = self
                    .apply(command)
                    .and_then(|_| self.read_geometry(command));
                if let Ok(actual) = result {
                    self.animations
                        .metrics_mut()
                        .record_final_error(command.target, actual);
                }
                self.events
                    .send(ManagedEvent::Presented { request, result })
                    .is_ok()
            }
            Completion::Slide { request } => {
                let result = self.apply(command).map(|_| ());
                self.events
                    .send(ManagedEvent::Slid { request, result })
                    .is_ok()
            }
        }
    }

    /// A transition replaced by a newer target of the same window. Layout
    /// targets are superseded silently; a scratchpad step still answers, at
    /// the frame it reached, so its operation never hangs.
    fn report_superseded(&mut self, superseded: Completion, reached: Rect) {
        match superseded {
            Completion::Apply(_) => {}
            Completion::Present { request } => {
                let _ = self.events.send(ManagedEvent::Presented {
                    request,
                    result: Ok(reached),
                });
            }
            Completion::Slide { request } => {
                let _ = self.events.send(ManagedEvent::Slid {
                    request,
                    result: Ok(()),
                });
            }
        }
    }

    /// The managed id of `pid`'s window `cg_window_id`, scanning the app if
    /// it is not known yet (it may just have arrived from another Space).
    fn window_by_cg(&mut self, pid: i32, cg_window_id: u32) -> Option<ManagedWindowId> {
        let find = |worker: &Self| {
            worker.apps.get(&pid).and_then(|watch| {
                watch
                    .handles
                    .iter()
                    .find(|handle| {
                        crate::spaces::ax_cg_window(handle.element.0) == Some(cg_window_id)
                    })
                    .map(|handle| handle.id)
            })
        };
        find(self).or_else(|| {
            self.scan_pid(pid);
            find(self)
        })
    }

    /// Scratchpad show that glides into `frame`. False when the policy wants
    /// it instant; the caller then presents it the usual way.
    fn present_animated(
        &mut self,
        request: u64,
        pid: i32,
        cg_window_id: u32,
        frame: Rect,
        focus: bool,
    ) -> bool {
        // A window that just arrived from another Space has no handle yet.
        self.scan_pid(pid);
        let Some(id) = self.window_by_cg(pid, cg_window_id) else {
            return false;
        };
        let Some(from) = self
            .handle_for(id)
            .and_then(|handle| ax_rect(handle.element.0).ok())
        else {
            return false;
        };
        let constraints = self.window_constraints(id, from, frame);
        let Decision::Animate(curve) = self.animation_policy.decide(
            Some(AnimationEvent::Scratchpad),
            from,
            frame,
            constraints,
        ) else {
            return false;
        };
        // Focus first: the window is usable while it is still moving.
        if focus {
            if let Err(code) = self.focus(id) {
                let _ = self.events.send(ManagedEvent::Presented {
                    request,
                    result: Err(code),
                });
                return true;
            }
        }
        let started = self.animations.start(
            StartRequest {
                id,
                generation: 0,
                version: 0,
                from,
                to: frame,
                curve,
                completion: Completion::Present { request },
            },
            Instant::now(),
        );
        if let Started::Retargeted { superseded } = started {
            self.report_superseded(superseded, from);
        }
        let _ = self.tick_animations();
        true
    }

    /// Scratchpad hide, first part: a short glide out before parking.
    fn slide(
        &mut self,
        request: u64,
        pid: i32,
        cg_window_id: u32,
        direction: SlideDirection,
    ) -> bool {
        let done = |worker: &Self, result| {
            worker
                .events
                .send(ManagedEvent::Slid { request, result })
                .is_ok()
        };
        let Some(id) = self.window_by_cg(pid, cg_window_id) else {
            return done(self, Err(-25202));
        };
        let Some(from) = self
            .handle_for(id)
            .and_then(|handle| ax_rect(handle.element.0).ok())
        else {
            return done(self, Err(-25202));
        };
        let to = direction.offstage(from);
        let constraints = self.window_constraints(id, from, to);
        match self
            .animation_policy
            .decide(Some(AnimationEvent::Scratchpad), from, to, constraints)
        {
            // No motion: park the window where it is.
            Decision::Instant(_) => done(self, Ok(())),
            Decision::Animate(curve) => {
                let started = self.animations.start(
                    StartRequest {
                        id,
                        generation: 0,
                        version: 0,
                        from,
                        to,
                        curve,
                        completion: Completion::Slide { request },
                    },
                    Instant::now(),
                );
                if let Started::Retargeted { superseded } = started {
                    self.report_superseded(superseded, from);
                }
                self.tick_animations()
            }
        }
    }

    /// Ends every transition at its target (Space switch, display change,
    /// settings change, stop).
    fn finish_all_animations(&mut self) -> bool {
        let mut ok = true;
        for transition in self.animations.cancel_all() {
            ok &= self.finish_transition(transition);
        }
        self.publish_animation_metrics();
        ok
    }

    /// Stops a transition where it is, for the user's drag. Its layout target
    /// is reported as abandoned so the coordinator plans again afterwards.
    fn cancel_animation(&mut self, id: ManagedWindowId) {
        let Some(transition) = self.animations.cancel(id) else {
            return;
        };
        match transition.completion {
            Completion::Apply(command) => {
                let _ = self.events.send(ManagedEvent::Applied {
                    command,
                    result: Err(ANIMATION_CANCELLED),
                });
            }
            // The user took the scratchpad mid-glide: it is where they hold it.
            other => self.report_superseded(other, transition.current),
        }
        self.publish_animation_metrics();
    }

    /// One intermediate frame: only the attributes that change, never read
    /// back. Growing windows move first and shrinking ones resize first, so
    /// no step pushes the window against a display edge.
    fn write_step(
        &self,
        id: ManagedWindowId,
        frame: Rect,
        previous: Rect,
    ) -> Result<Duration, i32> {
        let handle = self.handle_for(id).ok_or(-25202)?;
        let element = handle.element.0;
        let started = Instant::now();
        let set = |name: &'static CStr, kind: u32, value: *const c_void| -> Result<(), i32> {
            let key = cf_string(name);
            let value = OwnedCF(unsafe { AXValueCreate(kind, value) });
            if key.0.is_null() || value.0.is_null() {
                return Err(-1);
            }
            match unsafe { AXUIElementSetAttributeValue(element, key.0, value.0) } {
                0 => Ok(()),
                code => Err(code),
            }
        };
        let position = CGPoint {
            x: frame.x,
            y: frame.y,
        };
        let size = CGSize {
            width: frame.width,
            height: frame.height,
        };
        let moves = frame.x != previous.x || frame.y != previous.y;
        let resizes = frame.width != previous.width || frame.height != previous.height;
        let grows = frame.width * frame.height > previous.width * previous.height;
        if resizes && !grows {
            set(c"AXSize", 2, (&size as *const CGSize).cast())?;
        }
        if moves {
            set(c"AXPosition", 1, (&position as *const CGPoint).cast())?;
        }
        if resizes && grows {
            set(c"AXSize", 2, (&size as *const CGSize).cast())?;
        }
        Ok(started.elapsed())
    }

    fn publish_animation_metrics(&self) {
        if let Ok(mut metrics) = ANIMATION_METRICS.lock() {
            *metrics = Some(self.animations.metrics());
        }
        if let Ok(mut state) = ANIMATION_STATE.lock() {
            let now = Instant::now();
            *state = Some(AnimationState {
                policy_enabled: self.animation_policy.config.enabled,
                backend: self.animation_policy.backend(),
                reduce_motion: self.animation_policy.reduce_motion,
                accessibility: self.animation_policy.accessibility,
                slow_applications: self
                    .slow_pids
                    .iter()
                    .filter(|(_, until)| now < **until)
                    .map(|(pid, _)| *pid)
                    .collect(),
                instant_reasons: self.instant_reasons.clone(),
            });
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut slot) = AX_RUN_LOOP.lock() {
            if let Some(pointer) = slot.take() {
                unsafe { CFRelease(pointer as CFRef) };
            }
        }
        if self.process_queue >= 0 {
            unsafe {
                close(self.process_queue);
            }
        }
    }
}

#[cfg(test)]
mod offscreen_tests {
    use super::*;
    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect::new(x, y, w, h).unwrap()
    }
    #[test]
    fn parked_sliver_is_offscreen_and_partial_columns_are_not() {
        let displays = [rect(0.0, 0.0, 1470.0, 956.0)];
        assert!(mostly_offscreen(
            rect(1430.0, 924.0, 900.0, 900.0),
            &displays
        ));
        assert!(mostly_offscreen(
            rect(-860.0, 924.0, 900.0, 900.0),
            &displays
        ));
        assert!(!mostly_offscreen(
            rect(800.0, 41.0, 900.0, 900.0),
            &displays
        ));
        assert!(!mostly_offscreen(rect(8.0, 41.0, 700.0, 900.0), &displays));
    }
    #[test]
    fn safe_frame_returns_inside_the_overlapping_display() {
        let displays = [
            rect(0.0, 0.0, 1470.0, 956.0),
            rect(-201.0, -1080.0, 1920.0, 1080.0),
        ];
        let safe = safe_frame(rect(1430.0, 924.0, 900.0, 900.0), &displays).unwrap();
        assert!(safe.x >= 8.0 && safe.x + safe.width <= 1462.0 + 1e-6);
        assert!(safe.y >= 40.0 && safe.y + safe.height <= 948.0 + 1e-6);
        let wide = safe_frame(rect(1679.0, -32.0, 2500.0, 600.0), &displays).unwrap();
        assert!(wide.y < 0.0, "stays on the display it was parked on");
        assert!(wide.width <= 1904.0);
    }
}

#[cfg(test)]
mod abi_tests {
    use super::{KEvent, ProcBsdInfo};
    #[test]
    fn native_structs_match_macos_sdk_layout() {
        assert_eq!(std::mem::size_of::<ProcBsdInfo>(), 136);
        assert_eq!(std::mem::offset_of!(ProcBsdInfo, start_sec), 120);
        assert_eq!(std::mem::size_of::<KEvent>(), 32);
    }
}
