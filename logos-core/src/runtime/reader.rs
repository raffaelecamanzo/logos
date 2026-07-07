//! The read-only WAL connection pool ([ADR-02], [NFR-PE-01], [NFR-RA-10]).
//!
//! Navigation reads run on one of `N` read-only connections over the WAL
//! snapshot. Under WAL a reader takes no lock the writer would block on, so a
//! read **never waits for an in-flight write** ([NFR-PE-01]). The pool bounds
//! read concurrency to `N`: a caller that arrives when all connections are
//! checked out blocks until one is returned, rather than opening an unbounded
//! number of file handles.
//!
//! Connections live in a [`crossbeam_channel`] used as an mpmc free-list:
//! [`with_connection`](ReaderPool::with_connection) pops one, runs the read on
//! the **calling thread** (so concurrent callers genuinely read in parallel —
//! the pool is not a single worker thread), then a [`Checkout`] guard returns
//! the connection on the way out, even if the read closure panics.
//!
//! [ADR-02]: ../../../docs/specs/architecture/decisions/ADR-02.md
//! [NFR-PE-01]: ../../../docs/specs/requirements/NFR-PE-01.md
//! [NFR-RA-10]: ../../../docs/specs/requirements/NFR-RA-10.md

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::graph_store::{GraphStore, SqliteGraphStore};

/// A fixed-size pool of read-only WAL connections.
pub(crate) struct ReaderPool {
    /// Checked-in (idle) connections. `recv` checks one out (blocking if empty);
    /// the [`Checkout`] guard `send`s it back. mpmc, so many caller threads can
    /// hold distinct connections at once.
    idle: Receiver<SqliteGraphStore>,
    /// The return path the [`Checkout`] guard sends a connection back on.
    return_to: Sender<SqliteGraphStore>,
    /// How many connections the pool owns — the maximum read concurrency.
    size: usize,
}

impl ReaderPool {
    /// Open `size` read-only connections over the already-migrated `db_path`.
    ///
    /// # Errors
    /// Returns an error if `size` is zero, or if any connection cannot be opened
    /// read-only (e.g. the writer has not yet created/migrated the database).
    pub(crate) fn open(db_path: &Path, size: usize) -> Result<Self> {
        if size == 0 {
            return Err(anyhow!(
                "read-only pool size must be at least 1 (got 0) — no readers could serve navigation"
            ));
        }
        let (return_to, idle) = bounded::<SqliteGraphStore>(size);
        for i in 0..size {
            let conn = SqliteGraphStore::open_readonly(db_path)
                .with_context(|| format!("opening read-only pool connection {i}/{size}"))?;
            return_to
                .send(conn)
                .expect("the pool channel is sized to hold every connection");
        }
        Ok(Self {
            idle,
            return_to,
            size,
        })
    }

    /// The maximum number of concurrent readers (the pool size).
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    /// Check out a connection, run `f` against it, and return the connection.
    ///
    /// Blocks if every connection is currently checked out (bounded read
    /// concurrency). `f` runs on the **calling thread**, so it needs no `Send` /
    /// `'static` bound and concurrent callers read in true parallel. The
    /// connection is returned to the pool by the [`Checkout`] guard's `Drop`,
    /// even if `f` panics — a panicking read can never shrink the pool.
    ///
    /// # Errors
    /// Returns the read closure's error, or a runtime error if the pool has been
    /// torn down.
    pub(crate) fn with_connection<T>(
        &self,
        f: impl FnOnce(&dyn GraphStore) -> Result<T>,
    ) -> Result<T> {
        let conn = self
            .idle
            .recv()
            .map_err(|_| anyhow!("read-only connection pool is closed"))?;
        let checkout = Checkout {
            conn: Some(conn),
            return_to: &self.return_to,
        };
        // `expect` is unreachable: `conn` is `Some` until the guard's `Drop`.
        let store = checkout
            .conn
            .as_ref()
            .expect("checked-out connection is present for the read");
        f(store as &dyn GraphStore)
    }
}

/// RAII guard that returns a checked-out connection to the pool on drop.
///
/// Holding the return through a `Drop` impl (rather than an explicit `send`
/// after `f`) is what makes the pool panic-safe: if the read closure unwinds,
/// the connection is still checked back in as the stack unwinds past this guard.
struct Checkout<'a> {
    conn: Option<SqliteGraphStore>,
    return_to: &'a Sender<SqliteGraphStore>,
}

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // Best-effort: if the pool's receiver is gone (teardown in progress)
            // the connection simply drops and closes — no panic in `drop`.
            let _ = self.return_to.send(conn);
        }
    }
}
