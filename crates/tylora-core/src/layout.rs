//! Layout vocabulary shared by the layout engines, configuration and the daemon.
//! Pure data: nothing here performs platform calls.

use crate::Rect;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutKind {
    #[default]
    Bsp,
    Dwindle,
    Scrolling,
}

impl LayoutKind {
    pub const ALL: [LayoutKind; 3] = [LayoutKind::Bsp, LayoutKind::Dwindle, LayoutKind::Scrolling];

    pub fn name(self) -> &'static str {
        match self {
            Self::Bsp => "bsp",
            Self::Dwindle => "dwindle",
            Self::Scrolling => "scrolling",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(value))
    }

    pub fn next(self) -> Self {
        match self {
            Self::Bsp => Self::Dwindle,
            Self::Dwindle => Self::Scrolling,
            Self::Scrolling => Self::Bsp,
        }
    }
}

/// Orientation of a split: `Vertical` places children side by side,
/// `Horizontal` stacks them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitOrientation {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DwindleSettings {
    /// Orientation of the first split; later splits alternate.
    pub initial_orientation: SplitOrientation,
    /// Share of a split given to the existing window when a new one arrives.
    pub split_ratio: f64,
}

impl Default for DwindleSettings {
    fn default() -> Self {
        Self {
            initial_orientation: SplitOrientation::Vertical,
            split_ratio: 0.5,
        }
    }
}

impl DwindleSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !self.split_ratio.is_finite() || !(0.1..=0.9).contains(&self.split_ratio) {
            return Err("dwindle split_ratio must be between 0.1 and 0.9".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportAlignment {
    /// Move the strip only as far as needed to show the focused column.
    #[default]
    Nearest,
    /// Keep the focused column centred in the viewport.
    Center,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrollingSettings {
    /// Fractions of the usable display width (inside outer gaps).
    pub default_column_width: f64,
    pub min_column_width: f64,
    pub max_column_width: f64,
    /// Widths cycled by `ResizeScrollingColumn { delta: None }`.
    pub column_width_presets: Vec<f64>,
    pub viewport_alignment: ViewportAlignment,
    /// Scroll the strip so keyboard-focused columns become visible.
    pub focus_auto_scroll: bool,
    pub mouse_resize: bool,
    /// Fraction of the viewport width moved by one manual scroll command.
    pub scroll_sensitivity: f64,
}

impl Default for ScrollingSettings {
    fn default() -> Self {
        Self {
            default_column_width: 0.65,
            min_column_width: 0.2,
            max_column_width: 1.0,
            column_width_presets: vec![1.0 / 3.0, 0.5, 0.65, 1.0],
            viewport_alignment: ViewportAlignment::Nearest,
            focus_auto_scroll: true,
            mouse_resize: true,
            scroll_sensitivity: 0.25,
        }
    }
}

impl ScrollingSettings {
    pub fn validate(&self) -> Result<(), String> {
        let fraction = |value: f64| value.is_finite() && value > 0.0 && value <= 1.0;
        if !fraction(self.min_column_width)
            || !fraction(self.max_column_width)
            || !fraction(self.default_column_width)
            || self.min_column_width < 0.05
            || self.min_column_width > self.max_column_width
            || self.default_column_width < self.min_column_width
            || self.default_column_width > self.max_column_width
        {
            return Err(
                "scrolling column widths must satisfy 0.05 <= min <= default <= max <= 1".into(),
            );
        }
        if self.column_width_presets.is_empty()
            || self.column_width_presets.len() > 8
            || self.column_width_presets.iter().any(|preset| {
                !fraction(*preset)
                    || *preset < self.min_column_width
                    || *preset > self.max_column_width
            })
        {
            return Err(
                "scrolling column_width_presets must contain 1 to 8 widths within min..max".into(),
            );
        }
        if !self.scroll_sensitivity.is_finite() || !(0.05..=1.0).contains(&self.scroll_sensitivity)
        {
            return Err("scrolling scroll_sensitivity must be between 0.05 and 1".into());
        }
        Ok(())
    }
}

/// Physical surroundings of the display a layout is drawn on. Scrolling uses
/// it to decide where columns outside the viewport may be parked without
/// spilling onto another display.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayoutEnvironment {
    /// Full frame of the display, including menu bar and Dock regions.
    pub display_frame: Option<Rect>,
    /// Full frames of every other active display.
    pub other_displays: Vec<Rect>,
}

/// Why a window became focused. Only explicit navigation may scroll a
/// Scrolling strip past windows the user can already see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOrigin {
    /// A Tylora command (keyboard, CLI or GUI) selected the window.
    Command,
    /// macOS reported the focus change (click, Cmd-Tab, app activation).
    System,
    /// Focus-follows-mouse selected a visible window.
    Pointer,
}
