//! Narrow, validated edits that retain unrelated TOML entries and comments.

use crate::{
    Config, ConfigError, DwindleSettings, Focus, LayoutKind, ScrollingSettings, WindowMovement,
    WindowRule,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use toml_edit::{value, ArrayOfTables, DocumentMut, Item, Table};
use tylora_core::commands::StateCommand;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConfigPatch {
    SetGaps {
        inner: f64,
        outer: f64,
    },
    SetZoom {
        respect_outer_gaps: bool,
    },
    SetWindowMovement {
        settings: WindowMovement,
    },
    SetWorkspace {
        id: u8,
        name: String,
        display: Option<u32>,
        native_space_id: Option<u64>,
        /// `None` leaves the extra Spaces unchanged; `Some([])` removes them.
        #[serde(default)]
        native_space_ids: Option<Vec<u64>>,
    },
    SetHotkey {
        index: Option<usize>,
        chord: String,
        command: StateCommand,
        enabled: bool,
    },
    SetWindowRule {
        index: Option<usize>,
        rule: WindowRule,
    },
    RemoveWindowRule {
        index: usize,
    },
    SetFocus {
        focus: Focus,
    },
    SetDefaultLayout {
        layout: LayoutKind,
    },
    /// `None` removes the override so the workspace follows `default_layout`.
    SetWorkspaceLayout {
        id: u8,
        layout: Option<LayoutKind>,
    },
    SetDwindle {
        settings: DwindleSettings,
    },
    SetScrolling {
        settings: ScrollingSettings,
    },
}

fn ensure_table<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.as_table().get(key).is_some_and(Item::is_table) {
        doc[key] = Item::Table(Table::new());
    }
    doc[key].as_table_mut().expect("table inserted")
}

