//! `review --headroom` ([FR-GV-29], S-798): the room left under every budget
//! `review --branches` projects ([FR-GV-28]), listed before any branch exists.
//!
//! It reads the **same base** `--branches` reads — `--base`, else the default
//! branch — through the same measurement ([`projection_contract`],
//! [`shadow_before`]) and so the same cache entry: a headroom run after a
//! `--branches` run, or the reverse, measures the commit once. The working tree
//! and `HEAD` are never measured; the live store is read only for each listed
//! subject's trust label, as `--branches` reads it.
//!
//! A subject's allowance is its budget's ceiling, except under a ratchet budget
//! ([FR-GV-23]): a subject the blessed set — read as checked in at the base
//! commit — holds above `max` has its blessed value, and one past `max` that
//! the set does not hold has no room at all. A symbol set under ratchet is one
//! row with no room: any new member fails.
//!
//! The base measurement records a subject only above 0 (the projection contract
//! rewrites every budget to `{ max = 0, ratchet = true }`), so a subject at 0 is
//! not listed: it has its whole allowance, and `--subjects` says so when an
//! entry resolves to no row.
//!
//! [FR-GV-23]: ../../../../docs/specs/requirements/FR-GV-23.md
//! [FR-GV-28]: ../../../../docs/specs/requirements/FR-GV-28.md
//! [FR-GV-29]: ../../../../docs/specs/requirements/FR-GV-29.md

use std::collections::{BTreeSet, HashMap};

use anyhow::Result;

use super::{
    coordinate_less, declared_drift_points, drift_origin, drift_reference, projection_contract, projected_limits,
    resolve_base, shadow_before, tally, Limit, Tally, Trust, NO_BASE,
};
use crate::engine::Engine;
use crate::governance::ratchet::{BlessedSet, BLESSED_FILE_REL};
use crate::governance::{load_rules_cached, quality_runtime};
use crate::metrics::SignalTerms;
use crate::models::quality::{
    BranchReview, HeadroomDrift, HeadroomReport, HeadroomRule, HeadroomSubject, UnresolvedSubject,
};
use crate::navigate::branch::{git_command, git_line, rev_parse};

/// How many of a rule's subjects an unfiltered answer lists.
const TIGHTEST: usize = 10;

/// The per-function budgets, whose subject is a function's symbol id.
const FUNCTION_RULES: [&str; 3] = ["max_cc", "max_fn_lines", "max_nesting_depth"];

/// The module-grain coupling budgets, whose subject is a module key.
const MODULE_RULES: [&str; 2] = ["max_fan_in", "max_fan_out"];

/// The per-file budget, whose subject is the file's path.
const FILE_RULE: &str = "no_god_files";

