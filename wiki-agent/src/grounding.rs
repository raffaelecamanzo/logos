//! Deterministic **grounding-content resolution** for the in-process wiki
//! generator ([S-236], [CR-059], [ADR-51]).
//!
//! The tool-less `rig` agent ([ADR-42]) cannot fetch anything, so instead of
//! handing it the bare FR-WK-13 "read the source" *directive* (the external-hook
//! contract, [ADR-33]) we resolve the grounding material **in-binary** and inject
//! it straight into the synthesis prompt ([ADR-51]). Two paths, matching the
//! [`DocGrounding`](logos_core::wiki::DocGrounding) shape the queue already carries:
//!
//! - **doc-grounded** ([`DocGrounding::fallback_to_code`] is `false`) — read the
//!   named `docs/` source file(s)/glob, each **bounded/truncated per source** to a
//!   token budget so a large spec is summarized from a capped, deterministic
//!   excerpt rather than overflowing the context;
//! - **code-fallback** ([`DocGrounding::fallback_to_code`] is `true`) *and* the
//!   free-synthesis overview items (no directive at all) — build a structured
//!   **code-graph digest** from the [`Engine`] `context` query so the page is still
//!   grounded in the real graph.
//!
//! Every read here is **local I/O** — `std::fs` over the repo's own `docs/` tree
//! and the local code graph — so the offline posture is unchanged ([NFR-SE-01],
//! [ADR-51]): the agent stays tool-less and non-agentic, and this step makes no
//! LLM or network call. Resolution is a pure function of `docs/` + `wiki.db` +
//! the current graph revision, so it is deterministic ([NFR-RA-06]).
//!
//! [S-236]: ../../../docs/planning/journal.md#s-236-deterministic-grounding-content-resolution-and-injection-into-in-process-wiki-synthesis
//! [CR-059]: ../../../docs/requests/CR-059-wiki-generation-grounding-and-write-guard.md
//! [ADR-51]: ../../../docs/specs/architecture/decisions/ADR-51.md
//! [ADR-42]: ../../../docs/specs/architecture/decisions/ADR-42.md
//! [ADR-33]: ../../../docs/specs/architecture/decisions/ADR-33.md
//! [NFR-SE-01]: ../../../docs/specs/requirements/NFR-SE-01.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use logos_core::models::ContextBundle;
use logos_core::wiki::GenerationItem;
use logos_core::Engine;

/// The default **per-source token budget** ([S-236], [ADR-51]) each grounding
/// source is bounded to when a caller does not set one — a large source is
/// truncated deterministically to this budget rather than overflowing the
/// synthesis context. Overridable via
/// [`WikiAgent::with_grounding_budget`](crate::WikiAgent::with_grounding_budget).
pub const DEFAULT_GROUNDING_BUDGET: usize = 4000;

/// The deterministic characters-per-token estimate used to turn the token budget
/// into a concrete character cap. A coarse, model-agnostic heuristic (~4 chars per
/// token for English/Markdown) — exactness is unnecessary because the budget only
/// bounds context growth; determinism is what matters ([NFR-RA-06]).
const CHARS_PER_TOKEN: usize = 4;

/// The cap on ranked symbols pulled into a code-graph digest — enough to sketch a
/// page's neighbourhood without flooding the prompt; the whole digest is then
/// bounded to the token budget as well.
const MAX_DIGEST_NODES: usize = 12;

/// The per-symbol source-excerpt cap (in characters) inside a code-graph digest,
/// so one large declaration cannot dominate the digest before the overall
/// token-budget bound applies.
const PER_NODE_CODE_CHARS: usize = 400;

/// The deterministic cap on files pulled from a single glob source (e.g. the
/// consolidated `FR-*.md` category), sorted before capping so the excerpt is
/// stable; an honest note records any omission ([NFR-CC-04]).
const MAX_FILES_PER_GLOB: usize = 60;

/// Resolve the grounding content for one queue item into the text block injected
/// into the synthesis prompt ([S-236], [ADR-51]).
///
/// Synchronous by design — every touch is a local read (`std::fs` or the local
/// graph), so the caller runs this on the blocking pool ([ADR-03]) exactly like
/// the other synchronous [`Engine`] touches. Never errors: an unreadable source
/// degrades to an honest inline marker rather than aborting the page
/// ([NFR-CC-04], the infallible-surface degradation channel [ADR-14]).
pub(crate) fn resolve(engine: &Engine, item: &GenerationItem, budget: usize) -> String {
    match &item.grounding {
        // Doc-grounded: read and bound the named `docs/` source(s).
        Some(g) if !g.fallback_to_code => read_doc_sources(engine.root(), &g.sources, budget),
        // Code-fallback (an absent mapped doc) or a free-synthesis overview item
        // with no directive: ground in a structural code-graph digest seeded on
        // the page title.
        _ => {
            let bundle = engine.context(&item.title, Some(MAX_DIGEST_NODES), true);
            render_code_digest(&bundle, budget)
        }
    }
}

