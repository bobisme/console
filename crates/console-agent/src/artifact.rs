//! Shared filesystem handling for generated inspection artifacts.

use std::path::Path;

/// Write a generated artifact, creating any missing parent directories first.
pub fn write(path: impl AsRef<Path>, bytes: &[u8]) -> Result<(), String> {
    let path = path.as_ref();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "cannot create output directory {:?} for {:?}: {e}",
                parent, path
            )
        })?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("cannot write {path:?}: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn write_creates_nested_parent_directories() {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("console-agent-artifact-{}-{n}", std::process::id()))
            .join("deep")
            .join("frame.bin");

        super::write(&path, b"pixels").expect("write nested artifact");
        assert_eq!(std::fs::read(path).unwrap(), b"pixels");
    }
}
