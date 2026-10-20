use super::{
    DesktopNumber, DisplaySpaceOrder, NativeSpace, NativeSpaceBackend, NativeSpaceId, OrderedSpace,
    SpaceCapabilities, SpaceError, SpaceRegistry,
};
use serde::Serialize;
use std::{
    ffi::{c_char, c_int, c_void, CString},
    fs::{File, OpenOptions},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
        Arc, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use tylora_core::DisplayId;

type CFRef = *const c_void;
type Connection = unsafe extern "C" fn() -> c_int;
type CopyDisplaySpaces = unsafe extern "C" fn(c_int) -> CFRef;
type CurrentSpace = unsafe extern "C" fn(c_int, CFRef) -> u64;
type SpaceType = unsafe extern "C" fn(c_int, u64) -> c_int;
type SpacesForWindows = unsafe extern "C" fn(c_int, c_int, CFRef) -> CFRef;
type SetFrontProcess = unsafe extern "C" fn(*const ProcessSerialNumber, u32, u32) -> c_int;
type PostEventRecord = unsafe extern "C" fn(*const ProcessSerialNumber, *const u8) -> c_int;

#[repr(C)]
#[derive(Default)]
struct ProcessSerialNumber {
    high: u32,
    low: u32,
}

const UTF8: u32 = 0x0800_0100;
static SWITCH_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutcome {
    pub source: NativeSpaceId,
    pub target: NativeSpaceId,
    pub display: DisplayId,
    pub backend: &'static str,
    pub elapsed_ms: u128,
    pub confirmed: bool,
}

pub struct NativeSpaceSwitcher;
impl NativeSpaceSwitcher {
    pub fn cancel_pending() {
        SWITCH_GENERATION.fetch_add(1, Ordering::AcqRel);
    }

    pub fn focus_id(
        display: DisplayId,
        target: NativeSpaceId,
    ) -> Result<SwitchOutcome, SpaceError> {
        let generation = SWITCH_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
        let _lock = switch_lock()?;
        let backend = ExperimentalSkyLightBackend::new();
        let started = Instant::now();
        // Fullscreen Spaces count: the user may be leaving or entering one.
        let initial = backend.display_order_for(display)?.active;
        if initial == target {
            return Ok(SwitchOutcome {
                source: initial,
                target,
                display,
                backend: "dock_gesture",
                elapsed_ms: 0,
                confirmed: true,
            });
        }
        if cursor_display() != Some(display) {
            return Err(SpaceError::SwitchNotOnPointerDisplay);
        }
        if !super::super::accessibility_trusted() {
            return Err(SpaceError::Unsupported(
                "Accessibility permission is required to post Dock gesture events".into(),
            ));
        }
        // Jump the whole distance at once with instant swipes; the Dock
        // never settles on the Desktops in between. If they have no effect
        // (another macOS release), fall back to animated one-step swipes.
        let mut instant = true;
        let mut rounds = 0;
        while rounds < 12 && started.elapsed() < Duration::from_secs(12) {
            if SWITCH_GENERATION.load(Ordering::Acquire) != generation {
                return Err(SpaceError::SwitchCancelled);
            }
            let order = backend.display_order_for(display)?;
            let current = order.active;
            if current == target {
                return Ok(SwitchOutcome {
                    source: initial,
                    target,
                    display,
                    backend: if instant {
                        "instant_dock_gesture"
                    } else {
                        "dock_gesture"
                    },
                    elapsed_ms: started.elapsed().as_millis(),
                    confirmed: true,
                });
            }
            let (distance, sign) = gesture_distance(&order.ids(), current, target)?;
            if instant {
                post_instant_jump(sign, distance)?;
            } else {
                post_gesture(sign)?;
            }
            let deadline =
                Instant::now() + Duration::from_millis(if instant { 1500 } else { 2000 });
            loop {
                if SWITCH_GENERATION.load(Ordering::Acquire) != generation {
                    return Err(SpaceError::SwitchCancelled);
                }
                // Any movement counts: a partial jump continues from there.
                if backend
                    .display_order_for(display)
                    .is_ok_and(|order| order.active != current)
                {
                    break;
                }
                if Instant::now() >= deadline {
                    if !instant {
                        return Err(SpaceError::SwitchTimeout);
                    }
                    instant = false;
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            rounds += 1;
        }
        Err(SpaceError::SwitchTimeout)
    }
}

/// How many Spaces separate `current` from `target` in the display's
/// Mission Control order, and the swipe direction (+1 towards the end).
fn gesture_distance(
    ordered: &[NativeSpaceId],
    current: NativeSpaceId,
    target: NativeSpaceId,
) -> Result<(usize, f64), SpaceError> {
    let from = ordered
        .iter()
        .position(|id| *id == current)
        .ok_or(SpaceError::VerificationFailed)?;
    let to = ordered
        .iter()
        .position(|id| *id == target)
        .ok_or(SpaceError::Unsupported(
            "target native Space no longer exists on this display".into(),
        ))?;
    match from.cmp(&to) {
        std::cmp::Ordering::Equal => Err(SpaceError::VerificationFailed),
        std::cmp::Ordering::Less => Ok((to - from, 1.0)),
        std::cmp::Ordering::Greater => Ok((from - to, -1.0)),
    }
}

#[cfg(test)]
mod gesture_tests {
    use super::*;
    fn space(id: u64) -> NativeSpaceId {
        NativeSpaceId(id)
    }
    #[test]
    fn gesture_distance_tracks_current_order_and_deleted_target_fails() {
        let original = vec![space(4), space(7), space(5)];
        assert_eq!(
            gesture_distance(&original, NativeSpaceId(4), NativeSpaceId(5)).unwrap(),
            (2, 1.0)
        );
        let reordered = vec![space(5), space(4), space(7)];
        assert_eq!(
            gesture_distance(&reordered, NativeSpaceId(4), NativeSpaceId(5)).unwrap(),
            (1, -1.0)
        );
        assert!(matches!(
            gesture_distance(&reordered, NativeSpaceId(4), NativeSpaceId(99)),
            Err(SpaceError::Unsupported(_))
        ));
    }
}

fn switch_lock() -> Result<File, SpaceError> {
    let path =
        std::env::temp_dir().join(format!("tylora-space-switch-{}.lock", unsafe { geteuid() }));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| SpaceError::QueryFailed("Space switch lock file"))?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
        if Instant::now() >= deadline {
            return Err(SpaceError::SwitchBusy);
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(file)
}

/// Swipes `steps` Spaces towards `sign` without the Dock's sliding
/// animation, the form InstantSpaceSwitcher uses up to macOS 26: per step a
/// began, changed and ended Dock swipe with an almost zero progress (nothing
/// left to animate) and a high velocity that commits it, all posted back to
/// back. The velocity grows with the distance, as there.
fn post_instant_jump(sign: f64, steps: usize) -> Result<(), SpaceError> {
    let velocity = 2000.0 * steps as f64 * sign;
    for _ in 0..steps {
        for phase in [1, 2, 4] {
            let event = OwnedCF(unsafe { CGEventCreate(ptr::null()) });
            if event.0.is_null() {
                return Err(SpaceError::QueryFailed("CGEventCreate"));
            }
            unsafe {
                CGEventSetIntegerValueField(event.0, 55, 30); // Dock control event
                CGEventSetIntegerValueField(event.0, 110, 23); // Dock swipe HID type
                CGEventSetIntegerValueField(event.0, 132, phase);
                CGEventSetDoubleValueField(event.0, 124, 0.000016 * sign);
                CGEventSetIntegerValueField(event.0, 123, 1); // horizontal
                CGEventSetDoubleValueField(event.0, 129, velocity);
                CGEventSetDoubleValueField(event.0, 130, velocity);
                CGEventPost(1, event.0); // session event tap
            }
        }
    }
    Ok(())
}

fn post_gesture(sign: f64) -> Result<(), SpaceError> {
    let event = OwnedCF(unsafe { CGEventCreate(ptr::null()) });
    if event.0.is_null() {
        return Err(SpaceError::QueryFailed("CGEventCreate"));
    }
    unsafe {
        CGEventSetIntegerValueField(event.0, 55, 30); // Dock control event
        CGEventSetIntegerValueField(event.0, 110, 23); // Dock swipe HID type
        CGEventSetIntegerValueField(event.0, 123, 1); // horizontal
        CGEventSetDoubleValueField(event.0, 124, sign);
        CGEventSetDoubleValueField(event.0, 129, sign * 9999.0);
        CGEventSetIntegerValueField(event.0, 132, 1); // begin
        CGEventPost(1, event.0);
        CGEventSetIntegerValueField(event.0, 132, 4); // end
        CGEventPost(1, event.0);
    }
    Ok(())
}

#[repr(C)]
struct CFUUIDBytes([u8; 16]);

struct OwnedCF(CFRef);
impl Drop for OwnedCF {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CFRelease(self.0) };
        }
    }
}
fn key(value: &str) -> OwnedCF {
    let Ok(value) = CString::new(value) else {
        return OwnedCF(ptr::null());
    };
    OwnedCF(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), UTF8) })
}
fn dictionary_value(dictionary: CFRef, name: &str) -> CFRef {
    if dictionary.is_null() {
        return ptr::null();
    }
    let key = key(name);
    if key.0.is_null() {
        return ptr::null();
    }
    unsafe { CFDictionaryGetValue(dictionary, key.0) }
}
fn number(value: CFRef) -> Option<i64> {
    if value.is_null() {
        return None;
    }
    let mut result = 0_i64;
    (unsafe { CFNumberGetValue(value, 4, (&mut result as *mut i64).cast()) } != 0).then_some(result)
}
fn array_items(array: CFRef) -> impl Iterator<Item = CFRef> {
    let count = if array.is_null() {
        0
    } else {
        unsafe { CFArrayGetCount(array) }
    };
    (0..count).map(move |index| unsafe { CFArrayGetValueAtIndex(array, index) })
}

