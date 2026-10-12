//! Shared SQLite open helpers for the side stores that are created on demand by
//! concurrent requests (`history.db` here, `chat.db` in `chat-agent`).

use std::time::Duration;

use rusqlite::Connection;

/// How many times [`execute_pragmas_retrying_busy`] retries a busy pragma batch,
/// and how long it sleeps between tries: at most about one second in all.
const PRAGMA_BUSY_RETRIES: u32 = 40;
const PRAGMA_BUSY_BACKOFF: Duration = Duration::from_millis(25);

/// Run a store's pragma batch, retrying while another connection holds the lock.
///
/// Switching a brand-new file to WAL needs the database lock. When a second
/// connection creates the same store at the same moment — the Chat view reads
/// several routes at once on a fresh project — SQLite can answer `SQLITE_BUSY`
/// without consulting the busy handler (it declines to wait on a lock upgrade
/// that could deadlock), so `busy_timeout` alone does not cover it. Any other
/// error, or a busy one that outlasts the retries, is returned unchanged.
///
/// Put `PRAGMA busy_timeout` first in `pragmas`, so every later statement on the
/// connection waits for the lock as usual.
///
/// # Errors
/// Returns the batch's error once it is not `SQLITE_BUSY`/`SQLITE_LOCKED`, or
/// after the retries run out.
pub fn execute_pragmas_retrying_busy(conn: &Connection, pragmas: &str) -> rusqlite::Result<()> {
    let mut attempt = 0;
    loop {
        match conn.execute_batch(pragmas) {
            Err(rusqlite::Error::SqliteFailure(e, _))
                if attempt < PRAGMA_BUSY_RETRIES
                    && matches!(
                        e.code,
                        rusqlite::ffi::ErrorCode::DatabaseBusy
                            | rusqlite::ffi::ErrorCode::DatabaseLocked
                    ) =>
            {
                attempt += 1;
                std::thread::sleep(PRAGMA_BUSY_BACKOFF);
            }
            result => return result,
        }
    }
}
