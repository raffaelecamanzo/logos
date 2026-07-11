//! Structural config & artifact extraction (S-062, [CR-010], [ADR-25]).
//!
//! The config sibling of [`super::doc`]: a file whose plugin is an **artifact**
//! grammar ([`LanguagePlugin::is_artifact`]) is extracted structurally into a
//! [`NodeKind::ConfigFile`] root and a [`EdgeKind::Contains`] tree of
//! [`NodeKind::ConfigSection`] nodes ([FR-CG-02]) — never via the code `symbols`
//! query. Per-format **typed anchors** (Dockerfile stages, Makefile targets,
//! shell functions, …, [FR-CG-03]) ride this *same* generic walk: the
//! descriptor's `[config] node_kind` names the kind to emit, so a single-anchor
//! format becomes pure plugin data with no per-format core code (S-064). A
//! format whose anchors carry richer structure (multiple kinds, a type-kind
//! payload — Protobuf, GraphQL, …) may still layer its own walk over the
//! `ConfigFile` root in a later story; the substrate ships the generic file +
//! depth-bounded section walk every format reuses.
//!
//! # Generic, descriptor-driven section walk
//!
//! The walk is grammar-agnostic: the descriptor's `[config] section_kinds`
//! ([`ConfigDescriptor`]) names the tree-sitter node kinds that introduce a
//! `ConfigSection` (a YAML/TOML/JSON mapping pair), and `key_field` names the
//! field whose text is the section's key. This is the same declarative pattern
//! as `nesting_block_kinds` — the core algorithm is fixed, the per-format node
//! kinds are pure plugin data ([NFR-MA-01], [ADR-09]). `node_kind` names the
//! emitted [`NodeKind`] (default [`NodeKind::ConfigSection`]); a build format
//! sets e.g. `node_kind = "dockerfile_stage"` with `section_kinds` naming the
//! grammar's stage node, and the walk emits one typed anchor per match. A format
//! that needs no sections at all ships no `[config]` table and yields just a
//! `ConfigFile` here.
//!
//! # Depth bound ([BR-30])
//!
//! Sections nest to a **fixed depth of 2**, deterministic regardless of file
//! size: top-level mapping keys are depth 1, their nested keys depth 2, and
//! deeper structure is deliberately invisible rather than risk graph/FTS blow-up
//! on config-heavy repos (large Helm values, generated JSON). The bound is fixed
//! here, never size-dependent ([NFR-RA-06]).
//!
//! # Identity — `path#anchor`, reusing [ADR-07]
//!
//! A `ConfigFile`'s symbol is the file path rendered as a namespace; a
//! `ConfigSection` appends one type-descriptor per enclosing key slug, so it
//! renders as the `…/path/key#` anchor form, sibling-ordinal-disambiguated in
//! document order — the exact construction the documentation layer uses
//! ([`super::doc`]), so config nodes ride the shared symbols/nodes/FTS tables
//! with no parallel store. Because a slug depends only on its own key and an
//! ordinal only on same-slug siblings, editing one key never churns another
//! section's id ([NFR-RA-06]).
//!
//! # Metric-neutrality ([FR-CG-05])
//!
//! Config kinds are [`NodeKind::is_config`], folded into the non-code scope
//! [`NodeKind::is_non_code`] that graph hydration excludes from the code
//! subgraph — so a `ConfigFile`/`ConfigSection` (and the `Contains` edges between
//! them) never enters metrics, cycles, DSM, or dead-code. Adding or removing a
//! config artifact leaves the aggregate signal byte-identical.
//!
//! [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
//! [ADR-09]: ../../../docs/specs/architecture/decisions/ADR-09.md
//! [ADR-25]: ../../../docs/specs/architecture/decisions/ADR-25.md
//! [FR-CG-02]: ../../../docs/specs/requirements/FR-CG-02.md
//! [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
//! [FR-CG-05]: ../../../docs/specs/requirements/FR-CG-05.md
//! [NFR-MA-01]: ../../../docs/specs/requirements/NFR-MA-01.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use crate::model::{EdgeKind, LogosSymbol, NodeKind};
use crate::plugin::{AnchorDescriptor, ConfigDescriptor, LanguagePlugin};

use super::symbol::{build_symbol, descriptor_for, path_segments};
use super::{EdgeFact, Facts, FileInput, NodeFact, SymbolContext};

