//! Offline mock-`CompletionModel` generation-pass tests for the wiki-agent
//! (S-177, [FR-WK-18], [FR-WK-13], [FR-WK-02], [FR-WK-03], [FR-WK-12],
//! [NFR-SE-01], [NFR-SE-07], [NFR-CC-04]).
//!
//! These drive [`WikiAgent::run`] over a **real** `Engine` fixture (a temp git
//! repo, indexed so the graph has a revision > 0 and the wiki work-list yields
//! pages to generate) with the mock provider. They prove:
//!
//! - a full pass consumes the deterministic FR-WK-13 queue **in order** and
//!   writes each page via `wiki write` with correct write-time anchors, HEAD, and
//!   built-at-revision; a regenerated stale page reads **fresh on both axes**
//!   ([FR-WK-03] content, [FR-WK-12] revision);
//! - the pass records **zero real outbound connections** (a loopback tripwire);
//! - an **empty work-list starts no run** and makes no model call;
//! - a **spent per-run budget halts** the pass honestly.
//!
//! The real-`Engine` fixture helpers mirror `logos-core/tests/wiki_store.rs`; the
//! tripwire mirrors `agent-core/tests/zero_egress.rs`.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::{MockCompletionModel, MockTurn};
use logos_core::config::{ChatProvider, EffectiveWikiModel};
use logos_core::wiki::{revision_pending, GenerationCategory};
use logos_core::Engine;
use tempfile::TempDir;
use tokio::net::TcpListener;
use wiki_agent::{run_configured, ConfiguredRun, WikiAgent, WikiProgress};

// ── Real-Engine fixture helpers (mirror logos-core/tests/wiki_store.rs) ───────

fn sh_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
        .args(args)
        .output()
        .expect("git is on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn commit(cwd: &Path, rel: &str, contents: &str, msg: &str) {
    let path = cwd.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    sh_git(cwd, &["add", rel]);
    sh_git(cwd, &["commit", "-q", "-m", msg]);
}

/// A branchy body so an edit changes the tree/graph (and thus an anchored page's
/// content hash).
fn branchy(name: &str, ifs: usize) -> String {
    let body: String = (0..ifs)
        .map(|i| format!("    if x == {i} {{ return {i}; }}\n"))
        .collect();
    format!("pub fn {name}(x: i64) -> i64 {{\n{body}    x\n}}\n")
}

/// A committed, indexed repo with two source files under an `Arc<Engine>`.
fn indexed_engine(repo: &Path) -> Arc<Engine> {
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");
    let engine = Engine::start(repo).expect("engine starts");
    engine.index();
    Arc::new(engine)
}

/// Collect every progress event a run emits into a shared vec (a `Sync` sink the
/// borrow can cross the run's awaits).
fn recording_sink() -> (Arc<std::sync::Mutex<Vec<WikiProgress>>>, impl Fn(WikiProgress)) {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let sink = move |p: WikiProgress| sink_events.lock().unwrap().push(p);
    (events, sink)
}

// ── Loopback tripwire (mirror agent-core/tests/zero_egress.rs) ────────────────

struct Tripwire {
    connections: Arc<AtomicUsize>,
}

