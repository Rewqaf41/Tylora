use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};
use tylora_config::{Focus, FocusMode};
use tylora_core::registry::ManagedWindowId;

#[derive(Clone, Copy)]
pub struct Hit {
    pub sequence: u64,
    pub moved_at: Instant,
    pub window: Option<(u32, i32)>,
    pub hit_us: u128,
}

pub struct FocusFollowsMouseManager {
    requests: SyncSender<(u64, (f64, f64), Instant)>,
    results: Receiver<Hit>,
    pub sequence: u64,
    pub point: Option<(f64, f64)>,
    pub pending: Option<(ManagedWindowId, u32, Instant, Instant)>,
    pub requested: Option<(ManagedWindowId, Instant)>,
    pub applied_for: Option<Instant>,
    pub cancelled: u64,
    pub failures: u64,
    pub hit_count: u64,
    pub hit_total_us: u128,
    pub requests_sent: u64,
    pub confirmations: u64,
    pub applied: u64,
    pub request_to_applied_total_us: u128,
    pub request_to_confirm_total_us: u128,
    pub mouse_to_request_total_us: u128,
    last_sample: Option<Instant>,
    dirty_since: Option<Instant>,
}

impl FocusFollowsMouseManager {
    pub fn start() -> Self {
        Self::start_with(tylora_macos::topmost_window_at)
    }

    fn start_with<F>(hit_test: F) -> Self
    where
        F: Fn((f64, f64)) -> Option<(u32, i32)> + Send + 'static,
    {
        let (requests, request_rx) = mpsc::sync_channel(1);
        let (result_tx, results) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("tylora-pointer-hit".into())
            .spawn(move || {
                while let Ok((sequence, point, moved_at)) = request_rx.recv() {
                    let start = Instant::now();
                    let window = hit_test(point);
                    let hit = Hit {
                        sequence,
                        moved_at,
                        window,
                        hit_us: start.elapsed().as_micros(),
                    };
                    let _ = result_tx.try_send(hit);
                }
            })
            .expect("pointer hit worker");
        Self {
            requests,
            results,
            sequence: 0,
            point: None,
            pending: None,
            requested: None,
            applied_for: None,
            cancelled: 0,
            failures: 0,
            hit_count: 0,
            hit_total_us: 0,
            requests_sent: 0,
            confirmations: 0,
            applied: 0,
            request_to_applied_total_us: 0,
            request_to_confirm_total_us: 0,
            mouse_to_request_total_us: 0,
            last_sample: None,
            dirty_since: None,
        }
    }

    pub fn cancel(&mut self) {
        if self.pending.take().is_some() {
            self.cancelled += 1;
        }
        self.requested = None;
        self.applied_for = None;
        self.point = None;
        self.sequence = self.sequence.wrapping_add(1);
        self.dirty_since = None;
    }

    pub fn moved(&mut self, point: (f64, f64), at: Instant, config: &Focus) {
        self.point = Some(point);
        if !config.follows_mouse || config.mode == FocusMode::Disabled {
            self.cancel();
            return;
        }
        self.sequence = self.sequence.wrapping_add(1);
        self.dirty_since = Some(at);
    }

    pub fn poll(&mut self, config: &Focus) {
        let Some(at) = self.dirty_since else {
            return;
        };
        let sample_interval = if config.mode == FocusMode::Instant {
            0
        } else {
            30
        };
        if self
            .last_sample
            .is_some_and(|last| last.elapsed() < Duration::from_millis(sample_interval))
        {
            return;
        }
        let Some(point) = self.point else {
            return;
        };
        if self.requests.try_send((self.sequence, point, at)).is_ok() {
            self.last_sample = Some(Instant::now());
            self.dirty_since = None;
        }
    }

    pub fn take_hit(&mut self) -> Option<Hit> {
        let hit = self.results.try_recv().ok()?;
        self.point?;
        self.hit_count += 1;
        self.hit_total_us += hit.hit_us;
        if hit.sequence != self.sequence && self.point.is_some() {
            self.dirty_since = Some(Instant::now());
        }
        Some(hit)
    }

