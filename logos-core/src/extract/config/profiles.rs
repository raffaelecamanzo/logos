//! Content-sniffed **profile promotion** for the config/artifact layer (S-067,
//! [CR-010], [ADR-25], [FR-CG-03], [NFR-RA-06]).
//!
//! The generic substrate ([`super`], S-062) extracts every `.yaml`/`.json` file
//! into a [`NodeKind::ConfigFile`] root and a depth-bounded
//! [`NodeKind::ConfigSection`] tree (S-063). This module layers an **additive**
//! pass on top: it inspects the *already-parsed* tree's top-level keys and, when a
//! document's **content** matches a recognised profile, *promotes* it — tagging
//! its `ConfigFile` and emitting the profile's typed nodes over the same root.
//!
//! # Generic-first, additive, never destructive
//!
//! Promotion is the [`super::super::doc::enrich`] pattern ([FR-DG-07]) applied to
//! the config layer: a document that does not sniff is left exactly as the
//! generic walk produced it ([`promote`] is a no-op), and a document that *does*
//! sniff keeps **all** its generic `ConfigSection` nodes — promotion only *adds*
//! `ApiPath`/`ApiOperation` nodes and tags the `ConfigFile`'s body, never altering
//! or removing a section. So toggling promotion on or off never churns a generic
//! id, and the generic layer stands alone on any non-profile document.
//!
//! # The sniff is content, not a grammar node
//!
//! Unlike the static typed-anchor mechanisms ([`super`]'s `[config] node_kind` /
//! `[[config.anchors]]` walks, S-064/S-065, and the per-format [`super::anchors`]
//! walk, S-066) — each a fixed *tree-sitter node kind* → config kind map — a
//! profile keys on the *semantic* shape of the document: a top-level
//! version-bearing `openapi:`/`swagger:` key. It therefore reads the tree through
//! a small **format-agnostic** mapping view ([`shallowest_pairs`]/[`pair_key`]/
//! [`pair_value`]) that spans both the YAML (`block_mapping_pair`) and JSON
//! (`pair`) grammars, so a JSON-bodied spec promotes **identically** to its YAML
//! twin — the same anchor names, kinds, and `Contains` structure, differing only
//! in the file-path segment of the symbol ([NFR-RA-06]).
//!
//! # Extensible hook ([FR-CG-03])
//!
//! [`promote`] tries each registered profile in order, first match wins. This
//! story ships exactly **one** profile ([`openapi`]); a future profile (AsyncAPI,
//! JSON Schema, …) is added as one more guarded call in [`promote`] with no change
//! to the sniff dispatch or the generic extraction — the requirement claims no
//! profile beyond OpenAPI.
//!
//! [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
//! [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
//! [FR-CG-03]: ../../../../docs/specs/requirements/FR-CG-03.md
//! [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::HashMap;

use tree_sitter::Node;

use crate::model::{LogosSymbol, NodeKind};

use super::super::{Facts, SymbolContext};
use super::{anchor_slug, next_ordinal};

/// The tree-sitter mapping-pair node kinds a profile sniffs across the data
/// grammars: YAML's `block_mapping_pair` and JSON's `pair`. Both expose a `key`
/// and a `value` field, so [`shallowest_pairs`]/[`pair_key`]/[`pair_value`] read
/// either grammar's tree uniformly — the basis of the YAML/JSON-twin determinism
/// ([NFR-RA-06]). (Flow-style inline mappings are deliberately out of scope, as
/// they are for the generic S-063 section walk, which declares only these kinds.)
const PAIR_KINDS: &[&str] = &["block_mapping_pair", "pair"];

/// Per-profile promotion context — a type alias for the shared
/// [`super::EmitCtx`] so `promote` and the OpenAPI sub-walk keep their existing
/// `&PromotionCtx<'_>` parameter type without change. The two structs were
/// structurally identical; the alias collapses them into one (sprint-10
/// coherence fix, S-066/S-067).
type PromotionCtx<'a> = super::EmitCtx<'a>;

