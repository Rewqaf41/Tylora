//! `tylora subscribe`: a stream of JSON lines describing what changes.
//!
//! A client sends `subscribe [EVENTS]` on the daemon socket and keeps the
//! connection open. The daemon answers with a `subscribed` line, the current
//! state (`workspace`, `focus`, `layout` and `display_added` per display),
//! then one line per change. The coordinator only diffs plain snapshots
//! ([`EventState`]); every subscriber has its own writer thread behind a
//! bounded queue, so a slow or stuck client is dropped instead of stalling
//! the daemon.

use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    io::Write,
    os::unix::net::UnixStream,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

/// Event names a client may ask for.
pub const EVENT_NAMES: [&str; 8] = [
    "workspace",
    "focus",
    "layout",
    "window_opened",
    "window_closed",
    "display_added",
    "display_removed",
    "daemon",
];
const MAX_SUBSCRIBERS: usize = 32;
const QUEUE: usize = 256;

struct Subscriber {
    filter: Option<HashSet<String>>,
    lines: SyncSender<String>,
}
impl Subscriber {
    fn wants(&self, event: &str) -> bool {
        self.filter
            .as_ref()
            .is_none_or(|names| names.contains(event))
    }
}

#[derive(Default)]
struct Hub {
    subscribers: Vec<Subscriber>,
    /// Latest state event per key ("workspace:1", "focus", …), replayed to
    /// new subscribers.
    latest: BTreeMap<String, Value>,
}

#[derive(Clone, Default)]
pub struct EventHub {
    hub: Arc<Mutex<Hub>>,
    count: Arc<AtomicUsize>,
}

/// Parses `subscribe`, `subscribe workspace,focus` or `subscribe --events …`.
pub fn parse_request(line: &str) -> Option<Result<Option<HashSet<String>>, String>> {
    let rest = line.strip_prefix("subscribe")?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let rest = rest.trim();
    let rest = rest.strip_prefix("--events").unwrap_or(rest).trim();
    if rest.is_empty() {
        return Some(Ok(None));
    }
    let mut names = HashSet::new();
    for name in rest
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        if !EVENT_NAMES.contains(&name) {
            return Some(Err(format!(
                "unknown event {name:?}; known: {}",
                EVENT_NAMES.join(", ")
            )));
        }
        names.insert(name.to_owned());
    }
    Some(Ok(Some(names)))
}

impl EventHub {
    pub fn has_subscribers(&self) -> bool {
        self.count.load(Ordering::Acquire) > 0
    }

    /// Takes over an accepted connection whose first line was `subscribe`.
    pub fn subscribe(&self, mut stream: UnixStream, filter: Option<HashSet<String>>) {
        let Ok(mut hub) = self.hub.lock() else {
            return;
        };
        if hub.subscribers.len() >= MAX_SUBSCRIBERS {
            let _ = stream.write_all(
                b"{\"version\":1,\"event\":\"error\",\"message\":\"too many subscribers\"}\n",
            );
            return;
        }
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let (lines, queue) = mpsc::sync_channel::<String>(QUEUE);
        let count = self.count.clone();
        let spawned = thread::Builder::new()
            .name("tylora-subscriber".into())
            .spawn(move || {
                for line in queue {
                    if stream.write_all(line.as_bytes()).is_err() {
                        break;
                    }
                }
            });
        if spawned.is_err() {
            return;
        }
        let subscriber = Subscriber { filter, lines };
        let mut events: Vec<&str> = EVENT_NAMES
            .iter()
            .copied()
            .filter(|name| subscriber.wants(name))
            .collect();
        events.sort_unstable();
        let mut greeting = vec![json!({"version":1,"event":"subscribed","events":events})];
        greeting.extend(
            hub.latest
                .values()
                .filter(|event| subscriber.wants(event["event"].as_str().unwrap_or("")))
                .cloned(),
        );
        for event in greeting {
            if subscriber.lines.try_send(line(&event)).is_err() {
                return;
            }
        }
        hub.subscribers.push(subscriber);
        count.store(hub.subscribers.len(), Ordering::Release);
    }