/// `review --headroom` ([FR-GV-29]): see the module docs.
///
/// `subjects` are project-relative file paths and symbol names; empty lists
/// every budget's tightest subjects. `near` keeps the subjects whose headroom
/// is at most that many; either filter lifts the cap of [`TIGHTEST`].
///
/// # Errors
/// Returns an error on a transient engine, a store fault or an invalid
/// `rules.toml`. A base that does not resolve, one that cannot be measured and
/// a blessed set the base commit holds unreadable are `measured: false`
/// answers.
///
/// [FR-GV-29]: ../../../../docs/specs/requirements/FR-GV-29.md
pub(crate) fn headroom_review(
    engine: &Engine,
    base: Option<&str>,
    subjects: &[String],
    near: Option<u64>,
) -> Result<HeadroomReport> {
    let root = engine.root();
    let (base, base_origin) = resolve_base(root, base);
    let mut out = HeadroomReport {
        base: base.clone().unwrap_or_default(),
        base_origin,
        ..HeadroomReport::default()
    };
    let Some(base) = base else {
        return Ok(unmeasured(out, NO_BASE));
    };
    let Some(base_commit) = rev_parse(root, &base) else {
        return Ok(unmeasured(out, &format!("{base} does not resolve to a commit")));
    };
    out.base_commit = Some(base_commit.clone());
    let declared = load_rules_cached(engine, None)?;
    let (limits, unprojected) = projected_limits(&declared.rules);
    out.not_projected = unprojected;
    let (drift_points, unprojected_drift) = declared_drift_points(&declared.rules);
    let drift_off = unprojected_drift.as_ref().map_or_else(
        || "rules.toml declares no [review] drift_budget".to_string(),
        |report| report.reason.clone(),
    );
    out.not_projected.extend(unprojected_drift);
    if limits.is_empty() && drift_points.is_none() {
        out.measured = true;
        out.drift = Some(not_applicable(drift_off));
        out.unresolved = no_per_subject_budget(subjects);
        out.notes.push(
            "rules.toml declares no projectable [constraints] budget and no numeric [review] drift_budget: \
             there is no limit to list"
                .to_string(),
        );
        return Ok(out);
    }
    let ratchet_rules = declared.rules.constraints.ratchet_rules();
    let blessed = match blessed_for(root, &base_commit, &limits, &ratchet_rules) {
        Ok(set) => set,
        Err(why) => return Ok(unmeasured(out, &why)),
    };
    let contract = projection_contract(engine, declared)?;
    let base_side = match shadow_before(engine, &base_commit, &contract) {
        Ok((side, _)) => side,
        Err(err) => {
            return Ok(unmeasured(out, &format!("the base commit {base_commit} could not be measured ({err:#})")))
        }
    };

    let values = tally(&base_side, &limits);
    let evaluated: HashMap<&str, u64> = limits
        .iter()
        .map(|l| (l.rule, values.keys().filter(|(rule, _)| *rule == l.rule).count() as u64))
        .collect();
    let selection = (!subjects.is_empty()).then(|| {
        let (keys, unresolved) = resolve(root, &base_commit, subjects, &limits, &values);
        out.unresolved = unresolved;
        keys
    });
    let allowances = Allowances::new(blessed.as_ref());
    let mut rules = Vec::new();
    for limit in &limits {
        let ratchet = ratchet_rules.contains(&limit.rule);
        let mut rows: Vec<HeadroomSubject> = values
            .iter()
            .filter(|((rule, _), _)| *rule == limit.rule)
            .filter(|((rule, subject), _)| {
                // A project-wide count contains no `--subjects` entry.
                selection.as_ref().is_none_or(|keys| keys.contains(&(*rule, subject.clone())))
            })
            .map(|((_, subject), (current, file))| allowances.row(limit, ratchet, subject, *current, file))
            .filter(|row| near.is_none_or(|n| row.headroom <= i64::try_from(n).unwrap_or(i64::MAX)))
            .collect();
        rows.sort_by(|a, b| a.headroom.cmp(&b.headroom).then_with(|| a.subject.cmp(&b.subject)));
        if selection.is_none() && near.is_none() {
            rows.truncate(TIGHTEST);
        }
        rules.push(HeadroomRule {
            rule: limit.rule.to_string(),
            limit: limit.ceiling,
            evaluated: if limit.per_subject { evaluated[limit.rule] } else { 1 },
            listed: rows.len() as u64,
            subjects: rows,
        });
    }
    let files: BTreeSet<String> = rules
        .iter()
        .flat_map(|r| r.subjects.iter().map(|s| s.file.clone()))
        .filter(|f| !f.is_empty())
        .collect();
    let labels = Trust::labels(engine, quality_runtime(engine)?, files, &BTreeSet::new())?;
    for subject in rules.iter_mut().flat_map(|r| r.subjects.iter_mut()) {
        subject.trust = labels
            .get(&subject.file)
            .cloned()
            .unwrap_or_else(|| BranchReview::TRUST_RESOLVED.to_string());
    }
    out.rules = rules;

    out.drift = Some(match drift_points {
        None => not_applicable(drift_off),
        Some(points) => {
            let reference = drift_reference(engine, &contract.thresholds.hash())?;
            drift_margin(points, base_side.terms.as_ref().map(SignalTerms::signal), reference, &mut out.notes)
        }
    });
    out.measured = true;
    out.notes.extend(headroom_notes(blessed.as_ref()));
    Ok(out)
}

/// The `measured: false` answer: the stated refs, the reason, nothing listed.
fn unmeasured(mut out: HeadroomReport, why: &str) -> HeadroomReport {
    out.not_measured_because = Some(why.to_string());
    out
}

/// A drift margin that does not apply, with why.
fn not_applicable(reason: String) -> HeadroomDrift {
    HeadroomDrift {
        applicable: false,
        reason: Some(reason),
        ..HeadroomDrift::default()
    }
}

