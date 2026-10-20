//! Animation Engine: timing, easing and scheduling of window geometry
//! transitions.
//!
//! The layout engines compute where windows go (target geometry); this module
//! only decides whether and how a window travels from its current geometry to
//! that target. It never touches layout models: animated geometry exists only
//! here and in the frames handed to the geometry writer.
//!
//! Everything is pure and takes `now`, so tests drive it with a synthetic
//! clock. The AX worker owns the only scheduler and performs every write.
//!
//! Three kinds of geometry are kept apart:
//! - observed: what AX last reported (registry, not here),
//! - target: what the layout engine wants (`Transition::to`),
//! - animated: the intermediate frame last written (`Transition::current`).

use crate::{registry::ManagedWindowId, Rect};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Longest configurable transition; longer feels like lag, not motion.
pub const MAX_DURATION_MS: u32 = 500;
pub const MIN_FPS: u32 = 15;
pub const MAX_FPS: u32 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    Linear,
    #[default]
    EaseOutCubic,
    EaseInOutCubic,
    EaseOutQuart,
    EaseOutExpo,
}

impl Easing {
    pub const ALL: [Self; 5] = [
        Self::Linear,
        Self::EaseOutCubic,
        Self::EaseInOutCubic,
        Self::EaseOutQuart,
        Self::EaseOutExpo,
    ];

    /// The configuration name (as serialized).
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::EaseOutCubic => "ease_out_cubic",
            Self::EaseInOutCubic => "ease_in_out_cubic",
            Self::EaseOutQuart => "ease_out_quart",
            Self::EaseOutExpo => "ease_out_expo",
        }
    }

    /// Maps linear progress `t` in [0, 1] to eased progress in [0, 1].
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
            Self::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::EaseOutQuart => 1.0 - (1.0 - t).powi(4),
            Self::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - 2f64.powf(-10.0 * t)
                }
            }
        }
    }
}

/// How progress evolves over time. Only eased durations exist today; a
/// spring adds a variant whose `progress` settles instead of ending at a
/// fixed duration, without changing the scheduler.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Eased { easing: Easing, duration: Duration },
}

impl Curve {
    /// Eased progress at `elapsed`, and whether the motion has ended.
    pub fn progress(self, elapsed: Duration) -> (f64, bool) {
        match self {
            Self::Eased { easing, duration } => {
                if duration.is_zero() {
                    return (1.0, true);
                }
                let t = elapsed.as_secs_f64() / duration.as_secs_f64();
                (easing.apply(t), t >= 1.0)
            }
        }
    }
}

/// Linear blend of two frames at eased progress `s`, rounded to whole points:
/// sub-point frames only make apps redo layout for no visible change.
pub fn interpolate(from: Rect, to: Rect, s: f64) -> Rect {
    let s = s.clamp(0.0, 1.0);
    let blend = |a: f64, b: f64| (a + (b - a) * s).round();
    Rect {
        x: blend(from.x, to.x),
        y: blend(from.y, to.y),
        width: blend(from.width, to.width).max(1.0),
        height: blend(from.height, to.height).max(1.0),
    }
}

/// What caused a layout change; each can be switched off on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationEvent {
    WindowOpen,
    WindowClose,
    LayoutChange,
    LayoutZoom,
    Scrolling,
    Scratchpad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationEvents {
    pub window_open: bool,
    pub window_close: bool,
    pub layout_change: bool,
    pub layout_zoom: bool,
    pub scrolling: bool,
    pub scratchpad: bool,
}

impl Default for AnimationEvents {
    fn default() -> Self {
        Self {
            window_open: true,
            window_close: true,
            layout_change: true,
            layout_zoom: true,
            scrolling: true,
            scratchpad: true,
        }
    }
}

impl AnimationEvents {
    pub fn enabled(&self, event: AnimationEvent) -> bool {
        match event {
            AnimationEvent::WindowOpen => self.window_open,
            AnimationEvent::WindowClose => self.window_close,
            AnimationEvent::LayoutChange => self.layout_change,
            AnimationEvent::LayoutZoom => self.layout_zoom,
            AnimationEvent::Scrolling => self.scrolling,
            AnimationEvent::Scratchpad => self.scratchpad,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    /// The best verified backend available: Accessibility, else Instant.
    #[default]
    Auto,
    Instant,
    Accessibility,
    /// WindowServer-side geometry of other apps' windows. Not available
    /// without injecting into the Dock; kept so the choice is explicit.
    NativeExperimental,
}

impl BackendChoice {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Instant => "instant",
            Self::Accessibility => "accessibility",
            Self::NativeExperimental => "native_experimental",
        }
    }
}

