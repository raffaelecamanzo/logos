//! The **branch delta** ([FR-GV-24], [ADR-82], S-677): every offender
//! ([FR-QM-19]) and every `check` violation of the working tree, classified
//! per symbol against the merge-base of `HEAD` and a base ref.
//!
//! # The two measurements
//!
//! - **After** is the graph as it stands. Nothing is reconciled first: a
//!   review does not sync, so an edit made since the last sync is not in the
//!   graph — and every entry in such a file says so (`stale`).
//! - **Before** is the merge-base commit, and only that commit ([ADR-82]):
//!   1. the snapshot recorded at it, when that snapshot holds everything the
//!      review compares — its uncapped offender list under the current
//!      thresholds and metric semantics, and a contract that evaluates no
//!      rule (a snapshot keeps no violation set, so a contract with rules
//!      needs the tree itself);
//!   2. otherwise a shadow index of the merge-base tree, materialised from
//!      the object database by `git read-tree`/`checkout-index` into a private
//!      temp index and directory, indexed by [`shadow_reindex`], measured, and
//!      cached by commit under `.logos/review-cache/` (gitignored). A cached
//!      entry is reused only under the same contract, thresholds, admission,
//!      semantics and logos version; the cache keeps the newest
//!      [`CACHE_KEEP`] entries.
//!
//!   No other commit is ever read. When neither is possible the review says
//!   why and classifies nothing.
//!
//! Both sides are evaluated under the **current** contract and thresholds, so
//! the delta measures the code, not a contract edit: each side's derived
//! annotations are re-materialised under the live `rules.toml` before it is
//! measured, as `check` re-materialises the live graph ([`rematerialise`]).
//!
//! # Matching
//!
//! An offender is matched by dimension, measure and identity — its
//! [`Offender::node_id`], a Depth chain's head name, or an Acyclicity cycle's
//! directories (a cycle is named by its lowest-storage-id member, which a
//! re-index can change). A violation is matched by rule and subject, and
//! carries a value wherever the budget names one per subject: the review
//! evaluates the per-subject budgets with their offender marks switched on,
//! which changes no violation, only what it records. A violation with no
//! value is matched by its message too.
//!
//! # The security section
//!
//! [`security_delta`] classifies each scanner's latest stored findings against
//! the merge-base ([FR-SC-18], S-686), over evidence the engine reads and hands
//! in ([`SecurityEvidence`]) — this module never opens `history.db`, and the
//! merge-base tree is never scanned:
//!
//! - a scanner whose report was **recorded at the merge-base** is compared by
//!   fingerprint: absent there is `introduced`; present there and inside a
//!   hunk the branch changed is `touched`. A region-keyed fingerprint moves
//!   with its line, so a new fingerprint outside every hunk that has a
//!   vanished merge-base twin (same scanner, rule and file) is that finding,
//!   moved: `pre-existing`, and the twin is not resolved;
//! - any other scanner by **hunk**: inside a changed hunk is `introduced`,
//!   unless the branch deleted the file;
//! - otherwise `pre-existing`;
//! - a merge-base fingerprint the latest report no longer carries is
//!   `resolved` only when that report saw its file as it stands — the file
//!   still anchors a fresh finding of that report, or was last modified before
//!   it was ingested — and `stale` otherwise. A deleted file's finding is
//!   resolved: no scan can report it.
//!
//! A suppressed finding is listed apart with its reason and expiry.
//!
//! [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
//! [FR-SC-18]: ../../../docs/specs/requirements/FR-SC-18.md
//! [FR-QM-19]: ../../../docs/specs/requirements/FR-QM-19.md
//! [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
//! [`shadow_reindex`]: crate::pipeline::shadow_reindex

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{Budget, GodContainersBudget, RatchetBudget, RatchetFlag, Rules};
use crate::engine::Engine;
use crate::history::{SecurityFinding, FRESHNESS_FRESH, FRESHNESS_NA, FRESHNESS_STALE};
use crate::hydrate::{build_view, Granularity};
use crate::models::quality::{
    BeforeStateCache, BranchReview, CloneGroupMember, IntroducedCloneGroup, Offender,
    ReviewStatus, ReviewedOffender, ReviewedViolation, SecurityDelta, SecurityDeltaEntry,
    SecurityDeltaScanner, SecurityDeltaStatus, SuppressedDeltaEntry, Violation, WorstOffenders,
};
use crate::navigate::branch::{changed_hunks, git_command, git_line, rev_parse, FileRanges, WHOLE_FILE};
use crate::runtime::Runtime;
use crate::security::context::FindingContext;

use super::thin_graph::{absence_tag, ThinGraph};
use super::{
    effective_thresholds, load_rules_cached, member_access_extensions, quality_runtime,
    rematerialise, CompiledRules, SecurityEvidence, StoreFacts,
};

/// The cache directory, relative to the project root. Ignored by the managed
/// `.logos/.gitignore` block.
const CACHE_DIR_REL: &str = ".logos/review-cache";

/// How many cached before-states are kept; older ones are pruned, oldest first
/// ([ADR-82]: bounded by distinct merge-bases).
///
/// [ADR-82]: ../../../docs/specs/architecture/decisions/ADR-82.md
const CACHE_KEEP: usize = 16;

/// The cache entry layout. Bumped when [`Measured`] changes shape, so an old
/// entry is rebuilt rather than misread.
const CACHE_FORMAT: u32 = 2;

/// One offender of one measurement, as the cache stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Point {
    dimension: String,
    node_id: Option<String>,
    name: String,
    file: String,
    line: Option<i64>,
    detail: String,
    measure: Option<String>,
    value: Option<i64>,
    threshold: Option<i64>,
}

/// One violation of one measurement, as the cache stores it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Breach {
    rule: String,
    rule_type: String,
    subject: String,
    file: String,
    value: Option<u64>,
    message: String,
}

/// Everything one side of the review compares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Measured {
    offenders: Vec<Point>,
    violations: Vec<Breach>,
    /// The graph dimensions drop out of this side's signal: too few
    /// cross-module edges to measure module structure ([FR-QM-18]).
    ///
    /// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
    graph_not_applicable: bool,
}

/// The dimensions whose offenders explain the module graph, outside the signal
/// when it is too thin to measure ([FR-QM-18]). Modularity lists no offenders.
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
const GRAPH_DIMENSIONS: [&str; 2] = ["acyclicity", "depth"];

/// Drop the graph dimensions' offenders from both sides when either side's
/// graph is too thin to measure: the gate counts no movement of a dimension
/// that is not applicable on either side (`metric_regressions`), so a longer
/// chain on a thin graph is no regression here either. The note that says so,
/// when it applies.
fn without_inapplicable_graph_points(before: &mut Measured, after: &mut Measured) -> Option<String> {
    if !(before.graph_not_applicable || after.graph_not_applicable) {
        return None;
    }
    for side in [before, after] {
        side.offenders.retain(|p| !GRAPH_DIMENSIONS.contains(&p.dimension.as_str()));
    }
    Some(
        "acyclicity and depth offenders are not classified: the module graph has too few \
         cross-module edges to measure on the merge-base or the branch"
            .to_string(),
    )
}

/// A cached before-state, keyed by commit and by everything it was measured
/// under.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    format: u32,
    commit: String,
    key: String,
    before: Measured,
}

/// What both sides are measured under.
struct Contract {
    /// The contract as declared — what the shadow tree's policy graph is
    /// materialised under.
    declared: std::sync::Arc<CompiledRules>,
    /// The same contract with every per-subject budget's offender marks on.
    marked: CompiledRules,
    thresholds: crate::metrics::Thresholds,
    member_access: HashSet<String>,
}

