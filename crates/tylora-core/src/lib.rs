//! Platform independent identities, geometry and events.

use serde::{Deserialize, Serialize};
use std::fmt;

pub mod animation;
pub mod commands;
pub mod coordinator;
pub mod layout;
pub mod navigation;
pub mod registry;
pub mod rules;
pub mod scratchpad;
pub mod workspace;

pub use coordinator::{
    ApplyCommand, CommandEffect, Coordinator, CoordinatorEvent, LayoutMetrics, LayoutPolicy,
};
pub use registry::{
    ManagedWindowId, ManagementMode, ProcessInstance, Registry, WindowLifecycle, WindowRecord,
    WindowSnapshot,
};

/// A Core Graphics window number. Valid only for the current WindowServer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowId(pub u32);

/// A Core Graphics direct display ID. May change after display reconfiguration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DisplayId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    NonFinite,
    NegativeSize,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid rectangle: {self:?}")
    }
}

impl std::error::Error for GeometryError {}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self, GeometryError> {
        if ![x, y, width, height].into_iter().all(f64::is_finite) {
            return Err(GeometryError::NonFinite);
        }
        if width < 0.0 || height < 0.0 {
            return Err(GeometryError::NegativeSize);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn area(self) -> f64 {
        self.width * self.height
    }

    pub fn near(self, other: Self, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }

    pub fn intersects(self, other: Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    pub id: WindowId,
    pub owner_pid: i32,
    pub owner_name: Option<String>,
    pub display_id: Option<DisplayId>,
    /// Core Graphics bounds, in global screen coordinates (points).
    pub frame: Rect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    Discovered(WindowState),
    GeometryChanged { id: WindowId, frame: Rect },
    Removed(WindowId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_rejects_invalid_components() {
        assert_eq!(
            Rect::new(0.0, 0.0, f64::NAN, 1.0),
            Err(GeometryError::NonFinite)
        );
        assert_eq!(
            Rect::new(0.0, 0.0, -1.0, 1.0),
            Err(GeometryError::NegativeSize)
        );
    }

    #[test]
    fn touching_edges_do_not_intersect() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0).unwrap();
        let b = Rect::new(10.0, 0.0, 3.0, 3.0).unwrap();
        assert!(!a.intersects(b));
        assert_eq!(a.area(), 100.0);
    }

    #[test]
    fn identities_are_distinct() {
        assert_ne!(WindowId(1), WindowId(2));
        assert_ne!(DisplayId(1), DisplayId(2));
    }
}