/// The margin under a numeric `drift_budget` of `points`: the budget less the
/// drop from the reference [`drift_reference`] names to the base commit's
/// `signal`, with the note that says which reference it is.
fn drift_margin(
    points: f64,
    signal: Option<u32>,
    reference: std::result::Result<(u32, i64), String>,
    notes: &mut Vec<String>,
) -> HeadroomDrift {
    let Some(at_base) = signal else {
        return not_applicable("the base commit has no signal: its production graph is empty".to_string());
    };
    notes.push(match &reference {
        Ok((_, id)) => format!(
            "drift_budget's margin is measured from this checkout's blessed gate baseline (snapshot {id}), as the \
             gate would judge it: another checkout's baseline can give another answer"
        ),
        Err(why) => format!("drift_budget's margin is measured from the base commit's signal: {why}"),
    });
    let (from, reference) = drift_origin(reference, at_base);
    let drop = i64::from(from) - i64::from(at_base);
    HeadroomDrift {
        applicable: true,
        budget: Some(points),
        drop: Some(drop),
        headroom: Some(points - drop as f64),
        reference: Some(reference),
        reason: None,
    }
}

/// How every headroom answer was produced.
fn headroom_notes(blessed: Option<&BlessedSet>) -> Vec<String> {
    let mut notes = vec![
        "the values are the base commit's: a full index of a committed tree, shared with `review --branches`; \
         uncommitted work is not in the answer"
            .to_string(),
        "a module, function or file at 0 on the base is not listed under a budget: it has the whole allowance"
            .to_string(),
    ];
    match blessed {
        Some(set) if set.len() > 0 => notes.push(format!(
            "a ratchet budget's subject blessed above its max has its blessed value as allowance, read from \
             {BLESSED_FILE_REL} at the base commit; any other subject is held to max"
        )),
        Some(_) => notes.push(format!(
            "{BLESSED_FILE_REL} at the base commit blesses nothing: a ratchet budget's subjects are held to max"
        )),
        None => {}
    }
    notes
}

