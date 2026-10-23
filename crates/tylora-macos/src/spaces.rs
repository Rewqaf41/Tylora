//! Experimental native macOS Spaces adapter. All SkyLight symbols are private
//! and are resolved at runtime; no operation may assume that they exist.

use serde::Serialize;
use std::collections::HashMap;
use tylora_core::DisplayId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct NativeSpaceId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DesktopNumber(pub u8);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeSpace {
    pub id: NativeSpaceId,
    pub desktop: DesktopNumber,
    pub display: DisplayId,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpaceCapabilities {
    pub macos_version: String,
    pub architecture: String,
    pub sip_enabled: Option<bool>,
    pub switching_backend: &'static str,
    pub can_list_native_spaces: bool,
    pub can_observe_active_space: bool,
    pub can_switch_native_space: bool,
    pub can_move_window_between_spaces: bool,
    pub can_determine_window_space: bool,
    pub private_backend_available: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceError {
    Unsupported(String),
    NoDesktop {
        display: DisplayId,
        desktop: u8,
    },
    NoWindowId,
    QueryFailed(&'static str),
    VerificationFailed,
    SwitchBusy,
    SwitchCancelled,
    SwitchNotOnPointerDisplay,
    SwitchTimeout,
    MoveBusy,
    MoveTimeout {
        window_id: u32,
        target: NativeSpaceId,
    },
}

impl std::fmt::Display for SpaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) => write!(f, "native Spaces operation unavailable: {reason}"),
            Self::NoDesktop { display, desktop } => {
                write!(
                    f,
                    "Desktop {desktop} does not exist on display {}",
                    display.0
                )
            }
            Self::NoWindowId => write!(f, "the focused AX window has no verified CGWindowID"),
            Self::QueryFailed(operation) => write!(f, "native Spaces query failed: {operation}"),
            Self::VerificationFailed => {
                write!(f, "macOS did not confirm the requested Space change")
            }
            Self::SwitchBusy => write!(f, "another native Space switch is still in progress"),
            Self::SwitchCancelled => write!(f, "native Space switch superseded by a newer request"),
            Self::SwitchNotOnPointerDisplay => write!(f, "target Space is on another display; move the pointer to that display before switching"),
            Self::SwitchTimeout => write!(f, "gesture posted, but macOS did not complete the native Space switch before timeout"),
            Self::MoveBusy => write!(f, "a native move is already in progress for this window"),
            Self::MoveTimeout { window_id, target } => write!(f, "window {window_id} was submitted for Space {}, but membership was not confirmed before timeout; reconcile before retrying", target.0),
        }
    }
}
impl std::error::Error for SpaceError {}

pub trait NativeSpaceBackend {
    fn capabilities(&self) -> SpaceCapabilities;
    fn list_spaces(&self) -> Result<Vec<NativeSpace>, SpaceError>;
    fn get_active_space(&self, display: DisplayId) -> Result<NativeSpace, SpaceError>;
    fn get_window_spaces(&self, window_id: u32) -> Result<Vec<NativeSpaceId>, SpaceError>;
    fn focus_space(
        &self,
        display: DisplayId,
        desktop: DesktopNumber,
    ) -> Result<NativeSpace, SpaceError>;
    fn move_window_to_space(&self, window_id: u32, space: NativeSpaceId) -> Result<(), SpaceError>;
}

pub struct SpaceRegistry;
impl SpaceRegistry {
    pub fn desktop(
        spaces: &[NativeSpace],
        display: DisplayId,
        desktop: DesktopNumber,
    ) -> Result<&NativeSpace, SpaceError> {
        spaces
            .iter()
            .find(|space| space.display == display && space.desktop == desktop)
            .ok_or(SpaceError::NoDesktop {
                display,
                desktop: desktop.0,
            })
    }

    /// Returns success only after WindowServer confirms target membership.
    /// Callers may update their BSP ownership after this returns.
    pub fn move_verified<B: NativeSpaceBackend>(
        backend: &B,
        window_id: u32,
        display: DisplayId,
        desktop: DesktopNumber,
    ) -> Result<NativeSpaceId, SpaceError> {
        if window_id == 0 {
            return Err(SpaceError::NoWindowId);
        }
        let spaces = backend.list_spaces()?;
        let target = Self::desktop(&spaces, display, desktop)?.id;
        if backend.get_window_spaces(window_id)?.contains(&target) {
            return Ok(target);
        }
        backend.move_window_to_space(window_id, target)?;
        if backend.get_window_spaces(window_id)?.contains(&target) {
            Ok(target)
        } else {
            Err(SpaceError::VerificationFailed)
        }
    }
}

