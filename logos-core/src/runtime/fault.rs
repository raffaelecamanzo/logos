//! The **test-only persistence fault seam** (S-513, [FR-EH-05]).
//!
//! [FR-EH-05] promises that one file whose facts cannot be persisted fails
//! alone. Every real trigger of that path is a defect — the cross-kind symbol
//! collision [CR-168] found is being fixed in the same sprint — so the
//! promise can only be tested by failing a file on purpose. This seam does
//! that: a file named here fails **after** its facts were written inside its
//! own isolation unit, so the rollback that must undo them is exercised, not
//! skipped.
//!
//! Compiled into **debug builds only**. In a release build [`PersistFaults`] is
//! a zero-sized type whose [`fails`](PersistFaults::fails) is a constant
//! `false`, so the shipped binary carries neither the check nor the
//! environment read. Two ways in, both scoped to one [`Runtime`](super::Runtime):
//!
//! - in process, [`Runtime::inject_persist_fault`](super::Runtime::inject_persist_fault)
//!   — what the library tests use, so parallel tests never see each other's
//!   faults;
//! - across a process boundary (the CLI and MCP binaries under test), the
//!   [`PERSIST_FAULT_ENV`] variable, read once when the runtime opens: a
//!   comma-separated list of project-relative paths, or `*` for every file.
//!
//! [FR-EH-05]: ../../../docs/specs/requirements/FR-EH-05.md
//! [CR-168]: ../../../docs/requests/CR-168-an-index-never-silently-empties.md

#[cfg(debug_assertions)]
use std::collections::BTreeSet;
#[cfg(debug_assertions)]
use std::sync::Mutex;

/// The environment variable a debug build reads at runtime open to fail the
/// persistence of the files it names (comma-separated project-relative paths,
/// or `*` for all). Ignored — never read — by a release build.
#[cfg(debug_assertions)]
pub const PERSIST_FAULT_ENV: &str = "LOGOS_TEST_FAIL_PERSIST";

/// The reason a faulted file's write is rolled back with — what
/// `persist_failures` (and the file's warning) carries as the file's reason.
pub(crate) const INJECTED_FAULT_REASON: &str = "injected persistence fault (test seam)";

/// The set of files whose persistence a runtime fails on purpose.
#[derive(Debug, Default)]
pub(crate) struct PersistFaults {
    #[cfg(debug_assertions)]
    paths: Mutex<BTreeSet<String>>,
}

impl PersistFaults {
    /// The faults named by [`PERSIST_FAULT_ENV`], or none.
    pub(crate) fn from_env() -> Self {
        #[cfg(debug_assertions)]
        {
            let paths = std::env::var(PERSIST_FAULT_ENV)
                .map(|raw| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Self {
                paths: Mutex::new(paths),
            }
        }
        #[cfg(not(debug_assertions))]
        {
            Self {}
        }
    }

    /// Whether persisting `rel` must fail. Always `false` in a release build.
    #[inline]
    pub(crate) fn fails(&self, rel: &str) -> bool {
        #[cfg(debug_assertions)]
        {
            let paths = self.paths.lock().unwrap_or_else(|p| p.into_inner());
            paths.contains(rel) || paths.contains("*")
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = rel;
            false
        }
    }

    /// Fail every later persistence of `rel` (`*`: of every file).
    #[cfg(debug_assertions)]
    pub(crate) fn inject(&self, rel: &str) {
        self.paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(rel.to_string());
    }

    /// Remove every injected fault.
    #[cfg(debug_assertions)]
    pub(crate) fn clear(&self) {
        self.paths.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::PersistFaults;

    #[test]
    fn a_named_path_fails_its_near_misses_do_not_and_star_fails_all() {
        let faults = PersistFaults::default();
        assert!(!faults.fails("src/b.rs"), "no fault is injected by default");
        faults.inject("src/b.rs");
        assert!(faults.fails("src/b.rs"));
        for near in ["src/b.r", "src/b.rs ", "src/bb.rs", "b.rs", "src/b.rs/x"] {
            assert!(!faults.fails(near), "{near:?} is not the named file");
        }
        faults.clear();
        assert!(!faults.fails("src/b.rs"), "clear removes the fault");
        faults.inject("*");
        assert!(faults.fails("anything/at/all.go"), "`*` fails every file");
    }
}
