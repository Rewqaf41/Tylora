//! Versioned, validated TOML configuration. Parsing does not mutate the daemon.

use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fmt, fs, path::Path};
use tylora_core::commands::{Direction, StateCommand};
pub use tylora_core::layout::{
    DwindleSettings, LayoutKind, ScrollingSettings, SplitOrientation, ViewportAlignment,
};
pub use tylora_core::rules::{ApplyPolicy, RuleDefinition, RuleMode, TitleMatch};
pub use tylora_core::scratchpad::{
    HideStrategy, ScratchpadAlignment, ScratchpadDefinition, ScratchpadScope, ScratchpadTarget,
};

pub mod mutation;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub gaps: Gaps,
    pub zoom: Zoom,
    pub default_layout: LayoutKind,
    pub dwindle: DwindleSettings,
    pub scrolling: ScrollingSettings,
    pub hotkeys: Vec<Hotkey>,
    /// Legacy mode-only rules; still applied, see `rules` for the full engine.
    pub window_rules: Vec<WindowRule>,
    pub rules: Vec<RuleDefinition>,
    pub scratchpads: Vec<ScratchpadDefinition>,
    pub mouse: Mouse,
    pub focus: Focus,
    pub workspaces: Vec<WorkspaceDefinition>,
    pub displays: Vec<DisplayAssignment>,
    pub window_movement: WindowMovement,
    pub resize_step: f64,
    pub cross_monitor_navigation: bool,
    pub disconnect_policy: DisconnectPolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Gaps {
    pub inner: f64,
    pub outer: f64,
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Zoom {
    pub respect_outer_gaps: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowMovement {
    pub focus_after_move: bool,
    pub wrap_around: bool,
    pub preserve_floating_position: bool,
}
impl Default for WindowMovement {
    fn default() -> Self {
        Self {
            focus_after_move: false,
            wrap_around: true,
            preserve_floating_position: true,
        }
    }
}
impl Default for Gaps {
    fn default() -> Self {
        Self {
            inner: 8.0,
            outer: 8.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hotkey {
    pub chord: String,
    pub command: StateCommand,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowRule {
    pub application_id: String,
    pub mode: WindowRuleMode,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowRuleMode {
    Floating,
    Tiled,
}
#[derive(Debug, Clone)]
pub struct CompiledHotkey {
    pub keycode: u16,
    pub modifiers: u64,
    pub command: StateCommand,
}

pub fn compile_hotkey(binding: &Hotkey) -> Result<CompiledHotkey, ConfigError> {
    let mut modifiers = 0u64;
    let mut key = None;
    for part in binding.chord.to_ascii_lowercase().split('+') {
        match part.trim() {
            "shift" => modifiers |= 0x0002_0000,
            "control" | "ctrl" => modifiers |= 0x0004_0000,
            "option" | "alt" => modifiers |= 0x0008_0000,
            "command" | "cmd" => modifiers |= 0x0010_0000,
            value => {
                if key.is_some() {
                    return Err(ConfigError::Invalid(format!(
                        "multiple keys in hotkey: {}",
                        binding.chord
                    )));
                }
                key = Some(match value {
                    "a" => 0,
                    "s" => 1,
                    "d" => 2,
                    "f" => 3,
                    "h" => 4,
                    "g" => 5,
                    "z" => 6,
                    "x" => 7,
                    "c" => 8,
                    "v" => 9,
                    "b" => 11,
                    "q" => 12,
                    "w" => 13,
                    "e" => 14,
                    "r" => 15,
                    "y" => 16,
                    "t" => 17,
                    "1" => 18,
                    "2" => 19,
                    "3" => 20,
                    "4" => 21,
                    "6" => 22,
                    "5" => 23,
                    "9" => 25,
                    "7" => 26,
                    "8" => 28,
                    "0" => 29,
                    "o" => 31,
                    "u" => 32,
                    "i" => 34,
                    "p" => 35,
                    "l" => 37,
                    "j" => 38,
                    "k" => 40,
                    "n" => 45,
                    "m" => 46,
                    "tab" => 48,
                    "grave" => 50,
                    "left" => 123,
                    "right" => 124,
                    "down" => 125,
                    "up" => 126,
                    "f1" => 122,
                    "f2" => 120,
                    "f3" => 99,
                    "f4" => 118,
                    "f5" => 96,
                    "f6" => 97,
                    "f7" => 98,
                    "f8" => 100,
                    "f9" => 101,
                    "f10" => 109,
                    "f11" => 103,
                    "f12" => 111,
                    _ => {
                        return Err(ConfigError::Invalid(format!(
                            "unsupported hotkey key: {value}"
                        )))
                    }
                });
            }
        }
    }
    let keycode =
        key.ok_or_else(|| ConfigError::Invalid(format!("missing hotkey key: {}", binding.chord)))?;
    if modifiers == 0 {
        return Err(ConfigError::Invalid(format!(
            "hotkey needs a modifier: {}",
            binding.chord
        )));
    }
    Ok(CompiledHotkey {
        keycode,
        modifiers,
        command: binding.command.clone(),
    })
}
fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Mouse {
    pub enabled: bool,
    pub modifier: String,
    pub drag_threshold: f64,
    pub resize_sensitivity: f64,
    pub update_interval_ms: u64,
}
impl Default for Mouse {
    fn default() -> Self {
        Self {
            enabled: true,
            modifier: "option".into(),
            drag_threshold: 6.0,
            resize_sensitivity: 1.0,
            update_interval_ms: 16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Focus {
    pub follows_mouse: bool,
    pub mode: FocusMode,
    pub delay_ms: u16,
    pub raise_behavior: RaiseBehavior,
    pub ignore_apps: Vec<FocusExclusion>,
    pub restore_last_window: bool,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusMode {
    Disabled,
    Instant,
    #[default]
    Delayed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RaiseBehavior {
    #[default]
    FocusAndRaise,
    FocusOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusExclusion {
    pub bundle_id: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
fn default_focus_delay() -> u16 {
    120
}
impl Default for Focus {
    fn default() -> Self {
        Self {
            follows_mouse: false,
            mode: FocusMode::default(),
            delay_ms: default_focus_delay(),
            raise_behavior: RaiseBehavior::default(),
            ignore_apps: Vec::new(),
            restore_last_window: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceDefinition {
    pub id: u8,
    pub name: String,
    pub display: Option<u32>,
    /// Explicit, stable macOS Space ID. Missing means unresolved, never inferred by index.
    #[serde(default)]
    pub native_space_id: Option<u64>,
    /// Further native Spaces, typically one per additional display. Focusing
    /// the workspace selects the instance with the most recent focus;
    /// focusing it again from one instance moves to the next.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_space_ids: Vec<u64>,
    /// Layout used by this workspace; `None` follows `default_layout`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<LayoutKind>,
}

impl WorkspaceDefinition {
    pub fn layout_or(&self, default: LayoutKind) -> LayoutKind {
        self.layout.unwrap_or(default)
    }
    /// Every bound native Space: the primary first, then the extras.
    pub fn space_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self.native_space_id.into_iter().collect();
        for id in &self.native_space_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
        ids
    }
    pub fn binds(&self, space: u64) -> bool {
        self.native_space_id == Some(space) || self.native_space_ids.contains(&space)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayAssignment {
    pub display: u32,
    pub workspace: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisconnectPolicy {
    MoveToPrimary,
}

impl Default for Config {
    fn default() -> Self {
        let mut hotkeys = Vec::new();
        for (key, direction) in [
            ("h", Direction::Left),
            ("j", Direction::Down),
            ("k", Direction::Up),
            ("l", Direction::Right),
        ] {
            hotkeys.push(Hotkey {
                chord: format!("option+{key}"),
                command: StateCommand::FocusWindow { direction },
                enabled: true,
            });
            hotkeys.push(Hotkey {
                chord: format!("option+shift+{key}"),
                command: StateCommand::MoveWindow { direction },
                enabled: true,
            });
        }
        for id in 1..=9 {
            hotkeys.push(Hotkey {
                chord: format!("option+{id}"),
                command: StateCommand::FocusWorkspace { id },
                enabled: true,
            });
            hotkeys.push(Hotkey {
                chord: format!("option+shift+{id}"),
                command: StateCommand::MoveWindowToWorkspace {
                    window: None,
                    workspace: id,
                },
                enabled: true,
            });
        }
        hotkeys.push(Hotkey {
            chord: "option+f".into(),
            command: StateCommand::ToggleFloating { window: None },
            enabled: true,
        });
        hotkeys.push(Hotkey {
            chord: "option+shift+f".into(),
            command: StateCommand::ToggleLayoutZoom { window: None },
            enabled: true,
        });
        hotkeys.push(Hotkey {
            chord: "option+shift+tab".into(),
            command: StateCommand::MoveWindowToNextDisplay { window: None },
            enabled: true,
        });
        hotkeys.push(Hotkey {
            chord: "option+r".into(),
            command: StateCommand::ReconcileWindows,
            enabled: true,
        });
        hotkeys.extend(layout_hotkeys());
        Self {
            version: 1,
            gaps: Gaps::default(),
            zoom: Zoom::default(),
            default_layout: LayoutKind::Bsp,
            dwindle: DwindleSettings::default(),
            scrolling: ScrollingSettings::default(),
            hotkeys,
            window_rules: Vec::new(),
            rules: Vec::new(),
            scratchpads: Vec::new(),
            mouse: Mouse::default(),
            focus: Focus::default(),
            workspaces: (1..=9)
                .map(|id| WorkspaceDefinition {
                    id,
                    name: id.to_string(),
                    display: None,
                    native_space_id: None,
                    native_space_ids: Vec::new(),
                    layout: None,
                })
                .collect(),
            displays: Vec::new(),
            window_movement: WindowMovement::default(),
            resize_step: 50.0,
            cross_monitor_navigation: true,
            disconnect_policy: DisconnectPolicy::MoveToPrimary,
        }
    }
}

/// Toggle bindings declared on scratchpads.
pub fn scratchpad_hotkeys(config: &Config) -> Vec<Hotkey> {
    config
        .scratchpads
        .iter()
        .filter_map(|pad| {
            pad.hotkey.as_ref().map(|chord| Hotkey {
                chord: chord.clone(),
                command: StateCommand::ToggleScratchpad { id: pad.id.clone() },
                enabled: true,
            })
        })
        .collect()
}

/// Default layout bindings. The daemon only installs one when the user has
/// neither bound the same action nor taken the chord.
pub fn layout_hotkeys() -> Vec<Hotkey> {
    [
        ("option+control+h", StateCommand::ScrollLayoutLeft),
        ("option+control+l", StateCommand::ScrollLayoutRight),
        ("option+control+c", StateCommand::CenterFocusedWindow),
        (
            "option+control+w",
            StateCommand::ResizeScrollingColumn { delta: None },
        ),
        ("option+control+t", StateCommand::CycleLayout),
    ]
    .into_iter()
    .map(|(chord, command)| Hotkey {
        chord: chord.into(),
        command,
        enabled: true,
    })
    .collect()
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Invalid(String),
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read config: {e}"),
            Self::Parse(e) => write!(f, "invalid TOML: {e}"),
            Self::Invalid(e) => write!(f, "invalid config: {e}"),
        }
    }
}
impl std::error::Error for ConfigError {}

impl Config {
    pub fn workspace(&self, selector: &str) -> Option<&WorkspaceDefinition> {
        self.workspaces.iter().find(|workspace| {
            workspace.name.eq_ignore_ascii_case(selector)
                || selector.parse::<u8>().ok() == Some(workspace.id)
        })
    }
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(source).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::parse(&fs::read_to_string(path).map_err(ConfigError::Io)?)
    }
    pub fn validate(&self) -> Result<(), ConfigError> {
        let fail = |message: &str| Err(ConfigError::Invalid(message.into()));
        if self.version != 1 {
            return fail("unsupported schema version");
        }
        if !self.gaps.inner.is_finite()
            || !self.gaps.outer.is_finite()
            || self.gaps.inner < 0.0
            || self.gaps.outer < 0.0
        {
            return fail("gaps must be finite and nonnegative");
        }
        self.dwindle.validate().map_err(ConfigError::Invalid)?;
        tylora_core::rules::validate_rules(&self.rules, |selector| {
            self.workspace(selector).is_some()
        })
        .map_err(ConfigError::Invalid)?;
        tylora_core::scratchpad::validate_scratchpads(&self.scratchpads)
            .map_err(ConfigError::Invalid)?;
        for rule in &self.rules {
            if let Some(pad) = &rule.scratchpad {
                if !self.scratchpads.iter().any(|known| known.id == *pad) {
                    return fail(&format!(
                        "rule {:?}: scratchpad {pad:?} is not configured",
                        rule.id
                    ));
                }
            }
        }
        self.scrolling.validate().map_err(ConfigError::Invalid)?;
        if !self.resize_step.is_finite() || self.resize_step <= 0.0 {
            return fail("resize_step must be positive");
        }
        if !self.mouse.drag_threshold.is_finite()
            || self.mouse.drag_threshold < 0.0
            || !self.mouse.resize_sensitivity.is_finite()
            || self.mouse.resize_sensitivity <= 0.0
            || self.mouse.resize_sensitivity > 10.0
            || !(8..=1000).contains(&self.mouse.update_interval_ms)
        {
            return fail("invalid mouse sensitivity or update interval");
        }
        if !matches!(
            self.mouse.modifier.as_str(),
            "option" | "command" | "control" | "shift"
        ) {
            return fail("invalid mouse modifier");
        }
        if self.focus.delay_ms > 500 {
            return fail("focus delay_ms must be between 0 and 500");
        }
        if self.focus.raise_behavior == RaiseBehavior::FocusOnly {
            return fail("focus_only is unsupported: macOS focus without raise is not reliable");
        }
        let mut excluded = HashSet::new();
        for app in &self.focus.ignore_apps {
            if app.bundle_id.trim().is_empty()
                || !excluded.insert(app.bundle_id.to_ascii_lowercase())
            {
                return fail("focus ignore_apps bundle IDs must be nonempty and unique");
            }
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let mut native_ids = HashSet::new();
        for workspace in &self.workspaces {
            if workspace.id == 0
                || workspace.name.trim().is_empty()
                || !ids.insert(workspace.id)
                || !names.insert(workspace.name.to_ascii_lowercase())
            {
                return fail("workspace IDs must be unique, nonzero, and named");
            }
            let mut own = HashSet::new();
            for native in workspace
                .native_space_id
                .iter()
                .chain(workspace.native_space_ids.iter())
            {
                if *native == 0 || !own.insert(*native) || !native_ids.insert(*native) {
                    return fail("native Space IDs must be nonzero and unique");
                }
            }
        }
        for assignment in &self.displays {
            if !ids.contains(&assignment.workspace) {
                return fail("display assignment references unknown workspace");
            }
        }
        let mut chords = HashSet::new();
        for hotkey in self.hotkeys.iter().filter(|hotkey| hotkey.enabled) {
            let compiled = compile_hotkey(hotkey)?;
            if !chords.insert((compiled.keycode, compiled.modifiers)) {
                return fail("conflicting or empty hotkey chord");
            }
        }
        for pad in &self.scratchpads {
            let Some(chord) = &pad.hotkey else {
                continue;
            };
            let compiled = compile_hotkey(&Hotkey {
                chord: chord.clone(),
                command: StateCommand::ToggleScratchpad { id: pad.id.clone() },
                enabled: true,
            })?;
            if !chords.insert((compiled.keycode, compiled.modifiers)) {
                return fail(&format!(
                    "scratchpad {:?} hotkey {chord:?} is already bound",
                    pad.id
                ));
            }
        }
        for rule in &self.window_rules {
            if rule.application_id.trim().is_empty()
                || rule.application_id.len() > 255
                || !(-100..=100).contains(&rule.priority)
            {
                return fail("window rules need a bundle ID and priority from -100 to 100");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_validate() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn legacy_focus_defaults_off_and_focus_only_is_rejected() {
        let config = Config::parse("version = 1\n[focus]\nrestore_last_window = true\n").unwrap();
        assert!(!config.focus.follows_mouse);
        assert_eq!(config.focus.mode, FocusMode::Delayed);
        let mut unsupported = config;
        unsupported.focus.raise_behavior = RaiseBehavior::FocusOnly;
        assert!(unsupported.validate().is_err());
    }
    #[test]
    fn invalid_reload_does_not_replace_previous_config() {
        let previous = Config::default();
        let mut next = Config::default();
        next.hotkeys.push(next.hotkeys[0].clone());
        assert!(next.validate().is_err());
        assert!(previous.validate().is_ok());
    }
    #[test]
    fn rejects_unsupported_version_and_bad_gaps() {
        assert!(Config::parse("version = 2").is_err());
        assert!(Config::parse("version = 1\n[gaps]\ninner = -1").is_err());
    }
    #[test]
    fn physical_hotkey_aliases_conflict_and_disabled_bindings_do_not() {
        let mut config = Config::default();
        let command = StateCommand::ReconcileWindows;
        config.hotkeys = vec![
            Hotkey {
                chord: "option+h".into(),
                command: command.clone(),
                enabled: true,
            },
            Hotkey {
                chord: "alt+h".into(),
                command: command.clone(),
                enabled: true,
            },
        ];
        assert!(config.validate().is_err());
        config.hotkeys[1].enabled = false;
        assert!(config.validate().is_ok());
        assert_eq!(compile_hotkey(&config.hotkeys[0]).unwrap().keycode, 4);
    }
    #[test]
    fn tab_display_move_hotkey_uses_physical_keycode_and_detects_alias_conflict() {
        let mut config = Config::default();
        let first = config
            .hotkeys
            .iter()
            .find(|hotkey| matches!(hotkey.command, StateCommand::MoveWindowToNextDisplay { .. }))
            .unwrap()
            .clone();
        assert_eq!(compile_hotkey(&first).unwrap().keycode, 48);
        config.hotkeys = vec![
            first,
            Hotkey {
                chord: "alt+shift+tab".into(),
                command: StateCommand::ReconcileWindows,
                enabled: true,
            },
        ];
        assert!(config.validate().is_err());
    }
    #[test]
    fn documented_example_parses() {
        let example = include_str!("../../../examples/config.toml");
        let config = Config::parse(example).unwrap();
        assert_eq!(config.workspaces.len(), 2);
        assert_eq!(config.hotkeys.len(), 11);
    }
    #[test]
    fn workspace_names_resolve_to_explicit_native_ids() {
        let mut config = Config::default();
        config.workspaces[0].name = "Code".into();
        config.workspaces[0].native_space_id = Some(71);
        config.workspaces[1].name = "Browser".into();
        config.workspaces[1].native_space_id = Some(4);
        assert_eq!(config.workspace("code").unwrap().native_space_id, Some(71));
        assert_eq!(config.workspace("2").unwrap().native_space_id, Some(4));
        assert!(config.workspace("3").unwrap().native_space_id.is_none());
        config.workspaces[1].native_space_id = Some(71);
        assert!(config.validate().is_err());
    }
}
