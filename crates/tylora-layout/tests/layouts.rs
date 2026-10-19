//! Pure model tests for Dwindle, Scrolling and layout switching. These do not
//! exercise macOS; real-window behaviour is checked separately.

use std::collections::HashSet;
use tylora_core::{
    commands::{CommandError, Direction, StateCommand},
    coordinator::LayoutPolicy,
    layout::{
        DwindleSettings, FocusOrigin, LayoutEnvironment, LayoutKind, ScrollingSettings,
        SplitOrientation, ViewportAlignment,
    },
    registry::{ManagedWindowId, ProcessInstance},
    Rect,
};
use tylora_layout::{
    Axis, BspLayout, ColumnPlacement, LayoutSettings, ScrollingLayout, ScrollingMode, TilingLayout,
};

fn id(n: u64) -> ManagedWindowId {
    ManagedWindowId {
        process: ProcessInstance { pid: 7, epoch: 1 },
        token: n,
    }
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect::new(x, y, w, h).unwrap()
}
fn area() -> Rect {
    rect(0.0, 33.0, 1470.0, 923.0)
}
fn display() -> Rect {
    rect(0.0, 0.0, 1470.0, 956.0)
}
fn strip_frame(layout: &ScrollingLayout, window: ManagedWindowId, area: Rect) -> Rect {
    layout
        .calculate_layout(area)
        .into_iter()
        .find(|(w, _)| *w == window)
        .unwrap()
        .1
}
fn assert_unique(frames: &[(ManagedWindowId, Rect)]) {
    let unique: HashSet<_> = frames.iter().map(|(id, _)| *id).collect();
    assert_eq!(unique.len(), frames.len(), "duplicate window in layout");
}

// ---------------------------------------------------------------- Dwindle

#[test]
fn dwindle_splits_focused_leaf_and_alternates_axis() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    tree.insert_window(id(1));
    tree.insert_window(id(2));
    // First split follows the configured initial orientation (side by side).
    assert_eq!(tree.splits()[0].1, Axis::Vertical);
    // Focus the first window: the next one splits it, not the newest leaf.
    tree.set_focused(Some(id(1)));
    tree.insert_window(id(3));
    let splits = tree.splits();
    assert_eq!(splits.len(), 2);
    assert_eq!(splits[1].1, Axis::Horizontal, "child split alternates");
    let frames = tree.calculate_layout(area());
    let one = frames.iter().find(|(w, _)| *w == id(1)).unwrap().1;
    let three = frames.iter().find(|(w, _)| *w == id(3)).unwrap().1;
    let two = frames.iter().find(|(w, _)| *w == id(2)).unwrap().1;
    assert_eq!(one.x, three.x, "3 stacks under 1");
    assert!(three.y > one.y);
    assert!(two.x > one.x + one.width, "2 keeps the right half");
}

#[test]
fn dwindle_initial_orientation_and_ratio_are_configurable() {
    let mut tree = BspLayout::dwindle(DwindleSettings {
        initial_orientation: SplitOrientation::Horizontal,
        split_ratio: 0.7,
    });
    tree.insert_window(id(1));
    tree.insert_window(id(2));
    let (_, axis, ratio) = tree.splits()[0];
    assert_eq!(axis, Axis::Horizontal);
    assert!((ratio - 0.7).abs() < 1e-9);
    let frames = tree.calculate_layout(area());
    assert!(frames[0].1.height > frames[1].1.height);
    assert_eq!(frames[0].1.x, frames[1].1.x);
}

#[test]
fn dwindle_new_window_follows_last_when_unfocused() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    for n in 1..=4 {
        tree.insert_window(id(n));
    }
    // Each insertion focuses the new window, producing the dwindle spiral.
    let axes: Vec<_> = tree.splits().iter().map(|split| split.1).collect();
    assert_eq!(axes, vec![Axis::Vertical, Axis::Horizontal, Axis::Vertical]);
}