/// Read every `docs/` source (a plain file or a single-`*` filename glob),
/// bounding each **per source** to `budget` tokens and labeling it with its
/// repo-relative path so deep references survive into the prose ([CR-034]).
///
/// The **whole** assembled block is then bounded to `budget` tokens as well — a
/// glob can expand to up to [`MAX_FILES_PER_GLOB`] files (e.g. a consolidated
/// `FR-*.md` category), so a per-source-only bound would still let the aggregate
/// overflow the model context. This mirrors the whole-output bound in
/// [`render_code_digest`], so both grounding paths honor the [S-236] anti-overflow
/// acceptance criterion, not just the literal per-source one.
fn read_doc_sources(root: &Path, sources: &[String], budget: usize) -> String {
    let mut out = String::new();
    for pattern in sources {
        let files = expand_source(root, pattern);
        if files.is_empty() {
            let _ = writeln!(out, "[source unavailable: {pattern}]\n");
            continue;
        }
        let omitted = files.len().saturating_sub(MAX_FILES_PER_GLOB);
        for path in files.iter().take(MAX_FILES_PER_GLOB) {
            let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
            match std::fs::read_to_string(path) {
                Ok(content) => {
                    let _ = writeln!(out, "## Source: {rel}\n");
                    out.push_str(&bound_source(&content, budget));
                    out.push_str("\n\n");
                }
                Err(e) => {
                    let _ = writeln!(out, "[source unavailable: {rel} ({e})]\n");
                }
            }
        }
        if omitted > 0 {
            let _ = writeln!(
                out,
                "[… {omitted} more file(s) matching `{pattern}` omitted for length …]\n"
            );
        }
    }
    // Bound the whole block, not just each source, so a many-file glob cannot
    // overflow the context ([S-236] AC3; symmetric with `render_code_digest`).
    bound_source(out.trim_end(), budget)
}

/// Expand a `docs/`-relative source pattern into the concrete files it names.
///
/// Supports a plain file path or a **single-`*`** filename glob scoped to one
/// directory — exactly the forms the [`DocGrounding`](logos_core::wiki::DocGrounding)
/// sources use (`*.md`, `FR-*.md`, `docs/specs/software-spec.md`). Directory-scoped
/// (never a tree walk) and **sorted**, so the excerpt is deterministic
/// ([NFR-RA-06]); a missing directory or file reads as "no match".
fn expand_source(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let (dir_part, file_part) = match pattern.rfind('/') {
        Some(i) => (&pattern[..i], &pattern[i + 1..]),
        None => ("", pattern),
    };
    match file_part.find('*') {
        Some(star) => {
            let prefix = &file_part[..star];
            let suffix = &file_part[star + 1..];
            let Ok(entries) = std::fs::read_dir(root.join(dir_part)) else {
                return Vec::new();
            };
            let mut matches: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    // `len >= prefix+suffix` stops the prefix and suffix aliasing the
                    // same char on a too-short name (e.g. pattern `x*x` must not match
                    // a bare `x`). A bare `.md` legitimately matches `*.md` — `*` may
                    // stand for zero chars — and is intentionally kept.
                    name.len() >= prefix.len() + suffix.len()
                        && name.starts_with(prefix)
                        && name.ends_with(suffix)
                        && e.path().is_file()
                })
                .map(|e| e.path())
                .collect();
            matches.sort();
            matches
        }
        None => {
            let p = root.join(pattern);
            if p.is_file() {
                vec![p]
            } else {
                Vec::new()
            }
        }
    }
}

