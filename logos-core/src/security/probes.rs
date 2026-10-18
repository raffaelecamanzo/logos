//! The version probes a page read runs, once per process for each binary
//! (HF-3, [ADR-38], [ADR-80] §4).
//!
//! The web read path asks for a scanner's version on every page load, and a
//! cross-site page can make that load happen. [`cached`] answers a repeat ask
//! from memory, so each binary's version command runs at most once for the
//! life of the serve process. The key is the scanner, the binary's resolved
//! path and its modification time: a reinstall — a new target behind the same
//! symlink, or a file rewritten in place — is probed again. A failed probe is
//! remembered as failed, under the same key.
//!
//! [ADR-38]: ../../../docs/specs/architecture/decisions/ADR-38.md
//! [ADR-80]: ../../../docs/specs/architecture/decisions/ADR-80.md

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::SystemTime;

/// A probe's identity: the scanner (its version command), the binary's
/// canonical path, and that file's modification time.
type ProbeKey = (&'static str, PathBuf, SystemTime);

/// One probe's cell: empty until its first probe. Its own lock is what makes
/// concurrent first reads of one binary probe it once.
type ProbeSlot = Arc<Mutex<Option<Option<String>>>>;

/// Every probe this process has run on the read path, one cell per distinct
/// (scanner, binary, mtime). Bounded by construction: a handful of scanners,
/// each re-keyed only when its binary changes.
static PROBES: LazyLock<Mutex<HashMap<ProbeKey, ProbeSlot>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `probe`'s answer for `scanner`'s `binary`, run only when no earlier call in
/// this process probed the same binary unchanged. A binary whose path cannot
/// be resolved or whose modification time cannot be read is probed uncached.
pub(super) fn cached(scanner: &'static str, binary: &Path, probe: impl FnOnce() -> Option<String>) -> Option<String> {
    let Some(key) = key(scanner, binary) else {
        return probe();
    };
    let slot = Arc::clone(PROBES.lock().unwrap_or_else(PoisonError::into_inner).entry(key).or_default());
    let mut cell = slot.lock().unwrap_or_else(PoisonError::into_inner);
    cell.get_or_insert_with(probe).clone()
}

fn key(scanner: &'static str, binary: &Path) -> Option<ProbeKey> {
    let resolved = binary.canonicalize().ok()?;
    let modified = resolved.metadata().and_then(|m| m.modified()).ok()?;
    Some((scanner, resolved, modified))
}
