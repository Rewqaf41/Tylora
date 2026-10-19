use super::{ExperimentalSkyLightBackend, NativeSpaceBackend, NativeSpaceId, SpaceError};
use serde::Serialize;
use std::ffi::c_void;
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

#[link(name = "tylora_window_move_bridge", kind = "static")]
unsafe extern "C" {
    fn tylora_move_bridge_probe() -> i32;
    fn tylora_focused_cg_window(wid: *mut u32, pid: *mut i32) -> i32;
    fn tylora_app_focused_cg_window(pid: i32, wid: *mut u32) -> i32;
    fn tylora_submit_window_move(wid: u32, target: u64) -> i32;
    fn tylora_ax_cg_window(element: *const c_void, wid: *mut u32) -> i32;
}

const BACKEND: &str = "SkyLight bridged move";
unsafe extern "C" {
    fn geteuid() -> u32;
    fn flock(fd: i32, operation: i32) -> i32;
}
static DIAGNOSTIC_SERIAL: AtomicU64 = AtomicU64::new(0);

fn diagnostic_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("tylora-space-move-{}.json", unsafe { geteuid() }))
}
pub fn last_move_diagnostic() -> Option<serde_json::Value> {
    serde_json::from_slice(&fs::read(diagnostic_path()).ok()?).ok()
}
pub fn record_move_diagnostic(value: &serde_json::Value) {
    let path = diagnostic_path();
    let serial = DIAGNOSTIC_SERIAL.fetch_add(1, Ordering::Relaxed);
    let temp = path.with_extension(format!("{}-{serial}.tmp", std::process::id()));
    let Ok(bytes) = serde_json::to_vec(value) else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
    {
        use std::io::Write;
        if file.write_all(&bytes).is_ok() {
            let _ = fs::rename(&temp, &path);
        }
    }
    let _ = fs::remove_file(temp);
}

