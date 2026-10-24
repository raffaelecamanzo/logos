//! The **attribution** of an aggregate change ([FR-GV-26], [ADR-89], S-755):
//! how far each dimension moved the 0–10000 signal between two measurements,
//! and which symbols moved it.
//!
//! # The decomposition
//!
//! The signal is `S = 10000 · exp(Σ ln nᵢ / k)` over the `k` applicable
//! dimensions ([`signal_terms`](crate::metrics::signal_terms)), so in log space
//! it is a sum: `Δ ln S = Σᵢ (ln nᵢᵃ / kₐ − ln nᵢᵇ / k_b)`, a dimension present
//! on one side only contributing its one term. Each term is that dimension's
//! share, and the logarithmic mean `L = (Sₐ − S_b) / Δ ln S` of the two
//! **exact** signals turns the shares into points that sum to `Sₐ − S_b`
//! exactly (the LMDI identity). The gate compares the two *rounded* signals,
//! so the observed change differs from the exact one by their rounding — under
//! one point by construction — which is stated beside it, never folded into a
//! dimension. The residual is what the contributions, each rounded to two
//! decimals, leave of the exact change.
//!
//! A signal the [ADR-12] zero short-circuit collapsed has no finite log; an
//! empty production graph has no signal at all. Either side being so, the change is stated and not
//! attributed.
//!
//! # The drivers
//!
//! Each contribution names the offenders that pushed its dimension the way its
//! **value** moved — `new` and `worsened` for a fall, `resolved` and
//! `improved` for a rise — largest change first, capped at
//! [`SignalAttribution::DRIVER_CAP`] with the total stated. The value, not the
//! sign of the share: when a dimension enters or leaves the mean, every other
//! term is re-weighted, so an improved dimension can carry a negative share —
//! and its drivers are still the entries that improved it. A dimension whose
//! value did not move names none; one that entered or left the mean is read by
//! its share. Uniqueness is
//! driven by near-clone groups, so it names the groups that appeared (a fall)
//! or went (a rise) rather than their members one by one.
//!
//! [FR-GV-26]: ../../../docs/specs/requirements/FR-GV-26.md
//! [ADR-89]: ../../../docs/specs/architecture/decisions/ADR-89.md
//! [ADR-12]: ../../../docs/specs/architecture/decisions/ADR-12.md

use std::cmp::Reverse;

use crate::metrics::insight::group_identity;
use crate::metrics::SignalTerms;
use crate::models::quality::{
    AttributionDriver, DimensionContribution, DriverMember, FixUnit, FixUnitKind, ReviewStatus,
    ReviewedOffender, SignalAttribution,
};

/// What names the drivers of each contribution: the classified offenders of
/// the two measurements, and the near-clone groups only one side has.
#[derive(Debug, Default)]
pub(super) struct Drivers<'a> {
    /// Every offender the change touched; `unchanged` ones drive nothing.
    pub(super) offenders: &'a [ReviewedOffender],
    /// Near-clone groups present after and not before, each its members.
    pub(super) introduced_groups: Vec<Vec<DriverMember>>,
    /// Near-clone groups present before and not after.
    pub(super) removed_groups: Vec<Vec<DriverMember>>,
}

