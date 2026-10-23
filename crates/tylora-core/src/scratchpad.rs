//! Named scratchpads: definitions, geometry, registry and toggle planning.
//! Pure logic — the daemon performs the native moves, AX geometry and focus
//! a [`ScratchpadPlan`] asks for and reports results back.
//!
//! macOS has no per-window "hidden" state that another process can set with
//! SIP enabled (sticky/all-Spaces collection behaviour belongs to the owning
//! app). A hidden scratchpad therefore lives on a dedicated *parking*
//! Desktop of its display; showing moves it to the active Space with the
//! verified native mover. Minimizing is an opt-in alternative.

use crate::{registry::ProcessInstance, DisplayId, Rect};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchpadScope {
    /// Shown only on its home Space.
    Local,
    /// Brought to the active Space of the target display.
    #[default]
    Global,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchpadAlignment {
    #[default]
    Center,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Which display a global scratchpad appears on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchpadTarget {
    /// The display under the pointer.
    #[default]
    Pointer,
    /// The display of the focused window, else the pointer's.
    Focused,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HideStrategy {
    /// Move to the display's parking Desktop (verified native move).
    #[default]
    Park,
    /// Minimize to the Dock (explicit opt-in; restored by recovery).
    Minimize,
}

/// One `[[scratchpads]]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchpadDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotkey: Option<String>,
    #[serde(default)]
    pub scope: ScratchpadScope,
    /// Fractions of the display's work area.
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default = "default_height")]
    pub height: f64,
    #[serde(default)]
    pub alignment: ScratchpadAlignment,
    #[serde(default)]
    pub offset_x: f64,
    #[serde(default)]
    pub offset_y: f64,
    #[serde(default)]
    pub target: ScratchpadTarget,
    /// Always use this display (`tylora display list` ID) when connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<u32>,
    #[serde(default = "enabled")]
    pub remember_position: bool,
    #[serde(default = "enabled")]
    pub remember_size: bool,
    /// Shorthand that sets both `remember_position` and `remember_size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember_geometry: Option<bool>,
    #[serde(default)]
    pub hide: HideStrategy,
    /// Application hint for the GUI and rules; not used for matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
}

fn default_width() -> f64 {
    0.7
}
fn default_height() -> f64 {
    0.65
}
fn enabled() -> bool {
    true
}

impl ScratchpadDefinition {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            name: String::new(),
            hotkey: None,
            scope: ScratchpadScope::Global,
            width: default_width(),
            height: default_height(),
            alignment: ScratchpadAlignment::Center,
            offset_x: 0.0,
            offset_y: 0.0,
            target: ScratchpadTarget::Pointer,
            display: None,
            remember_position: true,
            remember_size: true,
            remember_geometry: None,
            hide: HideStrategy::Park,
            bundle_id: None,
        }
    }

    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
    pub fn remembers_position(&self) -> bool {
        self.remember_geometry.unwrap_or(self.remember_position)
    }
    pub fn remembers_size(&self) -> bool {
        self.remember_geometry.unwrap_or(self.remember_size)
    }
}

pub fn validate_scratchpads(scratchpads: &[ScratchpadDefinition]) -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for pad in scratchpads {
        let id = pad.id.trim();
        if id.is_empty()
            || id.len() > 32
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "scratchpad ID {id:?} must be 1 to 32 letters, digits, '-' or '_'"
            ));
        }
        if !ids.insert(id.to_ascii_lowercase()) {
            return Err(format!("duplicate scratchpad ID {id:?}"));
        }
        let fraction = |value: f64| value.is_finite() && (0.1..=1.0).contains(&value);
        if !fraction(pad.width) || !fraction(pad.height) {
            return Err(format!(
                "scratchpad {id:?}: width and height must be between 0.1 and 1"
            ));
        }
        let offset = |value: f64| value.is_finite() && value.abs() <= 4000.0;
        if !offset(pad.offset_x) || !offset(pad.offset_y) {
            return Err(format!(
                "scratchpad {id:?}: offsets must be within ±4000 points"
            ));
        }
    }
    Ok(())
}