/// Every Space of one display in Mission Control order, including native
/// fullscreen Spaces, which `list_spaces` hides because they are not Desktops.
/// Dock swipes step through this order, so switching must walk it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplaySpaceOrder {
    pub display: DisplayId,
    /// The display's stable UUID; WindowServer drops a display from this
    /// list the moment it disconnects, well before AppKit reports it.
    pub uuid: Option<u128>,
    pub active: NativeSpaceId,
    pub spaces: Vec<OrderedSpace>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct OrderedSpace {
    pub id: NativeSpaceId,
    pub fullscreen: bool,
}

/// Fullscreen Space -> the Desktop it was entered from. macOS may place a
/// new fullscreen Space anywhere in the strip (often at its end), so the
/// link is learned from the Space transition, never from position.
pub type FullscreenLinks = HashMap<NativeSpaceId, NativeSpaceId>;

impl DisplaySpaceOrder {
    pub fn ids(&self) -> Vec<NativeSpaceId> {
        self.spaces.iter().map(|space| space.id).collect()
    }

    fn contains(&self, id: NativeSpaceId, fullscreen: bool) -> bool {
        self.spaces
            .iter()
            .any(|space| space.id == id && space.fullscreen == fullscreen)
    }

    /// The Desktop a Space belongs to: a Desktop owns itself, a fullscreen
    /// Space belongs to its linked Desktop while that still exists here.
    pub fn owner(&self, id: NativeSpaceId, links: &FullscreenLinks) -> Option<NativeSpaceId> {
        if self.contains(id, false) {
            return Some(id);
        }
        links
            .get(&id)
            .copied()
            .filter(|desktop| self.contains(id, true) && self.contains(*desktop, false))
    }

    /// Fullscreen Spaces linked to `desktop`, in Mission Control order.
    pub fn fullscreen_of(
        &self,
        desktop: NativeSpaceId,
        links: &FullscreenLinks,
    ) -> Vec<NativeSpaceId> {
        self.spaces
            .iter()
            .filter(|space| space.fullscreen && links.get(&space.id) == Some(&desktop))
            .map(|space| space.id)
            .collect()
    }

    /// Where a workspace hotkey for `desktop` should go: into the Desktop's
    /// fullscreen Spaces first, cycling through them on repeated presses, and
    /// back to the Desktop itself after the last one. Pressing again only
    /// counts as "from inside" on the display the user is on (`focused`); a
    /// fullscreen Space showing on another display is simply focused.
    pub fn workspace_target(
        &self,
        desktop: NativeSpaceId,
        links: &FullscreenLinks,
        focused: bool,
    ) -> NativeSpaceId {
        let fullscreen = self.fullscreen_of(desktop, links);
        match fullscreen.iter().position(|id| *id == self.active) {
            Some(index) if focused => fullscreen.get(index + 1).copied().unwrap_or(desktop),
            Some(_) => self.active,
            None => fullscreen.first().copied().unwrap_or(desktop),
        }
    }
}