struct Symbols {
    connection: Option<Connection>,
    copy_displays: Option<CopyDisplaySpaces>,
    current_space: Option<CurrentSpace>,
    space_type: Option<SpaceType>,
    spaces_for_windows: Option<SpacesForWindows>,
    set_front_process: Option<SetFrontProcess>,
    post_event_record: Option<PostEventRecord>,
}
impl Symbols {
    fn load() -> Self {
        let path = c"/System/Library/PrivateFrameworks/SkyLight.framework/Versions/A/SkyLight";
        // The dyld shared cache can provide this framework even without a
        // standalone file at the path. Keep it loaded for process lifetime.
        let library = unsafe { dlopen(path.as_ptr(), 1) };
        if library.is_null() {
            return Self {
                connection: None,
                copy_displays: None,
                current_space: None,
                space_type: None,
                spaces_for_windows: None,
                set_front_process: None,
                post_event_record: None,
            };
        }
        let lookup = |name: &std::ffi::CStr| unsafe { dlsym(library, name.as_ptr()) };
        let connection = lookup(c"SLSMainConnectionID");
        let copy_displays = lookup(c"SLSCopyManagedDisplaySpaces");
        let current_space = lookup(c"SLSManagedDisplayGetCurrentSpace");
        let space_type = lookup(c"SLSSpaceGetType");
        let spaces_for_windows = lookup(c"SLSCopySpacesForWindows");
        let set_front_process = lookup(c"_SLPSSetFrontProcessWithOptions");
        let post_event_record = lookup(c"SLPSPostEventRecordTo");
        Self {
            connection: (!connection.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, Connection>(connection) }),
            copy_displays: (!copy_displays.is_null()).then(|| unsafe {
                std::mem::transmute::<*mut c_void, CopyDisplaySpaces>(copy_displays)
            }),
            current_space: (!current_space.is_null()).then(|| unsafe {
                std::mem::transmute::<*mut c_void, CurrentSpace>(current_space)
            }),
            space_type: (!space_type.is_null())
                .then(|| unsafe { std::mem::transmute::<*mut c_void, SpaceType>(space_type) }),
            spaces_for_windows: (!spaces_for_windows.is_null()).then(|| unsafe {
                std::mem::transmute::<*mut c_void, SpacesForWindows>(spaces_for_windows)
            }),
            set_front_process: (!set_front_process.is_null()).then(|| unsafe {
                std::mem::transmute::<*mut c_void, SetFrontProcess>(set_front_process)
            }),
            post_event_record: (!post_event_record.is_null()).then(|| unsafe {
                std::mem::transmute::<*mut c_void, PostEventRecord>(post_event_record)
            }),
        }
    }
    fn can_list(&self) -> bool {
        self.connection.is_some()
            && self.copy_displays.is_some()
            && self.current_space.is_some()
            && self.space_type.is_some()
    }
}
fn symbols() -> &'static Symbols {
    static SYMBOLS: OnceLock<Symbols> = OnceLock::new();
    SYMBOLS.get_or_init(Symbols::load)
}

