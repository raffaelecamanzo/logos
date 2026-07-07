//! The wiki-agent runner: the bounded, queue-driven generation loop ([FR-WK-18],
//! [ADR-42]).
//!
//! [`WikiAgent`] wraps a `rig` [`CompletionModel`] (the mock offline, a real
//! provider in production) and the embedded `logos-wiki` skill body used as the
//! system prompt. [`WikiAgent::run`] reads the deterministic FR-WK-13 queue,
//! synthesizes each page with a fresh tool-less agent, and persists it through the
//! unchanged `wiki write` contract — emitting per-page progress for the surface to
//! stream ([FR-UI-19], wired by S-178).
//!
//! [FR-WK-18]: ../../../docs/specs/requirements/FR-WK-18.md
//! [FR-UI-19]: ../../../docs/specs/requirements/FR-UI-19.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md

use std::collections::HashSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use agent_core::rig::agent::AgentBuilder;
use agent_core::rig::completion::{CompletionModel, Prompt};
use agent_core::{classify_provider_error, ToolBudget};
use anyhow::{Context, Result};
use logos_core::wiki::{Freshness, GenerationItem, WikiGenerationQueue};
use logos_core::Engine;

use crate::grounding;
pub use crate::grounding::DEFAULT_GROUNDING_BUDGET;

/// The default **per-chunk page budget** ([ADR-42], [CR-056]) when a caller does
/// not set one: the number of pages one auto-continue slice writes before the run
/// re-reads the deterministic FR-WK-13 queue and continues ([NFR-RA-06]). It bounds
/// a single chunk, **not** the whole run — the run auto-continues across chunks
/// until the work-list drains, bounded only by [`DEFAULT_RUN_CEILING`].
/// Overridable via [`WikiAgent::with_budget`].
pub const DEFAULT_RUN_BUDGET: usize = 256;

/// The default **hard safety ceiling** on total pages a single run may write
/// ([NFR-CC-04], [CR-056]): a backstop that halts the auto-continue loop honestly
/// rather than letting a pathological work-list loop unbounded. Set well above any
/// real work-list (the pre-prune cold-start list was ~1336 pages, [S-221]), so it
/// never truncates a legitimate drain; a run that hits it halts with an honest
/// reason. Overridable via [`WikiAgent::with_ceiling`].
pub const DEFAULT_RUN_CEILING: usize = 5000;

/// The default **per-page synthesis liveness timeout** ([NFR-CC-04], [CR-056]): the
/// longest a single page's model call may run before the run halts honestly rather
/// than hanging. Because a run is now owned by application state (not the SSE body),
/// nothing else bounds a stalled/dead-air provider connection — without this a hung
/// call would keep the run alive forever, wedging the single-run lock for the
/// server's lifetime. Set generously so it never trips a legitimately slow
/// synthesis, only a genuine hang; overridable via
/// [`WikiAgent::with_synthesis_timeout`].
pub const DEFAULT_SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(180);

/// A per-page progress event emitted across a generation run ([FR-WK-18]). The
/// S-178 web surface maps these onto the SSE stream; the `event`-tagged
/// serialization keeps the wire form self-describing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum WikiProgress {
    /// The run started over a non-empty work-list of `total` pages.
    Started {
        /// The number of queued pages this run will attempt, in order.
        total: usize,
        /// The configured per-page synthesis liveness timeout, in whole seconds
        /// ([`DEFAULT_SYNTHESIS_TIMEOUT`], [`WikiAgent::with_synthesis_timeout`]) —
        /// carried so the surface can make the bound VISIBLE (a long-running page is
        /// a liveness guard counting down, not an unexplained stall) without the
        /// frontend duplicating the value ([CR-059], [S-239]).
        synthesis_timeout_secs: u64,
    },
    /// Synthesis of one page began.
    PageStarted {
        /// The target wiki slug.
        slug: String,
        /// The page title.
        title: String,
        /// The 1-based position of this page in the queue.
        index: usize,
        /// The total queued page count.
        total: usize,
    },
    /// One page was synthesized and persisted via [`wiki write`](Engine::wiki_write).
    PageWritten {
        /// The target wiki slug.
        slug: String,
        /// The number of anchors resolved and stored at write time.
        anchor_count: usize,
        /// `true` when the write replaced an existing page (a refresh).
        replaced: bool,
    },
    /// One page's write was rejected (e.g. an over-cap body or an unknown anchor);
    /// the run continues with the next page ([NFR-CC-04] — no fabrication).
    PageFailed {
        /// The target wiki slug.
        slug: String,
        /// The honest failure reason.
        error: String,
    },
    /// The run halted early — a spent per-run budget or a provider failure —
    /// reported honestly ([NFR-CC-04]). Pages already written persist.
    Halted {
        /// Which limit or failure halted the run.
        reason: String,
    },
    /// The run finished (whether completed or halted); the terminal event.
    Completed {
        /// Pages persisted this run.
        pages_written: usize,
        /// Pages whose write was rejected this run.
        pages_failed: usize,
    },
}

