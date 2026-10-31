//! Cohesion links inherited members and needs enough classes to score (S-816,
//! [CR-252], [FR-QM-11], metric-semantics v9), driven end-to-end through the
//! public [`Engine`] façade against real TSX fixtures, so the unbound
//! `this.state` accesses LCOM4 links are the ones extraction and binding
//! record, not hand-set rows.
//!
//! - Each fixture component reads `this.state` from `render` and
//!   `componentDidUpdate` and declares no `state` member (React's is
//!   inherited), so those two methods are one LCOM4 component; `label` reads
//!   nothing and is another. Every class is LCOM4 2, where v8 read 3.
//! - Two such classes are fewer than the default `cohesion_min_classes` (5):
//!   Cohesion reads n/a ("too few classes") and no class is ranked for it in
//!   "where to start", while the LCOM4 offender list still lists both.
//! - Five are scored.
//!
//! [CR-252]: ../../docs/requests/CR-252-findings-and-offenders-without-the-noise.md
//! [FR-QM-11]: ../../docs/specs/requirements/FR-QM-11.md
#![cfg(feature = "lang-typescript")]

use std::fs;
use std::path::Path;

use logos_core::metrics::{self, RecordedSnapshot};
use logos_core::{Engine, Granularity};
use tempfile::TempDir;

/// Write `contents` at `root/rel`, creating parents.
fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// A React class component named `name` whose `render` and
/// `componentDidUpdate` both read the inherited `this.state`.
fn component(name: &str) -> String {
    format!(
        r#"import React from "react";

type State = {{ count: number }};

export class {name} extends React.Component<{{}}, State> {{
  render() {{
    return <div>{{this.state.count}}</div>;
  }}

  componentDidUpdate() {{
    if (this.state.count > 3) {{
      console.log("{name}");
    }}
  }}

  label() {{
    return "{name}";
  }}
}}
"#
    )
}

/// A project of `n` component classes, one per `.tsx` file, indexed and
/// scanned once under the default thresholds.
fn scan_components(n: usize) -> (TempDir, RecordedSnapshot) {
    let tmp = TempDir::new().unwrap();
    for i in 0..n {
        write(tmp.path(), &format!("src/Board{i}.tsx"), &component(&format!("Board{i}")));
    }
    let engine = Engine::start(tmp.path()).expect("engine starts");
    engine.index();
    let view = engine
        .hydrate(Granularity::ExcludeContains)
        .expect("dependency view hydrates");
    let rt = engine.runtime().expect("runtime");
    let member_access = engine
        .registry()
        .expect("a started engine holds its registry")
        .member_access_extensions();
    let snapshot = metrics::snapshot(rt, &view, None, metrics::Thresholds::default(), &member_access).expect("snapshot runs");
    (tmp, snapshot)
}

/// The Cohesion offender details, by class name.
fn lcom4_offenders(s: &RecordedSnapshot) -> Vec<(&str, &str)> {
    s.worst_offenders
        .cohesion
        .iter()
        .map(|o| (o.name.as_str(), o.detail.as_str()))
        .collect()
}

/// Two classes: Cohesion is n/a with the reason "too few classes", leaves the
/// aggregate and "where to start", and the LCOM4 offender list still lists
/// both classes — at LCOM4 2, the inherited `this.state` linking `render` and
/// `componentDidUpdate`.
#[test]
fn two_tsx_classes_read_cohesion_not_applicable_too_few_classes() {
    let (tmp, s) = scan_components(2);
    assert!(s.metrics.cohesion.is_none(), "2 of 5 classes: Cohesion is not applicable");
    let na = s.metrics.cohesion_not_applicable.as_ref().expect("the drop-out names its cause");
    assert_eq!((na.classes, na.min_classes), (2, 5));
    assert!(na.reason.contains("too few classes"), "{}", na.reason);
    // What `scan --json` serializes under `metrics`.
    let metrics_json = serde_json::to_value(&s.metrics).unwrap();
    assert_eq!(
        metrics_json["cohesion_not_applicable"],
        serde_json::json!({
            "classes": 2,
            "min_classes": 5,
            "reason": "2 of 5 classes — too few classes to score"
        })
    );
    assert!(metrics_json["cohesion"].is_null());
    assert!(s.metrics.focus.is_some(), "Focus still counts the two classes");
    assert_eq!(
        lcom4_offenders(&s),
        [("Board0", "LCOM4 2"), ("Board1", "LCOM4 2")],
        "the offender list is not gated by the minimum"
    );
    assert!(
        s.where_to_start.units.iter().all(|u| !u.dimensions.contains(&"cohesion")),
        "no class is ranked for Cohesion: {:?}",
        s.where_to_start.units
    );

    // The `quality-report` / session readout computes fresh and carries the
    // same reason beside the graph dimensions' ones.
    let engine = Engine::start(tmp.path()).expect("engine starts");
    let readout = engine.quality_readout().expect("readout");
    assert_eq!(readout.cohesion_not_applicable.as_ref(), Some(na));
    let json = serde_json::to_value(&readout).unwrap();
    assert_eq!(json["cohesion_not_applicable"]["reason"], "2 of 5 classes — too few classes to score");
}

/// Five classes are scored: Cohesion applies, each class at LCOM4 2 — the
/// two `this.state` readers one component, `label` the other — so the mean of
/// `1/LCOM4` is 0.5.
#[test]
fn five_tsx_classes_are_scored_with_inherited_state_linking_their_readers() {
    let (_tmp, s) = scan_components(5);
    assert!(s.metrics.cohesion_not_applicable.is_none());
    let cohesion = s.metrics.cohesion.expect("5 of 5 classes: Cohesion applies");
    assert_eq!(cohesion.raw, 0.5, "every class is LCOM4 2 (v8 read 3, a mean of 1/3)");
    assert_eq!(lcom4_offenders(&s).len(), 5);
    assert!(lcom4_offenders(&s).iter().all(|&(_, detail)| detail == "LCOM4 2"));
}
