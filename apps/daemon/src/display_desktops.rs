//! `disconnect_policy = "recreate_desktops"`: when an external display
//! disconnects, macOS appends its Desktops to the remaining display. Tylora
//! instead moves their windows into the matching workspaces there, removes
//! those Desktops, and recreates them (windows and workspace bindings
//! included) when the same display (by UUID) returns.
//!
//! Planning is pure; the workers below run on their own thread because
//! native moves and Mission Control take seconds.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tylora_config::Config;

/// A display stays disconnected this long before its Desktops are removed,
/// so sleep and cable glitches never cost the user their Desktops.
pub const DETACH_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
/// A returning display settles this long before Desktops are recreated.
pub const REATTACH_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayDesktops {
    /// Display UUID -> its Desktop Space IDs in order, while connected.
    pub layouts: HashMap<String, Vec<u64>>,
    /// Display UUID -> Desktops removed at its last disconnect.
    pub detached: HashMap<String, Detached>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Detached {
    /// Removed Desktop Space IDs; the index is the Desktop position.
    pub desktops: Vec<u64>,
    /// Position of the scratchpad parking Desktop among `desktops`.
    pub parking: Option<usize>,
    /// Windows moved off the removed Desktops, to return on reconnect.
    pub windows: Vec<MovedWindow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedWindow {
    pub pid: i32,
    pub window_id: u32,
    /// Desktop position on the disconnected display.
    pub desktop: usize,
    /// Space the window was moved to on the remaining display.
    pub moved_to: u64,
}

/// Where windows of the removed Desktops go on the remaining display.
pub struct DetachTargets<'a> {
    pub config: &'a Config,
    /// Desktops of the remaining display that stay.
    pub remaining: &'a [u64],
    /// Parking Desktops: (of the disconnected display, of the remaining one).
    pub parking: (Option<u64>, Option<u64>),
    /// Fallback: the remaining display's active Desktop.
    pub active: u64,
}

impl DetachTargets<'_> {
    /// The remaining display's Space of a workspace, by name or ID.
    fn instance(&self, selector: &str) -> Option<u64> {
        self.config
            .workspace(selector)?
            .space_ids()
            .into_iter()
            .find(|space| self.remaining.contains(space))
    }

    /// Target Space for a window on removed Desktop `space`: parked
    /// scratchpads stay parked, a workspace Desktop maps to the same
    /// workspace, other windows follow their application rule and land on
    /// the active Desktop otherwise.
    pub fn target(&self, space: u64, bundle_id: Option<&str>) -> u64 {
        if let (Some(from), Some(to)) = self.parking {
            if from == space {
                return to;
            }
        }
        let by_workspace = self
            .config
            .workspaces
            .iter()
            .find(|ws| ws.binds(space))
            .and_then(|ws| self.instance(&ws.id.to_string()));
        let by_rule = || {
            let bundle_id = bundle_id?;
            self.config
                .rules
                .iter()
                // Titles are not visible to WindowServer; skip title rules.
                .filter(|rule| rule.enabled && rule.bundle_id == bundle_id && rule.title.is_none())
                .filter_map(|rule| Some((rule.priority, self.instance(rule.workspace.as_ref()?)?)))
                .max_by_key(|(priority, _)| *priority)
                .map(|(_, space)| space)
        };
        by_workspace.or_else(by_rule).unwrap_or(self.active)
    }
}

/// Workspace rebinds `(workspace, removed Space, recreated Space)` for the
/// recreated Desktops, in the format `apply_space_rebinds` takes.
pub fn plan_rebinds(config: &Config, removed: &[u64], recreated: &[u64]) -> Vec<(u8, u64, u64)> {
    config
        .workspaces
        .iter()
        .flat_map(|ws| {
            ws.space_ids().into_iter().filter_map(move |space| {
                let position = removed.iter().position(|old| *old == space)?;
                Some((ws.id, space, *recreated.get(position)?))
            })
        })
        .collect()
}

/// Space IDs of the Desktops removed at a disconnect.
pub fn removed_spaces(detached: &Detached) -> std::collections::HashSet<u64> {
    detached.desktops.iter().copied().collect()
}

#[cfg(target_os = "macos")]
pub use worker::{detach, reattach, Reattached};