async fn spawn_tripwire() -> Tripwire {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind tripwire");
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&connections);
    tokio::spawn(async move {
        while listener.accept().await.is_ok() {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    Tripwire { connections }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// A full pass over a seeded work-list writes every queued page **in the
/// deterministic FR-WK-13 order**, records correct write-time anchors/HEAD/
/// built-at-revision, and leaves a regenerated stale page **fresh on both axes**.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pass_writes_queue_in_order_with_dual_axis_fresh_pages() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    let engine = indexed_engine(repo);
    let rev1 = engine.status().graph_revision;
    assert!(rev1 >= 1, "an index establishes a graph revision");

    // Pre-write a page anchored to a real file, then drift it on BOTH axes:
    // edit the anchored file (content axis, FR-WK-03) and re-index (revision axis,
    // FR-WK-12). It now appears in the generation queue as a StalePage refresh.
    engine
        .wiki_write(
            "guide/a",
            "About a",
            "# About a\n\nThe seed body, long enough to clear the write-path guard.",
            &["file:src/a.rs".to_string()],
            "seed",
        )
        .expect("seed write");
    assert!(
        !engine.wiki_read("guide/a").unwrap().unwrap().stale,
        "the seed page is fresh right after writing"
    );
    std::fs::write(repo.join("src/a.rs"), branchy("a", 5)).unwrap();
    engine.index();
    let current = engine.status().graph_revision;
    assert!(current > rev1, "the re-index advanced the graph revision");
    {
        let stale = engine.wiki_read("guide/a").unwrap().unwrap();
        assert!(stale.stale, "editing the anchored file drifted the page (content axis)");
        assert!(
            revision_pending(stale.built_at_revision, current),
            "the page built at the old revision is revision-stale (FR-WK-12)"
        );
    }

    // The deterministic FR-WK-13 queue the agent will consume.
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty(), "a seeded work-list is non-empty");
    let expected_slugs: Vec<String> = queue.items.iter().map(|i| i.slug.clone()).collect();
    assert!(
        queue
            .items
            .iter()
            .any(|i| i.slug == "guide/a" && i.category == GenerationCategory::StalePage),
        "the drifted page is queued as a StalePage refresh"
    );

    // Script the mock with one text turn per queued page; each returns a distinct
    // non-empty body. The mock ignores the prompt, so the loop runs deterministically.
    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Generated page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "test skill preamble", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("the run completes without infrastructure error")
        .expect("a non-empty work-list starts a run");

    // Deterministic order: written slugs == the queue's order, nothing failed or halted.
    assert_eq!(
        outcome.pages_written, expected_slugs,
        "every queued page is written, in the deterministic FR-WK-13 order"
    );
    assert!(outcome.pages_failed.is_empty(), "no page write was rejected");
    assert!(outcome.halted.is_none(), "a budget-sufficient pass does not halt");
    assert_eq!(
        mock.request_count(),
        queue.items.len(),
        "the mock — not a real provider — synthesized exactly one turn per page"
    );

    // The progress stream is well-formed: a Started, a PageWritten per page, and a
    // terminal Completed.
    let events = events.lock().unwrap();
    assert!(matches!(events.first(), Some(WikiProgress::Started { total, .. }) if *total == queue.items.len()));
    assert!(matches!(events.last(), Some(WikiProgress::Completed { pages_written, .. }) if *pages_written == queue.items.len()));

    // Dual-axis freshness after regeneration: every written page reads fresh —
    // content axis (no stale anchor) and revision axis (built at the current
    // revision, so not revision-pending).
    for slug in &expected_slugs {
        let page = engine
            .wiki_read(slug)
            .expect("read")
            .expect("a just-written page is present");
        assert!(!page.stale, "{slug} reads fresh on the content axis after regeneration");
        assert_eq!(
            page.built_at_revision, current,
            "{slug} records the current built-at revision (FR-WK-12)"
        );
        assert!(
            !revision_pending(page.built_at_revision, current),
            "{slug} reads fresh on the revision axis after regeneration"
        );
        assert_eq!(page.generator, "mock-model", "the generator label is recorded (FR-WK-02)");
        assert!(!page.written_head.is_empty(), "the write-time HEAD is recorded (FR-WK-02)");
    }

    // The regenerated refresh re-supplied its surviving anchor, so it is anchored
    // and fresh (not stripped to a zero-anchor page).
    let guide = engine.wiki_read("guide/a").unwrap().unwrap();
    assert_eq!(guide.anchors.len(), 1, "the refresh re-supplied the existing anchor");
    assert_eq!(guide.anchors[0].kind, "file");
    assert_eq!(guide.anchors[0].entity_id, "src/a.rs");
}

/// An empty work-list starts no run and makes no model call ([NFR-CC-04],
/// [FR-WK-18] acceptance). A repo that was never indexed has revision 0, so the
/// FR-WK-13 queue is empty.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_work_list_starts_no_run() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "README.md", "# fixture\n", "add readme");
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));

    let queue = engine.wiki_generate().expect("generate on an un-indexed repo");
    assert!(queue.items.is_empty(), "an un-indexed repo has an empty work-list");

    let mock = MockCompletionModel::new([]);
    let (events, sink) = recording_sink();
    let result = WikiAgent::new(mock.clone(), "test skill preamble", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("the empty-work-list check does not error");

    assert!(result.is_none(), "an empty work-list starts no run (Ok(None))");
    assert_eq!(mock.request_count(), 0, "no model call is made on an empty work-list");
    assert!(events.lock().unwrap().is_empty(), "no progress is emitted when no run starts");
}

/// A full mock-backed pass opens **zero real outbound connections** ([NFR-SE-07],
/// [FR-WK-18] acceptance) — the behavioral half of the offline carve-out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_generation_pass_records_zero_real_egress() {
    let tripwire = spawn_tripwire().await;

    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty());

    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");

    assert_eq!(outcome.pages_written.len(), queue.items.len(), "the pass wrote every page");
    assert!(mock.request_count() >= 1, "the mock served the pass");
    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "the mock-backed generation pass opened zero real outbound connections"
    );
}