/// Direction a scratchpad comes from when shown (and leaves to when hidden).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlideDirection {
    #[default]
    Down,
    Up,
    Left,
    Right,
}

impl SlideDirection {
    pub fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// Where a window at `frame` starts (or ends) its slide: offset by a
    /// fraction of its size, so the motion is noticeable but short.
    pub fn offstage(self, frame: Rect) -> Rect {
        let dy = (frame.height * 0.25).round();
        let dx = (frame.width * 0.25).round();
        match self {
            Self::Down => Rect {
                y: frame.y - dy,
                ..frame
            },
            Self::Up => Rect {
                y: frame.y + dy,
                ..frame
            },
            Self::Left => Rect {
                x: frame.x + dx,
                ..frame
            },
            Self::Right => Rect {
                x: frame.x - dx,
                ..frame
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationConfig {
    pub enabled: bool,
    pub backend: BackendChoice,
    pub duration_ms: u32,
    pub easing: Easing,
    pub preferred_fps: u32,
    pub respect_reduce_motion: bool,
    pub scratchpad_slide: SlideDirection,
    pub events: AnimationEvents,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: BackendChoice::Auto,
            duration_ms: 180,
            easing: Easing::EaseOutCubic,
            preferred_fps: 60,
            respect_reduce_motion: true,
            scratchpad_slide: SlideDirection::Down,
            events: AnimationEvents::default(),
        }
    }
}

impl AnimationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.duration_ms > MAX_DURATION_MS {
            return Err(format!(
                "animations.duration_ms must be at most {MAX_DURATION_MS}"
            ));
        }
        if !(MIN_FPS..=MAX_FPS).contains(&self.preferred_fps) {
            return Err(format!(
                "animations.preferred_fps must be between {MIN_FPS} and {MAX_FPS}"
            ));
        }
        Ok(())
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(u64::from(self.duration_ms))
    }

    pub fn frame_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.preferred_fps.clamp(MIN_FPS, MAX_FPS)))
    }
}

/// The backend that actually moves windows, after availability checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedBackend {
    Instant,
    Accessibility,
}

/// Why a geometry change was applied at once instead of animated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstantReason {
    Disabled,
    ReduceMotion,
    InstantBackend,
    NoEvent,
    EventDisabled,
    ZeroDuration,
    NoMotion,
    SlowApplication,
    Offscreen,
    CrossDisplay,
    InactiveSpace,
    Dragging,
}

/// Facts about one window the policy cannot know by itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WindowConstraints {
    /// The app answered frame writes too slowly before.
    pub slow_application: bool,
    /// Start or target is mostly outside every display (Scrolling parking).
    pub offscreen: bool,
    /// Start and target are on different displays.
    pub cross_display: bool,
    /// The window is not on a Space some display shows.
    pub inactive_space: bool,
    /// The user drags or resizes it with the mouse.
    pub dragging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Instant(InstantReason),
    Animate(Curve),
}

/// Decides, per geometry change, between an animation and an instant write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimationPolicy {
    pub config: AnimationConfig,
    /// System "Reduce motion" accessibility setting.
    pub reduce_motion: bool,
    /// Accessibility geometry writes are possible (process is trusted).
    pub accessibility: bool,
}

impl Default for AnimationPolicy {
    fn default() -> Self {
        Self {
            config: AnimationConfig {
                enabled: false,
                ..AnimationConfig::default()
            },
            reduce_motion: false,
            accessibility: false,
        }
    }
}

impl AnimationPolicy {
    pub fn backend(&self) -> ResolvedBackend {
        match self.config.backend {
            BackendChoice::Auto | BackendChoice::Accessibility if self.accessibility => {
                ResolvedBackend::Accessibility
            }
            // NativeExperimental is unsupported; it degrades to Instant.
            _ => ResolvedBackend::Instant,
        }
    }