/// Stable content revision used for optimistic concurrency across GUI sessions.
pub fn revision(source: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

pub fn apply_patch(source: &str, patch: ConfigPatch) -> Result<(String, Config), ConfigError> {
    let mut doc = DocumentMut::from_str(source)
        .map_err(|error| ConfigError::Invalid(format!("invalid TOML document: {error}")))?;
    match patch {
        ConfigPatch::SetWindowMovement { settings } => {
            if !doc
                .as_table()
                .get("window_movement")
                .is_some_and(Item::is_table)
            {
                doc["window_movement"] = Item::Table(Table::new());
            }
            doc["window_movement"]["focus_after_move"] = value(settings.focus_after_move);
            doc["window_movement"]["wrap_around"] = value(settings.wrap_around);
            doc["window_movement"]["preserve_floating_position"] =
                value(settings.preserve_floating_position);
        }
        ConfigPatch::SetZoom { respect_outer_gaps } => {
            if !doc.as_table().get("zoom").is_some_and(Item::is_table) {
                doc["zoom"] = Item::Table(Table::new());
            }
            doc["zoom"]["respect_outer_gaps"] = value(respect_outer_gaps);
        }
        ConfigPatch::SetGaps { inner, outer } => {
            if !inner.is_finite() || !outer.is_finite() || inner < 0.0 || outer < 0.0 {
                return Err(ConfigError::Invalid(
                    "gaps must be finite and nonnegative".into(),
                ));
            }
            if !doc.as_table().get("gaps").is_some_and(Item::is_table) {
                doc["gaps"] = Item::Table(Table::new());
            }
            doc["gaps"]["inner"] = value(inner);
            doc["gaps"]["outer"] = value(outer);
        }
        ConfigPatch::SetWorkspace {
            id,
            name,
            display,
            native_space_id,
            native_space_ids,
        } => {
            if !doc
                .as_table()
                .get("workspaces")
                .is_some_and(Item::is_array_of_tables)
            {
                doc["workspaces"] = Item::ArrayOfTables(ArrayOfTables::new());
            }
            let workspaces = doc["workspaces"]
                .as_array_of_tables_mut()
                .expect("array inserted");
            let index = workspaces.iter().position(|table| {
                table.get("id").and_then(Item::as_integer) == Some(i64::from(id))
            });
            if index.is_none() {
                workspaces.push(Table::new());
            }
            let table = workspaces
                .get_mut(index.unwrap_or(workspaces.len() - 1))
                .expect("table inserted");
            table["id"] = value(i64::from(id));
            table["name"] = value(name);
            match display {
                Some(display) => table["display"] = value(i64::from(display)),
                None => {
                    table.remove("display");
                }
            }
            match native_space_id {
                Some(native_id) => {
                    let native_id = i64::try_from(native_id).map_err(|_| {
                        ConfigError::Invalid("native Space ID exceeds TOML integer range".into())
                    })?;
                    table["native_space_id"] = value(native_id);
                }
                None => {
                    table.remove("native_space_id");
                }
            }
            match native_space_ids {
                Some(ids) if ids.is_empty() => {
                    table.remove("native_space_ids");
                }
                Some(ids) => {
                    let mut array = toml_edit::Array::new();
                    for id in ids {
                        array.push(i64::try_from(id).map_err(|_| {
                            ConfigError::Invalid(
                                "native Space ID exceeds TOML integer range".into(),
                            )
                        })?);
                    }
                    table["native_space_ids"] = value(array);
                }
                None => {}
            }
        }
        ConfigPatch::SetHotkey {
            index,
            chord,
            command,
            enabled,
        } => {
            if !doc
                .as_table()
                .get("hotkeys")
                .is_some_and(Item::is_array_of_tables)
            {
                doc["hotkeys"] = Item::ArrayOfTables(ArrayOfTables::new());
            }
            let hotkeys = doc["hotkeys"]
                .as_array_of_tables_mut()
                .expect("array inserted");
            let index = match index {
                Some(index) if index < hotkeys.len() => index,
                Some(_) => {
                    return Err(ConfigError::Invalid("hotkey index no longer exists".into()))
                }
                None => {
                    hotkeys.push(Table::new());
                    hotkeys.len() - 1
                }
            };
            let table = hotkeys.get_mut(index).expect("hotkey index checked");
            table["chord"] = value(chord);
            table["enabled"] = value(enabled);
            table["command"] = Item::Table(
                toml_edit::ser::to_document(&command)
                    .map_err(|error| {
                        ConfigError::Invalid(format!("cannot encode hotkey action: {error}"))
                    })?
                    .as_table()
                    .clone(),
            );
        }
        ConfigPatch::SetWindowRule { index, rule } => {
            if !doc
                .as_table()
                .get("window_rules")
                .is_some_and(Item::is_array_of_tables)
            {
                doc["window_rules"] = Item::ArrayOfTables(ArrayOfTables::new());
            }
            let rules = doc["window_rules"]
                .as_array_of_tables_mut()
                .expect("array inserted");
            let index = match index {
                Some(index) if index < rules.len() => index,
                Some(_) => {
                    return Err(ConfigError::Invalid(
                        "window rule index no longer exists".into(),
                    ))
                }
                None => {
                    rules.push(Table::new());
                    rules.len() - 1
                }
            };
            let table = rules.get_mut(index).expect("rule index checked");
            table["application_id"] = value(rule.application_id);
            table["mode"] = value(match rule.mode {
                crate::WindowRuleMode::Floating => "floating",
                crate::WindowRuleMode::Tiled => "tiled",
            });
            table["priority"] = value(i64::from(rule.priority));
            table["enabled"] = value(rule.enabled);
        }
        ConfigPatch::RemoveWindowRule { index } => {
            let rules = doc
                .as_table_mut()
                .get_mut("window_rules")
                .and_then(Item::as_array_of_tables_mut)
                .ok_or_else(|| ConfigError::Invalid("window rule index no longer exists".into()))?;
            if index >= rules.len() {
                return Err(ConfigError::Invalid(
                    "window rule index no longer exists".into(),
                ));
            }
            rules.remove(index);
        }
        ConfigPatch::SetDefaultLayout { layout } => {
            doc["default_layout"] = value(layout.name());
        }
        ConfigPatch::SetWorkspaceLayout { id, layout } => {
            let table = doc
                .as_table_mut()
                .get_mut("workspaces")
                .and_then(Item::as_array_of_tables_mut)
                .and_then(|workspaces| {
                    workspaces.iter_mut().find(|table| {
                        table.get("id").and_then(Item::as_integer) == Some(i64::from(id))
                    })
                })
                .ok_or_else(|| ConfigError::Invalid(format!("workspace {id} is not configured")))?;
            match layout {
                Some(layout) => table["layout"] = value(layout.name()),
                None => {
                    table.remove("layout");
                }
            }
        }
        ConfigPatch::SetDwindle { settings } => {
            settings.validate().map_err(ConfigError::Invalid)?;
            let table = ensure_table(&mut doc, "dwindle");
            table["initial_orientation"] = value(match settings.initial_orientation {
                crate::SplitOrientation::Vertical => "vertical",
                crate::SplitOrientation::Horizontal => "horizontal",
            });
            table["split_ratio"] = value(settings.split_ratio);
        }
        ConfigPatch::SetScrolling { settings } => {
            settings.validate().map_err(ConfigError::Invalid)?;
            let table = ensure_table(&mut doc, "scrolling");
            table["default_column_width"] = value(settings.default_column_width);
            table["min_column_width"] = value(settings.min_column_width);
            table["max_column_width"] = value(settings.max_column_width);
            let mut presets = toml_edit::Array::new();
            for preset in &settings.column_width_presets {
                presets.push(*preset);
            }
            table["column_width_presets"] = value(presets);
            table["viewport_alignment"] = value(match settings.viewport_alignment {
                crate::ViewportAlignment::Nearest => "nearest",
                crate::ViewportAlignment::Center => "center",
            });
            table["focus_auto_scroll"] = value(settings.focus_auto_scroll);
            table["mouse_resize"] = value(settings.mouse_resize);
            table["scroll_sensitivity"] = value(settings.scroll_sensitivity);
        }
        ConfigPatch::SetFocus { focus } => {
            if !doc.as_table().get("focus").is_some_and(Item::is_table) {
                doc["focus"] = Item::Table(Table::new());
            }
            let table = doc["focus"].as_table_mut().expect("focus table inserted");
            table["follows_mouse"] = value(focus.follows_mouse);
            table["mode"] = value(match focus.mode {
                crate::FocusMode::Disabled => "disabled",
                crate::FocusMode::Instant => "instant",
                crate::FocusMode::Delayed => "delayed",
            });
            table["delay_ms"] = value(i64::from(focus.delay_ms));
            table["raise_behavior"] = value(match focus.raise_behavior {
                crate::RaiseBehavior::FocusAndRaise => "focus_and_raise",
                crate::RaiseBehavior::FocusOnly => "focus_only",
            });
            let mut ignored = toml_edit::Array::new();
            for app in focus.ignore_apps {
                let mut item = toml_edit::InlineTable::new();
                item.insert("bundle_id", toml_edit::Value::from(app.bundle_id));
                item.insert("enabled", toml_edit::Value::from(app.enabled));
                ignored.push(item);
            }
            table["ignore_apps"] = value(ignored);
            table["restore_last_window"] = value(focus.restore_last_window);
        }
    }
    let text = doc.to_string();
    let config = Config::parse(&text)?;
    Ok((text, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_keeps_comments_and_unknown_settings() {
        let source =
            "# personal comment\nversion = 1\nfuture_key = 42\n[gaps]\ninner = 8\nouter = 8\n";
        let (edited, config) = apply_patch(
            source,
            ConfigPatch::SetGaps {
                inner: 12.0,
                outer: 6.0,
            },
        )
        .unwrap();
        assert!(edited.contains("# personal comment"));
        assert!(edited.contains("future_key = 42"));
        assert_eq!(config.gaps.inner, 12.0);
    }

    #[test]
    fn invalid_patch_does_not_change_source() {
        let source = toml::to_string(&Config::default()).unwrap();
        let original = revision(&source);
        assert!(apply_patch(
            &source,
            ConfigPatch::SetGaps {
                inner: -1.0,
                outer: 4.0
            }
        )
        .is_err());
        assert_eq!(revision(&source), original);
    }

    #[test]
    fn focus_patch_preserves_unrelated_toml_and_validates_delay() {
        let source = "version = 1\n# custom\n[focus]\nrestore_last_window = false\n";
        let mut focus = crate::Focus {
            follows_mouse: true,
            delay_ms: 500,
            restore_last_window: false,
            ..Default::default()
        };
        focus.ignore_apps.push(crate::FocusExclusion {
            bundle_id: "com.example.Panel".into(),
            enabled: false,
        });
        let (edited, config) = apply_patch(
            source,
            ConfigPatch::SetFocus {
                focus: focus.clone(),
            },
        )
        .unwrap();
        assert!(edited.contains("# custom"));
        assert!(!config.focus.restore_last_window);
        assert_eq!(config.focus.ignore_apps[0].bundle_id, "com.example.Panel");
        focus.delay_ms = 501;
        assert!(apply_patch(&edited, ConfigPatch::SetFocus { focus }).is_err());
    }
    #[test]
    fn workspace_binding_and_hotkey_round_trip() {
        let source = "# custom settings\nversion = 1\n[[workspaces]]\nid = 1\nname = 'Code'\n";
        let (bound, config) = apply_patch(
            source,
            ConfigPatch::SetWorkspace {
                id: 1,
                name: "Editor".into(),
                display: None,
                native_space_id: Some(72),
                native_space_ids: None,
            },
        )
        .unwrap();
        assert_eq!(config.workspaces[0].native_space_id, Some(72));
        let (hotkey, config) = apply_patch(
            &bound,
            ConfigPatch::SetHotkey {
                index: None,
                chord: "option+f12".into(),
                command: StateCommand::FocusWorkspace { id: 1 },
                enabled: true,
            },
        )
        .unwrap();
        assert_eq!(config.hotkeys.len(), 1);
        assert_eq!(config.hotkeys[0].chord, "option+f12");
        assert!(hotkey.contains("# custom settings"));
    }

    #[test]
    fn window_rule_round_trip_and_removal_preserve_unrelated_toml() {
        let source = "# retain this\nversion = 1\ncustom = 'untouched'\n";
        let (added, config) = apply_patch(
            source,
            ConfigPatch::SetWindowRule {
                index: None,
                rule: crate::WindowRule {
                    application_id: "com.example.Editor".into(),
                    mode: crate::WindowRuleMode::Floating,
                    priority: 5,
                    enabled: true,
                },
            },
        )
        .unwrap();
        assert_eq!(config.window_rules.len(), 1);
        assert_eq!(config.window_rules[0].mode, crate::WindowRuleMode::Floating);
        let (removed, config) =
            apply_patch(&added, ConfigPatch::RemoveWindowRule { index: 0 }).unwrap();
        assert!(config.window_rules.is_empty());
        assert!(removed.contains("# retain this"));
        assert!(removed.contains("custom = 'untouched'"));
    }

    #[test]
    fn window_movement_patch_preserves_hotkeys_and_comments() {
        let source = "version = 1\n# keep this\n[[hotkeys]]\nchord = 'option+shift+tab'\n[hotkeys.command]\ncommand = 'move_window_to_next_display'\n";
        let settings = WindowMovement {
            focus_after_move: true,
            wrap_around: false,
            preserve_floating_position: false,
        };
        let (edited, config) =
            apply_patch(source, ConfigPatch::SetWindowMovement { settings }).unwrap();
        assert!(edited.contains("# keep this"));
        assert!(edited.contains("option+shift+tab"));
        assert_eq!(config.hotkeys.len(), 1);
        assert!(config.window_movement.focus_after_move);
        assert!(!config.window_movement.wrap_around);
        assert!(!config.window_movement.preserve_floating_position);
    }

    #[test]
    fn layout_patches_round_trip_and_preserve_toml() {
        let source = "# keep me\nversion = 1\n[[workspaces]]\nid = 1\nname = \"Code\"\nnative_space_id = 4\n[[workspaces]]\nid = 2\nname = \"Web\"\n";
        let (edited, config) = apply_patch(
            source,
            ConfigPatch::SetWorkspaceLayout {
                id: 1,
                layout: Some(LayoutKind::Scrolling),
            },
        )
        .unwrap();
        assert!(edited.contains("# keep me"));
        assert_eq!(config.workspaces[0].layout, Some(LayoutKind::Scrolling));
        assert_eq!(config.workspaces[0].native_space_id, Some(4));
        assert_eq!(config.workspaces[1].layout, None);
        let (edited, config) = apply_patch(
            &edited,
            ConfigPatch::SetDefaultLayout {
                layout: LayoutKind::Dwindle,
            },
        )
        .unwrap();
        assert_eq!(config.default_layout, LayoutKind::Dwindle);
        assert_eq!(
            config.workspaces[1].layout_or(config.default_layout),
            LayoutKind::Dwindle
        );
        let scrolling = ScrollingSettings {
            default_column_width: 0.5,
            viewport_alignment: crate::ViewportAlignment::Center,
            ..ScrollingSettings::default()
        };
        let (edited, config) = apply_patch(
            &edited,
            ConfigPatch::SetScrolling {
                settings: scrolling.clone(),
            },
        )
        .unwrap();
        assert_eq!(config.scrolling, scrolling);
        let dwindle = DwindleSettings {
            initial_orientation: crate::SplitOrientation::Horizontal,
            split_ratio: 0.6,
        };
        let (edited, config) =
            apply_patch(&edited, ConfigPatch::SetDwindle { settings: dwindle }).unwrap();
        assert_eq!(config.dwindle, dwindle);
        let (_, config) = apply_patch(
            &edited,
            ConfigPatch::SetWorkspaceLayout {
                id: 1,
                layout: None,
            },
        )
        .unwrap();
        assert_eq!(config.workspaces[0].layout, None);
        assert!(apply_patch(
            source,
            ConfigPatch::SetWorkspaceLayout {
                id: 9,
                layout: Some(LayoutKind::Bsp)
            }
        )
        .is_err());
    }

    #[test]
    fn invalid_layout_settings_are_rejected() {
        let scrolling = ScrollingSettings {
            min_column_width: 0.8,
            ..ScrollingSettings::default()
        };
        assert!(apply_patch(
            "version = 1\n",
            ConfigPatch::SetScrolling {
                settings: scrolling
            }
        )
        .is_err());
        let scrolling = ScrollingSettings {
            column_width_presets: vec![-0.5],
            ..ScrollingSettings::default()
        };
        assert!(apply_patch(
            "version = 1\n",
            ConfigPatch::SetScrolling {
                settings: scrolling
            }
        )
        .is_err());
        let dwindle = DwindleSettings {
            split_ratio: 0.99,
            ..DwindleSettings::default()
        };
        assert!(apply_patch(
            "version = 1\n",
            ConfigPatch::SetDwindle { settings: dwindle }
        )
        .is_err());
        assert!(
            crate::Config::parse("version = 1\n[scrolling]\nmax_column_width = 1.5\n").is_err()
        );
        assert!(crate::Config::parse("version = 1\ndefault_layout = \"spiral\"\n").is_err());
        let parsed = crate::Config::parse("version = 1\ndefault_layout = \"scrolling\"\n[scrolling]\ndefault_column_width = 0.5\n").unwrap();
        assert_eq!(parsed.default_layout, LayoutKind::Scrolling);
        assert_eq!(parsed.scrolling.column_width_presets.len(), 4);
    }

    #[test]
    fn extra_native_spaces_round_trip_and_stay_unique() {
        let source = "version = 1\n[[workspaces]]\nid = 1\nname = \"Browser\"\nnative_space_id = 7\n[[workspaces]]\nid = 2\nname = \"Code\"\nnative_space_id = 4\n";
        let (edited, config) = apply_patch(
            source,
            ConfigPatch::SetWorkspace {
                id: 1,
                name: "Browser".into(),
                display: None,
                native_space_id: Some(7),
                native_space_ids: Some(vec![4665]),
            },
        )
        .unwrap();
        assert_eq!(config.workspaces[0].space_ids(), vec![7, 4665]);
        assert!(config.workspaces[0].binds(4665));
        // Omitting the list keeps it.
        let (edited, config) = apply_patch(
            &edited,
            ConfigPatch::SetWorkspace {
                id: 1,
                name: "Web".into(),
                display: None,
                native_space_id: Some(7),
                native_space_ids: None,
            },
        )
        .unwrap();
        assert_eq!(config.workspaces[0].native_space_ids, vec![4665]);
        // A Space may belong to only one workspace.
        assert!(apply_patch(
            &edited,
            ConfigPatch::SetWorkspace {
                id: 2,
                name: "Code".into(),
                display: None,
                native_space_id: Some(4),
                native_space_ids: Some(vec![4665]),
            },
        )
        .is_err());
        let (_, config) = apply_patch(
            &edited,
            ConfigPatch::SetWorkspace {
                id: 1,
                name: "Web".into(),
                display: None,
                native_space_id: Some(7),
                native_space_ids: Some(Vec::new()),
            },
        )
        .unwrap();
        assert!(config.workspaces[0].native_space_ids.is_empty());
    }
}
