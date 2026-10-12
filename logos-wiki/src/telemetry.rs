//! The wiki's telemetry tools ([FR-OB-09], [FR-OB-11], [FR-DS-04]).
//!
//! The fourteen names the wiki's operations report under — the wire names and
//! classes they had as the core's `Tool::Wiki*` variants until the wiki became the
//! `wiki` extension (S-644), so telemetry history keyed on them reads on
//! unchanged. Each operation in [`crate::WikiEngine`] emits through the core's
//! extension door ([`traced_extension`](logos_core::observability::traced_extension))
//! under one of these; [`crate::WikiExtension`] declares them, so the stats
//! read-model classifies them while the extension is active. Every one is counted
//! as usage (an extension tool always is), as every wiki tool was.
//!
//! The quality-report hook's `wiki_quality_report_hook_emit` is not here: that
//! command is the core's (`logos hook --emit`, S-638), and so is its name.
//!
//! [FR-OB-09]: ../../docs/specs/requirements/FR-OB-09.md
//! [FR-OB-11]: ../../docs/specs/requirements/FR-OB-11.md
//! [FR-DS-04]: ../../docs/specs/requirements/FR-DS-04.md

use logos_core::observability::{ExtensionTool, ToolClass};

/// `wiki_write` — a page upsert ([FR-WK-02]).
///
/// [FR-WK-02]: ../../docs/specs/requirements/FR-WK-02.md
pub const WIKI_WRITE: ExtensionTool = ExtensionTool::new("wiki_write", ToolClass::EngineInternal);
/// `wiki_read` — a page read. A read over the codebase's documentation that
/// substitutes for opening source: navigation.
pub const WIKI_READ: ExtensionTool = ExtensionTool::new("wiki_read", ToolClass::Navigation);
/// `wiki_delete` — an explicit page delete.
pub const WIKI_DELETE: ExtensionTool = ExtensionTool::new("wiki_delete", ToolClass::EngineInternal);
/// `wiki_pruned_log` — the auto-pruned page log.
pub const WIKI_PRUNED_LOG: ExtensionTool =
    ExtensionTool::new("wiki_pruned_log", ToolClass::EngineInternal);
/// `wiki_search` — the full-text search: navigation, like `wiki_read`.
pub const WIKI_SEARCH: ExtensionTool = ExtensionTool::new("wiki_search", ToolClass::Navigation);
/// `wiki_status` — the store's own freshness/anchor state: a read-model.
pub const WIKI_STATUS: ExtensionTool = ExtensionTool::new("wiki_status", ToolClass::ReadModel);
/// `wiki_generate` — the generation queue.
pub const WIKI_GENERATE: ExtensionTool =
    ExtensionTool::new("wiki_generate", ToolClass::EngineInternal);
/// `wiki_native` — the native (extracted) tier's render.
pub const WIKI_NATIVE: ExtensionTool = ExtensionTool::new("wiki_native", ToolClass::EngineInternal);
/// `wiki_doc_category_present` — a presented-tier source probe.
pub const WIKI_DOC_CATEGORY_PRESENT: ExtensionTool =
    ExtensionTool::new("wiki_doc_category_present", ToolClass::EngineInternal);
/// `wiki_srs_mode` — the SRS-mode probe.
pub const WIKI_SRS_MODE: ExtensionTool =
    ExtensionTool::new("wiki_srs_mode", ToolClass::EngineInternal);
/// `wiki_guide_pages` — the User Guide page set.
pub const WIKI_GUIDE_PAGES: ExtensionTool =
    ExtensionTool::new("wiki_guide_pages", ToolClass::EngineInternal);
/// `wiki_reconcile` — the reconciliation sweep.
pub const WIKI_RECONCILE: ExtensionTool =
    ExtensionTool::new("wiki_reconcile", ToolClass::EngineInternal);
/// `wiki_materialize` — the presented tier's deterministic assembly.
pub const WIKI_MATERIALIZE: ExtensionTool =
    ExtensionTool::new("wiki_materialize", ToolClass::EngineInternal);
/// `wiki_skill_emit` — the embedded skill's materialization.
pub const WIKI_SKILL_EMIT: ExtensionTool =
    ExtensionTool::new("wiki_skill_emit", ToolClass::EngineInternal);

/// Every telemetry tool the wiki reports under, in the order the core
/// registered them.
pub const TELEMETRY_TOOLS: &[ExtensionTool] = &[
    WIKI_WRITE,
    WIKI_READ,
    WIKI_DELETE,
    WIKI_PRUNED_LOG,
    WIKI_SEARCH,
    WIKI_STATUS,
    WIKI_GENERATE,
    WIKI_NATIVE,
    WIKI_DOC_CATEGORY_PRESENT,
    WIKI_SRS_MODE,
    WIKI_GUIDE_PAGES,
    WIKI_RECONCILE,
    WIKI_MATERIALIZE,
    WIKI_SKILL_EMIT,
];

#[cfg(test)]
mod tests {
    use super::TELEMETRY_TOOLS;

    /// Every chokepoint reports under its own name: each `fn wiki_<op>` in
    /// [`crate::WikiEngine`]'s impl traces `WIKI_<OP>`, each `WIKI_<OP>` is the wire
    /// name `wiki_<op>`, and the fourteen traced are the fourteen declared — so
    /// one call site swapped to a sibling's constant cannot keep a declared-list
    /// test green while it reports under the wrong name.
    #[test]
    fn each_wiki_operation_reports_under_its_own_name() {
        let engine = include_str!("engine.rs");
        let mut traced: Vec<String> = Vec::new();
        for (index, _) in engine.match_indices("\n    fn wiki_") {
            let signature = &engine[index + "\n    fn ".len()..];
            let op = &signature[..signature.find('(').expect("a signature")];
            // The trait's declarations (`…;`) precede the impl's bodies (`… {`).
            if signature.find(';') < signature.find('{') {
                continue;
            }
            let body = &signature[..signature.find("\n    }\n").expect("the method ends")];
            let constant = body
                .split("traced_extension")
                .nth(1)
                .and_then(|call| call.split("telemetry::").nth(1))
                .and_then(|rest| rest.split(',').next())
                .unwrap_or_else(|| panic!("`{op}` traces no telemetry constant"));
            assert_eq!(constant, op.to_uppercase(), "`{op}` reports under `{constant}`");
            traced.push(op.to_string());
        }

        let declarations = include_str!("telemetry.rs");
        for op in &traced {
            let declaration = format!("pub const {}: ExtensionTool =", op.to_uppercase());
            let tail = &declarations[declarations.find(&declaration).expect("declared")..];
            let wire = tail.split('"').nth(1).expect("a wire name");
            assert_eq!(wire, op, "`{}` is the wire name `{op}`", op.to_uppercase());
        }

        let mut declared: Vec<&str> = TELEMETRY_TOOLS.iter().map(|tool| tool.name()).collect();
        traced.sort();
        declared.sort_unstable();
        assert_eq!(traced, declared);
    }
}