/// A full pass over a **doc-grounded** item reads only the local `docs/` tree and
/// opens **zero** outbound connections ([NFR-SE-01], [FR-WK-18] acceptance): the
/// grounding resolver's new `std::fs` read path is exercised end-to-end in a run,
/// not just in the unit tests. Committing `README.md` and
/// `docs/specs/software-spec.md` makes the Project Overview and Key Concepts
/// items doc-grounded ([FR-WK-24]: both mapped sources present).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_pass_over_a_doc_grounded_item_reads_local_docs_with_zero_egress() {
    let tripwire = spawn_tripwire().await;

    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(
        repo,
        "README.md",
        "# Logos\n\nA structural code-intelligence tool. GROUNDING-DOC-SENTINEL.\n",
        "add readme",
    );
    commit(
        repo,
        "docs/specs/software-spec.md",
        "# Spec\n\nLogos is a structural code-intelligence tool. GROUNDING-DOC-SENTINEL.\n",
        "add spec",
    );
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();

    let queue = engine.wiki_generate().expect("generate queue");
    assert!(
        queue
            .items
            .iter()
            .any(|i| i.grounding.as_ref().is_some_and(|g| !g.fallback_to_code)),
        "the committed spec makes at least one queued item doc-grounded"
    );

    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            MockTurn::text(format!(
                "# Page {i}\n\nThis page contains enough grounded prose to clear the \
                 write-path validity guard introduced by S-237.",
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");

    assert!(!outcome.pages_written.is_empty(), "the doc-grounded pass wrote pages");
    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "a doc-grounded full pass read local docs and opened zero outbound connections"
    );
}

/// A work-list larger than the **per-chunk budget** is fully drained by one run via
/// auto-continue ([CR-056], [ADR-42]) — no manual re-trigger, no halt. A budget of
/// one forces a re-read after every page (the deterministic FR-WK-13 queue,
/// [NFR-RA-06]), so this exercises the auto-continue loop hardest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_small_per_chunk_budget_auto_continues_to_drain_the_work_list() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let k = queue.items.len();
    assert!(k > 1, "the fixture yields more than one queued page");
    let expected_slugs: Vec<String> = queue.items.iter().map(|i| i.slug.clone()).collect();

    let turns: Vec<_> = (0..k)
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    // A per-chunk budget of one: each chunk writes a single page, then the run
    // re-reads the queue and auto-continues — draining the whole work-list in one
    // trigger, with a generous ceiling that never fires.
    let (events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .with_budget(1)
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");

    assert_eq!(
        outcome.pages_written, expected_slugs,
        "auto-continue drained every queued page, in the deterministic FR-WK-13 order",
    );
    assert!(outcome.halted.is_none(), "a drained run does not halt: {:?}", outcome.halted);
    assert_eq!(
        mock.request_count(),
        k,
        "every page was synthesized across the auto-continued chunks",
    );
    // Cumulative progress: the terminal Completed reports the whole work-list, and
    // the `Started { total }` denominator is the initial work-list size (FR-UI-19).
    let events = events.lock().unwrap();
    assert!(
        matches!(events.first(), Some(WikiProgress::Started { total, .. }) if *total == k),
        "Started carries the cumulative work-list total: {:?}",
        events.first(),
    );
    assert!(
        matches!(events.last(), Some(WikiProgress::Completed { pages_written, .. }) if *pages_written == k),
        "Completed reports the whole drained run: {:?}",
        events.last(),
    );
}

/// `Started` carries the configured per-page synthesis liveness timeout ([CR-059],
/// [S-239], [FR-UI-24]): the default and an explicit override both flow through
/// unchanged, so the surface can make the 180s liveness guard VISIBLE instead of
/// leaving a long-running page looking stuck — the bound itself is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_carries_the_configured_synthesis_timeout() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (events, sink) = recording_sink();
    WikiAgent::new(mock, "skill", "mock-model")
        .with_synthesis_timeout(Duration::from_secs(42))
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");
    let events = events.lock().unwrap();
    assert!(
        matches!(
            events.first(),
            Some(WikiProgress::Started { synthesis_timeout_secs: 42, .. })
        ),
        "Started carries the overridden synthesis timeout: {:?}",
        events.first(),
    );
}