/// Frame for a scratchpad on `area` (a display work area, already excluding
/// menu bar and Dock). A remembered frame keeps its size and/or position as
/// configured. The result always lies inside `area`.
pub fn scratchpad_frame(
    definition: &ScratchpadDefinition,
    area: Rect,
    remembered: Option<Rect>,
    remembered_area: Option<Rect>,
) -> Rect {
    let mut width = (area.width * definition.width).max(160.0_f64.min(area.width));
    let mut height = (area.height * definition.height).max(120.0_f64.min(area.height));
    if let Some(previous) = remembered.filter(|_| definition.remembers_size()) {
        width = previous.width;
        height = previous.height;
    }
    width = width.min(area.width).max(1.0);
    height = height.min(area.height).max(1.0);
    let free_x = area.width - width;
    let free_y = area.height - height;
    use ScratchpadAlignment::*;
    let mut fx = match definition.alignment {
        Left | TopLeft | BottomLeft => 0.0,
        Right | TopRight | BottomRight => 1.0,
        Center | Top | Bottom => 0.5,
    };
    let mut fy = match definition.alignment {
        Top | TopLeft | TopRight => 0.0,
        Bottom | BottomLeft | BottomRight => 1.0,
        Center | Left | Right => 0.5,
    };
    let mut offset = (definition.offset_x, definition.offset_y);
    if let Some(previous) = remembered.filter(|_| definition.remembers_position()) {
        match remembered_area {
            // Keep the remembered position as a share of the free space of
            // the work area it was seen in, so a centred window stays
            // centred on a display of another size or place.
            Some(origin) => {
                let share = |at: f64, start: f64, free: f64, fallback: f64| {
                    if free > 0.0 {
                        ((at - start) / free).clamp(0.0, 1.0)
                    } else {
                        fallback
                    }
                };
                fx = share(previous.x, origin.x, origin.width - previous.width, fx);
                fy = share(previous.y, origin.y, origin.height - previous.height, fy);
                offset = (0.0, 0.0);
            }
            // Recorded before positions knew their display: trust it only
            // where it still fits, otherwise fall back to the alignment.
            None if previous.x >= area.x - 1.0
                && previous.y >= area.y - 1.0
                && previous.x + previous.width <= area.x + area.width + 1.0
                && previous.y + previous.height <= area.y + area.height + 1.0 =>
            {
                return Rect::new(previous.x, previous.y, width, height)
                    .expect("finite scratchpad frame");
            }
            None => {}
        }
    }
    let x = (area.x + free_x * fx + offset.0).clamp(area.x, area.x + free_x.max(0.0));
    let y = (area.y + free_y * fy + offset.1).clamp(area.y, area.y + free_y.max(0.0));
    Rect::new(x, y, width, height).expect("finite scratchpad frame")
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScratchpadState {
    #[default]
    Unassigned,
    Hidden,
    Showing,
    Visible,
    Hiding,
    /// The assigned window is gone or cannot be reached.
    Unavailable,
}

/// Identity of a scratchpad window. No AX element or pointer is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchpadWindow {
    pub process: ProcessInstance,
    pub cg_window_id: u32,
    pub bundle_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScratchpadEntry {
    pub id: String,
    pub window: Option<ScratchpadWindow>,
    /// Space the window lived on when assigned.
    pub home_space: Option<u64>,
    pub current_space: Option<u64>,
    pub current_display: Option<DisplayId>,
    /// Whether the window was tiled before assignment, to restore it.
    pub was_tiled: bool,
    pub state: ScratchpadState,
    pub geometry: Option<Rect>,
    /// Work area of the display `geometry` was seen on; the position is
    /// carried over to other displays relative to it.
    #[serde(default)]
    pub geometry_area: Option<Rect>,
    pub generation: u64,
    /// Assigned explicitly; rules never replace a manual assignment.
    pub manual: bool,
    /// CGWindowID focused before the scratchpad was shown.
    pub previous_focus: Option<(i32, u32)>,
    pub last_error: Option<String>,
}

impl ScratchpadEntry {
    fn new(id: &str) -> Self {
        Self {
            id: id.into(),
            window: None,
            home_space: None,
            current_space: None,
            current_display: None,
            was_tiled: false,
            state: ScratchpadState::Unassigned,
            geometry: None,
            geometry_area: None,
            generation: 0,
            manual: false,
            previous_focus: None,
            last_error: None,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(
            self.state,
            ScratchpadState::Showing | ScratchpadState::Hiding
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScratchpadError {
    UnknownScratchpad,
    NotAssigned,
    /// The window already belongs to another scratchpad.
    WindowTaken(String),
    /// The scratchpad holds another window; replacing needs `force`.
    Occupied,
    Busy,
    Unavailable,
    /// A local scratchpad can only show on its home Space.
    NotOnHomeSpace(u64),
    StaleOperation,
}

impl std::fmt::Display for ScratchpadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownScratchpad => write!(f, "no such scratchpad"),
            Self::NotAssigned => write!(f, "no window is assigned to this scratchpad"),
            Self::WindowTaken(other) => write!(f, "window already belongs to scratchpad {other:?}"),
            Self::Occupied => write!(
                f,
                "scratchpad already holds a window; unassign it first or pass --force"
            ),
            Self::Busy => write!(f, "scratchpad operation in progress"),
            Self::Unavailable => write!(f, "scratchpad window is unavailable"),
            Self::NotOnHomeSpace(space) => {
                write!(f, "local scratchpad shows only on its home Space {space}")
            }
            Self::StaleOperation => write!(f, "operation superseded"),
        }
    }
}

/// What to do for a toggle, show or hide.
#[derive(Debug, Clone, PartialEq)]
pub enum ScratchpadPlan {
    /// Present on `space` of `display`; `move_from` is set when the window
    /// must first be moved there, `unminimize` when it is minimized.
    Show {
        display: DisplayId,
        space: u64,
        move_from: Option<u64>,
        unminimize: bool,
    },
    /// Park on `space` (the display's parking Desktop), or minimize.
    Hide {
        park: Option<u64>,
        minimize: bool,
    },
    /// The display needs a parking Desktop before the window can be hidden.
    NeedsParking(DisplayId),
    /// A local scratchpad was called from another Space: switch `display`
    /// to its home `space` first, then show it there.
    VisitHome {
        display: DisplayId,
        space: u64,
    },
    /// A local scratchpad called again on its home Space: switch `display`
    /// back to `space`, where the user came from. The window stays put.
    Return {
        display: DisplayId,
        space: u64,
    },
    Nothing,
}

/// Live facts the planner needs; all come from the daemon's state.
#[derive(Debug, Clone, Copy)]
pub struct ScratchpadContext {
    /// Active Space of the display the scratchpad should appear on.
    pub target_display: DisplayId,
    pub target_space: u64,
    /// Space currently holding the window, if WindowServer reports one.
    pub window_space: Option<u64>,
    /// Display of the window's Space.
    pub window_display: Option<DisplayId>,
    /// Whether the window's Space is the active one on its display.
    pub window_space_active: bool,
    pub minimized: bool,
    /// Parking Desktop of the window's display, if known.
    pub parking_space: Option<u64>,
    /// Whether the home Space is active (local scope).
    pub home_active: bool,
    /// Display holding the home Space, if it still exists.
    pub home_display: Option<DisplayId>,
    /// Where a local scratchpad's home visit came from, if it did.
    pub return_to: Option<(DisplayId, u64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScratchpadAction {
    Toggle,
    Show,
    Hide,
}

/// Decides the plan. A scratchpad counts as visible when its window is on
/// an active Space and not minimized, regardless of the recorded state, so
/// external changes (Mission Control, Cmd-Tab) never invert the toggle.
pub fn plan_scratchpad(
    definition: &ScratchpadDefinition,
    entry: &ScratchpadEntry,
    action: ScratchpadAction,
    context: ScratchpadContext,
) -> Result<ScratchpadPlan, ScratchpadError> {
    if entry.busy() {
        return Err(ScratchpadError::Busy);
    }
    if entry.window.is_none() {
        return Err(ScratchpadError::NotAssigned);
    }
    if entry.state == ScratchpadState::Unavailable {
        return Err(ScratchpadError::Unavailable);
    }
    let visible = context.window_space_active && !context.minimized;
    let show = match action {
        ScratchpadAction::Show => true,
        ScratchpadAction::Hide => false,
        ScratchpadAction::Toggle => {
            // Visible elsewhere (another display) and global: bring it here.
            !visible
                || (definition.scope == ScratchpadScope::Global
                    && context.window_space != Some(context.target_space))
        }
    };
    if show {
        let (display, space) = match definition.scope {
            ScratchpadScope::Global => (context.target_display, context.target_space),
            ScratchpadScope::Local => {
                let home = entry.home_space.ok_or(ScratchpadError::Unavailable)?;
                if !context.home_active {
                    let display = context
                        .home_display
                        .ok_or(ScratchpadError::NotOnHomeSpace(home))?;
                    return Ok(ScratchpadPlan::VisitHome {
                        display,
                        space: home,
                    });
                }
                (
                    context.window_display.unwrap_or(context.target_display),
                    home,
                )
            }
        };
        if visible && context.window_space == Some(space) && action == ScratchpadAction::Show {
            return Ok(ScratchpadPlan::Show {
                display,
                space,
                move_from: None,
                unminimize: false,
            });
        }
        return Ok(ScratchpadPlan::Show {
            display,
            space,
            move_from: context.window_space.filter(|current| *current != space),
            unminimize: context.minimized,
        });
    }
    if !visible {
        return Ok(ScratchpadPlan::Nothing);
    }
    // A local scratchpad is never hidden or moved: calling it again goes
    // back to the Space the visit came from (nothing to do without one).
    if definition.scope == ScratchpadScope::Local {
        return Ok(match context.return_to {
            Some((display, space)) => ScratchpadPlan::Return { display, space },
            None => ScratchpadPlan::Nothing,
        });
    }
    match definition.hide {
        HideStrategy::Minimize => Ok(ScratchpadPlan::Hide {
            park: None,
            minimize: true,
        }),
        HideStrategy::Park => match context.parking_space {
            Some(space) if Some(space) != context.window_space => Ok(ScratchpadPlan::Hide {
                park: Some(space),
                minimize: false,
            }),
            _ => Ok(ScratchpadPlan::NeedsParking(
                context.window_display.unwrap_or(context.target_display),
            )),
        },
    }
}

/// All scratchpads and their windows. Persisted by the daemon.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScratchpadRegistry {
    pub entries: BTreeMap<String, ScratchpadEntry>,
    /// Parking Desktop per display UUID (hex).
    pub parking: HashMap<String, u64>,
}

impl ScratchpadRegistry {
    /// Adds entries for new definitions and drops entries whose definition
    /// was removed (their windows are returned by the caller first).
    pub fn sync_definitions(
        &mut self,
        definitions: &[ScratchpadDefinition],
    ) -> Vec<ScratchpadEntry> {
        let ids: std::collections::HashSet<_> =
            definitions.iter().map(|pad| pad.id.clone()).collect();
        let removed: Vec<_> = self
            .entries
            .keys()
            .filter(|id| !ids.contains(*id))
            .cloned()
            .collect();
        let removed = removed
            .into_iter()
            .filter_map(|id| self.entries.remove(&id))
            .collect();
        for pad in definitions {
            self.entries
                .entry(pad.id.clone())
                .or_insert_with(|| ScratchpadEntry::new(&pad.id));
        }
        removed
    }

    pub fn get(&self, id: &str) -> Option<&ScratchpadEntry> {
        self.entries.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ScratchpadEntry> {
        self.entries.get_mut(id)
    }

    /// Scratchpad holding a window.
    pub fn owner_of(&self, process: ProcessInstance, cg: u32) -> Option<&ScratchpadEntry> {
        self.entries.values().find(|entry| {
            entry
                .window
                .as_ref()
                .is_some_and(|window| window.process == process && window.cg_window_id == cg)
        })
    }

    /// Assigns a window. A window belongs to at most one scratchpad; an
    /// occupied scratchpad is only replaced with `force` (manual) and never
    /// by a rule. Returns the previously assigned window, if replaced.
    #[allow(clippy::too_many_arguments)]
    pub fn assign(
        &mut self,
        id: &str,
        window: ScratchpadWindow,
        space: Option<u64>,
        display: Option<DisplayId>,
        was_tiled: bool,
        manual: bool,
        force: bool,
    ) -> Result<Option<ScratchpadWindow>, ScratchpadError> {
        if let Some(other) = self.owner_of(window.process, window.cg_window_id) {
            if other.id != id {
                return Err(ScratchpadError::WindowTaken(other.id.clone()));
            }
        }
        let entry = self
            .entries
            .get_mut(id)
            .ok_or(ScratchpadError::UnknownScratchpad)?;
        if entry.busy() {
            return Err(ScratchpadError::Busy);
        }
        let occupied = entry.window.is_some() && entry.state != ScratchpadState::Unavailable;
        if occupied
            && entry.window.as_ref().map(|w| (w.process, w.cg_window_id))
                != Some((window.process, window.cg_window_id))
        {
            // Only an explicit, forced assignment replaces a held window.
            if !(manual && force) {
                return Err(ScratchpadError::Occupied);
            }
        }
        let previous = entry.window.replace(window);
        entry.home_space = space;
        entry.current_space = space;
        entry.current_display = display;
        entry.was_tiled = was_tiled;
        entry.state = ScratchpadState::Visible;
        entry.manual = manual;
        entry.generation = entry.generation.wrapping_add(1);
        entry.last_error = None;
        Ok(previous)
    }

    /// Releases a scratchpad's window; returns it with whether it was tiled.
    pub fn unassign(
        &mut self,
        id: &str,
    ) -> Result<(ScratchpadEntry, ScratchpadWindow), ScratchpadError> {
        let entry = self
            .entries
            .get_mut(id)
            .ok_or(ScratchpadError::UnknownScratchpad)?;
        if entry.busy() {
            return Err(ScratchpadError::Busy);
        }
        let window = entry.window.take().ok_or(ScratchpadError::NotAssigned)?;
        let before = entry.clone();
        let generation = entry.generation.wrapping_add(1);
        *entry = ScratchpadEntry::new(id);
        entry.generation = generation;
        Ok((before, window))
    }

    /// Starts an operation; returns its generation. Rejects concurrent ones.
    pub fn begin(&mut self, id: &str, showing: bool) -> Result<u64, ScratchpadError> {
        let entry = self
            .entries
            .get_mut(id)
            .ok_or(ScratchpadError::UnknownScratchpad)?;
        if entry.busy() {
            return Err(ScratchpadError::Busy);
        }
        entry.generation = entry.generation.wrapping_add(1);
        entry.state = if showing {
            ScratchpadState::Showing
        } else {
            ScratchpadState::Hiding
        };
        Ok(entry.generation)
    }

    /// Completes an operation. A result for an older generation is ignored.
    pub fn finish(
        &mut self,
        id: &str,
        generation: u64,
        result: Result<(ScratchpadState, Option<u64>, Option<DisplayId>), String>,
    ) -> Result<(), ScratchpadError> {
        let entry = self
            .entries
            .get_mut(id)
            .ok_or(ScratchpadError::UnknownScratchpad)?;
        if entry.generation != generation || !entry.busy() {
            return Err(ScratchpadError::StaleOperation);
        }
        let was_showing = entry.state == ScratchpadState::Showing;
        match result {
            Ok((state, space, display)) => {
                entry.state = state;
                if space.is_some() {
                    entry.current_space = space;
                }
                if display.is_some() {
                    entry.current_display = display;
                }
                entry.last_error = None;
            }
            Err(error) => {
                // The operation failed: the window stays where it was, so
                // fall back to what it was before.
                entry.state = if was_showing {
                    ScratchpadState::Hidden
                } else {
                    ScratchpadState::Visible
                };
                entry.last_error = Some(error);
            }
        }
        Ok(())
    }

    /// Updates a scratchpad from an observation of its window on an active
    /// Space (reconciliation): it is visible there, whatever moved it.
    pub fn observed_visible(
        &mut self,
        process: ProcessInstance,
        cg: u32,
        space: Option<u64>,
        display: DisplayId,
        // The window frame and the work area of `display`, when known.
        (frame, area): (Rect, Option<Rect>),
        title: Option<&str>,
    ) {
        if let Some(entry) = self.entries.values_mut().find(|entry| {
            entry
                .window
                .as_ref()
                .is_some_and(|window| window.process == process && window.cg_window_id == cg)
        }) {
            if !entry.busy() {
                entry.state = ScratchpadState::Visible;
            }
            if space.is_some() {
                entry.current_space = space;
            }
            entry.current_display = Some(display);
            if entry.state == ScratchpadState::Visible {
                entry.geometry = Some(frame);
                entry.geometry_area = area;
            }
            if let (Some(window), Some(title)) = (entry.window.as_mut(), title) {
                window.title = Some(title.to_string());
            }
        }
    }

    /// Drops the remembered frame of scratchpads whose placement settings
    /// changed, so a new alignment or size applies on the next show.
    pub fn forget_changed_geometry(
        &mut self,
        old: &[ScratchpadDefinition],
        new: &[ScratchpadDefinition],
    ) -> Vec<String> {
        let placement = |pad: &ScratchpadDefinition| {
            (
                pad.alignment,
                pad.width.to_bits(),
                pad.height.to_bits(),
                pad.offset_x.to_bits(),
                pad.offset_y.to_bits(),
                pad.remembers_position(),
                pad.remembers_size(),
            )
        };
        let mut changed = Vec::new();
        for pad in new {
            let Some(before) = old.iter().find(|before| before.id == pad.id) else {
                continue;
            };
            if placement(before) == placement(pad) {
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&pad.id) {
                if entry.geometry.take().is_some() {
                    changed.push(pad.id.clone());
                }
                entry.geometry_area = None;
            }
        }
        changed
    }

    /// Marks scratchpads whose process exited or window vanished.
    pub fn window_gone(&mut self, gone: impl Fn(&ScratchpadWindow) -> bool) -> Vec<String> {
        let mut changed = Vec::new();
        for entry in self.entries.values_mut() {
            if entry.window.as_ref().is_some_and(&gone)
                && entry.state != ScratchpadState::Unavailable
            {
                entry.state = ScratchpadState::Unavailable;
                entry.generation = entry.generation.wrapping_add(1);
                changed.push(entry.id.clone());
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(cg: u32) -> ScratchpadWindow {
        ScratchpadWindow {
            process: ProcessInstance { pid: 7, epoch: 1 },
            cg_window_id: cg,
            bundle_id: Some("com.apple.Terminal".into()),
            title: None,
        }
    }
    fn registry() -> ScratchpadRegistry {
        let mut registry = ScratchpadRegistry::default();
        registry.sync_definitions(&[
            ScratchpadDefinition::new("terminal"),
            ScratchpadDefinition::new("messenger"),
        ]);
        registry
    }
    fn context(window_space: u64, active: bool) -> ScratchpadContext {
        ScratchpadContext {
            target_display: DisplayId(1),
            target_space: 5,
            window_space: Some(window_space),
            window_display: Some(DisplayId(1)),
            window_space_active: active,
            minimized: false,
            parking_space: Some(99),
            home_active: false,
            home_display: Some(DisplayId(1)),
            return_to: None,
        }
    }
    fn area() -> Rect {
        Rect::new(0.0, 33.0, 1470.0, 923.0).unwrap()
    }

    #[test]
    fn definitions_validate() {
        assert!(validate_scratchpads(&[ScratchpadDefinition::new("terminal")]).is_ok());
        assert!(validate_scratchpads(&[ScratchpadDefinition::new("bad id")]).is_err());
        assert!(validate_scratchpads(&[
            ScratchpadDefinition::new("a"),
            ScratchpadDefinition::new("A")
        ])
        .is_err());
        let mut wide = ScratchpadDefinition::new("w");
        wide.width = 1.5;
        assert!(validate_scratchpads(&[wide]).is_err());
        let mut far = ScratchpadDefinition::new("f");
        far.offset_x = f64::NAN;
        assert!(validate_scratchpads(&[far]).is_err());
    }

    #[test]
    fn geometry_defaults_alignment_and_clamping() {
        let pad = ScratchpadDefinition::new("t");
        let frame = scratchpad_frame(&pad, area(), None, None);
        assert!((frame.width - 1470.0 * 0.7).abs() < 1e-9);
        assert!((frame.height - 923.0 * 0.65).abs() < 1e-9);
        assert!((frame.x - (1470.0 - frame.width) / 2.0).abs() < 1e-9);
        assert!((frame.y - (33.0 + (923.0 - frame.height) / 2.0)).abs() < 1e-9);
        let mut corner = pad.clone();
        corner.alignment = ScratchpadAlignment::BottomRight;
        corner.offset_x = 500.0;
        let frame = scratchpad_frame(&corner, area(), None, None);
        assert!(
            frame.x + frame.width <= 1470.0 + 1e-9,
            "offset clamped into the area"
        );
        assert!((frame.y + frame.height - 956.0).abs() < 1e-9);
        // A remembered frame keeps size and position but stays inside a
        // smaller display.
        let remembered = Rect::new(1200.0, 700.0, 2000.0, 400.0).unwrap();
        let small = Rect::new(0.0, 25.0, 1280.0, 775.0).unwrap();
        let frame = scratchpad_frame(&pad, small, Some(remembered), Some(area()));
        assert_eq!(frame.width, 1280.0);
        assert!(frame.x >= 0.0 && frame.y + frame.height <= 800.0 + 1e-9);
        let mut forget = pad;
        forget.remember_size = false;
        forget.remember_position = false;
        assert_eq!(
            scratchpad_frame(&forget, area(), Some(remembered), Some(area())),
            scratchpad_frame(&forget, area(), None, None)
        );
    }

    #[test]
    fn remembered_position_follows_the_window_to_another_display() {
        let pad = ScratchpadDefinition::new("t");
        // Centred on the Mac (work area 0,33 1470x923).
        let centred = scratchpad_frame(&pad, area(), None, None);
        // An external display above it, as in a real arrangement.
        let external = Rect::new(-201.0, -1080.0, 1920.0, 1080.0).unwrap();
        let moved = scratchpad_frame(&pad, external, Some(centred), Some(area()));
        assert!((moved.x - (-201.0 + (1920.0 - moved.width) / 2.0)).abs() < 1e-6);
        assert!((moved.y - (-1080.0 + (1080.0 - moved.height) / 2.0)).abs() < 1e-6);
        // A legacy absolute position from the Mac does not fit up there:
        // the alignment applies instead of a clamp to the display edge.
        let legacy = Rect::new(739.0, 33.0, 731.0, 499.0).unwrap();
        let fallback = scratchpad_frame(&pad, external, Some(legacy), None);
        assert!((fallback.x - (-201.0 + (1920.0 - 731.0) / 2.0)).abs() < 1e-6);
        assert!((fallback.y - (-1080.0 + (1080.0 - 499.0) / 2.0)).abs() < 1e-6);
        // ...but where it fits, it is kept as is.
        assert_eq!(scratchpad_frame(&pad, area(), Some(legacy), None), legacy);
    }

    #[test]
    fn changed_placement_settings_forget_the_remembered_frame() {
        let mut registry = registry();
        let old = vec![ScratchpadDefinition::new("terminal")];
        let entry = registry.entries.get_mut("terminal").unwrap();
        entry.geometry = Some(Rect::new(739.0, 33.0, 731.0, 499.0).unwrap());
        assert!(registry.forget_changed_geometry(&old, &old).is_empty());
        let mut new = old.clone();
        new[0].alignment = ScratchpadAlignment::Top;
        assert_eq!(
            registry.forget_changed_geometry(&old, &new),
            vec!["terminal".to_string()]
        );
        assert_eq!(registry.get("terminal").unwrap().geometry, None);
    }

    #[test]
    fn assignment_conflicts_and_manual_priority() {
        let mut registry = registry();
        registry
            .assign(
                "terminal",
                window(10),
                Some(4),
                Some(DisplayId(1)),
                true,
                true,
                false,
            )
            .unwrap();
        assert_eq!(
            registry.assign("messenger", window(10), Some(4), None, false, true, false),
            Err(ScratchpadError::WindowTaken("terminal".into()))
        );
        // A rule never replaces a manual assignment.
        assert_eq!(
            registry.assign("terminal", window(11), Some(4), None, false, false, false),
            Err(ScratchpadError::Occupied)
        );
        assert_eq!(
            registry.assign("terminal", window(11), Some(4), None, false, true, false),
            Err(ScratchpadError::Occupied)
        );
        let previous = registry
            .assign("terminal", window(11), Some(4), None, false, true, true)
            .unwrap();
        assert_eq!(previous.unwrap().cg_window_id, 10);
        let (entry, released) = registry.unassign("terminal").unwrap();
        assert_eq!(released.cg_window_id, 11);
        assert!(!entry.was_tiled);
        assert_eq!(
            registry.get("terminal").unwrap().state,
            ScratchpadState::Unassigned
        );
        assert_eq!(
            registry.unassign("terminal").unwrap_err(),
            ScratchpadError::NotAssigned
        );
    }

    #[test]
    fn toggle_plans_show_hide_and_global_follow() {
        let mut registry = registry();
        registry
            .assign(
                "terminal",
                window(10),
                Some(4),
                Some(DisplayId(1)),
                false,
                true,
                false,
            )
            .unwrap();
        let pad = ScratchpadDefinition::new("terminal");
        let entry = registry.get("terminal").unwrap().clone();
        // Hidden on the parking Desktop: toggle moves it to the active Space.
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, context(99, false)),
            Ok(ScratchpadPlan::Show {
                display: DisplayId(1),
                space: 5,
                move_from: Some(99),
                unminimize: false
            })
        );
        // Visible here: toggle parks it.
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, context(5, true)),
            Ok(ScratchpadPlan::Hide {
                park: Some(99),
                minimize: false
            })
        );
        // Visible on another active Space (other display): brought here.
        let mut elsewhere = context(4, true);
        elsewhere.window_display = Some(DisplayId(2));
        assert!(matches!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, elsewhere),
            Ok(ScratchpadPlan::Show {
                move_from: Some(4),
                ..
            })
        ));
        // Hide with no parking Desktop yet asks for one.
        let mut no_parking = context(5, true);
        no_parking.parking_space = None;
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Hide, no_parking),
            Ok(ScratchpadPlan::NeedsParking(DisplayId(1)))
        );
        // Hiding something already hidden does nothing.
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Hide, context(99, false)),
            Ok(ScratchpadPlan::Nothing)
        );
        // Minimize strategy.
        let mut minimize = pad.clone();
        minimize.hide = HideStrategy::Minimize;
        assert_eq!(
            plan_scratchpad(
                &minimize,
                &entry,
                ScratchpadAction::Toggle,
                context(5, true)
            ),
            Ok(ScratchpadPlan::Hide {
                park: None,
                minimize: true
            })
        );
        let mut minimized = context(5, true);
        minimized.minimized = true;
        assert!(matches!(
            plan_scratchpad(&minimize, &entry, ScratchpadAction::Toggle, minimized),
            Ok(ScratchpadPlan::Show {
                unminimize: true,
                move_from: None,
                ..
            })
        ));
    }

    #[test]
    fn local_scope_returns_where_the_visit_came_from() {
        let mut registry = registry();
        registry
            .assign(
                "terminal",
                window(10),
                Some(4),
                Some(DisplayId(1)),
                false,
                true,
                false,
            )
            .unwrap();
        let mut pad = ScratchpadDefinition::new("terminal");
        pad.scope = ScratchpadScope::Local;
        pad.hide = HideStrategy::Park;
        let entry = registry.get("terminal").unwrap().clone();
        let mut here = context(4, true);
        here.home_active = true;
        // Called on its home Space after a visit: go back, leave the window.
        here.return_to = Some((DisplayId(1), 7));
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, here),
            Ok(ScratchpadPlan::Return {
                display: DisplayId(1),
                space: 7
            })
        );
        // Without a visit to undo, nothing changes; it is never parked.
        here.return_to = None;
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, here),
            Ok(ScratchpadPlan::Nothing)
        );
    }

    #[test]
    fn local_scope_visits_its_home_space() {
        let mut registry = registry();
        registry
            .assign(
                "terminal",
                window(10),
                Some(4),
                Some(DisplayId(1)),
                false,
                true,
                false,
            )
            .unwrap();
        let mut pad = ScratchpadDefinition::new("terminal");
        pad.scope = ScratchpadScope::Local;
        let entry = registry.get("terminal").unwrap().clone();
        // Called from elsewhere: go to the home Space first.
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Show, context(99, false)),
            Ok(ScratchpadPlan::VisitHome {
                display: DisplayId(1),
                space: 4
            })
        );
        // Unless that Space is gone.
        let mut gone = context(99, false);
        gone.home_display = None;
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Show, gone),
            Err(ScratchpadError::NotOnHomeSpace(4))
        );
        let mut home = context(99, false);
        home.home_active = true;
        home.target_space = 4;
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Show, home),
            Ok(ScratchpadPlan::Show {
                display: DisplayId(1),
                space: 4,
                move_from: Some(99),
                unminimize: false
            })
        );
    }

    #[test]
    fn operations_are_exclusive_and_late_results_ignored() {
        let mut registry = registry();
        let pad = ScratchpadDefinition::new("terminal");
        registry
            .assign("terminal", window(10), Some(4), None, false, true, false)
            .unwrap();
        let generation = registry.begin("terminal", true).unwrap();
        assert_eq!(
            registry.begin("terminal", false),
            Err(ScratchpadError::Busy)
        );
        let entry = registry.get("terminal").unwrap().clone();
        assert_eq!(
            plan_scratchpad(&pad, &entry, ScratchpadAction::Toggle, context(5, true)),
            Err(ScratchpadError::Busy)
        );
        assert_eq!(
            registry.finish(
                "terminal",
                generation + 1,
                Ok((ScratchpadState::Visible, Some(5), None))
            ),
            Err(ScratchpadError::StaleOperation)
        );
        // A failed show leaves it hidden (where it was) with the error.
        registry
            .finish("terminal", generation, Err("timeout".into()))
            .unwrap();
        let entry = registry.get("terminal").unwrap();
        assert_eq!(entry.state, ScratchpadState::Hidden);
        assert_eq!(entry.last_error.as_deref(), Some("timeout"));
        assert_eq!(
            registry.finish(
                "terminal",
                generation,
                Ok((ScratchpadState::Visible, None, None))
            ),
            Err(ScratchpadError::StaleOperation),
            "duplicate late result"
        );
    }

    #[test]
    fn closed_windows_become_unavailable_and_observation_reconciles() {
        let mut registry = registry();
        registry
            .assign("terminal", window(10), Some(4), None, false, true, false)
            .unwrap();
        let generation = registry.begin("terminal", false).unwrap();
        registry
            .finish(
                "terminal",
                generation,
                Ok((ScratchpadState::Hidden, Some(99), None)),
            )
            .unwrap();
        // The user brings it back through Mission Control: observed visible.
        let frame = Rect::new(10.0, 40.0, 500.0, 400.0).unwrap();
        registry.observed_visible(
            ProcessInstance { pid: 7, epoch: 1 },
            10,
            Some(5),
            DisplayId(1),
            (frame, None),
            Some("zsh"),
        );
        let entry = registry.get("terminal").unwrap();
        assert_eq!(entry.state, ScratchpadState::Visible);
        assert_eq!(entry.geometry, Some(frame));
        assert_eq!(
            registry.window_gone(|window| window.cg_window_id == 10),
            vec!["terminal".to_string()]
        );
        let entry = registry.get("terminal").unwrap().clone();
        assert_eq!(entry.state, ScratchpadState::Unavailable);
        assert_eq!(
            plan_scratchpad(
                &ScratchpadDefinition::new("terminal"),
                &entry,
                ScratchpadAction::Toggle,
                context(5, true)
            ),
            Err(ScratchpadError::Unavailable)
        );
        // A rule may fill an unavailable scratchpad with the new window.
        registry
            .assign("terminal", window(12), Some(4), None, false, false, false)
            .unwrap();
    }

    #[test]
    fn registry_persists_and_drops_removed_definitions() {
        let mut registry = registry();
        registry
            .assign("terminal", window(10), Some(4), None, true, true, false)
            .unwrap();
        registry.parking.insert("abc".into(), 99);
        let text = serde_json::to_string(&registry).unwrap();
        let restored: ScratchpadRegistry = serde_json::from_str(&text).unwrap();
        assert_eq!(restored, registry);
        let removed = registry.sync_definitions(&[ScratchpadDefinition::new("messenger")]);
        assert_eq!(removed.len(), 1);
        assert!(removed[0].window.is_some(), "caller gets the window back");
        assert!(registry.get("terminal").is_none());
    }
}