pub fn bridge_status() -> Result<(), SpaceError> {
    let code = unsafe { tylora_move_bridge_probe() };
    match code {
        0 => Ok(()),
        1 => Err(SpaceError::Unsupported(
            "SkyLight framework could not be loaded".into(),
        )),
        2 => Err(SpaceError::Unsupported(
            "static bridged move symbol is absent from SkyLight Mach-O symbol table".into(),
        )),
        3 => Err(SpaceError::Unsupported(
            "bridged move Objective-C class or initializer is absent".into(),
        )),
        4 => Err(SpaceError::Unsupported(
            "_AXUIElementGetWindow is absent".into(),
        )),
        _ => Err(SpaceError::Unsupported(format!(
            "bridge probe failed ({code})"
        ))),
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct FocusedCgWindow {
    pub window_id: u32,
    pub pid: i32,
}

pub fn focused_cg_window() -> Result<FocusedCgWindow, SpaceError> {
    let (mut window_id, mut pid) = (0, 0);
    // The system-wide AX element can stay broken after the focused app quits;
    // resolve the frontmost process independently, then ask that app.
    let code = match crate::frontmost_application_pid() {
        Some(front) => {
            pid = front;
            unsafe { tylora_app_focused_cg_window(front, &mut window_id) }
        }
        None => unsafe { tylora_focused_cg_window(&mut window_id, &mut pid) },
    };
    if code != 0 || window_id == 0 || pid <= 0 {
        return Err(SpaceError::Unsupported(format!(
            "focused AX window has no verified CGWindowID (AX error {code})"
        )));
    }
    Ok(FocusedCgWindow { window_id, pid })
}

pub(crate) fn ax_cg_window(element: *const c_void) -> Option<u32> {
    let mut wid = 0;
    (unsafe { tylora_ax_cg_window(element, &mut wid) } == 0 && wid != 0).then_some(wid)
}

#[derive(Debug, Clone, Serialize)]
pub struct MoveOutcome {
    pub window_id: u32,
    pub source: NativeSpaceId,
    pub target: NativeSpaceId,
    pub backend: &'static str,
    pub stage: &'static str,
    pub operation_ms: u128,
    pub verification_ms: u128,
    pub confirmed: bool,
    pub no_op: bool,
}

static IN_FLIGHT: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
struct Flight {
    id: u32,
    _file: File,
}
impl Flight {
    fn acquire(id: u32) -> Result<Self, SpaceError> {
        let mut set = IN_FLIGHT
            .get_or_init(|| Mutex::new(HashSet::new()))
            .lock()
            .map_err(|_| SpaceError::MoveBusy)?;
        if !set.insert(id) {
            return Err(SpaceError::MoveBusy);
        }
        let path = std::env::temp_dir().join(format!("tylora-space-move-{}-{id}.lock", unsafe {
            geteuid()
        }));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|_| SpaceError::QueryFailed("window move lock file"));
        match file {
            Ok(file) if unsafe { flock(file.as_raw_fd(), 2 | 4) } == 0 => {
                Ok(Self { id, _file: file })
            }
            Ok(_) => {
                set.remove(&id);
                Err(SpaceError::MoveBusy)
            }
            Err(error) => {
                set.remove(&id);
                Err(error)
            }
        }
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        if let Ok(mut set) = IN_FLIGHT.get_or_init(|| Mutex::new(HashSet::new())).lock() {
            set.remove(&self.id);
        }
    }
}

pub struct NativeWindowSpaceMover;
impl NativeWindowSpaceMover {
    pub fn can_move_window(window_id: u32) -> Result<NativeSpaceId, SpaceError> {
        if window_id == 0 {
            return Err(SpaceError::NoWindowId);
        }
        bridge_status()?;
        let backend = ExperimentalSkyLightBackend::new();
        let membership = backend.get_window_spaces(window_id)?;
        if membership.len() != 1 {
            return Err(SpaceError::Unsupported(format!(
                "window {window_id} has {} native Space memberships; exactly one is required",
                membership.len()
            )));
        }
        let source = membership[0];
        if !backend
            .list_spaces()?
            .iter()
            .any(|space| space.id == source)
        {
            return Err(SpaceError::Unsupported(format!(
                "source Space {} is not a regular Desktop",
                source.0
            )));
        }
        Ok(source)
    }

    pub fn verify_window_space(window_id: u32, target: NativeSpaceId) -> Result<bool, SpaceError> {
        let membership = ExperimentalSkyLightBackend::new().get_window_spaces(window_id)?;
        Ok(membership.len() == 1 && membership[0] == target)
    }

    /// Invoke only on a worker thread: bounded verification can wait three seconds.
    /// A timeout is indeterminate; the operation may complete later.
    pub fn move_window_to_space(
        window_id: u32,
        target: NativeSpaceId,
    ) -> Result<MoveOutcome, SpaceError> {
        let _flight = Flight::acquire(window_id)?;
        let started = Instant::now();
        let source = Self::can_move_window(window_id)?;
        let backend = ExperimentalSkyLightBackend::new();
        if !backend
            .list_spaces()?
            .iter()
            .any(|space| space.id == target)
        {
            return Err(SpaceError::Unsupported(format!(
                "target Space {} no longer exists",
                target.0
            )));
        }
        if source == target {
            return Ok(MoveOutcome {
                window_id,
                source,
                target,
                backend: BACKEND,
                stage: "noop",
                operation_ms: 0,
                verification_ms: 0,
                confirmed: true,
                no_op: true,
            });
        }
        let code = unsafe { tylora_submit_window_move(window_id, target.0) };
        let operation_ms = started.elapsed().as_millis();
        if code != 0 {
            return Err(SpaceError::Unsupported(format!(
                "native move bridge rejected operation ({code})"
            )));
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match Self::verify_window_space(window_id, target) {
                Ok(true) => {
                    return Ok(MoveOutcome {
                        window_id,
                        source,
                        target,
                        backend: BACKEND,
                        stage: "confirmed",
                        operation_ms,
                        verification_ms: started.elapsed().as_millis().saturating_sub(operation_ms),
                        confirmed: true,
                        no_op: false,
                    })
                }
                Ok(false) | Err(SpaceError::QueryFailed(_)) => {}
                Err(error) => return Err(error),
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(SpaceError::MoveTimeout { window_id, target })
    }
}
