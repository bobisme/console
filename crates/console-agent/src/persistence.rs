//! Explicit native persistence adapter.
//!
//! Headless sessions are ephemeral unless a caller selects this adapter with
//! `--save-file`; playtests and RPC never consume ambient files.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use console_core::MAX_SAVE_BYTES;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub fn read_sidecar(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read(path) {
        Ok(bytes) => {
            if bytes.len() > MAX_SAVE_BYTES {
                return Err(format!(
                    "save sidecar {} is {} bytes; maximum is {MAX_SAVE_BYTES}",
                    path.display(),
                    bytes.len()
                ));
            }
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| format!("save sidecar {} is not UTF-8: {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("reading save sidecar {}: {error}", path.display())),
    }
}

pub fn commit_sidecar(path: &Path, document: Option<&str>) -> Result<(), String> {
    if let Some(document) = document
        && document.len() > MAX_SAVE_BYTES
    {
        return Err(format!(
            "save sidecar document is {} bytes; maximum is {MAX_SAVE_BYTES}",
            document.len()
        ));
    }
    if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(format!(
            "refusing to replace symbolic-link save sidecar {}",
            path.display()
        ));
    }
    let Some(document) = document else {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("clearing save sidecar {}: {error}", path.display())),
        };
    };
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty());
    if let Some(parent) = parent {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating save directory {}: {error}", parent.display()))?;
    }
    let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("save.json");
    let temp = path.with_file_name(format!(
        ".{name}.console-tmp-{}-{sequence}",
        std::process::id()
    ));
    write_new(&temp, document.as_bytes())?;
    if let Err(error) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "committing save sidecar {} from {}: {error}",
            path.display(),
            temp.display()
        ));
    }
    Ok(())
}

fn write_new(path: &PathBuf, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("creating temporary save {}: {error}", path.display()))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "writing temporary save {}: {error}",
            path.display()
        ));
    }
    Ok(())
}