#[test]
fn dwindle_remove_middle_collapses_without_corruption() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    for n in 1..=5 {
        tree.insert_window(id(n));
    }
    assert!(tree.remove_window(id(3)));
    assert!(!tree.contains(id(3)));
    assert_eq!(tree.windows(), vec![id(1), id(2), id(4), id(5)]);
    assert_eq!(tree.splits().len(), 3, "one split per extra window");
    let frames = tree.calculate_layout(area());
    assert_unique(&frames);
    assert_eq!(frames.len(), 4);
    // Removing the focused window clears the insertion anchor safely.
    tree.set_focused(Some(id(5)));
    assert!(tree.remove_window(id(5)));
    assert_eq!(tree.focused(), None);
    tree.insert_window(id(6));
    assert_eq!(tree.len(), 4);
    for (_, frame) in tree.calculate_layout(area()) {
        assert!(frame.width > 0.0 && frame.height > 0.0);
    }
}

#[test]
fn dwindle_swap_and_mouse_resize_change_ratio() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    tree.insert_window(id(1));
    tree.insert_window(id(2));
    tree.insert_window(id(3));
    assert!(tree.swap_windows(id(1), id(3)));
    assert_eq!(tree.windows()[0], id(3));
    let before = tree.splits()[0].2;
    // Mouse resize maps to directional resize of the nearest split.
    assert!(tree.resize_window(id(3), Direction::Right, 60.0, area()));
    assert!(tree.splits()[0].2 > before);
    assert!(tree.resize_window(id(2), Direction::Down, 40.0, area()));
}

#[test]
fn dwindle_respects_gaps_minimums_and_display_resize() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    tree.set_gaps(12.0, 20.0).unwrap();
    tree.set_min_size(200.0, 120.0).unwrap();
    for n in 1..=3 {
        tree.insert_window(id(n));
    }
    let wide = tree.calculate_layout(area());
    assert_eq!(wide[0].1.x, 20.0);
    assert_eq!(wide[0].1.y, 53.0);
    let narrow = tree.calculate_layout(rect(0.0, 0.0, 800.0, 600.0));
    for (_, frame) in &narrow {
        assert!(frame.width >= 200.0 - 1e-6 && frame.height >= 120.0 - 1e-6);
        assert!(frame.x + frame.width <= 780.0 + 1e-6);
    }
    assert!(wide[0].1.width > narrow[0].1.width);
}

#[test]
fn dwindle_churn_keeps_tree_valid() {
    let mut tree = BspLayout::dwindle(DwindleSettings::default());
    let mut seed = 0x51_7cc1_b727_220a_u64;
    for _ in 0..400 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let candidate = id((seed >> 33) % 24);
        match seed & 3 {
            0 | 1 => {
                tree.insert_window(candidate);
            }
            2 => {
                tree.remove_window(candidate);
            }
            _ => tree.set_focused(Some(candidate)),
        }
        let frames = tree.calculate_layout(area());
        assert_unique(&frames);
        assert_eq!(frames.len(), tree.len());
        assert_eq!(tree.splits().len(), tree.len().saturating_sub(1));
    }
}

// --------------------------------------------------------------- Scrolling

fn strip(n: u64) -> ScrollingLayout {
    let mut layout = ScrollingLayout::new(ScrollingSettings::default());
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(display()),
        other_displays: Vec::new(),
    });
    for token in 1..=n {
        layout.insert_window(id(token));
    }
    layout
}

#[test]
fn scrolling_strip_keeps_column_widths_and_order() {
    let layout = strip(4);
    assert_eq!(layout.windows(), vec![id(1), id(2), id(3), id(4)]);
    let view = layout.view(area());
    let usable = 1470.0 - 16.0;
    for &(_, _, width) in &view.columns {
        assert!((width - usable * 0.65).abs() < 1e-6, "default 65% width");
    }
    // Strip is wider than the display; new windows were revealed in turn.
    assert!(view.strip_width > usable);
    let active = strip_frame(&layout, id(4), area());
    assert!(active.x >= 8.0 - 1e-6 && active.x + active.width <= 1462.0 + 1e-6);
}

#[test]
fn scrolling_insert_goes_right_of_active_and_reveals() {
    let mut layout = strip(3);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    layout.insert_window(id(9));
    assert_eq!(layout.windows(), vec![id(1), id(9), id(2), id(3)]);
    assert_eq!(layout.active(), Some(id(9)));
    let new = strip_frame(&layout, id(9), area());
    assert!(new.x >= 8.0 - 1e-6 && new.x + new.width <= 1462.0 + 1e-6);
}

