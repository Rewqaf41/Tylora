//! A session event tap on a dedicated run loop. The callback only enqueues commands.

use std::{
    collections::HashSet,
    ffi::c_void,
    ptr,
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Instant,
};
use tracing::{info, warn};
use tylora_config::{CompiledHotkey, Mouse};
use tylora_core::{commands::StateCommand, registry::ManagedWindowId, Rect};

type CFRef = *const c_void;
type CGEventRef = *mut c_void;
type PointerSample = ((f64, f64), Instant);
type PointerSlot = Arc<Mutex<Option<PointerSample>>>;
struct PointerInput {
    slot: PointerSlot,
    enabled: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MousePhase {
    Start,
    Update,
    End,
    Cancel,
}
#[derive(Debug, Clone, Copy)]
pub struct MouseInput {
    pub id: ManagedWindowId,
    pub button: MouseButton,
    pub phase: MousePhase,
    pub start: (f64, f64),
    pub current: (f64, f64),
    pub floating: bool,
    pub at: Instant,
}
#[derive(Debug, Clone, Copy)]
pub struct MouseTarget {
    pub id: ManagedWindowId,
    pub frame: Rect,
    pub floating: bool,
    pub enabled: bool,
}

#[derive(Debug)]
pub enum HotkeyEvent {
    Command {
        command: StateCommand,
        pressed_at: Instant,
    },
    MouseStart(MouseInput),
    PointerButton,
    TapDisabled,
    QueueOverflow,
    Stopped,
    /// Option alone went down (`true`), or that ended: Option released,
    /// another modifier joined, or a key or mouse button was pressed.
    OptionHold(bool),
}

pub struct HotkeyManager {
    pub events: Receiver<HotkeyEvent>,
    targets: SyncSender<Vec<MouseTarget>>,
    latest_mouse: Arc<Mutex<Option<MouseInput>>>,
    latest_pointer: PointerSlot,
    stop: SyncSender<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HotkeyManager {
    pub fn start(bindings: Vec<CompiledHotkey>, mouse: Mouse, follow_mouse: bool) -> Self {
        let (events_tx, events) = mpsc::sync_channel(128);
        let (stop, stop_rx) = mpsc::sync_channel(1);
        let (targets, target_rx) = mpsc::sync_channel(1);
        let latest_mouse = Arc::new(Mutex::new(None));
        let callback_mouse = latest_mouse.clone();
        let latest_pointer = Arc::new(Mutex::new(None));
        let callback_pointer = latest_pointer.clone();
        let thread = thread::Builder::new()
            .name("tylora-input".into())
            .spawn(move || {
                run(
                    bindings,
                    mouse,
                    events_tx,
                    stop_rx,
                    target_rx,
                    callback_mouse,
                    PointerInput {
                        slot: callback_pointer,
                        enabled: follow_mouse,
                    },
                );
            })
            .ok();
        Self {
            events,
            targets,
            latest_mouse,
            latest_pointer,
            stop,
            thread,
        }
    }
    pub fn update_targets(&self, targets: Vec<MouseTarget>) {
        let _ = self.targets.try_send(targets);
    }
    pub fn take_mouse(&self) -> Option<MouseInput> {
        self.latest_mouse.lock().ok()?.take()
    }
    pub fn take_pointer(&self) -> Option<PointerSample> {
        self.latest_pointer.lock().ok()?.take()
    }
    pub fn stop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for HotkeyManager {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Context {
    bindings: Vec<CompiledHotkey>,
    sender: SyncSender<HotkeyEvent>,
    consumed: HashSet<u16>,
    mouse_modifier: u64,
    mouse_enabled: bool,
    drag_threshold: f64,
    targets: Vec<MouseTarget>,
    drag: Option<MouseInput>,
    latest_mouse: Arc<Mutex<Option<MouseInput>>>,
    latest_pointer: PointerSlot,
    follow_mouse: bool,
    pointer_button_down: bool,
    option_hold: bool,
}

/// Reports Option-alone transitions; the receiver times the hold.
fn set_option_hold(state: &mut Context, held: bool) {
    if state.option_hold != held {
        state.option_hold = held;
        let _ = state.sender.try_send(HotkeyEvent::OptionHold(held));
    }
}

fn modifier_flag(name: &str) -> u64 {
    match name {
        "shift" => 0x0002_0000,
        "control" => 0x0004_0000,
        "command" => 0x0010_0000,
        _ => 0x0008_0000,
    }
}
fn contains(frame: Rect, point: (f64, f64)) -> bool {
    point.0 >= frame.x
        && point.0 < frame.x + frame.width
        && point.1 >= frame.y
        && point.1 < frame.y + frame.height
}
fn target_at(targets: &[MouseTarget], point: (f64, f64)) -> Option<MouseTarget> {
    let candidates: Vec<_> = targets
        .iter()
        .filter(|target| target.enabled && contains(target.frame, point))
        .copied()
        .collect();
    let floating: Vec<_> = candidates
        .iter()
        .filter(|target| target.floating)
        .copied()
        .collect();
    match floating.len() {
        1 => Some(floating[0]),
        0 if candidates.len() == 1 => Some(candidates[0]),
        _ => None,
    }
}
fn update_mouse(state: &Context, value: MouseInput) {
    if let Ok(mut slot) = state.latest_mouse.lock() {
        *slot = Some(value);
    }
}
fn mouse_callback(state: &mut Context, kind: u32, event: CGEventRef) -> CGEventRef {
    if !state.mouse_enabled {
        return event;
    }
    let point = unsafe { CGEventGetLocation(event) };
    let point = (point.x, point.y);
    let button = match kind {
        1 | 2 | 6 => MouseButton::Left,
        3 | 4 | 7 => MouseButton::Right,
        _ => return event,
    };
    if kind == 1 || kind == 3 {
        if state.drag.is_some() || state.latest_mouse.lock().is_ok_and(|slot| slot.is_some()) {
            return event;
        }
        if unsafe { CGEventGetFlags(event) } & state.mouse_modifier == 0 {
            return event;
        }
        let Some(target) = target_at(&state.targets, point) else {
            return event;
        };
        // A single floating window sits over its underlying tile. If multiple
        // floating windows overlap, their z-order is unknown: pass input on.
        let drag = MouseInput {
            id: target.id,
            button,
            phase: MousePhase::Start,
            start: point,
            current: point,
            floating: target.floating,
            at: Instant::now(),
        };
        if state
            .sender
            .try_send(HotkeyEvent::MouseStart(drag))
            .is_err()
        {
            return event;
        }
        state.drag = Some(drag);
        return ptr::null_mut();
    }
    let Some(mut drag) = state.drag else {
        return event;
    };
    if drag.button != button {
        return event;
    }
    drag.current = point;
    drag.at = Instant::now();
    if kind == 2 || kind == 4 {
        drag.phase = MousePhase::End;
        update_mouse(state, drag);
        state.drag = None;
        return ptr::null_mut();
    }
    let dx = point.0 - drag.start.0;
    let dy = point.1 - drag.start.1;
    if dx * dx + dy * dy >= state.drag_threshold * state.drag_threshold {
        drag.phase = MousePhase::Update;
        update_mouse(state, drag);
    }
    ptr::null_mut()
}

extern "C" fn callback(
    _proxy: *mut c_void,
    kind: u32,
    event: CGEventRef,
    user: *mut c_void,
) -> CGEventRef {
    if user.is_null() {
        return event;
    }
    // SAFETY: the run-loop thread owns Context and keeps it alive through tap teardown.
    let state = unsafe { &mut *(user as *mut Context) };
    if kind == 0xffff_fffe || kind == 0xffff_ffff {
        set_option_hold(state, false);
        let _ = state.sender.try_send(HotkeyEvent::TapDisabled);
        state.consumed.clear();
        state.pointer_button_down = false;
        if let Some(mut drag) = state.drag.take() {
            drag.phase = MousePhase::Cancel;
            update_mouse(state, drag);
        }
        return event;
    }
    if event.is_null() || unsafe { IsSecureEventInputEnabled() } != 0 {
        set_option_hold(state, false);
        state.consumed.clear();
        state.pointer_button_down = false;
        if let Some(mut drag) = state.drag.take() {
            drag.phase = MousePhase::Cancel;
            update_mouse(state, drag);
        }
        return event;
    }
    // Ignore Tylora-tagged synthetic events if another backend posts them.
    if unsafe { CGEventGetIntegerValueField(event, 42) } == 0x464c_5558_5350_4143 {
        return event;
    }
    if kind == 5 {
        if state.follow_mouse && state.drag.is_none() && !state.pointer_button_down {
            let point = unsafe { CGEventGetLocation(event) };
            if let Ok(mut slot) = state.latest_pointer.lock() {
                *slot = Some(((point.x, point.y), Instant::now()));
            }
        }
        return event;
    }
    if matches!(kind, 1 | 2 | 3 | 4 | 6 | 7) {
        if matches!(kind, 1 | 3) {
            set_option_hold(state, false);
            state.pointer_button_down = true;
            if state.follow_mouse {
                let _ = state.sender.try_send(HotkeyEvent::PointerButton);
            }
        } else if matches!(kind, 2 | 4) {
            state.pointer_button_down = false;
        }
        return mouse_callback(state, kind, event);
    }
    if kind == 12 {
        // Shift, Control, Option, Command; Caps Lock and Fn do not count.
        let modifiers = unsafe { CGEventGetFlags(event) } & 0x001e_0000;
        set_option_hold(state, modifiers == 0x0008_0000);
        if state.drag.is_some() && unsafe { CGEventGetFlags(event) } & state.mouse_modifier == 0 {
            if let Some(mut drag) = state.drag.take() {
                drag.phase = MousePhase::Cancel;
                update_mouse(state, drag);
            }
        }
        return event;
    }
    let keycode = unsafe { CGEventGetIntegerValueField(event, 9) } as u16;
    if kind == 11 {
        return if state.consumed.remove(&keycode) {
            ptr::null_mut()
        } else {
            event
        };
    }
    if kind != 10 {
        return event;
    }
    set_option_hold(state, false);
    let mask = 0x001e_0000;
    let modifiers = unsafe { CGEventGetFlags(event) } & mask;
    let Some(binding) = state
        .bindings
        .iter()
        .find(|entry| entry.keycode == keycode && entry.modifiers == modifiers)
    else {
        return event;
    };
    if unsafe { CGEventGetIntegerValueField(event, 8) } != 0 {
        return ptr::null_mut();
    }
    match state.sender.try_send(HotkeyEvent::Command {
        command: binding.command.clone(),
        pressed_at: Instant::now(),
    }) {
        Ok(()) => {
            state.consumed.insert(keycode);
            ptr::null_mut()
        }
        Err(TrySendError::Full(_)) => {
            let _ = state.sender.try_send(HotkeyEvent::QueueOverflow);
            event
        }
        Err(TrySendError::Disconnected(_)) => event,
    }
}

fn run(
    bindings: Vec<CompiledHotkey>,
    mouse: Mouse,
    sender: SyncSender<HotkeyEvent>,
    stop: Receiver<()>,
    targets: Receiver<Vec<MouseTarget>>,
    latest_mouse: Arc<Mutex<Option<MouseInput>>>,
    pointer: PointerInput,
) {
    let mut context = Box::new(Context {
        bindings,
        sender: sender.clone(),
        consumed: HashSet::new(),
        mouse_modifier: modifier_flag(&mouse.modifier),
        mouse_enabled: mouse.enabled,
        drag_threshold: mouse.drag_threshold,
        targets: Vec::new(),
        drag: None,
        latest_mouse,
        latest_pointer: pointer.slot,
        follow_mouse: pointer.enabled,
        pointer_button_down: false,
        option_hold: false,
    });
    // kCGSessionEventTap=1, kCGHeadInsertEventTap=0, kCGEventTapOptionDefault=0.
    let mask = (1u64 << 10)
        | (1u64 << 11)
        | if context.follow_mouse { 1u64 << 5 } else { 0 }
        | [1, 2, 3, 4, 6, 7, 12]
            .iter()
            .fold(0u64, |acc, kind| acc | (1u64 << kind));
    let tap = unsafe {
        CGEventTapCreate(
            1,
            0,
            0,
            mask,
            callback,
            (&mut *context as *mut Context).cast(),
        )
    };
    if tap.is_null() {
        warn!(
            "global hotkey event tap unavailable; check Input Monitoring/Accessibility permission"
        );
        let _ = sender.try_send(HotkeyEvent::Stopped);
        return;
    }
    let source = unsafe { CFMachPortCreateRunLoopSource(ptr::null(), tap, 0) };
    if source.is_null() {
        unsafe { CFRelease(tap) };
        let _ = sender.try_send(HotkeyEvent::Stopped);
        return;
    }
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    unsafe {
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        CGEventTapEnable(tap, 1);
    }
    info!(bindings = context.bindings.len(), "global hotkeys active");
    while stop.try_recv().is_err() {
        while let Ok(new_targets) = targets.try_recv() {
            context.targets = new_targets;
        }
        unsafe {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, 0);
        }
        if unsafe { CGEventTapIsEnabled(tap) } == 0 {
            context.consumed.clear();
            unsafe { CGEventTapEnable(tap, 1) };
        }
    }
    unsafe {
        CGEventTapEnable(tap, 0);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopDefaultMode);
        CFRelease(source);
        CFRelease(tap);
    }
    let _ = sender.try_send(HotkeyEvent::Stopped);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        events: u64,
        callback: extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef,
        user: *mut c_void,
    ) -> CFRef;
    fn CGEventTapEnable(tap: CFRef, enable: u8);
    fn CGEventTapIsEnabled(tap: CFRef) -> u8;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFMachPortCreateRunLoopSource(allocator: CFRef, port: CFRef, order: isize) -> CFRef;
    fn CFRunLoopGetCurrent() -> CFRef;
    fn CFRunLoopAddSource(run_loop: CFRef, source: CFRef, mode: CFRef);
    fn CFRunLoopRemoveSource(run_loop: CFRef, source: CFRef, mode: CFRef);
    fn CFRunLoopRunInMode(mode: CFRef, seconds: f64, return_after_source_handled: u8) -> i32;
    fn CFRelease(value: CFRef);
    static kCFRunLoopDefaultMode: CFRef;
}
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tylora_core::registry::ProcessInstance;

    fn target(token: u64, floating: bool) -> MouseTarget {
        MouseTarget {
            id: ManagedWindowId {
                process: ProcessInstance { pid: 1, epoch: 1 },
                token,
            },
            frame: Rect::new(0.0, 0.0, 100.0, 100.0).unwrap(),
            floating,
            enabled: true,
        }
    }

    #[test]
    fn floating_window_receives_drag_over_tiled_window() {
        let targets = [target(1, false), target(2, true)];
        assert_eq!(target_at(&targets, (50.0, 50.0)).unwrap().id.token, 2);
        assert!(target_at(&[targets[1], target(3, true)], (50.0, 50.0)).is_none());
    }
}