// Per-format typed-anchor walks (S-066, CR-010, FR-CG-03): a typed-anchor format
// (Terraform, SQL, …) emits its structural anchors over the `ConfigFile` root
// here, dispatched by plugin. Pure tree-sitter-node traversal — no grammar crate
// — so this compiles under the default feature set and is only exercised when the
// matching grammar is linked.
mod anchors;

// Content-sniffed profile promotion (S-067, CR-010, FR-CG-03): an additive pass
// that inspects the already-parsed YAML/JSON tree's top-level keys and, for a
// recognised content profile (OpenAPI today), tags the `ConfigFile` and emits its
// typed nodes — never altering the generic section/anchor extraction. The hook
// stays extensible for future profiles, none claimed beyond OpenAPI.
mod profiles;

// Cross-artifact reference capture (S-068, CR-011, FR-CG-07): the seam that lets
// each format's walk capture references between artifacts and to code, bound by
// the resolution pass into ArtifactRef/ArtifactBinding edges. The substrate ships
// the dispatch point and the shared push/classify helper; the per-format arms are
// the consumer stories' (S-069/070/071) isolated extensions.
//
// `pub(crate)` so the pluggable invocation arms (S-252/253/254) can reach the
// generic `capture_invocation_refs` interpreter + `InvocationSite` from the
// code-extraction seam (a code arm's sites come from a `.scm` over code files,
// not the config walk).
pub(crate) mod refs;

/// The fixed maximum `ConfigSection` nesting depth ([BR-30], [FR-CG-02]). A
/// `ConfigFile` is depth 0; sections are emitted at depth 1 and 2 only. The
/// constant is the single source of truth for the bound — never size-dependent.
pub(crate) const DEPTH_BOUND: usize = 2;

/// The immutable per-file context threaded through all typed-anchor and
/// profile-promotion walks — the symbol-builder inputs shared by both
/// [`anchors`] and [`profiles`]. Replaces the structurally-identical local
/// `AnchorWalk` (S-066) and `PromotionCtx` (S-067) structs so both modules
/// share one emit helper and one ordinal helper rather than duplicating them.
pub(super) struct EmitCtx<'a> {
    pub(super) ctx: &'a SymbolContext,
    pub(super) segments: &'a [&'a str],
    pub(super) source: &'a [u8],
}

/// Advance the per-slug ordinal counter and return the ordinal **before** the
/// advance — 0 for the first occurrence of `slug`, then 1, 2, …. Shared by
/// all typed-anchor and profile-promotion walks ([ADR-07], [NFR-RA-06]).
///
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn next_ordinal(ordinals: &mut HashMap<String, u32>, slug: &str) -> u32 {
    let n = *ordinals.get(slug).unwrap_or(&0);
    *ordinals.entry(slug.to_owned()).or_insert(0) += 1;
    n
}

/// Build one typed anchor or profile node: its `path#anchor` symbol
/// (disambiguated by `ordinal` among same-slug siblings under `parent_symbol`),
/// the [`NodeFact`], and the [`EdgeKind::Contains`] edge from `parent_symbol`.
/// Returns `(symbol, chain)` so a nested walk can parent off it, or `None` if
/// the symbol build failed (a warning is recorded; the node is skipped rather
/// than half-emitted).
///
/// Shared by [`walk_anchors`], [`anchors`] (via [`anchors::emit_anchor`]), and
/// [`profiles`] (via [`profiles::emit_node`]) so the chain-build / NodeFact /
/// EdgeFact scaffolding is not duplicated across all four typed-anchor
/// mechanisms ([ADR-07], [NFR-RA-06]).
///
/// [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_anchored_node(
    kind: NodeKind,
    name: &str,
    slug: &str,
    ordinal: u32,
    parent_chain: &[String],
    parent_symbol: &LogosSymbol,
    start_line: u32,
    end_line: u32,
    body: Option<String>,
    ectx: &EmitCtx<'_>,
    facts: &mut Facts,
) -> Option<(LogosSymbol, Vec<String>)> {
    let mut chain = parent_chain.to_vec();
    chain.push(descriptor_for(kind, slug, ordinal));

    let symbol = match build_symbol(ectx.ctx, ectx.segments, &chain) {
        Ok(sym) => sym,
        Err(err) => {
            facts.warnings.push(format!(
                "could not build {kind:?} symbol for '{name}': {err}"
            ));
            return None;
        }
    };

    facts.nodes.push(NodeFact {
        symbol: symbol.clone(),
        kind,
        name: name.to_string(),
        start_line,
        end_line,
        metrics: None,
        exported: false,
        fingerprint: None,
        test_evidence: false,
        // Payload subtype in the FTS-indexed `body` (FR-CG-03): `None` for a
        // payloadless format (Terraform, SQL, ApiPath/ApiOperation), the
        // GraphQL subtype string for `gql_type`, or the OpenAPI profile tag.
        body,
        max_nesting_depth: None,
        shingles: Vec::new(),
    });
    facts.edges.push(EdgeFact {
        source: parent_symbol.clone(),
        target: symbol.clone(),
        kind: EdgeKind::Contains,
    });
    Some((symbol, chain))
}