    /// Instant unless every condition for a visible, safe transition holds.
    pub fn decide(
        &self,
        event: Option<AnimationEvent>,
        from: Rect,
        to: Rect,
        window: WindowConstraints,
    ) -> Decision {
        use InstantReason::*;
        let config = &self.config;
        let reason = if !config.enabled {
            Some(Disabled)
        } else if self.reduce_motion && config.respect_reduce_motion {
            Some(ReduceMotion)
        } else if self.backend() == ResolvedBackend::Instant {
            Some(InstantBackend)
        } else if config.duration_ms == 0 {
            Some(ZeroDuration)
        } else if window.dragging {
            Some(Dragging)
        } else if from.near(to, 1.0) {
            Some(NoMotion)
        } else {
            match event {
                None => Some(NoEvent),
                Some(event) if !config.events.enabled(event) => Some(EventDisabled),
                Some(_) if window.inactive_space => Some(InactiveSpace),
                Some(_) if window.cross_display => Some(CrossDisplay),
                Some(_) if window.offscreen => Some(Offscreen),
                Some(_) if window.slow_application => Some(SlowApplication),
                Some(_) => None,
            }
        };
        match reason {
            Some(reason) => Decision::Instant(reason),
            None => Decision::Animate(Curve::Eased {
                easing: config.easing,
                duration: config.duration(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnimationStatus {
    Running,
    Completed,
    Cancelled,
    Failed,
}

/// One window's transition. `completion` is what the owner must report when
/// it ends (for the AX worker: the layout command it fulfils).
#[derive(Debug, Clone, PartialEq)]
pub struct Transition<C> {
    pub id: ManagedWindowId,
    /// Registry generation and coordinator state version of the target.
    pub generation: u64,
    pub version: u64,
    pub from: Rect,
    pub to: Rect,
    /// Animated geometry: the frame last handed out for writing.
    pub current: Rect,
    pub started: Instant,
    pub curve: Curve,
    pub status: AnimationStatus,
    pub completion: C,
}

/// Request to move `id` to `to`, starting from `from` unless the window is
/// already animating (then from where it is now).
#[derive(Debug, Clone, PartialEq)]
pub struct StartRequest<C> {
    pub id: ManagedWindowId,
    pub generation: u64,
    pub version: u64,
    pub from: Rect,
    pub to: Rect,
    pub curve: Curve,
    pub completion: C,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Started<C> {
    New,
    /// A running transition was retargeted; its completion is superseded by
    /// the new one and must not be reported as a success.
    Retargeted {
        superseded: C,
    },
}

/// A frame to write now. `Final` ends the transition; its frame is the
/// target and must be written and verified like an instant apply.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame<C> {
    Step {
        id: ManagedWindowId,
        frame: Rect,
        previous: Rect,
    },
    Final(Transition<C>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct AnimationMetrics {
    pub started: u64,
    pub completed: u64,
    pub retargeted: u64,
    pub cancelled: u64,
    pub failed: u64,
    pub instant: u64,
    pub steps_written: u64,
    /// Frame slots that passed without a write because a tick ran late; the
    /// next frame is computed from elapsed time, so motion stays on time.
    pub skipped_slots: u64,
    pub ticks: u64,
    pub tick_interval_total_us: u64,
    pub tick_interval_max_us: u64,
    /// Time spent computing frames (not writing them).
    pub schedule_total_us: u64,
    pub schedule_max_us: u64,
    /// Geometry write latency, measured by the writer.
    pub write_total_us: u64,
    pub write_max_us: u64,
    pub writes: u64,
    /// Distance between target and the frame read back after the last write.
    pub final_error_total: f64,
    pub final_error_max: f64,
    pub finals_measured: u64,
    pub active: u64,
}

impl AnimationMetrics {
    pub fn record_write(&mut self, latency: Duration) {
        let us = latency.as_micros().min(u128::from(u64::MAX)) as u64;
        self.writes += 1;
        self.write_total_us = self.write_total_us.saturating_add(us);
        self.write_max_us = self.write_max_us.max(us);
    }

    pub fn record_final_error(&mut self, target: Rect, actual: Rect) {
        let error = (actual.x - target.x).abs()
            + (actual.y - target.y).abs()
            + (actual.width - target.width).abs()
            + (actual.height - target.height).abs();
        self.finals_measured += 1;
        self.final_error_total += error;
        self.final_error_max = self.final_error_max.max(error);
    }
}

/// All running transitions, ticked at most every `interval`. With nothing
/// running it has no deadline, so its owner can sleep.
#[derive(Debug)]
pub struct AnimationScheduler<C> {
    active: HashMap<ManagedWindowId, Transition<C>>,
    interval: Duration,
    next_tick: Option<Instant>,
    last_tick: Option<Instant>,
    metrics: AnimationMetrics,
}

impl<C> AnimationScheduler<C> {
    pub fn new(interval: Duration) -> Self {
        Self {
            active: HashMap::new(),
            interval: interval.max(Duration::from_millis(1)),
            next_tick: None,
            last_tick: None,
            metrics: AnimationMetrics::default(),
        }
    }

    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval.max(Duration::from_millis(1));
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn is_idle(&self) -> bool {
        self.active.is_empty()
    }

    pub fn contains(&self, id: ManagedWindowId) -> bool {
        self.active.contains_key(&id)
    }

    pub fn get(&self, id: ManagedWindowId) -> Option<&Transition<C>> {
        self.active.get(&id)
    }

    pub fn ids(&self) -> impl Iterator<Item = ManagedWindowId> + '_ {
        self.active.keys().copied()
    }

    /// When the owner must call `frames` next; `None` while idle.
    pub fn next_deadline(&self) -> Option<Instant> {
        (!self.active.is_empty())
            .then_some(self.next_tick)
            .flatten()
    }

    pub fn metrics(&self) -> AnimationMetrics {
        AnimationMetrics {
            active: self.active.len() as u64,
            ..self.metrics
        }
    }

    pub fn metrics_mut(&mut self) -> &mut AnimationMetrics {
        &mut self.metrics
    }

    /// Starts a transition, or retargets the running one of the same window
    /// from its current animated frame, keeping motion continuous.
    pub fn start(&mut self, request: StartRequest<C>, now: Instant) -> Started<C> {
        let previous = self.active.remove(&request.id);
        let from = previous
            .as_ref()
            .map_or(request.from, |running| running.current);
        self.active.insert(
            request.id,
            Transition {
                id: request.id,
                generation: request.generation,
                version: request.version,
                from,
                to: request.to,
                current: from,
                started: now,
                curve: request.curve,
                status: AnimationStatus::Running,
                completion: request.completion,
            },
        );
        // The first step is due at once: a new target must not wait a frame.
        self.next_tick = Some(self.next_tick.map_or(now, |next| next.min(now)));
        match previous {
            Some(previous) => {
                self.metrics.retargeted += 1;
                Started::Retargeted {
                    superseded: previous.completion,
                }
            }
            None => {
                self.metrics.started += 1;
                Started::New
            }
        }
    }

    /// Stops a window's transition where it is. The owner decides whether
    /// to write the target or leave the window to the user (drag).
    pub fn cancel(&mut self, id: ManagedWindowId) -> Option<Transition<C>> {
        let mut transition = self.active.remove(&id)?;
        transition.status = AnimationStatus::Cancelled;
        self.metrics.cancelled += 1;
        self.settle();
        Some(transition)
    }

    /// Ends a transition whose write failed (window gone, app refused).
    pub fn fail(&mut self, id: ManagedWindowId) -> Option<Transition<C>> {
        let mut transition = self.active.remove(&id)?;
        transition.status = AnimationStatus::Failed;
        self.metrics.failed += 1;
        self.settle();
        Some(transition)
    }

    /// Stops everything, e.g. on a Space switch, display change or stop.
    pub fn cancel_all(&mut self) -> Vec<Transition<C>> {
        let mut all: Vec<_> = self
            .active
            .drain()
            .map(|(_, transition)| transition)
            .collect();
        for transition in &mut all {
            transition.status = AnimationStatus::Cancelled;
        }
        self.metrics.cancelled += all.len() as u64;
        self.settle();
        all
    }

    /// Drops transitions of windows that no longer exist; nothing is written
    /// for them and their completions are returned for reporting.
    pub fn retain(&mut self, mut keep: impl FnMut(ManagedWindowId) -> bool) -> Vec<Transition<C>> {
        let gone: Vec<_> = self
            .active
            .keys()
            .copied()
            .filter(|id| !keep(*id))
            .collect();
        let mut removed = Vec::new();
        for id in gone {
            if let Some(transition) = self.fail(id) {
                removed.push(transition);
            }
        }
        removed
    }

    /// The frames due at `now`, at most one per window: the newest position
    /// along each trajectory, from elapsed time rather than frame count.
    /// Finished transitions come back as `Frame::Final` and leave the
    /// scheduler. Returns nothing before the next deadline.
    pub fn frames(&mut self, now: Instant) -> Vec<Frame<C>> {
        let Some(due) = self.next_deadline() else {
            return Vec::new();
        };
        if now < due {
            return Vec::new();
        }
        let started = Instant::now();
        if let Some(last) = self.last_tick {
            let gap = now.saturating_duration_since(last);
            let us = gap.as_micros().min(u128::from(u64::MAX)) as u64;
            self.metrics.tick_interval_total_us += us;
            self.metrics.tick_interval_max_us = self.metrics.tick_interval_max_us.max(us);
            let slots = gap.as_nanos() / self.interval.as_nanos().max(1);
            self.metrics.skipped_slots += (slots.saturating_sub(1)) as u64;
        }
        self.metrics.ticks += 1;
        self.last_tick = Some(now);
        let mut frames = Vec::new();
        let mut finished = Vec::new();
        for transition in self.active.values_mut() {
            let (progress, done) = transition
                .curve
                .progress(now.saturating_duration_since(transition.started));
            if done {
                finished.push(transition.id);
                continue;
            }
            let frame = interpolate(transition.from, transition.to, progress);
            if frame != transition.current {
                frames.push(Frame::Step {
                    id: transition.id,
                    frame,
                    previous: transition.current,
                });
                transition.current = frame;
                self.metrics.steps_written += 1;
            }
        }
        for id in finished {
            if let Some(mut transition) = self.active.remove(&id) {
                transition.status = AnimationStatus::Completed;
                transition.current = transition.to;
                self.metrics.completed += 1;
                frames.push(Frame::Final(transition));
            }
        }
        self.next_tick = (!self.active.is_empty()).then_some(now + self.interval);
        if self.active.is_empty() {
            self.last_tick = None;
        }
        let us = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.metrics.schedule_total_us += us;
        self.metrics.schedule_max_us = self.metrics.schedule_max_us.max(us);
        frames
    }

    fn settle(&mut self) {
        if self.active.is_empty() {
            self.next_tick = None;
            self.last_tick = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProcessInstance;

    fn id(token: u64) -> ManagedWindowId {
        ManagedWindowId {
            process: ProcessInstance { pid: 1, epoch: 1 },
            token,
        }
    }
    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect::new(x, y, width, height).unwrap()
    }
    fn curve(ms: u64) -> Curve {
        Curve::Eased {
            easing: Easing::Linear,
            duration: Duration::from_millis(ms),
        }
    }
    fn request(token: u64, from: Rect, to: Rect, ms: u64) -> StartRequest<u64> {
        StartRequest {
            id: id(token),
            generation: 1,
            version: token,
            from,
            to,
            curve: curve(ms),
            completion: token,
        }
    }
    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn easing_curves_start_at_zero_end_at_one_and_never_overshoot() {
        for easing in Easing::ALL {
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?}");
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-12, "{easing:?}");
            let mut last = 0.0;
            for step in 1..=100 {
                let value = easing.apply(f64::from(step) / 100.0);
                assert!(value >= last - 1e-12 && value <= 1.0 + 1e-12, "{easing:?}");
                last = value;
            }
            // Out of range input is clamped.
            assert_eq!(easing.apply(-1.0), 0.0);
            assert!((easing.apply(2.0) - 1.0).abs() < 1e-12);
        }
        // Ease-out curves are ahead of linear early on, ease-in-out behind.
        assert!(Easing::EaseOutCubic.apply(0.25) > 0.25);
        assert!(Easing::EaseOutExpo.apply(0.25) > Easing::EaseOutCubic.apply(0.25));
        assert!(Easing::EaseInOutCubic.apply(0.25) < 0.25);
        assert!((Easing::EaseInOutCubic.apply(0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn interpolation_blends_every_edge_and_rounds_to_points() {
        let from = rect(0.0, 0.0, 100.0, 100.0);
        let to = rect(100.0, -50.0, 300.0, 200.0);
        assert_eq!(interpolate(from, to, 0.0), from);
        assert_eq!(interpolate(from, to, 1.0), to);
        assert_eq!(interpolate(from, to, 0.5), rect(50.0, -25.0, 200.0, 150.0));
        // Rounded: no sub-point frames.
        assert_eq!(interpolate(from, to, 0.333).x, 33.0);
    }

    #[test]
    fn progress_follows_elapsed_time_not_frame_count() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(1000.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        assert!(matches!(
            scheduler.frames(base).as_slice(),
            [] | [Frame::Step { .. }]
        ));
        // One tick 70 ms late lands where 70 ms of motion belongs.
        let frames = scheduler.frames(at(base, 70));
        let Frame::Step { frame, .. } = &frames[0] else {
            panic!("expected a step");
        };
        assert_eq!(frame.x, 700.0);
        assert!(scheduler.metrics().skipped_slots >= 3);
        // Past the duration: one final frame at the target, then idle.
        let frames = scheduler.frames(at(base, 150));
        let [Frame::Final(done)] = frames.as_slice() else {
            panic!("expected the final frame");
        };
        assert_eq!(done.current, done.to);
        assert_eq!(done.status, AnimationStatus::Completed);
        assert!(scheduler.is_idle());
        assert_eq!(scheduler.next_deadline(), None);
    }

    #[test]
    fn idle_scheduler_has_no_deadline_and_no_frames() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::<u64>::new(Duration::from_millis(16));
        assert_eq!(scheduler.next_deadline(), None);
        assert!(scheduler.frames(at(base, 1000)).is_empty());
        assert_eq!(scheduler.metrics().ticks, 0);
    }

    #[test]
    fn frames_are_not_handed_out_before_the_next_slot() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                200,
            ),
            base,
        );
        let _ = scheduler.frames(at(base, 1));
        assert!(
            scheduler.frames(at(base, 10)).is_empty(),
            "coalesced until the next slot"
        );
        assert_eq!(scheduler.next_deadline(), Some(at(base, 17)));
    }

    #[test]
    fn retarget_continues_from_the_current_animated_frame() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        let _ = scheduler.frames(at(base, 50));
        assert_eq!(scheduler.get(id(1)).unwrap().current.x, 50.0);
        // A new layout arrives mid-flight: no jump back, no waiting.
        let mut retarget = request(
            1,
            rect(0.0, 0.0, 10.0, 10.0),
            rect(0.0, 300.0, 10.0, 10.0),
            100,
        );
        retarget.version = 2;
        retarget.completion = 2;
        let started = scheduler.start(retarget, at(base, 50));
        assert_eq!(started, Started::Retargeted { superseded: 1 });
        let running = scheduler.get(id(1)).unwrap();
        assert_eq!((running.from.x, running.version), (50.0, 2));
        assert_eq!(scheduler.next_deadline(), Some(at(base, 50)), "due at once");
        let frames = scheduler.frames(at(base, 200));
        let [Frame::Final(done)] = frames.as_slice() else {
            panic!("expected the final frame");
        };
        // Only the newest target is reached; the old completion is gone.
        assert_eq!(
            (done.to, done.completion),
            (rect(0.0, 300.0, 10.0, 10.0), 2)
        );
        assert_eq!(scheduler.metrics().retargeted, 1);
    }

    #[test]
    fn concurrent_windows_share_ticks_and_finish_independently() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                50,
            ),
            base,
        );
        scheduler.start(
            request(
                2,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(0.0, 100.0, 10.0, 10.0),
                150,
            ),
            base,
        );
        let frames = scheduler.frames(at(base, 60));
        assert!(frames
            .iter()
            .any(|f| matches!(f, Frame::Final(t) if t.id == id(1))));
        assert!(frames
            .iter()
            .any(|f| matches!(f, Frame::Step { id: i, .. } if *i == id(2))));
        assert!(scheduler.contains(id(2)) && !scheduler.contains(id(1)));
    }

    #[test]
    fn cancel_and_fail_remove_transitions_and_go_idle() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        scheduler.start(
            request(
                2,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        let cancelled = scheduler.cancel(id(1)).unwrap();
        assert_eq!(cancelled.status, AnimationStatus::Cancelled);
        let failed = scheduler.fail(id(2)).unwrap();
        assert_eq!(failed.status, AnimationStatus::Failed);
        assert!(scheduler.is_idle());
        assert_eq!(scheduler.next_deadline(), None);
        assert!(
            scheduler.frames(at(base, 50)).is_empty(),
            "nothing written after cancel"
        );
        assert_eq!(scheduler.cancel(id(1)), None);
    }

    #[test]
    fn closed_windows_leave_no_transition_behind() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        scheduler.start(
            request(
                1,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        scheduler.start(
            request(
                2,
                rect(0.0, 0.0, 10.0, 10.0),
                rect(100.0, 0.0, 10.0, 10.0),
                100,
            ),
            base,
        );
        let gone = scheduler.retain(|window| window != id(1));
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].id, id(1));
        let frames = scheduler.frames(at(base, 50));
        assert!(frames
            .iter()
            .all(|f| !matches!(f, Frame::Step { id: i, .. } if *i == id(1))));
    }

    #[test]
    fn cancel_all_returns_every_transition_for_a_safe_finish() {
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        for token in 1..=3 {
            scheduler.start(
                request(
                    token,
                    rect(0.0, 0.0, 10.0, 10.0),
                    rect(100.0, 0.0, 10.0, 10.0),
                    100,
                ),
                base,
            );
        }
        let all = scheduler.cancel_all();
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|t| t.to == rect(100.0, 0.0, 10.0, 10.0)));
        assert!(scheduler.is_idle());
    }