/// The blessed set a projection of `limits` consults, read at `commit`: `None`
/// unless some per-subject budget is under ratchet — the only budgets the set
/// allows a subject above `max` under — so a set the commit holds unusable
/// blocks only an answer that would have used it.
pub(super) fn blessed_for(
    root: &std::path::Path,
    commit: &str,
    limits: &[Limit],
    ratchet_rules: &[&'static str],
) -> std::result::Result<Option<BlessedSet>, String> {
    if limits.iter().any(|l| l.per_subject && ratchet_rules.contains(&l.rule)) {
        blessed_at(root, commit).map(Some)
    } else {
        Ok(None)
    }
}

/// The blessed set as checked in at `commit`: empty when the commit holds none,
/// the reason it cannot be used when it holds one that cannot be read or does
/// not parse.
fn blessed_at(root: &std::path::Path, commit: &str) -> std::result::Result<BlessedSet, String> {
    let unusable = |why: String| format!("{BLESSED_FILE_REL} at the base commit {commit} is unusable ({why})");
    let listed = git_command(root).args(["ls-tree", "--name-only", commit, "--", BLESSED_FILE_REL]).output();
    match listed {
        Ok(out) if out.status.success() && out.stdout.iter().all(u8::is_ascii_whitespace) => {
            return Ok(BlessedSet::default())
        }
        Ok(out) if out.status.success() => {}
        _ => return Err(unusable("it could not be looked up".to_string())),
    }
    let spec = format!("{commit}:{BLESSED_FILE_REL}");
    match git_command(root).args(["cat-file", "blob", &spec]).output() {
        Ok(out) if out.status.success() => {
            BlessedSet::parse(&String::from_utf8_lossy(&out.stdout)).map_err(|err| unusable(format!("{err:#}")))
        }
        _ => Err(unusable("it could not be read".to_string())),
    }
}

/// Each budget subject's allowance, from its ceiling or its blessed value —
/// the one rule `--headroom` lists and `--branches` projects against.
pub(super) struct Allowances {
    /// Blessed value by (constraint, coordinate-less symbol id).
    blessed: HashMap<(String, String), u64>,
}

impl Allowances {
    pub(super) fn new(blessed: Option<&BlessedSet>) -> Self {
        let blessed = blessed
            .into_iter()
            .flat_map(BlessedSet::entries)
            .map(|(rule, symbol, value)| ((rule.to_string(), coordinate_less(symbol)), value))
            .collect();
        Self { blessed }
    }

    /// The value the blessed set holds `subject` at under `limit`, when the
    /// budget is a per-subject ratchet and the set holds it above `max`.
    fn blessed_above_max(&self, limit: &Limit, ratchet: bool, subject: &str) -> Option<u64> {
        if !ratchet || !limit.per_subject {
            return None;
        }
        let blessed = *self.blessed.get(&(limit.rule.to_string(), subject.to_string()))?;
        (blessed > limit.ceiling).then_some(blessed)
    }

    /// The value `subject` may reach under `limit` before `--branches` lists
    /// it: its blessed value under a ratchet budget that holds it above `max`,
    /// else `max`. `ratchet` is whether the budget is declared under ratchet.
    pub(super) fn ceiling(&self, limit: &Limit, ratchet: bool, subject: &str) -> u64 {
        self.blessed_above_max(limit, ratchet, subject).unwrap_or(limit.ceiling)
    }

    /// `subject`'s row under `limit`, `current` on the base. `ratchet` is
    /// whether the budget is declared under ratchet.
    fn row(&self, limit: &Limit, ratchet: bool, subject: &str, current: u64, file: &str) -> HeadroomSubject {
        let signed = |v: u64| i64::try_from(v).unwrap_or(i64::MAX);
        let row = |kind, allowance: u64, reason: Option<String>| HeadroomSubject {
            subject: subject.to_string(),
            file: file.to_string(),
            trust: String::new(),
            kind,
            allowance,
            current,
            headroom: signed(allowance) - signed(current),
            reason,
        };
        let max = limit.ceiling;
        if !ratchet {
            return row(HeadroomSubject::LIMIT, max, None);
        }
        if !limit.per_subject {
            return row(
                HeadroomSubject::RATCHET,
                current,
                Some("a symbol set under ratchet: any member beyond those on record is new debt".to_string()),
            );
        }
        match self.blessed_above_max(limit, ratchet, subject) {
            Some(blessed) => row(HeadroomSubject::RATCHET, blessed, None),
            None if current > max => row(
                HeadroomSubject::RATCHET,
                current,
                Some(format!(
                    "past max = {max} and not blessed above it at the base commit: any further rise is new debt"
                )),
            ),
            None => row(HeadroomSubject::LIMIT, max, None),
        }
    }
}

/// `--subjects` entries that select nothing because no per-subject budget is
/// declared.
fn no_per_subject_budget(entries: &[String]) -> Vec<UnresolvedSubject> {
    entries
        .iter()
        .map(|entry| UnresolvedSubject {
            entry: entry.clone(),
            reason: "no per-subject [constraints] budget is declared".to_string(),
        })
        .collect()
}

/// The (budget, subject) pairs the `--subjects` entries resolve to on the base,
/// and each entry that resolved to none, with why.
///
/// A file path names its module (the coupling budgets), itself (`no_god_files`)
/// and its functions (the per-function budgets). A symbol name names the
/// functions it matches for the per-function budgets, plus their file and
/// module; a module's name names that module. Only subjects the measurement
/// records — a value above 0 — can be named.
fn resolve(
    root: &std::path::Path,
    commit: &str,
    entries: &[String],
    limits: &[Limit],
    values: &Tally,
) -> (BTreeSet<(&'static str, String)>, Vec<UnresolvedSubject>) {
    if !limits.iter().any(|l| l.per_subject) {
        return (BTreeSet::new(), no_per_subject_budget(entries));
    }
    let mut keys = BTreeSet::new();
    let mut unresolved = Vec::new();
    for entry in entries {
        match resolve_entry(root, commit, entry, values) {
            Ok(found) => keys.extend(found),
            Err(reason) => unresolved.push(UnresolvedSubject {
                entry: entry.clone(),
                reason: reason.to_string(),
            }),
        }
    }
    (keys, unresolved)
}

/// What one `--subjects` entry selects, or why it selects nothing.
fn resolve_entry(
    root: &std::path::Path,
    commit: &str,
    entry: &str,
    values: &Tally,
) -> std::result::Result<BTreeSet<(&'static str, String)>, &'static str> {
    let path = entry.trim().strip_prefix("./").unwrap_or(entry.trim());
    if path.is_empty() {
        return Err("an empty entry names nothing");
    }
    let found: BTreeSet<_> = match object_kind(root, commit, path).as_deref() {
        Some("blob") => values
            .iter()
            .filter(|(_, (_, file))| file == path)
            .map(|((rule, subject), _)| (*rule, subject.clone()))
            .collect(),
        Some("tree") => return Err("a directory: name a file in it, or a function or module"),
        _ if path.contains('/') => return Err("not in the base commit"),
        _ => by_name(values, path),
    };
    if !found.is_empty() {
        return Ok(found);
    }
    Err(if path.contains('/') || object_kind(root, commit, path).is_some() {
        "it is in the base commit, but no declared budget records a value for it: every subject of it is at 0, or \
         it holds no indexed code"
    } else {
        "no function or module of that name has a value under a declared budget on the base commit"
    })
}

/// What `path` is in `commit`'s tree: `blob` (a file), `tree` (a directory), or
/// `None` when the commit holds nothing there.
fn object_kind(root: &std::path::Path, commit: &str, path: &str) -> Option<String> {
    git_line(root, &["cat-file", "-t", &format!("{commit}:{path}")])
}

/// What the symbol name `name` selects: every function it names under the
/// per-function budgets with the file and module of each, and every module it
/// names under the coupling budgets.
fn by_name(values: &Tally, name: &str) -> BTreeSet<(&'static str, String)> {
    let mut files: BTreeSet<&str> = BTreeSet::new();
    let mut found = BTreeSet::new();
    for ((rule, subject), (_, file)) in values {
        if FUNCTION_RULES.contains(rule) && function_names(subject).iter().any(|n| n == name) {
            files.insert(file.as_str());
            found.insert((*rule, subject.clone()));
        } else if MODULE_RULES.contains(rule) && module_name(subject).as_deref() == Some(name) {
            found.insert((*rule, subject.clone()));
        }
    }
    for ((rule, subject), (_, file)) in values {
        if (*rule == FILE_RULE || MODULE_RULES.contains(rule)) && files.contains(file.as_str()) {
            found.insert((*rule, subject.clone()));
        }
    }
    found
}

/// The last `/`-separated segment of a coordinate-less symbol id or module key,
/// without its trailing `.` and its parameter group: `src/`engine.rs`/Engine#scan().`
/// gives `Engine#scan`, a Java overload ``Foo#bar(1).`` gives `Foo#bar`, and
/// `module:src/`engine.rs`/` gives `` `engine.rs` ``. Backticks stay; callers
/// strip them.
fn descriptor(subject: &str) -> &str {
    let piece = subject.trim_end_matches('/').rsplit('/').next().unwrap_or_default().trim_end_matches('.');
    piece
        .strip_suffix(')')
        .and_then(|head| head.rfind('(').map(|open| &head[..open]))
        .unwrap_or(piece)
}

/// The names a function symbol answers to: its own (`scan`) and, where the
/// symbol carries its owner (a Java or Python method, `Foo#bar`), the owner's
/// (`Foo::bar`). A Rust or Go method's symbol names no owner: it answers to its
/// own name only.
fn function_names(subject: &str) -> Vec<String> {
    let piece = descriptor(subject).replace('`', "");
    let qualified = piece.replace(['#', '.'], "::");
    let own = piece.rsplit(['#', '.']).next().unwrap_or_default().to_string();
    let mut names = vec![own, qualified];
    names.dedup();
    names
}

/// A module key's name: its file's name (`module:src/`engine.rs`/` →
/// `engine.rs`).
fn module_name(subject: &str) -> Option<String> {
    let name = descriptor(subject.strip_prefix("module:")?).replace('`', "");
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(rule: &'static str, ceiling: u64, per_subject: bool) -> Limit {
        Limit {
            rule,
            ceiling,
            per_subject,
        }
    }

    fn allowances(entries: &[(&str, &str, u64)]) -> Allowances {
        Allowances {
            blessed: entries
                .iter()
                .map(|(rule, symbol, value)| ((rule.to_string(), symbol.to_string()), *value))
                .collect(),
        }
    }

    #[test]
    fn a_limit_subject_reads_its_room_under_the_ceiling_and_a_breach_reads_negative() {
        let a = allowances(&[]);
        let cc = limit("max_fan_out", 30, true);
        let row = a.row(&cc, false, "module:m", 28, "m.rs");
        assert_eq!((row.kind, row.allowance, row.current, row.headroom), ("limit", 30, 28, 2));
        let over = a.row(&cc, false, "module:m", 33, "m.rs");
        assert_eq!(over.headroom, -3, "a subject past its limit has negative room");
    }

    #[test]
    fn a_ratchet_subject_reads_against_its_blessed_value_or_has_no_room_past_max() {
        let a = allowances(&[("max_cc", "f", 22)]);
        let cc = limit("max_cc", 15, true);
        let blessed = a.row(&cc, true, "f", 20, "a.rs");
        assert_eq!((blessed.kind, blessed.allowance, blessed.headroom), ("ratchet", 22, 2));
        assert_eq!(blessed.reason, None);
        let unblessed = a.row(&cc, true, "g", 17, "a.rs");
        assert_eq!((unblessed.kind, unblessed.headroom), ("ratchet", 0));
        assert!(unblessed.reason.as_deref().is_some_and(|r| r.contains("not blessed")), "{unblessed:?}");
        let under = a.row(&cc, true, "h", 10, "a.rs");
        assert_eq!((under.kind, under.allowance, under.headroom), ("limit", 15, 5));
        let worsened = a.row(&cc, true, "f", 24, "a.rs");
        assert_eq!(worsened.headroom, -2, "worse than its blessed value");
        let at_max = allowances(&[("max_cc", "f", 15)]);
        let row = at_max.row(&cc, true, "f", 14, "a.rs");
        assert_eq!((row.kind, row.allowance, row.reason), ("limit", 15, None), "blessed at max is no allowance above it");
        let exactly = a.row(&cc, true, "g", 15, "a.rs");
        assert_eq!((exactly.kind, exactly.headroom), ("limit", 0), "a subject exactly at max is not past it");
        let stale = allowances(&[("max_cc", "f", 12)]).row(&cc, true, "f", 17, "a.rs");
        assert_eq!((stale.kind, stale.headroom), ("ratchet", 0));
        assert!(!stale.reason.unwrap().contains("not blessed at"), "a blessed-below-max subject is not called unblessed");
        let weaker = allowances(&[("max_cc", "f", 12)]);
        let row = weaker.row(&cc, true, "f", 11, "a.rs");
        assert_eq!((row.kind, row.allowance), ("limit", 15), "a blessed value at or below max is not an allowance");
    }

    /// A symbol set has no per-subject allowance: a stale blessed entry for it
    /// never lifts its ceiling, and a budget not under ratchet ignores the set.
    #[test]
    fn only_a_per_subject_ratchet_budget_takes_its_ceiling_from_the_blessed_set() {
        let a = allowances(&[("max_dead", "", 9), ("max_cc", "f", 22), ("max_fan_out", "m", 40)]);
        assert_eq!(a.ceiling(&limit("max_dead", 0, false), true, ""), 0, "a symbol set keeps its max");
        assert_eq!(a.ceiling(&limit("max_cc", 15, true), true, "f"), 22);
        assert_eq!(a.ceiling(&limit("max_cc", 15, true), false, "f"), 15, "not under ratchet");
        assert_eq!(a.ceiling(&limit("max_fan_out", 30, true), true, "m"), 40);
        assert_eq!(a.ceiling(&limit("max_fan_out", 30, true), false, "m"), 30);
    }

    #[test]
    fn a_symbol_set_under_ratchet_is_one_row_with_no_room() {
        let a = allowances(&[]);
        let dead = limit("max_dead", 0, false);
        let row = a.row(&dead, true, "", 3, "");
        assert_eq!((row.kind, row.current, row.headroom), ("ratchet", 3, 0));
        let absolute = a.row(&limit("max_dead", 5, false), false, "", 3, "");
        assert_eq!((absolute.kind, absolute.headroom), ("limit", 2));
    }

    #[test]
    fn a_drift_budget_of_5_with_a_drop_of_3_has_a_margin_of_2_from_its_named_reference() {
        let mut notes = Vec::new();
        // A gate baseline at 73 against a base commit at 70: the signal fell 3.
        let drift = drift_margin(5.0, Some(70), Ok((73, 9)), &mut notes);
        assert!(drift.applicable, "{drift:?}");
        assert_eq!((drift.budget, drift.drop, drift.headroom), (Some(5.0), Some(3), Some(2.0)));
        let reference = drift.reference.expect("the reference is named");
        assert_eq!((reference.kind, reference.snapshot_id), ("baseline", Some(9)));
        assert!(notes[0].contains("snapshot 9"), "{notes:?}");

        // A base above the baseline has risen: the margin is more than the budget.
        let risen = drift_margin(5.0, Some(75), Ok((73, 9)), &mut Vec::new());
        assert_eq!((risen.drop, risen.headroom), (Some(-2), Some(7.0)));
        let past = drift_margin(5.0, Some(66), Ok((73, 9)), &mut Vec::new());
        assert_eq!(past.headroom, Some(-2.0), "a drop past the budget reads negative");

        let from_base = drift_margin(5.0, Some(70), Err("no gate baseline is saved".to_string()), &mut notes);
        assert_eq!((from_base.drop, from_base.headroom), (Some(0), Some(5.0)));
        assert_eq!(from_base.reference.unwrap().kind, "base");
        let empty = drift_margin(5.0, None, Ok((73, 9)), &mut Vec::new());
        assert!(!empty.applicable && empty.reason.unwrap().contains("no signal"), "a base with no signal has no margin");
    }

    #[test]
    fn a_symbol_name_matches_its_own_name_or_its_owners_not_a_neighbour() {
        let method = "src/`engine.rs`/Engine#scan().";
        assert_eq!(function_names(method), ["scan", "Engine::scan"]);
        assert_eq!(function_names("src/`engine.rs`/run()."), ["run"]);
        let neighbour = function_names("src/`engine.rs`/rescan().");
        assert!(!neighbour.iter().any(|n| n == "scan"), "{neighbour:?}");
        assert_eq!(function_names("src/`engine.rs`/scan()."), ["scan"], "a Rust method names no owner");
        assert_eq!(function_names("java/`Foo.java`/Foo#bar(1)."), ["bar", "Foo::bar"], "an overload is its method");
        assert_eq!(function_names("java/`Foo.java`/Foo#bar()."), ["bar", "Foo::bar"]);
        assert_eq!(module_name("module:src/`engine.rs`/").as_deref(), Some("engine.rs"));
        assert_eq!(module_name(method), None, "a function is not a module");
    }

    #[test]
    fn a_name_selects_its_functions_with_their_file_and_module_and_nothing_else() {
        let mut values = Tally::new();
        let scan = "src/`engine.rs`/Engine#scan().";
        let other = "src/`engine.rs`/other().";
        for (rule, subject, value, file) in [
            ("max_cc", scan, 20, "src/engine.rs"),
            ("max_cc", other, 9, "src/engine.rs"),
            ("max_fan_out", "module:src/`engine.rs`/", 28, "src/engine.rs"),
            ("max_fan_out", "module:src/`lib.rs`/", 4, "src/lib.rs"),
            ("no_god_files", "src/engine.rs", 40, "src/engine.rs"),
            ("no_god_files", "src/lib.rs", 3, "src/lib.rs"),
        ] {
            values.insert((rule, subject.to_string()), (value, file.to_string()));
        }
        let found = by_name(&values, "scan");
        let rules: Vec<(&str, &str)> = found.iter().map(|(r, s)| (*r, s.as_str())).collect();
        assert_eq!(
            rules,
            [
                ("max_cc", scan),
                ("max_fan_out", "module:src/`engine.rs`/"),
                ("no_god_files", "src/engine.rs"),
            ]
        );
        assert!(by_name(&values, "missing").is_empty());
        let module = by_name(&values, "engine.rs");
        assert_eq!(
            module.iter().map(|(r, s)| (*r, s.as_str())).collect::<Vec<_>>(),
            [("max_fan_out", "module:src/`engine.rs`/")],
            "a module's name names that module and nothing of its file"
        );
    }
}