/// The content-sniffed **profile-promotion hook** (S-067, [FR-CG-03]): inspect
/// the already-parsed `root` and, for the first recognised content profile,
/// promote the document additively over its existing `ConfigFile`
/// (`config_file_symbol`) — tagging the file and emitting typed nodes — then stop.
///
/// A no-op when no profile sniffs: the generic section/anchor extraction the
/// caller already ran stands untouched. The hook is extensible — a future profile
/// is one more guarded call here, claiming nothing beyond OpenAPI today.
pub(super) fn promote(
    root: Node<'_>,
    segments: &[&str],
    config_file_symbol: &LogosSymbol,
    source: &[u8],
    ctx: &SymbolContext,
    facts: &mut Facts,
) {
    let pctx = PromotionCtx {
        ctx,
        segments,
        source,
    };

    // Registered content profiles, tried in order via `||` short-circuit: the
    // first whose sniff matches promotes the document, and the rest are skipped
    // (first match wins). This story ships exactly one. A future profile chains on
    // as ` || asyncapi::try_promote(&pctx, root, config_file_symbol, facts)` with
    // no change to the generic extraction above ([FR-CG-03] "extensible for future
    // profiles, none of which are claimed").
    let _promoted = openapi::try_promote(&pctx, root, config_file_symbol, facts);
}

/// Emit one promoted typed node and its [`EdgeKind::Contains`] edge from
/// `parent_symbol`, returning `(symbol, chain)` so a nested node can parent off
/// it. A thin wrapper over [`super::emit_anchored_node`] that extracts line
/// numbers from `node` and hides the `body` parameter (promotion nodes carry no
/// payload in `body` — the OpenAPI profile tag lives on the `ConfigFile` node).
#[allow(clippy::too_many_arguments)]
fn emit_node(
    kind: NodeKind,
    name: &str,
    slug: &str,
    ordinal: u32,
    parent_chain: &[String],
    parent_symbol: &LogosSymbol,
    node: Node<'_>,
    pctx: &PromotionCtx<'_>,
    facts: &mut Facts,
) -> Option<(LogosSymbol, Vec<String>)> {
    super::emit_anchored_node(
        kind,
        name,
        slug,
        ordinal,
        parent_chain,
        parent_symbol,
        node.start_position().row as u32 + 1,
        node.end_position().row as u32 + 1,
        None,
        pctx,
        facts,
    )
}

// ── Format-agnostic mapping view over the YAML/JSON parse trees ───────────────

/// Collect the **shallowest** mapping-pair descendants of `node` — the entries at
/// the nearest mapping level — descending through wrapper nodes (`document`,
/// `block_node`, `block_mapping`, `object`, `flow_node`, …) but never *into* a
/// matched pair (its own nested pairs belong to a deeper level). Document (tree)
/// order is preserved, which is the deterministic order ordinals are assigned in.
/// Works uniformly for YAML and JSON, so a spec and its twin yield the same keys.
fn shallowest_pairs<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if PAIR_KINDS.contains(&child.kind()) {
            out.push(child);
        } else {
            shallowest_pairs(child, out);
        }
    }
}

/// A mapping pair's key text — the `key` field, trimmed and unquoted (JSON keys
/// carry their surrounding quotes; YAML keys may be quoted), so a key compares and
/// slugifies identically across the two grammars. `None` if absent/empty/non-UTF-8.
fn pair_key(pair: Node<'_>, source: &[u8]) -> Option<String> {
    let raw = field_text(pair, "key", source)?;
    let key = unquote(&raw);
    (!key.is_empty()).then_some(key)
}

