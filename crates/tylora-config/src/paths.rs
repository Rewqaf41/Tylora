//! The one place that resolves Tylora's configuration path, shared by the
//! daemon and the Settings app so both always read and write the same file.
//!
//! The former FluxWM location is only ever read: an existing
//! `~/.config/fluxwm/config.toml` is copied once into
//! `~/.config/tylora/config.toml` when the Tylora file does not exist yet.
//! The old file is never modified or deleted, an existing Tylora file is never
//! overwritten, and the two are never merged.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use crate::Config;

/// `$XDG_CONFIG_HOME`, else `~/.config`, else a temporary directory.
pub fn config_base() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| std::env::temp_dir().join("tylora-config"))
}

/// The Tylora configuration file (it may not exist yet).
pub fn config_file(base: &Path) -> PathBuf {
    base.join("tylora/config.toml")
}

/// The FluxWM configuration file that is migrated from (read only).
pub fn legacy_config_file(base: &Path) -> PathBuf {
    base.join("fluxwm/config.toml")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Migration {
    /// No legacy file, or the Tylora file already existed and the legacy one does not.
    NotNeeded,
    /// The legacy file was copied. For the configuration, `problem` holds the
    /// validation error of the copy; an invalid copy is kept so no setting is
    /// lost, and the daemon falls back to defaults until it is fixed.
    Copied {
        problem: Option<String>,
    },
    /// Both files exist; the Tylora file wins and nothing is merged.
    BothExist,
    Failed(String),
}

/// Copies `old_path` to `new_path` when only the old one exists. The copy is
/// written to a private temporary file and published with a hard link, which
/// fails instead of replacing a file created concurrently.
pub fn migrate_legacy_file(new_path: &Path, old_path: &Path) -> Migration {
    if !old_path.is_file() {
        return Migration::NotNeeded;
    }
    if new_path.exists() {
        return Migration::BothExist;
    }
    let result = (|| -> io::Result<()> {
        let content = fs::read(old_path)?;
        let parent = new_path
            .parent()
            .ok_or_else(|| io::Error::other("missing parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = new_path.with_extension(format!("migration-{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&content)?;
        file.sync_all()?;
        let linked = fs::hard_link(&temporary, new_path);
        let _ = fs::remove_file(&temporary);
        match linked {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "created concurrently",
            )),
            Err(error) => Err(error),
        }
    })();
    match result {
        Ok(_) => Migration::Copied { problem: None },
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Migration::BothExist,
        Err(error) => Migration::Failed(error.to_string()),
    }
}

/// Migrates the FluxWM configuration and validates the copy.
pub fn migrate_legacy_config(new_path: &Path, old_path: &Path) -> Migration {
    match migrate_legacy_file(new_path, old_path) {
        Migration::Copied { .. } => Migration::Copied {
            problem: fs::read_to_string(new_path)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    Config::parse(&source)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
                .err(),
        },
        other => other,
    }
}

/// Resolves the configuration file, migrating the FluxWM file on first use.
pub fn resolve_config() -> (PathBuf, Migration) {
    let base = config_base();
    let path = config_file(&base);
    let migration = migrate_legacy_config(&path, &legacy_config_file(&base));
    (path, migration)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tylora-paths-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn copies_once_keeps_old_and_never_overwrites() {
        let base = root("copy");
        let old = legacy_config_file(&base);
        let new = config_file(&base);
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        let source = "version = 1\ndefault_layout = \"scrolling\"\n# keep me\n";
        fs::write(&old, source).unwrap();
        assert_eq!(
            migrate_legacy_config(&new, &old),
            Migration::Copied { problem: None }
        );
        assert_eq!(fs::read_to_string(&new).unwrap(), source);
        assert_eq!(fs::read_to_string(&old).unwrap(), source);
        fs::write(&new, "version = 1\n# newer\n").unwrap();
        assert_eq!(migrate_legacy_config(&new, &old), Migration::BothExist);
        assert!(fs::read_to_string(&new).unwrap().contains("newer"));
        assert_eq!(fs::read_to_string(&old).unwrap(), source);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn invalid_legacy_config_is_copied_and_reported() {
        let base = root("invalid");
        let old = legacy_config_file(&base);
        let new = config_file(&base);
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "version = 1\ndefault_layout = \"spiral\"\n").unwrap();
        let outcome = migrate_legacy_config(&new, &old);
        assert!(matches!(outcome, Migration::Copied { problem: Some(_) }));
        assert!(new.is_file());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn nothing_to_migrate() {
        let base = root("none");
        assert_eq!(
            migrate_legacy_file(&config_file(&base), &legacy_config_file(&base)),
            Migration::NotNeeded
        );
        assert!(!config_file(&base).exists());
        fs::remove_dir_all(base).unwrap();
    }
}
