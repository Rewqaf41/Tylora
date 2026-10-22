//! Content of the hotkey cheat sheet shown while Option is held alone: the
//! active bindings grouped by purpose, with keys of the same action folded
//! together ("⌥ H J K L", "⌥ 1…9").

use tylora_core::commands::StateCommand;

pub type Group = (String, Vec<(String, String)>);

/// One folded row while building: group, label, modifiers, keys, details.
type Entry = (usize, String, String, Vec<String>, Vec<String>);

const GROUPS: [&str; 4] = ["Windows", "Workspaces & displays", "Layout", "Scratchpads"];

/// Group, label and an optional detail (a workspace name) for a command;
/// directions and digits stay in the key, details are listed after the label.
fn describe(
    command: &StateCommand,
    scratchpad: &dyn Fn(&str) -> String,
) -> (usize, String, Option<String>) {
    use StateCommand::*;
    match command {
        FocusNamedWorkspace { name } => (1, "Go to workspace".into(), Some(name.clone())),
        MoveWindowToNamedWorkspace { name } => {
            (1, "Move window to workspace".into(), Some(name.clone()))
        }
        other => {
            let (group, label) = describe_plain(other, scratchpad);
            (group, label, None)
        }
    }
}

fn describe_plain(command: &StateCommand, scratchpad: &dyn Fn(&str) -> String) -> (usize, String) {
    use StateCommand::*;
    match command {
        FocusWindow { .. } => (0, "Focus window".into()),
        MoveWindow { .. } | SwapWindow { .. } => (0, "Move / swap window".into()),
        ResizeWindow { .. } => (0, "Resize".into()),
        ToggleFloating { .. } => (0, "Toggle floating".into()),
        ToggleLayoutZoom { .. } => (0, "Layout Zoom".into()),
        FocusNextWindow => (0, "Next window".into()),
        FocusPreviousWindow => (0, "Previous window".into()),
        ReconcileWindows => (0, "Reconcile windows".into()),
        FocusWorkspace { .. } => (1, "Go to workspace".into()),
        FocusNamedWorkspace { .. } => (1, "Go to workspace".into()),
        FocusNextWorkspace => (1, "Next workspace".into()),
        FocusPreviousWorkspace => (1, "Previous workspace".into()),
        MoveWindowToWorkspace { .. } => (1, "Move window to workspace".into()),
        MoveWindowToNamedWorkspace { .. } => (1, "Move window to workspace".into()),
        MoveWindowToDisplay { .. }
        | MoveWindowToNextDisplay { .. }
        | MoveWindowToPreviousDisplay { .. } => (1, "Move window to display".into()),
        FocusDisplay { .. } | FocusDisplayNext => (1, "Focus display".into()),
        CycleLayout => (2, "Cycle layout".into()),
        SetLayout { layout } => (
            2,
            format!("Layout: {}", format!("{layout:?}").to_lowercase()),
        ),
        ScrollLayoutLeft | ScrollLayoutRight => (2, "Scroll".into()),
        CenterFocusedWindow => (2, "Center column".into()),
        ResizeScrollingColumn { .. } => (2, "Column width".into()),
        ToggleScratchpad { id } => (3, scratchpad(id)),
        ShowScratchpad { id } => (3, format!("Show {}", scratchpad(id))),
        HideScratchpad { id } => (3, format!("Hide {}", scratchpad(id))),
    }
}

/// "option+shift+h" → ("⌥⇧", "H").
fn split_chord(chord: &str) -> (String, String) {
    let mut modifiers = [false; 4];
    let mut key = String::new();
    for part in chord.split('+') {
        match part {
            "control" | "ctrl" => modifiers[0] = true,
            "option" | "alt" => modifiers[1] = true,
            "shift" => modifiers[2] = true,
            "command" | "cmd" => modifiers[3] = true,
            other => {
                key = match other {
                    "left" => "←".into(),
                    "right" => "→".into(),
                    "up" => "↑".into(),
                    "down" => "↓".into(),
                    "grave" => "`".into(),
                    "tab" => "Tab".into(),
                    "space" => "Space".into(),
                    "return" => "↩".into(),
                    "escape" => "Esc".into(),
                    single if single.chars().count() == 1 => single.to_uppercase(),
                    word => word.to_owned(),
                }
            }
        }
    }
    let glyphs: String = ["⌃", "⌥", "⇧", "⌘"]
        .iter()
        .zip(modifiers)
        .filter(|(_, on)| *on)
        .map(|(glyph, _)| *glyph)
        .collect();
    (glyphs, key)
}