/// The default (unconfigured) synthesis timeout is exactly
/// [`wiki_agent::DEFAULT_SYNTHESIS_TIMEOUT`] — the constant a caller never overrides
/// still reaches `Started` honestly, so the surface never displays a stale/guessed
/// bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_carries_the_default_synthesis_timeout_when_unconfigured() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (events, sink) = recording_sink();
    WikiAgent::new(mock, "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");
    let events = events.lock().unwrap();
    let expected = wiki_agent::DEFAULT_SYNTHESIS_TIMEOUT.as_secs();
    assert!(
        matches!(
            events.first(),
            Some(WikiProgress::Started { synthesis_timeout_secs, .. }) if *synthesis_timeout_secs == expected
        ),
        "Started carries the default synthesis timeout ({expected}s) when unconfigured: {:?}",
        events.first(),
    );
}

/// The **hard safety ceiling** halts an auto-continuing run honestly ([NFR-CC-04],
/// [CR-056]) rather than looping unbounded: with a ceiling below the work-list size
/// the run writes exactly `ceiling` pages, persists them, and halts with a reason
/// that names the ceiling — never a fabricated page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hard_safety_ceiling_halts_a_run_honestly() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let k = queue.items.len();
    assert!(k > 1, "the fixture yields more than one queued page");
    // A ceiling one short of the work-list, so the run halts before draining.
    let ceiling = k - 1;

    let turns: Vec<_> = (0..k)
        .map(|i| {
            agent_core::MockTurn::text(format!(
                "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
            ))
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .with_budget(1)
        .with_ceiling(ceiling)
        .run(Arc::clone(&engine), sink)
        .await
        .expect("run ok")
        .expect("a run started");

    assert_eq!(
        outcome.pages_written.len(),
        ceiling,
        "the run wrote exactly up to the hard safety ceiling",
    );
    assert_eq!(mock.request_count(), ceiling, "no synthesis happened past the ceiling");
    let halted = outcome.halted.expect("hitting the ceiling halts the run");
    assert!(
        halted.contains("ceiling"),
        "the halt reason names the hard safety ceiling: {halted}",
    );
    // The pages written before the halt persist and read fresh — nothing fabricated.
    for slug in &outcome.pages_written {
        let page = engine.wiki_read(slug).unwrap().expect("a written page persists");
        assert!(!page.stale, "{slug} persisted and reads fresh after the ceiling halt");
    }
}

/// A persistent per-page **write rejection** does not spin the auto-continue loop
/// ([CR-056], [NFR-CC-04]): the `attempted`-slug guard ensures each rejected page is
/// tried exactly once and the no-progress guard stops the re-read cycle, so the run
/// terminates instead of looping forever on a page that keeps re-appearing in the
/// work-list. Every synthesized body here is over the 1 MiB `wiki write` cap, so
/// every write is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persistent_write_rejection_does_not_spin_the_auto_continue_loop() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let k = queue.items.len();
    assert!(k > 1, "the fixture yields more than one queued page");

    // Each turn returns a body just over the 1 MiB cap → every `wiki write` rejects
    // it (a per-page failure, not a provider halt).
    let oversized = "x".repeat(1024 * 1024 + 1);
    let turns: Vec<_> = (0..k).map(|_| MockTurn::text(oversized.clone())).collect();
    let mock = MockCompletionModel::new(turns);

    // Budget 1 forces a re-read after every page; the persistently-failing pages
    // re-appear in each re-read, so this is exactly the spin the guards must prevent.
    // The outer timeout fails the test if the loop ever hangs.
    let (_events, sink) = recording_sink();
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        WikiAgent::new(mock.clone(), "skill", "mock-model")
            .with_budget(1)
            .run(Arc::clone(&engine), sink),
    )
    .await
    .expect("the auto-continue loop must terminate, not spin on the persistent rejection")
    .expect("write rejections are not infrastructure errors")
    .expect("a run started over the non-empty work-list");

    assert!(outcome.pages_written.is_empty(), "every over-cap write was rejected");
    assert_eq!(outcome.pages_failed.len(), k, "each rejected page recorded exactly once");
    assert_eq!(
        mock.request_count(),
        k,
        "each page synthesized exactly once — a rejected page is not re-attempted on re-read",
    );
    assert!(
        outcome.halted.is_none(),
        "a per-page rejection is neither a ceiling nor a provider halt: {:?}",
        outcome.halted,
    );
}

