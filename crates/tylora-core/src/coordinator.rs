use crate::{
    commands::{CommandError, Direction, StateCommand},
    layout::{FocusOrigin, LayoutEnvironment},
    navigation::neighbor,
    registry::{
        ManagedWindowId, ManagementMode, ProcessInstance, Registry, WindowLifecycle, WindowSnapshot,
    },
    DisplayId, Rect,
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tracing::debug;

pub trait LayoutPolicy {
    fn insert_window(&mut self, id: ManagedWindowId);
    fn remove_window(&mut self, id: ManagedWindowId);
    fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)>;
    fn set_window_min_size(&mut self, _id: ManagedWindowId, _width: f64, _height: f64) -> bool {
        false
    }
    fn swap_windows(&mut self, _first: ManagedWindowId, _second: ManagedWindowId) -> bool {
        false
    }
    fn resize_window(
        &mut self,
        _id: ManagedWindowId,
        _direction: Direction,
        _amount: f64,
        _area: Rect,
    ) -> bool {
        false
    }
    /// Physical surroundings of the display, for layouts that place windows
    /// beyond the work area.
    fn set_environment(&mut self, _environment: &LayoutEnvironment) {}
    /// Windows in layout order. Empty means "use geometric order".
    fn order(&self) -> Vec<ManagedWindowId> {
        Vec::new()
    }
    /// Records which window is focused. Returns true when the layout changed
    /// (for example a Scrolling viewport moved).
    fn focus_changed(&mut self, _id: ManagedWindowId, _origin: FocusOrigin, _area: Rect) -> bool {
        false
    }
    /// Layout-specific neighbour. `None` means the layout has no opinion and
    /// geometric navigation applies; `Some(None)` means there is no neighbour.
    fn neighbor(
        &self,
        _id: ManagedWindowId,
        _direction: Direction,
        _area: Rect,
    ) -> Option<Option<ManagedWindowId>> {
        None
    }
    /// Commands that only some layouts understand. `None` means unsupported.
    fn layout_command(
        &mut self,
        _command: &StateCommand,
        _focused: Option<ManagedWindowId>,
        _area: Rect,
    ) -> Option<Result<(), CommandError>> {
        None
    }
    /// True when the window is deliberately placed outside the viewport.
    fn is_parked(&self, _id: ManagedWindowId, _area: Rect) -> bool {
        false
    }
    /// Gives `old`'s slot (position, ratios, column width) to `new`: the same
    /// window re-identified under a new AX token. Returns false when
    /// unsupported; the caller then removes `old` and inserts `new`.
    fn replace_window(&mut self, _old: ManagedWindowId, _new: ManagedWindowId) -> bool {
        false
    }
}

/// Timing and failure counters for layout planning and AX application.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayoutMetrics {
    pub calculations: u64,
    pub calculation_total_us: u64,
    pub calculation_max_us: u64,
    pub applies: u64,
    pub apply_total_us: u64,
    pub apply_max_us: u64,
    pub ax_failures: u64,
    pub geometry_mismatches: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    None,
    Focus(ManagedWindowId),
    Reconcile,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApplyCommand {
    pub id: ManagedWindowId,
    pub generation: u64,
    pub version: u64,
    pub target: Rect,
}

pub enum CoordinatorEvent {
    Observed(WindowSnapshot),
    Destroyed(ManagedWindowId),
    ProcessExited(ProcessInstance),
    Reconciled {
        process: ProcessInstance,
        windows: Vec<WindowSnapshot>,
    },
    /// AX failure never removes state.
    ObservationFailed {
        process: ProcessInstance,
        code: i32,
    },
    Applied {
        command: ApplyCommand,
        result: Result<Rect, i32>,
    },
}

pub struct Coordinator<L: LayoutPolicy> {
    pub registry: Registry,
    pub layout: L,
    pub version: u64,
    pub stale_results: u64,
    pub display: DisplayId,
    pub work_area: Rect,
    tiled_ids: HashSet<ManagedWindowId>,
    floating_ids: HashSet<ManagedWindowId>,
    suspended_ids: HashSet<ManagedWindowId>,
    active_native_space: Option<u64>,
    environment: LayoutEnvironment,
    apply_started: HashMap<ManagedWindowId, Instant>,
    /// Windows a restored layout still holds while their observation is
    /// pending (a Space switch drops inactive-Space windows from the
    /// registry until the next scan). They keep their slot until the
    /// deadline, then leave the layout if they never reappear.
    awaiting: HashMap<ManagedWindowId, (Instant, Option<u32>)>,
    /// CGWindowID of windows tiled here. A window that left the registry
    /// (inactive Space) is re-scanned under a new AX token but keeps its
    /// CGWindowID, which links it back to its saved slot.
    known_cg: HashMap<ManagedWindowId, u32>,
    /// State version and zoom override of the last full plan, and the next
    /// instant a retry or placeholder expiry needs planning again. While
    /// nothing changed the plan is empty without recalculating the layout.
    planned: Option<(u64, Option<(ManagedWindowId, Rect)>)>,
    next_due: Option<Instant>,
    pub metrics: LayoutMetrics,
}

