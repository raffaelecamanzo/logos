//! Per-format **typed-anchor** walks for the config/artifact layer (S-066,
//! [CR-010], [ADR-25], [FR-CG-03]).
//!
//! The generic substrate ([`super`], S-062) builds a [`NodeKind::ConfigFile`]
//! root and — for data formats with a `[config]` table — a depth-bounded
//! [`NodeKind::ConfigSection`] tree. A **typed-anchor** format ships no `[config]`
//! table and instead emits its own structural anchors over that `ConfigFile`
//! root through the per-format walk dispatched here ([`extract_typed_anchors`]),
//! exactly as the substrate contract describes ("its own per-format walk, added
//! by that story").
//!
//! This module is the S-066 increment: **Terraform** ([`terraform`]) → `TfBlock`
//! anchors whose name carries the block-type payload (resource/data/module/
//! variable/output/provider/…), and **SQL** ([`sql`]) → `SqlObject` anchors over
//! a conservative, DDL-only set of `create_*` statements.
//!
//! # The honesty discipline ([NFR-RA-05], never fabricate)
//!
//! Both walks are pure [`tree_sitter::Node`] traversals keyed by node-kind
//! strings, so this module needs **no** grammar crate and compiles under the
//! default feature set; it is only *exercised* when the matching grammar is
//! linked. An anchor is emitted only for a node kind the walk explicitly
//! recognises, with a name read from the parse tree — never invented. The SQL
//! walk additionally **skips on misparse**: a candidate whose subtree carries a
//! syntax error, or whose object name cannot be read cleanly, yields no node
//! rather than a guessed one. An unparseable dialect construct degrades to an
//! `ERROR` node, which is never a recognised `create_*` kind, so it is
//! structurally impossible to fabricate an anchor for it.
//!
//! # Identity & metric-neutrality
//!
//! Anchors reuse the substrate's `path#anchor` identity ([`super::super::symbol`],
//! [ADR-07]) and the shared [`super::anchor_slug`], so re-index is byte-identical
//! ([NFR-RA-06]). `TfBlock`/`SqlObject` are [`NodeKind::is_config`] and hence
//! excluded from the code subgraph at hydration ([FR-CG-05]) — adding or removing
//! an infra artifact never moves the quality signal.
//!
//! [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
//! [ADR-07]: ../../../../docs/specs/architecture/decisions/ADR-07.md
//! [ADR-25]: ../../../../docs/specs/architecture/decisions/ADR-25.md
//! [FR-CG-03]: ../../../../docs/specs/requirements/FR-CG-03.md
//! [FR-CG-05]: ../../../../docs/specs/requirements/FR-CG-05.md
//! [NFR-RA-05]: ../../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::HashMap;

use tree_sitter::Node;

use crate::model::{LogosSymbol, NodeKind};
use crate::plugin::LanguagePlugin;

use super::super::{Facts, SymbolContext};
use super::{anchor_slug, next_ordinal, DEPTH_BOUND};

/// Dispatch the per-format typed-anchor walk for an artifact `plugin` over its
/// already-parsed `root`, attaching anchors under the `ConfigFile`
/// `parent_symbol`. A no-op for an artifact that declares no typed-anchor format
/// (e.g. a generic data format, whose sections the substrate already walked).
///
/// The selector is the plugin **name** — the documented per-format walk
/// identifier ([CR-010]). A format the engine does not recognise simply gets no
/// anchors, never an error.
///
/// [CR-010]: ../../../../docs/requests/CR-010-config-artifact-graph-layer.md
#[allow(clippy::too_many_arguments)]
pub(super) fn extract_typed_anchors(
    plugin: &dyn LanguagePlugin,
    root: Node<'_>,
    segments: &[&str],
    parent_symbol: &LogosSymbol,
    source: &[u8],
    ctx: &SymbolContext,
    facts: &mut Facts,
) {
    let ectx = AnchorWalk {
        ctx,
        segments,
        source,
    };
    match plugin.name() {
        "terraform" => terraform::walk(root, parent_symbol, &ectx, facts),
        "sql" => sql::walk(root, parent_symbol, &ectx, facts),
        _ => {}
    }
}