/// Extract one config/artifact file with an explicit plugin, allocating a fresh
/// parser. The config analogue of [`super::extract`]/[`super::doc::extract_doc`].
pub fn extract_config(
    input: &FileInput,
    plugin: &dyn LanguagePlugin,
    ctx: &SymbolContext,
) -> Facts {
    let mut parser = Parser::new();
    extract_one_config(&mut parser, input, plugin, ctx)
}

/// Core single-file config extraction, reusing the caller's parser.
///
/// Routed here from [`super::extract_one`] when `plugin.is_artifact()` is true,
/// so it shares the per-rayon-worker parser of the parallel driver.
pub(super) fn extract_one_config(
    parser: &mut Parser,
    input: &FileInput,
    plugin: &dyn LanguagePlugin,
    ctx: &SymbolContext,
) -> Facts {
    let mut facts = Facts {
        path: input.path.clone(),
        language: plugin.name().to_string(),
        partial: false,
        nodes: Vec::new(),
        edges: Vec::new(),
        refs: Vec::new(),
        warnings: Vec::new(),
    };

    if parser.set_language(plugin.language()).is_err() {
        facts.warnings.push(format!(
            "grammar '{}' failed to bind; file skipped",
            plugin.name()
        ));
        return facts;
    }

    let Some(tree) = parser.parse(&input.source, None) else {
        facts
            .warnings
            .push("parser returned no tree; file skipped".to_string());
        return facts;
    };
    // Error-tolerant like the code and doc paths: a malformed config file is
    // partially extracted, the run never aborted ([FR-IX-04]).
    if tree.root_node().has_error() {
        facts.partial = true;
        facts
            .warnings
            .push("syntax error(s) present; partial extraction".to_string());
    }

    let segments: Vec<&str> = path_segments(&input.path);

    // The ConfigFile root: the file path rendered as a namespace (empty scope
    // chain), built first so it is the Contains parent of every top-level section.
    let config_file_symbol = match build_symbol(ctx, &segments, &[]) {
        Ok(sym) => sym,
        Err(err) => {
            facts
                .warnings
                .push(format!("could not build the config-file symbol: {err}"));
            return facts;
        }
    };
    facts.nodes.push(NodeFact {
        symbol: config_file_symbol.clone(),
        kind: NodeKind::ConfigFile,
        name: config_file_name(&segments),
        start_line: 1,
        end_line: input.source.lines().count().max(1) as u32,
        metrics: None,
        exported: false,
        fingerprint: None,
        test_evidence: false,
        // The searchable section keys live on the ConfigSection nodes; the file
        // node carries no body of its own.
        body: None,
        // Config nodes are not functions — no CR-005 structural facts.
        max_nesting_depth: None,
        shingles: Vec::new(),
    });

    // The generic, depth-bounded section walk runs only when the descriptor
    // declares the section node kinds; a typed-anchor-only format (no `[config]`
    // table) yields just the ConfigFile root from this generic step.
    if let Some(cfg) = plugin.config_extraction() {
        let walk = ConfigWalk {
            ctx,
            segments: &segments,
            source: input.source.as_bytes(),
            cfg,
            // The section-walk emit kind: the descriptor's `node_kind` override
            // (S-064 build formats) or `ConfigSection` for a generic data-format
            // walk. The anchor walk ignores this (each anchor carries its own kind).
            node_kind: cfg.node_kind.unwrap_or(NodeKind::ConfigSection),
        };
        // The generic, depth-bounded section walk runs only when the descriptor
        // declares the section node kinds; a typed-anchor-only format yields no
        // generic sections here.
        if !cfg.section_kinds.is_empty() {
            walk_sections(
                tree.root_node(),
                &[],
                &config_file_symbol,
                0,
                &walk,
                &mut facts,
            );
        }

        // The generic, descriptor-driven typed-anchor walk (S-065, [FR-CG-03]):
        // a typed-anchor format (Protobuf, GraphQL, …) declares `[[config.anchors]]`
        // mapping tree-sitter node kinds to config node kinds, and the anchors are
        // emitted here over the same `ConfigFile` root — `Contains`-only, no
        // reference edges ([CR-010] scope; reference edges are [CR-011]). A format
        // can declare both sections and anchors; both walks run independently.
        if !cfg.anchors.is_empty() {
            walk_anchors(&walk, tree.root_node(), &config_file_symbol, &mut facts);
        }
    }

    // Per-format typed anchors (S-066, CR-010, FR-CG-03): a typed-anchor format
    // (Terraform → TfBlock, SQL → SqlObject, …) layers its structural anchors over
    // the same ConfigFile root via its own walk. Additive to the generic step
    // above and a no-op for a format with no recognised typed-anchor walk.
    anchors::extract_typed_anchors(
        plugin,
        tree.root_node(),
        &segments,
        &config_file_symbol,
        input.source.as_bytes(),
        ctx,
        &mut facts,
    );

    // Content-sniffed profile promotion (S-067, [CR-010], [FR-CG-03], [NFR-RA-06]):
    // an additive pass over the already-parsed tree that, for a recognised content
    // profile (OpenAPI: a top-level version-bearing `openapi:`/`swagger:` key),
    // tags the `ConfigFile` and emits `ApiPath`/`ApiOperation` typed nodes over the
    // same root. Purely additive — it never alters the generic `ConfigSection`
    // extraction above — and a no-op for any document that does not sniff. The hook
    // is extensible: a future profile is one entry in `profiles`, claiming none here.
    profiles::promote(
        tree.root_node(),
        &segments,
        &config_file_symbol,
        input.source.as_bytes(),
        ctx,
        &mut facts,
    );

    // Cross-artifact reference capture (S-068, CR-011, [FR-CG-07], [ADR-26]):
    // after every structural anchor is emitted, capture the references between
    // artifacts and to code so the resolution pass can bind them into
    // `ArtifactRef`/`ArtifactBinding` edges. The dispatch is per-format (each
    // consumer story adds its arm); externals are classified out before the
    // ledger inside the shared push helper. A no-op until a consumer arm lands.
    refs::capture_artifact_refs(
        plugin,
        tree.root_node(),
        &segments,
        &config_file_symbol,
        input.source.as_bytes(),
        ctx,
        &mut facts,
    );

    // Canonical output ordering ([NFR-RA-06]) — the shared sort the code/doc
    // paths also apply, so config facts are byte-stable regardless of traversal.
    super::sort_facts(&mut facts);
    facts
}

