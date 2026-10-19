//! Commands shared by CLI, hotkeys and future GUI clients.

use crate::{layout::LayoutKind, registry::ManagedWindowId, DisplayId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum StateCommand {
    FocusWindow {
        direction: Direction,
    },
    MoveWindow {
        direction: Direction,
    },
    SwapWindow {
        first: ManagedWindowId,
        second: ManagedWindowId,
    },
    ResizeWindow {
        direction: Direction,
        amount: f64,
    },
    ToggleFloating {
        window: Option<ManagedWindowId>,
    },
    ToggleLayoutZoom {
        window: Option<ManagedWindowId>,
    },
    FocusWorkspace {
        id: u8,
    },
    FocusNamedWorkspace {
        name: String,
    },
    FocusNextWorkspace,
    FocusPreviousWorkspace,
    MoveWindowToWorkspace {
        window: Option<ManagedWindowId>,
        workspace: u8,
    },
    MoveWindowToNamedWorkspace {
        name: String,
    },
    MoveWindowToDisplay {
        window: Option<ManagedWindowId>,
        display: DisplayId,
    },
    MoveWindowToNextDisplay {
        window: Option<ManagedWindowId>,
    },
    MoveWindowToPreviousDisplay {
        window: Option<ManagedWindowId>,
    },
    FocusDisplay {
        direction: Direction,
    },
    FocusDisplayNext,
    /// Switch the current workspace to the next layout kind.
    CycleLayout,
    /// Switch the current workspace to a specific layout kind.
    SetLayout {
        layout: LayoutKind,
    },
    /// Focus the next window in layout order (strip order for Scrolling).
    FocusNextWindow,
    FocusPreviousWindow,
    /// Move the Scrolling viewport without changing focus.
    ScrollLayoutLeft,
    ScrollLayoutRight,
    /// Centre the focused Scrolling column in the viewport.
    CenterFocusedWindow,
    /// Change the focused Scrolling column width. `delta` is a fraction of
    /// the usable display width; `None` cycles the configured presets.
    ResizeScrollingColumn {
        delta: Option<f64>,
    },
    ReconcileWindows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandError {
    NoFocusedWindow,
    NoNeighbor,
    WindowNotFound,
    DisplayNotFound,
    WorkspaceNotFound,
    VisibilityUnsupported,
    AccessibilityUnavailable,
    InvalidArgument,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub ok: bool,
    pub message: String,
    pub error: Option<CommandError>,
    pub window: Option<ManagedWindowId>,
    pub display: Option<DisplayId>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub transfer: Option<Box<DisplayTransferStatus>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayTransferStatus {
    pub cg_window_id: u32,
    pub source_display: DisplayId,
    pub target_display: DisplayId,
    pub source_space_id: u64,
    pub target_space_id: u64,
    pub native_transfer: String,
    pub bsp_update: String,
    pub focus: String,
    pub elapsed_ms: u128,
}

impl CommandResult {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            error: None,
            window: None,
            display: None,
            transfer: None,
        }
    }
    pub fn failure(error: CommandError, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: message.into(),
            error: Some(error),
            window: None,
            display: None,
            transfer: None,
        }
    }
}
