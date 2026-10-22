//! In-app updates from GitHub Releases (Tauri updater). The release workflow
//! publishes `latest.json` with a minisign signature of `Tylora.app.tar.gz`;
//! the updater only installs an archive whose signature matches the public key
//! in `tauri.conf.json`. Nothing is downloaded or installed without a click.

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::ipc::{self, BridgeError};

/// The update found by the last check, kept for `install_update`.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<Update>>);

#[derive(Debug, Serialize)]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub available: Option<AvailableUpdate>,
}

#[derive(Debug, Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

fn failure(error: impl std::fmt::Display) -> BridgeError {
    BridgeError::new("update", error.to_string())
}

#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<UpdateCheck, BridgeError> {
    let update = app
        .updater()
        .map_err(failure)?
        .check()
        .await
        .map_err(|error| match error {
            // The latest release has no latest.json (releases before 0.1.2).
            tauri_plugin_updater::Error::ReleaseNotFound => BridgeError::new(
                "update_unpublished",
                "the latest GitHub release has no update information yet",
            ),
            other => failure(other),
        })?;
    let available = update.as_ref().map(|update| AvailableUpdate {
        version: update.version.clone(),
        notes: update.body.clone(),
        date: update.date.map(|date| date.to_string()),
    });
    *pending.0.lock().map_err(failure)? = update;
    Ok(UpdateCheck {
        current: app.package_info().version.to_string(),
        available,
    })
}

/// Downloads, verifies and installs the pending update, restarts a running
/// daemon from the new bundle (it restores hidden windows on the way), then
/// relaunches the app. Progress is emitted as `update://progress`.
#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    pending: tauri::State<'_, PendingUpdate>,
) -> Result<(), BridgeError> {
    let update = pending
        .0
        .lock()
        .map_err(failure)?
        .take()
        .ok_or_else(|| BridgeError::new("update", "no update to install; check again"))?;
    let mut downloaded = 0u64;
    let events = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = events.emit("update://progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(failure)?;
    let restarted = tauri::async_runtime::spawn_blocking(|| {
        if ipc::query(ipc::QueryKind::Status).is_ok() {
            ipc::control(ipc::Control::Restart).map(|_| ())
        } else {
            Ok(())
        }
    })
    .await
    .map_err(failure)?;
    if let Err(error) = restarted {
        // The new app shows the daemon state; the update itself succeeded.
        eprintln!("daemon restart after update failed: {}", error.message);
    }
    app.restart()
}

/// Opens the GitHub release page of `version` (or the release list).
#[tauri::command]
pub fn open_release_notes(version: Option<String>) -> Result<(), BridgeError> {
    const RELEASES: &str = "https://github.com/Rewqaf41/Tylora/releases";
    let url = match version {
        Some(version)
            if !version.is_empty()
                && version.len() <= 32
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) =>
        {
            format!("{RELEASES}/tag/v{version}")
        }
        Some(_) => return Err(BridgeError::new("invalid_request", "invalid version")),
        None => RELEASES.to_owned(),
    };
    std::process::Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map_err(failure)?;
    Ok(())
}
