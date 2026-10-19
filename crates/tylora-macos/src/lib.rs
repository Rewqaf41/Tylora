//! Read-only macOS discovery. All foreign pointers and unsafe calls live in `native`.

use std::{error::Error, fmt, sync::mpsc};
use tylora_core::{Rect, WindowState};

pub mod spaces;

#[derive(Debug, Clone)]
pub struct AxWindowGeometry {
    pub owner_pid: i32,
    pub frame: Rect,
}

#[derive(Debug, Clone)]
pub struct AxIssue {
    pub owner_pid: i32,
    pub operation: &'static str,
    pub code: i32,
}

impl AxIssue {
    pub fn meaning(&self) -> &'static str {
        match self.code {
            -25201 => "illegal argument",
            -25202 => "invalid UI element",
            -25204 => "application busy or unresponsive",
            -25205 => "attribute unsupported",
            -25208 => "accessibility not implemented by application",
            -25211 => "accessibility API disabled",
            -25212 => "no value",
            -1 => "missing or invalid value",
            _ => "other AX error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoverySnapshot {
    /// On-screen, layer-zero Core Graphics windows. CGWindowID is the identity.
    pub windows: Vec<WindowState>,
    /// AX geometries are deliberately unpaired: public AX has no CGWindowID accessor.
    pub ax_windows: Vec<AxWindowGeometry>,
    pub ax_issues: Vec<AxIssue>,
    pub accessibility_trusted: bool,
}

#[derive(Debug)]
pub enum DiscoveryError {
    UnsupportedPlatform,
    WindowServerUnavailable,
    WorkerStopped,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => write!(f, "window discovery requires macOS"),
            Self::WindowServerUnavailable => write!(f, "WindowServer window list is unavailable"),
            Self::WorkerStopped => write!(f, "macOS discovery worker stopped before responding"),
        }
    }
}

impl Error for DiscoveryError {}

/// Spawns a worker so a future coordinator need never wait on AX calls.
pub fn discover_windows_async() -> mpsc::Receiver<Result<DiscoverySnapshot, DiscoveryError>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(discover_windows());
    });
    rx
}

pub fn discover_windows() -> Result<DiscoverySnapshot, DiscoveryError> {
    #[cfg(target_os = "macos")]
    {
        native::discover()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(DiscoveryError::UnsupportedPlatform)
    }
}

