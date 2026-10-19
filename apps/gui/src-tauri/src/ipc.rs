use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use tylora_config::mutation::ConfigPatch;
use tylora_core::commands::StateCommand;

#[derive(Debug, Serialize)]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
}
impl BridgeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Status,
    Displays,
    Spaces,
    Workspaces,
    Windows,
    Hotkeys,
    Config,
    Capabilities,
    Diagnostics,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mutation {
    StateCommand {
        command: StateCommand,
    },
    PatchConfig {
        revision: String,
        patch: ConfigPatch,
    },
    Reconcile,
    RecoverWindows,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Start,
    Stop,
    Restart,
}

fn socket_path() -> PathBuf {
    std::env::temp_dir().join(format!("tylora-{}.sock", unsafe { libc::geteuid() }))
}

pub fn send(request: &str) -> Result<String, BridgeError> {
    send_to(&socket_path(), request)
}

fn send_to(path: &Path, request: &str) -> Result<String, BridgeError> {
    if request.len() > 64 * 1024 || request.contains('\n') {
        return Err(BridgeError::new(
            "invalid_request",
            "request exceeds protocol limit",
        ));
    }
    let meta = fs::symlink_metadata(path).map_err(|error| {
        BridgeError::new(
            "disconnected",
            format!("daemon socket unavailable: {error}"),
        )
    })?;
    if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
        return Err(BridgeError::new(
            "unsafe_socket",
            "daemon socket owner or type is unexpected",
        ));
    }
    let mut stream = UnixStream::connect(path)
        .map_err(|error| BridgeError::new("disconnected", error.to_string()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(18)))
        .map_err(|error| BridgeError::new("io", error.to_string()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| BridgeError::new("io", error.to_string()))?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|error| BridgeError::new("io", error.to_string()))?;
    let mut response = String::new();
    stream
        .take(1024 * 1024)
        .read_to_string(&mut response)
        .map_err(|error| BridgeError::new("timeout", error.to_string()))?;
    Ok(response.trim().to_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn preview_forwards_only_known_options() {
        let request = super::preview_request(
            4.0,
            6.0,
            Some(serde_json::json!({"layout":"scrolling","windows":4,"evil":"x","focus":null})),
        );
        assert_eq!(request["layout"], "scrolling");
        assert_eq!(request["windows"], 4);
        assert!(request.get("evil").is_none());
        assert!(request.get("focus").is_none());
        assert_eq!(request["inner"], 4.0);
    }
    use super::*;
    use std::{os::unix::net::UnixListener, thread};

    #[test]
    fn unavailable_socket_then_reconnects_to_protocol_peer() {
        let path = std::env::temp_dir().join(format!(
            "tylora-ipc-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(
            send_to(&path, "status --json").unwrap_err().code,
            "disconnected"
        );
        let listener = UnixListener::bind(&path).unwrap();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            use std::io::BufRead;
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "status --json\n");
            stream
                .write_all(b"{\"version\":1,\"state\":\"running\"}\n")
                .unwrap();
        });
        let reply = send_to(&path, "status --json").unwrap();
        assert!(reply.contains("running"));
        peer.join().unwrap();
        fs::remove_file(path).unwrap();
    }
}

fn json_reply(request: &str) -> Result<Value, BridgeError> {
    let response = send(request)?;
    let value: Value = serde_json::from_str(&response)
        .map_err(|_| BridgeError::new("protocol", "daemon returned an invalid JSON response"))?;
    if value.get("version").and_then(Value::as_u64) != Some(1) {
        return Err(BridgeError::new(
            "protocol",
            "daemon protocol version mismatch",
        ));
    }
    Ok(value)
}

pub fn query(kind: QueryKind) -> Result<Value, BridgeError> {
    let request = match kind {
        QueryKind::Status => "status --json",
        QueryKind::Displays => "display list --json",
        QueryKind::Spaces => "space list --json",
        QueryKind::Workspaces => "workspace list --json",
        QueryKind::Windows => "windows --json",
        QueryKind::Hotkeys => "hotkeys list --json",
        QueryKind::Config => "config get --json",
        QueryKind::Capabilities => "space capabilities --json",
        QueryKind::Diagnostics => "diagnostics --json",
    };
    let value = json_reply(request)?;
    if value.get("ok") == Some(&Value::Bool(false)) {
        return Err(envelope_error(&value));
    }
    Ok(value.get("data").cloned().unwrap_or(value))
}

fn envelope_error(value: &Value) -> BridgeError {
    let code = match value.pointer("/error/code").and_then(Value::as_str) {
        Some("config_conflict") => "config_conflict",
        Some("unsupported") => "unsupported",
        Some("config_validation_failed") => "validation",
        _ => "daemon_error",
    };
    BridgeError::new(
        code,
        value
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("daemon rejected request"),
    )
}