/// `review --base` ([FR-GV-24]): see the module docs.
///
/// # Errors
/// Returns an error on a transient engine, a store fault, an invalid
/// `rules.toml`, or a failed shadow index. A base that does not resolve, or a
/// merge-base that cannot be measured, is a `measured: false` answer, not an
/// error.
///
/// [FR-GV-24]: ../../../docs/specs/requirements/FR-GV-24.md
pub(crate) fn branch_review(
    engine: &Engine,
    base: Option<&str>,
    security: impl FnOnce(&str) -> Result<SecurityEvidence>,
) -> Result<BranchReview> {
    let root = engine.root();
    let runtime = quality_runtime(engine)?;
    let stated = base.map(str::trim).filter(|b| !b.is_empty());
    let (base, base_origin) = match stated {
        Some(base) => (Some(base.to_string()), BranchReview::STATED),
        None => (default_branch(root), BranchReview::DEFAULT_BRANCH),
    };
    let mut review = BranchReview {
        base: base.clone().unwrap_or_default(),
        base_origin,
        head: rev_parse(root, "HEAD"),
        ..BranchReview::default()
    };
    let Some(base) = base else {
        return Ok(not_measured(
            review,
            "no base was given and no default branch resolves (no origin/HEAD, main or master); \
             pass --base <ref>",
        ));
    };
    let Some(head) = review.head.clone() else {
        return Ok(not_measured(review, "HEAD does not resolve to a commit"));
    };
    let Some(base_commit) = rev_parse(root, &base) else {
        return Ok(not_measured(review, &format!("{base} does not resolve to a commit")));
    };
    let Some(merge_base) = git_line(root, &["merge-base", &head, &base_commit]) else {
        return Ok(not_measured(
            review,
            &format!("HEAD and {base} have no common ancestor"),
        ));
    };
    review.merge_base = Some(merge_base.clone());

    let declared = load_rules_cached(engine, None)?;
    let contract = Contract {
        marked: CompiledRules::compile(marked(&declared.rules), declared.hash.clone())?,
        thresholds: effective_thresholds(&declared.rules),
        member_access: member_access_extensions(engine),
        declared,
    };
    // The live graph was annotated under the contract of its last sync, which
    // a `rules.toml` edit since may have changed; `check` re-materialises for
    // the same reason, and so does the shadow below, so both sides are
    // measured under one contract.
    let (_, registry, config) = engine.pipeline_ctx()?;
    rematerialise(runtime, registry, &config, &contract.declared.rules)?;
    engine.advance_sync_stamp();
    let (mut after, after_checked) = measure(runtime, &contract)?;
    let mut before = match recorded_before(runtime, &merge_base, &contract, after_checked)? {
        Some(before) => {
            review.before_state = Some(BranchReview::RECORDED_SNAPSHOT);
            before
        }
        None => match shadow_before(engine, &merge_base, &contract) {
            Ok((before, cache)) => {
                review.before_state = Some(BranchReview::SHADOW_INDEX);
                review.before_state_cache = cache;
                before
            }
            // ADR-82 §3: no comparable before-state is an answer that says so
            // and classifies nothing, never a substitute and never a fault.
            Err(err) => {
                return Ok(not_measured(
                    review,
                    &format!("no comparable before-state: the merge-base tree could not be measured ({err:#})"),
                ))
            }
        },
    };

    let trust = Trust::read(engine, runtime, &before, &after)?;
    review.stale_files = trust.stale.iter().cloned().collect();
    review.notes.extend(without_inapplicable_graph_points(&mut before, &mut after));
    // Every entry is classified and counted; an `unchanged` one is counted and
    // not listed. On this repository it is 1,614 of 1,694 offenders, and
    // listing them took the answer past 500 KB — more than an agent host
    // accepts from one tool call, for entries no consumer acts on.
    review.offenders = classify_offenders(&before.offenders, &after.offenders, &trust);
    review.offenders.retain(|entry| {
        review.offender_counts.add(entry.status);
        entry.status != ReviewStatus::Unchanged
    });
    review.violations = classify_violations(&before.violations, &after.violations, &contract, &trust);
    review.violations.retain(|entry| {
        review.violation_counts.add(entry.status);
        entry.status != ReviewStatus::Unchanged
    });
    review.introduced_clone_groups =
        introduced_clone_groups(&before.offenders, &after.offenders, &trust);
    review.sections.security = Some(security_section(root, &head, &merge_base, security));
    review.measured = true;
    review.notes.push(
        "after is the graph as last synced, not reconciled: an entry in a file edited since \
         reads stale, and a file never synced is not in the answer"
            .to_string(),
    );
    review.notes.push(
        "an unchanged entry is counted in offender_counts or violation_counts and not listed"
            .to_string(),
    );
    Ok(review)
}

/// The `measured: false` answer: the stated refs, the reason, nothing classified.
fn not_measured(mut review: BranchReview, why: &str) -> BranchReview {
    review.not_measured_because = Some(why.to_string());
    review
}

/// The repository's default branch: `origin/HEAD`'s target, else `main`, else
/// `master` — whichever resolves first.
fn default_branch(root: &Path) -> Option<String> {
    git_line(root, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"])
        .into_iter()
        .chain(["main".to_string(), "master".to_string()])
        .find(|name| rev_parse(root, name).is_some())
}

/// `rules` with every per-subject budget's offender marks on. Only the budgets
/// whose violations are one per subject whatever their form are switched —
/// switching changes what each violation records (its subject and value), not
/// which violations exist. `max_dead`, `max_duplicates`, `max_brain_methods`
/// and `max_cycles` are left as declared: their absolute forms are one
/// project-wide count, and their ratchet forms a different violation set.
fn marked(rules: &Rules) -> Rules {
    let mut rules = rules.clone();
    let constraints = &mut rules.constraints;
    for budget in [
        &mut constraints.max_cc,
        &mut constraints.max_fn_lines,
        &mut constraints.no_god_files,
        &mut constraints.max_fan_in,
        &mut constraints.max_fan_out,
        &mut constraints.max_nesting_depth,
    ] {
        if let Some(declared) = budget.as_ref() {
            *budget = Some(Budget::Ratchet(RatchetBudget {
                max: declared.max(),
                ratchet: true,
            }));
        }
    }
    if constraints
        .no_god_containers
        .as_ref()
        .is_some_and(GodContainersBudget::enforced)
    {
        constraints.no_god_containers = Some(GodContainersBudget::Ratchet(RatchetFlag { ratchet: true }));
    }
    rules
}

/// The ceiling `rule` holds each subject to, for the per-subject budgets.
fn budget_max(rules: &Rules, rule: &str) -> Option<u64> {
    let constraints = &rules.constraints;
    let budget = match rule {
        "max_cc" => constraints.max_cc.as_ref(),
        "max_fn_lines" => constraints.max_fn_lines.as_ref(),
        "no_god_files" => constraints.no_god_files.as_ref(),
        "max_fan_in" => constraints.max_fan_in.as_ref(),
        "max_fan_out" => constraints.max_fan_out.as_ref(),
        "max_nesting_depth" => constraints.max_nesting_depth.as_ref(),
        _ => None,
    };
    budget.map(|b| u64::from(b.max()))
}

/// Measure the store behind `runtime` — its uncapped offenders and its
/// violations under the marked contract — and the number of rules evaluated.
/// A pure read.
fn measure(runtime: &Runtime, contract: &Contract) -> Result<(Measured, u32)> {
    let facts = StoreFacts::read(runtime)?;
    let view = build_view(Granularity::ExcludeContains, &facts.nodes, &facts.edges);
    let test_ids = facts.test_ids();
    let offenders = crate::metrics::all_offenders(
        &view,
        &facts.nodes,
        &facts.edges,
        &facts.function_metrics,
        &test_ids,
        contract.thresholds,
        &contract.member_access,
    );
    let cycles = if contract.marked.rules.constraints.max_cycles.is_some() {
        facts.cycle_count(&view)
    } else {
        0
    };
    let (violations, checked) = facts.evaluate(&contract.marked, cycles);
    let identity: HashMap<i64, &str> = facts
        .nodes
        .iter()
        .map(|n| (n.id.get(), n.identity()))
        .collect();
    let violations = violations
        .iter()
        .map(|v| breach(v, &identity))
        .collect();
    let graph_not_applicable = crate::metrics::graph_dimensions_not_applicable(&view, &facts.nodes, &test_ids);
    Ok((
        Measured {
            offenders: points(&offenders),
            violations,
            graph_not_applicable,
        },
        checked,
    ))
}

/// Flatten the nine lists into points, in [`WorstOffenders::DIMENSIONS`] order.
fn points(offenders: &WorstOffenders) -> Vec<Point> {
    offenders
        .lists()
        .into_iter()
        .flat_map(|(dimension, list)| list.iter().map(move |o| point(dimension, o)))
        .collect()
}

fn point(dimension: &str, o: &Offender) -> Point {
    Point {
        dimension: dimension.to_string(),
        node_id: o.node_id.clone(),
        name: o.name.clone(),
        file: o.file.clone(),
        line: o.line,
        detail: o.detail.clone(),
        measure: o.measure.map(str::to_string),
        value: o.value,
        threshold: o.threshold,
    }
}

/// A violation with its subject: the ratchet mark's symbol, else the node's
/// identity, else the file — empty for a project-wide count.
fn breach(v: &Violation, identity: &HashMap<i64, &str>) -> Breach {
    let subject = match (&v.ratchet, v.node_id) {
        (Some(mark), _) => mark.symbol.clone(),
        (None, Some(id)) => identity
            .get(&id)
            .map_or_else(|| v.file.clone(), |s| (*s).to_string()),
        (None, None) => v.file.clone(),
    };
    Breach {
        rule: v.rule.clone(),
        rule_type: v.rule_type.clone(),
        subject,
        file: v.file.clone(),
        value: v.ratchet.as_ref().map(|mark| mark.value),
        message: v.message.clone(),
    }
}

/// The before-state from the snapshot recorded at `merge_base`, when one holds
/// everything the review compares (see the module docs); otherwise `None`.
fn recorded_before(
    runtime: &Runtime,
    merge_base: &str,
    contract: &Contract,
    checked_rules: u32,
) -> Result<Option<Measured>> {
    if checked_rules > 0 {
        return Ok(None);
    }
    let hash = contract.thresholds.hash();
    let snapshot = runtime
        .submit_read(|store| store.metric_snapshots())?
        .into_iter()
        .filter(|row| {
            row.commit_sha.as_deref() == Some(merge_base)
                && row.metric_version == crate::metrics::METRIC_SEMANTICS_VERSION
                && row.thresholds_hash.as_deref() == Some(hash.as_str())
        })
        .max_by_key(|row| row.id);
    let Some(snapshot) = snapshot else {
        return Ok(None);
    };
    let graph_not_applicable =
        snapshot.acyclicity_applicable == Some(false) || snapshot.depth_applicable == Some(false);
    let offenders = runtime.submit_read(move |store| store.uncapped_offenders_of(snapshot.id))?;
    Ok(offenders
        .filter(|o| o.values_recorded)
        .map(|o| Measured {
            offenders: points(&o),
            violations: Vec::new(),
            graph_not_applicable,
        }))
}

/// The before-state from the cache, or from a shadow index of the merge-base
/// tree that is then cached.
fn shadow_before(
    engine: &Engine,
    merge_base: &str,
    contract: &Contract,
) -> Result<(Measured, BeforeStateCache)> {
    let (_, _, config) = engine.pipeline_ctx()?;
    let key = cache_key(contract, &config);
    let dir = engine.root().join(CACHE_DIR_REL);
    let path = dir.join(format!("{merge_base}.json"));
    if let Some(before) = read_cache(&path, merge_base, &key) {
        return Ok((before, BeforeStateCache::Reused));
    }
    let before = measure_tree(engine, merge_base, contract)?;
    let entry = CacheEntry {
        format: CACHE_FORMAT,
        commit: merge_base.to_string(),
        key,
        before,
    };
    // The cache is derived state (ADR-82): a before-state that was measured
    // but cannot be kept still answers, and the next run measures again.
    match write_cache(&dir, &path, &entry) {
        Ok(()) => prune_cache(&dir, &path),
        Err(err) => tracing::warn!("review: the merge-base before-state was measured but not cached: {err:#}"),
    }
    Ok((entry.before, BeforeStateCache::Built))
}

/// Write `entry` to `path` under `dir`, atomically.
fn write_cache(dir: &Path, path: &Path, entry: &CacheEntry) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating the review cache {}", dir.display()))?;
    let bytes = serde_json::to_vec(entry).context("serialising the review cache entry")?;
    crate::config::atomic::publish(path, &bytes, None)
        .with_context(|| format!("writing the review cache entry {}", path.display()))
}

