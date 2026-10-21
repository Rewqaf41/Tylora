//! macOS integration of the Settings app: the login item (SMAppService),
//! the Accessibility trust check and the Privacy & Security panes. Nothing
//! here changes a permission; macOS asks the user, and only the user grants.

use serde::Serialize;
use serde_json::Value;
use std::{ffi::CStr, os::raw::c_char, process::Command};

use objc2::{
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject},
};

use crate::ipc::BridgeError;

#[link(name = "ServiceManagement", kind = "framework")]
extern "C" {}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const std::ffi::c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: *const std::ffi::c_void;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFDictionaryCreate(
        allocator: *const std::ffi::c_void,
        keys: *const *const std::ffi::c_void,
        values: *const *const std::ffi::c_void,
        count: isize,
        key_callbacks: *const u8,
        value_callbacks: *const u8,
    ) -> *const std::ffi::c_void;
    fn CFRelease(object: *const std::ffi::c_void);
}

/// `SMAppService.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginItemStatus {
    NotRegistered,
    Enabled,
    /// Registered, but the user must allow it in System Settings → General →
    /// Login Items before it runs.
    RequiresApproval,
    NotFound,
    Unavailable,
}

fn main_app_service() -> Option<Retained<AnyObject>> {
    let class = AnyClass::get(c"SMAppService")?;
    unsafe { msg_send![class, mainAppService] }
}

pub fn login_item_status() -> LoginItemStatus {
    let Some(service) = main_app_service() else {
        return LoginItemStatus::Unavailable;
    };
    let status: isize = unsafe { msg_send![&*service, status] };
    match status {
        0 => LoginItemStatus::NotRegistered,
        1 => LoginItemStatus::Enabled,
        2 => LoginItemStatus::RequiresApproval,
        3 => LoginItemStatus::NotFound,
        _ => LoginItemStatus::Unavailable,
    }
}

fn error_text(error: *mut AnyObject) -> String {
    if error.is_null() {
        return "unknown error".into();
    }
    unsafe {
        let description: Option<Retained<AnyObject>> = msg_send![&*error, localizedDescription];
        description
            .map(|text| {
                let utf8: *const c_char = msg_send![&*text, UTF8String];
                if utf8.is_null() {
                    "unknown error".into()
                } else {
                    CStr::from_ptr(utf8).to_string_lossy().into_owned()
                }
            })
            .unwrap_or_else(|| "unknown error".into())
    }
}

/// Registers or unregisters the app itself as a login item. The item is tied
/// to the bundle identifier, so replacing Tylora.app with a newer copy keeps it.
pub fn set_login_item(enabled: bool) -> Result<LoginItemStatus, BridgeError> {
    let service = main_app_service().ok_or_else(|| {
        BridgeError::new("unsupported", "SMAppService is unavailable on this macOS")
    })?;
    let current = login_item_status();
    let registered = matches!(
        current,
        LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval
    );
    if enabled == registered {
        return Ok(current);
    }
    let mut error: *mut AnyObject = std::ptr::null_mut();
    let ok: bool = unsafe {
        if enabled {
            msg_send![&*service, registerAndReturnError: &mut error]
        } else {
            msg_send![&*service, unregisterAndReturnError: &mut error]
        }
    };
    if !ok {
        return Err(BridgeError::new(
            "login_item",
            format!(
                "could not {} the login item: {}",
                if enabled { "register" } else { "remove" },
                error_text(error)
            ),
        ));
    }
    Ok(login_item_status())
}

pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Shows the system Accessibility prompt (only if not yet trusted). The
/// daemon launched from this app inherits the app as its responsible process,
/// so the grant to "Tylora" covers it.
fn prompt_accessibility() -> bool {
    unsafe {
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub bundled_daemon: Option<String>,
    pub accessibility_trusted: bool,
    pub login_item: LoginItemStatus,
    /// Version reported by the running daemon (absent when stopped or older).
    pub daemon_version: Option<String>,
    pub daemon_executable: Option<String>,
}

#[tauri::command]
pub async fn app_info() -> Result<AppInfo, BridgeError> {
    tauri::async_runtime::spawn_blocking(|| {
        let status = crate::ipc::query(crate::ipc::QueryKind::Status).ok();
        let text = |key: &str| {
            status
                .as_ref()
                .and_then(|value| value.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        AppInfo {
            version: env!("CARGO_PKG_VERSION"),
            bundled_daemon: crate::ipc::daemon_binary()
                .ok()
                .map(|path| path.display().to_string()),
            accessibility_trusted: accessibility_trusted(),
            login_item: login_item_status(),
            daemon_version: text("daemon_version"),
            daemon_executable: text("executable"),
        }
    })
    .await
    .map_err(|error| BridgeError::new("internal", error.to_string()))
}

#[tauri::command]
pub fn request_accessibility() -> bool {
    prompt_accessibility()
}

/// Opens a Privacy & Security pane, or the Login Items list, for the user.
#[tauri::command]
pub fn open_system_pane(pane: String) -> Result<(), BridgeError> {
    let url = match pane.as_str() {
        "accessibility" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        "input_monitoring" => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        "login_items" => "x-apple.systempreferences:com.apple.LoginItems-Settings.extension",
        _ => return Err(BridgeError::new("invalid_request", "unknown settings pane")),
    };
    Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map_err(|error| BridgeError::new("io", error.to_string()))?;
    Ok(())
}
