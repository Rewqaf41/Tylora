mod ipc;
mod preferences;

use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

fn show_settings(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let preferences = preferences::read().unwrap_or_default();
            let status = MenuItemBuilder::with_id("status", "Tylora — checking…")
                .enabled(false)
                .build(app)?;
            let workspace = MenuItemBuilder::with_id("workspace", "Workspace —")
                .enabled(false)
                .build(app)?;
            let open = MenuItemBuilder::with_id("open", "Open Settings").build(app)?;
            let reconcile =
                MenuItemBuilder::with_id("reconcile", "Reconcile Windows").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit Settings").build(app)?;
            let stop = MenuItemBuilder::with_id("stop", "Stop Tylora").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&status, &workspace, &open, &reconcile, &quit, &stop])
                .build()?;
            let tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().expect("bundle icon").clone())
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_settings(app),
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
            app.manage(tray);
            if !preferences.open_settings_at_launch {
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
                if let Ok(data) = ipc::query(ipc::QueryKind::Status) {
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
                std::thread::sleep(std::time::Duration::from_secs(10));
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if preferences::read().is_ok_and(|prefs| prefs.show_menu_bar_icon) {
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
            preferences::export_diagnostics
        ])
        .run(tauri::generate_context!())
        .expect("Tylora settings failed to start");
}