/// How long a restored layout keeps a slot for a window that has not been
/// observed again yet.
pub const AWAIT_REOBSERVATION: Duration = Duration::from_secs(3);

impl<L: LayoutPolicy> Coordinator<L> {
    pub fn new(layout: L, display: DisplayId, work_area: Rect) -> Self {
        Self {
            registry: Registry::default(),
            layout,
            version: 0,
            stale_results: 0,
            display,
            work_area,
            tiled_ids: HashSet::new(),
            floating_ids: HashSet::new(),
            suspended_ids: HashSet::new(),
            active_native_space: None,
            environment: LayoutEnvironment::default(),
            apply_started: HashMap::new(),
            awaiting: HashMap::new(),
            known_cg: HashMap::new(),
            planned: None,
            next_due: None,
            metrics: LayoutMetrics::default(),
        }
    }

    pub fn set_environment(&mut self, environment: LayoutEnvironment) {
        if self.environment != environment {
            self.layout.set_environment(&environment);
            self.environment = environment;
            self.version = self.version.wrapping_add(1);
        }
    }

    /// Forces the next plan to recalculate. Needed only after mutating the
    /// public `registry` or `layout` fields directly instead of through
    /// `handle`/`execute`, which bump `version` themselves.
    pub fn invalidate_plan(&mut self) {
        self.planned = None;
    }

    pub fn environment(&self) -> &LayoutEnvironment {
        &self.environment
    }

    /// Passive focus report from macOS or focus-follows-mouse.
    pub fn focus_hint(&mut self, id: ManagedWindowId, origin: FocusOrigin) -> bool {
        if !self.tiled_ids.contains(&id) {
            return false;
        }
        let changed = self.layout.focus_changed(id, origin, self.work_area);
        if changed {
            self.version = self.version.wrapping_add(1);
        }
        changed
    }

    pub fn is_parked(&self, id: ManagedWindowId) -> bool {
        self.tiled_ids.contains(&id) && self.layout.is_parked(id, self.work_area)
    }

    /// Windows in navigation order: the layout's own order when it has one,
    /// otherwise reading order of the computed frames.
    pub fn ordered_windows(&self) -> Vec<ManagedWindowId> {
        let order = self.layout.order();
        if !order.is_empty() {
            return order;
        }
        let mut frames = self.layout.calculate_layout(self.work_area);
        frames.sort_by(|a, b| {
            a.1.y
                .total_cmp(&b.1.y)
                .then(a.1.x.total_cmp(&b.1.x))
                .then(a.0.cmp(&b.0))
        });
        frames.into_iter().map(|(id, _)| id).collect()
    }

    fn eligible(&self, w: &WindowSnapshot) -> bool {
        w.display_id == Some(self.display)
            && self
                .active_native_space
                .is_none_or(|active| w.native_space_id.is_none_or(|id| id == active))
            && w.mode == ManagementMode::Tiled
            && !self.floating_ids.contains(&w.id)
            && !self.suspended_ids.contains(&w.id)
            && matches!(
                w.lifecycle,
                WindowLifecycle::Discovered | WindowLifecycle::Active
            )
    }