    pub fn candidate(
        &mut self,
        id: ManagedWindowId,
        cg_id: u32,
        moved_at: Instant,
        config: &Focus,
    ) {
        if self.requested.is_some_and(|(requested, _)| requested == id) {
            return;
        }
        if self.pending.is_some_and(|(pending, _, _, _)| pending == id) {
            return;
        }
        if self.pending.take().is_some() {
            self.cancelled += 1;
        }
        let delay = if config.mode == FocusMode::Instant {
            0
        } else {
            config.delay_ms
        };
        self.pending = Some((
            id,
            cg_id,
            moved_at + Duration::from_millis(u64::from(delay)),
            moved_at,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tylora_core::registry::ProcessInstance;

    fn id(token: u64) -> ManagedWindowId {
        ManagedWindowId {
            process: ProcessInstance { pid: 42, epoch: 1 },
            token,
        }
    }

    #[test]
    fn disabled_does_not_schedule_hit_test() {
        let mut manager = FocusFollowsMouseManager::start_with(|_| panic!("disabled hit test"));
        manager.moved((1.0, 1.0), Instant::now(), &Focus::default());
        manager.poll(&Focus::default());
        assert!(manager.pending.is_none());
        assert!(manager.take_hit().is_none());
    }

    #[test]
    fn instant_and_delayed_candidates_are_distinct() {
        let mut manager = FocusFollowsMouseManager::start_with(|_| None);
        let mut config = Focus {
            follows_mouse: true,
            mode: FocusMode::Instant,
            ..Default::default()
        };
        let now = Instant::now();
        manager.candidate(id(1), 1, now, &config);
        assert_eq!(manager.pending.unwrap().2, now);
        manager.cancel();
        config.mode = FocusMode::Delayed;
        manager.candidate(id(1), 1, now, &config);
        assert_eq!(manager.pending.unwrap().2, now + Duration::from_millis(120));
    }

    #[test]
    fn rapid_transitions_keep_only_latest_candidate() {
        let mut manager = FocusFollowsMouseManager::start_with(|_| None);
        let config = Focus::default();
        let now = Instant::now();
        for token in 1..=3 {
            manager.candidate(id(token), token as u32, now, &config);
        }
        assert_eq!(manager.pending.unwrap().0, id(3));
        assert_eq!(manager.cancelled, 2);
        manager.candidate(id(3), 3, now, &config);
        assert_eq!(manager.cancelled, 2);
        manager.cancel();
        assert!(manager.pending.is_none());
    }

    #[test]
    fn hit_tester_receives_pointer_without_ax_work_in_callback() {
        let mut manager =
            FocusFollowsMouseManager::start_with(|point| (point == (5.0, 7.0)).then_some((99, 42)));
        let config = Focus {
            follows_mouse: true,
            ..Default::default()
        };
        manager.moved((5.0, 7.0), Instant::now(), &config);
        manager.poll(&config);
        let hit = manager
            .results
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(hit.window, Some((99, 42)));
    }

    #[test]
    fn instant_keeps_hit_result_while_pointer_is_moving() {
        let mut manager =
            FocusFollowsMouseManager::start_with(|point| (point == (1.0, 1.0)).then_some((99, 42)));
        let config = Focus {
            follows_mouse: true,
            mode: FocusMode::Instant,
            ..Default::default()
        };
        manager.moved((1.0, 1.0), Instant::now(), &config);
        manager.poll(&config);
        manager.moved((2.0, 2.0), Instant::now(), &config);
        let deadline = Instant::now() + Duration::from_secs(1);
        let hit = loop {
            if let Some(hit) = manager.take_hit() {
                break hit;
            }
            assert!(Instant::now() < deadline);
            thread::yield_now();
        };
        assert_eq!(hit.window, Some((99, 42)));
        assert!(hit.sequence < manager.sequence);
    }
}
