//! The one **atomic publish** primitive every file logos rewrites in place goes
//! through: write a sibling temp, `fsync` it, `rename` it over the target
//! ([NFR-RA-07]).
//!
//! # Why this is shared rather than written twice
//! It was written twice. [`config::writeback`](crate::config::writeback) has
//! published policy files this way since S-020, and
//! [`federation::warm_state`](crate::federation::warm_state) needed the same
//! guarantee for the warm-outcome sidecar ([FR-WS-17]) — and the second copy
//! silently dropped the thread id from the temp name, reintroducing the exact
//! corruption the first copy's doc comment exists to warn about. That is the
//! failure mode a shared primitive prevents: not the cost of the duplication,
//! but the *divergence*, which was invisible in review because both copies
//! looked correct in isolation.
//!
//! # The guarantee, precisely
//! A reader concurrent with a publish sees either the whole previous file or the
//! whole new one — never a partial write, and never a missing file. A crash
//! mid-publish leaves the previous file intact and at worst an orphan temp. The
//! temp is a **sibling** of the target (same directory ⇒ same filesystem), so
//! the `rename` is an in-place inode swap rather than a cross-device copy, and
//! it is `sync_all`-ed before the swap so the bytes are durable before they
//! become visible.
//!
//! What is deliberately **not** guaranteed: the containing directory is not
//! fsynced after the rename, so a power loss immediately after a publish can
//! lose the *rename* on some filesystems. Every caller here publishes advisory
//! or regenerable state, and the house convention has always been this shape.
//!
//! # What it does not own
//! Directory creation, error typing, and byte accounting stay with the caller —
//! they differ per call site (a policy file wants a `ConfigError` and a fresh
//! `.logos/` created; the warm sidecar wants a plain [`std::io::Error`] and a
//! workspace root that must already exist), and folding them in here would make
//! this primitive answer to two vocabularies at once.
//!
//! [FR-WS-17]: ../../docs/specs/requirements/FR-WS-17.md

use std::fs;
use std::path::{Path, PathBuf};

/// Atomically replace `target` with `bytes`.
///
/// `unix_mode` sets the **temp file's** Unix permission bits *at creation*, so
/// the perms hold for the whole window the bytes exist on disk and `rename`
/// carries them to `target`. `None` leaves the OS default (umask-dependent) —
/// correct for world-readable checked-in policy files and for the warm sidecar.
/// The secret store passes `Some(0o600)` so an API key is never even briefly
/// group/world-readable ([NFR-SE-07], defense-in-depth on the at-rest key). On
/// non-Unix the mode is a no-op.
///
/// The parent directory must already exist; a caller that may be writing into a
/// fresh tree creates it first.
///
/// # Errors
/// Any I/O failure from creating, writing, syncing or renaming. On every error
/// path the temp is removed and `target` is left byte-identical.
pub(crate) fn publish(target: &Path, bytes: &[u8], unix_mode: Option<u32>) -> std::io::Result<()> {
    let tmp = sibling_tmp_path(target);

    let result = (|| {
        use std::io::Write as _;
        let mut file = create_temp(&tmp, unix_mode)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, target)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Create the temp file, honoring an explicit Unix permission mode at creation
/// time.
///
/// On Unix with `Some(mode)` the file is opened with the mode applied via
/// [`std::os::unix::fs::OpenOptionsExt`], so it is never momentarily readable at
/// the default umask before a follow-up `chmod`. `None` (or non-Unix) falls back
/// to [`fs::File::create`].
fn create_temp(tmp: &Path, unix_mode: Option<u32>) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    if let Some(mode) = unix_mode {
        use std::os::unix::fs::OpenOptionsExt as _;
        return fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(tmp);
    }
    let _ = unix_mode;
    fs::File::create(tmp)
}

/// A sibling temp path for `target`: `.<name>.<pid>.<tid>.tmp`, in the same
/// directory.
///
/// Keyed by **both** the process id and the thread id so two threads in the same
/// process writing the same target (two concurrent surface requests, two workers
/// of one queue) never compute the same temp path — without it, one thread's
/// `File::create` would truncate the file the other is mid-`write_all` into, and
/// the `rename` would publish corrupt bytes **with no error returned to either**.
/// `rename` is still the only swap, so even concurrent writers each publish
/// their own whole document, last-writer-wins.
fn sibling_tmp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "logos".to_string());
    let tmp_name = format!(
        ".{name}.{}.{:?}.tmp",
        std::process::id(),
        std::thread::current().id()
    );
    match target.parent() {
        Some(parent) => parent.join(tmp_name),
        None => PathBuf::from(tmp_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The temp path separates two threads of ONE process — the property the
    /// federation copy of this code silently dropped, and the reason the
    /// primitive is shared rather than duplicated.
    ///
    /// A pid-only name returns the same path from both threads and fails here.
    #[test]
    fn the_temp_path_separates_two_threads_of_the_same_process() {
        let target = Path::new("/w/.logos.workspace.warm.json");
        let mine = sibling_tmp_path(target);
        let theirs = std::thread::spawn(move || sibling_tmp_path(target))
            .join()
            .expect("sibling thread");

        assert_ne!(mine, theirs, "two threads must not share a temp path");
        for path in [&mine, &theirs] {
            assert_eq!(path.parent(), target.parent(), "the temp is a SIBLING");
            assert!(path.to_string_lossy().ends_with(".tmp"));
        }
    }

    /// Concurrent publishes of different bytes to one target each land whole:
    /// the reader sees one writer's complete document, never a splice of two.
    #[test]
    fn concurrent_publishes_each_land_whole() {
        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("doc");
        let bodies: Vec<Vec<u8>> = (0..4).map(|i| vec![b'a' + i; 200_000]).collect();

        std::thread::scope(|scope| {
            for body in &bodies {
                scope.spawn(|| publish(&target, body, None).expect("publish"));
            }
        });

        let landed = fs::read(&target).expect("target");
        assert!(
            bodies.contains(&landed),
            "the target holds one writer's whole document, not a splice"
        );
        let leftovers: Vec<String> = fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
    }

    /// A failed publish returns the error, removes its temp, and leaves the
    /// previous target byte-identical.
    #[test]
    fn a_failed_publish_cleans_up_and_leaves_the_target_untouched() {
        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("doc");
        publish(&target, b"original", None).expect("seed");

        // A directory where the temp must be created: `File::create` cannot
        // succeed on it for any user, on any platform.
        fs::create_dir(sibling_tmp_path(&target)).expect("obstruct the temp path");

        assert!(publish(&target, b"replacement", None).is_err());
        assert_eq!(fs::read(&target).expect("target"), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn a_mode_is_applied_at_creation_and_survives_the_rename() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("dir");
        let target = dir.path().join("secret");
        publish(&target, b"key", Some(0o600)).expect("publish");

        let mode = fs::metadata(&target).expect("target").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the temp's mode reached the target");
    }
}