#[derive(Default)]
pub struct ExperimentalSkyLightBackend;

/// SkyLight Space type of a native fullscreen (or split view) Space.
const FULLSCREEN_SPACE_TYPE: c_int = 4;

impl ExperimentalSkyLightBackend {
    /// Every user Desktop and fullscreen Space per display, in Mission
    /// Control order, with the display's active Space.
    pub fn display_order(&self) -> Result<Vec<DisplaySpaceOrder>, SpaceError> {
        let syms = symbols();
        let (Some(connection), Some(copy_displays), Some(current_space), Some(space_type)) = (
            syms.connection,
            syms.copy_displays,
            syms.current_space,
            syms.space_type,
        ) else {
            return Err(SpaceError::Unsupported(
                "SkyLight discovery symbols are missing".into(),
            ));
        };
        let cid = unsafe { connection() };
        let displays = OwnedCF(unsafe { copy_displays(cid) });
        if displays.0.is_null() {
            return Err(SpaceError::QueryFailed("SLSCopyManagedDisplaySpaces"));
        }
        let mut result = Vec::new();
        for display in array_items(displays.0) {
            let identifier = dictionary_value(display, "Display Identifier");
            if identifier.is_null() {
                continue;
            }
            let uuid = OwnedCF(unsafe { CFUUIDCreateFromString(ptr::null(), identifier) });
            if uuid.0.is_null() {
                continue;
            }
            let display_id = DisplayId(unsafe { CGDisplayGetDisplayIDFromUUID(uuid.0) });
            if display_id.0 == 0 {
                continue;
            }
            let active = unsafe { current_space(cid, identifier) };
            let spaces = array_items(dictionary_value(display, "Spaces"))
                .filter_map(|space| number(dictionary_value(space, "id64")))
                .filter(|id| *id > 0)
                .filter_map(|id| {
                    match unsafe { space_type(cid, id as u64) } {
                        0 => Some(false),
                        FULLSCREEN_SPACE_TYPE => Some(true),
                        _ => None,
                    }
                    .map(|fullscreen| OrderedSpace {
                        id: NativeSpaceId(id as u64),
                        fullscreen,
                    })
                })
                .collect();
            let bytes = unsafe { CFUUIDGetUUIDBytes(uuid.0) };
            result.push(DisplaySpaceOrder {
                display: display_id,
                uuid: Some(u128::from_be_bytes(bytes.0)),
                active: NativeSpaceId(active),
                spaces,
            });
        }
        Ok(result)
    }