    fn policy() -> AnimationPolicy {
        AnimationPolicy {
            config: AnimationConfig::default(),
            reduce_motion: false,
            accessibility: true,
        }
    }

    #[test]
    fn policy_animates_only_when_every_condition_holds() {
        let from = rect(0.0, 0.0, 100.0, 100.0);
        let to = rect(200.0, 0.0, 100.0, 100.0);
        let window = WindowConstraints::default();
        let animate =
            |policy: &AnimationPolicy, event, window| policy.decide(event, from, to, window);
        assert!(matches!(
            animate(&policy(), Some(AnimationEvent::LayoutChange), window),
            Decision::Animate(Curve::Eased { duration, .. }) if duration == Duration::from_millis(180)
        ));
        use InstantReason::*;
        let instant = |decision| match decision {
            Decision::Instant(reason) => reason,
            Decision::Animate(_) => panic!("expected instant"),
        };
        let mut off = policy();
        off.config.enabled = false;
        assert_eq!(
            instant(animate(&off, Some(AnimationEvent::LayoutChange), window)),
            Disabled
        );
        let mut calm = policy();
        calm.reduce_motion = true;
        assert_eq!(
            instant(animate(&calm, Some(AnimationEvent::LayoutChange), window)),
            ReduceMotion
        );
        calm.config.respect_reduce_motion = false;
        assert!(matches!(
            animate(&calm, Some(AnimationEvent::LayoutChange), window),
            Decision::Animate(_)
        ));
        // No cause known (reconciliation, Space switch): instant.
        assert_eq!(instant(animate(&policy(), None, window)), NoEvent);
        let mut scrolling_off = policy();
        scrolling_off.config.events.scrolling = false;
        assert_eq!(
            instant(animate(
                &scrolling_off,
                Some(AnimationEvent::Scrolling),
                window
            )),
            EventDisabled
        );
        for (constraints, reason) in [
            (
                WindowConstraints {
                    dragging: true,
                    ..window
                },
                Dragging,
            ),
            (
                WindowConstraints {
                    offscreen: true,
                    ..window
                },
                Offscreen,
            ),
            (
                WindowConstraints {
                    cross_display: true,
                    ..window
                },
                CrossDisplay,
            ),
            (
                WindowConstraints {
                    inactive_space: true,
                    ..window
                },
                InactiveSpace,
            ),
            (
                WindowConstraints {
                    slow_application: true,
                    ..window
                },
                SlowApplication,
            ),
        ] {
            assert_eq!(
                instant(animate(
                    &policy(),
                    Some(AnimationEvent::LayoutChange),
                    constraints
                )),
                reason
            );
        }
        assert_eq!(
            instant(policy().decide(Some(AnimationEvent::LayoutChange), from, from, window)),
            NoMotion
        );
        let mut zero = policy();
        zero.config.duration_ms = 0;
        assert_eq!(
            instant(animate(&zero, Some(AnimationEvent::LayoutChange), window)),
            ZeroDuration
        );
    }