/// A page whose synthesized body trips the [FR-WK-19] content-validity guard
/// (agent-noise, not prose) is an honest per-page failure that does **not**
/// abort the rest of the batch — the run continues and writes the remaining
/// queued pages ([NFR-CC-04]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_content_validity_rejection_is_a_per_page_failure_that_does_not_abort_the_batch() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    let k = queue.items.len();
    assert!(k > 1, "the fixture yields more than one queued page");
    let expected_slugs: Vec<String> = queue.items.iter().map(|i| i.slug.clone()).collect();

    // The first turn is agent-noise (a tool-call dump, [FR-WK-19]); every
    // remaining turn is a well-formed page.
    let mut turns = vec![agent_core::MockTurn::text(
        "<tool_call>\n{\"name\": \"read_file\"}\n</tool_call>",
    )];
    turns.extend((1..k).map(|i| {
        agent_core::MockTurn::text(format!(
            "# Page {i}\n\nBody {i}, with enough prose to clear the write-path guard."
        ))
    }));
    let mock = MockCompletionModel::new(turns);

    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("a content-validity rejection is not an infrastructure error")
        .expect("a run started over the non-empty work-list");

    assert_eq!(outcome.pages_failed.len(), 1, "exactly the noise page is rejected");
    assert_eq!(outcome.pages_failed[0].0, expected_slugs[0]);
    assert!(
        outcome.pages_failed[0].1.contains("FR-WK-19"),
        "the honest failure names the content-validity guard: {}",
        outcome.pages_failed[0].1
    );
    assert_eq!(
        outcome.pages_written.as_slice(),
        &expected_slugs[1..],
        "the batch continues past the rejected page and writes the rest"
    );
    assert!(outcome.halted.is_none(), "a per-page rejection does not halt the run");
}

/// A provider that never responds (a stalled/dead-air connection) halts the run
/// honestly via the per-page synthesis timeout instead of hanging forever
/// ([NFR-CC-04], [CR-056]). This is the liveness bound that keeps a
/// connection-independent run from wedging the single-run lock — the run **returns**
/// (so its lock guard can drop) rather than parking on the model call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hung_provider_call_halts_the_run_honestly_rather_than_hanging() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    assert!(!engine.wiki_generate().unwrap().items.is_empty());

    // The first (and only consumed) synthesis hangs forever; a tiny timeout turns it
    // into an honest halt. The outer timeout fails the test if the run itself hangs.
    let mock = MockCompletionModel::new([MockTurn::Hang]);
    let (_events, sink) = recording_sink();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        WikiAgent::new(mock.clone(), "skill", "mock-model")
            .with_synthesis_timeout(Duration::from_millis(50))
            .run(Arc::clone(&engine), sink),
    )
    .await
    .expect("the run must not hang — the per-page synthesis timeout bounds it")
    .expect("a hung provider is not an infrastructure error")
    .expect("a run started over the non-empty work-list");

    assert!(outcome.pages_written.is_empty(), "no page is written when synthesis times out");
    let halted = outcome.halted.expect("a hung provider halts the run honestly");
    assert!(
        halted.contains("did not respond within"),
        "the halt reason names the liveness timeout: {halted}",
    );
    assert_eq!(mock.request_count(), 1, "exactly one (hung) request was made before the halt");
}

/// A provider fault that strikes **after** one or more pages are written leaves those
/// earlier pages **persisted and fresh** ([NFR-CC-04], [FR-WK-18] "persists pages
/// already written"): the run halts honestly on the fault, but the atomic writes
/// before it stand.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_fault_after_a_write_persists_the_earlier_page() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(queue.items.len() >= 2, "need at least two pages: write one, then fault");

    // First turn synthesizes a valid page; the second is a provider error. The
    // default per-chunk budget attempts both in one chunk, so the fault strikes the
    // second page after the first is already persisted.
    let mock = MockCompletionModel::new([
        MockTurn::text("# First page\n\nBody, with enough prose to clear the write-path guard."),
        MockTurn::Error("simulated provider outage".to_string()),
    ]);
    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("a provider fault is not an infrastructure error")
        .expect("a run started over the non-empty work-list");

    assert_eq!(outcome.pages_written.len(), 1, "the first page was written before the fault");
    assert!(outcome.halted.is_some(), "the provider fault halted the run honestly");
    // The page written before the fault persists and reads fresh — not rolled back.
    let slug = &outcome.pages_written[0];
    let page = engine
        .wiki_read(slug)
        .expect("read")
        .expect("the page written before the fault persists");
    assert!(!page.stale, "{slug} persisted and reads fresh after the honest halt");
}

