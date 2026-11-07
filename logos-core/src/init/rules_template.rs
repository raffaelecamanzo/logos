//! The `rules.toml` templates `init` writes, **rendered from the curated set**
//! (S-508, [FR-IN-10], [CR-165]).
//!
//! Two templates share one rendering of `[constraints]` and `[metric_thresholds]`:
//!
//! - [`member`] — what `logos init` writes to `<project>/.logos/rules.toml`;
//! - [`workspace`] — what `logos init --workspace` writes to
//!   `<workspace-root>/.logos/rules.toml`, the root contract every member inherits
//!   key by key ([FR-CF-08], [ADR-73]).
//!
//! Every value is serialised from [`Constraints::recommended`] and
//! [`MetricThresholds::documented_defaults`] — the single curated source the
//! Config editor already shows ([CR-067]) — so a template can no longer drift from
//! it (it had: `max_fn_lines = 100` / `no_god_files = 50` against the curated 80 /
//! 40, and no [CR-005] budget at all). Only the per-key notes are prose, and a test
//! requires one for every key the curated set serialises.
//!
//! Every key is commented out, so a freshly written template parses to
//! [`Rules::default`](crate::config::Rules::default) and enforces nothing until a
//! human uncomments a key ([BR-62]). Uncommented, the key lines parse into exactly
//! the curated set.
//!
//! [FR-IN-10]: ../../../docs/specs/requirements/FR-IN-10.md
//! [FR-CF-08]: ../../../docs/specs/requirements/FR-CF-08.md
//! [ADR-73]: ../../../docs/specs/architecture/decisions/ADR-73.md
//! [CR-165]: ../../../docs/requests/CR-165-workspace-rules-inherit-and-ratchet.md
//! [CR-067]: ../../../docs/requests/CR-067-config-default-surfacing.md
//! [CR-005]: ../../../docs/requests/CR-005-extended-structural-metrics.md
//! [BR-62]: ../../../docs/specs/software-spec.md#318-configuration

use crate::config::{Budget, Constraints, MetricThresholds};

/// What one `[constraints]` key enforces — the comment beside its line.
fn constraint_note(key: &str) -> &'static str {
    match key {
        "max_cycles" => "maximum allowed dependency cycles",
        "max_cc" => "maximum cyclomatic complexity per function",
        "max_fn_lines" => "maximum lines per function",
        "no_god_files" => "maximum symbols per file",
        "max_fan_in" => "max inbound dependency edges for any one symbol (FR-GV-11)",
        "max_fan_out" => "max outbound dependency edges for any one symbol (FR-GV-11)",
        "max_dead" => "max project-wide dead functions (FR-GV-11)",
        "max_duplicates" => "max project-wide duplicate functions (FR-GV-11)",
        "max_nesting_depth" => "max nesting depth per function (CR-005)",
        "max_brain_methods" => "max project-wide brain methods (CR-005)",
        "max_clone_ratio" => "max near-clone function ratio, 0.0-1.0 (CR-005)",
        "no_god_containers" => "no class-like container over the god thresholds (CR-005)",
        _ => "",
    }
}

/// What one `[metric_thresholds]` key tunes — the comment beside its line.
fn threshold_note(key: &str) -> &'static str {
    match key {
        "nesting_depth" => "depth at which a function counts as deeply nested",
        "brain_complexity" => "complexity floor of a brain method",
        "brain_lines" => "line-count floor of a brain method",
        "brain_nesting" => "nesting floor of a brain method",
        "god_methods" => "method count at which a container is a god container",
        "god_span" => "line span at which a container is a god container",
        "clone_similarity" => "near-clone similarity threshold, (0, 1]",
        "clone_min_tokens" => "token floor below which a function is never a near clone",
        "duplicate_min_tokens" => "token floor a body needs to be an exact duplicate",
        "cohesion_min_classes" => "fewest scoreable classes Cohesion needs to apply",
        _ => "",
    }
}