/// Attribute the change from `before` to `after`; see the module docs.
pub(super) fn attribute(
    before: Option<&SignalTerms>,
    after: Option<&SignalTerms>,
    drivers: &Drivers<'_>,
) -> SignalAttribution {
    let mut attribution = SignalAttribution {
        before: before.map(SignalTerms::signal),
        after: after.map(SignalTerms::signal),
        ..SignalAttribution::default()
    };
    let (Some(b), Some(a)) = (before, after) else {
        attribution.not_attributable_because =
            Some("a side has no signal: its production graph is empty, so there is no change to attribute".to_string());
        return attribution;
    };
    let change = i64::from(a.signal()) - i64::from(b.signal());
    attribution.change = Some(change);
    if b.collapsed() || a.collapsed() {
        attribution.not_attributable_because = Some(
            "a signal is 0 by the zero short-circuit (an applicable original dimension is 0), \
             which has no finite log-space term"
                .to_string(),
        );
        return attribution;
    }
    let (log_before, log_after) = (b.log_mean(), a.log_mean());
    let log_change = log_after - log_before;
    let (exact_before, exact_after) = (10_000.0 * log_before.exp(), 10_000.0 * log_after.exp());
    // The logarithmic mean of the two exact signals; its limit is the signal
    // itself when the two coincide.
    let scale = if log_change.abs() < 1e-12 {
        exact_after
    } else {
        (exact_after - exact_before) / log_change
    };
    let mut contributions: Vec<(usize, DimensionContribution)> = FixUnit::DIMENSIONS
        .iter()
        .enumerate()
        .filter_map(|(rank, &dimension)| {
            let (vb, va) = (value(b, dimension), value(a, dimension));
            if vb.is_none() && va.is_none() {
                return None;
            }
            let share = term(va, a) - term(vb, b);
            let points = round2(scale * share);
            let (drivers, driver_count) = drivers_of(dimension, falling(vb, va, points), drivers);
            Some((
                rank,
                DimensionContribution {
                    dimension: dimension.to_string(),
                    before: vb,
                    after: va,
                    points,
                    drivers,
                    driver_count,
                },
            ))
        })
        .collect();
    contributions.sort_by(|(ra, a), (rb, b)| b.points.abs().total_cmp(&a.points.abs()).then(ra.cmp(rb)));
    let exact_change = exact_after - exact_before;
    let carried: f64 = contributions.iter().map(|(_, c)| c.points).sum();
    attribution.exact_change = Some(round2(exact_change));
    attribution.rounding = Some(round2(change as f64 - exact_change));
    attribution.residual = Some(round2(exact_change - carried));
    attribution.contributions = contributions.into_iter().map(|(_, c)| c).collect();
    attribution
}

/// `dimension`'s normalized value on one side, when it was applicable there.
fn value(terms: &SignalTerms, dimension: &str) -> Option<f64> {
    terms.all().find(|(name, _)| name == dimension).map(|(_, n)| *n)
}

/// A dimension's term in its side's `ln S`: `ln n / k`, or nothing when it did
/// not enter that side's mean.
fn term(value: Option<f64>, terms: &SignalTerms) -> f64 {
    value.map_or(0.0, |n| n.ln() / terms.all().count() as f64)
}