/// The outcome of a generation run ([FR-WK-18], [NFR-CC-04]).
///
/// [`WikiAgent::run`] returns `Ok(None)` for an empty work-list (no run started)
/// and `Ok(Some(WikiRunOutcome))` when a run happened; an `Err` is reserved for an
/// infrastructure failure (the queue could not be read, a task join failed).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct WikiRunOutcome {
    /// The slugs persisted this run, in queue order.
    pub pages_written: Vec<String>,
    /// The `(slug, reason)` of each page whose write was rejected.
    pub pages_failed: Vec<(String, String)>,
    /// The honest halt reason when the run stopped early (budget or provider);
    /// `None` when the whole queue was attempted.
    pub halted: Option<String>,
}

/// A single-purpose wiki generator over a `rig` [`CompletionModel`] ([ADR-42]).
///
/// Holds the completion model (cloned to build a fresh tool-less agent per page;
/// the mock shares its scripted state across clones, so successive pages consume
/// successive scripted turns), the system preamble (the embedded `logos-wiki`
/// skill body), the mandatory generator label recorded on each write, the
/// per-chunk page budget (the auto-continue slice size), and the hard safety
/// ceiling on total pages per run.
#[derive(Clone)]
pub struct WikiAgent<M> {
    model: M,
    preamble: String,
    generator: String,
    budget: usize,
    ceiling: usize,
    synthesis_timeout: Duration,
    grounding_budget: usize,
}