    fn sync_tree(&mut self) {
        let mut eligible: HashSet<_> = self
            .registry
            .iter()
            .filter(|(_, r)| self.eligible(&r.observed))
            .map(|(id, _)| *id)
            .collect();
        // AX can report both an old and a reacquired element for the same CG
        // window before a complete process scan. Tokens increase when the AX
        // worker creates them, so only the newest element may own a tile.
        let mut newest_by_cg = HashMap::new();
        for &id in &eligible {
            if let Some(cg) = self.registry.get(id).and_then(|r| r.observed.cg_window_id) {
                newest_by_cg
                    .entry((id.process, cg))
                    .and_modify(|newest: &mut ManagedWindowId| {
                        if id.token > newest.token {
                            *newest = id;
                        }
                    })
                    .or_insert(id);
                self.known_cg.insert(id, cg);
            }
        }
        eligible.retain(|id| {
            self.registry
                .get(*id)
                .and_then(|r| r.observed.cg_window_id)
                .is_none_or(|cg| newest_by_cg.get(&(id.process, cg)) == Some(id))
        });

        // Reassign a saved leaf before removing stale tokens. Removing first
        // collapses a Dwindle split and changes its order and ratio; adding
        // first briefly gives one physical window two tiles.
        let mut added: Vec<_> = eligible.difference(&self.tiled_ids).copied().collect();
        added.sort();
        for id in added {
            let Some(cg) = self.known_cg.get(&id).copied() else {
                continue;
            };
            let previous = self
                .awaiting
                .iter()
                .find(|(old, (_, slot))| {
                    **old != id && old.process == id.process && *slot == Some(cg)
                })
                .map(|(old, _)| *old)
                .or_else(|| {
                    self.tiled_ids.iter().copied().find(|old| {
                        *old != id
                            && old.process == id.process
                            && self.known_cg.get(old) == Some(&cg)
                    })
                });
            if let Some(old) = previous {
                self.awaiting.remove(&old);
                self.known_cg.remove(&old);
                if self.layout.replace_window(old, id) {
                    self.tiled_ids.remove(&old);
                    self.tiled_ids.insert(id);
                } else {
                    self.layout.remove_window(old);
                    self.tiled_ids.remove(&old);
                }
            }
        }
        let mut removed: Vec<_> = self.tiled_ids.difference(&eligible).copied().collect();
        removed.sort();
        for id in removed {
            self.layout.remove_window(id);
            if let Some(record) = self.registry.get_mut(id) {
                record.desired = None;
                record.applied = None;
                record.pending_apply = None;
                record.failed_target = None;
                record.failed_attempts = 0;
                record.retry_after = None;
            }
        }
        let mut added: Vec<_> = eligible.difference(&self.tiled_ids).copied().collect();
        added.sort();
        for id in added {
            // A window returning to a restored layout keeps its old slot:
            // inserting an existing member is a no-op in every layout.
            if self.awaiting.remove(&id).is_some() {
                self.layout.insert_window(id);
                continue;
            }
            self.layout.insert_window(id);
        }
        self.tiled_ids = eligible;
        self.expire_awaiting(Instant::now());
    }

    /// Drops placeholder slots whose window did not come back in time, or
    /// whose window is known again but no longer belongs to this layout.
    pub fn expire_awaiting(&mut self, now: Instant) -> bool {
        let stale: Vec<_> = self
            .awaiting
            .iter()
            .filter(|(id, (deadline, _))| *deadline <= now || self.registry.get(**id).is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.awaiting.remove(id);
            if !self.tiled_ids.contains(id) {
                self.layout.remove_window(*id);
            }
        }
        if !stale.is_empty() {
            self.version = self.version.wrapping_add(1);
        }
        !stale.is_empty()
    }

    pub fn awaiting(&self) -> usize {
        self.awaiting.len()
    }

    /// Whether this coordinator explicitly tracks the window as floating.
    /// This survives snapshots that report the default tiled mode while a
    /// window is being reconciled on another display.
    pub fn is_floating(&self, id: ManagedWindowId) -> bool {
        self.floating_ids.contains(&id)
    }