    pub fn display_order_for(&self, display: DisplayId) -> Result<DisplaySpaceOrder, SpaceError> {
        self.display_order()?
            .into_iter()
            .find(|order| order.display == display)
            .ok_or(SpaceError::QueryFailed("display Spaces"))
    }
}

/// Whether `window` is on a Space some display shows right now. Unknown
/// membership counts as active, so a failing query never blocks layout.
pub(crate) fn window_on_active_space(window: u32) -> bool {
    let backend = ExperimentalSkyLightBackend::new();
    let Ok(spaces) = backend.get_window_spaces(window) else {
        return true;
    };
    let [space] = spaces.as_slice() else {
        return true;
    };
    backend.display_order().map_or(true, |orders| {
        orders.iter().any(|order| order.active == *space)
    })
}

/// Makes `window_id` the key window of its process and that process the
/// front one, through WindowServer. Unlike activating the application, this
/// never jumps to a Space holding another of the app's windows, and it needs
/// no Accessibility scan of the app. The window must be on an active Space.
pub fn focus_window(pid: i32, window_id: u32) -> Result<(), SpaceError> {
    let syms = symbols();
    let (Some(set_front), Some(post_event)) = (syms.set_front_process, syms.post_event_record)
    else {
        return Err(SpaceError::Unsupported(
            "SkyLight window focus symbols are missing".into(),
        ));
    };
    let mut psn = ProcessSerialNumber::default();
    if unsafe { GetProcessForPID(pid, &mut psn) } != 0 {
        return Err(SpaceError::QueryFailed("GetProcessForPID"));
    }
    // kCPSUserGenerated: treated like a click, so the app may take focus.
    if unsafe { set_front(&psn, window_id, 0x200) } != 0 {
        return Err(SpaceError::QueryFailed("_SLPSSetFrontProcessWithOptions"));
    }
    // Synthetic window-server event records that make the window key.
    let mut bytes = [0u8; 0xf8];
    bytes[0x04] = 0xf8;
    bytes[0x3a] = 0x10;
    bytes[0x3c..0x40].copy_from_slice(&window_id.to_ne_bytes());
    bytes[0x20..0x30].fill(0xff);
    for phase in [0x01, 0x02] {
        bytes[0x08] = phase;
        if unsafe { post_event(&psn, bytes.as_ptr()) } != 0 {
            return Err(SpaceError::QueryFailed("SLPSPostEventRecordTo"));
        }
    }
    Ok(())
}