#[test]
fn scrolling_remove_moves_focus_to_neighbor() {
    let mut layout = strip(3);
    layout.focus_changed(id(2), FocusOrigin::Command, area());
    assert!(layout.remove_window(id(2)));
    assert_eq!(layout.active(), Some(id(3)));
    assert!(layout.remove_window(id(3)));
    assert_eq!(layout.active(), Some(id(1)));
    assert!(layout.remove_window(id(1)));
    assert_eq!(layout.active(), None);
    assert!(layout.calculate_layout(area()).is_empty());
}

#[test]
fn scrolling_nearest_focus_scrolls_only_when_needed() {
    let mut layout = strip(4);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let offset = layout.view(area()).offset;
    assert_eq!(offset, 0.0);
    // Column 2 starts inside the viewport but ends outside: nearest scrolls
    // just enough to show its right edge.
    assert!(layout.focus_changed(id(2), FocusOrigin::Command, area()));
    let view = layout.view(area());
    let (_, x, w) = view.columns[1];
    assert!((view.offset - (x + w - view.usable.width)).abs() < 1e-6);
    // Focusing a fully visible column does not move the strip.
    let before = view.offset;
    layout.focus_changed(id(2), FocusOrigin::Command, area());
    assert_eq!(layout.view(area()).offset, before);
}

#[test]
fn scrolling_center_alignment_and_center_command() {
    let mut layout = ScrollingLayout::new(ScrollingSettings {
        viewport_alignment: ViewportAlignment::Center,
        ..ScrollingSettings::default()
    });
    for token in 1..=4 {
        layout.insert_window(id(token));
    }
    layout.focus_changed(id(2), FocusOrigin::Command, area());
    let view = layout.view(area());
    let (_, x, w) = view.columns[1];
    assert!((x + w / 2.0 - view.offset - view.usable.width / 2.0).abs() < 1e-6);

    let mut nearest = strip(4);
    nearest.focus_changed(id(1), FocusOrigin::Command, area());
    assert!(nearest
        .layout_command(&StateCommand::CenterFocusedWindow, Some(id(3)), area())
        .unwrap()
        .is_ok());
    let view = nearest.view(area());
    let (_, x, w) = view.columns[2];
    assert!((x + w / 2.0 - view.offset - view.usable.width / 2.0).abs() < 1e-6);
    assert_eq!(nearest.active(), Some(id(3)));
}

#[test]
fn scrolling_manual_scroll_moves_viewport_without_focus() {
    let mut layout = strip(4);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let result = layout.layout_command(&StateCommand::ScrollLayoutRight, Some(id(1)), area());
    assert_eq!(result, Some(Ok(())));
    assert!(layout.view(area()).offset > 0.0);
    assert_eq!(layout.active(), Some(id(1)));
    // Scrolling past the start is clamped and reported as no movement.
    layout
        .layout_command(&StateCommand::ScrollLayoutLeft, Some(id(1)), area())
        .unwrap()
        .unwrap();
    assert_eq!(
        layout.layout_command(&StateCommand::ScrollLayoutLeft, Some(id(1)), area()),
        Some(Err(CommandError::NoNeighbor))
    );
}

#[test]
fn scrolling_offscreen_columns_are_parked_in_display_corner() {
    let mut layout = strip(4);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let (mode, placements) = layout.placements(area());
    assert_eq!(mode, ScrollingMode::Strip);
    let state = |n: u64| placements.iter().find(|p| p.0 == id(n)).unwrap();
    assert_eq!(state(1).2, ColumnPlacement::Visible);
    assert_eq!(state(2).2, ColumnPlacement::Partial);
    for n in [3, 4] {
        let (_, frame, placement) = state(n);
        assert_eq!(*placement, ColumnPlacement::Parked);
        // A 40 x 32 point sliver stays on the display, as WindowServer requires.
        assert_eq!(frame.x, 1470.0 - 40.0);
        assert_eq!(frame.y, 956.0 - 32.0);
    }
    assert!(layout.is_parked(id(3), area()));
    assert!(!layout.is_parked(id(1), area()));
}