/// Per-format emit context — a type alias for the shared [`super::EmitCtx`] so
/// the Terraform and SQL sub-walks can keep their existing `&AnchorWalk<'_>`
/// parameter type without change. The two structs were structurally identical;
/// the alias collapses them into one (sprint-10 coherence fix, S-066/S-067).
type AnchorWalk<'a> = super::EmitCtx<'a>;

/// Build one typed anchor: its `path#anchor` symbol and [`EdgeKind::Contains`]
/// edge from `parent_symbol`. A thin wrapper over [`super::emit_anchored_node`]
/// that hides the `body` parameter (infra anchors carry no payload) and keeps
/// the Terraform/SQL sub-walk call sites unchanged.
#[allow(clippy::too_many_arguments)]
fn emit_anchor(
    kind: NodeKind,
    name: &str,
    slug: &str,
    ordinal: u32,
    parent_chain: &[String],
    parent_symbol: &LogosSymbol,
    start_line: u32,
    end_line: u32,
    walk: &AnchorWalk<'_>,
    facts: &mut Facts,
) -> Option<(LogosSymbol, Vec<String>)> {
    super::emit_anchored_node(
        kind,
        name,
        slug,
        ordinal,
        parent_chain,
        parent_symbol,
        start_line,
        end_line,
        None,
        walk,
        facts,
    )
}

/// Terraform/HCL `TfBlock` extraction ([FR-CG-03]).
mod terraform {
    use super::*;

    /// Walk the HCL block tree under the `config_file` root, emitting a `TfBlock`
    /// per `block` with its block-type payload, bounded at [`DEPTH_BOUND`] like
    /// the generic section walk.
    ///
    /// Unlike the SQL walk, Terraform is **not** skip-on-misparse: a block's type
    /// and labels live in its *header*, which stays readable even when the block
    /// *body* carries a syntax error, so a `TfBlock` is still emitted (error-
    /// tolerant, [FR-IX-04]) — never a fabricated name (a block with no type
    /// identifier is dropped by [`block_name`]). SQL needs the stricter skip
    /// because its honesty case is whole-statement dialect misparse ([NFR-RA-05]).
    pub(super) fn walk(
        root: Node<'_>,
        parent_symbol: &LogosSymbol,
        walk: &AnchorWalk<'_>,
        facts: &mut Facts,
    ) {
        // The grammar root (`config_file`) holds a single `body`; blocks live
        // inside it. Walk every `body` child defensively (the structure is
        // `config_file → body → block`).
        let mut cursor = root.walk();
        for child in root.named_children(&mut cursor) {
            if child.kind() == "body" {
                walk_blocks(child, &[], parent_symbol, 0, walk, facts);
            }
        }
    }

    /// Emit a `TfBlock` for each `block` directly inside `container`, then recurse
    /// into each block's own `body` for nested blocks while within the depth bound.
    fn walk_blocks(
        container: Node<'_>,
        parent_chain: &[String],
        parent_symbol: &LogosSymbol,
        depth: usize,
        walk: &AnchorWalk<'_>,
        facts: &mut Facts,
    ) {
        let mut ordinals: HashMap<String, u32> = HashMap::new();
        let mut cursor = container.walk();
        for block in container.named_children(&mut cursor) {
            if block.kind() != "block" {
                continue;
            }
            // The block header is `<type> <label>* { … }`: the first `identifier`
            // child is the block TYPE (the payload), the `string_lit` children are
            // its labels. A block with no type identifier is malformed — skip it
            // rather than fabricate a nameless anchor (NFR-RA-05).
            let Some(name) = block_name(block, walk.source) else {
                continue;
            };
            let slug = anchor_slug(&name);
            let ordinal = next_ordinal(&mut ordinals, &slug);

            let Some((symbol, chain)) = emit_anchor(
                NodeKind::TfBlock,
                &name,
                &slug,
                ordinal,
                parent_chain,
                parent_symbol,
                block.start_position().row as u32 + 1,
                block.end_position().row as u32 + 1,
                walk,
                facts,
            ) else {
                continue;
            };

            // Recurse into the block's body for nested blocks (e.g. a `lifecycle`
            // or `ingress` block inside a `resource`), staying within the fixed
            // bound. `depth` is the recursion level (0 = top-level blocks): with
            // DEPTH_BOUND = 2 we emit two visible block levels — top-level blocks
            // (depth 0) and one nested level (depth 1) — and stop, so a block
            // nested two levels deep is invisible. This matches the section walk's
            // two-level visibility (BR-30, FR-CG-02).
            if depth + 1 < DEPTH_BOUND {
                let mut bc = block.walk();
                for body in block.named_children(&mut bc) {
                    if body.kind() == "body" {
                        walk_blocks(body, &chain, &symbol, depth + 1, walk, facts);
                    }
                }
            }
        }
    }