pub fn mutate(action: Mutation) -> Result<Value, BridgeError> {
    match action {
        Mutation::PatchConfig { revision, patch } => {
            let request = json!({"version":1,"revision":revision,"patch":patch});
            let value = json_reply(&format!("config patch {request}"))?;
            if value.get("ok") != Some(&Value::Bool(true)) {
                return Err(envelope_error(&value));
            }
            value
                .get("data")
                .cloned()
                .ok_or_else(|| BridgeError::new("protocol", "missing config snapshot"))
        }
        Mutation::StateCommand { command } => {
            let payload = serde_json::to_string(&command)
                .map_err(|error| BridgeError::new("invalid_request", error.to_string()))?;
            let value = send(&format!("v1 {payload}"))?;
            let result: Value = serde_json::from_str(&value)
                .map_err(|_| BridgeError::new("protocol", "invalid command response"))?;
            if result.get("ok") != Some(&Value::Bool(true)) {
                return Err(BridgeError::new(
                    "daemon_error",
                    result
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("command failed"),
                ));
            }
            Ok(result)
        }
        Mutation::Reconcile => {
            let reply = send("reconcile")?;
            if !reply.starts_with("reconciliation scheduled") {
                return Err(BridgeError::new("daemon_busy", reply));
            }
            Ok(json!({"message":reply}))
        }
        Mutation::RecoverWindows => {
            let reply = send("recover-windows")?;
            if let Ok(result) = serde_json::from_str::<Value>(&reply) {
                if result.get("ok") != Some(&Value::Bool(true)) {
                    return Err(BridgeError::new(
                        "daemon_error",
                        result
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("recovery failed"),
                    ));
                }
                Ok(result)
            } else if reply.starts_with("recovery attempted") {
                Ok(json!({"message":reply}))
            } else {
                Err(BridgeError::new("daemon_error", reply))
            }
        }
    }
}

fn daemon_binary() -> Result<PathBuf, BridgeError> {
    let current =
        std::env::current_exe().map_err(|error| BridgeError::new("io", error.to_string()))?;
    let path = current
        .parent()
        .ok_or_else(|| BridgeError::new("io", "GUI executable has no parent"))?
        .join("tylora");
    if path.is_file() {
        Ok(path)
    } else {
        Err(BridgeError::new(
            "daemon_binary_missing",
            "bundled tylora daemon binary was not found",
        ))
    }
}

fn start() -> Result<Value, BridgeError> {
    if query(QueryKind::Status).is_ok() {
        return Ok(json!({"state":"running"}));
    }
    let output = Command::new(daemon_binary()?)
        .arg("start")
        .output()
        .map_err(|error| BridgeError::new("launch_failed", error.to_string()))?;
    if !output.status.success() {
        return Err(BridgeError::new(
            "launch_failed",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    query(QueryKind::Status)
}

pub fn control(op: Control) -> Result<Value, BridgeError> {
    match op {
        Control::Start => start(),
        Control::Stop => {
            send("stop")?;
            Ok(json!({"state":"stopping"}))
        }
        Control::Restart => {
            if query(QueryKind::Status).is_ok() {
                send("stop")?;
                for _ in 0..30 {
                    if query(QueryKind::Status).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            start()
        }
    }
}

#[tauri::command]
pub async fn daemon_query(kind: QueryKind) -> Result<Value, BridgeError> {
    tauri::async_runtime::spawn_blocking(move || query(kind))
        .await
        .map_err(|error| BridgeError::new("internal", error.to_string()))?
}

#[tauri::command]
pub async fn daemon_mutate(action: Mutation) -> Result<Value, BridgeError> {
    tauri::async_runtime::spawn_blocking(move || mutate(action))
        .await
        .map_err(|error| BridgeError::new("internal", error.to_string()))?
}

#[tauri::command]
pub async fn daemon_control(op: Control) -> Result<Value, BridgeError> {
    tauri::async_runtime::spawn_blocking(move || control(op))
        .await
        .map_err(|error| BridgeError::new("internal", error.to_string()))?
}

/// Keys the GUI may forward to the daemon's engine-backed preview.
const PREVIEW_OPTIONS: [&str; 5] = ["layout", "dwindle", "scrolling", "windows", "focus"];

fn preview_request(inner: f64, outer: f64, options: Option<Value>) -> Value {
    let mut request = json!({"inner":inner,"outer":outer});
    if let (Some(Value::Object(options)), Some(target)) = (options, request.as_object_mut()) {
        for (key, value) in options {
            if PREVIEW_OPTIONS.contains(&key.as_str()) && !value.is_null() {
                target.insert(key, value);
            }
        }
    }
    request
}

#[tauri::command]
pub async fn layout_preview(
    inner: f64,
    outer: f64,
    options: Option<Value>,
) -> Result<Value, BridgeError> {
    if !inner.is_finite()
        || !outer.is_finite()
        || inner < 0.0
        || outer < 0.0
        || inner > 100.0
        || outer > 100.0
    {
        return Err(BridgeError::new(
            "validation",
            "gaps must be between 0 and 100",
        ));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = json_reply(&format!(
            "layout preview {}",
            preview_request(inner, outer, options)
        ))?;
        if let Some(message) = result.get("error").and_then(Value::as_str) {
            return Err(BridgeError::new("validation", message));
        }
        Ok(result)
    })
    .await
    .map_err(|error| BridgeError::new("internal", error.to_string()))?
}