#[cfg(target_os = "macos")]
mod worker {
    use super::*;
    use std::{
        collections::HashSet,
        thread,
        time::{Duration, Instant},
    };
    use tracing::{info, warn};
    use tylora_core::DisplayId;
    use tylora_macos::spaces::{
        ExperimentalSkyLightBackend, NativeSpaceBackend, NativeSpaceId, NativeWindowSpaceMover,
    };

    /// Windows with exactly one membership among `spaces`.
    fn windows_on(spaces: &HashSet<u64>) -> Vec<(i32, u32, u64)> {
        let backend = ExperimentalSkyLightBackend::new();
        tylora_macos::discover_all_cg_windows()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|window| {
                let membership = backend.get_window_spaces(window.id.0).ok()?;
                match membership.as_slice() {
                    [space] if spaces.contains(&space.0) => {
                        Some((window.owner_pid, window.id.0, space.0))
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Moves the windows off `desktops` (the disconnected display's, now on
    /// another display) and removes those Desktops. Returns the display that
    /// held them and what to restore later.
    /// `parking` is the disconnected display's parking Desktop, `parkings`
    /// those of every display (the remaining one's is picked from them).
    pub fn detach(
        config: &Config,
        desktops: &[u64],
        parking: Option<u64>,
        parkings: &[u64],
    ) -> Result<(DisplayId, Detached), String> {
        let started = Instant::now();
        let backend = ExperimentalSkyLightBackend::new();
        let removed: HashSet<u64> = desktops.iter().copied().collect();
        let orders = backend.display_order().map_err(|error| error.to_string())?;
        let host = orders
            .iter()
            .find(|order| {
                order
                    .spaces
                    .iter()
                    .any(|space| removed.contains(&space.id.0))
            })
            .ok_or("the disconnected display's Desktops are on no display")?;
        let remaining: Vec<u64> = host
            .spaces
            .iter()
            .filter(|space| !space.fullscreen && !removed.contains(&space.id.0))
            .map(|space| space.id.0)
            .collect();
        let active = Some(host.active.0)
            .filter(|space| remaining.contains(space))
            .or_else(|| remaining.first().copied())
            .ok_or("the remaining display has no Desktop of its own")?;
        let host_parking = parkings
            .iter()
            .copied()
            .find(|space| remaining.contains(space));
        let targets = DetachTargets {
            config,
            remaining: &remaining,
            parking: (parking, host_parking),
            active,
        };
        let mut windows = Vec::new();
        for (pid, window_id, space) in windows_on(&removed) {
            let bundle = tylora_macos::bundle_identifier(pid);
            let target = targets.target(space, bundle.as_deref());
            match NativeWindowSpaceMover::move_window_to_space(window_id, NativeSpaceId(target)) {
                Ok(_) => windows.push(MovedWindow {
                    pid,
                    window_id,
                    desktop: desktops.iter().position(|id| *id == space).unwrap_or(0),
                    moved_to: target,
                }),
                Err(error) => {
                    warn!(%error, pid, window_id, "cannot move window off a disconnected display's Desktop")
                }
            }
        }
        // Mission Control positions include fullscreen Spaces.
        let order = backend
            .display_order_for(host.display)
            .map_err(|error| error.to_string())?;
        let positions: Vec<usize> = order
            .spaces
            .iter()
            .enumerate()
            .filter(|(_, space)| removed.contains(&space.id.0))
            .map(|(index, _)| index)
            .collect();
        tylora_macos::remove_spaces(host.display, &positions, order.spaces.len())?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let left = backend
                .list_spaces()
                .map_err(|error| error.to_string())?
                .iter()
                .filter(|space| removed.contains(&space.id.0))
                .count();
            if left == 0 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("{left} Desktops were not removed"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        info!(
            display_id = host.display.0,
            removed = desktops.len(),
            windows = windows.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "disconnected display's Desktops removed"
        );
        Ok((
            host.display,
            Detached {
                desktops: desktops.to_vec(),
                parking: parking.and_then(|space| desktops.iter().position(|id| *id == space)),
                windows,
            },
        ))
    }

    pub struct Reattached {
        /// New Desktop Space IDs, by the removed Desktops' positions.
        pub desktops: Vec<u64>,
        /// Windows returned: (window, new Space).
        pub windows: Vec<(u32, u64)>,
    }

    /// Recreates the removed Desktops on `display` and returns the windows
    /// that still sit where they were moved to.
    pub fn reattach(display: DisplayId, detached: &Detached) -> Result<Reattached, String> {
        let started = Instant::now();
        let backend = ExperimentalSkyLightBackend::new();
        let desktops_on = || -> Result<Vec<u64>, String> {
            Ok(backend
                .list_spaces()
                .map_err(|error| error.to_string())?
                .into_iter()
                .filter(|space| space.display == display)
                .map(|space| space.id.0)
                .collect())
        };
        let wanted = detached.desktops.len();
        let missing = wanted.saturating_sub(desktops_on()?.len());
        if missing > 0 {
            let missing = u8::try_from(missing).map_err(|_| "too many Desktops to recreate")?;
            tylora_macos::add_desktops(display, missing)?;
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        let desktops = loop {
            let current = desktops_on()?;
            if current.len() >= wanted {
                break current[..wanted].to_vec();
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "only {} of {wanted} Desktops exist on display {}",
                    current.len(),
                    display.0
                ));
            }
            thread::sleep(Duration::from_millis(50));
        };
        let moved: HashSet<u64> = detached
            .windows
            .iter()
            .map(|window| window.moved_to)
            .collect();
        let still: HashMap<u32, u64> = windows_on(&moved)
            .into_iter()
            .map(|(_, window_id, space)| (window_id, space))
            .collect();
        let mut windows = Vec::new();
        for window in &detached.windows {
            // Closed, or moved by the user since: leave it alone.
            if still.get(&window.window_id) != Some(&window.moved_to) {
                continue;
            }
            let Some(&target) = desktops.get(window.desktop) else {
                continue;
            };
            match NativeWindowSpaceMover::move_window_to_space(
                window.window_id,
                NativeSpaceId(target),
            ) {
                Ok(_) => windows.push((window.window_id, target)),
                Err(error) => {
                    warn!(%error, window_id = window.window_id, "cannot return window to its recreated Desktop")
                }
            }
        }
        let display_id = display.0;
        info!(
            display_id,
            desktops = desktops.len(),
            windows = windows.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "reconnected display's Desktops recreated"
        );
        Ok(Reattached { desktops, windows })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        toml::from_str(
            r#"
            version = 1
            [[workspaces]]
            id = 1
            name = "Browser"
            native_space_id = 7
            native_space_ids = [4865]
            [[workspaces]]
            id = 2
            name = "Code"
            native_space_id = 4
            native_space_ids = [4687]
            [[workspaces]]
            id = 4
            name = "Funy"
            native_space_id = 6
            [[rules]]
            id = "zen"
            bundle_id = "app.zen-browser.zen"
            workspace = "Browser"
            [[rules]]
            id = "titled"
            bundle_id = "com.example.titled"
            title = "Inbox"
            workspace = "Code"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn windows_land_in_the_same_workspace_on_the_remaining_display() {
        let config = config();
        let targets = DetachTargets {
            config: &config,
            remaining: &[7, 4, 5, 6, 1, 4763],
            parking: (Some(4779), Some(4763)),
            active: 5,
        };
        // Workspace Desktops map to the same workspace.
        assert_eq!(targets.target(4865, None), 7);
        assert_eq!(targets.target(4687, Some("app.zen-browser.zen")), 4);
        // Parked scratchpads stay parked.
        assert_eq!(targets.target(4779, None), 4763);
        // Unbound Desktops follow application rules, then the active Desktop.
        assert_eq!(targets.target(4778, Some("app.zen-browser.zen")), 7);
        assert_eq!(targets.target(4778, Some("com.example.titled")), 5);
        assert_eq!(targets.target(4778, None), 5);
    }

    #[test]
    fn recreated_desktops_take_over_workspace_bindings_by_position() {
        let config = config();
        let rebinds = plan_rebinds(&config, &[4865, 4687, 4698], &[9001, 9002, 9003]);
        assert_eq!(rebinds, vec![(1, 4865, 9001), (2, 4687, 9002)]);
        // Fewer recreated Desktops than removed: nothing rebinds past them.
        assert_eq!(
            plan_rebinds(&config, &[4865, 4687], &[9001]),
            vec![(1, 4865, 9001)]
        );
    }
}