/// Everything a cached before-state was measured under, hashed: a different
/// contract, threshold set, admission, semantics or logos build re-measures.
fn cache_key(contract: &Contract, config: &crate::config::Config) -> String {
    let semantics = serde_json::to_string(&config.semantics).unwrap_or_default();
    let canonical = format!(
        "format={CACHE_FORMAT};metrics={};thresholds={};rules={};admission={};semantics={semantics};logos={}",
        crate::metrics::METRIC_SEMANTICS_VERSION,
        contract.thresholds.hash(),
        contract.declared.hash,
        config.admission_fingerprint(),
        env!("CARGO_PKG_VERSION"),
    );
    blake3::hash(canonical.as_bytes()).to_hex().to_string()
}

/// The cached before-state at `path`, when it is for `commit` under `key`.
/// An unreadable or mismatched entry reads as absent and is rebuilt.
fn read_cache(path: &Path, commit: &str, key: &str) -> Option<Measured> {
    let bytes = std::fs::read(path).ok()?;
    let entry: CacheEntry = serde_json::from_slice(&bytes).ok()?;
    (entry.format == CACHE_FORMAT && entry.commit == commit && entry.key == key)
        .then_some(entry.before)
}

/// Keep the newest [`CACHE_KEEP`] entries of `dir`, never removing `keep`.
/// Best-effort: a cache that cannot be pruned still serves.
fn prune_cache(dir: &Path, keep: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = read
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json") && p != keep)
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    // Newest first, then by name, so ties prune deterministically.
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, stale) in entries.into_iter().skip(CACHE_KEEP.saturating_sub(1)) {
        let _ = std::fs::remove_file(stale);
    }
}

/// Shadow-index the merge-base tree and measure it under the contract.
fn measure_tree(engine: &Engine, merge_base: &str, contract: &Contract) -> Result<Measured> {
    let (_, registry, config) = engine.pipeline_ctx()?;
    let tree = TreeExport::create(engine.root(), merge_base)?;
    let shadow = crate::pipeline::ShadowStore::create()?;
    let shadow_rt = crate::pipeline::shadow_reindex(registry, tree.path(), &config, &shadow)?;
    // The index annotated the export under the tree's own `rules.toml`, which
    // an untracked contract is not in: re-materialise under the live one.
    rematerialise(&shadow_rt, registry, &config, &contract.declared.rules)?;
    let (measured, _) = measure(&shadow_rt, contract)?;
    // The runtime releases the db before the guard removes it.
    drop(shadow_rt);
    Ok(measured)
}

/// Monotonic counter keeping each [`TreeExport`] directory unique in-process.
static EXPORT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A commit's tree written out under the OS temp dir from the object database
/// alone — a private temp index (`GIT_INDEX_FILE`), so the repository's own
/// index, worktree and refs are never touched — and removed on drop. Large
/// file filters are not run (`GIT_LFS_SKIP_SMUDGE`), so the export never
/// reaches a remote.
struct TreeExport {
    dir: PathBuf,
    tree: PathBuf,
}

impl TreeExport {
    fn create(root: &Path, commit: &str) -> Result<Self> {
        let dir = private_dir()?;
        // Owned from here, so the guard removes it on every error path below.
        let export = Self { tree: dir.join("tree"), dir };
        std::fs::create_dir(&export.tree)
            .with_context(|| format!("creating the merge-base export {}", export.tree.display()))?;
        let index = export.dir.join("index");
        let prefix = format!("--prefix={}/", export.tree.display());
        for args in [
            vec!["read-tree", commit],
            vec!["checkout-index", "--all", "--force", prefix.as_str()],
        ] {
            let out = git_command(root)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_LFS_SKIP_SMUDGE", "1")
                .args(&args)
                .output()
                .with_context(|| format!("running git {}", args[0]))?;
            anyhow::ensure!(
                out.status.success(),
                "exporting the merge-base tree {commit}: git {} failed: {}",
                args[0],
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(export)
    }

    fn path(&self) -> &Path {
        &self.tree
    }
}

/// A fresh directory under the OS temp dir that this process alone created:
/// `mkdir` is exclusive (it fails on any existing entry, a planted symlink
/// included), the name carries the clock's nanoseconds beside the pid and a
/// counter, and on Unix the mode is `0700`. A predictable path that is removed
/// and re-created could be swapped for a symlink in between by another user of
/// a shared temp dir, and `checkout-index --force` would write through it.
fn private_dir() -> Result<PathBuf> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    for _ in 0..16 {
        let seq = EXPORT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let dir = std::env::temp_dir().join(format!(
            "logos-review-{}-{seq}-{nanos:08x}",
            std::process::id()
        ));
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("creating the merge-base export {}", dir.display()))
            }
        }
    }
    anyhow::bail!("could not create a fresh merge-base export directory under the temp dir")
}

impl Drop for TreeExport {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The trust label of every file the review names ([FR-QM-18]).
///
/// [FR-QM-18]: ../../../docs/specs/requirements/FR-QM-18.md
struct Trust {
    /// Indexed files whose disk content no longer matches the graph.
    stale: BTreeSet<String>,
    /// Each named file's label.
    by_file: HashMap<String, String>,
}

impl Trust {
    fn read(engine: &Engine, runtime: &Runtime, before: &Measured, after: &Measured) -> Result<Self> {
        let named: BTreeSet<String> = [before, after]
            .into_iter()
            .flat_map(|m| {
                m.offenders
                    .iter()
                    .map(|p| p.file.clone())
                    .chain(m.violations.iter().map(|b| b.file.clone()))
            })
            .filter(|f| !f.is_empty())
            .collect();
        let paths: Vec<String> = named.iter().cloned().collect();
        let (thin, languages, indexed) = runtime.submit_read(move |store| {
            Ok((
                ThinGraph::read(store)?,
                store.file_languages(&paths)?,
                store.indexed_files()?,
            ))
        })?;
        let stale = stale_files(engine.root(), &indexed);
        let by_file = named
            .into_iter()
            .map(|file| {
                let label = if stale.contains(&file) {
                    BranchReview::TRUST_STALE.to_string()
                } else {
                    let language = languages
                        .get(&file)
                        .cloned()
                        .flatten()
                        .or_else(|| plugin_language(engine, &file));
                    language
                        .and_then(|l| thin.same_file_only(&l))
                        .map_or_else(|| BranchReview::TRUST_RESOLVED.to_string(), absence_tag)
                };
                (file, label)
            })
            .collect();
        Ok(Self { stale, by_file })
    }