#[test]
fn scrolling_never_parks_onto_another_display() {
    // A second display directly to the right blocks right-side parking.
    let mut layout = strip(4);
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(display()),
        other_displays: vec![rect(1470.0, 0.0, 1920.0, 1080.0)],
    });
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let (mode, placements) = layout.placements(area());
    assert_eq!(mode, ScrollingMode::Strip);
    for (window, frame, placement) in &placements {
        if *placement == ColumnPlacement::Parked {
            assert!(frame.x < 0.0, "{window:?} parked left instead");
        }
        assert!(
            frame.x + frame.width <= 1470.0 + 0.5,
            "nothing spills right"
        );
    }
    // Displays on both sides: no safe park, so the strip is bounded.
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(display()),
        other_displays: vec![
            rect(1470.0, 0.0, 1920.0, 1080.0),
            rect(-1920.0, 0.0, 1920.0, 1080.0),
        ],
    });
    let (mode, placements) = layout.placements(area());
    assert_eq!(mode, ScrollingMode::Bounded);
    for (_, frame, placement) in &placements {
        assert_eq!(*placement, ColumnPlacement::Visible);
        assert!(frame.x >= 8.0 - 1e-6 && frame.x + frame.width <= 1462.0 + 1e-6);
    }
}

#[test]
fn scrolling_system_focus_scrolls_only_for_parked_windows() {
    let mut layout = strip(4);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let before = layout.view(area()).offset;
    // Partially visible: a click must not move the strip.
    assert!(layout.focus_changed(id(2), FocusOrigin::System, area()));
    assert_eq!(layout.view(area()).offset, before);
    // Pointer focus never scrolls.
    layout.focus_changed(id(1), FocusOrigin::Pointer, area());
    assert_eq!(layout.view(area()).offset, before);
    // Cmd-Tab to a parked window brings it into view.
    assert!(layout.focus_changed(id(4), FocusOrigin::System, area()));
    let shown = strip_frame(&layout, id(4), area());
    assert!(shown.x >= 8.0 - 1e-6 && shown.x + shown.width <= 1462.0 + 1e-6);
}

#[test]
fn scrolling_resize_keeps_viewport_and_shifts_following_columns() {
    let mut layout = strip(3);
    layout.focus_changed(id(1), FocusOrigin::Command, area());
    let before = layout.view(area());
    // Mouse resize to the right widens the active column.
    assert!(layout.resize_window(id(1), Direction::Right, 100.0, area()));
    let after = layout.view(area());
    assert_eq!(after.offset, before.offset, "viewport not reset");
    assert!((after.columns[0].2 - before.columns[0].2 - 100.0).abs() < 1e-6);
    assert!((after.columns[1].1 - before.columns[1].1 - 100.0).abs() < 1e-6);
    assert!(!layout.resize_window(id(1), Direction::Up, 50.0, area()));
    // Width is clamped to the configured bounds and never negative.
    assert!(layout.resize_window(id(1), Direction::Left, 10_000.0, area()));
    assert!((layout.column_width(id(1)).unwrap() - 0.2).abs() < 1e-9);
}

#[test]
fn scrolling_width_presets_cycle() {
    let mut layout = strip(1);
    let command = StateCommand::ResizeScrollingColumn { delta: None };
    let mut seen = Vec::new();
    for _ in 0..5 {
        layout
            .layout_command(&command, Some(id(1)), area())
            .unwrap()
            .unwrap();
        seen.push((layout.column_width(id(1)).unwrap() * 100.0).round());
    }
    assert_eq!(seen, vec![100.0, 33.0, 50.0, 65.0, 100.0]);
    assert_eq!(
        layout.layout_command(
            &StateCommand::ResizeScrollingColumn {
                delta: Some(f64::NAN)
            },
            Some(id(1)),
            area()
        ),
        Some(Err(CommandError::InvalidArgument))
    );
}

#[test]
fn scrolling_swap_neighbors_and_learned_minimum() {
    let mut layout = strip(3);
    assert_eq!(layout.neighbor(id(2), Direction::Left), Some(id(1)));
    assert_eq!(layout.neighbor(id(3), Direction::Right), None);
    assert_eq!(layout.neighbor(id(2), Direction::Up), None);
    assert!(layout.swap_windows(id(1), id(2)));
    assert_eq!(layout.windows(), vec![id(2), id(1), id(3)]);
    // A window that refused a narrow frame keeps its learned minimum width.
    layout
        .layout_command(
            &StateCommand::ResizeScrollingColumn { delta: Some(-1.0) },
            Some(id(3)),
            area(),
        )
        .unwrap()
        .unwrap();
    assert!(layout.set_window_min_size(id(3), 700.0, 0.0));
    let view = layout.view(area());
    assert!(view.columns[2].2 >= 700.0);
}