    /// Single writer entry point. No platform calls are made here.
    pub fn handle(&mut self, event: CoordinatorEvent) -> bool {
        self.version = self.version.wrapping_add(1);
        let mut changed = false;
        match event {
            CoordinatorEvent::Observed(mut w) => {
                if self.floating_ids.contains(&w.id) && w.mode == ManagementMode::Tiled {
                    w.mode = ManagementMode::Floating;
                }
                let id = w.id;
                changed = self
                    .registry
                    .get(id)
                    .map(|r| r.observed != w)
                    .unwrap_or(true);
                self.registry.upsert(w, self.version);
            }
            CoordinatorEvent::Destroyed(id) => {
                changed = self.registry.remove(id).is_some();
                self.floating_ids.remove(&id);
                self.suspended_ids.remove(&id);
            }
            CoordinatorEvent::ProcessExited(process) => {
                changed = !self.registry.remove_process(process).is_empty();
                self.floating_ids.retain(|id| id.process != process);
                self.suspended_ids.retain(|id| id.process != process);
                self.known_cg.retain(|id, _| id.process != process);
            }
            CoordinatorEvent::Reconciled {
                process,
                mut windows,
            } => {
                for window in &mut windows {
                    if self.floating_ids.contains(&window.id)
                        && window.mode == ManagementMode::Tiled
                    {
                        window.mode = ManagementMode::Floating;
                    }
                }
                let before: Vec<_> = self
                    .registry
                    .iter()
                    .filter(|(id, _)| id.process == process)
                    .map(|(id, r)| (*id, r.observed.clone()))
                    .collect();
                let removed = self
                    .registry
                    .reconcile_process(process, &windows, self.version);
                for id in &removed {
                    self.floating_ids.remove(id);
                    self.suspended_ids.remove(id);
                }
                changed = !removed.is_empty()
                    || before.len() != windows.len()
                    || windows.iter().any(|w| {
                        before
                            .iter()
                            .find(|(id, _)| *id == w.id)
                            .map(|(_, b)| b != w)
                            .unwrap_or(true)
                    });
            }
            CoordinatorEvent::ObservationFailed { .. } => {}
            CoordinatorEvent::Applied { command, result } => {
                if let Some(started) = self.apply_started.remove(&command.id) {
                    let elapsed = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
                    self.metrics.applies = self.metrics.applies.saturating_add(1);
                    self.metrics.apply_total_us =
                        self.metrics.apply_total_us.saturating_add(elapsed);
                    self.metrics.apply_max_us = self.metrics.apply_max_us.max(elapsed);
                }
                match &result {
                    Err(_) => self.metrics.ax_failures = self.metrics.ax_failures.saturating_add(1),
                    Ok(actual) if !actual.near(command.target, 1.0) => {
                        self.metrics.geometry_mismatches =
                            self.metrics.geometry_mismatches.saturating_add(1)
                    }
                    Ok(_) => {}
                }
                let mut minimum = None;
                if let Some(record) = self.registry.get_mut(command.id) {
                    if record.generation == command.generation
                        && record.pending_apply == Some(command.version)
                    {
                        record.pending_apply = None;
                        match result {
                            Ok(actual) => {
                                record.applied = Some(actual);
                                record.observed.frame = actual;
                                if !actual.near(command.target, 1.0) {
                                    record.failed_target = Some(command.target);
                                    record.failed_attempts =
                                        record.failed_attempts.saturating_add(1);
                                    record.retry_after =
                                        Some(Instant::now() + retry_delay(record.failed_attempts));
                                    if record.failed_attempts >= 3 {
                                        minimum = Some((
                                            if actual.width > command.target.width + 1.0 {
                                                actual.width.min(self.work_area.width)
                                            } else {
                                                0.0
                                            },
                                            if actual.height > command.target.height + 1.0 {
                                                actual.height.min(self.work_area.height)
                                            } else {
                                                0.0
                                            },
                                        ));
                                    }
                                } else {
                                    record.failed_target = None;
                                    record.failed_attempts = 0;
                                    record.retry_after = None;
                                }
                            }
                            Err(_) => {
                                record.applied = None;
                                record.failed_target = Some(command.target);
                                record.failed_attempts = record.failed_attempts.saturating_add(1);
                                record.retry_after =
                                    Some(Instant::now() + retry_delay(record.failed_attempts));
                            }
                        }
                    } else {
                        self.stale_results = self.stale_results.saturating_add(1);
                        debug!(
                            pid = command.id.process.pid,
                            token = command.id.token,
                            "stale AX result ignored"
                        );
                    }
                } else {
                    self.stale_results = self.stale_results.saturating_add(1);
                    debug!(
                        pid = command.id.process.pid,
                        token = command.id.token,
                        "AX result for removed window ignored"
                    );
                }
                if let Some((width, height)) = minimum {
                    changed |= self.layout.set_window_min_size(command.id, width, height);
                }
            }
        }
        if changed {
            self.sync_tree();
        }
        changed
    }

    pub fn set_work_area(&mut self, display: DisplayId, area: Rect) {
        self.version = self.version.wrapping_add(1);
        self.display = display;
        self.work_area = area;
        self.sync_tree();
    }

