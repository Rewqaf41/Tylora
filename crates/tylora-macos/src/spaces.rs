//! Experimental native macOS Spaces adapter. All SkyLight symbols are private
//! and are resolved at runtime; no operation may assume that they exist.

use serde::Serialize;
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

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
mod window_mover;

#[cfg(target_os = "macos")]
pub use macos::{
    cursor_display, warp_cursor_to_display, ExperimentalSkyLightBackend, NativeSpaceSwitcher,
    SpaceObserver, SwitchOutcome,
};

#[cfg(target_os = "macos")]
pub use window_mover::{
    bridge_status, focused_cg_window, last_move_diagnostic, record_move_diagnostic,
    FocusedCgWindow, MoveOutcome, NativeWindowSpaceMover,
};

#[cfg(target_os = "macos")]
pub(crate) use window_mover::ax_cg_window;

#[cfg(test)]
mod tests {
    use super::*;
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
