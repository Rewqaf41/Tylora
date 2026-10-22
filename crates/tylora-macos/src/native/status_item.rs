//! The daemon's menu bar item (`status_item.m`): a monochrome Tylora icon,
//! the workspace numbers and a menu. Main thread only; menu choices arrive on
//! the channel given to [`StatusItem::new`].

use std::{
    ffi::{c_char, c_void, CString},
    sync::mpsc::SyncSender,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    OpenApp,
    Reconcile,
    Stop,
    FocusWorkspace(u8),
    /// "Show Workspace Numbers" was chosen; flips `[menu_bar] show_workspaces`.
    ToggleWorkspaces,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusWorkspace {
    pub id: u8,
    /// Menu label, for example "1  Code".
    pub label: String,
    pub active: bool,
    pub occupied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub enabled: bool,
    pub show_workspaces: bool,
    /// First, disabled menu line.
    pub headline: String,
    pub workspaces: Vec<StatusWorkspace>,
}

type Callback = extern "C" fn(*mut c_void, i32, i32);

extern "C" {
    fn tylora_status_create(callback: Callback, context: *mut c_void) -> *mut c_void;
    fn tylora_status_update(
        handle: *mut c_void,
        headline: *const c_char,
        show_digits: i32,
        ids: *const i32,
        labels: *const *const c_char,
        flags: *const u8,
        count: i32,
    );
    fn tylora_status_destroy(handle: *mut c_void);
    fn tylora_pump_events(seconds: f64);
}

extern "C" fn on_menu(context: *mut c_void, action: i32, argument: i32) {
    let sender = unsafe { &*(context as *const SyncSender<MenuAction>) };
    let action = match action {
        1 => MenuAction::OpenApp,
        2 => MenuAction::Reconcile,
        3 => MenuAction::Stop,
        4 => match u8::try_from(argument) {
            Ok(id) => MenuAction::FocusWorkspace(id),
            Err(_) => return,
        },
        5 => MenuAction::ToggleWorkspaces,
        _ => return,
    };
    let _ = sender.try_send(action);
}

pub struct StatusItem {
    handle: *mut c_void,
    sender: *mut SyncSender<MenuAction>,
}

impl StatusItem {
    /// Adds the item to the menu bar; `None` off the main thread.
    pub fn new(actions: SyncSender<MenuAction>) -> Option<Self> {
        let sender = Box::into_raw(Box::new(actions));
        let handle = unsafe { tylora_status_create(on_menu, sender.cast()) };
        if handle.is_null() {
            drop(unsafe { Box::from_raw(sender) });
            return None;
        }
        Some(Self { handle, sender })
    }

    pub fn update(&self, snapshot: &StatusSnapshot) {
        let clean = |text: &str| CString::new(text.replace('\0', "")).unwrap_or_default();
        let headline = clean(&snapshot.headline);
        let labels: Vec<CString> = snapshot
            .workspaces
            .iter()
            .map(|workspace| clean(&workspace.label))
            .collect();
        let label_ptrs: Vec<*const c_char> = labels.iter().map(|label| label.as_ptr()).collect();
        let ids: Vec<i32> = snapshot
            .workspaces
            .iter()
            .map(|workspace| i32::from(workspace.id))
            .collect();
        let flags: Vec<u8> = snapshot
            .workspaces
            .iter()
            .map(|workspace| u8::from(workspace.active) | (u8::from(workspace.occupied) << 1))
            .collect();
        unsafe {
            tylora_status_update(
                self.handle,
                headline.as_ptr(),
                i32::from(snapshot.show_workspaces),
                ids.as_ptr(),
                label_ptrs.as_ptr(),
                flags.as_ptr(),
                ids.len() as i32,
            )
        }
    }
}

impl Drop for StatusItem {
    fn drop(&mut self) {
        unsafe {
            tylora_status_destroy(self.handle);
            drop(Box::from_raw(self.sender));
        }
    }
}

/// Dispatches AppKit events (menu clicks, screen changes) on the main thread,
/// waiting up to `timeout` for the first one.
pub fn pump_events(timeout: Duration) {
    unsafe { tylora_pump_events(timeout.as_secs_f64()) }
}