    /// Install the active workspace's saved tree, then synchronize it against
    /// the current registry while retaining its existing split ratios.
    pub fn replace_layout(&mut self, layout: L) {
        self.layout = layout;
        self.layout.set_environment(&self.environment);
        self.awaiting.clear();
        let deadline = Instant::now() + AWAIT_REOBSERVATION;
        for (id, _) in self.layout.calculate_layout(self.work_area) {
            match self.registry.get(id) {
                Some(record) if self.eligible(&record.observed) => {}
                Some(_) => {
                    self.layout.remove_window(id);
                }
                None => {
                    let cg = self.known_cg.get(&id).copied();
                    self.awaiting.insert(id, (deadline, cg));
                }
            }
        }
        self.tiled_ids.clear();
        self.version = self.version.wrapping_add(1);
        self.sync_tree();
    }

    pub fn set_active_native_space(&mut self, space: Option<u64>) {
        if self.active_native_space != space {
            self.active_native_space = space;
            self.version = self.version.wrapping_add(1);
            self.sync_tree();
        }
    }

    /// Temporarily remove a window from BSP while an asynchronous cross-display
    /// AX move is in flight. Reconciliation or the caller later resumes it.
    pub fn suspend(&mut self, id: ManagedWindowId) -> Result<(), CommandError> {
        if self.registry.get(id).is_none() {
            return Err(CommandError::WindowNotFound);
        }
        if self.suspended_ids.insert(id) {
            self.version = self.version.wrapping_add(1);
            self.sync_tree();
        }
        Ok(())
    }
    pub fn resume(&mut self, id: ManagedWindowId) {
        if self.suspended_ids.remove(&id) {
            self.version = self.version.wrapping_add(1);
            self.sync_tree();
        }
    }

    /// Runs pure command logic on the single writer. Platform effects are
    /// returned to the caller and performed asynchronously by the adapter.
    pub fn execute(
        &mut self,
        command: &StateCommand,
        focused: Option<ManagedWindowId>,
    ) -> Result<CommandEffect, CommandError> {
        match command {
            StateCommand::FocusWindow { direction } | StateCommand::MoveWindow { direction } => {
                let source = focused.ok_or(CommandError::NoFocusedWindow)?;
                if !self.tiled_ids.contains(&source) {
                    return Err(CommandError::WindowNotFound);
                }
                let started = Instant::now();
                let target = match self.layout.neighbor(source, *direction, self.work_area) {
                    Some(target) => target,
                    None => neighbor(
                        source,
                        *direction,
                        self.layout.calculate_layout(self.work_area),
                    ),
                }
                .ok_or(CommandError::NoNeighbor)?;
                debug!(
                    elapsed_us = started.elapsed().as_micros(),
                    ?direction,
                    "directional neighbor selected"
                );
                if matches!(command, StateCommand::MoveWindow { .. }) {
                    if !self.layout.swap_windows(source, target) {
                        return Err(CommandError::Unsupported);
                    }
                    self.layout
                        .focus_changed(source, FocusOrigin::Command, self.work_area);
                    self.version = self.version.wrapping_add(1);
                    Ok(CommandEffect::None)
                } else {
                    if self
                        .layout
                        .focus_changed(target, FocusOrigin::Command, self.work_area)
                    {
                        self.version = self.version.wrapping_add(1);
                    }
                    Ok(CommandEffect::Focus(target))
                }
            }
            StateCommand::FocusNextWindow | StateCommand::FocusPreviousWindow => {
                let order = self.ordered_windows();
                if order.is_empty() {
                    return Err(CommandError::NoNeighbor);
                }
                let index = focused.and_then(|id| order.iter().position(|w| *w == id));
                let next = match (command, index) {
                    (StateCommand::FocusNextWindow, Some(index)) => (index + 1) % order.len(),
                    (StateCommand::FocusPreviousWindow, Some(index)) => {
                        (index + order.len() - 1) % order.len()
                    }
                    (StateCommand::FocusPreviousWindow, None) => order.len() - 1,
                    _ => 0,
                };
                let target = order[next];
                if Some(target) == focused {
                    return Err(CommandError::NoNeighbor);
                }
                if self
                    .layout
                    .focus_changed(target, FocusOrigin::Command, self.work_area)
                {
                    self.version = self.version.wrapping_add(1);
                }
                Ok(CommandEffect::Focus(target))
            }
            StateCommand::SwapWindow { first, second } => {
                if !self.layout.swap_windows(*first, *second) {
                    return Err(CommandError::WindowNotFound);
                }
                self.version = self.version.wrapping_add(1);
                Ok(CommandEffect::None)
            }
            StateCommand::ResizeWindow { direction, amount } => {
                let source = focused.ok_or(CommandError::NoFocusedWindow)?;
                if !amount.is_finite() || *amount <= 0.0 {
                    return Err(CommandError::InvalidArgument);
                }
                if !self
                    .layout
                    .resize_window(source, *direction, *amount, self.work_area)
                {
                    return Err(CommandError::NoNeighbor);
                }
                self.version = self.version.wrapping_add(1);
                Ok(CommandEffect::None)
            }
            StateCommand::ToggleFloating { window } => {
                let id = (*window).or(focused).ok_or(CommandError::NoFocusedWindow)?;
                let record = self
                    .registry
                    .get_mut(id)
                    .ok_or(CommandError::WindowNotFound)?;
                if !matches!(
                    record.observed.lifecycle,
                    WindowLifecycle::Active | WindowLifecycle::Discovered
                ) || record.observed.mode == ManagementMode::Unmanaged
                {
                    return Err(CommandError::Unsupported);
                }
                if self.floating_ids.remove(&id) {
                    record.observed.mode = ManagementMode::Tiled;
                } else {
                    self.floating_ids.insert(id);
                    record.observed.mode = ManagementMode::Floating;
                }
                self.version = self.version.wrapping_add(1);
                self.sync_tree();
                Ok(CommandEffect::None)
            }
            StateCommand::ReconcileWindows => Ok(CommandEffect::Reconcile),
            _ => {
                let focused = focused.filter(|id| self.tiled_ids.contains(id));
                match self.layout.layout_command(command, focused, self.work_area) {
                    Some(result) => {
                        result?;
                        self.version = self.version.wrapping_add(1);
                        Ok(CommandEffect::None)
                    }
                    None => Err(CommandError::Unsupported),
                }
            }
        }
    }

