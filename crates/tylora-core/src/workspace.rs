//! Pure workspace ownership. Visibility changes are a transaction supplied by the adapter.

use crate::{
    commands::CommandError, coordinator::LayoutPolicy, registry::ManagedWindowId, DisplayId,
};
use std::collections::{HashMap, HashSet};

pub trait WorkspaceVisibilityBackend {
    /// Either complete the transition or restore the original visibility.
    fn transition(
        &mut self,
        hide: &[ManagedWindowId],
        show: &[ManagedWindowId],
    ) -> Result<(), CommandError>;
}

/// Safe fallback for apps where per-window visibility cannot be controlled.
pub struct UnsupportedVisibility;
impl WorkspaceVisibilityBackend for UnsupportedVisibility {
    fn transition(
        &mut self,
        hide: &[ManagedWindowId],
        show: &[ManagedWindowId],
    ) -> Result<(), CommandError> {
        if hide.is_empty() && show.is_empty() {
            Ok(())
        } else {
            Err(CommandError::VisibilityUnsupported)
        }
    }
}

pub struct Workspace<L> {
    pub id: u8,
    pub name: String,
    pub display: DisplayId,
    pub layout: L,
    pub windows: HashSet<ManagedWindowId>,
    pub floating: HashSet<ManagedWindowId>,
    pub last_focused: Option<ManagedWindowId>,
}

pub struct WorkspaceEngine<L: LayoutPolicy + Default> {
    workspaces: HashMap<u8, Workspace<L>>,
    active: HashMap<DisplayId, u8>,
    owner: HashMap<ManagedWindowId, u8>,
}