/// `table`'s keys as `toml` serialises them, in declaration order, each
/// `(key, value)` — the value spelled exactly as a `rules.toml` would.
fn serialised_keys<T: serde::Serialize>(table: &T) -> Vec<(String, String)> {
    toml::to_string(table)
        .expect("a curated rules table always serialises")
        .lines()
        .filter_map(|line| line.split_once(" = "))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `# [table]` and one `# key = value  # note` line per key.
fn commented_table(out: &mut String, table: &str, keys: &[(String, String)], note: fn(&str) -> &'static str) {
    out.push_str(&format!("# [{table}]\n"));
    for (key, value) in keys {
        out.push_str(&format!("# {key} = {value}  # {}\n", note(key)));
    }
}

/// The ratchet spelling of the curated `max_cc`, for the adoption comment.
fn ratchet_example(curated: &Constraints) -> String {
    let max = curated.max_cc.as_ref().map_or(15, Budget::max);
    format!("{{ max = {max}, ratchet = true }}")
}

/// The `[constraints]` and `[metric_thresholds]` block both templates share,
/// with the ratchet adoption note between them. `blessed_set` is the comment
/// lines saying where `logos check --bless` records the blessed set for this
/// template's reader.
fn curated_block(blessed_set: &str) -> String {
    let curated = Constraints::recommended();
    let mut out = String::new();
    out.push_str(
        "# The values shown are the curated recommended baselines and the threshold\n\
         # defaults. Uncomment a key to enforce it; an omitted constraint is not enforced.\n",
    );
    commented_table(&mut out, "constraints", &serialised_keys(&curated), constraint_note);
    out.push_str(&format!(
        "#\n\
         # Ratchet adoption (FR-GV-23): a budget that names symbols may be declared\n\
         # as a table instead of a number, to tolerate today's offenders and fail only\n\
         # on new or worsened ones:\n\
         #   max_cc = {}\n\
         {blessed_set}\
         # A blessed offender at or below its blessed value is a warning; a new or worse\n\
         # one is an error. `max_dead` and `max_cycles` also take the count form\n\
         # `{{ baseline = N, delta = 0 }}`.\n\n",
        ratchet_example(&curated)
    ));
    commented_table(
        &mut out,
        "metric_thresholds",
        &serialised_keys(&MetricThresholds::documented_defaults()),
        threshold_note,
    );
    out
}

/// The member-only example tables of the member template: unchanged by S-508.
const MEMBER_PATH_TABLES: &str = r#"
# Ordered layers: a higher `order` may not depend on a lower one. Files not
# matching any layer are exempt from layer ordering (DL-05).
# [[layers]]
# name  = "domain"
# paths = ["src/domain/**"]
# order = 0
#
# [[layers]]
# name  = "infrastructure"
# paths = ["src/infra/**"]
# order = 1

# Forbidden dependencies between named layers, reported with the reason.
# [[boundaries]]
# from   = "domain"
# to     = "infrastructure"
# reason = "the domain stays persistence-free"

# Glob-level import bans (FR-GV-12): any import/reference edge from a `from`-glob
# file into a `to`-glob file is a violation. Globs match file paths, not layer
# names. v1 covers resolved intra-workspace edges.
# [[forbidden_imports]]
# from   = "src/web/**"
# to     = "src/db/**"
# reason = "the web layer must not import the db directly"

# Git-history analytics tuning (CR-006, FR-GH-03..05). These keys tune the
# NON-GATED temporal tier only (`logos hotspots`) — they never affect `check`
# or the quality gate (BR-26). All optional; shown with their defaults.
# [history]
# window_months              = 12   # HEAD-anchored window length, in calendar months
# co_change_max_commit_files = 50   # commits touching more files are skipped for co-change pairing only
# defect_patterns            = ["(?i)\\bfix(es|ed)?\\b", "(?i)\\bbug\\b", "(?i)\\bhotfix\\b"]

# Coverage-ingestion tuning (CR-007, FR-CV-03/09). Tunes the NON-GATED advisory
# evidence tier only (`logos coverage`) — it never affects `check` or the quality
# gate (BR-28). `path_strip_prefixes` are stripped from coverage-report paths
# before they map to indexed files, so absolute build-dir paths bind.
# [coverage]
# path_strip_prefixes = ["/home/runner/work/myproject/", "/build/"]
"#;

/// The starter `<project>/.logos/rules.toml` `logos init` writes ([FR-IN-01],
/// [FR-IN-10]): every key commented, so it parses to the empty contract.
///
/// [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
/// [FR-IN-10]: ../../../docs/specs/requirements/FR-IN-10.md
pub(crate) fn member() -> String {
    let mut out = String::from(
        "# Logos architecture contract (.logos/rules.toml) — checked-in policy\n\
         # enforced by `logos check` and the quality gate (FR-GV-01..05). Everything\n\
         # is optional: an omitted constraint is simply not enforced.\n\
         # Reference: docs/howto/configuration.md\n\n",
    );
    out.push_str(&curated_block(
        "# `logos check --bless` records today's offenders in .logos/rules-baseline.json.\n",
    ));
    out.push_str(MEMBER_PATH_TABLES);
    out
}

/// The workspace rules contract `logos init --workspace` writes to
/// `<workspace-root>/.logos/rules.toml` ([FR-IN-10]): every key commented, with
/// comments explaining key-by-key inheritance ([FR-CF-08]) and ratchet adoption
/// ([FR-GV-23]). It carries no member-only table, since the root refuses them.
///
/// [FR-IN-10]: ../../../docs/specs/requirements/FR-IN-10.md
/// [FR-CF-08]: ../../../docs/specs/requirements/FR-CF-08.md
/// [FR-GV-23]: ../../../docs/specs/requirements/FR-GV-23.md
pub(crate) fn workspace() -> String {
    let mut out = String::from(
        "# Logos workspace rules contract (<workspace-root>/.logos/rules.toml), written\n\
         # by `logos init --workspace` (FR-IN-10). Nothing in it is enforced until a\n\
         # key is uncommented (BR-62).\n\
         #\n\
         # Inheritance (FR-CF-08): every member of this workspace inherits each\n\
         # [constraints] and [metric_thresholds] key declared here, key by key. A\n\
         # member that declares the same key in its own .logos/rules.toml overrides\n\
         # it and still inherits every other key. `logos check --json` names each\n\
         # key's origin, member or workspace, and the member Config editor shows it.\n\
         #\n\
         # Only those two tables inherit. [[layers]], [[boundaries]],\n\
         # [[forbidden_imports]], [[require_tested]] and [[require_documented]] name\n\
         # member paths; [history] and [coverage] tune one member's own evidence;\n\
         # [security] and [review] are one member's own gate policy. Declared here,\n\
         # they are refused with that reason and applied to no member: declare them\n\
         # in the member's own rules.toml.\n\
         #\n\
         # Reference: docs/howto/configuration.md\n\n",
    );
    out.push_str(&curated_block(
        "# `logos check --bless`, run in a member, records that member's offenders in its\n\
         # own .logos/rules-baseline.json, so one root rule fits every member.\n",
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{parse_rules, Rules};

    /// Uncomment every `# key = value` and `# [table]` line, and nothing else.
    fn uncomment_keys(template: &str) -> String {
        template
            .lines()
            .map(|line| match line.strip_prefix("# ") {
                Some(rest)
                    if (rest.starts_with('[') && rest.ends_with(']') && !rest.contains(' '))
                        || rest.split_once('=').is_some_and(|(key, _)| {
                            let key = key.trim_end();
                            !key.is_empty() && key.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                        }) =>
                {
                    rest.to_string()
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn parse(text: &str) -> Rules {
        parse_rules(text, std::path::Path::new("rules.toml"))
            .unwrap_or_else(|e| panic!("the template parses: {e}\n{text}"))
    }

    /// FR-IN-10 AC: every curated field appears in **both** templates with its
    /// curated value, each with a note — read off the serialised curated set, so a
    /// key added to `Constraints` or `MetricThresholds` is required here too.
    #[test]
    fn every_curated_key_appears_in_both_templates_with_its_value_and_a_note() {
        let constraints = serialised_keys(&Constraints::recommended());
        let thresholds = serialised_keys(&MetricThresholds::documented_defaults());
        assert_eq!(constraints.len(), 12, "every constraint is curated: {constraints:?}");
        assert_eq!(thresholds.len(), 10, "every threshold has a default: {thresholds:?}");
        for template in [member(), workspace()] {
            for (key, value) in &constraints {
                assert!(!constraint_note(key).is_empty(), "`{key}` has a note");
                let line = format!("# {key} = {value}  # {}", constraint_note(key));
                assert!(template.lines().any(|l| l == line), "missing `{line}`:\n{template}");
            }
            for (key, value) in &thresholds {
                assert!(!threshold_note(key).is_empty(), "`{key}` has a note");
                let line = format!("# {key} = {value}  # {}", threshold_note(key));
                assert!(template.lines().any(|l| l == line), "missing `{line}`:\n{template}");
            }
        }
        // The values the story names, spelled out rather than only derived.
        for pinned in [
            "# max_fn_lines = 80 ",
            "# no_god_files = 40 ",
            "# max_nesting_depth = 4 ",
            "# max_brain_methods = 0 ",
            "# max_clone_ratio = 0.0 ",
            "# no_god_containers = true ",
        ] {
            assert!(member().contains(pinned) && workspace().contains(pinned), "{pinned}");
        }
    }

    /// FR-IN-10 AC: as written, both templates enforce nothing.
    #[test]
    fn both_templates_as_written_are_the_empty_contract() {
        assert_eq!(parse(&member()), Rules::default());
        assert_eq!(parse(&workspace()), Rules::default());
    }

    /// FR-IN-10 AC: uncommented, both templates parse into a contract equal to
    /// the curated set — and the root's declares no table the root refuses.
    #[test]
    fn both_templates_uncommented_are_the_curated_set() {
        for template in [member(), workspace()] {
            let rules = parse(&uncomment_keys(&template));
            assert_eq!(rules.constraints, Constraints::recommended());
            assert_eq!(rules.metric_thresholds, MetricThresholds::documented_defaults());
        }
        let root = parse(&uncomment_keys(&workspace()));
        assert!(
            Rules::default().inherit(Some(root)).refused.is_empty(),
            "the root template declares no member-only table"
        );
    }

    /// The root template explains inheritance and the ratchet, and names every
    /// member-only table a root declaration of which is refused.
    #[test]
    fn the_workspace_template_explains_inheritance_ratchet_and_refusal() {
        let text = workspace();
        assert!(text.contains("inherits each\n# [constraints] and [metric_thresholds] key"), "{text}");
        assert!(text.contains("max_cc = { max = 15, ratchet = true }"), "{text}");
        for table in [
            "[[layers]]",
            "[[boundaries]]",
            "[[forbidden_imports]]",
            "[[require_tested]]",
            "[[require_documented]]",
            "[history]",
            "[coverage]",
            "[security]",
            "[review]",
        ] {
            assert!(text.contains(table), "{table} is named as member-only");
        }
    }
}