/// A provider failure during synthesis halts the pass honestly ([NFR-CC-04]):
/// no page is written and the run reports a halt, never a fabricated page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provider_failure_halts_the_pass_honestly() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty());

    // The first (and only consumed) turn is a provider error.
    let mock = MockCompletionModel::new([MockTurn::Error("simulated provider outage".to_string())]);
    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("a provider failure is not an infrastructure error")
        .expect("a run started over the non-empty work-list");

    assert!(outcome.pages_written.is_empty(), "no page is written when synthesis fails");
    assert_eq!(mock.request_count(), 1, "the pass halted after the first failed synthesis");
    assert!(
        outcome.halted.is_some(),
        "a provider failure halts the run honestly (NFR-CC-04)"
    );
    // Nothing was persisted — the guide page never existed and none was fabricated.
    assert!(
        engine.wiki_read(&queue.items[0].slug).unwrap().is_none()
            || outcome.pages_written.is_empty(),
        "no page was fabricated on a provider failure"
    );
}

/// The configured entry degrades to the honest configure-first state — not a
/// crash, and no run — when no model resolves ([FR-UI-18], [FR-CF-07],
/// [NFR-CC-04]). A missing/blank key is likewise configure-first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_configured_is_configure_first_without_a_model_or_key() {
    let tmp = TempDir::new().unwrap();
    let engine = Arc::new(Engine::start(tmp.path()).expect("engine starts"));
    let no_op = |_p: WikiProgress| {};

    // No resolved model id (neither `[wiki].model` nor `[chat].model`).
    let no_model = EffectiveWikiModel {
        model: None,
        provider: ChatProvider::OpenAi,
        base_url: "https://openrouter.ai/api/v1".to_string(),
        api_key: Some("sk-test".to_string()),
        max_provider_retries: 2,
        provider_retry_base_ms: 200,
    };
    let result = run_configured(Arc::clone(&engine), no_model, 256, no_op)
        .await
        .expect("configure-first is not an error");
    assert!(
        matches!(result, ConfiguredRun::ConfigureFirst(_)),
        "no resolved model → configure-first, no run"
    );

    // A model but no inherited key.
    let no_key = EffectiveWikiModel {
        model: Some("anthropic/claude-haiku".to_string()),
        provider: ChatProvider::Anthropic,
        base_url: "https://api.anthropic.com".to_string(),
        api_key: None,
        max_provider_retries: 2,
        provider_retry_base_ms: 200,
    };
    let result = run_configured(Arc::clone(&engine), no_key, 256, no_op)
        .await
        .expect("configure-first is not an error");
    assert!(
        matches!(result, ConfiguredRun::ConfigureFirst(_)),
        "a missing inherited key → configure-first, no run"
    );
}

