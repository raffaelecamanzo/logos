//! The custom telemetry [`Layer`] and its async/batched background writer
//! ([FR-OB-03], [NFR-OO-02]).
//!
//! # Never on the hot path
//!
//! The layer's `on_event` does exactly two cheap things: visit the event's
//! fields into an [`EventRecord`], and `try_send` it down a **bounded**
//! channel. `try_send` never blocks — when the queue is full (a stalled or
//! slow writer), the event is **dropped**, which is the structural guarantee
//! behind "a telemetry write can never block a query or fail a command"
//! ([NFR-OO-02], [NFR-CC-03]). The SQLite I/O happens on one dedicated
//! background thread that drains the queue in batches and commits each batch
//! in a single transaction.
//!
//! # Shutdown
//!
//! The layer itself is owned by the global subscriber and lives for the
//! process; flushing is driven by the [`TelemetryGuard`] the surface holds.
//! Dropping the guard signals the writer over a dedicated control channel,
//! which drains whatever is still queued, writes the final batch, and joins.
//! A crash skips that and loses the last unflushed batch — the accepted
//! [ADR-13] trade-off.
//!
//! [FR-OB-03]: ../../../docs/specs/requirements/FR-OB-03.md
//! [NFR-OO-02]: ../../../docs/specs/requirements/NFR-OO-02.md
//! [NFR-CC-03]: ../../../docs/specs/requirements/NFR-CC-03.md
//! [ADR-13]: ../../../docs/specs/architecture/decisions/ADR-13.md

use std::path::PathBuf;
use std::thread::JoinHandle;

use crossbeam_channel::{bounded, select, Receiver, Sender};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::Layer;

use super::{EventRecord, Surface, TELEMETRY_TARGET};

/// Queue capacity. Sized for bursts (a full index emits a handful of pass
/// events; an MCP session a few events per tool call) while bounding memory:
/// at worst ~1024 small records (~100 B each) before drops begin.
const QUEUE_CAPACITY: usize = 1024;

/// Max events folded into one INSERT transaction by the writer.
const MAX_BATCH: usize = 256;

/// The producer half handed to the [`TelemetryLayer`]: a non-blocking,
/// best-effort emitter.
#[derive(Clone)]
pub(crate) struct TelemetrySink {
    tx: Sender<EventRecord>,
}

impl TelemetrySink {
    /// Queue `record` for the background writer. **Never blocks**: a full
    /// queue (slow/stalled writer) drops the record on the floor
    /// ([NFR-OO-02] — best-effort by construction).
    pub(crate) fn record(&self, record: EventRecord) {
        let _ = self.tx.try_send(record);
    }

    /// A sink over an explicit bounded channel, with the consumer half — the
    /// test seam for the never-blocks and field-extraction contracts.
    #[cfg(test)]
    pub(crate) fn with_capacity(capacity: usize) -> (Self, Receiver<EventRecord>) {
        let (tx, rx) = bounded(capacity);
        (Self { tx }, rx)
    }
}