/// A mapping pair's `value` field node (the nested mapping/scalar), if present.
fn pair_value(pair: Node<'_>) -> Option<Node<'_>> {
    pair.child_by_field_name("value")
}

/// The trimmed text of `node`'s `field` child, or `None` when absent, non-UTF-8,
/// or empty after trimming.
fn field_text(node: Node<'_>, field: &str, source: &[u8]) -> Option<String> {
    let child = node.child_by_field_name(field)?;
    let text = child.utf8_text(source).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Strip one matching pair of surrounding ASCII quotes (`"`/`'`) from a trimmed
/// string, so a JSON `"openapi"` key and a YAML `openapi` key normalise to the
/// same token. Only a *matching* leading/trailing quote pair is removed.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// The OpenAPI / Swagger content profile (S-067, [FR-CG-03]).
mod openapi {
    use super::*;

    /// The profile tag written to a promoted document's `ConfigFile` body — the
    /// FTS-searchable marker that this file is an OpenAPI spec. Both the
    /// `openapi:` (3.x) and `swagger:` (2.0) sniff keys map to this single
    /// OpenAPI/Swagger profile.
    const PROFILE_TAG: &str = "openapi";

    /// The top-level version-bearing keys that mark an OpenAPI/Swagger document
    /// (`openapi: 3.x.y`, `swagger: "2.0"`). Compared case-insensitively.
    const VERSION_KEYS: &[&str] = &["openapi", "swagger"];

    /// The fixed key whose value is the map of path templates ([OpenAPI §4.8.8]).
    const PATHS_KEY: &str = "paths";

    /// The OpenAPI Path Item HTTP method keys ([OpenAPI §4.8.10]). A path-item key
    /// outside this set (`parameters`, `summary`, a `$ref`, an `x-` extension) is
    /// **not** an operation and yields no `ApiOperation` — never fabricated.
    /// Compared case-insensitively.
    const HTTP_METHODS: &[&str] = &[
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    /// Sniff `root` for the OpenAPI profile and, on a match, promote the document:
    /// tag its `ConfigFile` and emit the `ApiPath`/`ApiOperation` tree. Returns
    /// `true` iff the document sniffed as OpenAPI (so [`super::promote`] stops).
    pub(super) fn try_promote(
        pctx: &PromotionCtx<'_>,
        root: Node<'_>,
        config_file_symbol: &LogosSymbol,
        facts: &mut Facts,
    ) -> bool {
        let mut top = Vec::new();
        shallowest_pairs(root, &mut top);

        if !sniff(&top, pctx.source) {
            return false;
        }
        tag_config_file(config_file_symbol, facts);
        promote_paths(&top, pctx, config_file_symbol, facts);
        true
    }

    /// The deterministic content sniff ([FR-CG-03]): a top-level `openapi:` or
    /// `swagger:` key whose value is genuinely *version-bearing*. A document that
    /// merely has a `paths:` key but no version key — or an `openapi:` key with no
    /// value, an empty value, or a YAML null — is **not** OpenAPI; this guard is
    /// what the negative fixtures exercise ([UAT-CG-02]).
    fn sniff(top: &[Node<'_>], source: &[u8]) -> bool {
        top.iter().any(|&pair| {
            pair_key(pair, source)
                .is_some_and(|k| VERSION_KEYS.contains(&k.to_ascii_lowercase().as_str()))
                && pair_value(pair).is_some_and(|v| is_version_value(v, source))
        })
    }

    /// `true` if a `value` node carries a genuine version string: present, and
    /// non-empty / non-null after unquoting. A quoted empty string (`""`) or a
    /// YAML null literal (`null`/`~`) is **not** version-bearing, so `openapi: ""`
    /// and `swagger: null` do not promote — the "version-bearing" half of the
    /// sniff, honest over a degenerate value ([FR-CG-03], [NFR-RA-05]).
    fn is_version_value(value: Node<'_>, source: &[u8]) -> bool {
        let Ok(text) = value.utf8_text(source) else {
            return false;
        };
        let v = unquote(text.trim());
        !v.is_empty() && !matches!(v.as_str(), "null" | "Null" | "NULL" | "~")
    }

    /// Tag the document's `ConfigFile` node with the profile, in its FTS-indexed
    /// `body` — the only mutation promotion makes to an existing node, and the one
    /// the acceptance requires ("its `ConfigFile` is tagged with the profile").
    /// The generic `ConfigSection` nodes are never touched.
    fn tag_config_file(config_file_symbol: &LogosSymbol, facts: &mut Facts) {
        if let Some(node) = facts
            .nodes
            .iter_mut()
            .find(|n| n.kind == NodeKind::ConfigFile && &n.symbol == config_file_symbol)
        {
            node.body = Some(PROFILE_TAG.to_string());
        }
    }

    /// Emit one `ApiPath` per path template under `paths`, each with one
    /// `ApiOperation` per HTTP method nested beneath it by `Contains`. Ordinals are
    /// assigned in document order (per slug), so re-index is byte-identical and the
    /// YAML/JSON twins promote identically ([NFR-RA-06]).
    fn promote_paths(
        top: &[Node<'_>],
        pctx: &PromotionCtx<'_>,
        config_file_symbol: &LogosSymbol,
        facts: &mut Facts,
    ) {
        let Some(paths_pair) = top
            .iter()
            .copied()
            .find(|&p| pair_key(p, pctx.source).as_deref() == Some(PATHS_KEY))
        else {
            return; // a spec with no `paths` map promotes only the ConfigFile tag
        };
        let Some(paths_value) = pair_value(paths_pair) else {
            return;
        };

        let mut path_pairs = Vec::new();
        shallowest_pairs(paths_value, &mut path_pairs);

        let mut path_ordinals: HashMap<String, u32> = HashMap::new();
        for path_pair in path_pairs {
            let Some(template) = pair_key(path_pair, pctx.source) else {
                continue;
            };
            // A Path Item key is a path template — it always begins with `/`
            // ([OpenAPI §4.8.8]). A non-`/` key under `paths` (an `x-` extension, a
            // `$ref`) is not a path and is skipped — never fabricated as an ApiPath.
            if !template.starts_with('/') {
                continue;
            }
            let slug = anchor_slug(&template);
            let ordinal = next_ordinal(&mut path_ordinals, &slug);

            let Some((path_symbol, path_chain)) = emit_node(
                NodeKind::ApiPath,
                &template,
                &slug,
                ordinal,
                &[],
                config_file_symbol,
                path_pair,
                pctx,
                facts,
            ) else {
                continue;
            };

            promote_operations(path_pair, &path_chain, &path_symbol, pctx, facts);
        }
    }

    /// Emit one `ApiOperation` per HTTP-method key under `path_pair`'s value,
    /// nested under the `ApiPath` (`path_symbol`/`path_chain`) by `Contains`. The
    /// operation's name is the lowercased method, so a path's operations are
    /// distinguishable and the YAML/JSON twins agree.
    fn promote_operations(
        path_pair: Node<'_>,
        path_chain: &[String],
        path_symbol: &LogosSymbol,
        pctx: &PromotionCtx<'_>,
        facts: &mut Facts,
    ) {
        let Some(path_value) = pair_value(path_pair) else {
            return;
        };
        let mut op_pairs = Vec::new();
        shallowest_pairs(path_value, &mut op_pairs);

        let mut op_ordinals: HashMap<String, u32> = HashMap::new();
        for op_pair in op_pairs {
            let Some(method_key) = pair_key(op_pair, pctx.source) else {
                continue;
            };
            let method = method_key.to_ascii_lowercase();
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let slug = anchor_slug(&method);
            let ordinal = next_ordinal(&mut op_ordinals, &slug);

            emit_node(
                NodeKind::ApiOperation,
                &method,
                &slug,
                ordinal,
                path_chain,
                path_symbol,
                op_pair,
                pctx,
                facts,
            );
        }
    }
}

#[cfg(all(test, feature = "lang-yaml", feature = "lang-json"))]
mod tests;