    #[test]
    fn backend_auto_picks_accessibility_only_when_trusted() {
        let mut policy = policy();
        assert_eq!(policy.backend(), ResolvedBackend::Accessibility);
        policy.accessibility = false;
        assert_eq!(policy.backend(), ResolvedBackend::Instant);
        assert_eq!(
            policy.decide(
                Some(AnimationEvent::LayoutChange),
                rect(0.0, 0.0, 1.0, 1.0),
                rect(50.0, 0.0, 1.0, 1.0),
                WindowConstraints::default()
            ),
            Decision::Instant(InstantReason::InstantBackend)
        );
        // The unverified native backend never animates: it degrades to Instant.
        let mut native = self::tests::policy();
        native.config.backend = BackendChoice::NativeExperimental;
        assert_eq!(native.backend(), ResolvedBackend::Instant);
        let mut instant = self::tests::policy();
        instant.config.backend = BackendChoice::Instant;
        assert_eq!(instant.backend(), ResolvedBackend::Instant);
    }

    #[test]
    fn config_defaults_validation_and_old_files() {
        let config = AnimationConfig::default();
        assert!(config.enabled);
        assert_eq!((config.duration_ms, config.preferred_fps), (180, 60));
        assert_eq!(config.easing, Easing::EaseOutCubic);
        assert!(config.validate().is_ok());
        assert!(AnimationConfig {
            duration_ms: 501,
            ..config
        }
        .validate()
        .is_err());
        assert!(AnimationConfig {
            preferred_fps: 5,
            ..config
        }
        .validate()
        .is_err());
        assert!(AnimationConfig {
            preferred_fps: 500,
            ..config
        }
        .validate()
        .is_err());
        // Partial tables keep defaults for the rest.
        let partial: AnimationConfig =
            serde_json::from_str(r#"{"duration_ms":90,"events":{"scrolling":false}}"#).unwrap();
        assert_eq!(partial.duration_ms, 90);
        assert!(!partial.events.scrolling && partial.events.window_open);
        assert_eq!(config.frame_interval(), Duration::from_secs_f64(1.0 / 60.0));
    }

    #[test]
    fn scrolling_strip_moves_rigidly_and_keeps_column_widths() {
        // Three columns shift left by the same viewport delta: at every tick
        // they keep their spacing and widths, so the strip never stretches.
        let base = Instant::now();
        let mut scheduler = AnimationScheduler::new(Duration::from_millis(16));
        let columns = [(0.0, 600.0), (610.0, 400.0), (1020.0, 800.0)];
        for (token, (x, width)) in columns.iter().enumerate() {
            scheduler.start(
                StartRequest {
                    curve: Curve::Eased {
                        easing: Easing::EaseOutCubic,
                        duration: Duration::from_millis(180),
                    },
                    ..request(
                        token as u64,
                        rect(*x, 0.0, *width, 900.0),
                        rect(*x - 610.0, 0.0, *width, 900.0),
                        180,
                    )
                },
                base,
            );
        }
        for ms in [16, 50, 90, 140] {
            let frames = scheduler.frames(at(base, ms));
            let mut xs: Vec<(u64, f64, f64)> = frames
                .iter()
                .filter_map(|frame| match frame {
                    Frame::Step { id, frame, .. } => Some((id.token, frame.x, frame.width)),
                    Frame::Final(_) => None,
                })
                .collect();
            xs.sort_by_key(|(token, _, _)| *token);
            assert_eq!(xs.len(), 3, "every column moves in the same tick");
            assert_eq!(xs[1].1 - xs[0].1, 610.0);
            assert_eq!(xs[2].1 - xs[1].1, 410.0);
            assert_eq!([xs[0].2, xs[1].2, xs[2].2], [600.0, 400.0, 800.0]);
        }
        // Fast navigation: a new viewport target mid-flight continues from
        // where each column is, without a jump.
        let current = scheduler.get(id(0)).unwrap().current;
        scheduler.start(
            request(0, current, rect(-1220.0, 0.0, 600.0, 900.0), 180),
            at(base, 140),
        );
        assert_eq!(scheduler.get(id(0)).unwrap().from, current);
    }

    #[test]
    fn single_events_can_be_switched_off() {
        let from = rect(0.0, 0.0, 100.0, 100.0);
        let to = rect(0.0, 0.0, 1000.0, 800.0);
        let mut policy = policy();
        policy.config.events.layout_zoom = false;
        policy.config.events.scratchpad = false;
        for event in [AnimationEvent::LayoutZoom, AnimationEvent::Scratchpad] {
            assert_eq!(
                policy.decide(Some(event), from, to, WindowConstraints::default()),
                Decision::Instant(InstantReason::EventDisabled)
            );
        }
        assert!(matches!(
            policy.decide(
                Some(AnimationEvent::WindowOpen),
                from,
                to,
                WindowConstraints::default()
            ),
            Decision::Animate(_)
        ));
    }

    #[test]
    fn names_match_the_serialized_form() {
        for easing in Easing::ALL {
            assert_eq!(serde_json::to_value(easing).unwrap(), easing.name());
        }
        for backend in [
            BackendChoice::Auto,
            BackendChoice::Instant,
            BackendChoice::Accessibility,
            BackendChoice::NativeExperimental,
        ] {
            assert_eq!(serde_json::to_value(backend).unwrap(), backend.name());
        }
        for slide in [
            SlideDirection::Down,
            SlideDirection::Up,
            SlideDirection::Left,
            SlideDirection::Right,
        ] {
            assert_eq!(serde_json::to_value(slide).unwrap(), slide.name());
        }
    }

    #[test]
    fn scratchpad_slides_start_off_by_a_quarter_of_the_window() {
        let frame = rect(100.0, 100.0, 400.0, 200.0);
        assert_eq!(
            SlideDirection::Down.offstage(frame),
            rect(100.0, 50.0, 400.0, 200.0)
        );
        assert_eq!(
            SlideDirection::Up.offstage(frame),
            rect(100.0, 150.0, 400.0, 200.0)
        );
        assert_eq!(
            SlideDirection::Left.offstage(frame),
            rect(200.0, 100.0, 400.0, 200.0)
        );
        assert_eq!(
            SlideDirection::Right.offstage(frame),
            rect(0.0, 100.0, 400.0, 200.0)
        );
    }
}