/// RAII handle owning the background writer thread.
///
/// Dropping it flushes the queue and joins the writer; the disabled variant
/// (no `.logos/` directory → no telemetry) is a no-op.
pub struct TelemetryGuard {
    shutdown: Option<Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

impl TelemetryGuard {
    /// A guard for a surface that runs without telemetry (e.g. a project that
    /// has no `.logos/` directory yet — telemetry must not create one as a
    /// side effect of an arbitrary read command).
    pub(crate) fn disabled() -> Self {
        Self {
            shutdown: None,
            handle: None,
        }
    }
}

impl Drop for TelemetryGuard {
    fn drop(&mut self) {
        // Wake the writer over the control channel (capacity 1 — never full),
        // then wait for it to drain and commit the final batch. If the writer
        // already exited (failed DB open), the send errs and join returns.
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The custom `tracing` [`Layer`] persisting telemetry-tagged events
/// ([FR-OB-03], [ADR-13]).
///
/// Only events whose target is [`TELEMETRY_TARGET`] are considered (the
/// installer additionally pins this with a per-layer filter so other layers'
/// verbosity never reaches the sink); everything else belongs to the stderr
/// fmt layer.
pub(crate) struct TelemetryLayer {
    surface: Surface,
    /// The per-process development-increment stamp ([FR-OB-08]): computed once
    /// at [`super::init`] and copied onto every record, orthogonal to the
    /// per-event `surface` override.
    ///
    /// [FR-OB-08]: ../../../docs/specs/requirements/FR-OB-08.md
    origin: String,
    sink: TelemetrySink,
}

impl TelemetryLayer {
    pub(crate) fn new(surface: Surface, origin: String, sink: TelemetrySink) -> Self {
        Self {
            surface,
            origin,
            sink,
        }
    }
}

impl<S: Subscriber> Layer<S> for TelemetryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if event.metadata().target() != TELEMETRY_TARGET {
            return;
        }
        let mut visitor = TelemetryVisitor::default();
        event.record(&mut visitor);
        if let Some(record) = visitor.into_record(self.surface, &self.origin) {
            self.sink.record(record);
        }
    }
}

/// Field visitor extracting `tool` / `duration_ms` / `ok` (and the optional
/// `surface` override) from a telemetry-tagged event.
#[derive(Default)]
struct TelemetryVisitor {
    tool: Option<String>,
    duration_ms: Option<u64>,
    ok: Option<bool>,
    surface_override: Option<&'static str>,
}

impl Visit for TelemetryVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "tool" {
            self.tool = Some(value.to_string());
        }
        // Per-event surface override (S-022): the debounced watcher runs
        // *inside* the `serve --mcp` process (whose process-level surface is
        // `mcp`) but its syncs must be attributable as `surface=watcher` per
        // the filesystem-watcher integration spec. Only the sanctioned value
        // is honoured — an arbitrary string cannot invent a surface.
        if field.name() == "surface" && value == "watcher" {
            self.surface_override = Some("watcher");
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "duration_ms" {
            self.duration_ms = Some(value);
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "ok" {
            self.ok = Some(value);
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {
        // Telemetry fields are emitted with primitive types by the single
        // emission helper; anything else is not ours to record.
    }
}

impl TelemetryVisitor {
    /// A record only if the emission helper's full shape was present —
    /// a malformed event is dropped, never half-recorded.
    ///
    /// `origin` is the process-wide increment stamp ([FR-OB-08]); the
    /// per-event `surface_override` (the watcher, S-022) is applied
    /// independently, so the two never interfere.
    fn into_record(self, surface: Surface, origin: &str) -> Option<EventRecord> {
        Some(EventRecord {
            at: now_unix(),
            surface: self.surface_override.unwrap_or(surface.as_str()),
            tool: self.tool?,
            duration_ms: self.duration_ms?,
            ok: self.ok?,
            origin: origin.to_string(),
        })
    }
}

/// Seconds since the Unix epoch (0 on a pre-1970 clock — best-effort).
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Spawn the bounded channel + background writer for `db_path`.
///
/// Returns the producer sink and the flushing guard. The writer:
/// 1. opens/migrates `telemetry.db` (failure → thread exits; the sink keeps
///    dropping events silently — best-effort, [NFR-CC-03]),
/// 2. rolls aged events into `daily_rollup` and prunes them ([NFR-OO-04]),
/// 3. drains the queue in batches of at most [`MAX_BATCH`], one transaction
///    per batch, until the guard signals shutdown.
pub(crate) fn spawn_writer(db_path: PathBuf) -> (TelemetrySink, TelemetryGuard) {
    let (tx, rx) = bounded::<EventRecord>(QUEUE_CAPACITY);
    let (shutdown_tx, shutdown_rx) = bounded::<()>(1);

    let handle = std::thread::Builder::new()
        .name("logos-telemetry".into())
        .spawn(move || writer_loop(&rx, &shutdown_rx, &db_path))
        .ok();

    (
        TelemetrySink { tx },
        TelemetryGuard {
            shutdown: Some(shutdown_tx),
            handle,
        },
    )
}

/// The background writer body. Every DB error is swallowed: telemetry is
/// best-effort and must never fail a command ([NFR-OO-02], [NFR-CC-03]).
fn writer_loop(rx: &Receiver<EventRecord>, shutdown: &Receiver<()>, db_path: &std::path::Path) {
    let Ok(mut conn) = super::db::open(db_path) else {
        // No store, no telemetry: exit; the bounded sink drops events.
        return;
    };
    let _ = super::db::rollup_and_prune(&mut conn, now_unix(), super::db::RETENTION_DAYS);

    loop {
        select! {
            recv(rx) -> msg => {
                let Ok(first) = msg else { break };
                let mut batch = Vec::with_capacity(16);
                batch.push(first);
                while batch.len() < MAX_BATCH {
                    match rx.try_recv() {
                        Ok(record) => batch.push(record),
                        Err(_) => break,
                    }
                }
                let _ = super::db::write_batch(&mut conn, &batch);
            }
            recv(shutdown) -> _ => {
                // Final flush: drain whatever is still queued, then exit.
                let remaining: Vec<EventRecord> = rx.try_iter().collect();
                if !remaining.is_empty() {
                    let _ = super::db::write_batch(&mut conn, &remaining);
                }
                break;
            }
        }
    }
}
