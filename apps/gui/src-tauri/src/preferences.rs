use serde::{Deserialize, Serialize};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, process::Command};
use tauri::tray::TrayIcon;

use crate::ipc::BridgeError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiPreferences {
    pub version: u32,
    /// Mirrors the SMAppService login item; never read from the file.
    pub launch_at_login: bool,
    pub start_daemon_automatically: bool,
    pub open_settings_at_launch: bool,
    pub show_menu_bar_icon: bool,
    /// First-run onboarding was finished or skipped. Preference files written
    /// before onboarding existed belong to configured installs, so it defaults
    /// to done there; a fresh install has no file and starts with it.
    #[serde(default = "onboarded_before")]
    pub onboarding_completed: bool,
    /// Look for a newer release on GitHub when the app opens.
    #[serde(default = "check_by_default")]
    pub check_for_updates: bool,
    /// App version the user last saw; a different running version means the
    /// app was updated and the "updated" notice is shown once.
    #[serde(default)]
    pub last_seen_version: Option<String>,
}
fn check_by_default() -> bool {
    true
}
fn onboarded_before() -> bool {
    true
}
impl Default for GuiPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            launch_at_login: false,
            start_daemon_automatically: false,
            open_settings_at_launch: true,
            show_menu_bar_icon: true,
            onboarding_completed: false,
            check_for_updates: true,
            last_seen_version: None,
        }
    }
}

fn home() -> Result<PathBuf, BridgeError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| BridgeError {
            code: "io",
            message: "HOME is unavailable".into(),
        })
}
fn prefs_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join(".config/tylora/gui.json"))
}
/// LaunchAgent written by earlier Tylora builds; replaced by SMAppService.
fn agent_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join("Library/LaunchAgents/com.tylora.settings.plist"))
}
fn legacy_prefs_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join(".config/fluxwm/gui.json"))
}
fn legacy_agent_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join("Library/LaunchAgents/com.fluxwm.settings.plist"))
}
/// Same resolver (and FluxWM migration) as the daemon, so both use one file.
fn daemon_config_path() -> Result<PathBuf, BridgeError> {
    Ok(tylora_config::paths::resolve_config().0)
}

pub fn read() -> Result<GuiPreferences, BridgeError> {
    let path = prefs_path()?;
    if !path.exists() {
        if let Ok(bytes) = fs::read(legacy_prefs_path()?) {
            write_private(&path, &bytes)?;
        }
    }
    let mut prefs = match fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<GuiPreferences>(&bytes).map_err(|error| BridgeError {
                code: "io",
                message: error.to_string(),
            })?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => GuiPreferences::default(),
        Err(error) => {
            return Err(BridgeError {
                code: "io",
                message: error.to_string(),
            })
        }
    };
    prefs.launch_at_login = matches!(
        crate::system::login_item_status(),
        crate::system::LoginItemStatus::Enabled | crate::system::LoginItemStatus::RequiresApproval
    ) || agent_path()?.exists()
        || legacy_agent_path()?.exists();
    Ok(prefs)
}

