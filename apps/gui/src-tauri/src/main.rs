mod ipc;
mod preferences;
mod system;
mod updater;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

fn show_settings(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        // The daemon keeps this window floating; it always opens centred.
        let _ = window.center();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// The daemon draws its own menu bar item while it runs (unless turned off
/// in `[menu_bar]`); the app's icon then stays hidden so there is only one.
fn daemon_shows_menu_bar(status: &serde_json::Value) -> bool {
    status
        .pointer("/menu_bar/enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Whether the app keeps its own menu bar icon right now.
fn app_tray_wanted() -> bool {
    preferences::read().is_ok_and(|prefs| prefs.show_menu_bar_icon)
        && !ipc::query(ipc::QueryKind::Status).is_ok_and(|status| daemon_shows_menu_bar(&status))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::PendingUpdate::default())
        .setup(|app| {
            if let Err(error) = preferences::migrate_login_agent() {
                eprintln!("login item migration failed: {}", error.message);
            }
            let preferences = preferences::read().unwrap_or_default();
            let status = MenuItemBuilder::with_id("status", "Tylora — checking…")
                .enabled(false)
                .build(app)?;
            let workspace = MenuItemBuilder::with_id("workspace", "Workspace —")
                .enabled(false)
                .build(app)?;
            let open = MenuItemBuilder::with_id("open", "Open Tylora…").build(app)?;
            let start = MenuItemBuilder::with_id("start", "Start Tylora").build(app)?;
            let reconcile =
                MenuItemBuilder::with_id("reconcile", "Reconcile Windows").build(app)?;
            let quit =
                MenuItemBuilder::with_id("quit", "Quit App (Tiling Keeps Running)").build(app)?;
            let stop = MenuItemBuilder::with_id("stop", "Stop Tylora").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&status, &workspace, &open, &start, &reconcile, &stop, &quit])
                .build()?;
            let tray = TrayIconBuilder::new()
                .icon(tauri::image::Image::from_bytes(include_bytes!(
                    "../icons/tray-template.png"
                ))?)
                .icon_as_template(true)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_settings(app),
                    "start" => {
                        std::thread::spawn(|| {
                            let _ = ipc::control(ipc::Control::Start);
                        });
                    }
                    "reconcile" => {
                        std::thread::spawn(|| {
                            let _ = ipc::send("reconcile");
                        });
                    }
                    "stop" => {
                        std::thread::spawn(|| {
                            let _ = ipc::send("stop");
                        });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_settings(tray.app_handle());
                    }
                })
                .build(app)?;
            tray.set_visible(preferences.show_menu_bar_icon)?;
            let tray_updates = tray.clone();
            app.manage(tray);
            if !preferences.open_settings_at_launch && preferences.onboarding_completed {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            if preferences.start_daemon_automatically {
                std::thread::spawn(|| {
                    let _ = ipc::control(ipc::Control::Start);
                });
            }

            std::thread::spawn(move || loop {
                let status_data = ipc::query(ipc::QueryKind::Status);
                let show = preferences::read().is_ok_and(|prefs| prefs.show_menu_bar_icon)
                    && !status_data.as_ref().is_ok_and(daemon_shows_menu_bar);
                let _ = tray_updates.set_visible(show);
                if let Ok(data) = status_data {
                    let _ = status.set_text("Tylora — Running");
                    let label = data
                        .get("active_workspaces")
                        .and_then(|entries| entries.as_array())
                        .and_then(|entries| entries.first())
                        .and_then(|entry| entry.get("id"))
                        .and_then(|id| id.as_u64())
                        .map(|id| format!("Workspace {id}"))
                        .unwrap_or_else(|| "Workspace —".into());
                    let _ = workspace.set_text(label);
                } else {
                    let _ = status.set_text("Tylora — Stopped");
                    let _ = workspace.set_text("Workspace —");
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hidden only while its own menu bar icon can bring it back;
                // otherwise closing quits the app (the daemon keeps running
                // and its menu bar item reopens the app).
                if app_tray_wanted() {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    window.app_handle().exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            ipc::daemon_query,
            ipc::daemon_mutate,
            ipc::daemon_control,
            ipc::layout_preview,
            preferences::gui_preferences,
            preferences::save_gui_preferences,
            preferences::config_location,
            preferences::open_config_file,
            preferences::macos_version,
            preferences::export_diagnostics,
            system::app_info,
            system::request_accessibility,
            system::open_system_pane,
            updater::check_update,
            updater::install_update,
            updater::open_release_notes
        ])
        .build(tauri::generate_context!())
        .expect("Tylora settings failed to start")
        .run(|app, event| {
            // Reopening the running app (Dock, Finder, or the daemon's
            // "Open Tylora…") shows the window again.
            if let tauri::RunEvent::Reopen { .. } = event {
                show_settings(app);
            }
        });
}