    /// Sends an event to every interested subscriber; `key` also keeps it as
    /// current state for later subscribers.
    pub fn publish(&self, key: Option<String>, event: Value) {
        let Ok(mut hub) = self.hub.lock() else {
            return;
        };
        if let Some(key) = key {
            hub.latest.insert(key, event.clone());
        }
        if hub.subscribers.is_empty() {
            return;
        }
        let name = event["event"].as_str().unwrap_or("").to_owned();
        let text = line(&event);
        hub.subscribers.retain(|subscriber| {
            !subscriber.wants(&name)
                || match subscriber.lines.try_send(text.clone()) {
                    Ok(()) => true,
                    // Gone, or too slow to keep up: drop rather than stall.
                    Err(TrySendError::Disconnected(_) | TrySendError::Full(_)) => false,
                }
        });
        self.count.store(hub.subscribers.len(), Ordering::Release);
    }

    /// Forgets state events whose key is gone (a removed display).
    pub fn forget(&self, key: &str) {
        if let Ok(mut hub) = self.hub.lock() {
            hub.latest.remove(key);
        }
    }
}

fn line(event: &Value) -> String {
    let mut text = event.to_string();
    text.push('\n');
    text
}

/// One managed window as events describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowFacts {
    pub pid: i32,
    pub app: Option<String>,
    pub title: Option<String>,
    pub cg_window_id: Option<u32>,
    pub display: u32,
    pub workspace: Option<u8>,
}

/// What a display currently shows.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayFacts {
    pub uuid: Option<String>,
    pub space_id: Option<u64>,
    pub desktop: Option<u8>,
    pub workspace: Option<(u8, String)>,
    pub layout: Option<String>,
}

/// Snapshot the coordinator diffs; keys are display IDs and (pid, token).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventState {
    pub displays: BTreeMap<u32, DisplayFacts>,
    pub windows: BTreeMap<(i32, u64), WindowFacts>,
    pub focus: Option<(i32, u64)>,
}

fn window_json(event: &str, window: &WindowFacts) -> Value {
    json!({
        "version": 1, "event": event, "pid": window.pid, "app": window.app,
        "title": window.title, "cg_window_id": window.cg_window_id,
        "display": window.display, "workspace": window.workspace,
    })
}