    /// Suppresses repeated writes while a matching result is pending or already applied.
    pub fn plan(&mut self) -> Vec<ApplyCommand> {
        self.plan_with_override(None)
    }

    /// Keep the BSP tree intact while one tiled window uses a temporary frame.
    pub fn plan_with_override(
        &mut self,
        override_frame: Option<(ManagedWindowId, crate::Rect)>,
    ) -> Vec<ApplyCommand> {
        let now = Instant::now();
        if self.planned == Some((self.version, override_frame))
            && self.next_due.is_none_or(|due| now < due)
        {
            return Vec::new();
        }
        if !self.awaiting.is_empty() {
            self.expire_awaiting(now);
        }
        let mut effects = Vec::new();
        let started = Instant::now();
        let frames = self.layout.calculate_layout(self.work_area);
        let elapsed = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.metrics.calculations = self.metrics.calculations.saturating_add(1);
        self.metrics.calculation_total_us =
            self.metrics.calculation_total_us.saturating_add(elapsed);
        self.metrics.calculation_max_us = self.metrics.calculation_max_us.max(elapsed);
        for (id, normal_target) in frames {
            let target = override_frame
                .filter(|(zoomed, _)| *zoomed == id)
                .map_or(normal_target, |(_, frame)| frame);
            if let Some(record) = self.registry.get_mut(id) {
                if record.desired != Some(target) {
                    record.desired = Some(target);
                    record.pending_apply = None;
                    record.failed_target = None;
                    record.failed_attempts = 0;
                    record.retry_after = None;
                }
                if record.pending_apply.is_some()
                    || record.applied.is_some_and(|r| r.near(target, 1.0))
                    || record.observed.frame.near(target, 1.0)
                    || (record.failed_target == Some(target)
                        && record.retry_after.is_some_and(|due| Instant::now() < due))
                {
                    continue;
                }
                effects.push(ApplyCommand {
                    id,
                    generation: record.generation,
                    version: self.version,
                    target,
                });
                record.pending_apply = Some(self.version);
                self.apply_started.insert(id, Instant::now());
            }
        }
        if self.apply_started.len() > 4 * self.registry.len().max(16) {
            let live: HashSet<_> = self.registry.iter().map(|(id, _)| *id).collect();
            self.apply_started.retain(|id, _| live.contains(id));
        }
        self.planned = Some((self.version, override_frame));
        self.next_due = self
            .registry
            .iter()
            .filter(|(_, record)| record.failed_target.is_some())
            .filter_map(|(_, record)| record.retry_after)
            .chain(self.awaiting.values().map(|(deadline, _)| *deadline))
            .min();
        effects
    }
}

fn retry_delay(attempt: u8) -> Duration {
    match attempt {
        1 => Duration::from_millis(250),
        2 => Duration::from_millis(750),
        3 => Duration::from_secs(5),
        4 => Duration::from_secs(15),
        _ => Duration::from_secs(60),
    }
}
