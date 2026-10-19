use crate::{DisplayId, Rect};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Instant};
use tracing::{debug, info};

/// PID plus adapter-assigned process incarnation; prevents PID reuse aliasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub pid: i32,
    pub epoch: u64,
}

/// Adapter token is stable while its AX element can be matched with CFEqual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ManagedWindowId {
    pub process: ProcessInstance,
    pub token: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLifecycle {
    Discovered,
    Active,
    Hidden,
    Minimized,
    Destroyed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagementMode {
    Tiled,
    Floating,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowSnapshot {
    pub id: ManagedWindowId,
    /// Verified AX-to-CG identity. None if the private identity API is absent.
    pub cg_window_id: Option<u32>,
    /// Single native Space membership observed with the snapshot, if known.
    pub native_space_id: Option<u64>,
    pub application_id: Option<String>,
    /// AX title when the application exposes one; may be empty while a
    /// window is being created.
    pub title: Option<String>,
    pub frame: Rect,
    pub display_id: Option<DisplayId>,
    pub lifecycle: WindowLifecycle,
    pub mode: ManagementMode,
}

#[derive(Debug, Clone)]
pub struct WindowRecord {
    pub observed: WindowSnapshot,
    pub desired: Option<Rect>,
    pub applied: Option<Rect>,
    pub last_successful_update: Instant,
    pub generation: u64,
    pub version: u64,
    pub pending_apply: Option<u64>,
    pub failed_target: Option<Rect>,
    pub failed_attempts: u8,
    pub retry_after: Option<Instant>,
}

#[derive(Default)]
pub struct Registry {
    windows: HashMap<ManagedWindowId, WindowRecord>,
    next_generation: u64,
}

impl Registry {
    pub fn get(&self, id: ManagedWindowId) -> Option<&WindowRecord> {
        self.windows.get(&id)
    }
    pub fn get_mut(&mut self, id: ManagedWindowId) -> Option<&mut WindowRecord> {
        self.windows.get_mut(&id)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ManagedWindowId, &WindowRecord)> {
        self.windows.iter()
    }
    pub fn len(&self) -> usize {
        self.windows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Returns true only when a new registration was made.
    pub fn upsert(&mut self, snapshot: WindowSnapshot, version: u64) -> bool {
        if let Some(record) = self.windows.get_mut(&snapshot.id) {
            debug!(
                pid = snapshot.id.process.pid,
                token = snapshot.id.token,
                "window observation updated"
            );
            if record.pending_apply.is_none()
                && record
                    .applied
                    .is_some_and(|applied| !applied.near(snapshot.frame, 1.0))
            {
                record.applied = None;
            }
            if record
                .desired
                .is_some_and(|desired| snapshot.frame.near(desired, 1.0))
            {
                record.applied = Some(snapshot.frame);
                record.failed_target = None;
                record.failed_attempts = 0;
                record.retry_after = None;
            }
            record.observed = snapshot;
            record.last_successful_update = Instant::now();
            record.version = version;
            return false;
        }
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        info!(
            pid = snapshot.id.process.pid,
            token = snapshot.id.token,
            generation = self.next_generation,
            "window registered"
        );
        self.windows.insert(
            snapshot.id,
            WindowRecord {
                observed: snapshot,
                desired: None,
                applied: None,
                last_successful_update: Instant::now(),
                generation: self.next_generation,
                version,
                pending_apply: None,
                failed_target: None,
                failed_attempts: 0,
                retry_after: None,
            },
        );
        true
    }

    pub fn remove(&mut self, id: ManagedWindowId) -> Option<WindowRecord> {
        let removed = self.windows.remove(&id);
        if removed.is_some() {
            info!(pid = id.process.pid, token = id.token, "window removed");
        }
        removed
    }
    pub fn remove_process(&mut self, process: ProcessInstance) -> Vec<ManagedWindowId> {
        let ids: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| id.process == process)
            .collect();
        for id in &ids {
            self.remove(*id);
        }
        ids
    }

    /// Only an explicitly complete process scan is authoritative for removals.
    pub fn reconcile_process(
        &mut self,
        process: ProcessInstance,
        observed: &[WindowSnapshot],
        version: u64,
    ) -> Vec<ManagedWindowId> {
        let seen: std::collections::HashSet<_> = observed.iter().map(|w| w.id).collect();
        let stale: Vec<_> = self
            .windows
            .keys()
            .copied()
            .filter(|id| id.process == process && !seen.contains(id))
            .collect();
        for id in &stale {
            self.remove(*id);
        }
        for snapshot in observed.iter().filter(|w| w.id.process == process) {
            self.upsert(snapshot.clone(), version);
        }
        stale
    }
}