/// Bounded read-only polling for Mission Control, gestures and Desktop edits.
/// It never calls AX or mutates window geometry.
pub struct SpaceObserver {
    pub changes: Receiver<Vec<NativeSpace>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl SpaceObserver {
    pub fn start() -> Self {
        let (sender, changes) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("tylora-space-observer".into())
            .spawn(move || {
                observe_loop(sender, worker_stop);
            })
            .ok();
        Self {
            changes,
            stop,
            worker,
        }
    }
}
impl Drop for SpaceObserver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn observe_loop(sender: SyncSender<Vec<NativeSpace>>, stop: Arc<AtomicBool>) {
    let backend = ExperimentalSkyLightBackend::new();
    let mut last_sent = None;
    while !stop.load(Ordering::Acquire) {
        if let Ok(spaces) = backend.list_spaces() {
            if last_sent.as_ref() != Some(&spaces) && sender.try_send(spaces.clone()).is_ok() {
                last_sent = Some(spaces);
            }
        }
        for _ in 0..10 {
            if stop.load(Ordering::Acquire) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}
pub fn cursor_display() -> Option<DisplayId> {
    let event = OwnedCF(unsafe { CGEventCreate(ptr::null()) });
    if event.0.is_null() {
        return None;
    }
    let point = unsafe { CGEventGetLocation(event.0.cast_mut()) };
    let mut display = 0_u32;
    let mut count = 0_u32;
    let error = unsafe { CGGetDisplaysWithPoint(point, 1, &mut display, &mut count) };
    (error == 0 && count == 1).then_some(DisplayId(display))
}
/// Puts the pointer back where it was, e.g. after switching another display.
pub fn warp_cursor_to(point: (f64, f64)) {
    if unsafe {
        CGWarpMouseCursorPosition(Point {
            x: point.0,
            y: point.1,
        })
    } == 0
    {
        unsafe { CGAssociateMouseAndMouseCursorPosition(1) };
    }
}
/// Moves the pointer to the centre of `display` so Dock gestures target it.
pub fn warp_cursor_to_display(display: DisplayId) -> Result<(), SpaceError> {
    if cursor_display() == Some(display) {
        return Ok(());
    }
    let bounds = unsafe { CGDisplayBounds(display.0) };
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err(SpaceError::Unsupported(format!(
            "display {} has no bounds",
            display.0
        )));
    }
    let centre = Point {
        x: bounds.origin.x + bounds.size.width / 2.0,
        y: bounds.origin.y + bounds.size.height / 2.0,
    };
    if unsafe { CGWarpMouseCursorPosition(centre) } != 0 {
        return Err(SpaceError::Unsupported("pointer warp failed".into()));
    }
    // A warp suspends pointer events briefly unless the association is restored.
    unsafe { CGAssociateMouseAndMouseCursorPosition(1) };
    let deadline = Instant::now() + Duration::from_millis(300);
    while cursor_display() != Some(display) {
        if Instant::now() >= deadline {
            return Err(SpaceError::SwitchNotOnPointerDisplay);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
impl ExperimentalSkyLightBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn gesture_switch_available(&self) -> bool {
        symbols().can_list() && super::super::accessibility_trusted()
    }
}

impl NativeSpaceBackend for ExperimentalSkyLightBackend {
    fn capabilities(&self) -> SpaceCapabilities {
        let syms = symbols();
        let mut reasons = Vec::new();
        if !syms.can_list() {
            reasons.push("SkyLight discovery symbols are missing on this macOS build".into());
        }
        if syms.spaces_for_windows.is_none() {
            reasons.push("SLSCopySpacesForWindows is unavailable".into());
        }
        let move_available = syms.connection.is_some()
            && syms.spaces_for_windows.is_some()
            && super::bridge_status().is_ok()
            && super::super::accessibility_trusted();
        if let Err(error) = super::bridge_status() {
            reasons.push(error.to_string());
        }
        if !super::super::accessibility_trusted() {
            reasons
                .push("Accessibility permission is required for AX-to-CG window identity".into());
        }
        let gesture_available = syms.can_list() && super::super::accessibility_trusted();
        if !gesture_available {
            reasons.push(
                "gesture switching requires SkyLight discovery and Accessibility permission".into(),
            );
        }
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .unwrap_or_else(|| "unknown".into())
            .trim()
            .to_string();
        let sip = std::process::Command::new("csrutil")
            .arg("status")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|value| {
                if value.contains("enabled") {
                    Some(true)
                } else if value.contains("disabled") {
                    Some(false)
                } else {
                    None
                }
            });
        SpaceCapabilities {
            macos_version: version,
            architecture: std::env::consts::ARCH.into(),
            sip_enabled: sip,
            switching_backend: "dock_gesture",
            can_list_native_spaces: syms.can_list(),
            can_observe_active_space: syms.can_list(),
            can_switch_native_space: gesture_available,
            can_move_window_between_spaces: move_available,
            can_determine_window_space: syms.connection.is_some()
                && syms.spaces_for_windows.is_some(),
            private_backend_available: syms.can_list(),
            reasons,
        }
    }

    fn list_spaces(&self) -> Result<Vec<NativeSpace>, SpaceError> {
        let mut result = Vec::new();
        for order in self.display_order()? {
            let desktops = order.spaces.iter().filter(|space| !space.fullscreen);
            for (index, space) in desktops.enumerate() {
                let Ok(desktop) = u8::try_from(index + 1) else {
                    break;
                };
                result.push(NativeSpace {
                    id: space.id,
                    desktop: DesktopNumber(desktop),
                    display: order.display,
                    active: order.active == space.id,
                });
            }
        }
        result.sort_by_key(|space| (space.display.0, space.desktop.0));
        Ok(result)
    }

    fn get_active_space(&self, display: DisplayId) -> Result<NativeSpace, SpaceError> {
        self.list_spaces()?
            .into_iter()
            .find(|space| space.display == display && space.active)
            .ok_or(SpaceError::QueryFailed("active user Desktop"))
    }

    fn get_window_spaces(&self, window_id: u32) -> Result<Vec<NativeSpaceId>, SpaceError> {
        let syms = symbols();
        let (Some(connection), Some(copy)) = (syms.connection, syms.spaces_for_windows) else {
            return Err(SpaceError::Unsupported(
                "window-to-Space query unavailable".into(),
            ));
        };
        if window_id == 0 {
            return Err(SpaceError::NoWindowId);
        }
        let id = unsafe { CFNumberCreate(ptr::null(), 3, (&window_id as *const u32).cast()) };
        let id = OwnedCF(id);
        if id.0.is_null() {
            return Err(SpaceError::QueryFailed("CFNumberCreate"));
        }
        let values = [id.0];
        let windows = OwnedCF(unsafe {
            CFArrayCreate(ptr::null(), values.as_ptr(), 1, &kCFTypeArrayCallBacks)
        });
        if windows.0.is_null() {
            return Err(SpaceError::QueryFailed("CFArrayCreate"));
        }
        let spaces = OwnedCF(unsafe { copy(connection(), 0x7, windows.0) });
        if spaces.0.is_null() {
            return Err(SpaceError::QueryFailed("SLSCopySpacesForWindows"));
        }
        Ok(array_items(spaces.0)
            .filter_map(number)
            .filter(|id| *id > 0)
            .map(|id| NativeSpaceId(id as u64))
            .collect())
    }

    fn focus_space(
        &self,
        display: DisplayId,
        desktop: DesktopNumber,
    ) -> Result<NativeSpace, SpaceError> {
        let spaces = self.list_spaces()?;
        let target = SpaceRegistry::desktop(&spaces, display, desktop)?.clone();
        if target.active {
            return Ok(target);
        }
        NativeSpaceSwitcher::focus_id(display, target.id)?;
        self.get_active_space(display)
    }

    fn move_window_to_space(&self, window_id: u32, space: NativeSpaceId) -> Result<(), SpaceError> {
        super::NativeWindowSpaceMover::move_window_to_space(window_id, space).map(|_| ())
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn GetProcessForPID(pid: i32, psn: *mut ProcessSerialNumber) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: CFRef);
    fn CFStringCreateWithCString(allocator: CFRef, value: *const c_char, encoding: u32) -> CFRef;
    fn CFDictionaryGetValue(dictionary: CFRef, key: CFRef) -> CFRef;
    fn CFArrayGetCount(array: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
    fn CFNumberGetValue(number: CFRef, kind: c_int, value: *mut c_void) -> u8;
    fn CFNumberCreate(allocator: CFRef, kind: c_int, value: *const c_void) -> CFRef;
    fn CFArrayCreate(
        allocator: CFRef,
        values: *const CFRef,
        count: isize,
        callbacks: *const c_void,
    ) -> CFRef;
    static kCFTypeArrayCallBacks: c_void;
    fn CFUUIDCreateFromString(allocator: CFRef, string: CFRef) -> CFRef;
    fn CFUUIDGetUUIDBytes(uuid: CFRef) -> CFUUIDBytes;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayGetDisplayIDFromUUID(uuid: CFRef) -> u32;
    fn CGEventSetIntegerValueField(event: CFRef, field: c_int, value: i64);
    fn CGEventSetDoubleValueField(event: CFRef, field: c_int, value: f64);
    fn CGEventPost(tap: c_int, event: CFRef);
    fn CGEventCreate(source: CFRef) -> CFRef;
    fn CGEventGetLocation(event: *mut c_void) -> Point;
    fn CGGetDisplaysWithPoint(point: Point, max: u32, displays: *mut u32, count: *mut u32)
        -> c_int;
    fn CGDisplayBounds(display: u32) -> Rect;
    fn CGWarpMouseCursorPosition(point: Point) -> c_int;
    fn CGAssociateMouseAndMouseCursorPosition(connected: c_int) -> c_int;
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Size,
}
#[link(name = "System")]
extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn flock(fd: c_int, operation: c_int) -> c_int;
    fn geteuid() -> u32;
}
