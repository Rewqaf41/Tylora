//! Scrolling layout: windows form a horizontal strip of columns that may be
//! wider than the display, viewed through a viewport.
//!
//! The model has two deliberately separate stages:
//!
//! 1. `view()` is the virtual strip: column positions in strip coordinates and
//!    the viewport offset. It is pure arithmetic over fractions of the usable
//!    width, so it scales with the display and is fully deterministic.
//! 2. `calculate_layout()` maps that strip to frames macOS can actually
//!    honour. WindowServer refuses to move a window fully off every display
//!    (it silently keeps about 40 pt visible; measured with
//!    `examples/offscreen_probe.rs`), so a column outside the viewport is
//!    parked in a bottom corner of its own display with a small sliver
//!    visible. A park position whose off-display part would land on another
//!    display is rejected; when no safe position exists the layout falls back
//!    to a bounded mode that fits every column inside the viewport.

use std::collections::HashMap;
use tylora_core::{
    commands::{CommandError, Direction, StateCommand},
    layout::{FocusOrigin, LayoutEnvironment, ScrollingSettings, ViewportAlignment},
    registry::ManagedWindowId,
    Rect,
};

/// Width of the sliver WindowServer keeps on screen at a horizontal edge.
pub const PARK_VISIBLE_WIDTH: f64 = 40.0;
/// Height kept on screen at the bottom edge.
pub const PARK_VISIBLE_HEIGHT: f64 = 32.0;
/// Columns are never narrower than this many points.
const MIN_COLUMN_POINTS: f64 = 80.0;
const EPSILON: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Column {
    id: ManagedWindowId,
    /// Fraction of the usable width.
    width: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollingMode {
    /// Columns outside the viewport are parked at a display edge.
    Strip,
    /// No safe park position exists (another display is adjacent on every
    /// candidate side), so all columns are compressed into the viewport.
    Bounded,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ColumnPlacement {
    Visible,
    /// Partly outside the viewport but at least a sliver stays visible.
    Partial,
    Parked,
}

/// Pure virtual-strip state for one work area.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ScrollingView {
    /// Usable area inside the outer gap.
    pub usable: Rect,
    /// Viewport offset in strip points.
    pub offset: f64,
    /// Total strip width in points.
    pub strip_width: f64,
    /// `(window, strip x, width)` in strip points, in column order.
    pub columns: Vec<(ManagedWindowId, f64, f64)>,
    pub active: Option<ManagedWindowId>,
}

#[derive(Debug, Clone)]
pub struct ScrollingLayout {
    columns: Vec<Column>,
    active: Option<ManagedWindowId>,
    /// Viewport offset as a fraction of the usable width.
    offset: f64,
    /// A newly inserted column that must be revealed the next time a work
    /// area is known. Insertion itself has no area.
    reveal: Option<ManagedWindowId>,
    settings: ScrollingSettings,
    inner_gap: f64,
    outer_gap: f64,
    window_min_size: HashMap<ManagedWindowId, (f64, f64)>,
    environment: LayoutEnvironment,
    /// Increments on every state change; useful for diagnostics.
    pub state_version: u64,
    pub viewport_calculations: u64,
    pub viewport_total_us: u64,
}

impl Default for ScrollingLayout {
    fn default() -> Self {
        Self::new(ScrollingSettings::default())
    }
}

impl ScrollingLayout {
    pub fn new(settings: ScrollingSettings) -> Self {
        Self {
            columns: Vec::new(),
            active: None,
            offset: 0.0,
            reveal: None,
            settings,
            inner_gap: 8.0,
            outer_gap: 8.0,
            window_min_size: HashMap::new(),
            environment: LayoutEnvironment::default(),
            state_version: 0,
            viewport_calculations: 0,
            viewport_total_us: 0,
        }
    }

    pub fn settings(&self) -> &ScrollingSettings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: ScrollingSettings) {
        if self.settings == settings {
            return;
        }
        for column in &mut self.columns {
            column.width = column
                .width
                .clamp(settings.min_column_width, settings.max_column_width);
        }
        self.settings = settings;
        self.bump();
    }

    pub fn set_gaps(&mut self, inner: f64, outer: f64) {
        if inner.is_finite() && outer.is_finite() && inner >= 0.0 && outer >= 0.0 {
            self.inner_gap = inner;
            self.outer_gap = outer;
        }
    }

    pub fn set_environment(&mut self, environment: &LayoutEnvironment) {
        self.environment = environment.clone();
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn windows(&self) -> Vec<ManagedWindowId> {
        self.columns.iter().map(|column| column.id).collect()
    }

    pub fn contains(&self, id: ManagedWindowId) -> bool {
        self.columns.iter().any(|column| column.id == id)
    }

    pub fn active(&self) -> Option<ManagedWindowId> {
        self.active
    }

    /// Marks a column active and reveals it on the next area-aware call.
    pub fn set_active(&mut self, id: ManagedWindowId) {
        if self.contains(id) {
            self.active = Some(id);
            self.reveal = Some(id);
            self.bump();
        }
    }

    pub fn column_width(&self, id: ManagedWindowId) -> Option<f64> {
        self.columns
            .iter()
            .find(|column| column.id == id)
            .map(|column| column.width)
    }

    fn bump(&mut self) {
        self.state_version = self.state_version.wrapping_add(1);
    }

    fn index(&self, id: ManagedWindowId) -> Option<usize> {
        self.columns.iter().position(|column| column.id == id)
    }

    /// Inserts right of the active column (at the end when there is none),
    /// makes it active and reveals it once a work area is known.
    pub fn insert_window(&mut self, id: ManagedWindowId) -> bool {
        self.insert_with_width(id, self.settings.default_column_width)
    }

    /// Inserts at the end with an explicit width; used when converting from
    /// another layout so the original order is kept.
    pub fn push_window(&mut self, id: ManagedWindowId, width: Option<f64>) -> bool {
        if self.contains(id) {
            return false;
        }
        let width = width.unwrap_or(self.settings.default_column_width).clamp(
            self.settings.min_column_width,
            self.settings.max_column_width,
        );
        self.columns.push(Column { id, width });
        if self.active.is_none() {
            self.active = Some(id);
        }
        self.bump();
        true
    }

    fn insert_with_width(&mut self, id: ManagedWindowId, width: f64) -> bool {
        if self.contains(id) {
            return false;
        }
        let width = width.clamp(
            self.settings.min_column_width,
            self.settings.max_column_width,
        );
        let at = self
            .active
            .and_then(|active| self.index(active))
            .map_or(self.columns.len(), |index| index + 1);
        self.columns.insert(at, Column { id, width });
        self.active = Some(id);
        self.reveal = Some(id);
        self.bump();
        true
    }

    pub fn remove_window(&mut self, id: ManagedWindowId) -> bool {
        let Some(index) = self.index(id) else {
            return false;
        };
        self.columns.remove(index);
        self.window_min_size.remove(&id);
        if self.reveal == Some(id) {
            self.reveal = None;
        }
        if self.active == Some(id) {
            // Focus moves to the column that took its place, else the left one.
            self.active = self
                .columns
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|left| self.columns.get(left)))
                .map(|column| column.id);
        }
        self.bump();
        true
    }

    pub fn swap_windows(&mut self, first: ManagedWindowId, second: ManagedWindowId) -> bool {
        let (Some(a), Some(b)) = (self.index(first), self.index(second)) else {
            return false;
        };
        if a == b {
            return false;
        }
        self.columns.swap(a, b);
        self.bump();
        true
    }

    /// Renames a column, keeping its position, width and the viewport.
    pub fn replace_window(&mut self, old: ManagedWindowId, new: ManagedWindowId) -> bool {
        if old == new || self.contains(new) {
            return false;
        }
        let Some(index) = self.index(old) else {
            return false;
        };
        self.columns[index].id = new;
        if let Some(size) = self.window_min_size.remove(&old) {
            self.window_min_size.insert(new, size);
        }
        if self.active == Some(old) {
            self.active = Some(new);
        }
        if self.reveal == Some(old) {
            self.reveal = Some(new);
        }
        self.bump();
        true
    }

    pub fn set_window_min_size(&mut self, id: ManagedWindowId, width: f64, height: f64) -> bool {
        if !width.is_finite() || !height.is_finite() || width < 0.0 || height < 0.0 {
            return false;
        }
        if !self.contains(id) {
            return false;
        }
        let current = self.window_min_size.entry(id).or_insert((0.0, 0.0));
        let next = (current.0.max(width), current.1.max(height));
        if next == *current {
            return false;
        }
        *current = next;
        self.bump();
        true
    }

    fn usable(&self, area: Rect) -> Rect {
        let gap_x = self.outer_gap.min(area.width / 2.0);
        let gap_y = self.outer_gap.min(area.height / 2.0);
        Rect::new(
            area.x + gap_x,
            area.y + gap_y,
            (area.width - 2.0 * gap_x).max(1.0),
            (area.height - 2.0 * gap_y).max(1.0),
        )
        .expect("finite inset")
    }

    fn column_points(&self, column: &Column, usable_width: f64) -> f64 {
        let learned = self
            .window_min_size
            .get(&column.id)
            .map_or(0.0, |size| size.0);
        let minimum = (self.settings.min_column_width * usable_width)
            .max(MIN_COLUMN_POINTS.min(usable_width))
            .max(learned);
        let maximum = (self.settings.max_column_width * usable_width).max(minimum);
        (column.width * usable_width).clamp(minimum, maximum)
    }

    /// Column positions in strip points, without any viewport.
    fn strip(&self, usable_width: f64) -> (Vec<(ManagedWindowId, f64, f64)>, f64) {
        let mut x = 0.0;
        let mut out = Vec::with_capacity(self.columns.len());
        for (index, column) in self.columns.iter().enumerate() {
            if index > 0 {
                x += self.inner_gap;
            }
            let width = self.column_points(column, usable_width);
            out.push((column.id, x, width));
            x += width;
        }
        (out, x)
    }

    fn clamp_offset(
        &self,
        offset: f64,
        columns: &[(ManagedWindowId, f64, f64)],
        total: f64,
        usable_width: f64,
    ) -> f64 {
        match self.settings.viewport_alignment {
            ViewportAlignment::Nearest => offset.clamp(0.0, (total - usable_width).max(0.0)),
            ViewportAlignment::Center => {
                let centre = |entry: &(ManagedWindowId, f64, f64)| entry.1 + entry.2 / 2.0;
                let (Some(first), Some(last)) = (columns.first(), columns.last()) else {
                    return 0.0;
                };
                let low = centre(first) - usable_width / 2.0;
                let high = (centre(last) - usable_width / 2.0).max(low);
                offset.clamp(low, high)
            }
        }
    }

    fn offset_revealing(
        &self,
        offset: f64,
        target: ManagedWindowId,
        columns: &[(ManagedWindowId, f64, f64)],
        usable_width: f64,
        alignment: ViewportAlignment,
    ) -> f64 {
        let Some(&(_, x, width)) = columns.iter().find(|entry| entry.0 == target) else {
            return offset;
        };
        match alignment {
            ViewportAlignment::Center => x + width / 2.0 - usable_width / 2.0,
            ViewportAlignment::Nearest => {
                if width >= usable_width || x < offset {
                    x
                } else if x + width > offset + usable_width {
                    x + width - usable_width
                } else {
                    offset
                }
            }
        }
    }

    /// The pure viewport. A pending reveal of a new column is folded in, so
    /// this is the state the next mutation will settle to.
    pub fn view(&self, area: Rect) -> ScrollingView {
        let usable = self.usable(area);
        let (columns, total) = self.strip(usable.width);
        let mut offset = self.offset * usable.width;
        if let Some(reveal) = self.reveal {
            offset = self.offset_revealing(
                offset,
                reveal,
                &columns,
                usable.width,
                self.settings.viewport_alignment,
            );
        }
        let offset = self.clamp_offset(offset, &columns, total, usable.width);
        ScrollingView {
            usable,
            offset,
            strip_width: total,
            columns,
            active: self.active,
        }
    }

    /// Commits the pure viewport (pending reveal and clamping) into state.
    fn settle(&mut self, area: Rect) -> ScrollingView {
        let started = std::time::Instant::now();
        let view = self.view(area);
        self.offset = view.offset / view.usable.width;
        self.reveal = None;
        self.viewport_calculations = self.viewport_calculations.saturating_add(1);
        self.viewport_total_us = self
            .viewport_total_us
            .saturating_add(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
        view
    }

    fn set_offset_points(&mut self, view: &ScrollingView, offset: f64) -> bool {
        let offset = self.clamp_offset(offset, &view.columns, view.strip_width, view.usable.width);
        let fraction = offset / view.usable.width;
        if (fraction - self.offset).abs() < 1e-9 {
            return false;
        }
        self.offset = fraction;
        self.bump();
        true
    }

    /// Scrolls so `id` is visible according to `alignment`.
    fn reveal_now(
        &mut self,
        id: ManagedWindowId,
        area: Rect,
        alignment: ViewportAlignment,
    ) -> bool {
        let view = self.settle(area);
        let offset =
            self.offset_revealing(view.offset, id, &view.columns, view.usable.width, alignment);
        self.set_offset_points(&view, offset)
    }

    /// Placement of every column for the current viewport.
    pub fn placements(
        &self,
        area: Rect,
    ) -> (ScrollingMode, Vec<(ManagedWindowId, Rect, ColumnPlacement)>) {
        let view = self.view(area);
        let display = self.environment.display_frame.unwrap_or(area);
        let others = &self.environment.other_displays;
        let blocked = |rect: Rect| others.iter().any(|other| intersects(rect, *other));
        let mut out = Vec::with_capacity(view.columns.len());
        for &(id, x, width) in &view.columns {
            let frame = Rect::new(
                view.usable.x + x - view.offset,
                view.usable.y,
                width,
                view.usable.height,
            )
            .expect("finite column");
            let inside = frame.x >= display.x - EPSILON
                && frame.x + frame.width <= display.x + display.width + EPSILON;
            if inside {
                out.push((id, frame, ColumnPlacement::Visible));
                continue;
            }
            let visible =
                (frame.x + frame.width).min(display.x + display.width) - frame.x.max(display.x);
            if visible >= PARK_VISIBLE_WIDTH && !blocked(frame) {
                out.push((id, frame, ColumnPlacement::Partial));
                continue;
            }
            let right_side = frame.x + frame.width / 2.0 >= view.usable.x + view.usable.width / 2.0;
            let park = |right: bool| {
                let x = if right {
                    display.x + display.width - PARK_VISIBLE_WIDTH
                } else {
                    display.x - frame.width + PARK_VISIBLE_WIDTH
                };
                Rect::new(
                    x,
                    display.y + display.height - PARK_VISIBLE_HEIGHT,
                    frame.width,
                    frame.height,
                )
                .expect("finite park")
            };
            let preferred = park(right_side);
            let fallback = park(!right_side);
            if !blocked(preferred) {
                out.push((id, preferred, ColumnPlacement::Parked));
            } else if !blocked(fallback) {
                out.push((id, fallback, ColumnPlacement::Parked));
            } else {
                return (ScrollingMode::Bounded, self.bounded(&view));
            }
        }
        (ScrollingMode::Strip, out)
    }

    /// Every column fits inside the viewport, keeping relative widths.
    fn bounded(&self, view: &ScrollingView) -> Vec<(ManagedWindowId, Rect, ColumnPlacement)> {
        let count = view.columns.len();
        if count == 0 {
            return Vec::new();
        }
        let gaps = self.inner_gap * (count.saturating_sub(1)) as f64;
        let available = (view.usable.width - gaps).max(count as f64);
        let total: f64 = view.columns.iter().map(|entry| entry.2).sum();
        let mut x = view.usable.x;
        view.columns
            .iter()
            .map(|&(id, _, width)| {
                let width = (width / total * available).max(1.0);
                let frame =
                    Rect::new(x, view.usable.y, width, view.usable.height).expect("finite column");
                x += width + self.inner_gap;
                (id, frame, ColumnPlacement::Visible)
            })
            .collect()
    }

    pub fn mode(&self, area: Rect) -> ScrollingMode {
        self.placements(area).0
    }

    pub fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)> {
        self.placements(area)
            .1
            .into_iter()
            .map(|(id, frame, _)| (id, frame))
            .collect()
    }

    pub fn is_parked(&self, id: ManagedWindowId, area: Rect) -> bool {
        self.placements(area)
            .1
            .iter()
            .any(|(window, _, placement)| *window == id && *placement == ColumnPlacement::Parked)
    }

    /// A frame inside the usable area with the column's size; used to bring
    /// a window back on screen after leaving the strip.
    pub fn safe_frame(&self, id: ManagedWindowId, area: Rect) -> Option<Rect> {
        let view = self.view(area);
        let &(_, _, width) = view.columns.iter().find(|entry| entry.0 == id)?;
        let width = width.min(view.usable.width);
        Rect::new(view.usable.x, view.usable.y, width, view.usable.height).ok()
    }

    pub fn neighbor(&self, id: ManagedWindowId, direction: Direction) -> Option<ManagedWindowId> {
        let index = self.index(id)?;
        match direction {
            Direction::Left => index.checked_sub(1).map(|left| self.columns[left].id),
            Direction::Right => self.columns.get(index + 1).map(|column| column.id),
            Direction::Up | Direction::Down => None,
        }
    }

    /// Mouse and keyboard resize: only horizontal edges exist.
    pub fn resize_window(
        &mut self,
        id: ManagedWindowId,
        direction: Direction,
        amount: f64,
        area: Rect,
    ) -> bool {
        if !amount.is_finite() || amount <= 0.0 {
            return false;
        }
        let delta = match direction {
            Direction::Right => amount,
            Direction::Left => -amount,
            Direction::Up | Direction::Down => return false,
        };
        let usable = self.usable(area).width;
        self.settle(area);
        self.change_width(id, delta / usable)
    }

    fn change_width(&mut self, id: ManagedWindowId, delta: f64) -> bool {
        let (min, max) = (
            self.settings.min_column_width,
            self.settings.max_column_width,
        );
        let Some(column) = self.columns.iter_mut().find(|column| column.id == id) else {
            return false;
        };
        let next = (column.width + delta).clamp(min, max);
        if (next - column.width).abs() < 1e-9 {
            return false;
        }
        column.width = next;
        self.bump();
        true
    }

    fn cycle_width(&mut self, id: ManagedWindowId) -> bool {
        let Some(current) = self.column_width(id) else {
            return false;
        };
        let mut presets = self.settings.column_width_presets.clone();
        presets.sort_by(f64::total_cmp);
        let next = presets
            .iter()
            .copied()
            .find(|preset| *preset > current + 1e-3)
            .or_else(|| presets.first().copied())
            .unwrap_or(current);
        self.change_width(id, next - current)
    }

    pub fn focus_changed(&mut self, id: ManagedWindowId, origin: FocusOrigin, area: Rect) -> bool {
        if !self.contains(id) {
            return false;
        }
        let parked = self.is_parked(id, area);
        let mut changed = false;
        if self.active != Some(id) {
            self.active = Some(id);
            self.bump();
            changed = true;
        }
        let scroll = match origin {
            FocusOrigin::Command => self.settings.focus_auto_scroll,
            // A click or Cmd-Tab on a parked window must bring it into view;
            // a window that is already visible never moves the strip, so
            // passive focus cannot start a scroll/focus feedback loop.
            FocusOrigin::System => parked,
            // Focus-follows-mouse only reaches visible windows.
            FocusOrigin::Pointer => false,
        };
        if scroll {
            changed |= self.reveal_now(id, area, self.settings.viewport_alignment);
        } else {
            self.settle(area);
        }
        changed
    }

    pub fn layout_command(
        &mut self,
        command: &StateCommand,
        focused: Option<ManagedWindowId>,
        area: Rect,
    ) -> Option<Result<(), CommandError>> {
        let target = focused.filter(|id| self.contains(*id)).or(self.active);
        Some(match command {
            StateCommand::ScrollLayoutLeft | StateCommand::ScrollLayoutRight => {
                if self.columns.is_empty() {
                    return Some(Err(CommandError::NoFocusedWindow));
                }
                let view = self.settle(area);
                let step = self.settings.scroll_sensitivity * view.usable.width;
                let step = if matches!(command, StateCommand::ScrollLayoutLeft) {
                    -step
                } else {
                    step
                };
                if self.set_offset_points(&view, view.offset + step) {
                    Ok(())
                } else {
                    Err(CommandError::NoNeighbor)
                }
            }
            StateCommand::CenterFocusedWindow => match target {
                Some(id) => {
                    self.active = Some(id);
                    self.reveal_now(id, area, ViewportAlignment::Center);
                    Ok(())
                }
                None => Err(CommandError::NoFocusedWindow),
            },
            StateCommand::ResizeScrollingColumn { delta } => match target {
                Some(id) => {
                    self.settle(area);
                    let changed = match delta {
                        Some(delta) if delta.is_finite() => self.change_width(id, *delta),
                        Some(_) => return Some(Err(CommandError::InvalidArgument)),
                        None => self.cycle_width(id),
                    };
                    if !changed {
                        return Some(Err(CommandError::NoNeighbor));
                    }
                    if self.settings.focus_auto_scroll && self.active == Some(id) {
                        self.reveal_now(id, area, ViewportAlignment::Nearest);
                    }
                    Ok(())
                }
                None => Err(CommandError::NoFocusedWindow),
            },
            _ => return None,
        })
    }
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.width - EPSILON
        && b.x < a.x + a.width - EPSILON
        && a.y < b.y + b.height - EPSILON
        && b.y < a.y + a.height - EPSILON
}