impl<M> WikiAgent<M>
where
    M: CompletionModel + Clone + 'static,
{
    /// A wiki-agent over `model` with `preamble` as its system prompt (the
    /// embedded `logos-wiki` skill body, [FR-WK-08]) and `generator` as the
    /// mandatory non-empty `wiki write` generator label ([FR-WK-02] — typically
    /// the resolved model id).
    pub fn new(
        model: M,
        preamble: impl Into<String>,
        generator: impl Into<String>,
    ) -> Self {
        Self {
            model,
            preamble: preamble.into(),
            generator: generator.into(),
            budget: DEFAULT_RUN_BUDGET,
            ceiling: DEFAULT_RUN_CEILING,
            synthesis_timeout: DEFAULT_SYNTHESIS_TIMEOUT,
            grounding_budget: DEFAULT_GROUNDING_BUDGET,
        }
    }

    /// Override the **per-chunk** page budget ([ADR-42], [CR-056]) — the number of
    /// pages written per auto-continue slice before the run re-reads the queue.
    pub fn with_budget(mut self, budget: usize) -> Self {
        self.budget = budget;
        self
    }

    /// Override the **hard safety ceiling** on total pages per run ([NFR-CC-04],
    /// [CR-056]) — the backstop that halts the auto-continue loop honestly rather
    /// than looping unbounded.
    pub fn with_ceiling(mut self, ceiling: usize) -> Self {
        self.ceiling = ceiling;
        self
    }

    /// Override the **per-page synthesis liveness timeout** ([NFR-CC-04], [CR-056])
    /// — the bound that turns a hung provider call into an honest halt so the run
    /// cannot wedge the single-run lock.
    pub fn with_synthesis_timeout(mut self, timeout: Duration) -> Self {
        self.synthesis_timeout = timeout;
        self
    }

    /// Override the **per-source grounding token budget** ([S-236], [ADR-51]) —
    /// the bound each resolved doc source / code-graph digest is truncated to
    /// before it is injected into the synthesis prompt, so a large source is
    /// summarized from a capped, deterministic excerpt rather than overflowing the
    /// model context ([NFR-RA-06]).
    pub fn with_grounding_budget(mut self, budget: usize) -> Self {
        self.grounding_budget = budget;
        self
    }

    /// Run a full generation pass over the deterministic FR-WK-13 queue,
    /// **auto-continuing** across per-chunk budget slices until the work-list drains
    /// ([CR-056], [ADR-42]).
    ///
    /// Reads the queue (a cheap, pure read on the blocking pool, [ADR-03]);
    /// **an empty work-list starts no run** (`Ok(None)`) and makes no model call
    /// ([NFR-CC-04]). Otherwise it emits `Started { total, synthesis_timeout_secs }` with the initial
    /// work-list size (the cumulative denominator for progress, [FR-UI-19]) and,
    /// for each queued page in order: synthesizes the body with a fresh tool-less
    /// agent, resolves the write-time anchors, and persists via the unchanged
    /// [`wiki write`](Engine::wiki_write) contract. Every [`WikiProgress`] transition
    /// is emitted through `sink`.
    ///
    /// After each **per-chunk budget** slice ([`with_budget`](Self::with_budget))
    /// the run **re-reads** the deterministic queue — which now reflects the pages
    /// just written (they are fresh, so they leave the work-list, [NFR-RA-06]) — and
    /// auto-continues into the next chunk, with **no** manual re-trigger, until the
    /// re-read yields an empty work-list (drained). Two invariants keep the loop
    /// honest and finite ([NFR-CC-04]):
    ///
    /// - a **hard safety ceiling** ([`with_ceiling`](Self::with_ceiling)) on total
    ///   pages attempted per run halts the loop with an honest reason rather than
    ///   looping unbounded;
    /// - a chunk that attempts **no new page** (every remaining queue item was
    ///   already attempted this run — i.e. a persistent per-page write failure that
    ///   re-appears in the work-list) stops the loop, so a rejected page can never
    ///   spin the re-read cycle forever.
    ///
    /// # Errors
    /// Returns an `Err` only on an infrastructure failure — the queue read or a
    /// `spawn_blocking` join failing. A provider failure or a per-page write
    /// rejection is captured in the [`WikiRunOutcome`] / progress stream, never a
    /// hard error ([NFR-CC-04]).
    pub async fn run(
        &self,
        engine: Arc<Engine>,
        sink: impl Fn(WikiProgress),
    ) -> Result<Option<WikiRunOutcome>> {
        // The FR-WK-13 no-content short-circuit: an empty work-list spawns nothing
        // and authors no prose ([NFR-CC-04], [FR-WK-18] acceptance).
        let mut queue = read_queue(&engine).await?;
        if queue.items.is_empty() {
            return Ok(None);
        }

        // The cumulative denominator: the whole initial work-list, so progress reads
        // "N of M" across every auto-continued chunk ([FR-UI-19]). Generation never
        // *adds* page-worthy entities (it does not change the graph revision), so the
        // work-list only shrinks — this total stays honest across re-reads.
        let total = queue.items.len();
        // The hard safety ceiling on total pages attempted this run — the honest-halt
        // primitive ([NFR-CC-04], [ADR-42]) that bounds the auto-continue loop.
        let ceiling = ToolBudget::new(self.ceiling);
        // The per-chunk budget slice size (at least one, so a zero budget still
        // makes forward progress rather than spinning empty chunks).
        let chunk_budget = self.budget.max(1);
        let mut outcome = WikiRunOutcome::default();
        // Slugs attempted this run — so a persistent per-page failure that re-appears
        // in a re-read is not re-attempted (and cannot spin the loop forever).
        let mut attempted: HashSet<String> = HashSet::new();
        // The cumulative 1-based page index across all chunks (for progress).
        let mut index = 0usize;
        sink(WikiProgress::Started {
            total,
            synthesis_timeout_secs: self.synthesis_timeout.as_secs(),
        });

        'run: loop {
            let mut in_chunk = 0usize;
            let mut progressed = false;
            for item in &queue.items {
                // Skip a slug already attempted this run — after a write it has left
                // the work-list, so a re-appearance is a persistent per-page failure.
                if attempted.contains(&item.slug) {
                    continue;
                }
                // The hard safety ceiling ([NFR-CC-04]): halt honestly rather than
                // loop unbounded. Charged before the attempt, so the cap bounds
                // attempts, not just successes.
                if ceiling.charge().is_err() {
                    let reason = format!(
                        "hard safety ceiling of {} pages per run reached; \
                         {} page(s) written, work-list not fully drained",
                        self.ceiling,
                        outcome.pages_written.len()
                    );
                    outcome.halted = Some(reason.clone());
                    sink(WikiProgress::Halted { reason });
                    break 'run;
                }
                attempted.insert(item.slug.clone());
                progressed = true;
                in_chunk += 1;
                index += 1;

                sink(WikiProgress::PageStarted {
                    slug: item.slug.clone(),
                    title: item.title.clone(),
                    index,
                    total,
                });

                // Resolve this item's grounding content in-binary ([S-236], [ADR-51])
                // — read the named `docs/` source(s) or build a code-graph digest, on
                // the blocking pool — so the tool-less agent writes from provided
                // context, not a bare "read the source" directive it cannot honor. A
                // resolution *join* failure is an infrastructure error (`?`); an
                // unreadable source degrades to an honest inline marker, never a halt.
                let grounding = self.resolve_grounding(&engine, item).await?;

                // Synthesize the page body with a fresh tool-less agent whose system
                // prompt is the embedded skill body. A provider failure halts the pass
                // honestly — the remaining queue is not attempted, and pages already
                // written persist (each write is atomic).
                let body = match self.synthesize(item, &grounding).await {
                    Ok(body) => body,
                    Err(reason) => {
                        outcome.halted = Some(reason.clone());
                        sink(WikiProgress::Halted { reason });
                        break 'run;
                    }
                };

                // Resolve the write-time anchors, then persist through the unchanged
                // write contract — HEAD and built-at-revision are captured by
                // `wiki_write` itself ([FR-WK-02], [FR-WK-12]).
                let anchors = self.resolve_anchors(&engine, item).await?;
                let write = {
                    let engine = Arc::clone(&engine);
                    let slug = item.slug.clone();
                    let title = item.title.clone();
                    let generator = self.generator.clone();
                    tokio::task::spawn_blocking(move || {
                        engine.wiki_write(&slug, &title, &body, &anchors, &generator)
                    })
                    .await
                    .context("the wiki write task failed")?
                };

                match write {
                    Ok(summary) => {
                        outcome.pages_written.push(item.slug.clone());
                        sink(WikiProgress::PageWritten {
                            slug: item.slug.clone(),
                            anchor_count: summary.anchor_count,
                            replaced: summary.replaced,
                        });
                    }
                    Err(e) => {
                        // A rejected write (over-cap body, unknown anchor) is a
                        // per-page honest failure; the store is byte-identical and the
                        // run continues ([FR-WK-02], [NFR-CC-04]).
                        let error = e.to_string();
                        outcome.pages_failed.push((item.slug.clone(), error.clone()));
                        sink(WikiProgress::PageFailed {
                            slug: item.slug.clone(),
                            error,
                        });
                    }
                }

                // Per-chunk budget slice complete → break out to re-read the queue
                // and auto-continue into the next chunk ([CR-056]).
                if in_chunk >= chunk_budget {
                    break;
                }
            }

            // A chunk that attempted no new page means every remaining work-list item
            // was already attempted this run (persistent per-page failures). The store
            // cannot advance further, so stop rather than spin the re-read cycle.
            if !progressed {
                break;
            }

            // Auto-continue ([CR-056]): re-read the deterministic queue, which now
            // reflects the pages written this chunk ([NFR-RA-06]). An empty re-read
            // means the work-list drained.
            queue = read_queue(&engine).await?;
            if queue.items.is_empty() {
                break;
            }
        }

        sink(WikiProgress::Completed {
            pages_written: outcome.pages_written.len(),
            pages_failed: outcome.pages_failed.len(),
        });
        Ok(Some(outcome))
    }

    /// Synthesize one page's Markdown body with a fresh tool-less `rig` agent whose
    /// system prompt is the embedded skill body — the planner's build pattern
    /// ([ADR-41]). A provider failure is classified into its full source chain
    /// (transport vs HTTP-status vs auth) so the halt reason is legible, never a
    /// flattened `to_string` ([FR-UI-24]).
    ///
    /// The call is bounded by the per-page synthesis timeout
    /// ([`with_synthesis_timeout`](Self::with_synthesis_timeout)): a stalled/dead-air
    /// provider that never responds is turned into an honest halt reason rather than
    /// hanging the run forever ([NFR-CC-04], [CR-056]) — essential now that a run is
    /// owned by application state, so a hung call can no longer be cancelled by a
    /// client disconnect and would otherwise wedge the single-run lock.
    ///
    /// `grounding` is the pre-resolved source content ([S-236], [ADR-51]) injected
    /// into the prompt so the tool-less agent writes only from provided context.
    async fn synthesize(
        &self,
        item: &GenerationItem,
        grounding: &str,
    ) -> std::result::Result<String, String> {
        let prompt = render_item_prompt(item, grounding);
        let agent = AgentBuilder::new(self.model.clone())
            .preamble(&self.preamble)
            .build();
        match tokio::time::timeout(self.synthesis_timeout, agent.prompt(prompt.as_str())).await {
            Ok(Ok(body)) => Ok(body),
            Ok(Err(e)) => Err(classify_provider_error(&e).to_string()),
            Err(_elapsed) => Err(format!(
                "the provider did not respond within {:?} — halting honestly rather \
                 than hanging the run",
                self.synthesis_timeout
            )),
        }
    }

    /// Resolve one queue item's grounding content in-binary ([S-236], [ADR-51]) on
    /// the blocking pool ([ADR-03]).
    ///
    /// The resolution itself ([`grounding::resolve`]) is a synchronous, local-only
    /// read (`docs/` files and the local code graph) that never errors — an
    /// unreadable source degrades to an honest inline marker ([NFR-CC-04]). This
    /// wrapper only surfaces a `spawn_blocking` **join** failure as an
    /// infrastructure error, matching [`resolve_anchors`](Self::resolve_anchors).
    /// Because every touch is local I/O, the offline posture is unchanged
    /// ([NFR-SE-01]).
    async fn resolve_grounding(
        &self,
        engine: &Arc<Engine>,
        item: &GenerationItem,
    ) -> Result<String> {
        let engine = Arc::clone(engine);
        let item = item.clone();
        let budget = self.grounding_budget;
        tokio::task::spawn_blocking(move || grounding::resolve(&engine, &item, budget))
            .await
            .context("the wiki grounding-resolution task failed")
    }

    /// Resolve the write-time anchors for one queue item ([FR-WK-02]).
    ///
    /// An unanchored-entity / per-file item carries its `"<kind>:<key>"` anchor
    /// directly. A stale/missing existing-page refresh carries **no** queue anchor
    /// (the work-list page row has none, [FR-WK-13]); we re-read the page and
    /// re-supply its still-present anchors so the rewrite stays anchored and reads
    /// fresh on the content axis ([FR-WK-03]). A gone (`Missing`) anchor is dropped
    /// — re-supplying it would be rejected as an unknown anchor ([FR-WK-02]).
    async fn resolve_anchors(
        &self,
        engine: &Arc<Engine>,
        item: &GenerationItem,
    ) -> Result<Vec<String>> {
        if !item.anchor.is_empty() {
            return Ok(vec![item.anchor.clone()]);
        }
        let engine = Arc::clone(engine);
        let slug = item.slug.clone();
        let page = tokio::task::spawn_blocking(move || engine.wiki_read(&slug))
            .await
            .context("the wiki read task failed")??;
        Ok(page
            .map(|p| {
                p.anchors
                    .iter()
                    .filter(|a| a.freshness != Freshness::Missing)
                    .map(|a| format!("{}:{}", a.kind, a.entity_id))
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Read the deterministic FR-WK-13 generation queue on the blocking pool ([ADR-03]).
///
/// The queue is a pure function of `wiki.db` + the current graph revision
/// ([NFR-RA-06]); running it on the blocking pool keeps the synchronous `Engine`
/// touch off `rig`'s async reactor. The auto-continue loop calls this once up front
/// and again after each chunk, so each re-read reflects the pages just written.
async fn read_queue(engine: &Arc<Engine>) -> Result<WikiGenerationQueue> {
    let engine = Arc::clone(engine);
    let queue = tokio::task::spawn_blocking(move || engine.wiki_generate())
        .await
        .context("the wiki generation-queue read task failed")??;
    Ok(queue)
}

/// Render the user prompt for one queue item ([FR-WK-13] → the page to write),
/// carrying the **pre-resolved grounding content** ([S-236], [ADR-51]).
///
/// The tool-less agent ([ADR-42]) cannot fetch anything, so the prompt injects the
/// resolved source content (doc excerpt or code-graph digest) directly and instructs
/// the model to write **only** from it — referencing no tools, commands, or file
/// reads it cannot perform. This is the fix for the ungrounded agent-noise the bare
/// "read the source" directive produced ([CR-059], [ADR-51]). A real provider reasons
/// over this against the skill-body system prompt; the mock ignores it and returns
/// its scripted turn, so the loop is exercised deterministically offline.
fn render_item_prompt(item: &GenerationItem, grounding: &str) -> String {
    let mut prompt = String::from(
        "Write the complete Markdown body for the wiki page described below, using ONLY \
         the material in the CONTEXT section. Do not reference, invoke, or invent any \
         tools, shell commands, file reads, or error messages; do not describe your \
         process, plans, or reasoning. Summarize faithfully — state only what the \
         context supports. Output ONLY the page body: no wrapping code fence, no \
         preamble, no sign-off.\n\n",
    );
    // `writeln!` to a String is infallible; the import is `std::fmt::Write`.
    let _ = writeln!(prompt, "Title: {}", item.title);
    let _ = writeln!(prompt, "Slug: {}", item.slug);
    let _ = writeln!(prompt, "Category: {} ({})", item.category.as_str(), item.reason);
    if !item.anchor.is_empty() {
        let _ = writeln!(prompt, "Anchor: {}", item.anchor);
    }
    prompt.push_str("\n----- CONTEXT (write only from this) -----\n");
    let grounding = grounding.trim();
    if grounding.is_empty() {
        prompt.push_str("(no source content could be resolved for this page)\n");
    } else {
        prompt.push_str(grounding);
        prompt.push('\n');
    }
    prompt.push_str("----- END CONTEXT -----\n");
    prompt
}

#[cfg(test)]
mod tests {
    //! End-to-end grounding-injection tests over a real `Engine` fixture ([S-236],
    //! [ADR-51]): they prove the resolved `docs/` content / code-graph digest
    //! actually reaches the synthesis prompt, and that the old bare "read the
    //! source" directive no longer leaks. `render_item_prompt` and
    //! `grounding::resolve` are crate-internal, so these live in-crate rather than
    //! in `tests/`.

    use std::path::Path;
    use std::process::Command;

    use logos_core::wiki::{DocGrounding, GenerationCategory, GenerationItem};
    use logos_core::Engine;

    use super::*;

    fn sh_git(cwd: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["-c", "user.email=dev@logos", "-c", "user.name=Logos Dev"])
            .args(args)
            .output()
            .expect("git is on PATH");
        assert!(out.status.success(), "git {args:?} failed");
    }

    fn commit(cwd: &Path, rel: &str, contents: &str) {
        let path = cwd.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        sh_git(cwd, &["add", rel]);
        sh_git(cwd, &["commit", "-q", "-m", "fixture"]);
    }

    /// A committed, indexed repo with one source file, so the code graph has a
    /// revision > 0 and `context` yields ranked symbols.
    fn indexed_engine(repo: &Path) -> Engine {
        sh_git(repo, &["init", "-q", "-b", "main"]);
        commit(
            repo,
            "src/widget.rs",
            "pub fn widget_handler(x: i64) -> i64 {\n    if x == 0 { return 0; }\n    x\n}\n",
        );
        let engine = Engine::start(repo).expect("engine starts");
        engine.index();
        engine
    }

    fn overview_item(grounding: Option<DocGrounding>) -> GenerationItem {
        GenerationItem {
            category: GenerationCategory::Overview,
            slug: "overview/project-overview".to_string(),
            title: "Project Overview".to_string(),
            anchor: String::new(),
            reason: "absent",
            command: String::new(),
            grounding,
        }
    }

    /// A **doc-grounded** item resolves to the referenced `docs/` source content,
    /// and that content is injected verbatim into the synthesis prompt — the page
    /// is written from the spec, not from a bare directive ([S-236] acceptance).
    #[test]
    fn doc_grounded_item_injects_source_content_into_the_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let engine = indexed_engine(repo);
        // The referenced doc source, with a distinctive sentinel line.
        let sentinel = "Logos is a structural code-intelligence tool. GROUNDING-SENTINEL-DOC.";
        std::fs::create_dir_all(repo.join("docs/specs")).unwrap();
        std::fs::write(repo.join("docs/specs/software-spec.md"), sentinel).unwrap();

        let item = overview_item(Some(DocGrounding {
            sources: vec!["docs/specs/software-spec.md".to_string()],
            fallback_to_code: false,
            directive: "Doc-grounded: read `docs/specs/software-spec.md`.".to_string(),
        }));

        let resolved = grounding::resolve(&engine, &item, DEFAULT_GROUNDING_BUDGET);
        assert!(resolved.contains("## Source: docs/specs/software-spec.md"));
        assert!(resolved.contains("GROUNDING-SENTINEL-DOC"), "doc content is resolved");

        let prompt = render_item_prompt(&item, &resolved);
        assert!(prompt.contains("GROUNDING-SENTINEL-DOC"), "doc content reaches the prompt");
        assert!(prompt.contains("using ONLY the material in the CONTEXT section"));
        // The old bare directive is no longer injected.
        assert!(!prompt.contains("Grounding: Doc-grounded"), "the bare directive no longer leaks");
    }

    /// A **code-fallback** item (its mapped doc is absent) resolves to a structured
    /// code-graph digest, injected into the prompt ([S-236] acceptance).
    #[test]
    fn code_fallback_item_injects_a_code_graph_digest_into_the_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let engine = indexed_engine(repo);

        let item = GenerationItem {
            title: "widget_handler".to_string(), // seed the digest on a real symbol
            ..overview_item(Some(DocGrounding {
                sources: vec!["docs/specs/software-spec.md".to_string()],
                fallback_to_code: true,
                directive: "Doc-grounded with code fallback: absent — summarize from the code graph."
                    .to_string(),
            }))
        };

        let resolved = grounding::resolve(&engine, &item, DEFAULT_GROUNDING_BUDGET);
        assert!(resolved.contains("Code-graph digest for \"widget_handler\""));
        assert!(resolved.contains("widget_handler"), "the digest names the real symbol");

        let prompt = render_item_prompt(&item, &resolved);
        assert!(prompt.contains("Code-graph digest"), "the digest reaches the prompt");
        assert!(prompt.contains("write only from this"));
    }

    /// A free-synthesis overview item (no grounding directive at all) still resolves
    /// to a code-graph digest so the page is grounded rather than fabricated.
    #[test]
    fn ungrounded_item_falls_back_to_a_code_graph_digest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let engine = indexed_engine(tmp.path());
        let item = GenerationItem {
            title: "widget_handler".to_string(),
            ..overview_item(None)
        };
        let resolved = grounding::resolve(&engine, &item, DEFAULT_GROUNDING_BUDGET);
        assert!(resolved.contains("Code-graph digest"));
    }

    /// A large doc source is bounded to the configured token budget before it is
    /// injected — a big spec is summarized from a capped excerpt, not overflowed
    /// ([S-236] acceptance).
    #[test]
    fn a_large_doc_source_is_bounded_to_the_grounding_budget_in_the_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let engine = indexed_engine(repo);
        std::fs::create_dir_all(repo.join("docs/specs")).unwrap();
        std::fs::write(repo.join("docs/specs/software-spec.md"), "S".repeat(100_000)).unwrap();

        let item = overview_item(Some(DocGrounding {
            sources: vec!["docs/specs/software-spec.md".to_string()],
            fallback_to_code: false,
            directive: "Doc-grounded".to_string(),
        }));
        // A small budget forces truncation; the marker proves the bound applied.
        let resolved = grounding::resolve(&engine, &item, 100);
        assert!(resolved.contains("truncated to the 100-token grounding budget"));
        assert!(resolved.chars().count() < 1_000, "bounded well under the 100k source");
    }

    /// The prompt renders an honest placeholder when no content resolves, rather
    /// than an empty CONTEXT block ([NFR-CC-04]).
    #[test]
    fn an_empty_grounding_renders_an_honest_placeholder() {
        let item = overview_item(None);
        let prompt = render_item_prompt(&item, "   ");
        assert!(prompt.contains("(no source content could be resolved for this page)"));
    }

    /// The configured `with_grounding_budget` actually flows through the real
    /// `WikiAgent` field into `resolve_grounding` → `grounding::resolve` — the run
    /// wiring, not just the free function. A regression that dropped the field
    /// would still pass the direct-`resolve` tests but fail here ([S-236] AC3).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_grounding_budget_flows_through_resolve_grounding() {
        use agent_core::MockCompletionModel;

        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let engine = Arc::new(indexed_engine(repo));
        std::fs::create_dir_all(repo.join("docs/specs")).unwrap();
        std::fs::write(repo.join("docs/specs/software-spec.md"), "Z".repeat(100_000)).unwrap();

        let item = overview_item(Some(DocGrounding {
            sources: vec!["docs/specs/software-spec.md".to_string()],
            fallback_to_code: false,
            directive: "Doc-grounded".to_string(),
        }));

        // A tiny configured budget must reach grounding::resolve via the field.
        let tiny = WikiAgent::new(MockCompletionModel::new([]), "skill", "gen")
            .with_grounding_budget(7);
        let tiny_resolved = tiny.resolve_grounding(&engine, &item).await.expect("resolves");
        assert!(
            tiny_resolved.contains("truncated to the 7-token grounding budget"),
            "the configured budget is applied through a real WikiAgent run path"
        );

        // The generous default injects more content than the tiny budget.
        let default = WikiAgent::new(MockCompletionModel::new([]), "skill", "gen");
        let default_resolved = default.resolve_grounding(&engine, &item).await.expect("resolves");
        assert!(
            default_resolved.chars().count() > tiny_resolved.chars().count(),
            "a larger configured budget injects more grounding"
        );
    }

    /// Resolution is a deterministic function of the fixture ([NFR-RA-06]) on both
    /// the doc-grounded and the code-digest branch — the same input yields
    /// byte-identical output across repeated calls.
    #[test]
    fn resolve_is_deterministic_on_both_branches() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path();
        let engine = indexed_engine(repo);
        std::fs::create_dir_all(repo.join("docs/specs")).unwrap();
        std::fs::write(repo.join("docs/specs/software-spec.md"), "spec body text").unwrap();

        let doc_item = overview_item(Some(DocGrounding {
            sources: vec!["docs/specs/software-spec.md".to_string()],
            fallback_to_code: false,
            directive: "d".to_string(),
        }));
        let a = grounding::resolve(&engine, &doc_item, DEFAULT_GROUNDING_BUDGET);
        let b = grounding::resolve(&engine, &doc_item, DEFAULT_GROUNDING_BUDGET);
        assert_eq!(a, b, "doc-grounded resolution is byte-identical (NFR-RA-06)");

        let code_item = GenerationItem {
            title: "widget_handler".to_string(),
            ..overview_item(None)
        };
        let c = grounding::resolve(&engine, &code_item, DEFAULT_GROUNDING_BUDGET);
        let d = grounding::resolve(&engine, &code_item, DEFAULT_GROUNDING_BUDGET);
        assert_eq!(c, d, "code-digest resolution is byte-identical (NFR-RA-06)");
    }

    /// A doc-grounded item whose source is absent stays on the doc path and reports
    /// the honest "source unavailable" marker — it does **not** silently fall
    /// through to a code-graph digest (the routing a refactor could get wrong).
    #[test]
    fn a_doc_grounded_item_with_a_missing_source_does_not_fall_through_to_code() {
        let tmp = tempfile::TempDir::new().unwrap();
        let engine = indexed_engine(tmp.path());
        let item = overview_item(Some(DocGrounding {
            sources: vec!["docs/specs/absent.md".to_string()],
            fallback_to_code: false,
            directive: "d".to_string(),
        }));
        let resolved = grounding::resolve(&engine, &item, DEFAULT_GROUNDING_BUDGET);
        assert!(resolved.contains("[source unavailable: docs/specs/absent.md]"));
        assert!(
            !resolved.contains("Code-graph digest"),
            "a doc-grounded item never falls through to the code path"
        );
    }
}
