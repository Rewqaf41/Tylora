use serde::{Deserialize, Serialize};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, process::Command};
use tauri::tray::TrayIcon;

use crate::ipc::BridgeError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiPreferences {
    pub version: u32,
    pub launch_at_login: bool,
    pub start_daemon_automatically: bool,
    pub open_settings_at_launch: bool,
    pub show_menu_bar_icon: bool,
}
impl Default for GuiPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            launch_at_login: false,
            start_daemon_automatically: false,
            open_settings_at_launch: true,
            show_menu_bar_icon: true,
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
fn agent_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join("Library/LaunchAgents/com.tylora.settings.plist"))
}
fn legacy_prefs_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join(".config/fluxwm/gui.json"))
}
fn legacy_agent_path() -> Result<PathBuf, BridgeError> {
    Ok(home()?.join("Library/LaunchAgents/com.fluxwm.settings.plist"))
}
fn daemon_config_path() -> Result<PathBuf, BridgeError> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or(home()?.join(".config"));
    Ok(base.join("tylora/config.toml"))
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
    prefs.launch_at_login = agent_path()?.exists() || legacy_agent_path()?.exists();
    Ok(prefs)
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

fn xml_escape(source: &str) -> String {
    source
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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
    if prefs.launch_at_login {
        let binary = std::env::current_exe().map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?;
        let path = agent_path()?;
        let plist = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>com.tylora.settings</string><key>ProgramArguments</key><array><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><false/></dict></plist>\n", xml_escape(&binary.to_string_lossy()));
        write_private(&path, plist.as_bytes())?;
    } else {
        match fs::remove_file(agent_path()?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(BridgeError {
                    code: "io",
                    message: error.to_string(),
                })
            }
        }
    }
    write_private(
        &prefs_path()?,
        &serde_json::to_vec_pretty(&prefs).map_err(|error| BridgeError {
            code: "io",
            message: error.to_string(),
        })?,
    )?;
    match fs::remove_file(legacy_agent_path()?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(BridgeError {
                code: "io",
                message: error.to_string(),
            })
        }
    }
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