impl<L: LayoutPolicy + Default> Default for WorkspaceEngine<L> {
    fn default() -> Self {
        Self {
            workspaces: HashMap::new(),
            active: HashMap::new(),
            owner: HashMap::new(),
        }
    }
}
impl<L: LayoutPolicy + Default> WorkspaceEngine<L> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn create(&mut self, id: u8, name: String, display: DisplayId) -> Result<(), CommandError> {
        if id == 0 || name.trim().is_empty() || self.workspaces.contains_key(&id) {
            return Err(CommandError::InvalidArgument);
        }
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name,
                display,
                layout: L::default(),
                windows: HashSet::new(),
                floating: HashSet::new(),
                last_focused: None,
            },
        );
        self.active.entry(display).or_insert(id);
        Ok(())
    }
    pub fn delete_empty(&mut self, id: u8) -> Result<(), CommandError> {
        let ws = self
            .workspaces
            .get(&id)
            .ok_or(CommandError::WorkspaceNotFound)?;
        if !ws.windows.is_empty() || self.active.get(&ws.display) == Some(&id) {
            return Err(CommandError::InvalidArgument);
        }
        self.workspaces.remove(&id);
        Ok(())
    }
    pub fn rename(&mut self, id: u8, name: String) -> Result<(), CommandError> {
        if name.trim().is_empty() {
            return Err(CommandError::InvalidArgument);
        }
        self.workspaces
            .get_mut(&id)
            .ok_or(CommandError::WorkspaceNotFound)?
            .name = name;
        Ok(())
    }
    pub fn workspace(&self, id: u8) -> Option<&Workspace<L>> {
        self.workspaces.get(&id)
    }
    pub fn workspace_mut(&mut self, id: u8) -> Option<&mut Workspace<L>> {
        self.workspaces.get_mut(&id)
    }
    pub fn owned_windows(&self) -> Vec<ManagedWindowId> {
        self.owner.keys().copied().collect()
    }
    pub fn workspaces(&self) -> impl Iterator<Item = &Workspace<L>> {
        self.workspaces.values()
    }
    pub fn active(&self, display: DisplayId) -> Option<u8> {
        self.active.get(&display).copied()
    }
    pub fn owner(&self, window: ManagedWindowId) -> Option<u8> {
        self.owner.get(&window).copied()
    }
    pub fn register(
        &mut self,
        window: ManagedWindowId,
        display: DisplayId,
        floating: bool,
    ) -> Result<(), CommandError> {
        if self.owner.contains_key(&window) {
            let owner = self.owner[&window];
            if let Some(ws) = self.workspaces.get_mut(&owner) {
                if floating && ws.floating.insert(window) {
                    ws.layout.remove_window(window);
                }
                if !floating && ws.floating.remove(&window) {
                    ws.layout.insert_window(window);
                }
            }
            return Ok(());
        }
        let id = self
            .active(display)
            .ok_or(CommandError::WorkspaceNotFound)?;
        self.register_into(window, id, floating)
    }
    pub fn register_into(
        &mut self,
        window: ManagedWindowId,
        id: u8,
        floating: bool,
    ) -> Result<(), CommandError> {
        if self.owner.contains_key(&window) {
            return Ok(());
        }
        let ws = self
            .workspaces
            .get_mut(&id)
            .ok_or(CommandError::WorkspaceNotFound)?;
        ws.windows.insert(window);
        if floating {
            ws.floating.insert(window);
        } else {
            ws.layout.insert_window(window);
        }
        self.owner.insert(window, id);
        Ok(())
    }
    pub fn remove(&mut self, window: ManagedWindowId) {
        if let Some(id) = self.owner.remove(&window) {
            if let Some(ws) = self.workspaces.get_mut(&id) {
                ws.windows.remove(&window);
                ws.floating.remove(&window);
                ws.layout.remove_window(window);
                if ws.last_focused == Some(window) {
                    ws.last_focused = None;
                }
            }
        }
    }
    pub fn set_focused(&mut self, window: ManagedWindowId) {
        if let Some(id) = self.owner(window) {
            self.workspaces.get_mut(&id).unwrap().last_focused = Some(window);
        }
    }
    pub fn focus<B: WorkspaceVisibilityBackend>(
        &mut self,
        display: DisplayId,
        id: u8,
        backend: &mut B,
    ) -> Result<Option<ManagedWindowId>, CommandError> {
        let target = self
            .workspaces
            .get(&id)
            .ok_or(CommandError::WorkspaceNotFound)?;
        if target.display != display {
            return Err(CommandError::DisplayNotFound);
        }
        let old_id = self
            .active(display)
            .ok_or(CommandError::WorkspaceNotFound)?;
        if old_id == id {
            return Ok(target.last_focused);
        }
        let mut hide: Vec<_> = self.workspaces[&old_id].windows.iter().copied().collect();
        let mut show: Vec<_> = target.windows.iter().copied().collect();
        hide.sort();
        show.sort();
        backend.transition(&hide, &show)?;
        self.active.insert(display, id);
        Ok(target.last_focused)
    }
    pub fn move_window<B: WorkspaceVisibilityBackend>(
        &mut self,
        window: ManagedWindowId,
        target: u8,
        backend: &mut B,
    ) -> Result<(), CommandError> {
        let source = self.owner(window).ok_or(CommandError::WindowNotFound)?;
        if source == target {
            return Ok(());
        }
        let from = self.workspaces.get(&source).unwrap();
        let to = self
            .workspaces
            .get(&target)
            .ok_or(CommandError::WorkspaceNotFound)?;
        if from.display != to.display {
            return Err(CommandError::DisplayNotFound);
        }
        let source_active = self.active(from.display) == Some(source);
        let target_active = self.active(to.display) == Some(target);
        let one = [window];
        backend.transition(
            if source_active { &one } else { &[] },
            if target_active { &one } else { &[] },
        )?;
        let floating = from.floating.contains(&window);
        let from = self.workspaces.get_mut(&source).unwrap();
        from.windows.remove(&window);
        from.floating.remove(&window);
        from.layout.remove_window(window);
        if from.last_focused == Some(window) {
            from.last_focused = None;
        }
        let to = self.workspaces.get_mut(&target).unwrap();
        to.windows.insert(window);
        if floating {
            to.floating.insert(window);
        } else {
            to.layout.insert_window(window);
        }
        self.owner.insert(window, target);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProcessInstance;
    #[derive(Default)]
    struct Layout(HashSet<ManagedWindowId>);
    impl LayoutPolicy for Layout {
        fn insert_window(&mut self, id: ManagedWindowId) {
            self.0.insert(id);
        }
        fn remove_window(&mut self, id: ManagedWindowId) {
            self.0.remove(&id);
        }
        fn calculate_layout(&self, _: crate::Rect) -> Vec<(ManagedWindowId, crate::Rect)> {
            vec![]
        }
    }
    fn id(token: u64) -> ManagedWindowId {
        ManagedWindowId {
            process: ProcessInstance { pid: 42, epoch: 1 },
            token,
        }
    }
    #[test]
    fn independent_displays_and_focus_restoration() {
        let mut engine = WorkspaceEngine::<Layout>::new();
        engine.create(1, "one".into(), DisplayId(1)).unwrap();
        engine.create(2, "two".into(), DisplayId(1)).unwrap();
        engine.create(3, "three".into(), DisplayId(2)).unwrap();
        engine.register(id(1), DisplayId(1), false).unwrap();
        engine.set_focused(id(1));
        assert_eq!(
            engine.focus(DisplayId(1), 2, &mut UnsupportedVisibility),
            Err(CommandError::VisibilityUnsupported)
        );
        assert_eq!(engine.active(DisplayId(1)), Some(1));
        assert_eq!(engine.active(DisplayId(2)), Some(3));
        engine.remove(id(1));
        assert_eq!(
            engine.focus(DisplayId(1), 2, &mut UnsupportedVisibility),
            Ok(None)
        );
    }
    #[test]
    fn moving_window_rolls_back_on_visibility_error() {
        let mut engine = WorkspaceEngine::<Layout>::new();
        engine.create(1, "one".into(), DisplayId(1)).unwrap();
        engine.create(2, "two".into(), DisplayId(1)).unwrap();
        engine.register(id(1), DisplayId(1), false).unwrap();
        assert_eq!(
            engine.move_window(id(1), 2, &mut UnsupportedVisibility),
            Err(CommandError::VisibilityUnsupported)
        );
        assert_eq!(engine.owner(id(1)), Some(1));
        assert!(engine.workspace(1).unwrap().layout.0.contains(&id(1)));
    }
}