#[test]
fn scrolling_adapts_to_display_resolution() {
    let layout = strip(2);
    let small = layout.view(rect(0.0, 0.0, 1280.0, 800.0));
    let large = layout.view(rect(0.0, 0.0, 2560.0, 1440.0));
    let ratio = |view: &tylora_layout::ScrollingView| view.columns[0].2 / view.usable.width;
    assert!((ratio(&small) - ratio(&large)).abs() < 1e-9);
    for frame in layout.calculate_layout(rect(0.0, 0.0, 1280.0, 800.0)) {
        assert!(frame.1.width > 0.0 && frame.1.height > 0.0);
    }
}

#[test]
fn scrolling_churn_has_no_ghosts_or_duplicates() {
    let mut layout = strip(0);
    let mut live = HashSet::new();
    let mut seed = 0xdead_beef_u64;
    for _ in 0..500 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let candidate = id((seed >> 33) % 16);
        match seed % 5 {
            0 | 1 => {
                layout.insert_window(candidate);
                live.insert(candidate);
            }
            2 => {
                layout.remove_window(candidate);
                live.remove(&candidate);
            }
            3 => {
                layout.focus_changed(candidate, FocusOrigin::Command, area());
            }
            _ => {
                layout.layout_command(&StateCommand::ScrollLayoutRight, None, area());
            }
        }
        let frames = layout.calculate_layout(area());
        assert_unique(&frames);
        let ids: HashSet<_> = frames.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, live, "layout membership equals live windows");
        assert!(layout.active().is_none_or(|active| live.contains(&active)));
        for (_, frame) in frames {
            assert!(frame.width > 0.0 && frame.height > 0.0);
        }
    }
}

// ----------------------------------------------------------- switching

#[test]
fn switching_between_all_layouts_keeps_every_window_once() {
    let mut layout = TilingLayout::new(LayoutKind::Bsp, &LayoutSettings::default());
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(display()),
        other_displays: Vec::new(),
    });
    for token in 1..=5 {
        LayoutPolicy::insert_window(&mut layout, id(token));
    }
    let original: HashSet<_> = layout.windows().into_iter().collect();
    for kind in [
        LayoutKind::Dwindle,
        LayoutKind::Scrolling,
        LayoutKind::Bsp,
        LayoutKind::Scrolling,
        LayoutKind::Dwindle,
        LayoutKind::Bsp,
    ] {
        assert!(layout.set_kind(kind));
        assert_eq!(layout.kind(), kind);
        let frames = layout.calculate_layout(area());
        assert_unique(&frames);
        let ids: HashSet<_> = frames.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, original);
    }
    assert!(!layout.set_kind(LayoutKind::Bsp));
}

#[test]
fn switching_back_restores_previous_ratios_and_widths() {
    let mut layout = TilingLayout::new(LayoutKind::Bsp, &LayoutSettings::default());
    for token in 1..=3 {
        LayoutPolicy::insert_window(&mut layout, id(token));
    }
    let node = layout.splits()[0].0;
    layout.resize_split(node, 0.7).unwrap();
    layout.set_kind(LayoutKind::Scrolling);
    LayoutPolicy::resize_window(&mut layout, id(2), Direction::Right, 120.0, area());
    let width = layout.scrolling().column_width(id(2)).unwrap();
    layout.set_kind(LayoutKind::Bsp);
    assert!((layout.splits()[0].2 - 0.7).abs() < 1e-9, "BSP ratio kept");
    // Membership changes while away are reconciled on return.
    LayoutPolicy::remove_window(&mut layout, id(1));
    LayoutPolicy::insert_window(&mut layout, id(4));
    layout.set_kind(LayoutKind::Scrolling);
    assert_eq!(layout.scrolling().column_width(id(2)), Some(width));
    let ids: HashSet<_> = layout.windows().into_iter().collect();
    assert_eq!(ids, [id(2), id(3), id(4)].into_iter().collect());
}