/// Keys of one action: a digit run becomes "1…9", others are spaced.
fn fold_keys(keys: &[String]) -> String {
    let digits: Option<Vec<u32>> = keys
        .iter()
        .map(|key| key.parse::<u32>().ok().filter(|digit| *digit <= 9))
        .collect();
    if let Some(mut digits) = digits.filter(|digits| digits.len() > 2) {
        digits.sort_unstable();
        if digits.windows(2).all(|pair| pair[1] == pair[0] + 1) {
            return format!("{}…{}", digits[0], digits[digits.len() - 1]);
        }
    }
    keys.join(" ")
}

/// Builds the sheet from `(chord, command)` pairs in configuration order.
pub fn build<'a>(
    bindings: impl IntoIterator<Item = (&'a str, &'a StateCommand)>,
    scratchpad: &dyn Fn(&str) -> String,
) -> Vec<Group> {
    // (group, label, modifiers) → keys and details, in first-seen order.
    let mut entries: Vec<Entry> = Vec::new();
    for (chord, command) in bindings {
        let (group, label, detail) = describe(command, scratchpad);
        let (modifiers, key) = split_chord(chord);
        let index = match entries
            .iter()
            .position(|(g, l, m, ..)| *g == group && *l == label && *m == modifiers)
        {
            Some(index) => index,
            None => {
                entries.push((group, label, modifiers, Vec::new(), Vec::new()));
                entries.len() - 1
            }
        };
        let (.., keys, details) = &mut entries[index];
        if !keys.contains(&key) {
            keys.push(key);
            details.extend(detail);
        }
    }
    GROUPS
        .iter()
        .enumerate()
        .filter_map(|(index, title)| {
            let rows: Vec<(String, String)> = entries
                .iter()
                .filter(|(group, ..)| *group == index)
                .map(|(_, label, modifiers, keys, details)| {
                    let label = if details.is_empty() {
                        label.clone()
                    } else {
                        format!("{label} ({})", details.join(", "))
                    };
                    (
                        format!("{modifiers} {}", fold_keys(keys)).trim().to_owned(),
                        label,
                    )
                })
                .collect();
            (!rows.is_empty()).then(|| ((*title).to_owned(), rows))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tylora_core::commands::Direction;

    #[test]
    fn folds_directions_and_digits_and_groups_scratchpads() {
        let focus: Vec<StateCommand> = [
            Direction::Left,
            Direction::Down,
            Direction::Up,
            Direction::Right,
        ]
        .into_iter()
        .map(|direction| StateCommand::FocusWindow { direction })
        .collect();
        let workspaces: Vec<StateCommand> = (1..=9)
            .map(|id| StateCommand::FocusWorkspace { id })
            .collect();
        let scratch = StateCommand::ToggleScratchpad { id: "term".into() };
        let mut bindings: Vec<(String, &StateCommand)> = ["h", "j", "k", "l"]
            .iter()
            .zip(&focus)
            .map(|(key, command)| (format!("option+{key}"), command))
            .collect();
        bindings.extend(
            (1..=9)
                .zip(&workspaces)
                .map(|(digit, command)| (format!("option+{digit}"), command)),
        );
        bindings.push(("option+grave".into(), &scratch));
        let sheet = build(
            bindings
                .iter()
                .map(|(chord, command)| (chord.as_str(), *command)),
            &|id| format!("Scratchpad {id}"),
        );
        assert_eq!(sheet.len(), 3);
        assert_eq!(
            sheet[0],
            (
                "Windows".into(),
                vec![("⌥ H J K L".into(), "Focus window".into())]
            )
        );
        assert_eq!(sheet[1].1, vec![("⌥ 1…9".into(), "Go to workspace".into())]);
        assert_eq!(sheet[2].1, vec![("⌥ `".into(), "Scratchpad term".into())]);
    }

    #[test]
    fn named_workspaces_fold_with_their_names() {
        let commands: Vec<StateCommand> = ["Web", "Code", "Chat"]
            .into_iter()
            .map(|name| StateCommand::FocusNamedWorkspace { name: name.into() })
            .collect();
        let chords = ["option+1", "option+2", "option+3"];
        let sheet = build(chords.into_iter().zip(&commands), &|id| id.to_owned());
        assert_eq!(
            sheet[0].1,
            vec![("⌥ 1…3".into(), "Go to workspace (Web, Code, Chat)".into())]
        );
    }

    #[test]
    fn chords_use_mac_modifier_order() {
        assert_eq!(
            split_chord("shift+option+control+tab"),
            ("⌃⌥⇧".into(), "Tab".into())
        );
        assert_eq!(fold_keys(&["1".into(), "3".into(), "5".into()]), "1 3 5");
    }
}
