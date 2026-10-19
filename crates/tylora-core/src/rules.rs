//! Application rules: declarative definitions, deterministic matching and
//! per-window tracking. Pure logic — no platform calls. The daemon turns an
//! [`Evaluation`] into native operations and reports results back through
//! [`RuleTracker`].

use crate::registry::ProcessInstance;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleMatch {
    #[default]
    Exact,
    Contains,
    Regex,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleMode {
    /// Leave the window's management mode alone.
    #[default]
    Unchanged,
    Tiled,
    Floating,
    /// Never tile or move it (dialogs, palettes).
    Unmanaged,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyPolicy {
    /// Windows created while the daemon runs.
    #[default]
    OnCreate,
    /// Also windows that already existed when the daemon started.
    OnFirstSeen,
    /// Like `on_first_seen`, and again when the window title changes.
    OnTitleChange,
    /// Only through an explicit `rules apply`.
    Manual,
}

impl ApplyPolicy {
    pub fn allows(self, trigger: Trigger) -> bool {
        match trigger {
            Trigger::Manual => true,
            Trigger::Created => self != ApplyPolicy::Manual,
            Trigger::FirstSeen => {
                matches!(self, ApplyPolicy::OnFirstSeen | ApplyPolicy::OnTitleChange)
            }
            Trigger::TitleChanged => self == ApplyPolicy::OnTitleChange,
        }
    }
}

/// One `[[rules]]` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleDefinition {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    pub bundle_id: String,
    /// Optional title condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub title_match: TitleMatch,
    /// Logical workspace by name or numeric ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Display ID as listed by `tylora display list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<u32>,
    #[serde(default)]
    pub mode: RuleMode,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub apply: ApplyPolicy,
    /// Stop looking at lower-priority rules once this one matched.
    #[serde(default = "enabled")]
    pub stop: bool,
    /// Assign the window to this scratchpad (first matching window only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratchpad: Option<String>,
}

fn enabled() -> bool {
    true
}

impl RuleDefinition {
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// Regex patterns are bounded so a rule cannot exhaust memory.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

pub fn compile_title_regex(pattern: &str) -> Result<regex::Regex, String> {
    regex::RegexBuilder::new(pattern)
        .size_limit(REGEX_SIZE_LIMIT)
        .dfa_size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|error| format!("invalid title regex {pattern:?}: {error}"))
}

/// Validates the rules themselves. `workspace_known` checks a workspace
/// selector against the configuration.
pub fn validate_rules(
    rules: &[RuleDefinition],
    workspace_known: impl Fn(&str) -> bool,
) -> Result<(), String> {
    let mut ids = HashSet::new();
    for rule in rules {
        let id = rule.id.trim();
        if id.is_empty() || id.len() > 64 {
            return Err("rule IDs must be 1 to 64 characters".into());
        }
        if !ids.insert(id.to_ascii_lowercase()) {
            return Err(format!("duplicate rule ID {id:?}"));
        }
        if rule.bundle_id.trim().is_empty() || rule.bundle_id.len() > 255 {
            return Err(format!("rule {id:?} needs a bundle ID"));
        }
        if !(-1000..=1000).contains(&rule.priority) {
            return Err(format!(
                "rule {id:?}: priority must be between -1000 and 1000"
            ));
        }
        if let Some(title) = &rule.title {
            if title.is_empty() || title.len() > 512 {
                return Err(format!(
                    "rule {id:?}: title condition must be 1 to 512 characters"
                ));
            }
            if rule.title_match == TitleMatch::Regex {
                compile_title_regex(title).map_err(|error| format!("rule {id:?}: {error}"))?;
            }
        }
        if let Some(workspace) = &rule.workspace {
            if !workspace_known(workspace) {
                return Err(format!(
                    "rule {id:?}: workspace {workspace:?} is not configured"
                ));
            }
        }
        if rule.scratchpad.is_some()
            && (rule.workspace.is_some()
                || rule.display.is_some()
                || rule.mode != RuleMode::Unchanged)
        {
            return Err(format!(
                "rule {id:?}: a scratchpad rule cannot also move the window or set its mode"
            ));
        }
        if rule.mode == RuleMode::Unmanaged && (rule.workspace.is_some() || rule.display.is_some())
        {
            return Err(format!(
                "rule {id:?}: an unmanaged window cannot also be moved"
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct CompiledRule {
    definition: RuleDefinition,
    regex: Option<regex::Regex>,
    /// Position in configuration; breaks priority ties deterministically.
    order: usize,
    legacy: bool,
}

impl CompiledRule {
    fn title_matches(&self, title: &str) -> bool {
        let Some(condition) = &self.definition.title else {
            return true;
        };
        match self.definition.title_match {
            TitleMatch::Exact => title == condition,
            TitleMatch::Contains => title.contains(condition.as_str()),
            TitleMatch::Regex => self
                .regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(title)),
        }
    }
}

/// What caused an evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// The window appeared while the daemon was running.
    Created,
    /// The window existed before the daemon started.
    FirstSeen,
    TitleChanged,
    /// `rules apply` from the CLI or GUI.
    Manual,
}

/// Window metadata a rule can match on.
#[derive(Debug, Clone, Copy)]
pub struct WindowFacts<'a> {
    pub bundle_id: Option<&'a str>,
    pub title: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum RuleCheck {
    /// Matched and supplied the listed actions.
    Applied {
        actions: Vec<&'static str>,
    },
    /// Matched, but higher-priority rules already chose every action it has.
    Overridden,
    Disabled,
    BundleMismatch,
    TitleMismatch,
    /// The apply policy does not run for this trigger.
    PolicySkipped {
        policy: ApplyPolicy,
    },
    /// A higher-priority matching rule stopped evaluation.
    Stopped {
        by: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleCandidate {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub legacy: bool,
    pub check: RuleCheck,
}

/// Actions chosen for a window; each records the rule that chose it.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RulePlan {
    pub scratchpad: Option<(String, String)>,
    pub workspace: Option<(String, String)>,
    pub display: Option<(u32, String)>,
    pub mode: Option<(RuleMode, String)>,
}

impl RulePlan {
    pub fn is_empty(&self) -> bool {
        self.workspace.is_none()
            && self.display.is_none()
            && self.mode.is_none()
            && self.scratchpad.is_none()
    }
    pub fn moves(&self) -> bool {
        self.workspace.is_some() || self.display.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evaluation {
    pub trigger: Trigger,
    pub candidates: Vec<RuleCandidate>,
    /// Highest-priority rule that contributed an action.
    pub winner: Option<String>,
    pub plan: RulePlan,
}

/// Compiled rules in evaluation order.
#[derive(Debug, Clone, Default)]
pub struct RuleEngine {
    rules: Vec<CompiledRule>,
}

impl RuleEngine {
    /// `legacy` are `[[window_rules]]` entries `(bundle_id, mode, priority,
    /// enabled)`; they apply their mode on first sight, as they always did.
    pub fn compile(
        rules: &[RuleDefinition],
        legacy: &[(String, RuleMode, i32, bool)],
    ) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(rules.len() + legacy.len());
        for (order, definition) in rules.iter().enumerate() {
            let regex = match (&definition.title, definition.title_match) {
                (Some(pattern), TitleMatch::Regex) => Some(compile_title_regex(pattern)?),
                _ => None,
            };
            compiled.push(CompiledRule {
                definition: definition.clone(),
                regex,
                order,
                legacy: false,
            });
        }
        for (index, (bundle_id, mode, priority, enabled)) in legacy.iter().enumerate() {
            compiled.push(CompiledRule {
                definition: RuleDefinition {
                    id: format!("legacy-{}", index + 1),
                    name: format!("Window rule {}", index + 1),
                    enabled: *enabled,
                    bundle_id: bundle_id.clone(),
                    title: None,
                    title_match: TitleMatch::Exact,
                    workspace: None,
                    display: None,
                    mode: *mode,
                    priority: *priority,
                    apply: ApplyPolicy::OnFirstSeen,
                    stop: false,
                    scratchpad: None,
                },
                regex: None,
                order: rules.len() + index,
                legacy: true,
            });
        }
        compiled.sort_by(|a, b| {
            b.definition
                .priority
                .cmp(&a.definition.priority)
                .then(a.order.cmp(&b.order))
        });
        Ok(Self { rules: compiled })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn definitions(&self) -> impl Iterator<Item = (&RuleDefinition, bool)> {
        self.rules
            .iter()
            .map(|rule| (&rule.definition, rule.legacy))
    }

    /// True when some enabled rule for this application has a title
    /// condition, so evaluation should wait for a title.
    pub fn wants_title(&self, bundle_id: Option<&str>) -> bool {
        bundle_id.is_some_and(|bundle| {
            self.rules.iter().any(|rule| {
                rule.definition.enabled
                    && rule.definition.bundle_id == bundle
                    && rule.definition.title.is_some()
            })
        })
    }

    /// True when any enabled rule names this application.
    pub fn concerns(&self, bundle_id: Option<&str>) -> bool {
        bundle_id.is_some_and(|bundle| {
            self.rules
                .iter()
                .any(|rule| rule.definition.enabled && rule.definition.bundle_id == bundle)
        })
    }

    /// Deterministic: rules are visited by priority, then configuration
    /// order. Each action (workspace, display, mode) is taken from the first
    /// matching rule that sets it; a matching rule with `stop` ends the walk.
    /// Workspace and display always come from the same rule, so one window
    /// never receives two conflicting moves.
    pub fn evaluate(&self, facts: WindowFacts<'_>, trigger: Trigger) -> Evaluation {
        let mut plan = RulePlan::default();
        let mut candidates = Vec::new();
        let mut winner = None;
        let mut stopped_by: Option<String> = None;
        let title = facts.title.unwrap_or("");
        for rule in &self.rules {
            let definition = &rule.definition;
            if facts.bundle_id != Some(definition.bundle_id.as_str()) {
                continue;
            }
            let check = if let Some(by) = &stopped_by {
                RuleCheck::Stopped { by: by.clone() }
            } else if !definition.enabled {
                RuleCheck::Disabled
            } else if !rule.title_matches(title) {
                RuleCheck::TitleMismatch
            } else if !definition.apply.allows(trigger) {
                RuleCheck::PolicySkipped {
                    policy: definition.apply,
                }
            } else {
                let mut actions = Vec::new();
                if let Some(scratchpad) = &definition.scratchpad {
                    // A scratchpad owns placement and mode; nothing else applies.
                    if plan.scratchpad.is_none()
                        && plan.workspace.is_none()
                        && plan.display.is_none()
                        && plan.mode.is_none()
                    {
                        plan.scratchpad = Some((scratchpad.clone(), definition.id.clone()));
                        actions.push("scratchpad");
                    }
                }
                let moves = plan.scratchpad.is_none()
                    && (definition.workspace.is_some() || definition.display.is_some());
                if moves && plan.workspace.is_none() && plan.display.is_none() {
                    if let Some(workspace) = &definition.workspace {
                        plan.workspace = Some((workspace.clone(), definition.id.clone()));
                        actions.push("workspace");
                    }
                    if let Some(display) = definition.display {
                        plan.display = Some((display, definition.id.clone()));
                        actions.push("display");
                    }
                }
                if definition.mode != RuleMode::Unchanged
                    && plan.mode.is_none()
                    && plan.scratchpad.is_none()
                {
                    plan.mode = Some((definition.mode, definition.id.clone()));
                    actions.push("mode");
                }
                if definition.stop {
                    stopped_by = Some(definition.id.clone());
                }
                if actions.is_empty() {
                    RuleCheck::Overridden
                } else {
                    winner.get_or_insert_with(|| definition.id.clone());
                    RuleCheck::Applied { actions }
                }
            };
            candidates.push(RuleCandidate {
                id: definition.id.clone(),
                name: definition.display_name().to_string(),
                priority: definition.priority,
                legacy: rule.legacy,
                check,
            });
        }
        Evaluation {
            trigger,
            candidates,
            winner,
            plan,
        }
    }
}

/// Stable identity of a window for rules. AX tokens change when a window
/// leaves the observed Spaces and comes back; its CGWindowID does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct RuleWindowKey {
    pub process: ProcessInstance,
    /// CGWindowID when verified, else the AX token (less stable).
    pub cg_window_id: Option<u32>,
    pub token: u64,
}

impl RuleWindowKey {
    fn normalized(self) -> Self {
        if self.cg_window_id.is_some() {
            Self { token: 0, ..self }
        } else {
            self
        }
    }
}

/// How many observations a title-dependent evaluation waits for a title.
pub const TITLE_WAIT_OBSERVATIONS: u8 = 4;
/// How long it waits at most.
pub const TITLE_WAIT: Duration = Duration::from_secs(2);
/// Automatic attempts per window before rules leave it alone.
pub const MAX_RULE_ATTEMPTS: u8 = 2;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackedWindow {
    #[serde(skip)]
    pub first_seen: Instant,
    pub created: bool,
    pub title: Option<String>,
    pub title_waits: u8,
    /// Initial evaluation done (or given up).
    pub evaluated: bool,
    pub applied_rule: Option<String>,
    pub mode: Option<RuleMode>,
    /// Space Tylora put the window into; any other observed Space means the
    /// user moved it.
    pub expected_space: Option<u64>,
    pub manual_override: bool,
    pub pending_operation: Option<u64>,
    pub attempts: u8,
    pub last_error: Option<String>,
}

/// Per-window rule state. It decides when a window is (re)evaluated and
/// detects manual moves; it never performs operations itself.
#[derive(Debug, Default)]
pub struct RuleTracker {
    windows: HashMap<RuleWindowKey, TrackedWindow>,
    /// CGWindowIDs that existed when the daemon started.
    preexisting: HashSet<(i32, u32)>,
    preexisting_known: bool,
    pub overrides_detected: u64,
}

impl RuleTracker {
    /// Records the windows that existed at startup; only windows outside
    /// this set count as created.
    pub fn set_preexisting(&mut self, windows: impl IntoIterator<Item = (i32, u32)>) {
        self.preexisting = windows.into_iter().collect();
        self.preexisting_known = true;
    }

    pub fn get(&self, key: RuleWindowKey) -> Option<&TrackedWindow> {
        self.windows.get(&key.normalized())
    }

    pub fn len(&self) -> usize {
        self.windows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&RuleWindowKey, &TrackedWindow)> {
        self.windows.iter()
    }

    /// Called for every observation of a manageable window. Returns the
    /// trigger to evaluate, if any. Never returns a trigger while an
    /// operation for the window is in flight.
    pub fn observe(
        &mut self,
        key: RuleWindowKey,
        title: Option<&str>,
        space: Option<u64>,
        wants_title: bool,
        now: Instant,
    ) -> Option<Trigger> {
        let key = key.normalized();
        let title = title.filter(|title| !title.trim().is_empty());
        let preexisting = !self.preexisting_known
            || key
                .cg_window_id
                .is_none_or(|cg| self.preexisting.contains(&(key.process.pid, cg)));
        let entry = self.windows.entry(key).or_insert_with(|| TrackedWindow {
            first_seen: now,
            created: !preexisting,
            title: None,
            title_waits: 0,
            evaluated: false,
            applied_rule: None,
            mode: None,
            expected_space: None,
            manual_override: false,
            pending_operation: None,
            attempts: 0,
            last_error: None,
        });
        if entry.pending_operation.is_some() {
            return None;
        }
        if let (Some(expected), Some(space)) = (entry.expected_space, space) {
            if expected != space && !entry.manual_override {
                entry.manual_override = true;
                self.overrides_detected = self.overrides_detected.saturating_add(1);
            }
        }
        let previous = entry.title.clone();
        if let Some(title) = title {
            entry.title = Some(title.to_string());
        }
        if !entry.evaluated {
            let waited = entry.title_waits >= TITLE_WAIT_OBSERVATIONS
                || now.duration_since(entry.first_seen) >= TITLE_WAIT;
            if wants_title && entry.title.is_none() && !waited {
                entry.title_waits = entry.title_waits.saturating_add(1);
                return None;
            }
            entry.evaluated = true;
            return Some(if entry.created {
                Trigger::Created
            } else {
                Trigger::FirstSeen
            });
        }
        let changed = title.is_some() && previous.as_deref() != title;
        (changed && !entry.manual_override && entry.attempts < MAX_RULE_ATTEMPTS)
            .then_some(Trigger::TitleChanged)
    }

    /// Records an open window as already handled, so rules added later (hot
    /// reload) do not treat it as new. Existing entries are untouched.
    pub fn mark_seen(&mut self, key: RuleWindowKey, now: Instant) {
        self.windows
            .entry(key.normalized())
            .or_insert_with(|| TrackedWindow {
                first_seen: now,
                created: false,
                title: None,
                title_waits: 0,
                evaluated: true,
                applied_rule: None,
                mode: None,
                expected_space: None,
                manual_override: false,
                pending_operation: None,
                attempts: 0,
                last_error: None,
            });
    }

    /// The window's mode chosen by a rule, re-applied on every observation
    /// because AX scans report the default classification.
    pub fn mode(&self, key: RuleWindowKey) -> Option<RuleMode> {
        self.windows
            .get(&key.normalized())
            .and_then(|entry| entry.mode)
    }

    pub fn set_mode(&mut self, key: RuleWindowKey, rule: &str, mode: RuleMode) {
        if let Some(entry) = self.windows.get_mut(&key.normalized()) {
            entry.mode = Some(mode);
            entry.applied_rule = Some(rule.to_string());
        }
    }

    /// True when automatic moves are allowed for this window.
    pub fn may_move(&self, key: RuleWindowKey, trigger: Trigger) -> bool {
        self.windows.get(&key.normalized()).is_some_and(|entry| {
            entry.pending_operation.is_none()
                && (trigger == Trigger::Manual
                    || (!entry.manual_override && entry.attempts < MAX_RULE_ATTEMPTS))
        })
    }

    pub fn begin(&mut self, key: RuleWindowKey, operation: u64, trigger: Trigger) {
        if let Some(entry) = self.windows.get_mut(&key.normalized()) {
            entry.pending_operation = Some(operation);
            entry.attempts = entry.attempts.saturating_add(1);
            if trigger == Trigger::Manual {
                entry.manual_override = false;
            }
        }
    }

    /// Result of an operation. Late results for a superseded operation are
    /// ignored. Returns false when the result was stale.
    pub fn finish(
        &mut self,
        key: RuleWindowKey,
        operation: u64,
        rule: &str,
        result: Result<u64, String>,
    ) -> bool {
        let Some(entry) = self.windows.get_mut(&key.normalized()) else {
            return false;
        };
        if entry.pending_operation != Some(operation) {
            return false;
        }
        entry.pending_operation = None;
        match result {
            Ok(space) => {
                entry.expected_space = Some(space);
                entry.applied_rule = Some(rule.to_string());
                entry.last_error = None;
            }
            Err(error) => entry.last_error = Some(error),
        }
        true
    }

    /// Clears a manual override so automatic rules may act again.
    pub fn clear_override(&mut self, key: RuleWindowKey) -> bool {
        self.windows
            .get_mut(&key.normalized())
            .is_some_and(|entry| std::mem::replace(&mut entry.manual_override, false))
    }

    /// Forgets windows of a process that exited.
    pub fn process_exited(&mut self, process: ProcessInstance) {
        self.windows.retain(|key, _| key.process != process);
    }

    /// Forgets windows whose CGWindowID no longer exists anywhere.
    pub fn retain_existing(&mut self, existing: &HashSet<(i32, u32)>) {
        self.windows.retain(|key, entry| {
            entry.pending_operation.is_some()
                || key
                    .cg_window_id
                    .is_none_or(|cg| existing.contains(&(key.process.pid, cg)))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, bundle: &str) -> RuleDefinition {
        RuleDefinition {
            id: id.into(),
            name: String::new(),
            enabled: true,
            bundle_id: bundle.into(),
            title: None,
            title_match: TitleMatch::Exact,
            workspace: None,
            display: None,
            mode: RuleMode::Unchanged,
            priority: 0,
            apply: ApplyPolicy::OnCreate,
            stop: true,
            scratchpad: None,
        }
    }
    fn facts<'a>(bundle: &'a str, title: &'a str) -> WindowFacts<'a> {
        WindowFacts {
            bundle_id: Some(bundle),
            title: Some(title),
        }
    }
    fn key(cg: u32) -> RuleWindowKey {
        RuleWindowKey {
            process: ProcessInstance { pid: 9, epoch: 1 },
            cg_window_id: Some(cg),
            token: 1,
        }
    }

    #[test]
    fn exact_bundle_and_title_conditions() {
        let mut exact = rule("exact", "com.example.app");
        exact.title = Some("Inbox".into());
        exact.mode = RuleMode::Floating;
        let mut contains = rule("contains", "com.example.app");
        contains.title = Some("Settings".into());
        contains.title_match = TitleMatch::Contains;
        contains.workspace = Some("Code".into());
        let mut regex = rule("regex", "com.example.app");
        regex.title = Some(r"^Project \d+ —".into());
        regex.title_match = TitleMatch::Regex;
        regex.mode = RuleMode::Tiled;
        let engine = RuleEngine::compile(&[exact, contains, regex], &[]).unwrap();
        let eval = engine.evaluate(facts("com.example.app", "Inbox"), Trigger::Created);
        assert_eq!(eval.winner.as_deref(), Some("exact"));
        assert_eq!(eval.plan.mode, Some((RuleMode::Floating, "exact".into())));
        let eval = engine.evaluate(facts("com.example.app", "App Settings"), Trigger::Created);
        assert_eq!(eval.winner.as_deref(), Some("contains"));
        assert_eq!(
            eval.plan.workspace,
            Some(("Code".into(), "contains".into()))
        );
        let eval = engine.evaluate(
            facts("com.example.app", "Project 42 — main"),
            Trigger::Created,
        );
        assert_eq!(eval.winner.as_deref(), Some("regex"));
        let eval = engine.evaluate(facts("com.example.app", "Inbox (2)"), Trigger::Created);
        assert_eq!(eval.winner, None, "exact title must match exactly");
        let eval = engine.evaluate(facts("com.other.app", "Inbox"), Trigger::Created);
        assert!(
            eval.candidates.is_empty(),
            "other bundles are not candidates"
        );
    }

    #[test]
    fn priority_order_conflicts_and_stop() {
        let mut low = rule("low", "app");
        low.workspace = Some("Browser".into());
        low.mode = RuleMode::Floating;
        low.priority = 10;
        let mut high = rule("high", "app");
        high.workspace = Some("Code".into());
        high.priority = 100;
        high.stop = false;
        let engine = RuleEngine::compile(&[low.clone(), high.clone()], &[]).unwrap();
        let eval = engine.evaluate(facts("app", ""), Trigger::Created);
        assert_eq!(eval.winner.as_deref(), Some("high"));
        // One move only: the lower rule cannot add a second workspace, but
        // supplies the mode the higher rule left unchanged.
        assert_eq!(eval.plan.workspace, Some(("Code".into(), "high".into())));
        assert_eq!(eval.plan.mode, Some((RuleMode::Floating, "low".into())));
        // With stop, lower rules are not consulted at all.
        high.stop = true;
        let engine = RuleEngine::compile(&[low, high], &[]).unwrap();
        let eval = engine.evaluate(facts("app", ""), Trigger::Created);
        assert_eq!(eval.plan.mode, None);
        assert_eq!(
            eval.candidates[1].check,
            RuleCheck::Stopped { by: "high".into() }
        );
        // Equal priority: configuration order decides.
        let mut first = rule("first", "app");
        first.mode = RuleMode::Tiled;
        let mut second = rule("second", "app");
        second.mode = RuleMode::Floating;
        let engine = RuleEngine::compile(&[first, second], &[]).unwrap();
        assert_eq!(
            engine
                .evaluate(facts("app", ""), Trigger::Created)
                .winner
                .as_deref(),
            Some("first")
        );
    }

    #[test]
    fn disabled_rules_and_policies() {
        let mut disabled = rule("off", "app");
        disabled.enabled = false;
        disabled.mode = RuleMode::Floating;
        disabled.priority = 50;
        let mut manual = rule("manual", "app");
        manual.apply = ApplyPolicy::Manual;
        manual.workspace = Some("Code".into());
        let engine = RuleEngine::compile(&[disabled, manual], &[]).unwrap();
        let eval = engine.evaluate(facts("app", ""), Trigger::Created);
        assert_eq!(eval.candidates[0].check, RuleCheck::Disabled);
        assert!(matches!(
            eval.candidates[1].check,
            RuleCheck::PolicySkipped { .. }
        ));
        assert!(eval.plan.is_empty());
        let eval = engine.evaluate(facts("app", ""), Trigger::Manual);
        assert_eq!(eval.winner.as_deref(), Some("manual"));
        assert!(ApplyPolicy::OnCreate.allows(Trigger::Created));
        assert!(!ApplyPolicy::OnCreate.allows(Trigger::FirstSeen));
        assert!(ApplyPolicy::OnFirstSeen.allows(Trigger::FirstSeen));
        assert!(!ApplyPolicy::OnFirstSeen.allows(Trigger::TitleChanged));
        assert!(ApplyPolicy::OnTitleChange.allows(Trigger::TitleChanged));
    }

    #[test]
    fn legacy_window_rules_keep_working() {
        let engine = RuleEngine::compile(
            &[],
            &[("com.apple.calculator".into(), RuleMode::Floating, 0, true)],
        )
        .unwrap();
        let eval = engine.evaluate(facts("com.apple.calculator", ""), Trigger::FirstSeen);
        assert_eq!(
            eval.plan.mode,
            Some((RuleMode::Floating, "legacy-1".into()))
        );
        assert!(eval.candidates[0].legacy);
    }

    #[test]
    fn validation_catches_bad_rules() {
        let known = |name: &str| name == "Code";
        let mut bad = rule("a", "app");
        bad.title = Some("(".into());
        bad.title_match = TitleMatch::Regex;
        assert!(validate_rules(&[bad], known).is_err());
        assert!(validate_rules(&[rule("a", "app"), rule("A", "app")], known).is_err());
        let mut missing = rule("m", "app");
        missing.workspace = Some("Nope".into());
        assert!(validate_rules(&[missing], known).is_err());
        let mut unmanaged = rule("u", "app");
        unmanaged.mode = RuleMode::Unmanaged;
        unmanaged.workspace = Some("Code".into());
        assert!(validate_rules(&[unmanaged], known).is_err());
        assert!(validate_rules(&[rule("ok", "app")], known).is_ok());
        assert!(validate_rules(&[rule("", "app")], known).is_err());
    }

    #[test]
    fn tracker_distinguishes_created_from_preexisting() {
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([(9, 100)]);
        let now = Instant::now();
        assert_eq!(
            tracker.observe(key(100), Some("a"), Some(1), false, now),
            Some(Trigger::FirstSeen)
        );
        assert_eq!(
            tracker.observe(key(200), Some("b"), Some(1), false, now),
            Some(Trigger::Created)
        );
        // Repeated events for the same window never re-trigger.
        assert_eq!(
            tracker.observe(key(200), Some("b"), Some(1), false, now),
            None
        );
        // A new AX token for the same CGWindowID is the same window.
        let mut retokened = key(200);
        retokened.token = 77;
        assert_eq!(
            tracker.observe(retokened, Some("b"), Some(1), false, now),
            None
        );
    }

    #[test]
    fn tracker_waits_for_title_a_bounded_number_of_times() {
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        let now = Instant::now();
        for _ in 0..TITLE_WAIT_OBSERVATIONS {
            assert_eq!(tracker.observe(key(5), None, None, true, now), None);
        }
        assert_eq!(
            tracker.observe(key(5), None, None, true, now),
            Some(Trigger::Created)
        );
        // A title arriving in time triggers immediately.
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        assert_eq!(tracker.observe(key(6), Some(""), None, true, now), None);
        assert_eq!(
            tracker.observe(key(6), Some("Doc"), None, true, now),
            Some(Trigger::Created)
        );
        // Waiting is also bounded in time.
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        assert_eq!(tracker.observe(key(7), None, None, true, now), None);
        assert_eq!(
            tracker.observe(key(7), None, None, true, now + TITLE_WAIT),
            Some(Trigger::Created)
        );
    }

    #[test]
    fn title_change_retriggers_until_override_or_attempt_limit() {
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        let now = Instant::now();
        tracker.observe(key(8), Some("one"), Some(1), false, now);
        assert_eq!(
            tracker.observe(key(8), Some("one"), Some(1), false, now),
            None
        );
        assert_eq!(
            tracker.observe(key(8), Some("two"), Some(1), false, now),
            Some(Trigger::TitleChanged)
        );
        for operation in 1..=MAX_RULE_ATTEMPTS as u64 {
            tracker.begin(key(8), operation, Trigger::TitleChanged);
            tracker.finish(key(8), operation, "r", Ok(1));
        }
        assert_eq!(
            tracker.observe(key(8), Some("three"), Some(1), false, now),
            None
        );
    }

    #[test]
    fn manual_move_sets_override_and_manual_apply_clears_it() {
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        let now = Instant::now();
        assert_eq!(
            tracker.observe(key(1), Some("x"), Some(10), false, now),
            Some(Trigger::Created)
        );
        tracker.begin(key(1), 1, Trigger::Created);
        // While the move is in flight, observations elsewhere are ignored.
        assert_eq!(
            tracker.observe(key(1), Some("x"), Some(10), false, now),
            None
        );
        assert!(tracker.finish(key(1), 1, "r", Ok(20)));
        assert!(!tracker.get(key(1)).unwrap().manual_override);
        // The user moves it to Space 30.
        tracker.observe(key(1), Some("x"), Some(30), false, now);
        assert!(tracker.get(key(1)).unwrap().manual_override);
        assert!(!tracker.may_move(key(1), Trigger::Created));
        assert!(tracker.may_move(key(1), Trigger::Manual));
        tracker.begin(key(1), 2, Trigger::Manual);
        assert!(!tracker.get(key(1)).unwrap().manual_override);
        assert_eq!(tracker.overrides_detected, 1);
    }

    #[test]
    fn late_and_stale_results_are_ignored_and_closed_windows_forgotten() {
        let mut tracker = RuleTracker::default();
        tracker.set_preexisting([]);
        let now = Instant::now();
        tracker.observe(key(1), None, Some(1), false, now);
        tracker.begin(key(1), 5, Trigger::Created);
        assert!(!tracker.finish(key(1), 4, "r", Ok(2)), "older operation");
        assert!(tracker.finish(key(1), 5, "r", Err("timeout".into())));
        assert_eq!(
            tracker.get(key(1)).unwrap().last_error.as_deref(),
            Some("timeout")
        );
        assert!(
            !tracker.finish(key(1), 5, "r", Ok(2)),
            "duplicate late result"
        );
        // Closed while unknown to the tracker: a result is simply dropped.
        assert!(!tracker.finish(key(99), 1, "r", Ok(2)));
        let existing: HashSet<_> = HashSet::new();
        tracker.retain_existing(&existing);
        assert!(tracker.is_empty());
        tracker.observe(key(2), None, None, false, now);
        tracker.process_exited(ProcessInstance { pid: 9, epoch: 1 });
        assert!(tracker.is_empty());
    }
}