#[test]
fn switching_to_bsp_returns_parked_windows_inside_work_area() {
    let mut layout = TilingLayout::new(LayoutKind::Scrolling, &LayoutSettings::default());
    layout.set_environment(&LayoutEnvironment {
        display_frame: Some(display()),
        other_displays: Vec::new(),
    });
    for token in 1..=5 {
        LayoutPolicy::insert_window(&mut layout, id(token));
    }
    assert!(layout
        .calculate_layout(area())
        .iter()
        .any(|(_, f)| f.x < 0.0 || f.x >= 1400.0));
    layout.set_kind(LayoutKind::Dwindle);
    for (_, frame) in layout.calculate_layout(area()) {
        assert!(frame.x >= 0.0 && frame.x + frame.width <= 1470.0 + 1e-6);
        assert!(frame.y >= 33.0 && frame.y + frame.height <= 956.0 + 1e-6);
    }
}

#[test]
fn layout_commands_report_unsupported_outside_scrolling() {
    let mut layout = TilingLayout::default();
    LayoutPolicy::insert_window(&mut layout, id(1));
    assert_eq!(
        layout.layout_command(&StateCommand::ScrollLayoutLeft, Some(id(1)), area()),
        Some(Err(CommandError::Unsupported))
    );
    assert_eq!(
        layout.layout_command(
            &StateCommand::SetLayout {
                layout: LayoutKind::Scrolling
            },
            Some(id(1)),
            area()
        ),
        Some(Ok(()))
    );
    assert_eq!(layout.kind(), LayoutKind::Scrolling);
    assert_eq!(
        layout.layout_command(&StateCommand::CycleLayout, Some(id(1)), area()),
        Some(Ok(()))
    );
    assert_eq!(layout.kind(), LayoutKind::Bsp);
}

// ------------------------------------------- coordinator + scrolling (mock AX)

mod with_coordinator {
    use super::*;
    use tylora_core::{
        coordinator::AWAIT_REOBSERVATION,
        registry::{ManagementMode, WindowLifecycle, WindowSnapshot},
        Coordinator, CoordinatorEvent, DisplayId,
    };

    fn snapshot(token: u64) -> WindowSnapshot {
        WindowSnapshot {
            id: id(token),
            cg_window_id: Some(token as u32),
            native_space_id: None,
            application_id: Some("test.app".into()),
            title: None,
            frame: rect(10.0, 40.0, 300.0, 200.0),
            display_id: Some(DisplayId(1)),
            lifecycle: WindowLifecycle::Active,
            mode: ManagementMode::Tiled,
        }
    }
    fn scrolling_coordinator(n: u64) -> Coordinator<TilingLayout> {
        let mut c = Coordinator::new(
            TilingLayout::new(LayoutKind::Scrolling, &LayoutSettings::default()),
            DisplayId(1),
            area(),
        );
        c.set_environment(LayoutEnvironment {
            display_frame: Some(display()),
            other_displays: Vec::new(),
        });
        for token in 1..=n {
            c.handle(CoordinatorEvent::Observed(snapshot(token)));
        }
        c
    }

    #[test]
    fn ax_failure_keeps_window_and_does_not_loop() {
        let mut c = scrolling_coordinator(3);
        let first = c.plan();
        assert_eq!(first.len(), 3);
        for command in &first {
            c.handle(CoordinatorEvent::Applied {
                command: *command,
                result: Err(-25204),
            });
        }
        assert_eq!(c.registry.len(), 3, "AX errors never drop windows");
        assert!(
            c.plan().is_empty(),
            "failed targets back off instead of looping"
        );
        assert_eq!(c.metrics.ax_failures, 3);
    }

    #[test]
    fn silent_clamp_by_window_server_is_counted_and_backed_off() {
        let mut c = scrolling_coordinator(4);
        let commands = c.plan();
        let parked = commands
            .iter()
            .find(|command| c.is_parked(command.id))
            .copied()
            .expect("a parked column");
        // Pretend WindowServer kept more of the window visible than requested.
        let mut clamped = parked.target;
        clamped.x -= 20.0;
        c.handle(CoordinatorEvent::Applied {
            command: parked,
            result: Ok(clamped),
        });
        assert_eq!(c.metrics.geometry_mismatches, 1);
        assert!(
            c.plan().iter().all(|command| command.id != parked.id),
            "mismatch retries wait for the backoff"
        );
    }

