use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::Snapshot;

pub fn snapshot_path(server_id: &str) -> PathBuf {
    runtime_directory().join(format!("{server_id}.json"))
}

pub fn write_snapshot(snapshot: &Snapshot) -> Result<PathBuf> {
    let path = snapshot_path(&snapshot.tmux_server);
    write_snapshot_to(snapshot, &path)?;
    Ok(path)
}

pub fn read_snapshot(server_id: &str) -> Result<Snapshot> {
    let path = snapshot_path(server_id);
    let bytes = fs::read(&path).with_context(|| {
        format!("failed to read snapshot {}", path.display())
    })?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse snapshot {}", path.display()))
}

pub fn runtime_directory() -> PathBuf {
    if let Some(directory) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(directory).join("tmux-agent-watch");
    }

    let user = std::env::var("USER").unwrap_or_else(|_| "unknown".to_owned());
    PathBuf::from(format!(
        "/tmp/tmux-agent-watch-{}",
        sanitize_component(&user)
    ))
}

pub fn prepare_runtime_directory() -> Result<PathBuf> {
    let directory = runtime_directory();
    prepare_directory(&directory)?;
    Ok(directory)
}

fn write_snapshot_to(snapshot: &Snapshot, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .context("snapshot path does not have a parent directory")?;
    prepare_directory(parent)?;

    let temporary =
        path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    serde_json::to_writer_pretty(&mut file, snapshot).with_context(|| {
        format!("failed to serialize {}", temporary.display())
    })?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| {
        format!("failed to atomically replace {}", path.display())
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect {}", path.display()))?;
    Ok(())
}

fn prepare_directory(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory).with_context(|| {
        format!("failed to create runtime directory {}", directory.display())
    })?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect {}", directory.display()))
}

fn sanitize_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || matches!(character, '-' | '_')
            {
                character
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Snapshot;

    #[test]
    fn writes_private_atomic_snapshot_without_terminal_content() {
        let directory = std::env::temp_dir()
            .join(format!("tmux-agent-watch-test-{}", std::process::id()));
        let path = directory.join("test.json");
        let snapshot = Snapshot {
            schema_version: 1,
            observed_at_ms: 123,
            tmux_server: "test".to_owned(),
            panes: Vec::new(),
        };

        write_snapshot_to(&snapshot, &path).expect("snapshot should write");
        let bytes = fs::read(&path).expect("snapshot should exist");
        let decoded: Snapshot =
            serde_json::from_slice(&bytes).expect("snapshot should parse");
        assert_eq!(decoded, snapshot);
        assert_eq!(
            fs::metadata(&path)
                .expect("metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let text = String::from_utf8(bytes).expect("snapshot should be UTF-8");
        assert!(!text.contains("capture"));
        assert!(!text.contains("prompt"));

        fs::remove_dir_all(directory).expect("test directory should be removed");
    }
}
