//! A rowid-independent fingerprint of a whole graph, shared by the test
//! binaries that compare two stores for equality: `indexing.rs` (sync ≡
//! reindex, CR-015) and `runtime_concurrency.rs` (queries compiled on first
//! use ≡ compiled up front, CR-197). Include it with
//! `#[path = "support/graph_fingerprint.rs"] mod graph_fingerprint;`.

use logos_core::Runtime;

/// A rowid-independent fingerprint of the whole graph (nodes, edges, the
/// whole reference ledger, annotation verdicts), each section a sorted multiset
/// of lines. `clone_group` is deliberately excluded: its representative is the
/// component's minimum rowid, which is insertion-order-sensitive and so differs
/// between two independently built stores even for identical clusters — and
/// near-clone clustering is orthogonal to (and unchanged by) CR-015.
pub fn graph_fingerprint(rt: &Runtime) -> String {
    rt.submit_read(|store| {
        let nodes = store.all_nodes()?;
        let edges = store.all_edges()?;
        let refs = store.unresolved_refs()?;
        let anns = store.annotation_nodes()?;
        let file_of: std::collections::BTreeMap<i64, String> = store
            .indexed_files()?
            .into_iter()
            .map(|f| (f.id, f.path))
            .collect();

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

        // Every ledger row, every column but its rowid (the file by path) —
        // the peeled wrappers of a proven Rust receiver (S-587) included — so
        // a synced ledger must equal a fresh index's row for row (FR-SY-10 as
        // amended by CR-187). Capture-before-delete rows (`RefForm::Symbol`,
        // ADR-10) are compared too: one that outlives its sync is a row a fresh
        // index never holds, and shows here as an extra line.
        let mut ref_lines: Vec<String> = refs
            .iter()
            .map(|r| {
                format!(
                    "R {}|{}|{}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{:?}|{:?}",
                    r.file_id.and_then(|id| file_of.get(&id)).map_or("", String::as_str),
                    r.source_symbol,
                    r.target,
                    r.alias,
                    r.form,
                    r.kind,
                    r.resolved,
                    r.payload,
                    r.receiver,
                    r.peeled,
                    r.line,
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