    /// The label of `file`; an entry with no file (a Depth chain, a
    /// project-wide count) reads `stale` while any indexed file is.
    fn of(&self, file: &str) -> String {
        match self.by_file.get(file) {
            Some(label) => label.clone(),
            None if self.stale.is_empty() => BranchReview::TRUST_RESOLVED.to_string(),
            None => BranchReview::TRUST_STALE.to_string(),
        }
    }
}

/// The language a plugin claims `file` by — for a file the index no longer
/// holds (a resolved entry's deleted file).
fn plugin_language(engine: &Engine, file: &str) -> Option<String> {
    engine
        .registry()
        .and_then(|r| r.for_path(file))
        .map(|p| p.name().to_string())
}

/// The indexed files whose content on disk differs from the content hash the
/// graph recorded, or that are gone — the blake3 the pipeline's dirty
/// detection compares ([FR-SY-03]).
///
/// [FR-SY-03]: ../../../docs/specs/requirements/FR-SY-03.md
fn stale_files(root: &Path, indexed: &[crate::graph_store::FileRecord]) -> BTreeSet<String> {
    indexed
        .iter()
        .filter(|f| {
            let Some(recorded) = f.content_hash.as_deref() else {
                return false;
            };
            std::fs::read(root.join(&f.path))
                .map_or(true, |bytes| blake3::hash(&bytes).to_hex().as_str() != recorded)
        })
        .map(|f| f.path.clone())
        .collect()
}

/// The identity an offender is matched by within its dimension and measure.
fn offender_key(p: &Point) -> String {
    match p.dimension.as_str() {
        // A chain names directories, not a symbol: its head is its name.
        "depth" => p.name.clone(),
        // A cycle is named by its lowest-storage-id member, which a re-index
        // can change; the directories it spans are what stays.
        "acyclicity" => cycle_directories(&p.detail).to_string(),
        _ => p
            .node_id
            .clone()
            .unwrap_or_else(|| format!("{}:{}", p.file, p.name)),
    }
}

/// The directory list of an Acyclicity detail (`"N symbols across M
/// directories: a, b"`), without the size that changes as the cycle grows.
fn cycle_directories(detail: &str) -> &str {
    detail.split_once(" across ").map_or(detail, |(_, dirs)| dirs)
}

/// The status of an entry present before (`before`) and/or after (`after`).
fn status<T: Ord>(before: Option<Option<T>>, after: Option<Option<T>>) -> ReviewStatus {
    match (before, after) {
        (None, _) => ReviewStatus::New,
        (Some(_), None) => ReviewStatus::Resolved,
        (Some(Some(b)), Some(Some(a))) if a > b => ReviewStatus::Worsened,
        (Some(Some(b)), Some(Some(a))) if a < b => ReviewStatus::Improved,
        _ => ReviewStatus::Unchanged,
    }
}

/// Pair the entries of the two sides by `key`. Entries sharing a key on one
/// side — two cycles across the same directories, say — are paired in their
/// computed order rather than overwriting one another, and the surplus on
/// either side is one entry new or resolved each.
fn join<'a, T, K: Ord + Clone>(
    before: &'a [T],
    after: &'a [T],
    key: impl Fn(&T) -> K,
) -> Vec<(K, Option<&'a T>, Option<&'a T>)> {
    let mut sides: BTreeMap<K, (Vec<&'a T>, Vec<&'a T>)> = BTreeMap::new();
    for b in before {
        sides.entry(key(b)).or_default().0.push(b);
    }
    for a in after {
        sides.entry(key(a)).or_default().1.push(a);
    }
    sides
        .into_iter()
        .flat_map(|(k, (b, a))| {
            (0..b.len().max(a.len())).map(move |i| (k.clone(), b.get(i).copied(), a.get(i).copied()))
        })
        .collect()
}

/// Classify every offender of either side; see the module docs for matching.
fn classify_offenders(before: &[Point], after: &[Point], trust: &Trust) -> Vec<ReviewedOffender> {
    type Key = (usize, Option<String>, String);
    let key = |p: &Point| -> Key {
        let rank = WorstOffenders::DIMENSIONS
            .iter()
            .position(|d| *d == p.dimension)
            .unwrap_or(usize::MAX);
        (rank, p.measure.clone(), offender_key(p))
    };
    let mut entries: Vec<(usize, ReviewedOffender)> = join(before, after, key)
        .into_iter()
        .filter_map(|((rank, _, _), b, a)| {
            let current = a.or(b)?;
            Some((
                rank,
                ReviewedOffender {
                    dimension: current.dimension.clone(),
                    status: status(b.map(|p| p.value), a.map(|p| p.value)),
                    node_id: current.node_id.clone(),
                    name: current.name.clone(),
                    file: current.file.clone(),
                    line: current.line,
                    measure: current.measure.clone(),
                    before: b.and_then(|p| p.value),
                    after: a.and_then(|p| p.value),
                    threshold: current.threshold,
                    detail: current.detail.clone(),
                    trust: trust.of(&current.file),
                },
            ))
        })
        .collect();
    entries.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb)
            .then(a.status.cmp(&b.status))
            .then_with(|| a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.measure.cmp(&b.measure))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    entries.into_iter().map(|(_, e)| e).collect()
}