/// Events turning `before` into `after`, each with its state key (if any).
pub fn diff(before: &EventState, after: &EventState) -> Vec<(Option<String>, Value)> {
    let mut events = Vec::new();
    let displays: BTreeSet<u32> = before
        .displays
        .keys()
        .chain(after.displays.keys())
        .copied()
        .collect();
    for display in displays {
        let old = before.displays.get(&display);
        let new = after.displays.get(&display);
        match (old, new) {
            (Some(_), None) => events.push((
                None,
                json!({"version":1,"event":"display_removed","display":display}),
            )),
            (None, Some(facts)) => events.push((
                Some(format!("display:{display}")),
                json!({"version":1,"event":"display_added","display":display,"uuid":facts.uuid}),
            )),
            _ => {}
        }
        let Some(new) = new else { continue };
        if old.map(|old| (old.space_id, &old.workspace)) != Some((new.space_id, &new.workspace)) {
            events.push((
                Some(format!("workspace:{display}")),
                json!({
                    "version": 1, "event": "workspace", "display": display,
                    "space_id": new.space_id, "desktop": new.desktop,
                    "workspace": new.workspace.as_ref().map(|(id, name)| json!({"id": id, "name": name})),
                }),
            ));
        }
        if old.map(|old| (old.space_id, &old.layout)) != Some((new.space_id, &new.layout)) {
            events.push((
                Some(format!("layout:{display}")),
                json!({
                    "version": 1, "event": "layout", "display": display,
                    "space_id": new.space_id, "layout": new.layout,
                }),
            ));
        }
    }
    for (key, window) in &after.windows {
        if !before.windows.contains_key(key) {
            events.push((None, window_json("window_opened", window)));
        }
    }
    for (key, window) in &before.windows {
        if !after.windows.contains_key(key) {
            events.push((None, window_json("window_closed", window)));
        }
    }
    let focused = |state: &EventState| state.focus.and_then(|key| state.windows.get(&key).cloned());
    let (old_focus, new_focus) = (focused(before), focused(after));
    if old_focus != new_focus {
        events.push((
            Some("focus".into()),
            match &new_focus {
                Some(window) => window_json("focus", window),
                None => json!({"version":1,"event":"focus","pid":null}),
            },
        ));
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn display(space: u64, workspace: Option<(u8, &str)>) -> DisplayFacts {
        DisplayFacts {
            uuid: Some("abc".into()),
            space_id: Some(space),
            desktop: Some(1),
            workspace: workspace.map(|(id, name)| (id, name.to_owned())),
            layout: Some("bsp".into()),
        }
    }
    fn window(pid: i32, app: &str) -> WindowFacts {
        WindowFacts {
            pid,
            app: Some(app.into()),
            title: Some("t".into()),
            cg_window_id: Some(9),
            display: 1,
            workspace: None,
        }
    }
    fn names(events: &[(Option<String>, Value)]) -> Vec<String> {
        events
            .iter()
            .map(|(_, event)| event["event"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn parses_requests_and_rejects_unknown_events() {
        assert_eq!(parse_request("subscribe"), Some(Ok(None)));
        assert_eq!(
            parse_request("subscribe --events focus, workspace"),
            Some(Ok(Some(HashSet::from([
                "focus".into(),
                "workspace".into()
            ]))))
        );
        assert!(matches!(parse_request("subscribe nope"), Some(Err(_))));
        assert_eq!(parse_request("subscriber"), None);
        assert_eq!(parse_request("status"), None);
    }

    #[test]
    fn diff_reports_workspace_layout_windows_focus_and_displays() {
        let mut before = EventState::default();
        before.displays.insert(1, display(4, Some((1, "Code"))));
        before
            .windows
            .insert((7, 1), window(7, "com.apple.Terminal"));
        before.focus = Some((7, 1));
        assert!(diff(&before, &before).is_empty());

        let mut after = before.clone();
        after.displays.insert(1, display(5, Some((2, "Web"))));
        after.displays.insert(2, display(9, None));
        after.windows.insert((8, 1), window(8, "com.apple.Safari"));
        after.focus = Some((8, 1));
        let events = diff(&before, &after);
        assert_eq!(
            names(&events),
            vec![
                "workspace",
                "layout",
                "display_added",
                "workspace",
                "layout",
                "window_opened",
                "focus"
            ]
        );
        assert_eq!(events[0].1["workspace"]["name"], "Web");
        assert_eq!(events.last().unwrap().1["app"], "com.apple.Safari");

        let mut closed = after.clone();
        closed.windows.remove(&(8, 1));
        closed.focus = None;
        closed.displays.remove(&2);
        assert_eq!(
            names(&diff(&after, &closed)),
            vec!["display_removed", "window_closed", "focus"]
        );
    }

    #[test]
    fn subscribers_get_state_then_filtered_changes() {
        let hub = EventHub::default();
        hub.publish(
            Some("focus".into()),
            json!({"version":1,"event":"focus","pid":7}),
        );
        let (client, server) = UnixStream::pair().unwrap();
        hub.subscribe(server, Some(HashSet::from(["focus".to_owned()])));
        assert!(hub.has_subscribers());
        hub.publish(None, json!({"version":1,"event":"window_opened","pid":8}));
        hub.publish(
            Some("focus".into()),
            json!({"version":1,"event":"focus","pid":8}),
        );
        let mut reader = BufReader::new(client);
        let mut read = || {
            let mut text = String::new();
            reader.read_line(&mut text).unwrap();
            serde_json::from_str::<Value>(&text).unwrap()
        };
        assert_eq!(read()["event"], "subscribed");
        assert_eq!(read()["pid"], 7);
        assert_eq!(read()["pid"], 8);
    }

    #[test]
    fn closed_clients_are_dropped() {
        let hub = EventHub::default();
        let (client, server) = UnixStream::pair().unwrap();
        hub.subscribe(server, None);
        drop(client);
        for _ in 0..(QUEUE + 8) {
            hub.publish(None, json!({"version":1,"event":"focus","pid":1}));
            if !hub.has_subscribers() {
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(!hub.has_subscribers());
    }
}
