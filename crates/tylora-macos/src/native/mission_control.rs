//! Creating Desktops through Mission Control.
//!
//! macOS has no API for adding a Desktop while SIP is enabled; private
//! SkyLight calls create unmanaged Spaces that Mission Control never shows.
//! Mission Control itself exposes an "add desktop" button per display in the
//! Dock's accessibility tree (`mc.display` → `mc.spaces` → `mc.spaces.add`),
//! so Tylora opens Mission Control, presses that button for the target
//! display and closes it again. The caller must verify the result through
//! WindowServer; this module only reports what it pressed.

use super::*;
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub enum MissionControlError {
    DockNotRunning,
    NotOpened,
    DisplayNotFound,
    ButtonNotFound,
    Changed,
    Press(i32),
}

impl fmt::Display for MissionControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DockNotRunning => write!(f, "Dock is not running"),
            Self::NotOpened => write!(f, "Mission Control did not open"),
            Self::DisplayNotFound => {
                write!(f, "Mission Control shows no Spaces bar for that display")
            }
            Self::ButtonNotFound => write!(f, "Mission Control has no add-desktop button"),
            Self::Changed => write!(f, "Mission Control Spaces changed while planning"),
            Self::Press(code) => write!(f, "pressing a Mission Control button failed (AX {code})"),
        }
    }
}