/// The immutable per-file context threaded through the section walk.
struct ConfigWalk<'a> {
    ctx: &'a SymbolContext,
    segments: &'a [&'a str],
    source: &'a [u8],
    cfg: &'a ConfigDescriptor,
    /// The node kind every matched section is emitted as — the descriptor's
    /// `node_kind` override, or [`NodeKind::ConfigSection`] for a generic
    /// data-format walk. Resolved once so a build/schema/infra format emits its
    /// typed anchor (`DockerfileStage`, `MakeTarget`, …) through this same walk
    /// as pure plugin data (S-064+, [FR-CG-03]).
    node_kind: NodeKind,
}

/// Materialise the **shallowest** section descendants of `container` as
/// `ConfigSection` nodes under `parent_symbol`, then recurse — bounded at
/// [`DEPTH_BOUND`].
///
/// `depth` is the depth of `parent_symbol` (0 for the `ConfigFile` root); the
/// sections found here are at `depth + 1`. `parent_chain` is the pre-rendered
/// descriptor chain of the enclosing sections (outermost-first). The per-call
/// `ordinals` map disambiguates sibling sections that slugify to the same value,
/// in document order ([ADR-07]).
fn walk_sections(
    container: Node<'_>,
    parent_chain: &[String],
    parent_symbol: &LogosSymbol,
    depth: usize,
    walk: &ConfigWalk<'_>,
    facts: &mut Facts,
) {
    let mut ordinals: HashMap<String, u32> = HashMap::new();
    let mut sections = Vec::new();
    collect_shallowest_sections(container, &walk.cfg.section_kinds, &mut sections);

    for sect in sections {
        let key = section_key(sect, walk.cfg.key_field.as_deref(), walk.source);
        let slug = anchor_slug(&key);
        let ordinal = next_ordinal(&mut ordinals, &slug);

        let mut chain = parent_chain.to_vec();
        chain.push(descriptor_for(walk.node_kind, &slug, ordinal));

        let symbol = match build_symbol(walk.ctx, walk.segments, &chain) {
            Ok(sym) => sym,
            Err(err) => {
                facts.warnings.push(format!(
                    "could not build config-section symbol for key '{key}': {err}"
                ));
                continue;
            }
        };

        facts.nodes.push(NodeFact {
            symbol: symbol.clone(),
            // The human-facing, FTS-indexed name is the raw key (FR-CG-04); the
            // slug lives only in the identity. The kind is the descriptor's
            // typed-anchor override, defaulting to `ConfigSection`.
            kind: walk.node_kind,
            name: key.clone(),
            start_line: sect.start_position().row as u32 + 1,
            end_line: sect.end_position().row as u32 + 1,
            metrics: None,
            exported: false,
            fingerprint: None,
            test_evidence: false,
            body: None,
            max_nesting_depth: None,
            shingles: Vec::new(),
        });
        facts.edges.push(EdgeFact {
            source: parent_symbol.clone(),
            target: symbol.clone(),
            kind: EdgeKind::Contains,
        });

        // Recurse for nested sections only while staying within the fixed bound:
        // a section at depth `depth + 1` is descended only if `depth + 1` is still
        // below DEPTH_BOUND, so the deepest emitted section is at DEPTH_BOUND.
        //
        // For the build-format typed-anchor descriptors (S-064: Dockerfile
        // `from_instruction`, Makefile `rule`, Shell `function_definition`) the
        // matched `section_kinds` node never contains another node of the same
        // kind, so this recursive call finds nothing and emits no nested anchors —
        // these formats are flat by construction. The recursion is meaningful only
        // for genuinely nested data formats (YAML/JSON/TOML mappings). A future
        // grammar whose `section_kinds` node *can* self-nest would emit depth-2
        // anchors of the same `node_kind` here; that is the intended, bounded
        // behaviour, not a bug.
        if depth + 1 < DEPTH_BOUND {
            walk_sections(sect, &chain, &symbol, depth + 1, walk, facts);
        }
    }
}

