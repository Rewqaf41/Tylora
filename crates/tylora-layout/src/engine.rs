//! One workspace's tiling state across all layout kinds.
//!
//! Each kind keeps its own model (BSP tree, Dwindle tree, Scrolling strip).
//! Only the active model tracks membership live; switching kinds rebuilds the
//! target model's membership from the active one in the active order, so no
//! window is lost or duplicated, while the target keeps whatever ratios or
//! column widths it remembered for windows that are still present.

use crate::{BspLayout, ColumnPlacement, ScrollingLayout, ScrollingMode};
use tylora_core::{
    commands::{CommandError, Direction, StateCommand},
    coordinator::LayoutPolicy,
    layout::{DwindleSettings, FocusOrigin, LayoutEnvironment, LayoutKind, ScrollingSettings},
    registry::ManagedWindowId,
    Rect,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutSettings {
    pub dwindle: DwindleSettings,
    pub scrolling: ScrollingSettings,
}

#[derive(Debug, Clone)]
pub struct TilingLayout {
    kind: LayoutKind,
    bsp: BspLayout,
    dwindle: BspLayout,
    scrolling: ScrollingLayout,
    environment: LayoutEnvironment,
}

impl Default for TilingLayout {
    fn default() -> Self {
        Self::new(LayoutKind::Bsp, &LayoutSettings::default())
    }
}

/// Serializable description for CLI, IPC and the GUI.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LayoutSummary {
    pub kind: LayoutKind,
    pub windows: usize,
    pub order: Vec<ManagedWindowId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrolling: Option<ScrollingSummary>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ScrollingSummary {
    pub mode: ScrollingMode,
    pub viewport_offset: f64,
    pub strip_width: f64,
    pub usable_width: f64,
    pub active: Option<ManagedWindowId>,
    pub state_version: u64,
    pub columns: Vec<ScrollingColumnSummary>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ScrollingColumnSummary {
    pub window: ManagedWindowId,
    pub width_fraction: f64,
    pub strip_x: f64,
    pub width: f64,
    pub frame: Rect,
    pub placement: ColumnPlacement,
}

impl TilingLayout {
    pub fn new(kind: LayoutKind, settings: &LayoutSettings) -> Self {
        Self {
            kind,
            bsp: BspLayout::new(),
            dwindle: BspLayout::dwindle(settings.dwindle),
            scrolling: ScrollingLayout::new(settings.scrolling.clone()),
            environment: LayoutEnvironment::default(),
        }
    }

    pub fn kind(&self) -> LayoutKind {
        self.kind
    }

    pub fn bsp(&self) -> &BspLayout {
        match self.kind {
            LayoutKind::Dwindle => &self.dwindle,
            _ => &self.bsp,
        }
    }

    pub fn scrolling(&self) -> &ScrollingLayout {
        &self.scrolling
    }

    pub fn set_settings(&mut self, settings: &LayoutSettings) {
        self.dwindle.set_dwindle_settings(settings.dwindle);
        self.scrolling.set_settings(settings.scrolling.clone());
    }

    pub fn set_gaps(&mut self, inner: f64, outer: f64) -> Result<(), crate::LayoutError> {
        self.bsp.set_gaps(inner, outer)?;
        self.dwindle.set_gaps(inner, outer)?;
        self.scrolling.set_gaps(inner, outer);
        Ok(())
    }

    pub fn set_min_size(&mut self, width: f64, height: f64) -> Result<(), crate::LayoutError> {
        self.bsp.set_min_size(width, height)?;
        self.dwindle.set_min_size(width, height)
    }

    pub fn windows(&self) -> Vec<ManagedWindowId> {
        match self.kind {
            LayoutKind::Bsp => self.bsp.windows(),
            LayoutKind::Dwindle => self.dwindle.windows(),
            LayoutKind::Scrolling => self.scrolling.windows(),
        }
    }

    pub fn len(&self) -> usize {
        self.windows().len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows().is_empty()
    }

    /// Splits of the active tree; empty for Scrolling.
    pub fn splits(&self) -> Vec<(crate::NodeId, crate::Axis, f64)> {
        match self.kind {
            LayoutKind::Bsp => self.bsp.splits(),
            LayoutKind::Dwindle => self.dwindle.splits(),
            LayoutKind::Scrolling => Vec::new(),
        }
    }

    pub fn resize_split(
        &mut self,
        id: crate::NodeId,
        ratio: f64,
    ) -> Result<(), crate::LayoutError> {
        match self.kind {
            LayoutKind::Bsp => self.bsp.resize_split(id, ratio),
            LayoutKind::Dwindle => self.dwindle.resize_split(id, ratio),
            LayoutKind::Scrolling => Err(crate::LayoutError::NodeNotFound),
        }
    }

    /// Switches the layout kind. Returns false when it is already active.
    pub fn set_kind(&mut self, kind: LayoutKind) -> bool {
        if kind == self.kind {
            return false;
        }
        let order = self.windows();
        let focused = match self.kind {
            LayoutKind::Bsp => self.bsp.focused(),
            LayoutKind::Dwindle => self.dwindle.focused(),
            LayoutKind::Scrolling => self.scrolling.active(),
        };
        let keep: std::collections::HashSet<_> = order.iter().copied().collect();
        match kind {
            LayoutKind::Bsp | LayoutKind::Dwindle => {
                let tree = if kind == LayoutKind::Bsp {
                    &mut self.bsp
                } else {
                    &mut self.dwindle
                };
                for stale in tree.windows() {
                    if !keep.contains(&stale) {
                        tree.remove_window(stale);
                    }
                }
                for id in &order {
                    if !tree.contains(*id) {
                        tree.insert_window(*id);
                    }
                }
                tree.set_focused(focused);
            }
            LayoutKind::Scrolling => {
                for stale in self.scrolling.windows() {
                    if !keep.contains(&stale) {
                        self.scrolling.remove_window(stale);
                    }
                }
                for id in &order {
                    self.scrolling.push_window(*id, None);
                }
            }
        }
        self.kind = kind;
        if let (LayoutKind::Scrolling, Some(id)) = (kind, focused) {
            // Activation only; the next area-aware call settles the viewport.
            self.scrolling.set_active(id);
        }
        true
    }

    pub fn summary(&self, area: Rect) -> LayoutSummary {
        LayoutSummary {
            kind: self.kind,
            windows: self.len(),
            order: self.windows(),
            scrolling: (self.kind == LayoutKind::Scrolling).then(|| {
                let view = self.scrolling.view(area);
                let (mode, placements) = self.scrolling.placements(area);
                ScrollingSummary {
                    mode,
                    viewport_offset: view.offset,
                    strip_width: view.strip_width,
                    usable_width: view.usable.width,
                    active: view.active,
                    state_version: self.scrolling.state_version,
                    columns: view
                        .columns
                        .iter()
                        .zip(placements)
                        .map(|(&(window, strip_x, width), (_, frame, placement))| {
                            ScrollingColumnSummary {
                                window,
                                width_fraction: self.scrolling.column_width(window).unwrap_or(0.0),
                                strip_x,
                                width,
                                frame,
                                placement,
                            }
                        })
                        .collect(),
                }
            }),
        }
    }

    /// Frame inside the work area for a window, used to rescue it from a
    /// parked position.
    pub fn safe_frame(&self, id: ManagedWindowId, area: Rect) -> Option<Rect> {
        match self.kind {
            LayoutKind::Scrolling => self.scrolling.safe_frame(id, area),
            _ => self
                .calculate_layout(area)
                .into_iter()
                .find(|(window, _)| *window == id)
                .map(|(_, frame)| frame),
        }
    }

    pub fn viewport_metrics(&self) -> (u64, u64) {
        (
            self.scrolling.viewport_calculations,
            self.scrolling.viewport_total_us,
        )
    }
}

impl LayoutPolicy for TilingLayout {
    fn insert_window(&mut self, id: ManagedWindowId) {
        match self.kind {
            LayoutKind::Bsp => {
                self.bsp.insert_window(id);
            }
            LayoutKind::Dwindle => {
                self.dwindle.insert_window(id);
            }
            LayoutKind::Scrolling => {
                self.scrolling.insert_window(id);
            }
        }
    }
    fn remove_window(&mut self, id: ManagedWindowId) {
        match self.kind {
            LayoutKind::Bsp => {
                self.bsp.remove_window(id);
            }
            LayoutKind::Dwindle => {
                self.dwindle.remove_window(id);
            }
            LayoutKind::Scrolling => {
                self.scrolling.remove_window(id);
            }
        }
    }
    fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)> {
        match self.kind {
            LayoutKind::Bsp => self.bsp.calculate_layout(area),
            LayoutKind::Dwindle => self.dwindle.calculate_layout(area),
            LayoutKind::Scrolling => self.scrolling.calculate_layout(area),
        }
    }
    fn set_window_min_size(&mut self, id: ManagedWindowId, width: f64, height: f64) -> bool {
        match self.kind {
            LayoutKind::Bsp => self.bsp.set_window_min_size(id, width, height),
            LayoutKind::Dwindle => self.dwindle.set_window_min_size(id, width, height),
            LayoutKind::Scrolling => self.scrolling.set_window_min_size(id, width, height),
        }
    }
    fn swap_windows(&mut self, first: ManagedWindowId, second: ManagedWindowId) -> bool {
        match self.kind {
            LayoutKind::Bsp => self.bsp.swap_windows(first, second),
            LayoutKind::Dwindle => self.dwindle.swap_windows(first, second),
            LayoutKind::Scrolling => self.scrolling.swap_windows(first, second),
        }
    }
    fn resize_window(
        &mut self,
        id: ManagedWindowId,
        direction: Direction,
        amount: f64,
        area: Rect,
    ) -> bool {
        match self.kind {
            LayoutKind::Bsp => self.bsp.resize_window(id, direction, amount, area),
            LayoutKind::Dwindle => self.dwindle.resize_window(id, direction, amount, area),
            LayoutKind::Scrolling => self.scrolling.resize_window(id, direction, amount, area),
        }
    }
    fn set_environment(&mut self, environment: &LayoutEnvironment) {
        self.environment = environment.clone();
        self.scrolling.set_environment(environment);
    }
    fn order(&self) -> Vec<ManagedWindowId> {
        self.windows()
    }
    fn focus_changed(&mut self, id: ManagedWindowId, origin: FocusOrigin, area: Rect) -> bool {
        match self.kind {
            LayoutKind::Bsp => {
                self.bsp.set_focused(Some(id));
                false
            }
            LayoutKind::Dwindle => {
                self.dwindle.set_focused(Some(id));
                false
            }
            LayoutKind::Scrolling => self.scrolling.focus_changed(id, origin, area),
        }
    }
    fn neighbor(
        &self,
        id: ManagedWindowId,
        direction: Direction,
        _area: Rect,
    ) -> Option<Option<ManagedWindowId>> {
        match self.kind {
            LayoutKind::Scrolling => Some(self.scrolling.neighbor(id, direction)),
            _ => None,
        }
    }
    fn layout_command(
        &mut self,
        command: &StateCommand,
        focused: Option<ManagedWindowId>,
        area: Rect,
    ) -> Option<Result<(), CommandError>> {
        match command {
            StateCommand::SetLayout { layout } => Some(if self.set_kind(*layout) {
                if let (LayoutKind::Scrolling, Some(id)) = (*layout, focused) {
                    self.scrolling.focus_changed(id, FocusOrigin::Command, area);
                }
                Ok(())
            } else {
                Err(CommandError::InvalidArgument)
            }),
            StateCommand::CycleLayout => {
                let next = self.kind.next();
                self.set_kind(next);
                if let (LayoutKind::Scrolling, Some(id)) = (next, focused) {
                    self.scrolling.focus_changed(id, FocusOrigin::Command, area);
                }
                Some(Ok(()))
            }
            _ if self.kind == LayoutKind::Scrolling => {
                self.scrolling.layout_command(command, focused, area)
            }
            StateCommand::ScrollLayoutLeft
            | StateCommand::ScrollLayoutRight
            | StateCommand::CenterFocusedWindow
            | StateCommand::ResizeScrollingColumn { .. } => Some(Err(CommandError::Unsupported)),
            _ => None,
        }
    }
    fn is_parked(&self, id: ManagedWindowId, area: Rect) -> bool {
        self.kind == LayoutKind::Scrolling && self.scrolling.is_parked(id, area)
    }
    fn replace_window(&mut self, old: ManagedWindowId, new: ManagedWindowId) -> bool {
        // Rename in every model so switching kinds later keeps the slot too.
        let bsp = self.bsp.replace_window(old, new);
        let dwindle = self.dwindle.replace_window(old, new);
        let scrolling = self.scrolling.replace_window(old, new);
        match self.kind {
            LayoutKind::Bsp => bsp,
            LayoutKind::Dwindle => dwindle,
            LayoutKind::Scrolling => scrolling,
        }
    }
}