    #[test]
    fn unobserved_slots_expire_without_ghosts() {
        let mut c = scrolling_coordinator(3);
        let saved = c.layout.clone();
        // Window 2 closed while its Space was inactive: it never reappears.
        c.handle(CoordinatorEvent::Destroyed(id(1)));
        c.handle(CoordinatorEvent::Destroyed(id(2)));
        c.handle(CoordinatorEvent::Destroyed(id(3)));
        c.replace_layout(saved);
        assert_eq!(c.awaiting(), 3);
        c.handle(CoordinatorEvent::Observed(snapshot(1)));
        c.handle(CoordinatorEvent::Observed(snapshot(3)));
        assert_eq!(c.awaiting(), 1);
        assert_eq!(c.layout.windows(), vec![id(1), id(2), id(3)], "slot kept");
        assert!(c.plan().iter().all(|command| command.id != id(2)));
        c.expire_awaiting(std::time::Instant::now() + AWAIT_REOBSERVATION);
        assert_eq!(c.layout.windows(), vec![id(1), id(3)]);
        assert_eq!(c.awaiting(), 0);
    }

    #[test]
    fn focus_commands_scroll_and_move_reorders_strip() {
        let mut c = scrolling_coordinator(4);
        let effect = c
            .execute(
                &StateCommand::FocusWindow {
                    direction: Direction::Left,
                },
                Some(id(4)),
            )
            .unwrap();
        assert_eq!(effect, tylora_core::CommandEffect::Focus(id(3)));
        assert_eq!(
            c.execute(
                &StateCommand::FocusWindow {
                    direction: Direction::Up
                },
                Some(id(3))
            ),
            Err(CommandError::NoNeighbor)
        );
        c.execute(
            &StateCommand::MoveWindow {
                direction: Direction::Left,
            },
            Some(id(3)),
        )
        .unwrap();
        assert_eq!(c.layout.windows(), vec![id(1), id(3), id(2), id(4)]);
        let frame = c
            .layout
            .calculate_layout(area())
            .into_iter()
            .find(|(w, _)| *w == id(3))
            .unwrap()
            .1;
        assert!(
            frame.x >= 0.0 && frame.x + frame.width <= 1470.0,
            "moved column visible"
        );
        assert_eq!(
            c.execute(&StateCommand::FocusPreviousWindow, Some(id(1)))
                .unwrap(),
            tylora_core::CommandEffect::Focus(id(4))
        );
    }

    #[test]
    fn floating_and_resize_with_minimum_size() {
        let mut c = scrolling_coordinator(2);
        c.execute(
            &StateCommand::ToggleFloating {
                window: Some(id(2)),
            },
            None,
        )
        .unwrap();
        assert_eq!(c.layout.windows(), vec![id(1)]);
        c.execute(
            &StateCommand::ToggleFloating {
                window: Some(id(2)),
            },
            None,
        )
        .unwrap();
        assert_eq!(c.layout.len(), 2);
        assert!(LayoutPolicy::set_window_min_size(
            &mut c.layout,
            id(1),
            1200.0,
            0.0
        ));
        let view = c.layout.scrolling().view(area());
        assert!(view.columns[0].2 >= 1200.0);
    }
}

mod reacquired_windows {
    use super::*;
    use tylora_core::{
        registry::{ManagementMode, WindowLifecycle, WindowSnapshot},
        Coordinator, CoordinatorEvent, DisplayId,
    };

    fn window(token: u64, cg: u32) -> WindowSnapshot {
        WindowSnapshot {
            id: id(token),
            cg_window_id: Some(cg),
            native_space_id: None,
            application_id: Some("test.app".into()),
            title: None,
            frame: rect(10.0, 40.0, 300.0, 200.0),
            display_id: Some(DisplayId(1)),
            lifecycle: WindowLifecycle::Active,
            mode: ManagementMode::Tiled,
        }
    }