/// Learns links from one observed change of the Space strips: a fullscreen
/// Space that did not exist before and is now active on a display whose
/// previous active Space was a Desktop was entered from that Desktop. Links
/// of fullscreen Spaces that no longer exist are dropped. Returns whether
/// `links` changed.
pub fn learn_fullscreen_links(
    previous: &[DisplaySpaceOrder],
    current: &[DisplaySpaceOrder],
    links: &mut FullscreenLinks,
) -> bool {
    let before = links.len();
    links.retain(|fullscreen, _| {
        current
            .iter()
            .any(|order| order.contains(*fullscreen, true))
    });
    let mut changed = links.len() != before;
    for order in current {
        let Some(old) = previous.iter().find(|old| old.display == order.display) else {
            continue;
        };
        let entered = order.active;
        if order.contains(entered, true)
            && !old.ids().contains(&entered)
            && old.contains(old.active, false)
            && !links.contains_key(&entered)
        {
            links.insert(entered, old.active);
            changed = true;
        }
    }
    changed
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod window_mover;

#[cfg(target_os = "macos")]
pub use macos::{
    cursor_display, focus_window, warp_cursor_to, warp_cursor_to_display,
    ExperimentalSkyLightBackend, NativeSpaceSwitcher, SpaceObserver, SwitchOutcome,
};

#[cfg(target_os = "macos")]
pub use window_mover::{
    bridge_status, focused_cg_window, last_move_diagnostic, record_move_diagnostic,
    set_window_frame, set_window_fullscreen, FocusedCgWindow, MoveOutcome, NativeWindowSpaceMover,
};

#[cfg(target_os = "macos")]
pub(crate) use window_mover::ax_cg_window;

#[cfg(target_os = "macos")]
pub(crate) use macos::window_on_active_space;

#[cfg(test)]
mod tests {
    use super::*;

    fn order(active: u64, spaces: &[(u64, bool)]) -> DisplaySpaceOrder {
        DisplaySpaceOrder {
            display: DisplayId(1),
            uuid: None,
            active: NativeSpaceId(active),
            spaces: spaces
                .iter()
                .map(|(id, fullscreen)| OrderedSpace {
                    id: NativeSpaceId(*id),
                    fullscreen: *fullscreen,
                })
                .collect(),
        }
    }

    #[test]
    fn fullscreen_link_is_learned_from_the_desktop_it_was_entered_from() {
        // macOS appended the video's fullscreen Space after Desktop 6.
        let before = order(4, &[(2, false), (4, false), (6, false)]);
        let after = order(90, &[(2, false), (4, false), (6, false), (90, true)]);
        let mut links = FullscreenLinks::new();
        assert!(learn_fullscreen_links(
            &[before],
            std::slice::from_ref(&after),
            &mut links
        ));
        assert_eq!(links.get(&NativeSpaceId(90)), Some(&NativeSpaceId(4)));
        assert_eq!(
            after.owner(NativeSpaceId(90), &links),
            Some(NativeSpaceId(4))
        );
        assert_eq!(
            after.owner(NativeSpaceId(6), &links),
            Some(NativeSpaceId(6))
        );
        // Switching to an existing fullscreen Space teaches nothing new.
        let elsewhere = order(
            2,
            &after
                .spaces
                .iter()
                .map(|s| (s.id.0, s.fullscreen))
                .collect::<Vec<_>>(),
        );
        let mut fresh = FullscreenLinks::new();
        assert!(!learn_fullscreen_links(
            &[elsewhere],
            std::slice::from_ref(&after),
            &mut fresh
        ));
        assert!(fresh.is_empty());
        // Leaving fullscreen removes the Space and its link.
        let closed = order(4, &[(2, false), (4, false), (6, false)]);
        assert!(learn_fullscreen_links(&[after], &[closed], &mut links));
        assert!(links.is_empty());
    }

    #[test]
    fn workspace_hotkey_enters_fullscreen_then_returns_to_its_desktop() {
        let spaces = [(4, false), (5, false), (6, false), (90, true), (91, true)];
        let links = FullscreenLinks::from([
            (NativeSpaceId(90), NativeSpaceId(4)),
            (NativeSpaceId(91), NativeSpaceId(4)),
        ]);
        let target = |active, desktop| {
            order(active, &spaces).workspace_target(NativeSpaceId(desktop), &links, true)
        };
        // From another Desktop, and from the owner Desktop itself: the video.
        assert_eq!(target(5, 4), NativeSpaceId(90));
        assert_eq!(target(4, 4), NativeSpaceId(90));
        // Several fullscreen Spaces are visited in order, then the Desktop.
        assert_eq!(target(90, 4), NativeSpaceId(91));
        assert_eq!(target(91, 4), NativeSpaceId(4));
        // Leaving the video for another workspace is a plain Desktop switch,
        // and the Desktop the video sits next to does not own it.
        assert_eq!(target(90, 5), NativeSpaceId(5));
        assert_eq!(target(5, 6), NativeSpaceId(6));
        // On another display the video showing there is focused, not left.
        assert_eq!(
            order(90, &spaces).workspace_target(NativeSpaceId(4), &links, false),
            NativeSpaceId(90)
        );
        assert_eq!(
            order(5, &spaces).workspace_target(NativeSpaceId(4), &links, false),
            NativeSpaceId(90)
        );
        // A fullscreen Space without a learned link belongs to no workspace.
        assert_eq!(
            order(91, &spaces).owner(NativeSpaceId(91), &FullscreenLinks::new()),
            None
        );
    }
    use std::cell::RefCell;

    struct MockBackend {
        spaces: Vec<NativeSpace>,
        membership: RefCell<Vec<NativeSpaceId>>,
        accept_move: bool,
        calls: RefCell<u32>,
    }
    impl NativeSpaceBackend for MockBackend {
        fn capabilities(&self) -> SpaceCapabilities {
            unreachable!()
        }
        fn list_spaces(&self) -> Result<Vec<NativeSpace>, SpaceError> {
            Ok(self.spaces.clone())
        }
        fn get_active_space(&self, display: DisplayId) -> Result<NativeSpace, SpaceError> {
            self.spaces
                .iter()
                .find(|space| space.display == display && space.active)
                .cloned()
                .ok_or(SpaceError::QueryFailed("active"))
        }
        fn get_window_spaces(&self, _: u32) -> Result<Vec<NativeSpaceId>, SpaceError> {
            Ok(self.membership.borrow().clone())
        }
        fn focus_space(&self, _: DisplayId, _: DesktopNumber) -> Result<NativeSpace, SpaceError> {
            Err(SpaceError::Unsupported("mock".into()))
        }
        fn move_window_to_space(&self, _: u32, space: NativeSpaceId) -> Result<(), SpaceError> {
            *self.calls.borrow_mut() += 1;
            if self.accept_move {
                *self.membership.borrow_mut() = vec![space];
            }
            Ok(())
        }
    }
    fn mock(accept_move: bool) -> MockBackend {
        MockBackend {
            spaces: vec![
                NativeSpace {
                    id: NativeSpaceId(10),
                    desktop: DesktopNumber(1),
                    display: DisplayId(1),
                    active: true,
                },
                NativeSpace {
                    id: NativeSpaceId(42),
                    desktop: DesktopNumber(2),
                    display: DisplayId(1),
                    active: false,
                },
            ],
            membership: RefCell::new(vec![NativeSpaceId(10)]),
            accept_move,
            calls: RefCell::new(0),
        }
    }
    #[test]
    fn verified_move_is_idempotent_and_requires_native_membership() {
        let backend = mock(true);
        assert_eq!(
            SpaceRegistry::move_verified(&backend, 123, DisplayId(1), DesktopNumber(2)),
            Ok(NativeSpaceId(42))
        );
        assert_eq!(
            SpaceRegistry::move_verified(&backend, 123, DisplayId(1), DesktopNumber(2)),
            Ok(NativeSpaceId(42))
        );
        assert_eq!(*backend.calls.borrow(), 1);
    }
    #[test]
    fn failed_move_does_not_claim_success() {
        let backend = mock(false);
        assert_eq!(
            SpaceRegistry::move_verified(&backend, 123, DisplayId(1), DesktopNumber(2)),
            Err(SpaceError::VerificationFailed)
        );
        assert_eq!(*backend.membership.borrow(), vec![NativeSpaceId(10)]);
        assert_eq!(
            SpaceRegistry::move_verified(&backend, 0, DisplayId(1), DesktopNumber(2)),
            Err(SpaceError::NoWindowId)
        );
        assert!(matches!(
            SpaceRegistry::move_verified(&backend, 123, DisplayId(1), DesktopNumber(3)),
            Err(SpaceError::NoDesktop { .. })
        ));
    }
    #[test]
    fn desktop_number_is_not_a_space_id_or_global_across_displays() {
        let spaces = vec![
            NativeSpace {
                id: NativeSpaceId(1005),
                desktop: DesktopNumber(1),
                display: DisplayId(1),
                active: true,
            },
            NativeSpace {
                id: NativeSpaceId(8012),
                desktop: DesktopNumber(2),
                display: DisplayId(1),
                active: false,
            },
            NativeSpace {
                id: NativeSpaceId(7001),
                desktop: DesktopNumber(1),
                display: DisplayId(2),
                active: true,
            },
        ];
        assert_eq!(
            SpaceRegistry::desktop(&spaces, DisplayId(1), DesktopNumber(2))
                .unwrap()
                .id,
            NativeSpaceId(8012)
        );
        assert_eq!(
            SpaceRegistry::desktop(&spaces, DisplayId(2), DesktopNumber(1))
                .unwrap()
                .id,
            NativeSpaceId(7001)
        );
        assert!(matches!(
            SpaceRegistry::desktop(&spaces, DisplayId(2), DesktopNumber(2)),
            Err(SpaceError::NoDesktop { .. })
        ));
    }
}
