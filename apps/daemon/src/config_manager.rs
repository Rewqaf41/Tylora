use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use tylora_config::{
    mutation::{apply_patch, revision, ConfigPatch},
    Config,
};

#[derive(Debug, Serialize)]
pub struct ConfigSnapshot {
    pub version: u32,
    pub revision: String,
    pub config: Config,
}

#[derive(Debug, Deserialize)]
pub struct PatchRequest {
    pub version: u32,
    pub revision: String,
    pub patch: ConfigPatch,
}

fn source(path: &Path) -> Result<String, String> {
    match fs::read_to_string(path) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            toml::to_string_pretty(&Config::default()).map_err(|error| error.to_string())
        }
        Err(error) => Err(error.to_string()),
    }
}

pub fn read(path: &Path) -> Result<ConfigSnapshot, String> {
    let source = source(path)?;
    let config = Config::parse(&source).map_err(|error| error.to_string())?;
    Ok(ConfigSnapshot {
        version: 1,
        revision: revision(&source),
        config,
    })
}

pub fn patch(path: &Path, request: PatchRequest) -> Result<ConfigSnapshot, String> {
    if request.version != 1 {
        return Err("unsupported config protocol version".into());
    }
    let old = source(path)?;
    if revision(&old) != request.revision {
        return Err("config changed since it was read; refresh and retry".into());
    }
    let (updated, config) = apply_patch(&old, request.patch).map_err(|error| error.to_string())?;
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    let result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|error| error.to_string())?;
        if let Ok(meta) = fs::metadata(path) {
            file.set_permissions(fs::Permissions::from_mode(
                meta.permissions().mode() & 0o777,
            ))
            .map_err(|error| error.to_string())?;
        }
        file.write_all(updated.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        if source(path)? != old {
            return Err("config changed during update; refresh and retry".into());
        }
        fs::rename(&temp, path).map_err(|error| error.to_string())?;
        if let Ok(dir) = OpenOptions::new().read(true).open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result?;
    Ok(ConfigSnapshot {
        version: 1,
        revision: revision(&updated),
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_conflict_prevents_lost_update() {
        let root = std::env::temp_dir().join(format!(
            "tylora-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("config.toml");
        fs::write(
            &path,
            "# keep me\nversion = 1\n[gaps]\ninner = 8\nouter = 8\n",
        )
        .unwrap();
        let first = read(&path).unwrap();
        let request = PatchRequest {
            version: 1,
            revision: first.revision.clone(),
            patch: ConfigPatch::SetGaps {
                inner: 12.0,
                outer: 4.0,
            },
        };
        let updated = patch(&path, request).unwrap();
        assert_eq!(updated.config.gaps.inner, 12.0);
        assert!(fs::read_to_string(&path).unwrap().contains("# keep me"));
        let stale = PatchRequest {
            version: 1,
            revision: first.revision,
            patch: ConfigPatch::SetGaps {
                inner: 3.0,
                outer: 3.0,
            },
        };
        assert!(patch(&path, stale).unwrap_err().contains("changed"));
        assert_eq!(read(&path).unwrap().config.gaps.inner, 12.0);
        fs::remove_dir_all(root).unwrap();
    }
}