/// Removes a LaunchAgent plist written by an earlier build and unloads its
/// job from this session (both are the user's own; no privileges needed).
fn remove_agent(path: &PathBuf, label: &str) -> Result<bool, BridgeError> {
    match fs::remove_file(path) {
        Ok(()) => {
            let uid = unsafe { libc::geteuid() };
            let _ = Command::new("/bin/launchctl")
                .args(["bootout", &format!("gui/{uid}/{label}")])
                .output();
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(BridgeError {
            code: "io",
            message: error.to_string(),
        }),
    }
}

/// Moves a login LaunchAgent of an earlier build (Tylora or FluxWM) to an
/// SMAppService login item. The old plist is removed only once the new item
/// is registered, so login keeps working if registration fails.
pub fn migrate_login_agent() -> Result<(), BridgeError> {
    let tylora = agent_path()?;
    let fluxwm = legacy_agent_path()?;
    if !tylora.exists() && !fluxwm.exists() {
        return Ok(());
    }
    crate::system::set_login_item(true)?;
    remove_agent(&tylora, "com.tylora.settings")?;
    remove_agent(&fluxwm, "com.fluxwm.settings")?;
    Ok(())
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<(), BridgeError> {
    let parent = path.parent().ok_or_else(|| BridgeError {
        code: "io",
        message: "path has no parent".into(),
    })?;
    fs::create_dir_all(parent).map_err(|error| BridgeError {
        code: "io",
        message: error.to_string(),
    })?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(|error| BridgeError {
        code: "io",
        message: error.to_string(),
    })
}

pub fn save(prefs: GuiPreferences, tray: &TrayIcon) -> Result<GuiPreferences, BridgeError> {
    if prefs.version != 1 {
        return Err(BridgeError {
            code: "validation",
            message: "unsupported GUI preference version".into(),
        });
    }
    if !prefs.open_settings_at_launch && !prefs.show_menu_bar_icon {
        return Err(BridgeError {
            code: "validation",
            message: "keep the settings window or menu bar icon visible at launch".into(),
        });
    }
    let mut prefs = prefs;
    let login = crate::system::set_login_item(prefs.launch_at_login)?;
    if !prefs.launch_at_login {
        remove_agent(&agent_path()?, "com.tylora.settings")?;
        remove_agent(&legacy_agent_path()?, "com.fluxwm.settings")?;
    }
    prefs.launch_at_login = matches!(
        login,
        crate::system::LoginItemStatus::Enabled | crate::system::LoginItemStatus::RequiresApproval
    );
    write_private(
        &prefs_path()?,
        &serde_json::to_vec_pretty(&prefs).map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?,
    )?;
    tray.set_visible(prefs.show_menu_bar_icon)
        .map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?;
    Ok(prefs)
}

#[tauri::command]
pub fn gui_preferences() -> Result<GuiPreferences, BridgeError> {
    read()
}

#[tauri::command]
pub fn save_gui_preferences(
    prefs: GuiPreferences,
    tray: tauri::State<'_, TrayIcon>,
) -> Result<GuiPreferences, BridgeError> {
    save(prefs, &tray)
}

#[tauri::command]
pub fn config_location() -> Result<String, BridgeError> {
    Ok(daemon_config_path()?.display().to_string())
}

#[tauri::command]
pub fn open_config_file() -> Result<(), BridgeError> {
    let path = daemon_config_path()?;
    if !path.is_file() {
        return Err(BridgeError {
            code: "missing",
            message: "config file does not exist yet".into(),
        });
    }
    Command::new("/usr/bin/open")
        .arg(path)
        .spawn()
        .map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?;
    Ok(())
}

#[tauri::command]
pub fn macos_version() -> Result<String, BridgeError> {
    let output = Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(BridgeError {
            code: "io",
            message: "macOS version is unavailable".into(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[tauri::command]
pub fn export_diagnostics() -> Result<String, BridgeError> {
    let status = crate::ipc::query(crate::ipc::QueryKind::Status)?;
    let diagnostics = crate::ipc::query(crate::ipc::QueryKind::Diagnostics)?;
    let capabilities = crate::ipc::query(crate::ipc::QueryKind::Capabilities)?;
    let hotkeys = crate::ipc::query(crate::ipc::QueryKind::Hotkeys)?;
    let bindings: Vec<_> = hotkeys.get("hotkeys").and_then(|value| value.as_array()).into_iter().flatten()
        .map(|hotkey| serde_json::json!({
            "chord":hotkey.get("chord"), "enabled":hotkey.get("enabled"), "active":hotkey.get("active"),
            "action":hotkey.pointer("/command/command"),
        })).collect();
    let report = serde_json::json!({
        "version":1,"macos_version":macos_version().ok(),"status":{
            "state":status.get("state"),"displays":status.get("displays"),"windows":status.get("windows"),
            "tiled":status.get("tiled"),"floating":status.get("floating"),"degraded":status.get("degraded"),
        },"diagnostics":diagnostics,"capabilities":capabilities,"hotkeys":bindings,
    });
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?
        .as_millis();
    let path = home()?.join(format!("Downloads/Tylora-diagnostics-{timestamp}.json"));
    write_private(
        &path,
        serde_json::to_string_pretty(&report)
            .map_err(|error| BridgeError {
                code: "io",
                message: error.to_string(),
            })?
            .as_bytes(),
    )?;
    Ok(path.display().to_string())
}
