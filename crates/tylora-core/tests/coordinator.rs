use std::time::Instant;
use tylora_core::{
    registry::{ManagedWindowId, ManagementMode, ProcessInstance, WindowLifecycle, WindowSnapshot},
    Coordinator, CoordinatorEvent, DisplayId, LayoutPolicy, Rect,
};

#[derive(Default)]
struct MockLayout {
    ids: Vec<ManagedWindowId>,
}
impl LayoutPolicy for MockLayout {
    fn insert_window(&mut self, id: ManagedWindowId) {
        if !self.ids.contains(&id) {
            self.ids.push(id);
        }
    }
    fn remove_window(&mut self, id: ManagedWindowId) {
        self.ids.retain(|found| *found != id);
    }
    fn calculate_layout(&self, area: Rect) -> Vec<(ManagedWindowId, Rect)> {
        let len = self.ids.len();
        self.ids
            .iter()
            .enumerate()
            .map(|(index, id)| {
                (
                    *id,
                    Rect::new(
                        area.x,
                        area.y + area.height * index as f64 / len as f64,
                        area.width,
                        area.height / len as f64,
                    )
                    .unwrap(),
                )
            })
            .collect()
    }
}
fn process(epoch: u64) -> ProcessInstance {
    ProcessInstance { pid: 42, epoch }
}
fn id(epoch: u64, token: u64) -> ManagedWindowId {
    ManagedWindowId {
        process: process(epoch),
        token,
    }
}
fn snapshot(id: ManagedWindowId) -> WindowSnapshot {
    WindowSnapshot {
        id,
        cg_window_id: None,
        native_space_id: None,
        application_id: Some("test.app".into()),
        title: None,
        frame: Rect::new(10.0, 10.0, 200.0, 100.0).unwrap(),
        display_id: Some(DisplayId(1)),
        lifecycle: WindowLifecycle::Discovered,
        mode: ManagementMode::Tiled,
    }
}
fn coordinator() -> Coordinator<MockLayout> {
    Coordinator::new(
        MockLayout::default(),
        DisplayId(1),
        Rect::new(0.0, 0.0, 800.0, 600.0).unwrap(),
    )
}

#[test]
fn duplicate_registration_and_repeated_destroy() {
    let mut c = coordinator();
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    assert_eq!(c.registry.len(), 1);
    assert_eq!(c.layout.ids.len(), 1);
    c.handle(CoordinatorEvent::Destroyed(id(1, 1)));
    c.handle(CoordinatorEvent::Destroyed(id(1, 1)));
    assert!(c.registry.is_empty());
    assert!(c.layout.ids.is_empty());
}

#[test]
fn rapid_open_close_and_complete_reconciliation() {
    let mut c = coordinator();
    for token in 0..30 {
        c.handle(CoordinatorEvent::Observed(snapshot(id(1, token))));
    }
    for token in (0..30).step_by(2) {
        c.handle(CoordinatorEvent::Destroyed(id(1, token)));
    }
    let remaining: Vec<_> = (1..30)
        .step_by(2)
        .map(|token| snapshot(id(1, token)))
        .collect();
    c.handle(CoordinatorEvent::Reconciled {
        process: process(1),
        windows: remaining,
    });
    assert_eq!(c.registry.len(), 15);
    c.handle(CoordinatorEvent::Reconciled {
        process: process(1),
        windows: Vec::new(),
    });
    assert!(c.registry.is_empty());
    assert!(c.layout.ids.is_empty());
}

#[test]
fn ax_failure_preserves_window_and_failed_apply_does_not_loop() {
    let mut c = coordinator();
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    let command = c.plan().pop().unwrap();
    let last_success = c.registry.get(id(1, 1)).unwrap().last_successful_update;
    c.handle(CoordinatorEvent::ObservationFailed {
        process: process(1),
        code: -25204,
    });
    assert_eq!(c.registry.len(), 1);
    assert_eq!(
        c.registry.get(id(1, 1)).unwrap().last_successful_update,
        last_success
    );
    c.handle(CoordinatorEvent::Applied {
        command,
        result: Err(-25204),
    });
    assert!(c.plan().is_empty());
    c.set_work_area(DisplayId(1), Rect::new(0.0, 0.0, 700.0, 600.0).unwrap());
    assert_eq!(c.plan().len(), 1);
}

#[test]
fn geometry_mismatch_backs_off_then_can_recover() {
    let mut c = coordinator();
    let window = id(1, 1);
    c.handle(CoordinatorEvent::Observed(snapshot(window)));
    let first = c.plan().pop().unwrap();
    c.handle(CoordinatorEvent::Applied {
        command: first,
        result: Ok(snapshot(window).frame),
    });
    assert!(c.plan().is_empty());
    for attempt in 2..=3 {
        c.registry.get_mut(window).unwrap().retry_after = Some(Instant::now());
        c.invalidate_plan();
        let retry = c.plan().pop().unwrap();
        c.handle(CoordinatorEvent::Applied {
            command: retry,
            result: Err(-25204),
        });
        assert_eq!(c.registry.get(window).unwrap().failed_attempts, attempt);
    }
    assert!(c.plan().is_empty());
    c.registry.get_mut(window).unwrap().retry_after = Some(Instant::now());
    c.invalidate_plan();
    let recovery = c.plan().pop().unwrap();
    c.handle(CoordinatorEvent::Applied {
        command: recovery,
        result: Ok(recovery.target),
    });
    assert_eq!(c.registry.get(window).unwrap().failed_attempts, 0);
    assert!(c.plan().is_empty());
    assert_eq!(c.registry.len(), 1);
}