    fn single_window_returns_with_new_token_and_keeps_full_frame(kind: LayoutKind) {
        let mut c = Coordinator::new(
            TilingLayout::new(kind, &LayoutSettings::default()),
            DisplayId(1),
            area(),
        );
        c.handle(CoordinatorEvent::Observed(window(27, 500)));
        let full = c.layout.calculate_layout(area())[0].1;
        let saved = c.layout.clone();
        // Leaving the Space drops the window; the next scan re-registers the
        // same CGWindowID under a new AX token.
        c.handle(CoordinatorEvent::Destroyed(id(27)));
        c.replace_layout(saved);
        assert_eq!(c.awaiting(), 1);
        c.handle(CoordinatorEvent::Observed(window(29, 500)));
        assert_eq!(c.awaiting(), 0, "placeholder taken over, not kept");
        assert_eq!(c.layout.windows(), vec![id(29)]);
        let frames = c.layout.calculate_layout(area());
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].1, full, "no half-width frame while waiting");
    }

    #[test]
    fn bsp_keeps_slot_for_reacquired_window() {
        single_window_returns_with_new_token_and_keeps_full_frame(LayoutKind::Bsp);
    }
    #[test]
    fn dwindle_keeps_slot_for_reacquired_window() {
        single_window_returns_with_new_token_and_keeps_full_frame(LayoutKind::Dwindle);
    }
    #[test]
    fn scrolling_keeps_column_for_reacquired_window() {
        single_window_returns_with_new_token_and_keeps_full_frame(LayoutKind::Scrolling);
    }

    #[test]
    fn dwindle_reacquires_before_stale_ax_token_is_removed() {
        let mut c = Coordinator::new(
            TilingLayout::new(LayoutKind::Dwindle, &LayoutSettings::default()),
            DisplayId(1),
            area(),
        );
        c.handle(CoordinatorEvent::Observed(window(27, 500)));
        let full = c.layout.calculate_layout(area())[0].1;
        // A quick Space round trip can restore the saved layout before a
        // complete scan removes the old AX token from the registry.
        c.replace_layout(c.layout.clone());
        assert_eq!(c.awaiting(), 0);
        c.handle(CoordinatorEvent::Observed(window(29, 500)));
        assert_eq!(c.layout.windows(), vec![id(29)]);
        assert_eq!(c.layout.calculate_layout(area())[0].1, full);
        assert_eq!(c.plan().len(), 1);

        // A genuinely new second window should create the first split.
        c.handle(CoordinatorEvent::Observed(window(30, 600)));
        assert_eq!(c.layout.windows(), vec![id(29), id(30)]);
        assert_eq!(c.layout.splits().len(), 1);
    }

    #[test]
    fn dwindle_reconcile_rekeys_leaf_without_changing_order_or_ratio() {
        let mut c = Coordinator::new(
            TilingLayout::new(LayoutKind::Dwindle, &LayoutSettings::default()),
            DisplayId(1),
            area(),
        );
        c.handle(CoordinatorEvent::Observed(window(1, 501)));
        c.handle(CoordinatorEvent::Observed(window(2, 502)));
        let split = c.layout.splits()[0].0;
        c.layout.resize_split(split, 0.7).unwrap();
        c.handle(CoordinatorEvent::Reconciled {
            process: id(1).process,
            windows: vec![window(11, 501), window(2, 502)],
        });
        assert_eq!(c.layout.windows(), vec![id(11), id(2)]);
        assert_eq!(c.layout.splits().len(), 1);
        assert!((c.layout.splits()[0].2 - 0.7).abs() < 1e-9);
    }

    #[test]
    fn reacquired_window_keeps_its_position_among_others() {
        let mut c = Coordinator::new(
            TilingLayout::new(LayoutKind::Bsp, &LayoutSettings::default()),
            DisplayId(1),
            area(),
        );
        for (token, cg) in [(1, 501), (2, 502), (3, 503)] {
            c.handle(CoordinatorEvent::Observed(window(token, cg)));
        }
        let node = c.layout.splits()[0].0;
        c.layout.resize_split(node, 0.7).unwrap();
        let saved = c.layout.clone();
        for token in 1..=3 {
            c.handle(CoordinatorEvent::Destroyed(id(token)));
        }
        c.replace_layout(saved);
        for (token, cg) in [(13, 503), (11, 501), (12, 502)] {
            c.handle(CoordinatorEvent::Observed(window(token, cg)));
        }
        assert_eq!(c.layout.windows(), vec![id(11), id(12), id(13)]);
        assert!((c.layout.splits()[0].2 - 0.7).abs() < 1e-9, "ratio kept");
        assert_eq!(c.awaiting(), 0);
    }
}