/// Public CG snapshot of ordinary windows across visible and inactive Spaces.
/// It does not by itself establish their native Space membership.
pub fn discover_all_cg_windows() -> Result<Vec<WindowState>, DiscoveryError> {
    #[cfg(target_os = "macos")]
    {
        native::cg_windows_with_option(0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(DiscoveryError::UnsupportedPlatform)
    }
}

/// Front-to-back WindowServer hit test of visible windows. Returns the first
/// covering CG window, including unmanaged overlays so they block focus.
pub fn topmost_window_at(point: (f64, f64)) -> Option<(u32, i32)> {
    #[cfg(target_os = "macos")]
    {
        native::topmost_window_at(point)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = point;
        None
    }
}

/// PID of the application that owns keyboard focus.
///
/// The system-wide AX element is asked first. It can get stuck after the
/// focused application quits (it then fails with -25204 for every query, or
/// names the dead process), so a dead or missing answer falls back to the
/// owner of the frontmost ordinary window in WindowServer order. NSWorkspace
/// is not used: off the main run loop its frontmost application never updates.
pub fn frontmost_application_pid() -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        native::frontmost_application_pid()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Adds `count` Desktops to `display` by pressing Mission Control's
/// add-desktop button through Accessibility (briefly showing Mission
/// Control). Returns the number of presses; verify through WindowServer.
pub fn add_desktops(display: tylora_core::DisplayId, count: u8) -> Result<u8, String> {
    #[cfg(target_os = "macos")]
    {
        native::mission_control::add_desktops(display, count).map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (display, count);
        Err("macOS only".into())
    }
}

pub fn current_pointer() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        native::current_pointer()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        native::trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Requests macOS to inform the user when this process lacks Accessibility
/// access. The prompt is asynchronous; the return value is current trust.
pub fn prompt_for_accessibility() -> bool {
    #[cfg(target_os = "macos")]
    {
        native::prompt_for_trust()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[cfg(target_os = "macos")]
pub use native::input::{
    HotkeyEvent, HotkeyManager, MouseButton, MouseInput, MousePhase, MouseTarget,
};
#[cfg(target_os = "macos")]
pub use native::managed::{
    suppress_pointer_focus, work_areas, DisplayEvents, ManagedAdapter, ManagedCommand,
    ManagedEvent, WorkArea, VISIBILITY_ROLLBACK_FAILED,
};

#[cfg(target_os = "macos")]
mod native {
    pub mod input;
    pub mod managed;
    use super::*;
    use std::{
        collections::BTreeSet,
        ffi::{c_char, c_void, CStr},
        ptr,
    };
    use tylora_core::{DisplayId, WindowId};

    type CFRef = *const c_void;
    type AXRef = *const c_void;

    pub(super) fn trusted() -> bool {
        (unsafe { AXIsProcessTrusted() }) != 0
    }

    pub(super) fn prompt_for_trust() -> bool {
        // SAFETY: The key and value are process-wide CoreFoundation constants.
        // The dictionary only borrows them during the synchronous API call.
        let key = unsafe { kAXTrustedCheckOptionPrompt };
        let value = unsafe { kCFBooleanTrue };
        let options =
            unsafe { CFDictionaryCreate(ptr::null(), &key, &value, 1, ptr::null(), ptr::null()) };
        if options.is_null() {
            return trusted();
        }
        let trusted = unsafe { AXIsProcessTrustedWithOptions(options) } != 0;
        unsafe { CFRelease(options) };
        trusted
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(value: CFRef);
        fn CFDictionaryCreate(
            allocator: CFRef,
            keys: *const CFRef,
            values: *const CFRef,
            count: isize,
            key_callbacks: CFRef,
            value_callbacks: CFRef,
        ) -> CFRef;
        static kCFBooleanTrue: CFRef;
        fn CFGetTypeID(value: CFRef) -> usize;
        fn CFArrayGetTypeID() -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFArrayGetCount(array: CFRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFRef, index: isize) -> CFRef;
        fn CFDictionaryGetValue(dict: CFRef, key: CFRef) -> CFRef;
        fn CFNumberGetValue(number: CFRef, number_type: i32, value: *mut c_void) -> u8;
        fn CFStringCreateWithCString(allocator: CFRef, text: *const c_char, encoding: u32)
            -> CFRef;
        fn CFStringGetCString(string: CFRef, buffer: *mut c_char, size: isize, encoding: u32)
            -> u8;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> CFRef;
        fn CGEventCreate(source: CFRef) -> CFRef;
        fn CGEventGetLocation(event: *mut c_void) -> CGPoint;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFRef, rect: *mut CGRect) -> u8;
        fn CGGetDisplaysWithRect(
            rect: CGRect,
            max: u32,
            displays: *mut u32,
            count: *mut u32,
        ) -> i32;
        static kCGWindowNumber: CFRef;
        static kCGWindowOwnerPID: CFRef;
        static kCGWindowOwnerName: CFRef;
        static kCGWindowLayer: CFRef;
        static kCGWindowBounds: CFRef;
    }
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXIsProcessTrustedWithOptions(options: CFRef) -> u8;
        static kAXTrustedCheckOptionPrompt: CFRef;
        fn AXUIElementCreateApplication(pid: i32) -> AXRef;
        fn AXUIElementCreateSystemWide() -> AXRef;
        fn AXUIElementGetPid(element: AXRef, pid: *mut i32) -> i32;
        fn AXUIElementPerformAction(element: AXRef, action: CFRef) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXRef, timeout: f32) -> i32;
        fn AXUIElementCopyAttributeValue(
            element: AXRef,
            attribute: CFRef,
            value: *mut CFRef,
        ) -> i32;
        fn AXValueGetValue(value: CFRef, kind: u32, output: *mut c_void) -> u8;
    }

    struct OwnedCF(CFRef);
    impl Drop for OwnedCF {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) }
            }
        }
    }

    fn cf_string(value: &'static CStr) -> OwnedCF {
        // SAFETY: NUL-terminated static string and valid UTF-8 encoding constant.
        OwnedCF(unsafe { CFStringCreateWithCString(ptr::null(), value.as_ptr(), 0x0800_0100) })
    }

    fn number(dict: CFRef, key: CFRef) -> Option<i32> {
        // SAFETY: Core Graphics supplies a CFDictionary and numeric values for these keys.
        let value = unsafe { CFDictionaryGetValue(dict, key) };
        if value.is_null() {
            return None;
        }
        let mut result = 0i32;
        // kCFNumberSInt32Type = 3.
        (unsafe { CFNumberGetValue(value, 3, (&mut result as *mut i32).cast()) } != 0)
            .then_some(result)
    }

    fn string(dict: CFRef, key: CFRef) -> Option<String> {
        // SAFETY: `key` refers to a Core Graphics CFString field.
        let value = unsafe { CFDictionaryGetValue(dict, key) };
        if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
            return None;
        }
        let mut buffer = vec![0u8; 4096];
        // SAFETY: buffer has the stated capacity and CFStringGetCString NUL terminates on success.
        if unsafe {
            CFStringGetCString(
                value,
                buffer.as_mut_ptr().cast(),
                buffer.len() as isize,
                0x0800_0100,
            )
        } == 0
        {
            return None;
        }
        Some(
            unsafe { CStr::from_ptr(buffer.as_ptr().cast()) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    fn rect(dict: CFRef) -> Option<Rect> {
        let mut native = CGRect::default();
        // SAFETY: Core Graphics documents this dictionary representation for kCGWindowBounds.
        if unsafe { CGRectMakeWithDictionaryRepresentation(dict, &mut native) } == 0 {
            return None;
        }
        Rect::new(
            native.origin.x,
            native.origin.y,
            native.size.width,
            native.size.height,
        )
        .ok()
    }

    fn display_for(frame: Rect) -> Option<DisplayId> {
        let native = CGRect {
            origin: CGPoint {
                x: frame.x,
                y: frame.y,
            },
            size: CGSize {
                width: frame.width,
                height: frame.height,
            },
        };
        let mut count = 0u32;
        // Query the full count first; a one-element buffer would hide spanning windows.
        let status = unsafe { CGGetDisplaysWithRect(native, 0, ptr::null_mut(), &mut count) };
        if status != 0 || count != 1 {
            return None;
        }
        let mut id = 0u32;
        let status = unsafe { CGGetDisplaysWithRect(native, 1, &mut id, &mut count) };
        (status == 0 && count == 1).then_some(DisplayId(id))
    }

    fn ax_attribute(element: AXRef, name: &'static CStr) -> Result<OwnedCF, i32> {
        let key = cf_string(name);
        if key.0.is_null() {
            return Err(-1);
        }
        let mut value = ptr::null();
        // SAFETY: AX element is retained by caller; API returns a retained value on success.
        let code = unsafe { AXUIElementCopyAttributeValue(element, key.0, &mut value) };
        if code != 0 {
            return Err(code);
        }
        if value.is_null() {
            return Err(-1);
        }
        Ok(OwnedCF(value))
    }

    fn ax_rect(element: AXRef) -> Result<Rect, (&'static str, i32)> {
        let position = ax_attribute(element, c"AXPosition").map_err(|e| ("AXPosition", e))?;
        let size = ax_attribute(element, c"AXSize").map_err(|e| ("AXSize", e))?;
        let mut origin = CGPoint::default();
        let mut dimensions = CGSize::default();
        // AXValueType: CGPoint=1, CGSize=2. Check return values before using output.
        let ok_position =
            unsafe { AXValueGetValue(position.0, 1, (&mut origin as *mut CGPoint).cast()) };
        let ok_size =
            unsafe { AXValueGetValue(size.0, 2, (&mut dimensions as *mut CGSize).cast()) };
        if ok_position == 0 || ok_size == 0 {
            return Err(("AXValueGetValue", -1));
        }
        Rect::new(origin.x, origin.y, dimensions.width, dimensions.height)
            .map_err(|_| ("AX geometry", -1))
    }

    pub(super) fn cg_windows_with_option(option: u32) -> Result<Vec<WindowState>, DiscoveryError> {
        let list = OwnedCF(unsafe { CGWindowListCopyWindowInfo(option, 0) });
        if list.0.is_null() {
            return Err(DiscoveryError::WindowServerUnavailable);
        }
        let mut windows = Vec::new();
        let count = unsafe { CFArrayGetCount(list.0) };
        for index in 0..count {
            let item = unsafe { CFArrayGetValueAtIndex(list.0, index) };
            if item.is_null() || number(item, unsafe { kCGWindowLayer }) != Some(0) {
                continue;
            }
            let (Some(id), Some(pid)) = (
                number(item, unsafe { kCGWindowNumber }),
                number(item, unsafe { kCGWindowOwnerPID }),
            ) else {
                continue;
            };
            if id <= 0 || pid <= 0 {
                continue;
            }
            let bounds = unsafe { CFDictionaryGetValue(item, kCGWindowBounds) };
            let Some(frame) = (!bounds.is_null()).then(|| rect(bounds)).flatten() else {
                continue;
            };
            if frame.area() <= 0.0 {
                continue;
            }
            windows.push(WindowState {
                id: WindowId(id as u32),
                owner_pid: pid,
                owner_name: string(item, unsafe { kCGWindowOwnerName }),
                display_id: display_for(frame),
                frame,
            });
        }

        Ok(windows)
    }

    pub(super) fn cg_windows() -> Result<Vec<WindowState>, DiscoveryError> {
        // kCGWindowListOptionOnScreenOnly = 1, kCGNullWindowID = 0.
        cg_windows_with_option(1)
    }

    pub(super) fn topmost_window_at(point: (f64, f64)) -> Option<(u32, i32)> {
        // OnScreenOnly | ExcludeDesktopElements. CG returns front-to-back order.
        let list = OwnedCF(unsafe { CGWindowListCopyWindowInfo(1 | 16, 0) });
        if list.0.is_null() {
            return None;
        }
        let count = unsafe { CFArrayGetCount(list.0) };
        for index in 0..count {
            let item = unsafe { CFArrayGetValueAtIndex(list.0, index) };
            if item.is_null() {
                continue;
            }
            let bounds = unsafe { CFDictionaryGetValue(item, kCGWindowBounds) };
            let Some(frame) = (!bounds.is_null()).then(|| rect(bounds)).flatten() else {
                continue;
            };
            if point.0 < frame.x
                || point.0 >= frame.x + frame.width
                || point.1 < frame.y
                || point.1 >= frame.y + frame.height
            {
                continue;
            }
            // Do not see through a system panel or another application's window.
            if number(item, unsafe { kCGWindowLayer }) != Some(0) {
                return None;
            }
            let id = number(item, unsafe { kCGWindowNumber })?;
            let pid = number(item, unsafe { kCGWindowOwnerPID })?;
            return (id > 0 && pid > 0).then_some((id as u32, pid));
        }
        None
    }

    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    pub(super) fn frontmost_application_pid() -> Option<i32> {
        let alive = |pid: i32| pid > 0 && unsafe { kill(pid, 0) } == 0;
        let system = OwnedCF(unsafe { AXUIElementCreateSystemWide() });
        if !system.0.is_null() {
            if let Ok(app) = ax_attribute(system.0, c"AXFocusedApplication") {
                let mut pid = 0;
                if unsafe { AXUIElementGetPid(app.0, &mut pid) } == 0 && alive(pid) {
                    return Some(pid);
                }
            }
        }
        frontmost_window_pid().filter(|pid| alive(*pid))
    }

    /// Owner of the first on-screen layer-0 window, front to back.
    fn frontmost_window_pid() -> Option<i32> {
        let list = OwnedCF(unsafe { CGWindowListCopyWindowInfo(1 | 16, 0) });
        if list.0.is_null() {
            return None;
        }
        let own = std::process::id() as i32;
        let count = unsafe { CFArrayGetCount(list.0) };
        (0..count).find_map(|index| {
            let item = unsafe { CFArrayGetValueAtIndex(list.0, index) };
            if item.is_null() || number(item, unsafe { kCGWindowLayer }) != Some(0) {
                return None;
            }
            number(item, unsafe { kCGWindowOwnerPID }).filter(|pid| *pid > 0 && *pid != own)
        })
    }

    pub(super) fn current_pointer() -> Option<(f64, f64)> {
        let event = OwnedCF(unsafe { CGEventCreate(ptr::null()) });
        if event.0.is_null() {
            return None;
        }
        let point = unsafe { CGEventGetLocation(event.0.cast_mut()) };
        Some((point.x, point.y))
    }

    pub(super) fn discover() -> Result<DiscoverySnapshot, DiscoveryError> {
        let windows = cg_windows()?;
        let pids: BTreeSet<_> = windows.iter().map(|w| w.owner_pid).collect();
        let trusted = unsafe { AXIsProcessTrusted() } != 0;
        let mut snapshot = DiscoverySnapshot {
            windows,
            ax_windows: Vec::new(),
            ax_issues: Vec::new(),
            accessibility_trusted: trusted,
        };
        if !trusted {
            return Ok(snapshot);
        }
        for pid in pids {
            let app = OwnedCF(unsafe { AXUIElementCreateApplication(pid) });
            if app.0.is_null() {
                snapshot.ax_issues.push(AxIssue {
                    owner_pid: pid,
                    operation: "AXUIElementCreateApplication",
                    code: -1,
                });
                continue;
            }
            // Bound the synchronous system call on the dedicated discovery worker.
            let timeout_code = unsafe { AXUIElementSetMessagingTimeout(app.0, 0.25) };
            if timeout_code != 0 {
                snapshot.ax_issues.push(AxIssue {
                    owner_pid: pid,
                    operation: "AXUIElementSetMessagingTimeout",
                    code: timeout_code,
                });
            }
            let list = match ax_attribute(app.0, c"AXWindows") {
                Ok(value) => value,
                Err(code) => {
                    snapshot.ax_issues.push(AxIssue {
                        owner_pid: pid,
                        operation: "AXWindows",
                        code,
                    });
                    continue;
                }
            };
            if unsafe { CFGetTypeID(list.0) != CFArrayGetTypeID() } {
                snapshot.ax_issues.push(AxIssue {
                    owner_pid: pid,
                    operation: "AXWindows type",
                    code: -1,
                });
                continue;
            }
            let count = unsafe { CFArrayGetCount(list.0) };
            for index in 0..count {
                let element = unsafe { CFArrayGetValueAtIndex(list.0, index) };
                if element.is_null() {
                    continue;
                }
                let timeout_code = unsafe { AXUIElementSetMessagingTimeout(element, 0.25) };
                if timeout_code != 0 {
                    snapshot.ax_issues.push(AxIssue {
                        owner_pid: pid,
                        operation: "AX window timeout",
                        code: timeout_code,
                    });
                    continue;
                }
                match ax_rect(element) {
                    Ok(frame) => snapshot.ax_windows.push(AxWindowGeometry {
                        owner_pid: pid,
                        frame,
                    }),
                    Err((operation, code)) => snapshot.ax_issues.push(AxIssue {
                        owner_pid: pid,
                        operation,
                        code,
                    }),
                }
            }
        }
        Ok(snapshot)
    }
}
