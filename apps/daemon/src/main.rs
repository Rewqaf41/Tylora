use std::{env, process::ExitCode};
use tracing_subscriber::EnvFilter;
use tylora_core::commands::{Direction, StateCommand};
use tylora_macos::{discover_windows_async, DiscoveryError};

#[cfg(target_os = "macos")]
use tylora_macos::spaces::{
    bridge_status, cursor_display, focused_cg_window, last_move_diagnostic, record_move_diagnostic,
    DesktopNumber, ExperimentalSkyLightBackend, NativeSpaceBackend, NativeSpaceId,
    NativeSpaceSwitcher, NativeWindowSpaceMover, SpaceRegistry,
};

#[cfg(target_os = "macos")]
mod config_manager;
mod focus_follows_mouse;
#[cfg(target_os = "macos")]
mod runtime;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = env::args().skip(1).collect();
    #[cfg(target_os = "macos")]
    if args.first().is_some_and(|word| word == "space") {
        return native_space_command(&args[1..]);
    }
    #[cfg(target_os = "macos")]
    if args.first().is_some_and(|word| word == "workspace") {
        return native_space_command(&args[1..]);
    }
    if let Some(command) = parse_state_command(&args) {
        #[cfg(target_os = "macos")]
        return runtime::request(&format!(
            "v1 {}",
            serde_json::to_string(&command).expect("typed command")
        ));
        #[cfg(not(target_os = "macos"))]
        return usage();
    }
    match args.as_slice() {
        [command] if command == "list-windows" => list_windows(false),
        [command, flag] if command == "list-windows" && flag == "--ax" => list_windows(true),
        #[cfg(target_os = "macos")]
        [command] if command == "start" => runtime::start(None),
        #[cfg(target_os = "macos")]
        [command, flag, id] if command == "start" && flag == "--display" => {
            match id.parse::<u32>() {
                Ok(id) => runtime::start(Some(id)),
                Err(_) => usage(),
            }
        }
        #[cfg(target_os = "macos")]
        [command] if command == "serve" => runtime::serve(None),
        #[cfg(target_os = "macos")]
        [command, flag, id] if command == "serve" && flag == "--display" => {
            match id.parse::<u32>() {
                Ok(id) => runtime::serve(Some(id)),
                Err(_) => usage(),
            }
        }
        #[cfg(target_os = "macos")]
        [command] if command == "stop" => runtime::stop(),
        #[cfg(target_os = "macos")]
        [command]
            if matches!(
                command.as_str(),
                "status" | "windows" | "layout" | "reconcile" | "recover-windows"
            ) =>
        {
            runtime::request(command)
        }
        #[cfg(target_os = "macos")]
        [group, action, rest @ ..]
            if group == "rules"
                && matches!(
                    action.as_str(),
                    "list" | "validate" | "test" | "apply" | "reload" | "diagnostics"
                ) =>
        {
            let _ = rest;
            runtime::request(&args.join(" "))
        }
        #[cfg(target_os = "macos")]
        [group, action] if group == "hotkeys" && matches!(action.as_str(), "pause" | "resume") => {
            runtime::request(&args.join(" "))
        }
        #[cfg(target_os = "macos")]
        [group, action, ..]
            if group == "scratchpad"
                && matches!(
                    action.as_str(),
                    "list"
                        | "assign"
                        | "unassign"
                        | "toggle"
                        | "show"
                        | "hide"
                        | "recover"
                        | "capabilities"
                ) =>
        {
            runtime::request(&args.join(" "))
        }
        #[cfg(target_os = "macos")]
        [group, action] | [group, action, _]
            if group == "apps" && action == "list" && (args.len() == 2 || args[2] == "--json") =>
        {
            runtime::request(&args.join(" "))
        }
        #[cfg(target_os = "macos")]
        [group, action] | [group, action, _]
            if group == "layout"
                && matches!(action.as_str(), "list" | "current")
                && (args.len() == 2 || args[2] == "--json") =>
        {
            runtime::request(&args.join(" "))
        }
        #[cfg(target_os = "macos")]
        [group, action] if group == "hotkeys" && action == "list" => {
            runtime::request("hotkeys list")
        }
        #[cfg(target_os = "macos")]
        [group, action] if group == "display" && action == "list" => {
            runtime::request("display list")
        }
        #[cfg(target_os = "macos")]
        [command, flag] if command == "status" && flag == "--json" => {
            runtime::request("status --json")
        }
        #[cfg(target_os = "macos")]
        [group, action, flag] if group == "display" && action == "list" && flag == "--json" => {
            runtime::request("display list --json")
        }
        #[cfg(target_os = "macos")]
        [group, action, flag] if group == "hotkeys" && action == "list" && flag == "--json" => {
            runtime::request("hotkeys list --json")
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: tylora start [--display ID] | stop | recover-windows | status [--json] | windows | layout | reconcile | space list|current|capabilities|diagnostics|move-diagnostics|reconcile [--json] | space focus WORKSPACE [--json] | space focus-id NATIVE_ID [--json] | space move-window WORKSPACE [--window-id CG_ID] [--json] | space windows DESKTOP_NUMBER [--json] | display list [--json] | display focus next|DIR | window move-to-display next|previous|ID [--json] | hotkeys list [--json] | hotkeys pause|resume | list-windows [--ax] | focus DIR|next|previous | move DIR | resize DIR --amount N | floating toggle | zoom toggle | layout list|current [--json] | rules list|validate|reload|diagnostics [--json] | rules test|apply --window CG_ID [--json] | apps list [--json] | scratchpad list|capabilities|recover [--json] | scratchpad assign ID [--window CG_ID] [--force] [--json] | scratchpad unassign|toggle|show|hide ID [--json] | layout set bsp|dwindle|scrolling [--json] | layout cycle [--json] | scroll left|right | center | column cycle-width | column width DELTA");
    ExitCode::from(2)
}

#[cfg(target_os = "macos")]
fn native_space_command(args: &[String]) -> ExitCode {
    let json = args.last().is_some_and(|arg| arg == "--json");
    let args = if json { &args[..args.len() - 1] } else { args };
    let backend = ExperimentalSkyLightBackend::new();
    let result: Result<serde_json::Value, String> = (|| {
        let spaces = backend.list_spaces();
        match args {
            [action] if action == "capabilities" => Ok(serde_json::to_value(backend.capabilities()).unwrap_or_default()),
            [action] if action == "list" => Ok(serde_json::json!({"spaces":spaces.map_err(|e| e.to_string())?})),
            [action] if action == "current" => Ok(serde_json::json!({"spaces":spaces.map_err(|e| e.to_string())?.into_iter().filter(|space| space.active).collect::<Vec<_>>()})),
            [action, number] if action == "windows" => {
                let desktop = number.parse::<u8>().map_err(|_| "invalid Desktop number".to_string())?;
                let spaces = spaces.map_err(|e| e.to_string())?;
                let display = cursor_display().or_else(|| spaces.iter().find(|space| space.active).map(|space| space.display)).ok_or("no current display")?;
                let target = SpaceRegistry::desktop(&spaces, display, DesktopNumber(desktop)).map_err(|e| e.to_string())?;
                let windows = tylora_macos::discover_all_cg_windows().map_err(|e| e.to_string())?
                    .into_iter().map(|window| {
                        let member = backend.get_window_spaces(window.id.0).map_err(|e| e.to_string())?.contains(&target.id);
                        Ok::<_, String>(member.then(|| serde_json::json!({"cg_window_id":window.id.0,"pid":window.owner_pid,"owner":window.owner_name,"frame":window.frame})))
                    })
                    .collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect::<Vec<_>>();
                Ok(serde_json::json!({"space":target,"windows":windows}))
            }
            [action, selector] if action == "focus" || action == "focus-id" => {
                let spaces = spaces.map_err(|e| e.to_string())?;
                let target_id = if action == "focus-id" {
                    NativeSpaceId(selector.parse::<u64>().map_err(|_| "invalid native Space ID".to_string())?)
                } else {
                    let config = if runtime::config_path().exists() { tylora_config::Config::load(&runtime::config_path()).map_err(|e| e.to_string())? } else { tylora_config::Config::default() };
                    let workspace = config.workspace(selector).ok_or_else(|| format!("unknown workspace: {selector}"))?;
                    // Diagnostic path: the first bound Space that still exists.
                    NativeSpaceId(workspace.space_ids().into_iter().find(|id| spaces.iter().any(|space| space.id.0 == *id)).ok_or_else(|| format!("workspace {selector} is unresolved; set native_space_id in config.toml from `tylora space list`"))?)
                };
                let target = spaces.iter().find(|space| space.id == target_id).ok_or_else(|| format!("native Space {} no longer exists; workspace binding is unresolved", target_id.0))?;
                let outcome = NativeSpaceSwitcher::focus_id(target.display, target_id).map_err(|e| e.to_string())?;
                Ok(serde_json::json!({"workspace":selector,"switch":outcome,"space":backend.get_active_space(target.display).map_err(|e| e.to_string())?}))
            }
            [action] if action == "diagnostics" => {
                let spaces = spaces.map_err(|e| e.to_string())?;
                let config = if runtime::config_path().exists() { tylora_config::Config::load(&runtime::config_path()).map_err(|e| e.to_string())? } else { tylora_config::Config::default() };
                let bindings = config.workspaces.iter().map(|workspace| serde_json::json!({"id":workspace.id,"name":workspace.name,"native_space_id":workspace.native_space_id,"native_space_ids":workspace.native_space_ids,"resolved":workspace.space_ids().into_iter().filter(|id| spaces.iter().any(|space| space.id.0 == *id)).collect::<Vec<_>>()})).collect::<Vec<_>>();
                let current: Vec<_> = spaces.iter().filter(|space| space.active).collect();
                Ok(serde_json::json!({"backend":"dock_gesture","capabilities":backend.capabilities(),"pointer_display":cursor_display(),"current":current,"spaces":spaces,"bindings":bindings}))
            }
            [action, selector] | [action, selector, _, _] if action == "move-window" => {
                let spaces = spaces.map_err(|e| e.to_string())?;
                let config = if runtime::config_path().exists() { tylora_config::Config::load(&runtime::config_path()).map_err(|e| e.to_string())? } else { tylora_config::Config::default() };
                let workspace = config.workspace(selector).ok_or_else(|| format!("unknown workspace: {selector}"))?;
                let native = workspace.space_ids().into_iter().find(|id| spaces.iter().any(|space| space.id.0 == *id)).ok_or_else(|| format!("workspace {selector} is unresolved; set native_space_id"))?;
                let target = spaces.iter().find(|space| space.id.0 == native).ok_or_else(|| format!("native Space {native} no longer exists; workspace binding is unresolved"))?;
                let focused = if let [_, _, flag, id] = args {
                    if flag != "--window-id" { return Err("expected --window-id CG_ID".into()); }
                    let id = id.parse::<u32>().map_err(|_| "invalid CGWindowID".to_string())?;
                    let pid = tylora_macos::discover_all_cg_windows().ok().and_then(|windows| windows.into_iter().find(|window| window.id.0 == id).map(|window| window.owner_pid));
                    (id, pid)
                } else {
                    let window = focused_cg_window().map_err(|e| e.to_string())?;
                    (window.window_id, Some(window.pid))
                };
                let started = std::time::Instant::now();
                let outcome = match NativeWindowSpaceMover::move_window_to_space(focused.0, target.id) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        record_move_diagnostic(&serde_json::json!({"window_id":focused.0,"pid":focused.1,"target_space_id":target.id,"backend":"SkyLight bridged move","stage":"failed_or_indeterminate","elapsed_ms":started.elapsed().as_millis(),"verified":false,"error":error.to_string()}));
                        return Err(error.to_string());
                    }
                };
                record_move_diagnostic(&serde_json::json!({"window_id":outcome.window_id,"pid":focused.1,"source_space_id":outcome.source,"target_space_id":outcome.target,"backend":outcome.backend,"stage":outcome.stage,"operation_ms":outcome.operation_ms,"verification_ms":outcome.verification_ms,"verified":outcome.confirmed,"no_op":outcome.no_op,"error":null}));
                let _ = runtime::query("reconcile");
                Ok(serde_json::json!({"workspace":selector,"pid":focused.1,"move":outcome}))
            }
            [action] if action == "move-diagnostics" => {
                let capability = bridge_status().map(|_| "available".to_string()).unwrap_or_else(|e| e.to_string());
                let focused = focused_cg_window().ok();
                let source = focused.and_then(|window| NativeWindowSpaceMover::can_move_window(window.window_id).ok());
                Ok(serde_json::json!({"backend":"SkyLight bridged move","capability":capability,"window_id":focused.map(|window| window.window_id),"pid":focused.map(|window| window.pid),"source_space_id":source,"stage":"preflight","last_operation":last_move_diagnostic()}))
            }
            [action] if action == "reconcile" => {
                Ok(serde_json::json!({"message":"native Space discovery refreshed","spaces":spaces.map_err(|e| e.to_string())?}))
            }
            _ => Err("usage: tylora space list|current|capabilities|diagnostics|reconcile [--json] | focus WORKSPACE | focus-id NATIVE_ID | move-window WORKSPACE | windows DESKTOP_NUMBER [--json]".into()),
        }
    })();
    match result {
        Ok(value) => {
            if json {
                println!("{value}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            }
            ExitCode::SUCCESS
        }
        Err(reason) => {
            if json {
                println!("{}", serde_json::json!({"ok":false,"error":reason}));
            } else {
                eprintln!("{reason}");
            }
            ExitCode::FAILURE
        }
    }
}

fn direction(value: &str) -> Option<Direction> {
    match value {
        "left" => Some(Direction::Left),
        "right" => Some(Direction::Right),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}

fn parse_state_command(args: &[String]) -> Option<StateCommand> {
    // A trailing --json only selects output; typed commands always reply JSON.
    let args = match args {
        [rest @ .., flag] if flag == "--json" && !rest.is_empty() => rest,
        _ => args,
    };
    match args {
        [group, action, kind] if group == "layout" && action == "set" => {
            Some(StateCommand::SetLayout {
                layout: tylora_core::layout::LayoutKind::parse(kind)?,
            })
        }
        [group, action] if group == "layout" && action == "cycle" => {
            Some(StateCommand::CycleLayout)
        }
        [verb, target] if verb == "focus" && target == "next" => {
            Some(StateCommand::FocusNextWindow)
        }
        [verb, target] if verb == "focus" && target == "previous" => {
            Some(StateCommand::FocusPreviousWindow)
        }
        [verb, dir] if verb == "scroll" && dir == "left" => Some(StateCommand::ScrollLayoutLeft),
        [verb, dir] if verb == "scroll" && dir == "right" => Some(StateCommand::ScrollLayoutRight),
        [verb] if verb == "center" => Some(StateCommand::CenterFocusedWindow),
        [group, action] if group == "column" && action == "cycle-width" => {
            Some(StateCommand::ResizeScrollingColumn { delta: None })
        }
        [group, action, delta] if group == "column" && action == "width" => {
            let delta: f64 = delta.parse().ok()?;
            delta
                .is_finite()
                .then_some(StateCommand::ResizeScrollingColumn { delta: Some(delta) })
        }
        [verb, dir] if verb == "focus" => Some(StateCommand::FocusWindow {
            direction: direction(dir)?,
        }),
        [verb, dir] if verb == "move" => Some(StateCommand::MoveWindow {
            direction: direction(dir)?,
        }),
        [verb, dir, flag, amount] if verb == "resize" && flag == "--amount" => {
            Some(StateCommand::ResizeWindow {
                direction: direction(dir)?,
                amount: amount.parse().ok()?,
            })
        }
        [verb, action] if verb == "floating" && action == "toggle" => {
            Some(StateCommand::ToggleFloating { window: None })
        }
        [verb, action] if verb == "zoom" && action == "toggle" => {
            Some(StateCommand::ToggleLayoutZoom { window: None })
        }
        [group, action, target] | [group, action, target, _]
            if group == "window"
                && action == "move-to-display"
                && (args.len() == 3 || args[3] == "--json") =>
        {
            Some(match target.as_str() {
                "next" => StateCommand::MoveWindowToNextDisplay { window: None },
                "previous" => StateCommand::MoveWindowToPreviousDisplay { window: None },
                id => StateCommand::MoveWindowToDisplay {
                    window: None,
                    display: tylora_core::DisplayId(id.parse().ok()?),
                },
            })
        }
        [group, action, target] if group == "display" && action == "focus" && target == "next" => {
            Some(StateCommand::FocusDisplayNext)
        }
        [group, action, target] if group == "display" && action == "focus" => {
            Some(StateCommand::FocusDisplay {
                direction: direction(target)?,
            })
        }
        [group, action, target]
            if group == "display" && action == "move-window" && target == "next" =>
        {
            Some(StateCommand::MoveWindowToNextDisplay { window: None })
        }
        [group, action, target] if group == "display" && action == "move-window" => {
            if target == "previous" {
                return Some(StateCommand::MoveWindowToPreviousDisplay { window: None });
            }
            Some(StateCommand::MoveWindowToDisplay {
                window: None,
                display: tylora_core::DisplayId(target.parse().ok()?),
            })
        }
        _ => None,
    }
}

fn list_windows(show_ax: bool) -> ExitCode {
    let worker = discover_windows_async();
    let result = worker.recv().unwrap_or(Err(DiscoveryError::WorkerStopped));
    match result {
        Ok(snapshot) => {
            tracing::info!(
                cg_windows = snapshot.windows.len(),
                ax_geometries = snapshot.ax_windows.len(),
                ax_errors = snapshot.ax_issues.len(),
                ax_trusted = snapshot.accessibility_trusted,
                "discovery complete"
            );
            if !snapshot.accessibility_trusted {
                tracing::warn!("Accessibility permission is absent; grant this executable access in System Settings > Privacy & Security > Accessibility");
            }
            for issue in &snapshot.ax_issues {
                tracing::warn!(
                    pid = issue.owner_pid,
                    operation = issue.operation,
                    code = issue.code,
                    meaning = issue.meaning(),
                    "AX read failed"
                );
            }
            println!("ID\tPID\tDISPLAY\tX\tY\tWIDTH\tHEIGHT\tOWNER");
            for window in &snapshot.windows {
                println!(
                    "{}\t{}\t{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{}",
                    window.id.0,
                    window.owner_pid,
                    window
                        .display_id
                        .map(|id| id.0.to_string())
                        .unwrap_or_else(|| "?".into()),
                    window.frame.x,
                    window.frame.y,
                    window.frame.width,
                    window.frame.height,
                    window.owner_name.as_deref().unwrap_or("?")
                );
            }
            if show_ax {
                println!("\nAX GEOMETRY (unpaired with CG IDs)");
                println!("PID\tX\tY\tWIDTH\tHEIGHT");
                for window in &snapshot.ax_windows {
                    println!(
                        "{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}",
                        window.owner_pid,
                        window.frame.x,
                        window.frame.y,
                        window.frame.width,
                        window.frame.height
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "window discovery failed");
            ExitCode::FAILURE
        }
    }
}