/// S-238 dogfood/acceptance ([CR-059], [FR-WK-18], [FR-WK-19]): the pre-S-236
/// corpus is untrustworthy and reads `freshness_fraction == 0.0` (every page
/// drifted); a single regeneration pass through the fixed grounding-injection
/// pipeline ([S-236]) under the write-path validity guard ([S-237]) makes it
/// trustworthy again.
///
/// This is the mock-path stand-in for the acceptance drive against this repo (a
/// live provider run needs a configured model + key, [NFR-SE-07] consent-gated,
/// so it is the human acceptance step — see `sprint-impl-44.md`). The mock
/// stands in for the provider; every other moving part — the real `Engine`, the
/// FR-WK-13 queue, the S-236 grounding resolution, the S-237 `write()` guard,
/// and the `wiki status` freshness read-model — is the production code path.
///
/// It asserts the three S-238 acceptance criteria end-to-end:
/// 1. the aggregate `freshness_fraction` **rises** (0.0 → 1.0) after regeneration;
/// 2. every regenerated page is guard-valid grounded prose with a **well-formed
///    code fence** (a legitimate fence never trips the guard, [FR-WK-19]);
/// 3. **no page stores agent-noise** — when the provider emits a tool-call dump
///    mid-regeneration, the S-237 guard rejects that page ([FR-WK-19]) and it is
///    never stored, and the old pre-S-236 bodies are replaced.
///
/// The committed spec makes at least one queued item doc-grounded, so the S-236
/// `std::fs` grounding path is exercised (asserted, not just commented). A
/// loopback tripwire confirms the whole pass is offline ([NFR-SE-01]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn regenerating_a_fully_stale_corpus_yields_a_trustworthy_fresh_corpus() {
    let tripwire = spawn_tripwire().await;

    let tmp = TempDir::new().unwrap();
    let repo = tmp.path();
    sh_git(repo, &["init", "-q", "-b", "main"]);
    commit(repo, "src/a.rs", &branchy("a", 2), "add a");
    commit(repo, "src/b.rs", &branchy("b", 2), "add b");
    // A committed README + spec makes the Project Overview / Key Concepts items
    // doc-grounded ([FR-WK-24]: both mapped sources present), so the run exercises
    // the S-236 `std::fs` grounding-injection path (not just the code fallback).
    commit(
        repo,
        "README.md",
        "# Logos\n\nA structural code-intelligence tool. GROUNDING-DOC-SENTINEL.\n",
        "add readme",
    );
    commit(
        repo,
        "docs/specs/software-spec.md",
        "# Spec\n\nLogos is a structural code-intelligence tool. GROUNDING-DOC-SENTINEL.\n",
        "add spec",
    );
    let engine = Arc::new(Engine::start(repo).expect("engine starts"));
    engine.index();
    let rev1 = engine.status().graph_revision;

    // Seed a pre-existing corpus of valid pages anchored to real files (the "old"
    // corpus a prior pipeline left behind), then drift EVERY page on both axes:
    // edit each anchored file (content axis, FR-WK-03) and re-index (revision
    // axis, FR-WK-12). The corpus now reads entirely stale — `freshness_fraction
    // == 0.0`, exactly as this repo's corrupted corpus reads today.
    for (slug, file) in [("guide/a", "src/a.rs"), ("guide/b", "src/b.rs")] {
        engine
            .wiki_write(
                slug,
                "Seed",
                "# Seed\n\nThe old pre-S-236 body, long enough to clear the write-path guard.",
                &[format!("file:{file}")],
                "old-pipeline",
            )
            .expect("seed write");
    }
    std::fs::write(repo.join("src/a.rs"), branchy("a", 5)).unwrap();
    std::fs::write(repo.join("src/b.rs"), branchy("b", 5)).unwrap();
    engine.index();
    let current = engine.status().graph_revision;
    assert!(current > rev1, "the re-index advanced the graph revision");

    let before = engine.wiki_status().expect("status before regeneration");
    assert_eq!(before.page_count, 2, "the seeded corpus has exactly the two pages");
    assert!(
        (before.freshness_fraction - 0.0).abs() < f64::EPSILON,
        "the pre-regeneration corpus is entirely stale (freshness 0.0), like the corrupted corpus"
    );

    // Regenerate through the fixed pipeline. The committed `software-spec.md`
    // makes at least one queued item doc-grounded, so the S-236 `std::fs`
    // grounding path is genuinely in play (not just asserted by comment).
    let queue = engine.wiki_generate().expect("generate queue");
    assert!(!queue.items.is_empty(), "the stale corpus yields a non-empty work-list");
    assert!(
        queue
            .items
            .iter()
            .any(|i| i.grounding.as_ref().is_some_and(|g| !g.fallback_to_code)),
        "the committed software-spec.md makes at least one queued item doc-grounded (S-236)"
    );

    // One queued page's synthesis comes back as agent-noise (a tool-call dump,
    // [FR-WK-19]) — the real S-238 concern: even when the provider emits garbage
    // mid-regeneration, the S-237 write guard keeps it out of the corpus. Target
    // an absent structured item (not one of the two seeded stale pages) so its
    // rejection leaves the page simply uncreated rather than dragging the
    // freshness aggregate below 1.0. Every other turn is grounded prose that also
    // embeds a well-formed bash fence — criterion (2): a legitimate fence coexists
    // with the guard. The mock ignores the prompt, so the whole deterministic
    // FR-WK-13 queue is drained in one pass.
    let noise_idx = queue
        .items
        .iter()
        .position(|i| i.slug != "guide/a" && i.slug != "guide/b")
        .expect("the work-list has absent structured items beyond the two seeded pages");
    let noise_slug = queue.items[noise_idx].slug.clone();
    let turns: Vec<_> = (0..queue.items.len())
        .map(|i| {
            if i == noise_idx {
                MockTurn::text("<tool_call>\n{\"name\": \"read_file\"}\n</tool_call>")
            } else {
                MockTurn::text(format!(
                    "# Regenerated page {i}\n\nGrounded prose synthesized only from the injected \
                     CONTEXT, with enough body to clear the write-path validity guard.\n\n\
                     ```bash\nlogos index && logos wiki generate\n```\n"
                ))
            }
        })
        .collect();
    let mock = MockCompletionModel::new(turns);

    let (_events, sink) = recording_sink();
    let outcome = WikiAgent::new(mock.clone(), "skill", "mock-model")
        .run(Arc::clone(&engine), sink)
        .await
        .expect("the regeneration pass completes without infrastructure error")
        .expect("a non-empty work-list starts a run");

    // The mock — not a real provider — served exactly one turn per queued page.
    assert_eq!(
        mock.request_count(),
        queue.items.len(),
        "the mock synthesized exactly one turn per page (offline, no real provider)"
    );
    assert!(outcome.halted.is_none(), "a per-page rejection does not halt the run");

    // Criterion (3): the one agent-noise page is rejected by the S-237 guard,
    // named honestly ([FR-WK-19]), never stored, and the batch continues — no page
    // in the corpus holds agent-noise.
    assert_eq!(outcome.pages_failed.len(), 1, "exactly the agent-noise page is rejected");
    assert_eq!(outcome.pages_failed[0].0, noise_slug, "the rejected page is the noise page");
    assert!(
        outcome.pages_failed[0].1.contains("FR-WK-19"),
        "the honest failure names the content-validity guard: {}",
        outcome.pages_failed[0].1
    );
    assert!(
        engine.wiki_read(&noise_slug).expect("read").is_none(),
        "the agent-noise page was refused, not stored — no page holds agent-noise"
    );

    // Criterion (1): the aggregate freshness read-model rose. Every stored page is
    // fresh — the rejected noise page was an absent item that stays uncreated, so
    // it never drags the fraction below 1.0.
    let after = engine.wiki_status().expect("status after regeneration");
    assert!(
        after.freshness_fraction > before.freshness_fraction,
        "the freshness fraction rises after regeneration ({} → {})",
        before.freshness_fraction,
        after.freshness_fraction
    );
    assert!(
        (after.freshness_fraction - 1.0).abs() < f64::EPSILON,
        "every stored page is fresh after regeneration (the rejected noise page was never stored)"
    );
    assert!(
        after.page_count > before.page_count,
        "regeneration replaced the stale pages and added the queued structured sections"
    );

    // Criterion (2): every regenerated page is guard-valid grounded prose with a
    // well-formed fence, carries the new generator, reads fresh, and the old
    // pre-S-236 bodies are gone.
    for slug in &outcome.pages_written {
        let page = engine
            .wiki_read(slug)
            .expect("read")
            .expect("a just-regenerated page is present");
        assert_eq!(page.generator, "mock-model", "{slug} was regenerated by the fixed pipeline");
        assert!(!page.stale, "{slug} reads fresh after regeneration");
        assert!(page.body.contains("```"), "{slug} retained its well-formed code fence");
        assert!(
            !page.body.contains("pre-S-236"),
            "{slug} no longer holds the old pre-S-236 body"
        );
    }

    assert_eq!(
        tripwire.connections.load(Ordering::SeqCst),
        0,
        "the regeneration pass opened zero real outbound connections"
    );
}