/// `x` to two decimals, with no negative zero.
fn round2(x: f64) -> f64 {
    let rounded = (x * 100.0).round() / 100.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

/// Whether a dimension fell (`Some(true)`), rose, or did not move (`None`):
/// by its value when it applied on both sides, else by its share.
fn falling(before: Option<f64>, after: Option<f64>, points: f64) -> Option<bool> {
    match (before, after) {
        (Some(b), Some(a)) => (a != b).then_some(a < b),
        _ => (points != 0.0).then_some(points < 0.0),
    }
}

/// The capped drivers of `dimension`, moving the way `falling` says, and how
/// many there were.
fn drivers_of(dimension: &str, falling: Option<bool>, drivers: &Drivers<'_>) -> (Vec<AttributionDriver>, u64) {
    let Some(falling) = falling else {
        return (Vec::new(), 0);
    };
    let mut ranked: Vec<(i64, AttributionDriver)> = if dimension == "uniqueness" {
        let (groups, status) = if falling {
            (&drivers.introduced_groups, ReviewStatus::New)
        } else {
            (&drivers.removed_groups, ReviewStatus::Resolved)
        };
        groups.iter().filter_map(|members| clone_group(members, status)).collect()
    } else {
        drivers
            .offenders
            .iter()
            .filter(|e| e.dimension == dimension && pushes(e.status, falling))
            .map(|e| (magnitude(e), symbol(e)))
            .collect()
    };
    ranked.sort_by(|(ma, a), (mb, b)| {
        Reverse(*ma)
            .cmp(&Reverse(*mb))
            .then_with(|| a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.measure.cmp(&b.measure))
    });
    let count = ranked.len() as u64;
    let capped = ranked
        .into_iter()
        .take(SignalAttribution::DRIVER_CAP)
        .map(|(_, d)| d)
        .collect();
    (capped, count)
}

/// `true` when an entry of `status` pushes its dimension down (`falling`) or up.
fn pushes(status: ReviewStatus, falling: bool) -> bool {
    match status {
        ReviewStatus::New | ReviewStatus::Worsened => falling,
        ReviewStatus::Resolved | ReviewStatus::Improved => !falling,
        ReviewStatus::Unchanged => false,
    }
}

/// How far an entry moved: its value when it appeared or went, else the
/// difference.
fn magnitude(e: &ReviewedOffender) -> i64 {
    match (e.before, e.after) {
        (Some(b), Some(a)) => (a - b).abs(),
        (b, a) => a.or(b).unwrap_or(0),
    }
}

fn symbol(e: &ReviewedOffender) -> AttributionDriver {
    AttributionDriver {
        status: e.status,
        name: e.name.clone(),
        node_id: e.node_id.clone(),
        file: e.file.clone(),
        line: e.line,
        measure: e.measure.clone(),
        before: e.before,
        after: e.after,
        members: Vec::new(),
    }
}

/// A near-clone group as a driver, ranked by its size and known the way its
/// "where to start" unit is ([`group_identity`]): `node_id` is
/// `clone_group:<digest>` over its members' identities, and the member first in
/// identity order names it. Both are functions of the member set alone, so a
/// re-index or a file reorder leaves them as they were. A group with a member
/// that carries no identity names none — it has no id to derive, and is shown
/// by its first member in file order, never by a fabricated id; `None` for an
/// empty group.
fn clone_group(members: &[DriverMember], status: ReviewStatus) -> Option<(i64, AttributionDriver)> {
    let first = members.first()?;
    let size = members.len() as i64;
    let (before, after) = match status {
        ReviewStatus::New => (None, Some(size)),
        _ => (Some(size), None),
    };
    let identities: Option<Vec<&str>> = members.iter().map(|m| m.node_id.as_deref()).collect();
    let (node_id, named_by) = match identities {
        Some(ids) => (
            Some(group_identity(FixUnitKind::CloneGroup, &ids)),
            members.iter().min_by(|a, b| a.node_id.cmp(&b.node_id)).unwrap_or(first),
        ),
        None => (None, first),
    };
    Some((
        size,
        AttributionDriver {
            status,
            name: named_by.name.clone(),
            node_id,
            file: named_by.file.clone(),
            line: named_by.line,
            measure: Some("group_size".to_string()),
            before,
            after,
            members: members.to_vec(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;

    fn terms(original: &[(&'static str, f64)], extended: &[(&'static str, f64)]) -> SignalTerms {
        let named = |t: &[(&'static str, f64)]| t.iter().map(|(n, v)| (Cow::Borrowed(*n), *v)).collect();
        SignalTerms {
            original: named(original),
            extended: named(extended),
        }
    }

    const ORIGINAL: [(&str, f64); 5] = [
        ("modularity", 0.811230),
        ("acyclicity", 1.0),
        ("depth", 0.421053),
        ("equality", 0.493762),
        ("redundancy", 0.962988),
    ];
    const EXTENDED: [(&str, f64); 5] = [
        ("nesting", 0.980825),
        ("conciseness", 0.998439),
        ("cohesion", 1.0),
        ("focus", 1.0),
        ("uniqueness", 0.964548),
    ];

    fn sum(a: &SignalAttribution) -> f64 {
        a.contributions.iter().map(|c| c.points).sum()
    }

    fn offender(dimension: &str, name: &str, status: ReviewStatus, before: Option<i64>, after: Option<i64>) -> ReviewedOffender {
        ReviewedOffender {
            dimension: dimension.to_string(),
            status,
            node_id: Some(format!("id/{name}")),
            name: name.to_string(),
            file: format!("src/{name}.rs"),
            line: Some(1),
            measure: Some("value".to_string()),
            before,
            after,
            threshold: Some(10),
            detail: String::new(),
            trust: "resolved".to_string(),
            gating: false,
        }
    }

    /// The Sprint 106 I7-INT measurements (sprint-impl-106.md): base
    /// `27dc0b5a` against the merged `def5c501`, exact 8290.59 → 8288.31.
    fn sprint_106_merged() -> SignalTerms {
        terms(
            &[
                ("modularity", 0.810668),
                ("acyclicity", 1.0),
                ("depth", 0.421053),
                ("equality", 0.493195),
                ("redundancy", 0.962898),
            ],
            &[
                ("nesting", 0.980786),
                ("conciseness", 0.998454),
                ("cohesion", 1.0),
                ("focus", 1.0),
                ("uniqueness", 0.963781),
            ],
        )
    }

    #[test]
    fn contributions_sum_to_the_change_within_the_stated_residual() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let after = sprint_106_merged();
        let a = attribute(Some(&before), Some(&after), &Drivers::default());
        assert_eq!((a.before, a.after, a.change), (Some(8291), Some(8288), Some(-3)));
        // The exact change is 8288.31 − 8290.59; the integer signals round it
        // by −0.72, which no dimension is charged with.
        assert_eq!((a.exact_change, a.rounding), (Some(-2.28), Some(-0.72)));
        let residual = a.residual.expect("attributable");
        assert!(residual.abs() < 0.5, "residual {residual}");
        assert!(
            (sum(&a) + residual - a.exact_change.unwrap()).abs() < 1e-9,
            "contributions {} + residual {residual} make the exact change",
            sum(&a)
        );
        let named: Vec<&str> = a.contributions.iter().map(|c| c.dimension.as_str()).collect();
        // ln(0.493195/0.493762) = −0.00115 outweighs Uniqueness's −0.00080 and
        // Modularity's −0.00069.
        assert_eq!(named[..3], ["equality", "uniqueness", "modularity"], "largest share first");
        assert_eq!(a.contributions.len(), 10, "every applicable dimension is listed");
    }

    #[test]
    fn a_dimension_entering_the_mean_carries_its_own_term() {
        // Cohesion drops out after: the mean re-weights over nine dimensions.
        let before = terms(&ORIGINAL, &EXTENDED);
        let after = terms(
            &ORIGINAL,
            &[("nesting", 0.980825), ("conciseness", 0.998439), ("focus", 1.0), ("uniqueness", 0.964548)],
        );
        let a = attribute(Some(&before), Some(&after), &Drivers::default());
        let residual = a.residual.expect("attributable");
        assert!(residual.abs() < 0.5, "residual {residual}");
        assert!((sum(&a) + residual - a.exact_change.unwrap()).abs() < 1e-9);
        let cohesion = a.contributions.iter().find(|c| c.dimension == "cohesion").expect("listed");
        assert_eq!((cohesion.before, cohesion.after), (Some(1.0), None));
        // Every other dimension's term grew by 1/9 − 1/10 of its log, so the
        // drop-out of a perfect dimension lowers the signal.
        assert!(a.change.unwrap() < 0, "{a:?}");
    }

    #[test]
    fn no_change_attributes_nothing_and_leaves_no_residual() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let a = attribute(Some(&before), Some(&before), &Drivers::default());
        assert_eq!(
            (a.change, a.exact_change, a.rounding, a.residual),
            (Some(0), Some(0.0), Some(0.0), Some(0.0))
        );
        assert!(a.contributions.iter().all(|c| c.points == 0.0 && c.drivers.is_empty()));
    }

    #[test]
    fn a_share_under_half_a_hundredth_reads_zero_never_negative_zero() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let mut lower = EXTENDED;
        lower[0].1 = 0.980824; // nesting falls by a millionth: −0.0008 points
        let a = attribute(Some(&before), Some(&terms(&ORIGINAL, &lower)), &Drivers::default());
        let nesting = a.contributions.iter().find(|c| c.dimension == "nesting").unwrap();
        assert_eq!(nesting.points.to_bits(), 0.0_f64.to_bits(), "{nesting:?}");
        let json = serde_json::to_string(&a).unwrap();
        assert!(!json.contains("-0.0"), "{json}");
    }

    #[test]
    fn an_na_or_collapsed_signal_is_stated_not_attributed() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let a = attribute(None, Some(&before), &Drivers::default());
        assert_eq!((a.before, a.after, a.change, a.residual), (None, Some(8291), None, None));
        assert!(a.not_attributable_because.as_deref().unwrap().contains("production graph is empty"));

        let mut zero = ORIGINAL;
        zero[1].1 = 0.0;
        let collapsed = terms(&zero, &EXTENDED);
        let a = attribute(Some(&before), Some(&collapsed), &Drivers::default());
        assert_eq!((a.after, a.change, a.residual), (Some(0), Some(-8291), None));
        assert!(a.contributions.is_empty());
        assert!(a.not_attributable_because.as_deref().unwrap().contains("short-circuit"));
    }

    #[test]
    fn drivers_follow_the_direction_and_are_capped_largest_first() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let mut lower = EXTENDED;
        lower[0].1 = 0.97; // nesting falls
        let after = terms(&ORIGINAL, &lower);
        let mut offenders: Vec<ReviewedOffender> = (0..7)
            .map(|i| offender("nesting", &format!("f{i}"), ReviewStatus::New, None, Some(5 + i)))
            .collect();
        offenders.push(offender("nesting", "worse", ReviewStatus::Worsened, Some(5), Some(20)));
        offenders.push(offender("nesting", "fixed", ReviewStatus::Resolved, Some(9), None));
        offenders.push(offender("equality", "elsewhere", ReviewStatus::New, None, Some(99)));
        offenders.push(offender("equality", "gone", ReviewStatus::Resolved, Some(9), None));
        let a = attribute(
            Some(&before),
            Some(&after),
            &Drivers {
                offenders: &offenders,
                ..Drivers::default()
            },
        );
        let nesting = &a.contributions[0];
        assert_eq!(nesting.dimension, "nesting");
        assert!(nesting.points < 0.0);
        assert_eq!(nesting.driver_count, 8, "seven new and one worsened push it down; the resolved one does not");
        let names: Vec<&str> = nesting.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["worse", "f6", "f5", "f4", "f3"], "largest change first, capped at five");
        let equality = a.contributions.iter().find(|c| c.dimension == "equality").unwrap();
        assert_eq!((equality.points, equality.driver_count), (0.0, 0), "an unmoved dimension names no driver");
    }

    /// Cohesion leaves the mean while Nesting improves: the re-weighting makes
    /// Nesting's share negative, and its drivers are still the entry that
    /// improved it, never the one pushing the other way.
    #[test]
    fn drivers_follow_the_value_not_a_share_the_reweighting_flipped() {
        let mut before_ext = EXTENDED;
        before_ext[0].1 = 0.98;
        before_ext[2].1 = 0.8;
        let before = terms(&ORIGINAL, &before_ext);
        let after = terms(
            &ORIGINAL,
            &[("nesting", 0.981), ("conciseness", 0.998439), ("focus", 1.0), ("uniqueness", 0.964548)],
        );
        let offenders = [
            offender("nesting", "fixed_deep", ReviewStatus::Resolved, Some(6), None),
            offender("nesting", "added_deep", ReviewStatus::New, None, Some(5)),
        ];
        let a = attribute(
            Some(&before),
            Some(&after),
            &Drivers {
                offenders: &offenders,
                ..Drivers::default()
            },
        );
        let nesting = a.contributions.iter().find(|c| c.dimension == "nesting").unwrap();
        assert!(nesting.points < 0.0 && nesting.after > nesting.before, "the share flipped: {nesting:?}");
        let names: Vec<&str> = nesting.drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["fixed_deep"], "{nesting:?}");
        let equality = a.contributions.iter().find(|c| c.dimension == "equality").unwrap();
        assert!(equality.points != 0.0 && equality.drivers.is_empty(), "an unmoved value names none: {equality:?}");
    }

    /// A rise is driven by the entries that improved most, and equal changes
    /// rank by file; a worsened entry never drives a rise.
    #[test]
    fn a_rise_ranks_its_improvements_by_how_far_they_moved_then_by_file() {
        let before = terms(&ORIGINAL, &EXTENDED);
        let mut higher = EXTENDED;
        higher[0].1 = 0.99;
        let offenders = [
            offender("nesting", "small_step", ReviewStatus::Improved, Some(20), Some(15)),
            offender("nesting", "big_step", ReviewStatus::Improved, Some(20), Some(5)),
            offender("nesting", "worse", ReviewStatus::Worsened, Some(5), Some(30)),
            offender("nesting", "b_tie", ReviewStatus::Resolved, Some(7), None),
            offender("nesting", "a_tie", ReviewStatus::Resolved, Some(7), None),
        ];
        let a = attribute(
            Some(&before),
            Some(&terms(&ORIGINAL, &higher)),
            &Drivers {
                offenders: &offenders,
                ..Drivers::default()
            },
        );
        let names: Vec<&str> = a.contributions[0].drivers.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["big_step", "a_tie", "b_tie", "small_step"], "{:?}", a.contributions[0]);
    }

    #[test]
    fn the_larger_clone_group_that_went_is_named_first() {
        let before = sprint_106_merged();
        let mut higher = before.extended.clone();
        higher[4].1 = 0.97;
        let after = SignalTerms {
            original: before.original.clone(),
            extended: higher,
        };
        let member = |name: &str| DriverMember {
            node_id: None,
            name: name.to_string(),
            file: "src/a.rs".to_string(),
            line: Some(1),
        };
        let a = attribute(
            Some(&before),
            Some(&after),
            &Drivers {
                removed_groups: vec![vec![member("pair_a"), member("pair_b")], vec![member("trio_a"), member("trio_b"), member("trio_c")]],
                ..Drivers::default()
            },
        );
        let sizes: Vec<Option<i64>> = a.contributions[0].drivers.iter().map(|d| d.before).collect();
        assert_eq!(sizes, [Some(3), Some(2)]);
    }

    #[test]
    fn uniqueness_names_the_clone_groups_that_went() {
        let before = sprint_106_merged();
        let mut higher = before.extended.clone();
        higher[4].1 = 0.965411;
        let after = SignalTerms {
            original: before.original.clone(),
            extended: higher,
        };
        let member = |name: &str| DriverMember {
            node_id: None,
            name: name.to_string(),
            file: "web/ui/src/api/client.ts".to_string(),
            line: Some(1),
        };
        let removed = vec![vec![member("cancelInstall"), member("cancelScan")]];
        let a = attribute(
            Some(&before),
            Some(&after),
            &Drivers {
                removed_groups: removed,
                introduced_groups: vec![vec![member("unrelated"), member("other")]],
                ..Drivers::default()
            },
        );
        let uniqueness = &a.contributions[0];
        assert_eq!(uniqueness.dimension, "uniqueness");
        assert!(uniqueness.points > 0.0);
        assert_eq!(uniqueness.driver_count, 1, "only the groups that went drive a rise");
        let group = &uniqueness.drivers[0];
        assert_eq!((group.status, group.before, group.after), (ReviewStatus::Resolved, Some(2), None));
        let members: Vec<&str> = group.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(members, ["cancelInstall", "cancelScan"]);
    }

    fn identified(id: &str, name: &str, file: &str, line: i64) -> DriverMember {
        DriverMember {
            node_id: Some(id.to_string()),
            name: name.to_string(),
            file: file.to_string(),
            line: Some(line),
        }
    }

    #[test]
    fn a_group_driver_is_the_group_named_by_its_first_member_in_identity_order() {
        // By file and line `zulu` is first; by identity `alpha` is.
        let members = [identified("a/`z.rs`/zulu().", "zulu", "a/z.rs", 1), identified("a/`z.rs`/alpha().", "alpha", "a/z.rs", 9)];
        let (_, driver) = clone_group(&members, ReviewStatus::New).expect("a group");
        assert_eq!(
            driver.node_id.as_deref(),
            Some(group_identity(FixUnitKind::CloneGroup, &["a/`z.rs`/alpha().", "a/`z.rs`/zulu()."]).as_str())
        );
        assert!(driver.node_id.as_deref().unwrap().starts_with("clone_group:"));
        assert_eq!((driver.name.as_str(), driver.line), ("alpha", Some(9)), "named by the identity-first member");
        let listed: Vec<&str> = driver.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(listed, ["zulu", "alpha"], "every member, as given");

        // Reordered members: the same group, the same name; a member fewer is another.
        let reordered = [members[1].clone(), members[0].clone()];
        let (_, again) = clone_group(&reordered, ReviewStatus::New).expect("a group");
        assert_eq!((again.node_id, again.name), (driver.node_id.clone(), driver.name.clone()));
        let (_, fewer) = clone_group(&members[..1], ReviewStatus::New).expect("a group");
        assert_ne!(fewer.node_id, driver.node_id);
    }

    #[test]
    fn a_group_with_a_member_that_has_no_identity_carries_no_id() {
        let members = [
            identified("a/`z.rs`/zulu().", "zulu", "a/z.rs", 1),
            DriverMember { node_id: None, name: "anon".to_string(), file: "a/z.rs".to_string(), line: Some(5) },
        ];
        let (_, driver) = clone_group(&members, ReviewStatus::Resolved).expect("a group");
        assert_eq!((driver.node_id, driver.name.as_str()), (None, "zulu"), "an id is never fabricated");
    }
}