fn dock_pid() -> Option<i32> {
    let output = Command::new("/usr/bin/pgrep")
        .args(["-x", "Dock"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn ax_text(element: AXRef, name: &'static CStr) -> Option<String> {
    let value = ax_attribute(element, name).ok()?;
    if unsafe { CFGetTypeID(value.0) != CFStringGetTypeID() } {
        return None;
    }
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer size is passed; CFStringGetCString NUL-terminates on success.
    (unsafe {
        CFStringGetCString(
            value.0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as isize,
            0x0800_0100,
        )
    } != 0)
        .then(|| {
            unsafe { CStr::from_ptr(buffer.as_ptr().cast()) }
                .to_string_lossy()
                .into_owned()
        })
}

/// Children of an AX element. Values are retained by the returned array.
fn children(element: AXRef) -> (Option<OwnedCF>, Vec<AXRef>) {
    let Ok(array) = ax_attribute(element, c"AXChildren") else {
        return (None, Vec::new());
    };
    if unsafe { CFGetTypeID(array.0) != CFArrayGetTypeID() } {
        return (None, Vec::new());
    }
    let count = unsafe { CFArrayGetCount(array.0) };
    let items = (0..count)
        .map(|index| unsafe { CFArrayGetValueAtIndex(array.0, index) })
        .filter(|item| !item.is_null())
        .collect();
    (Some(array), items)
}

/// Depth-first search for elements with an AXIdentifier.
fn find_all(
    element: AXRef,
    identifier: &str,
    depth: usize,
    keep: &mut Vec<OwnedCF>,
    out: &mut Vec<AXRef>,
) {
    if depth > 6 {
        return;
    }
    if ax_text(element, c"AXIdentifier").as_deref() == Some(identifier) {
        out.push(element);
        return;
    }
    let (array, items) = children(element);
    if let Some(array) = array {
        keep.push(array);
    }
    for item in items {
        find_all(item, identifier, depth + 1, keep, out);
    }
}

fn mission_control_open(dock: AXRef) -> bool {
    let mut keep = Vec::new();
    let mut found = Vec::new();
    find_all(dock, "mc", 0, &mut keep, &mut found);
    !found.is_empty()
}

fn toggle_mission_control() {
    let _ = Command::new("/usr/bin/open")
        .args(["-b", "com.apple.exposelauncher"])
        .status();
}

/// Opens Mission Control unless it already is, runs `body` against the Dock
/// and closes Mission Control again when this call opened it.
fn with_mission_control<T>(
    body: impl FnOnce(AXRef) -> Result<T, MissionControlError>,
) -> Result<T, MissionControlError> {
    let pid = dock_pid().ok_or(MissionControlError::DockNotRunning)?;
    let dock = OwnedCF(unsafe { AXUIElementCreateApplication(pid) });
    if dock.0.is_null() {
        return Err(MissionControlError::DockNotRunning);
    }
    let was_open = mission_control_open(dock.0);
    if !was_open {
        toggle_mission_control();
    }
    let result = body(dock.0);
    if !was_open && mission_control_open(dock.0) {
        toggle_mission_control();
    }
    result
}

/// Finds the Mission Control group of `display`, waiting until `deadline`
/// for Mission Control to appear. Elements stay alive through `keep`.
fn display_group(
    dock: AXRef,
    display: DisplayId,
    deadline: Instant,
    keep: &mut Vec<OwnedCF>,
) -> Result<AXRef, MissionControlError> {
    loop {
        let mut groups = Vec::new();
        find_all(dock, "mc.display", 0, keep, &mut groups);
        if groups.is_empty() {
            if Instant::now() >= deadline {
                return Err(MissionControlError::NotOpened);
            }
            thread::sleep(Duration::from_millis(20));
            continue;
        }
        // Each display's Mission Control group covers that display.
        return groups
            .iter()
            .copied()
            .find(|group| {
                ax_rect(*group).ok().is_some_and(|frame| {
                    let centre = Rect::new(
                        frame.x + frame.width / 2.0,
                        frame.y + frame.height / 2.0,
                        1.0,
                        1.0,
                    );
                    centre.ok().and_then(display_for) == Some(display)
                })
            })
            .ok_or(MissionControlError::DisplayNotFound);
    }
}

/// The entries of `display`'s Spaces bar: Desktops and fullscreen Spaces in
/// Mission Control order. Elements stay alive through `keep`.
fn space_buttons(
    dock: AXRef,
    display: DisplayId,
    deadline: Instant,
    keep: &mut Vec<OwnedCF>,
) -> Result<Vec<AXRef>, MissionControlError> {
    let group = display_group(dock, display, deadline, keep)?;
    let mut lists = Vec::new();
    find_all(group, "mc.spaces.list", 0, keep, &mut lists);
    let list = *lists.first().ok_or(MissionControlError::ButtonNotFound)?;
    let (array, items) = children(list);
    if let Some(array) = array {
        keep.push(array);
    }
    Ok(items)
}

/// Waits until the Dock shows `count` entries in `display`'s Spaces bar,
/// which it does as soon as it has applied an add or remove. Usually takes
/// well under 200 ms; gives up silently after a second so a slow Dock only
/// costs time, and callers verify the result through WindowServer anyway.
fn wait_for_spaces(dock: AXRef, display: DisplayId, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        let mut keep = Vec::new();
        if space_buttons(dock, display, deadline, &mut keep).is_ok_and(|items| items.len() == count)
        {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Adds `count` Desktops to `display` and closes Mission Control again.
/// Returns how many presses succeeded.
pub fn add_desktops(display: DisplayId, count: u8) -> Result<u8, MissionControlError> {
    with_mission_control(|dock| {
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut pressed = 0u8;
        while pressed < count {
            let mut keep = Vec::new();
            let before = space_buttons(dock, display, deadline, &mut keep)?.len();
            let group = display_group(dock, display, deadline, &mut keep)?;
            let mut buttons = Vec::new();
            find_all(group, "mc.spaces.add", 0, &mut keep, &mut buttons);
            let button = *buttons.first().ok_or(MissionControlError::ButtonNotFound)?;
            let action = cf_string(c"AXPress");
            let code = unsafe { AXUIElementPerformAction(button, action.0) };
            if code != 0 {
                return Err(MissionControlError::Press(code));
            }
            pressed += 1;
            // The next press or closing must not race the Dock.
            wait_for_spaces(dock, display, before + 1);
        }
        Ok(pressed)
    })
}

/// Removes the Spaces at `positions` of `display`'s Spaces bar, which lists
/// Desktops and fullscreen Spaces in Mission Control order. The bar must
/// still hold `expected` entries, otherwise the strip changed since the
/// caller planned and nothing is removed. macOS moves the windows of a
/// removed Desktop to a neighbour, so callers empty them first. Returns how
/// many Spaces were removed.
pub fn remove_spaces(
    display: DisplayId,
    positions: &[usize],
    expected: usize,
) -> Result<u8, MissionControlError> {
    let mut positions = positions.to_vec();
    positions.sort_unstable();
    positions.dedup();
    with_mission_control(|dock| {
        let deadline = Instant::now() + Duration::from_millis(1500);
        let mut removed = 0usize;
        // Highest first, so the remaining positions stay valid.
        for position in positions.into_iter().rev() {
            let mut keep = Vec::new();
            let items = space_buttons(dock, display, deadline, &mut keep)?;
            if items.len() + removed != expected {
                return Err(MissionControlError::Changed);
            }
            let button = *items
                .get(position)
                .ok_or(MissionControlError::ButtonNotFound)?;
            let action = cf_string(c"AXRemoveDesktop");
            let code = unsafe { AXUIElementPerformAction(button, action.0) };
            if code != 0 {
                return Err(MissionControlError::Press(code));
            }
            removed += 1;
            // The next press or closing must not race the Dock.
            wait_for_spaces(dock, display, expected - removed);
        }
        Ok(u8::try_from(removed).unwrap_or(u8::MAX))
    })
}