    /// The human-facing, FTS-indexed name of a `TfBlock`: the block type followed
    /// by its unquoted labels, space-joined (`resource "aws_instance" "web"` →
    /// `resource aws_instance web`, `variable "region"` → `variable region`,
    /// `terraform { … }` → `terraform`). The leading token is the block-type
    /// payload that distinguishes resource/data/module/variable/output/provider.
    /// `None` when the block carries no type identifier (malformed — never named).
    fn block_name(block: Node<'_>, source: &[u8]) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        let mut cursor = block.walk();
        for child in block.named_children(&mut cursor) {
            match child.kind() {
                "identifier" if parts.is_empty() => {
                    let text = child.utf8_text(source).ok()?.trim();
                    if text.is_empty() {
                        return None;
                    }
                    parts.push(text.to_string());
                }
                "string_lit" => {
                    if let Ok(text) = child.utf8_text(source) {
                        let unquoted = text.trim().trim_matches('"').trim();
                        if !unquoted.is_empty() {
                            parts.push(unquoted.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        // The first part must be the block-type identifier.
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    }
}

/// SQL `SqlObject` extraction — the conservative, DDL-anchors-only honesty case
/// ([FR-CG-03], [NFR-RA-05]).
mod sql {
    use super::*;

    /// The conservative, portable set of DDL `create_*` statement kinds that
    /// `tree-sitter-sequel` parses cleanly **and** from which a stable object name
    /// is reliably extractable — the *measured* dialect coverage (see the impl
    /// notes). A statement whose kind is not in this set (an unparsed dialect
    /// construct degrades to an `ERROR` node, never a `create_*` kind) is skipped,
    /// so no anchor is ever fabricated.
    const DDL_KINDS: &[&str] = &[
        "create_table",
        "create_view",
        "create_materialized_view",
        "create_index",
        "create_schema",
        "create_sequence",
        "create_function",
        "create_trigger",
        "create_type",
        "create_database",
        "create_extension",
        "create_role",
    ];

    /// Walk the top-level statements under the `program` root, emitting one
    /// `SqlObject` per recognised, cleanly-parsed DDL definition.
    pub(super) fn walk(
        root: Node<'_>,
        parent_symbol: &LogosSymbol,
        walk: &AnchorWalk<'_>,
        facts: &mut Facts,
    ) {
        let mut ordinals: HashMap<String, u32> = HashMap::new();
        let mut cursor = root.walk();
        for stmt in root.named_children(&mut cursor) {
            // The grammar wraps each top-level DDL in a `statement`; tolerate a
            // `create_*` directly under the root too. Anything else (a `select`, an
            // `ERROR` from an unparsed construct) is not a DDL definition — skip it.
            let Some(ddl) = ddl_node(stmt) else {
                continue;
            };
            // Skip-on-misparse: a candidate whose subtree carries any syntax error
            // is not anchored — we never emit a node we are not sure of (NFR-RA-05).
            if ddl.has_error() {
                continue;
            }
            let Some(object_name) = object_name(ddl, walk.source) else {
                continue; // name not cleanly readable → skip, never fabricate
            };
            let object_type = object_type(ddl.kind());
            // The FTS-indexed name carries the object-type payload as its leading
            // token (`table app.orders`, `view active_users`, `index idx_email`),
            // so SqlObject anchors are distinguishable and kind-filterable.
            let name = format!("{object_type} {object_name}");
            let slug = anchor_slug(&name);
            let ordinal = next_ordinal(&mut ordinals, &slug);

            emit_anchor(
                NodeKind::SqlObject,
                &name,
                &slug,
                ordinal,
                &[],
                parent_symbol,
                ddl.start_position().row as u32 + 1,
                ddl.end_position().row as u32 + 1,
                walk,
                facts,
            );
        }
    }

    /// The recognised DDL node for a top-level item: the item itself if it is a
    /// `create_*` kind, else its first `create_*` child (the common
    /// `statement → create_*` wrapping). `None` for any non-DDL item.
    fn ddl_node(item: Node<'_>) -> Option<Node<'_>> {
        if is_ddl(item.kind()) {
            return Some(item);
        }
        // Index iteration (not a `walk()` cursor) so the returned node's lifetime
        // is tied to the tree, not a local cursor borrow.
        for i in 0..item.named_child_count() {
            let child = item.named_child(i)?;
            if is_ddl(child.kind()) {
                return Some(child);
            }
        }
        None
    }

    fn is_ddl(kind: &str) -> bool {
        DDL_KINDS.contains(&kind)
    }

    /// The object-type payload of a `create_*` node: the kind with the `create_`
    /// prefix stripped and underscores spaced (`create_materialized_view` →
    /// `materialized view`, `create_table` → `table`).
    fn object_type(kind: &str) -> String {
        kind.strip_prefix("create_")
            .unwrap_or(kind)
            .replace('_', " ")
    }

    /// The object's name, read from the parse tree per the DDL kind's measured
    /// shape — never invented. `None` when no name is cleanly readable, so the
    /// caller skips the anchor (NFR-RA-05):
    /// - `create_index`: the index name is the `column`-field identifier.
    /// - `create_schema`/`database`/`extension`/`role`: a direct `identifier` child.
    /// - all others (table/view/materialized view/sequence/function/trigger/type):
    ///   the first `object_reference` child, schema-qualified (`schema.name`) when
    ///   it carries a `schema` field.
    fn object_name(ddl: Node<'_>, source: &[u8]) -> Option<String> {
        match ddl.kind() {
            "create_index" => ddl
                .child_by_field_name("column")
                .and_then(|n| non_empty_text(n, source)),
            "create_schema" | "create_database" | "create_extension" | "create_role" => {
                first_child_of_kind(ddl, "identifier").and_then(|n| non_empty_text(n, source))
            }
            _ => {
                let obj = first_child_of_kind(ddl, "object_reference")?;
                let name = obj
                    .child_by_field_name("name")
                    .and_then(|n| non_empty_text(n, source))?;
                match obj
                    .child_by_field_name("schema")
                    .and_then(|n| non_empty_text(n, source))
                {
                    Some(schema) => Some(format!("{schema}.{name}")),
                    None => Some(name),
                }
            }
        }
    }

    /// The first direct named child of `node` with the given `kind`. Index
    /// iteration keeps the returned node's lifetime tied to the tree, not a local
    /// `walk()` cursor borrow.
    fn first_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
        for i in 0..node.named_child_count() {
            let child = node.named_child(i)?;
            if child.kind() == kind {
                return Some(child);
            }
        }
        None
    }

    /// The trimmed UTF-8 text of `node`, or `None` when it is non-UTF-8 or empty.
    fn non_empty_text(node: Node<'_>, source: &[u8]) -> Option<String> {
        let text = node.utf8_text(source).ok()?.trim();
        (!text.is_empty()).then(|| text.to_string())
    }
}

// The anchor walks need a linked grammar to exercise, so the tests run only when
// both infra grammars are compiled in (`--features lang-terraform,lang-sql`). The
// walk *logic* compiles under the default set; these prove it end-to-end through
// the public `extract` entry point over the real `tree-sitter-hcl`/`-sequel`
// grammars (S-066, the format-level proofs over the S-062 substrate mechanism).
#[cfg(all(test, feature = "lang-terraform", feature = "lang-sql"))]
mod tests {
    use std::collections::BTreeMap;

    use crate::extract::{extract, Facts, FileInput, SymbolContext};
    use crate::model::NodeKind;
    use crate::plugin::{CompiledPlugin, PluginManifest};

    /// The real Terraform artifact plugin, built from the embedded descriptor and
    /// the linked `tree-sitter-hcl` grammar — pure plugin data, no core hooks.
    fn terraform_plugin() -> CompiledPlugin {
        let toml = include_str!("../../../plugins/terraform/plugin.toml");
        let manifest = PluginManifest::parse("terraform/plugin.toml", toml).unwrap();
        let language: tree_sitter::Language = tree_sitter_hcl::LANGUAGE.into();
        CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
    }

    /// The real SQL artifact plugin, built from the embedded descriptor and the
    /// linked `tree-sitter-sequel` grammar.
    fn sql_plugin() -> CompiledPlugin {
        let toml = include_str!("../../../plugins/sql/plugin.toml");
        let manifest = PluginManifest::parse("sql/plugin.toml", toml).unwrap();
        let language: tree_sitter::Language = tree_sitter_sequel::LANGUAGE.into();
        CompiledPlugin::new(manifest, language, BTreeMap::new(), Vec::new())
    }

    fn names_of_kind(facts: &Facts, kind: NodeKind) -> Vec<String> {
        facts
            .nodes
            .iter()
            .filter(|n| n.kind == kind)
            .map(|n| n.name.clone())
            .collect()
    }

    // ── Terraform ───────────────────────────────────────────────────────────

    /// A Terraform fixture yields one `ConfigFile` root and `TfBlock` nodes whose
    /// name leads with the block type, distinguishing resource/data/module/
    /// variable/output/provider ([FR-CG-03]). The first token is the payload.
    #[test]
    fn terraform_blocks_distinguish_block_type() {
        let src = r#"
resource "aws_instance" "web" {
  ami = "ami-123"
}
data "aws_ami" "ubuntu" {
  most_recent = true
}
module "vpc" {
  source = "./vpc"
}
variable "region" {
  default = "us-east-1"
}
output "endpoint" {
  value = "x"
}
provider "aws" {
  region = "us-east-1"
}
terraform {
  required_version = ">= 1.0"
}
"#;
        let facts = extract(
            &FileInput::new("main.tf", src),
            &terraform_plugin(),
            &SymbolContext::default(),
        );

        assert_eq!(
            names_of_kind(&facts, NodeKind::ConfigFile),
            ["main.tf"],
            "exactly one ConfigFile root"
        );

        let blocks = names_of_kind(&facts, NodeKind::TfBlock);
        // The six payload-bearing block types the acceptance enumerates, plus the
        // settings `terraform` block.
        let block_type = |name: &str| name.split(' ').next().unwrap().to_string();
        let types: Vec<String> = blocks.iter().map(|n| block_type(n)).collect();
        for expected in [
            "resource", "data", "module", "variable", "output", "provider",
        ] {
            assert!(
                types.iter().any(|t| t == expected),
                "block type '{expected}' must be a distinguishable TfBlock payload; got {blocks:?}"
            );
        }
        // The labels ride in the name, so the payload is fully recoverable.
        assert!(blocks.iter().any(|n| n == "resource aws_instance web"));
        assert!(blocks.iter().any(|n| n == "variable region"));
        assert!(blocks.iter().any(|n| n == "provider aws"));
        assert!(blocks.iter().any(|n| n == "terraform"));
    }

    /// A nested block (`lifecycle` inside a `resource`) is emitted as a child
    /// `TfBlock` under its parent, bounded at the fixed depth of 2 — a third-level
    /// block is deliberately invisible, mirroring the section depth bound (BR-30).
    #[test]
    fn terraform_nested_blocks_are_bounded() {
        let src = r#"
resource "aws_instance" "web" {
  ami = "ami-123"
  lifecycle {
    create_before_destroy = true
    nested_too {
      deep = true
    }
  }
}
"#;
        let facts = extract(
            &FileInput::new("main.tf", src),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        let blocks = names_of_kind(&facts, NodeKind::TfBlock);
        assert!(blocks.iter().any(|n| n == "resource aws_instance web"));
        assert!(
            blocks.iter().any(|n| n == "lifecycle"),
            "depth-2 nested block is emitted: {blocks:?}"
        );
        assert!(
            !blocks.iter().any(|n| n == "nested_too"),
            "depth-3 block is past the bound and must not be emitted: {blocks:?}"
        );
    }

    /// CRA-02: a `.tfvars` file parses with the same grammar and yields a clean
    /// `ConfigFile` root (its top-level entries are attributes, not blocks, so it
    /// has no `TfBlock` — and crucially the extraction is not partial).
    #[test]
    fn tfvars_parses_cleanly_as_configfile() {
        let src = "region = \"us-east-1\"\ninstance_count = 3\ntags = { Name = \"web\" }\n";
        let facts = extract(
            &FileInput::new("terraform.tfvars", src),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        assert!(
            !facts.partial,
            "CRA-02: .tfvars must parse cleanly (not partial)"
        );
        assert_eq!(
            names_of_kind(&facts, NodeKind::ConfigFile),
            ["terraform.tfvars"]
        );
        assert!(
            names_of_kind(&facts, NodeKind::TfBlock).is_empty(),
            "a values-only .tfvars has no blocks"
        );
    }

    // ── SQL — the honesty case ───────────────────────────────────────────────

    /// THE honesty fixture ([NFR-RA-05], [UAT-CG-02]): a file mixing portable DDL
    /// with an unparseable dialect construct (T-SQL `CREATE PROCEDURE`). The
    /// portable objects yield `SqlObject` anchors; the procedure yields **none** —
    /// no fabricated node — and the parse is marked partial.
    #[test]
    fn sql_portable_ddl_anchored_unparseable_dialect_skipped() {
        let src = r#"
CREATE TABLE app.users (id INT PRIMARY KEY, email TEXT);
CREATE VIEW active_users AS SELECT * FROM app.users WHERE active = 1;
CREATE INDEX idx_email ON app.users (email);

-- T-SQL stored procedure: tree-sitter-sequel cannot parse this dialect
-- construct, so it degrades to an ERROR node and must NOT be anchored.
CREATE PROCEDURE dbo.GetUsers AS BEGIN SELECT 1 END;
"#;
        let facts = extract(
            &FileInput::new("schema.sql", src),
            &sql_plugin(),
            &SymbolContext::default(),
        );

        let objects = names_of_kind(&facts, NodeKind::SqlObject);
        // Exactly the three portable objects — never four.
        assert_eq!(
            objects.len(),
            3,
            "only the portable DDL is anchored; the dialect construct is skipped: {objects:?}"
        );
        assert!(objects.iter().any(|n| n == "table app.users"));
        assert!(objects.iter().any(|n| n == "view active_users"));
        assert!(objects.iter().any(|n| n == "index idx_email"));
        // Never-fabricate: no anchor mentions the procedure.
        assert!(
            !objects
                .iter()
                .any(|n| n.contains("GetUsers") || n.starts_with("procedure")),
            "the unparseable CREATE PROCEDURE must not be fabricated as an anchor: {objects:?}"
        );
        // The unparsed construct made the parse partial — honestly recorded.
        assert!(
            facts.partial,
            "a file containing an unparseable construct is partially extracted (FR-IX-04)"
        );
    }

    /// `SqlObject` names lead with the object-type payload, distinguishing
    /// table/view/index/materialized view/schema/sequence/function ([FR-CG-03]).
    #[test]
    fn sql_object_type_payload_distinguishes_kinds() {
        let src = r#"
CREATE TABLE t (id INT);
CREATE VIEW v AS SELECT 1;
CREATE MATERIALIZED VIEW mv AS SELECT 1;
CREATE INDEX i ON t (id);
CREATE SCHEMA s;
CREATE SEQUENCE seq START 1;
CREATE FUNCTION f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql;
"#;
        let facts = extract(
            &FileInput::new("ddl.sql", src),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        let objects = names_of_kind(&facts, NodeKind::SqlObject);
        let obj_type = |name: &str| name.rsplit_once(' ').map(|(t, _)| t.to_string());
        for (expected, obj_name) in [
            ("table", "t"),
            ("view", "v"),
            ("materialized view", "mv"),
            ("index", "i"),
            ("schema", "s"),
            ("sequence", "seq"),
            ("function", "f"),
        ] {
            let full = format!("{expected} {obj_name}");
            assert!(
                objects.iter().any(|n| n == &full),
                "expected SqlObject '{full}' (payload distinguishes the object type); got {objects:?}"
            );
        }
        // The materialized-view payload is multi-word but still leads the name.
        assert!(objects
            .iter()
            .any(|n| obj_type(n).as_deref() == Some("materialized view")));
    }

    /// A SQL file that is *entirely* an unparseable dialect construct yields no
    /// `SqlObject` at all — only the `ConfigFile` root — proving the conservative
    /// floor never invents an anchor under total misparse ([NFR-RA-05]).
    #[test]
    fn sql_total_misparse_yields_no_fabricated_anchor() {
        let src = "CREATE PROCEDURE dbo.X AS BEGIN SELECT 1 END;\n";
        let facts = extract(
            &FileInput::new("proc.sql", src),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        assert_eq!(
            names_of_kind(&facts, NodeKind::ConfigFile),
            ["proc.sql"],
            "the ConfigFile root is always emitted"
        );
        assert!(
            names_of_kind(&facts, NodeKind::SqlObject).is_empty(),
            "no SqlObject is fabricated for a wholly-unparseable file"
        );
        // The unparseable construct is honestly recorded as a partial parse
        // ([FR-IX-04]) — guards against a regression that silently drops the flag.
        assert!(
            facts.partial,
            "a wholly-unparseable file is partially extracted"
        );
    }

    // ── Cross-cutting: determinism & metric-neutrality ───────────────────────

    /// Re-extraction is byte-identical for both formats ([NFR-RA-06]).
    #[test]
    fn anchors_reindex_byte_identical() {
        let tf = "resource \"aws_instance\" \"web\" {\n  ami = \"x\"\n}\nvariable \"r\" {}\n";
        let a = extract(
            &FileInput::new("m.tf", tf),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        let b = extract(
            &FileInput::new("m.tf", tf),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        assert_eq!(a.nodes, b.nodes, "Terraform re-extract is byte-identical");
        assert_eq!(a.edges, b.edges);

        let sql =
            "CREATE TABLE a (id INT);\nCREATE TABLE a (id INT);\nCREATE VIEW v AS SELECT 1;\n";
        let c = extract(
            &FileInput::new("s.sql", sql),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        let d = extract(
            &FileInput::new("s.sql", sql),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        assert_eq!(
            c.nodes, d.nodes,
            "SQL re-extract is byte-identical (incl. sibling-ordinal disambiguation)"
        );
        assert_eq!(c.edges, d.edges);
        // Two identically-named tables disambiguate by sibling ordinal, so both
        // anchors exist with distinct symbols.
        assert_eq!(
            names_of_kind(&c, NodeKind::SqlObject)
                .iter()
                .filter(|n| *n == "table a")
                .count(),
            2,
            "same-named siblings are both anchored (ordinal-disambiguated)"
        );
    }

    /// Every anchor is a config kind in the non-code scope, so it is excluded from
    /// the code subgraph at hydration — the metric-neutrality contract at the node
    /// level ([FR-CG-05]). `TfBlock`/`SqlObject` and their `Contains` edges never
    /// enter metrics, cycles, DSM, or dead-code.
    #[test]
    fn anchors_are_metric_neutral_config_kinds() {
        let tf = extract(
            &FileInput::new("m.tf", "resource \"a\" \"b\" {}\n"),
            &terraform_plugin(),
            &SymbolContext::default(),
        );
        for n in &tf.nodes {
            assert!(
                n.kind.is_config() && n.kind.is_non_code(),
                "{:?} must be a non-code config kind",
                n.kind
            );
        }
        let sql = extract(
            &FileInput::new("s.sql", "CREATE TABLE t (id INT);\n"),
            &sql_plugin(),
            &SymbolContext::default(),
        );
        for n in &sql.nodes {
            assert!(
                n.kind.is_config() && n.kind.is_non_code(),
                "{:?} must be a non-code config kind",
                n.kind
            );
        }
    }
}