/// Render an [`Engine::context`](logos_core::Engine::context) bundle into a compact,
/// deterministic **code-graph digest** — the code-fallback grounding ([ADR-51]).
///
/// Lists each ranked symbol with its kind and `file:line` location plus a bounded
/// source excerpt, then the covered files. The whole digest is bounded to `budget`
/// tokens. An empty bundle degrades to an honest one-line note ([NFR-CC-04]).
fn render_code_digest(bundle: &ContextBundle, budget: usize) -> String {
    if bundle.nodes.is_empty() {
        return format!(
            "Code-graph digest for \"{}\": the code graph yielded no ranked symbols for \
             this page.",
            bundle.task
        );
    }
    let mut out = format!(
        "Code-graph digest for \"{}\" — {} ranked symbol(s) across {} file(s):\n",
        bundle.task,
        bundle.nodes.len(),
        bundle.files.len()
    );
    for node in &bundle.nodes {
        let sym = &node.symbol;
        let loc = match (&sym.file, sym.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => "(unbound)".to_string(),
        };
        let _ = writeln!(out, "\n- {} ({}) — {}", sym.name, sym.kind.as_str(), loc);
        if let Some(code) = &node.code {
            let (excerpt, _) = truncate_chars(code, PER_NODE_CODE_CHARS);
            // Indent the excerpt so it reads as a nested block under the symbol.
            let _ = writeln!(out, "  {}", excerpt.replace('\n', "\n  "));
        }
    }
    if !bundle.files.is_empty() {
        let _ = writeln!(out, "\nFiles covered: {}", bundle.files.join(", "));
    }
    bound_source(&out, budget)
}

/// Bound `content` to `budget_tokens`, truncating **deterministically** on a
/// character boundary and appending an honest truncation marker when it is cut
/// ([S-236] acceptance, [NFR-CC-04]).
fn bound_source(content: &str, budget_tokens: usize) -> String {
    let max_chars = budget_tokens.saturating_mul(CHARS_PER_TOKEN);
    let (bounded, truncated) = truncate_chars(content, max_chars);
    if truncated {
        format!("{bounded}\n\n[… truncated to the {budget_tokens}-token grounding budget …]")
    } else {
        bounded
    }
}

