//! Atomic file saving.

use std::io::Write;

/// Atomically writes `data` to `path`.
///
/// Strategy: write a temp file in the *same directory*, fsync it, then
/// `rename` over the target. A crash mid-write never leaves a truncated
/// or partially written target file, and the rename fails cleanly (with
/// the temp file cleaned up) if the directory is not writable.
pub fn atomic(path: &std::path::Path, data: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.rustcode-swap"));

    {
        let mut file = std::fs::File::create(&tmp)?;
        {
            let mut writer = std::io::BufWriter::new(&mut file);
            writer.write_all(data.as_bytes())?;
            writer.flush()?;
        }
        file.sync_all()?;
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    // Persist the rename itself; best-effort (some filesystems refuse).
    if let Ok(dir_file) = std::fs::File::open(dir) {
        let _ = dir_file.sync_all();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.py");
        atomic(&path, "x = 1\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "x = 1\n");
        // No leftover swap file.
        assert!(dir.path().read_dir().unwrap().count() == 1);
    }

    #[test]
    fn overwrite_preserves_no_partial_state_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        // A *directory* at the target path makes the rename fail.
        let target = dir.path().join("conflict");
        std::fs::create_dir(&target).unwrap();
        let err = atomic(&target, "data").unwrap_err();
        assert!(matches!(
            err.kind(),
            std::io::ErrorKind::NotADirectory
                | std::io::ErrorKind::PermissionDenied
                | std::io::ErrorKind::IsADirectory
                | std::io::ErrorKind::DirectoryNotEmpty
        ));
        // The failed attempt left no swap file behind.
        let leftovers: Vec<_> = dir
            .path()
            .read_dir()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("rustcode-swap"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