/// Classify every violation of either side: by rule and subject, and by
/// message too where the rule records no value.
fn classify_violations(
    before: &[Breach],
    after: &[Breach],
    contract: &Contract,
    trust: &Trust,
) -> Vec<ReviewedViolation> {
    type Key = (String, String, String);
    let key = |b: &Breach| -> Key {
        let message = if b.value.is_some() { String::new() } else { b.message.clone() };
        (b.rule.clone(), b.subject.clone(), message)
    };
    let mut entries: Vec<ReviewedViolation> = join(before, after, key)
        .into_iter()
        .filter_map(|(_, b, a)| {
            let current = a.or(b)?;
            Some(ReviewedViolation {
                rule: current.rule.clone(),
                rule_type: current.rule_type.clone(),
                status: status(b.map(|v| v.value), a.map(|v| v.value)),
                subject: current.subject.clone(),
                file: current.file.clone(),
                before: b.and_then(|v| v.value),
                after: a.and_then(|v| v.value),
                threshold: budget_max(&contract.declared.rules, &current.rule),
                message: current.message.clone(),
                trust: trust.of(&current.file),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        a.rule
            .cmp(&b.rule)
            .then(a.status.cmp(&b.status))
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.message.cmp(&b.message))
    });
    entries
}

/// The near-clone groups of one side: Uniqueness rows come one group at a
/// time, each member carrying the group's size, so a group is the run of
/// consecutive rows that share a size and a detail, cut at that size.
fn clone_groups(points: &[Point]) -> Vec<Vec<&Point>> {
    let mut groups: Vec<Vec<&Point>> = Vec::new();
    for p in points.iter().filter(|p| p.dimension == "uniqueness") {
        let size = usize::try_from(p.value.unwrap_or(0)).unwrap_or(0);
        match groups.last_mut() {
            Some(group)
                if group.len() < size
                    && group[0].value == p.value
                    && group[0].detail == p.detail =>
            {
                group.push(p);
            }
            _ => groups.push(vec![p]),
        }
    }
    groups
}

/// The groups after whose member set no group before has.
fn introduced_clone_groups(before: &[Point], after: &[Point], trust: &Trust) -> Vec<IntroducedCloneGroup> {
    let before_groups = clone_groups(before);
    let existing: HashSet<BTreeSet<String>> = before_groups
        .iter()
        .map(|g| g.iter().map(|p| offender_key(p)).collect())
        .collect();
    let duplicated_before: HashSet<String> = before_groups
        .iter()
        .flatten()
        .map(|p| offender_key(p))
        .collect();
    let mut introduced: Vec<IntroducedCloneGroup> = clone_groups(after)
        .into_iter()
        .filter(|g| !existing.contains(&g.iter().map(|p| offender_key(p)).collect::<BTreeSet<_>>()))
        .map(|g| {
            let mut members: Vec<CloneGroupMember> = g
                .iter()
                .map(|p| CloneGroupMember {
                    node_id: p.node_id.clone(),
                    name: p.name.clone(),
                    file: p.file.clone(),
                    line: p.line,
                    newly_duplicated: !duplicated_before.contains(&offender_key(p)),
                    trust: trust.of(&p.file),
                })
                .collect();
            members.sort_by(|a, b| {
                a.file
                    .cmp(&b.file)
                    .then(a.line.cmp(&b.line))
                    .then_with(|| a.name.cmp(&b.name))
            });
            IntroducedCloneGroup {
                size: members.len() as u64,
                members,
            }
        })
        .collect();
    introduced.sort_by(|a, b| {
        let first = |g: &IntroducedCloneGroup| g.members.first().map(|m| (m.file.clone(), m.line));
        first(a).cmp(&first(b))
    });
    introduced
}

/// The security section: the evidence `read` returns for `merge_base`,
/// classified ([`security_delta`]). A read that fails, a diff that cannot be
/// read, or a project with no stored report answers `measured: false` with the
/// reason — the quality half of the review stands either way.
fn security_section(
    root: &Path,
    head: &str,
    merge_base: &str,
    read: impl FnOnce(&str) -> Result<SecurityEvidence>,
) -> SecurityDelta {
    let not_measured = |why: String| SecurityDelta {
        not_measured_because: Some(why),
        ..SecurityDelta::default()
    };
    let evidence = match read(merge_base) {
        Ok(evidence) => evidence,
        Err(err) => return not_measured(format!("the security findings could not be read ({err:#})")),
    };
    if evidence.current.scanned.is_empty() {
        return not_measured(
            evidence
                .current
                .notice
                .unwrap_or("no security report is stored")
                .to_string(),
        );
    }
    let hunks = match changed_hunks(root, merge_base) {
        Ok(hunks) => hunks,
        Err(err) => return not_measured(format!("the branch's changed hunks could not be read ({err:#})")),
    };
    let mut delta = security_delta(&evidence, &hunks, |path| modified_secs(&root.join(path)));
    if head == merge_base && !evidence.base.snapshots.is_empty() {
        // One report is kept per commit and scanner, so a scan on a branch
        // with no commit of its own replaces the merge-base record.
        delta.notes.push(
            "HEAD is the merge-base: a scan run now replaces the report recorded there, so the \
             fingerprint comparison is only as old as the last scan; commit before re-scanning"
                .to_string(),
        );
    }
    delta
}

/// A file's last modification, in whole Unix seconds; `None` when it is gone
/// or its time cannot be read.
fn modified_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let secs = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Classify the evidence against the branch's `hunks` (see the module docs).
/// `modified` answers a file's modification time in Unix seconds, `None` when
/// the file is gone. Pure apart from `modified`.
fn security_delta(
    evidence: &SecurityEvidence,
    hunks: &FileRanges,
    modified: impl Fn(&str) -> Option<i64>,
) -> SecurityDelta {
    let current = &evidence.current;
    // A merge-base report that `--path` re-scans rewrote since its full scan
    // (one per commit and scanner, so a scoped run at HEAD = merge-base, as the
    // Stop hook's, replaces it) holds what the working tree said then inside its
    // scope, not the commit: it is no before-state, and that scanner is
    // classified by hunk, as one never scanned there.
    let rewritten: HashSet<&str> = evidence
        .base
        .snapshots
        .iter()
        .filter(|s| !s.scope.is_empty())
        .map(|s| s.scanner_name.as_str())
        .collect();
    let recorded = |f: &SecurityFinding| !rewritten.contains(f.scanner_name.as_str());
    // A scanner is compared by fingerprint exactly when its report was
    // recorded at the merge-base.
    let mut before: HashMap<&str, HashSet<&str>> = evidence
        .base
        .snapshots
        .iter()
        .filter(|s| s.scope.is_empty())
        .map(|s| (s.scanner_name.as_str(), HashSet::new()))
        .collect();
    for f in evidence.base.findings.iter().filter(|f| recorded(f)) {
        before.entry(f.scanner_name.as_str()).or_default().insert(f.fingerprint.as_str());
    }
    let method = |scanner: &str| {
        if before.contains_key(scanner) {
            SecurityDelta::METHOD_FINGERPRINT
        } else {
            SecurityDelta::METHOD_HUNK
        }
    };
    let suppressed: HashMap<&str, &crate::history::security::IgnoreEntry> = current
        .suppressions
        .iter()
        .filter(|e| !e.expired)
        .map(|e| (e.entry.fingerprint.as_str(), &e.entry))
        .collect();

    let mut delta = SecurityDelta {
        measured: true,
        scanners: current
            .scanned
            .iter()
            .map(|r| SecurityDeltaScanner {
                scanner: r.driver.clone(),
                method: method(&r.driver),
                latest_head: r.head_sha.clone(),
            })
            .collect(),
        ..SecurityDelta::default()
    };
    let mut entries: Vec<(SecurityDeltaEntry, Option<&crate::history::security::IgnoreEntry>)> = Vec::new();

    let mut now: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut freshness_now: HashMap<(&str, &str), &str> = HashMap::new();
    for f in current.findings.iter().map(|listed| &listed.finding) {
        now.entry(f.scanner_name.as_str()).or_default().insert(f.fingerprint.as_str());
        if let Some(path) = f.path.as_deref() {
            freshness_now.insert((f.scanner_name.as_str(), path), f.freshness);
        }
    }
    // A report with no stable fingerprint keys a finding by its region, so a
    // line added above it moves its fingerprint. The merge-base findings the
    // latest report no longer carries, by scanner, rule and file, in store
    // order: a finding outside every hunk whose fingerprint is new is paired
    // with one of them — the same finding, moved — rather than read as one
    // introduced and another resolved.
    let mut vanished: HashMap<(&str, &str, Option<&str>), std::collections::VecDeque<&str>> = HashMap::new();
    for f in evidence.base.findings.iter().filter(|f| recorded(f)) {
        if !now.get(f.scanner_name.as_str()).is_some_and(|fps| fps.contains(f.fingerprint.as_str())) {
            vanished
                .entry((f.scanner_name.as_str(), f.rule_id.as_str(), f.path.as_deref()))
                .or_default()
                .push_back(f.fingerprint.as_str());
        }
    }
    let mut moved: HashSet<(&str, &str)> = HashSet::new();
    for listed in &current.findings {
        let f = &listed.finding;
        let method = method(&f.scanner_name);
        let changed = in_hunk(hunks, f);
        // A file the branch deleted is a whole-file hunk, and deleting a file
        // introduces nothing.
        let gone = f.location().is_some_and(|p| modified(&p).is_none());
        let status = match before.get(f.scanner_name.as_str()) {
            Some(fingerprints) if !fingerprints.contains(f.fingerprint.as_str()) => SecurityDeltaStatus::Introduced,
            Some(_) if changed => SecurityDeltaStatus::Touched,
            None if changed && !gone => SecurityDeltaStatus::Introduced,
            _ => SecurityDeltaStatus::PreExisting,
        };
        let status = match status {
            SecurityDeltaStatus::Introduced if method == SecurityDelta::METHOD_FINGERPRINT && !changed => {
                let twin = vanished
                    .get_mut(&(f.scanner_name.as_str(), f.rule_id.as_str(), f.path.as_deref()))
                    .and_then(std::collections::VecDeque::pop_front);
                match twin {
                    Some(fingerprint) => {
                        moved.insert((f.scanner_name.as_str(), fingerprint));
                        SecurityDeltaStatus::PreExisting
                    }
                    None => status,
                }
            }
            status => status,
        };
        let entry = delta_entry(status, method, f, &listed.owasp, &listed.context, f.freshness);
        entries.push((entry, suppressed.get(f.fingerprint.as_str()).copied()));
    }

    // A merge-base fingerprint the latest report no longer carries.
    let ingested: HashMap<&str, i64> = evidence
        .latest
        .iter()
        .map(|s| (s.scanner_name.as_str(), s.created_at))
        .collect();
    for ((f, owasp), context) in evidence
        .base
        .findings
        .iter()
        .zip(&evidence.base_owasp)
        .zip(&evidence.base_contexts)
    {
        let scanner = f.scanner_name.as_str();
        if !recorded(f)
            || now.get(scanner).is_some_and(|fps| fps.contains(f.fingerprint.as_str()))
            || moved.contains(&(scanner, f.fingerprint.as_str()))
        {
            continue;
        }
        let freshness = match f.path.as_deref() {
            // No file to have moved: the re-scan simply no longer reports it.
            None => FRESHNESS_NA,
            Some(path) => match freshness_now.get(&(scanner, path)) {
                // The latest report still anchors the file.
                Some(freshness) => freshness,
                None => match (modified(path), ingested.get(scanner)) {
                    // Gone: no scan can report it.
                    (None, _) => FRESHNESS_NA,
                    // Strictly before, so an edit in the ingest's own second
                    // is never taken as scanned.
                    (Some(at), Some(&ingested)) if at < ingested => FRESHNESS_FRESH,
                    _ => FRESHNESS_STALE,
                },
            },
        };
        let status = if freshness == FRESHNESS_STALE {
            SecurityDeltaStatus::Stale
        } else {
            SecurityDeltaStatus::Resolved
        };
        let entry = delta_entry(status, SecurityDelta::METHOD_FINGERPRINT, f, owasp, context, freshness);
        entries.push((entry, suppressed.get(f.fingerprint.as_str()).copied()));
    }

    entries.sort_by(|(a, _), (b, _)| {
        a.status
            .cmp(&b.status)
            .then_with(|| a.path.cmp(&b.path))
            .then(a.start_line.cmp(&b.start_line))
            .then_with(|| a.scanner.cmp(&b.scanner))
            .then_with(|| a.rule_id.cmp(&b.rule_id))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    for (entry, ignore) in entries {
        match ignore {
            Some(ignore) => {
                delta.counts.suppressed += 1;
                delta.suppressed.push(SuppressedDeltaEntry {
                    entry,
                    reason: ignore.reason.clone(),
                    expires: ignore.expires.map(|d| d.to_string()),
                });
            }
            None => {
                delta.counts.add(entry.status);
                delta.findings.push(entry);
            }
        }
    }
    let not_rescanned: Vec<&str> = evidence
        .base
        .snapshots
        .iter()
        .filter(|b| evidence.latest.iter().any(|l| l.snapshot_id == b.snapshot_id))
        .map(|b| b.scanner_name.as_str())
        .collect();
    if !not_rescanned.is_empty() {
        delta.notes.push(format!(
            "{} has not re-scanned since the merge-base: a finding in a file edited since reads stale, \
             and nothing it found can be resolved until it does",
            not_rescanned.join(", ")
        ));
    }
    if !rewritten.is_empty() {
        let mut names: Vec<&str> = rewritten.iter().copied().collect();
        names.sort_unstable();
        delta.notes.push(format!(
            "{}: the merge-base report was rewritten by a --path re-scan at that commit, so it holds \
             the working tree of that run, not the commit; its findings are classified by the \
             branch's changed hunks",
            names.join(", ")
        ));
    }
    if !moved.is_empty() {
        delta.notes.push(format!(
            "{} finding(s) outside every changed hunk carry a fingerprint that moved with their line \
             (a report with no stable fingerprint); each is matched to its merge-base finding by \
             scanner, rule and file and reads pre-existing",
            moved.len()
        ));
    }
    delta.notes.push(
        "a resolved finding is one the latest re-scan no longer reports on a file it saw as it \
         stands; on a file edited since, it reads stale instead"
            .to_string(),
    );
    delta
}

/// Whether a finding lies inside a hunk the branch changed: its line range
/// overlaps one, or — with no line to place it — its whole file is new or gone.
/// The file is the finding's [location](SecurityFinding::location), so a
/// finding in a file the index did not hold when it was ingested (a new file
/// not yet synced: `unmatched`) is placed by the path its report named.
fn in_hunk(hunks: &FileRanges, f: &SecurityFinding) -> bool {
    let Some(ranges) = f.location().and_then(|p| hunks.get(&p)) else {
        return false;
    };
    let line = |n: i64| u32::try_from(n.max(1)).unwrap_or(u32::MAX);
    match f.start_line {
        Some(start) => {
            let (lo, hi) = (line(start), line(f.end_line.unwrap_or(start).max(start)));
            ranges.iter().any(|&(a, b)| a <= hi && b >= lo)
        }
        None => ranges.contains(&WHOLE_FILE),
    }
}

/// One delta entry. Every string is the stored one: a secret-class finding's
/// message is already the redacted text.
fn delta_entry(
    status: SecurityDeltaStatus,
    method: &'static str,
    f: &SecurityFinding,
    owasp: &[&'static str],
    context: &FindingContext,
    freshness: &str,
) -> SecurityDeltaEntry {
    SecurityDeltaEntry {
        status,
        method,
        scanner: f.scanner_name.clone(),
        fingerprint: f.fingerprint.clone(),
        rule_id: f.rule_id.clone(),
        severity: f.severity,
        owasp: owasp.iter().map(|c| (*c).to_string()).collect(),
        message: f.message.clone(),
        redacted: f.redacted,
        path: f.path.clone(),
        start_line: f.start_line,
        end_line: f.end_line,
        freshness: freshness.to_string(),
        reachability: wire(&context.reachability.verdict),
        confidence: wire(&context.reachability.confidence),
    }
}

/// A unit enum's wire token, read from its own serialisation rather than spelled.
fn wire(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::quality::ReviewCounts;

    fn point(dimension: &str, id: &str, value: i64, detail: &str) -> Point {
        Point {
            dimension: dimension.to_string(),
            node_id: Some(id.to_string()),
            name: id.to_string(),
            file: format!("src/{id}.rs"),
            line: Some(1),
            detail: detail.to_string(),
            measure: Some("nesting_depth".to_string()),
            value: Some(value),
            threshold: Some(4),
        }
    }

    fn no_trust() -> Trust {
        Trust {
            stale: BTreeSet::new(),
            by_file: HashMap::new(),
        }
    }

    #[test]
    fn each_class_follows_presence_and_value() {
        let before = [
            point("nesting", "worse", 5, ""),
            point("nesting", "better", 7, ""),
            point("nesting", "gone", 6, ""),
            point("nesting", "same", 5, ""),
        ];
        let after = [
            point("nesting", "worse", 6, ""),
            point("nesting", "better", 5, ""),
            point("nesting", "same", 5, ""),
            point("nesting", "fresh", 6, ""),
        ];
        let got: BTreeMap<String, (ReviewStatus, Option<i64>, Option<i64>)> =
            classify_offenders(&before, &after, &no_trust())
                .into_iter()
                .map(|e| (e.name, (e.status, e.before, e.after)))
                .collect();
        assert_eq!(got["worse"], (ReviewStatus::Worsened, Some(5), Some(6)));
        assert_eq!(got["better"], (ReviewStatus::Improved, Some(7), Some(5)));
        assert_eq!(got["gone"], (ReviewStatus::Resolved, Some(6), None));
        assert_eq!(got["same"], (ReviewStatus::Unchanged, Some(5), Some(5)));
        assert_eq!(got["fresh"], (ReviewStatus::New, None, Some(6)));
    }

    #[test]
    fn each_class_is_counted_in_its_own_slot() {
        let mut counts = ReviewCounts::default();
        for (status, times) in [
            (ReviewStatus::New, 1),
            (ReviewStatus::Worsened, 2),
            (ReviewStatus::Improved, 3),
            (ReviewStatus::Resolved, 4),
            (ReviewStatus::Unchanged, 5),
        ] {
            (0..times).for_each(|_| counts.add(status));
        }
        assert_eq!(
            counts,
            ReviewCounts { new: 1, worsened: 2, improved: 3, resolved: 4, unchanged: 5 }
        );
    }

    #[test]
    fn entries_of_one_class_are_ordered_by_file_line_and_subject() {
        let mut later = point("nesting", "later", 5, "");
        later.file = "src/b.rs".to_string();
        let mut sooner = point("nesting", "sooner", 5, "");
        sooner.file = "src/a.rs".to_string();
        sooner.line = Some(9);
        let mut first = point("nesting", "first", 5, "");
        first.file = "src/a.rs".to_string();
        let names: Vec<String> = classify_offenders(&[], &[later, sooner, first], &no_trust())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["first", "sooner", "later"]);

        let breach = |subject: &str| Breach {
            rule: "max_cc".to_string(),
            rule_type: "constraint".to_string(),
            subject: subject.to_string(),
            file: String::new(),
            value: Some(20),
            message: format!("{subject} is complex"),
        };
        let rules = Rules::default();
        let contract = Contract {
            marked: CompiledRules::compile(rules.clone(), String::new()).unwrap(),
            declared: std::sync::Arc::new(CompiledRules::compile(rules, String::new()).unwrap()),
            thresholds: crate::metrics::Thresholds::default(),
            member_access: HashSet::new(),
        };
        let subjects: Vec<String> =
            classify_violations(&[], &[breach("z()."), breach("a().")], &contract, &no_trust())
                .into_iter()
                .map(|v| v.subject)
                .collect();
        assert_eq!(subjects, ["a().", "z()."]);
    }

    #[test]
    fn a_measure_switch_is_one_entry_resolved_and_another_new() {
        let mut methods = point("focus", "Big", 25, "");
        methods.measure = Some("methods".to_string());
        let mut span = point("focus", "Big", 600, "");
        span.measure = Some("span".to_string());
        let got: Vec<(Option<String>, ReviewStatus)> =
            classify_offenders(&[methods], &[span], &no_trust())
                .into_iter()
                .map(|e| (e.measure, e.status))
                .collect();
        assert_eq!(
            got,
            vec![
                (Some("span".to_string()), ReviewStatus::New),
                (Some("methods".to_string()), ReviewStatus::Resolved),
            ]
        );
    }

    #[test]
    fn a_cycle_is_matched_by_its_directories_not_its_naming_member() {
        let before = point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/b");
        let after = point("acyclicity", "b", 4, "4 symbols across 2 directories: src/a, src/b");
        let got = classify_offenders(&[before], &[after], &no_trust());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].status, ReviewStatus::Worsened);
        let elsewhere = point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/c");
        let moved = classify_offenders(&[elsewhere], &[point("acyclicity", "a", 3, "3 symbols across 2 directories: src/a, src/b")], &no_trust());
        assert_eq!(moved.len(), 2, "a cycle over other directories is another cycle");
    }

    #[test]
    fn two_cycles_over_the_same_directories_never_overwrite_each_other() {
        let dirs = "2 symbols across 2 directories: src/a, src/b";
        let before = [point("acyclicity", "f1", 2, dirs), point("acyclicity", "f2", 2, dirs)];
        let after = [point("acyclicity", "f1", 2, dirs)];
        let got: Vec<ReviewStatus> = classify_offenders(&before, &after, &no_trust())
            .into_iter()
            .map(|e| e.status)
            .collect();
        assert_eq!(got, vec![ReviewStatus::Resolved, ReviewStatus::Unchanged], "the broken cycle is reported");
    }

    #[test]
    fn clone_groups_are_cut_at_their_size_even_when_two_groups_read_alike() {
        let rows = [
            point("uniqueness", "a1", 2, "2 functions · 10 lines"),
            point("uniqueness", "a2", 2, "2 functions · 10 lines"),
            point("uniqueness", "b1", 2, "2 functions · 10 lines"),
            point("uniqueness", "b2", 2, "2 functions · 10 lines"),
            point("uniqueness", "c1", 3, "3 functions · 10 lines"),
        ];
        let sizes: Vec<usize> = clone_groups(&rows).iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
    }

    #[test]
    fn only_a_group_with_a_new_member_set_is_introduced() {
        let kept = [
            point("uniqueness", "a1", 2, "x"),
            point("uniqueness", "a2", 2, "x"),
        ];
        let mut after = kept.to_vec();
        after.extend([point("uniqueness", "b1", 2, "y"), point("uniqueness", "b2", 2, "y")]);
        let before = [kept.to_vec(), vec![point("uniqueness", "b1", 2, "y"), point("uniqueness", "z", 2, "y")]].concat();
        let groups = introduced_clone_groups(&before, &after, &no_trust());
        assert_eq!(groups.len(), 1);
        let fresh: Vec<(&str, bool)> = groups[0]
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.newly_duplicated))
            .collect();
        assert_eq!(fresh, vec![("b1", false), ("b2", true)]);
    }

    #[test]
    fn a_valued_violation_compares_its_value_and_an_unvalued_one_its_message() {
        let contract_rules = Rules::default();
        let contract = Contract {
            marked: CompiledRules::compile(contract_rules.clone(), String::new()).unwrap(),
            declared: std::sync::Arc::new(CompiledRules::compile(contract_rules, String::new()).unwrap()),
            thresholds: crate::metrics::Thresholds::default(),
            member_access: HashSet::new(),
        };
        let breach = |rule: &str, subject: &str, value: Option<u64>, message: &str| Breach {
            rule: rule.to_string(),
            rule_type: "constraint".to_string(),
            subject: subject.to_string(),
            file: String::new(),
            value,
            message: message.to_string(),
        };
        let before = [
            breach("max_cc", "f().", Some(12), "fn `f` has cyclomatic complexity 12"),
            breach("layer-ordering", "src/a.rs", None, "a -> b"),
        ];
        let after = [
            breach("max_cc", "f().", Some(14), "fn `f` has cyclomatic complexity 14"),
            breach("layer-ordering", "src/a.rs", None, "a -> c"),
        ];
        let got: Vec<(String, ReviewStatus)> = classify_violations(&before, &after, &contract, &no_trust())
            .into_iter()
            .map(|v| (v.message, v.status))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a -> c".to_string(), ReviewStatus::New),
                ("a -> b".to_string(), ReviewStatus::Resolved),
                ("fn `f` has cyclomatic complexity 14".to_string(), ReviewStatus::Worsened),
            ]
        );
    }

    #[test]
    fn marking_switches_only_the_per_subject_budgets() {
        let rules = crate::config::parse_rules(
            "[constraints]\nmax_cc = 10\nmax_dead = 3\nno_god_containers = false\nmax_nesting_depth = 4\n",
            Path::new("rules.toml"),
        )
        .unwrap();
        let marked = marked(&rules);
        let c = &marked.constraints;
        assert!(c.max_cc.as_ref().is_some_and(Budget::is_ratchet));
        assert_eq!(c.max_cc.as_ref().map(Budget::max), Some(10));
        assert!(c.max_nesting_depth.as_ref().is_some_and(Budget::is_ratchet));
        assert_eq!(c.max_dead, rules.constraints.max_dead, "a set budget keeps its declared form");
        assert_eq!(c.no_god_containers, rules.constraints.no_god_containers, "an unenforced flag stays off");
        assert_eq!(budget_max(&rules, "max_nesting_depth"), Some(4));
        assert_eq!(budget_max(&rules, "max_dead"), None);
    }

    #[test]
    fn the_cache_keeps_the_newest_entries_and_never_the_one_just_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = |i: usize| dir.path().join(format!("{i:02}.json"));
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        for i in 0..CACHE_KEEP + 4 {
            std::fs::write(path(i), b"{}").unwrap();
            let when = base + std::time::Duration::from_secs(i as u64);
            std::fs::File::options().write(true).open(path(i)).unwrap().set_modified(when).unwrap();
        }
        std::fs::write(dir.path().join("notes.txt"), b"kept: not a cache entry").unwrap();
        // The entry just written is the oldest by mtime here, and still survives.
        prune_cache(dir.path(), &path(0));
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        let mut expected: Vec<String> = std::iter::once(0)
            .chain(CACHE_KEEP + 4 - (CACHE_KEEP - 1)..CACHE_KEEP + 4)
            .map(|i| format!("{i:02}.json"))
            .collect();
        expected.push("notes.txt".to_string());
        assert_eq!(left, expected);
    }

    #[cfg(unix)]
    #[test]
    fn the_export_directory_is_fresh_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let (a, b) = (private_dir().unwrap(), private_dir().unwrap());
        assert_ne!(a, b, "every export gets its own directory");
        let mode = std::fs::metadata(&a).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{mode:o}");
        for dir in [a, b] {
            std::fs::remove_dir(dir).unwrap();
        }
    }

    fn finding(scanner: &str, fingerprint: &str, path: Option<&str>, lines: (Option<i64>, Option<i64>)) -> SecurityFinding {
        SecurityFinding {
            scanner_name: scanner.to_string(),
            scanner_version: "1".to_string(),
            head_sha: "0".repeat(40),
            fingerprint: fingerprint.to_string(),
            rule_id: format!("rule-{fingerprint}"),
            severity: crate::model::Severity::High,
            cwes: Vec::new(),
            message: "m".to_string(),
            redacted: false,
            binding: crate::history::FindingBinding::Bound,
            report_path: path.map(str::to_string),
            path: path.map(str::to_string),
            start_line: lines.0,
            start_column: None,
            end_line: lines.1,
            end_column: None,
            freshness: FRESHNESS_FRESH,
        }
    }

    fn snapshot(id: i64, scanner: &str, created_at: i64) -> crate::history::ScannerSnapshot {
        crate::history::ScannerSnapshot {
            snapshot_id: id,
            scanner_name: scanner.to_string(),
            scanner_version: "1".to_string(),
            head_sha: "0".repeat(40),
            report_hash: String::new(),
            finding_count: 0,
            created_at,
            // A full scan: its own HEAD stands behind every finding.
            full_head_sha: Some("0".repeat(40)),
            scope: Vec::new(),
        }
    }

    /// Evidence with `current` findings in the latest report (snapshot 2,
    /// ingested at second 1000) and `base` ones recorded at the merge-base
    /// (snapshot 1), all from scanner `s`.
    fn evidence(current: Vec<SecurityFinding>, base: Vec<SecurityFinding>) -> SecurityEvidence {
        let listed = current
            .into_iter()
            .map(|finding| crate::security::ListedFinding {
                context: FindingContext::unbound(finding.start_line, crate::security::context::UnboundBecause::NoSymbol),
                finding,
                owasp: Vec::new(),
                unclassified: None,
                suppression: None,
            })
            .collect::<Vec<_>>();
        let n = base.len();
        SecurityEvidence {
            current: crate::security::SecurityFindings {
                edition: crate::history::security::OwaspEdition::default(),
                as_of: "2026-01-01".to_string(),
                scanned: vec![crate::security::ScannedReport {
                    driver: "s".to_string(),
                    version: "1".to_string(),
                    head_sha: "0".repeat(40),
                    findings: listed.len(),
                }],
                never_run: Vec::new(),
                notice: None,
                total: listed.len(),
                findings: listed,
                categories: Vec::new(),
                suppressions: Vec::new(),
            },
            latest: vec![snapshot(2, "s", 1000)],
            base: crate::history::SecurityView { snapshots: vec![snapshot(1, "s", 10)], findings: base },
            base_owasp: vec![Vec::new(); n],
            base_contexts: (0..n)
                .map(|_| FindingContext::unbound(None, crate::security::context::UnboundBecause::NoSymbol))
                .collect(),
        }
    }

    #[test]
    fn a_finding_is_in_a_hunk_only_when_its_lines_overlap_one() {
        let hunks: FileRanges = [
            ("a.py".to_string(), vec![(10, 12)]),
            ("new.py".to_string(), vec![WHOLE_FILE]),
        ]
        .into_iter()
        .collect();
        let at = |path: &str, start: Option<i64>, end: Option<i64>| in_hunk(&hunks, &finding("s", "f", Some(path), (start, end)));
        assert!(at("a.py", Some(10), None), "first line of the hunk");
        assert!(at("a.py", Some(12), Some(12)), "last line of the hunk");
        assert!(at("a.py", Some(8), Some(10)), "a region reaching into it");
        assert!(!at("a.py", Some(9), Some(9)), "the line before");
        assert!(!at("a.py", Some(13), None), "the line after");
        assert!(!at("a.py", None, None), "no line on a partly changed file");
        assert!(at("new.py", None, None), "no line on a new file");
        assert!(!at("b.py", Some(10), None), "another file");
        assert!(!in_hunk(&hunks, &finding("s", "f", None, (Some(10), None))), "no file at all");
    }

    #[test]
    fn a_deleted_file_introduces_nothing_by_hunk_but_a_new_one_does() {
        let mut evidence = evidence(
            vec![
                finding("s", "deleted", Some("deleted.py"), (Some(2), None)),
                finding("s", "added", Some("added.py"), (Some(2), None)),
            ],
            Vec::new(),
        );
        evidence.base.snapshots.clear();
        let hunks: FileRanges = [
            ("deleted.py".to_string(), vec![WHOLE_FILE]),
            ("added.py".to_string(), vec![WHOLE_FILE]),
        ]
        .into_iter()
        .collect();
        let delta = security_delta(&evidence, &hunks, |path| (path == "added.py").then_some(1));
        let got: BTreeMap<&str, (SecurityDeltaStatus, &str)> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), (e.status, e.method))).collect();
        assert_eq!(got["deleted"], (SecurityDeltaStatus::PreExisting, SecurityDelta::METHOD_HUNK));
        assert_eq!(got["added"], (SecurityDeltaStatus::Introduced, SecurityDelta::METHOD_HUNK));
    }

    #[test]
    fn a_secret_in_a_file_not_yet_indexed_is_introduced_by_its_report_path() {
        // A scan of a new file nothing synced yet: the report names the file,
        // the index does not hold it, so the finding is `unmatched` with no
        // bound path. Its hunk is still the branch's.
        let unmatched = |fingerprint: &str, report: &str| SecurityFinding {
            binding: crate::history::FindingBinding::Unmatched,
            report_path: Some(report.to_string()),
            path: None,
            freshness: FRESHNESS_NA,
            ..finding("s", fingerprint, None, (Some(1), Some(1)))
        };
        let mut evidence = evidence(
            vec![unmatched("new", "./app/new.py"), unmatched("elsewhere", "/ci/checkout/app/new.py")],
            Vec::new(),
        );
        evidence.base.snapshots.clear();
        let hunks: FileRanges = [("app/new.py".to_string(), vec![WHOLE_FILE])].into_iter().collect();
        let delta = security_delta(&evidence, &hunks, |_| Some(1));
        let got: BTreeMap<&str, SecurityDeltaStatus> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), e.status)).collect();
        assert_eq!(got["new"], SecurityDeltaStatus::Introduced, "the report path, `./` dropped, is the hunk's file");
        assert_eq!(got["elsewhere"], SecurityDeltaStatus::PreExisting, "a path spelled for another root is no match");
    }

    #[test]
    fn a_finding_that_only_moved_lines_is_paired_with_its_merge_base_twin() {
        let moved = |line: i64, fingerprint: &str| SecurityFinding {
            rule_id: "weak-hash".to_string(),
            ..finding("s", fingerprint, Some("a.py"), (Some(line), None))
        };
        // Moved below an added line, outside every hunk: one pre-existing entry.
        let delta = security_delta(
            &evidence(vec![moved(3, "at-3")], vec![moved(2, "at-2")]),
            &[("a.py".to_string(), vec![(1, 1)])].into_iter().collect(),
            |_| Some(1),
        );
        let got: Vec<(&str, SecurityDeltaStatus)> =
            delta.findings.iter().map(|e| (e.fingerprint.as_str(), e.status)).collect();
        assert_eq!(got, vec![("at-3", SecurityDeltaStatus::PreExisting)]);
        assert!(delta.notes.iter().any(|n| n.contains("moved with their line")), "{:?}", delta.notes);

        // Inside a hunk the new fingerprint is the branch's.
        let delta = security_delta(
            &evidence(vec![moved(3, "at-3")], vec![moved(2, "at-2")]),
            &[("a.py".to_string(), vec![(3, 3)])].into_iter().collect(),
            |_| Some(1),
        );
        let got: Vec<SecurityDeltaStatus> = delta.findings.iter().map(|e| e.status).collect();
        assert_eq!(got, vec![SecurityDeltaStatus::Introduced, SecurityDeltaStatus::Resolved]);

        // Another rule is another finding.
        let mut other = moved(3, "at-3");
        other.rule_id = "another-rule".to_string();
        let delta = security_delta(&evidence(vec![other], vec![moved(2, "at-2")]), &FileRanges::new(), |_| Some(1));
        let got: Vec<SecurityDeltaStatus> = delta.findings.iter().map(|e| e.status).collect();
        assert_eq!(got, vec![SecurityDeltaStatus::Introduced, SecurityDeltaStatus::Resolved]);
    }

    /// `evidence` over two scanners, `a` and `b`, both recorded at the
    /// merge-base and both re-scanned since.
    fn two_scanners(current: Vec<SecurityFinding>, base: Vec<SecurityFinding>) -> SecurityEvidence {
        let mut evidence = evidence(current, base);
        let report = evidence.current.scanned[0].clone();
        evidence.current.scanned = ["a", "b"]
            .map(|driver| crate::security::ScannedReport { driver: driver.to_string(), ..report.clone() })
            .to_vec();
        evidence.latest = vec![snapshot(3, "a", 1000), snapshot(4, "b", 1000)];
        evidence.base.snapshots = vec![snapshot(1, "a", 10), snapshot(2, "b", 10)];
        evidence
    }

    #[test]
    fn a_shared_fingerprint_is_judged_per_scanner() {
        let at = |scanner: &str| finding(scanner, "x", Some("a.py"), (Some(1), None));
        let class = |delta: &SecurityDelta| -> Vec<(String, SecurityDeltaStatus)> {
            delta.findings.iter().map(|e| (e.scanner.clone(), e.status)).collect()
        };
        // Both reported `x` at the merge-base; only `a` still does.
        let delta = security_delta(&two_scanners(vec![at("a")], vec![at("a"), at("b")]), &FileRanges::new(), |_| Some(1));
        assert_eq!(
            class(&delta),
            vec![("b".to_string(), SecurityDeltaStatus::Resolved), ("a".to_string(), SecurityDeltaStatus::PreExisting)]
        );
        // Only `a` reported `x` at the merge-base; now `b` does.
        let delta = security_delta(&two_scanners(vec![at("b")], vec![at("a")]), &FileRanges::new(), |_| Some(1));
        assert_eq!(
            class(&delta),
            vec![("b".to_string(), SecurityDeltaStatus::Introduced), ("a".to_string(), SecurityDeltaStatus::Resolved)]
        );
    }

    #[test]
    fn a_section_that_cannot_be_read_says_why_and_classifies_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let commit = "0".repeat(40);
        let failed = security_section(dir.path(), &commit, &commit, |_| Err(anyhow::anyhow!("ignore file is malformed")));
        assert!(!failed.measured);
        let why = failed.not_measured_because.unwrap_or_default();
        assert!(why.contains("findings could not be read") && why.contains("ignore file is malformed"), "{why}");

        // Not a repository: the branch's hunks cannot be read.
        let undiffable = security_section(dir.path(), &commit, &commit, |_| Ok(evidence(Vec::new(), Vec::new())));
        assert!(!undiffable.measured && undiffable.findings.is_empty());
        let why = undiffable.not_measured_because.unwrap_or_default();
        assert!(why.contains("changed hunks could not be read"), "{why}");
    }

    #[test]
    fn a_vanished_fingerprint_is_resolved_only_on_a_file_older_than_the_rescan() {
        let base = vec![
            finding("s", "older", Some("older.py"), (Some(1), None)),
            finding("s", "same-second", Some("same.py"), (Some(1), None)),
            finding("s", "gone", Some("gone.py"), (Some(1), None)),
            finding("s", "kept", Some("kept.py"), (Some(1), None)),
        ];
        let current = vec![finding("s", "kept", Some("kept.py"), (Some(1), None))];
        let delta = security_delta(&evidence(current, base), &FileRanges::new(), |path| match path {
            "older.py" => Some(999),
            "same.py" => Some(1000),
            _ => None,
        });
        let got: BTreeMap<&str, (SecurityDeltaStatus, &str)> = delta
            .findings
            .iter()
            .map(|e| (e.fingerprint.as_str(), (e.status, e.freshness.as_str())))
            .collect();
        assert_eq!(got["older"], (SecurityDeltaStatus::Resolved, FRESHNESS_FRESH));
        assert_eq!(got["same-second"], (SecurityDeltaStatus::Stale, FRESHNESS_STALE), "an edit in the ingest's second is not taken as scanned");
        assert_eq!(got["gone"], (SecurityDeltaStatus::Resolved, FRESHNESS_NA), "a deleted file's finding cannot be reported");
        assert_eq!(got["kept"].0, SecurityDeltaStatus::PreExisting);
        assert_eq!((delta.counts.resolved, delta.counts.stale, delta.counts.pre_existing), (2, 1, 1));
    }

    #[test]
    fn only_an_unexpired_entry_suppresses() {
        let mut evidence = evidence(
            vec![finding("s", "live", Some("a.py"), (Some(1), None)), finding("s", "lapsed", Some("a.py"), (Some(2), None))],
            Vec::new(),
        );
        let entry = |fingerprint: &str, expired: bool| crate::history::security::EntryStatus {
            entry: crate::history::security::IgnoreEntry {
                fingerprint: fingerprint.to_string(),
                reason: format!("reason {fingerprint}"),
                expires: chrono::NaiveDate::from_ymd_opt(2026, 1, 1),
            },
            expired,
            orphaned: false,
        };
        evidence.current.suppressions = vec![entry("live", false), entry("lapsed", true)];
        let delta = security_delta(&evidence, &FileRanges::new(), |_| None);
        assert_eq!(
            delta.suppressed.iter().map(|s| (s.entry.fingerprint.as_str(), s.reason.as_str(), s.expires.as_deref())).collect::<Vec<_>>(),
            vec![("live", "reason live", Some("2026-01-01"))]
        );
        assert_eq!(delta.findings.iter().map(|e| e.fingerprint.as_str()).collect::<Vec<_>>(), vec!["lapsed"]);
        assert_eq!(delta.counts.suppressed, 1);
    }

    #[test]
    fn a_file_less_entry_reads_stale_while_any_file_is() {
        let mut trust = no_trust();
        assert_eq!(trust.of(""), BranchReview::TRUST_RESOLVED);
        trust.stale.insert("src/x.rs".to_string());
        assert_eq!(trust.of(""), BranchReview::TRUST_STALE);
    }
}