/// Collect the **nearest** descendant nodes of `container` whose kind is in
/// `section_kinds` — a matched node is included and **not** descended into (its
/// own nested sections are found by the recursive [`walk_sections`] call on it),
/// so each section is attributed to exactly one depth. Document (tree) order is
/// preserved, which is the canonical sort for sections.
fn collect_shallowest_sections<'tree>(
    container: Node<'tree>,
    section_kinds: &[String],
    out: &mut Vec<Node<'tree>>,
) {
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        if section_kinds.iter().any(|k| k == child.kind()) {
            out.push(child);
        } else {
            collect_shallowest_sections(child, section_kinds, out);
        }
    }
}

/// The section's key text: the `key_field` child's text when the descriptor
/// names one and the field is present, else the section node's first source
/// line — so a key is always derivable, never fabricated ([NFR-RA-05]).
fn section_key(section: Node<'_>, key_field: Option<&str>, source: &[u8]) -> String {
    if let Some(field) = key_field {
        if let Some(key_node) = section.child_by_field_name(field) {
            if let Ok(text) = key_node.utf8_text(source) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    // Fallback: the section node's own first non-empty source line.
    first_source_line(section, source)
}

/// A node's first non-empty, trimmed source line — the shared, never-fabricating
/// fallback for both the section key ([`section_key`]) and the anchor name
/// ([`anchor_name`]) when a descriptor-named key/name child is absent
/// ([NFR-RA-05]). Returns `""` only for an empty/whitespace node, which the
/// callers slugify ([`anchor_slug`]) to a stable, non-empty fallback.
fn first_source_line(node: Node<'_>, source: &[u8]) -> String {
    node.utf8_text(source)
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// The human-facing name of a `ConfigFile`: the file's basename, or `config` for
/// a pathological empty path.
fn config_file_name(segments: &[&str]) -> String {
    segments
        .last()
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| "config".to_string())
}

/// The stable anchor slug of a section key — the documentation layer's
/// [`heading_slug`](super::doc::heading_slug), reused verbatim so config
/// identity is deterministic ([NFR-RA-06]) and shares the single [ADR-07]
/// slug construction (a GitHub-style lowercased fragment, each run of
/// non-alphanumeric characters collapsed to a single `-`, with a stable
/// non-empty fallback so a punctuation-only key still slugifies).
fn anchor_slug(key: &str) -> String {
    super::doc::heading_slug(key)
}

/// Emit the descriptor's **typed anchors** (S-065, [CR-010], [FR-CG-03]) as
/// config nodes hung off the `ConfigFile` root by `Contains`.
///
/// The walk is grammar-agnostic — exactly the declarative pattern of the section
/// walk: every tree-sitter node whose kind matches an [`AnchorDescriptor::node_kind`]
/// becomes a config node of the mapped [`NodeKind`], named from its
/// [`AnchorDescriptor::name_child`] child (or first source line, never fabricated),
/// carrying any [`AnchorDescriptor::payload`] subtype in the FTS-indexed `body`.
/// Matched nodes are collected across the whole tree (so a nested Protobuf
/// `message` is found too) and assigned per-slug sibling ordinals in **start-byte
/// order**, so identities are deterministic regardless of traversal ([NFR-RA-06])
/// and two same-named anchors never collide. The layer is `Contains`-only: no
/// import/reference edges are produced ([CR-010] scope rule).
///
/// [CR-010]: ../../../docs/requests/CR-010-config-artifact-graph-layer.md
/// [FR-CG-03]: ../../../docs/specs/requirements/FR-CG-03.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
fn walk_anchors(
    walk: &ConfigWalk<'_>,
    root: Node<'_>,
    config_file_symbol: &LogosSymbol,
    facts: &mut Facts,
) {
    for anchor in resolve_anchor_identities(walk.cfg, walk.ctx, walk.segments, walk.source, root) {
        facts.nodes.push(NodeFact {
            symbol: anchor.symbol.clone(),
            // The human-facing, FTS-indexed name is the raw declared name (or the
            // first-source-line fallback); the slug lives only in the identity.
            kind: anchor.kind,
            name: anchor.name,
            start_line: anchor.node.start_position().row as u32 + 1,
            end_line: anchor.node.end_position().row as u32 + 1,
            metrics: None,
            exported: false,
            fingerprint: None,
            test_evidence: false,
            // Payload subtype in FTS-indexed `body` (FR-CG-03): None for a
            // payloadless format (Protobuf); Some("object") … for GraphQL.
            body: anchor.payload,
            max_nesting_depth: None,
            shingles: Vec::new(),
        });
        facts.edges.push(EdgeFact {
            source: config_file_symbol.clone(),
            target: anchor.symbol,
            kind: EdgeKind::Contains,
        });
    }
}

/// One typed anchor resolved to the identity [`walk_anchors`] assigns it: the
/// tree node (for line spans and sub-walks), its emitted [`NodeKind`], the
/// FTS-indexed declared name, the optional payload subtype, and the
/// `path#anchor` [`LogosSymbol`].
///
/// [`resolve_anchor_identities`] is the **single source of truth** for anchor
/// identity, shared by [`walk_anchors`] (which emits the node/edge) and the
/// cross-artifact reference-capture arms ([`refs`], CR-011 S-070/S-071) that
/// source a reference *from* an anchor (a proto message field → its message, a
/// schema type → its code symbol). Because both read the same construction, a
/// reference's source symbol is byte-identical to the emitted anchor node's,
/// with no risk of the two walks drifting ([NFR-RA-06]).
pub(super) struct ResolvedAnchor<'tree> {
    pub(super) node: Node<'tree>,
    pub(super) kind: NodeKind,
    pub(super) name: String,
    pub(super) payload: Option<String>,
    pub(super) symbol: LogosSymbol,
}

/// Resolve every typed anchor the descriptor declares to its [`ResolvedAnchor`]
/// identity, in the exact start-byte order and per-slug ordinal assignment
/// [`walk_anchors`] emits.
///
/// Ordinals are keyed by **slug alone**, not (kind, slug): [`descriptor_for`]
/// maps every anchor kind to the same `#` type-descriptor, so a `message Foo`
/// and a `service Foo` would render the identical `foo#` symbol — the shared
/// counter is exactly what disambiguates them (`foo#`, `foo#1:`). Keying by
/// (kind, slug) would reintroduce that collision ([NFR-RA-06]).
///
/// Pure: it mutates no [`Facts`]. An anchor whose descriptor `kind` is unknown
/// (descriptor validation already rejects this) or whose symbol fails to build
/// (a genuinely malformed name) is skipped — it cannot be emitted, so it has no
/// identity to source a reference from either ([NFR-RA-05], never fabricate).
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
/// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
pub(super) fn resolve_anchor_identities<'tree>(
    cfg: &ConfigDescriptor,
    ctx: &SymbolContext,
    segments: &[&str],
    source: &[u8],
    root: Node<'tree>,
) -> Vec<ResolvedAnchor<'tree>> {
    // node-kind → descriptor, first-declared wins on a duplicate node_kind
    // (descriptor validation already rejects duplicate `node_kind`s).
    let mut lookup: HashMap<&str, &AnchorDescriptor> = HashMap::new();
    for desc in &cfg.anchors {
        lookup.entry(desc.node_kind.as_str()).or_insert(desc);
    }

    let mut found: Vec<(Node<'tree>, &AnchorDescriptor)> = Vec::new();
    collect_anchor_nodes(root, &lookup, &mut found);
    // Deterministic ordinal assignment independent of traversal order.
    found.sort_by_key(|(node, _)| node.start_byte());

    let mut ordinals: HashMap<String, u32> = HashMap::new();
    let mut anchors: Vec<ResolvedAnchor<'tree>> = Vec::with_capacity(found.len());
    for (node, desc) in found {
        // `kind` is validated to a config kind at descriptor parse; defensively
        // skip rather than fabricate if that ever drifts ([FR-IX-04]).
        let Some(kind) = NodeKind::from_wire(&desc.kind) else {
            continue;
        };
        let name = anchor_name(node, desc.name_child.as_deref(), source);
        let slug = anchor_slug(&name);
        let ordinal = next_ordinal(&mut ordinals, &slug);
        let chain = [descriptor_for(kind, &slug, ordinal)];
        let Ok(symbol) = build_symbol(ctx, segments, &chain) else {
            continue;
        };
        anchors.push(ResolvedAnchor {
            node,
            kind,
            name,
            payload: desc.payload.clone(),
            symbol,
        });
    }
    anchors
}

