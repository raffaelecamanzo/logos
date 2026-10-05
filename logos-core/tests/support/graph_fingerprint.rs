//! A rowid-independent fingerprint of a whole graph, shared by the test
//! binaries that compare two stores for equality: `indexing.rs` (sync ≡
//! reindex, CR-015) and `runtime_concurrency.rs` (queries compiled on first
//! use ≡ compiled up front, CR-197). Include it with
//! `#[path = "support/graph_fingerprint.rs"] mod graph_fingerprint;`.

use logos_core::model::RefForm;
use logos_core::Runtime;

/// A rowid-independent fingerprint of the whole graph (nodes, edges, the
/// reference ledger, annotation verdicts), each section a sorted multiset of
/// lines. `clone_group` is deliberately excluded: its representative is the
/// component's minimum rowid, which is insertion-order-sensitive and so differs
/// between two independently built stores even for identical clusters — and
/// near-clone clustering is orthogonal to (and unchanged by) CR-015.
pub fn graph_fingerprint(rt: &Runtime) -> String {
    rt.submit_read(|store| {
        let nodes = store.all_nodes()?;
        let edges = store.all_edges()?;
        let refs = store.unresolved_refs()?;
        let anns = store.annotation_nodes()?;

        // rowid -> canonical symbol, so edge endpoints compare by identity, not
        // by store-local rowid.
        let sym_of: std::collections::BTreeMap<i64, String> = nodes
            .iter()
            .map(|n| (n.id.0, n.symbol.as_str().to_string()))
            .collect();
        let key = |id: i64| -> String {
            sym_of
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("<unknown:{id}>"))
        };

        let mut node_lines: Vec<String> = nodes
            .iter()
            .map(|n| {
                format!(
                    "N {}|{:?}|{}|{}|{:?}|{:?}",
                    n.symbol.as_str(),
                    n.kind,
                    n.name,
                    n.file_path.as_deref().unwrap_or(""),
                    n.start_line,
                    n.end_line,
                )
            })
            .collect();
        node_lines.sort();

        let mut edge_lines: Vec<String> = edges
            .iter()
            .map(|e| format!("E {} -> {} [{:?}]", key(e.source.0), key(e.target.0), e.kind))
            .collect();
        edge_lines.sort();

        let mut ref_lines: Vec<String> = refs
            .iter()
            // Exclude capture-before-delete rows (`RefForm::Symbol`, ADR-10): they
            // are a sync-only internal bookkeeping artifact a from-scratch index
            // never produces (capture runs only on re-extraction), so counting them
            // would make sync ≠ reindex for reasons orthogonal to CR-015. The edges
            // they preserve ARE compared above, so a mis-bound capture still fails
            // the edge section — only the redundant ledger row itself is ignored.
            .filter(|r| r.form != RefForm::Symbol)
            .map(|r| {
                format!(
                    "R {}|{}|{:?}|{:?}|{}|{:?}",
                    r.source_symbol, r.target, r.form, r.kind, r.resolved, r.payload,
                )
            })
            .collect();
        ref_lines.sort();

        let mut ann_lines: Vec<String> = anns
            .iter()
            .map(|a| {
                format!(
                    "A {}|dead={:?}|dup={:?}|test={}|layer={:?}|exp={}|der={}|fp={:?}",
                    key(a.id.0),
                    a.is_dead,
                    a.is_duplicate,
                    a.is_test,
                    a.layer_membership,
                    a.exported,
                    a.derived,
                    a.fingerprint,
                )
            })
            .collect();
        ann_lines.sort();

        let mut out = String::new();
        for section in [node_lines, edge_lines, ref_lines, ann_lines] {
            for line in section {
                out.push_str(&line);
                out.push('\n');
            }
            out.push_str("----\n");
        }
        Ok(out)
    })
    .expect("fingerprint read runs")
}