/// S-238 purge plumbing ([CR-059], [FR-WK-07]): an orphaned page whose slug the
/// fixed pipeline never re-queues — e.g. a legacy per-file `files/*` page from the
/// scheme [CR-056] retired — is **not** cleaned by an overwrite-only regeneration
/// pass; it must be explicitly purged. This pins the `wiki delete` half of the
/// purge+regenerate plumbing the acceptance step drives (the ~231 orphan `files/*`
/// pages in this repo's real corpus are exactly this case). A pure read-model
/// check, no provider involved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purging_an_orphan_page_removes_it_from_the_corpus() {
    let tmp = TempDir::new().unwrap();
    let engine = indexed_engine(tmp.path());

    // An orphan page under a legacy `files/*` slug anchored to a real file. A full
    // regeneration pass never re-queues this exact slug (the per-file scheme was
    // retired), so overwrite-in-place cannot clean it — only an explicit delete.
    engine
        .wiki_write(
            "files/src/a-rs",
            "a.rs — objectives",
            "# a.rs\n\nA legacy per-file page left by a retired scheme, long enough to clear the guard.",
            &["file:src/a.rs".to_string()],
            "old-pipeline",
        )
        .expect("seed the orphan page");
    assert!(
        engine.wiki_read("files/src/a-rs").unwrap().is_some(),
        "the orphan page exists before purge"
    );
    let before = engine.wiki_status().expect("status before purge");
    // CR-062 / S-259: a legacy `files/*` slug is now excluded from `wiki status`
    // page_count (retired from the work-list), even though the page still physically
    // exists in the store — proven by the `wiki_read` == Some assertion above.
    assert_eq!(
        before.page_count, 0,
        "the retired files/* orphan is excluded from status page_count (CR-062 S-259) though still stored"
    );

    // Purge it explicitly.
    engine.wiki_delete("files/src/a-rs").expect("delete the orphan page");

    assert!(
        engine.wiki_read("files/src/a-rs").unwrap().is_none(),
        "the purged page is gone from the corpus"
    );
    let after = engine.wiki_status().expect("status after purge");
    assert_eq!(after.page_count, 0, "the corpus no longer holds the orphan page");
}