/// Take at most `max_chars` characters (UTF-8-safe, on a `char` boundary),
/// reporting whether truncation happened. Deterministic ([NFR-RA-06]).
fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
    // `char_indices().nth(max_chars)` yields the byte offset of the char *after*
    // the first `max_chars` chars (always a valid UTF-8 boundary), or `None` when
    // the string is shorter — exactly the truncate-or-keep decision.
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => (s[..idx].to_string(), true),
        None => (s.to_string(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use logos_core::model::NodeKind;
    use logos_core::models::{ContextNode, SymbolRef};

    fn node(name: &str, kind: NodeKind, file: &str, line: u32, code: Option<&str>) -> ContextNode {
        ContextNode {
            symbol: SymbolRef {
                symbol: format!("sym::{name}"),
                name: name.to_string(),
                kind,
                file: Some(file.to_string()),
                line: Some(line),
            },
            score: 1.0,
            seed: true,
            code: code.map(str::to_string),
        }
    }

    #[test]
    fn truncate_is_deterministic_on_a_char_boundary() {
        let (t, cut) = truncate_chars("abcdef", 3);
        assert_eq!(t, "abc");
        assert!(cut);
        let (t, cut) = truncate_chars("abc", 10);
        assert_eq!(t, "abc");
        assert!(!cut);
    }

    #[test]
    fn truncate_never_splits_a_multibyte_char() {
        // Four 3-byte characters; capping at 2 chars must land on a char boundary,
        // never mid-codepoint (which would panic on a byte slice).
        let (t, cut) = truncate_chars("日本語文", 2);
        assert_eq!(t, "日本");
        assert!(cut);
    }

    #[test]
    fn bound_source_marks_a_truncated_source_and_leaves_a_small_one_intact() {
        // Budget 1 token → 4 chars. A longer source is cut and marked.
        let bounded = bound_source("abcdefghij", 1);
        assert!(bounded.starts_with("abcd"));
        assert!(bounded.contains("truncated to the 1-token grounding budget"));
        // A source within budget is returned verbatim, no marker.
        let small = bound_source("ab", 1);
        assert_eq!(small, "ab");
    }

    #[test]
    fn code_digest_renders_symbols_locations_and_files_bounded() {
        let bundle = ContextBundle {
            task: "Key concepts".to_string(),
            hops: 1,
            nodes: vec![
                node("Engine", NodeKind::Struct, "logos-core/src/engine.rs", 42, Some("pub struct Engine {}")),
                node("run", NodeKind::Function, "wiki-agent/src/agent.rs", 220, None),
            ],
            files: vec![
                "logos-core/src/engine.rs".to_string(),
                "wiki-agent/src/agent.rs".to_string(),
            ],
            est_reads_replaced: 2,
            suggestions: vec![],
            warnings: vec![],
        };
        let digest = render_code_digest(&bundle, DEFAULT_GROUNDING_BUDGET);
        assert!(digest.contains("Code-graph digest for \"Key concepts\""));
        assert!(digest.contains("Engine (struct) — logos-core/src/engine.rs:42"));
        assert!(digest.contains("run (function) — wiki-agent/src/agent.rs:220"));
        assert!(digest.contains("pub struct Engine {}"));
        assert!(digest.contains("Files covered: logos-core/src/engine.rs, wiki-agent/src/agent.rs"));
    }

    #[test]
    fn code_digest_bounds_the_whole_digest_to_the_token_budget() {
        // A node with a large code excerpt, a tiny budget → the digest is cut.
        let big = "x".repeat(10_000);
        let bundle = ContextBundle {
            task: "big".to_string(),
            hops: 1,
            nodes: vec![node("Big", NodeKind::Function, "a.rs", 1, Some(&big))],
            files: vec!["a.rs".to_string()],
            est_reads_replaced: 1,
            suggestions: vec![],
            warnings: vec![],
        };
        let digest = render_code_digest(&bundle, 10); // 40 chars
        assert!(digest.contains("truncated to the 10-token grounding budget"));
        // Per-node excerpt cap keeps a single symbol from dominating pre-bound.
        assert!(digest.chars().count() < 200);
    }

    #[test]
    fn empty_bundle_degrades_to_an_honest_note() {
        let bundle = ContextBundle {
            task: "nothing".to_string(),
            ..ContextBundle::default()
        };
        let digest = render_code_digest(&bundle, DEFAULT_GROUNDING_BUDGET);
        assert!(digest.contains("no ranked symbols"));
    }

    #[test]
    fn expand_source_matches_a_single_star_glob_sorted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join("docs/specs/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("FR-02.md"), "b").unwrap();
        std::fs::write(dir.join("FR-01.md"), "a").unwrap();
        std::fs::write(dir.join("NFR-01.md"), "n").unwrap(); // must not match FR-*
        std::fs::write(dir.join("README.txt"), "x").unwrap(); // wrong suffix

        let files = expand_source(root, "docs/specs/requirements/FR-*.md");
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["FR-01.md", "FR-02.md"], "sorted, FR-only");
    }

    #[test]
    fn expand_source_resolves_a_plain_file_and_misses_are_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("docs/specs")).unwrap();
        std::fs::write(root.join("docs/specs/software-spec.md"), "spec").unwrap();

        assert_eq!(expand_source(root, "docs/specs/software-spec.md").len(), 1);
        assert!(expand_source(root, "docs/specs/absent.md").is_empty());
        assert!(expand_source(root, "docs/specs/*.rs").is_empty());
    }

    #[test]
    fn read_doc_sources_bounds_per_source_and_labels_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join("docs/specs");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("software-spec.md"), "a".repeat(1000)).unwrap();

        let out = read_doc_sources(
            root,
            &["docs/specs/software-spec.md".to_string()],
            20, // 20 tokens → 80 chars: room for the header, but the 1000-char source is cut
        );
        assert!(out.contains("## Source: docs/specs/software-spec.md"));
        assert!(out.contains("truncated to the 20-token grounding budget"));
    }

    #[test]
    fn read_doc_sources_reports_a_missing_source_honestly() {
        let tmp = tempfile::TempDir::new().unwrap();
        let out = read_doc_sources(tmp.path(), &["docs/specs/absent.md".to_string()], 100);
        assert!(out.contains("[source unavailable: docs/specs/absent.md]"));
    }

    #[test]
    fn read_doc_sources_bounds_the_whole_block_not_just_each_source() {
        // Each file is within the per-source budget, but their sum is not: the
        // aggregate bound must still cut the block so a many-file glob cannot
        // overflow the context ([S-236] AC3).
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join("docs/specs/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        // 20 files × 30 chars each (each under the 40-char per-source budget) → the
        // aggregate is ~600+ chars, far over the 40-char whole-block budget.
        for i in 0..20 {
            std::fs::write(dir.join(format!("FR-{i:03}.md")), "y".repeat(30)).unwrap();
        }
        let out = read_doc_sources(root, &["docs/specs/requirements/FR-*.md".to_string()], 10);
        assert!(out.contains("truncated to the 10-token grounding budget"));
        // Bounded near the 40-char budget (plus the short marker), not ~600.
        assert!(out.chars().count() < 120, "the whole block is aggregate-bounded");
    }

    #[test]
    fn read_doc_sources_caps_a_large_glob_and_notes_the_omission() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let dir = root.join("docs/specs/requirements");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(MAX_FILES_PER_GLOB + 5) {
            std::fs::write(dir.join(format!("FR-{i:03}.md")), "x").unwrap();
        }
        // A large budget so the aggregate bound never cuts the trailing omission
        // note — this test isolates the file-count cap, not the token bound.
        let out = read_doc_sources(
            root,
            &["docs/specs/requirements/FR-*.md".to_string()],
            100_000,
        );
        assert!(out.contains("5 more file(s) matching"));
    }
}