/// Collect every named descendant of `node` whose kind matches an anchor — a
/// matched node is recorded **and still descended into**, so nested anchors (a
/// Protobuf `message` inside a `message`) are found too. Order is irrelevant: the
/// caller re-sorts by start byte for deterministic ordinals.
fn collect_anchor_nodes<'tree, 'a>(
    node: Node<'tree>,
    lookup: &HashMap<&str, &'a AnchorDescriptor>,
    out: &mut Vec<(Node<'tree>, &'a AnchorDescriptor)>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(desc) = lookup.get(child.kind()) {
            out.push((child, *desc));
        }
        collect_anchor_nodes(child, lookup, out);
    }
}

/// The anchor's declared name: the text of the first direct named child of kind
/// `name_child`, else the node's first non-empty source line — so a name is
/// always derivable, never fabricated ([NFR-RA-05]). The fallback mirrors
/// [`section_key`].
fn anchor_name(node: Node<'_>, name_child: Option<&str>, source: &[u8]) -> String {
    if let Some(child_kind) = name_child {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == child_kind {
                if let Ok(text) = child.utf8_text(source) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        return trimmed.to_string();
                    }
                }
                break;
            }
        }
    }
    first_source_line(node, source)
}

#[cfg(all(test, feature = "lang-markdown"))]
mod tests;

#[cfg(all(test, feature = "lang-protobuf"))]
mod proto_tests;

#[cfg(all(test, feature = "lang-graphql"))]
mod graphql_tests;