#[test]
fn undersized_window_is_accepted_after_repeated_mismatch() {
    let mut c = coordinator();
    let window = id(1, 1);
    c.handle(CoordinatorEvent::Observed(snapshot(window)));
    let mut command = c.plan().pop().unwrap();
    // The app keeps its window 14 pt shorter than its slot, every time.
    let short = |target: Rect| {
        Rect::new(
            target.x,
            target.y + 22.0,
            target.width,
            target.height - 14.0,
        )
        .unwrap()
    };
    for attempt in 1..=3 {
        c.handle(CoordinatorEvent::Applied {
            command,
            result: Ok(short(command.target)),
        });
        assert_eq!(c.registry.get(window).unwrap().failed_attempts, attempt);
        if attempt < 3 {
            c.registry.get_mut(window).unwrap().retry_after = Some(Instant::now());
            c.invalidate_plan();
            command = c.plan().pop().unwrap();
        }
    }
    let record = c.registry.get(window).unwrap();
    assert_eq!(
        record.accepted,
        Some((command.target, short(command.target)))
    );
    assert!(record.retry_after.is_none());
    c.invalidate_plan();
    assert!(c.plan().is_empty());
    // Moved away (e.g. dragged): the layout target applies again.
    c.registry.get_mut(window).unwrap().observed.frame =
        Rect::new(300.0, 300.0, 200.0, 200.0).unwrap();
    c.invalidate_plan();
    assert_eq!(
        c.plan().pop().map(|retry| retry.target),
        Some(command.target)
    );
}

#[test]
fn successful_retry_clears_geometry_failure() {
    let mut c = coordinator();
    let window = id(1, 1);
    c.handle(CoordinatorEvent::Observed(snapshot(window)));
    let first = c.plan().pop().unwrap();
    c.handle(CoordinatorEvent::Applied {
        command: first,
        result: Err(-25204),
    });
    c.registry.get_mut(window).unwrap().retry_after = Some(Instant::now());
    c.invalidate_plan();
    let retry = c.plan().pop().unwrap();
    c.handle(CoordinatorEvent::Applied {
        command: retry,
        result: Ok(retry.target),
    });
    let record = c.registry.get(window).unwrap();
    assert_eq!(record.failed_target, None);
    assert_eq!(record.failed_attempts, 0);
    assert!(c.plan().is_empty());
}

#[test]
fn stale_result_cannot_overwrite_new_registration() {
    let mut c = coordinator();
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    let old = c.plan().pop().unwrap();
    c.handle(CoordinatorEvent::Destroyed(id(1, 1)));
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    let fresh = c.plan().pop().unwrap();
    assert_ne!(old.generation, fresh.generation);
    c.handle(CoordinatorEvent::Applied {
        command: old,
        result: Ok(old.target),
    });
    assert!(c.registry.get(id(1, 1)).unwrap().applied.is_none());
    c.handle(CoordinatorEvent::Applied {
        command: fresh,
        result: Ok(fresh.target),
    });
    assert!(c.plan().is_empty());
}

#[test]
fn process_reconnect_has_distinct_identity() {
    let mut c = coordinator();
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    c.handle(CoordinatorEvent::ProcessExited(process(1)));
    c.handle(CoordinatorEvent::Observed(snapshot(id(2, 1))));
    assert!(c.registry.get(id(1, 1)).is_none());
    assert!(c.registry.get(id(2, 1)).is_some());
    assert_eq!(c.layout.ids, vec![id(2, 1)]);
}

#[test]
fn minimized_window_leaves_tree_but_remains_registered() {
    let mut c = coordinator();
    let mut w = snapshot(id(1, 1));
    c.handle(CoordinatorEvent::Observed(w.clone()));
    let pending = c.plan().pop().unwrap();
    w.lifecycle = WindowLifecycle::Minimized;
    c.handle(CoordinatorEvent::Observed(w));
    assert_eq!(c.registry.len(), 1);
    assert!(c.layout.ids.is_empty());
    let record = c.registry.get(id(1, 1)).unwrap();
    assert_eq!(record.desired, None);
    assert_eq!(record.pending_apply, None);
    c.handle(CoordinatorEvent::Applied {
        command: pending,
        result: Ok(pending.target),
    });
    assert!(c.registry.get(id(1, 1)).unwrap().applied.is_none());
}

struct MockAdapter {
    fail: bool,
    calls: usize,
}
impl MockAdapter {
    fn apply(&mut self, coordinator: &mut Coordinator<MockLayout>) {
        for command in coordinator.plan() {
            self.calls += 1;
            let result = if self.fail {
                Err(-25204)
            } else {
                Ok(command.target)
            };
            coordinator.handle(CoordinatorEvent::Applied { command, result });
        }
    }
}

#[test]
fn mock_adapter_applies_once_and_does_not_feedback_loop() {
    let mut c = coordinator();
    let mut adapter = MockAdapter {
        fail: false,
        calls: 0,
    };
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    adapter.apply(&mut c);
    adapter.apply(&mut c);
    assert_eq!(adapter.calls, 1);
    assert_eq!(
        c.registry.get(id(1, 1)).unwrap().applied,
        c.registry.get(id(1, 1)).unwrap().desired
    );
}

#[test]
fn external_move_requires_one_new_apply() {
    let mut c = coordinator();
    let mut adapter = MockAdapter {
        fail: false,
        calls: 0,
    };
    c.handle(CoordinatorEvent::Observed(snapshot(id(1, 1))));
    adapter.apply(&mut c);
    let mut moved = snapshot(id(1, 1));
    moved.frame = Rect::new(50.0, 50.0, 200.0, 100.0).unwrap();
    c.handle(CoordinatorEvent::Observed(moved));
    adapter.apply(&mut c);
    adapter.apply(&mut c);
    assert_eq!(adapter.calls, 2);
}
