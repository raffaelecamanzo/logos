//! Structural documentation extraction (S-033, [CR-003], [ADR-19]).
//!
//! Where the code path ([`super::extract_one`]) runs a `symbols` tagging query
//! and folds SCIP descriptors into stable IDs, a *documentation* file is
//! extracted **structurally**: the `tree-sitter-md` block grammar already nests
//! its `section` nodes by heading depth, so the heading hierarchy maps directly
//! onto a [`NodeKind::DocFile`] root and a [`EdgeKind::Contains`] tree of
//! [`NodeKind::DocSection`] nodes ([FR-DG-02]).
//!
//! # Identity — `path#heading-slug`, reusing [ADR-07]
//!
//! Documentation uses a **parallel** identity scheme, not SCIP module paths
//! ([FR-DG-02]). A `DocFile`'s symbol is the file path rendered as a namespace
//! ([`build_symbol`] with an empty scope chain); a `DocSection`'s symbol appends
//! one type-descriptor per enclosing heading slug, so it renders as the
//! `…/path.md/heading-slug#` anchor form. Two sibling headings that slugify to
//! the same value are disambiguated by **ordinal-within-parent** in document
//! order — the exact [ADR-07] construction the code path uses
//! ([`descriptor_for`] folds the ordinal in). Because a slug depends only on its
//! own heading text and an ordinal only on same-slug siblings, renaming one
//! heading never churns another section's id ([FR-DG-02], [NFR-RA-06]).
//!
//! The result is still a canonical [`LogosSymbol`], so doc nodes ride the shared
//! `symbols`/`nodes` tables, FTS, and sync with no parallel store ([ADR-19]).
//!
//! # References — the raw material for doc→doc / doc→code resolution (S-035)
//!
//! Alongside the section tree, this module emits the file's outgoing
//! [`RefFact`]s ([`EdgeKind::DocReference`]), the doc analogue of the code
//! `references` query ([`super::refs`]):
//!
//! - **markdown links** (`[text](dest)`) → a [`RefForm::Path`] ref whose target
//!   is the link's destination (path + optional `#anchor`), bound later to the
//!   target [`NodeKind::DocFile`]/[`NodeKind::DocSection`] — or, for a link to a
//!   code file, that file's module node ([FR-DG-03]);
//! - **inline-code / single-token fenced spans** → a [`RefForm::Method`] ref (a
//!   bare code name) or a [`RefForm::Path`] ref (an explicit repo file-path),
//!   bound later to the one code symbol it names ([FR-DG-04]).
//!
//! Extraction never *binds* anything and never guesses: prose is not a
//! candidate (only links and code spans are), and the [resolution-engine] binds
//! a ref only on an exactly-one-candidate match — ambiguous or unknown mentions
//! persist in `unresolved_refs` and are retried each sync ([NFR-RA-05], the
//! never-fabricate invariant). The inline content lives in the markdown *inline*
//! grammar (`tree_sitter_md::INLINE_LANGUAGE`), parsed here per-`inline`-block,
//! since the block grammar treats inline text as an opaque leaf.
//!
//! [CR-003]: ../../../docs/requests/CR-003-documentation-graph-layer.md
//! [ADR-19]: ../../../docs/specs/architecture/decisions/ADR-19.md
//! [ADR-07]: ../../../docs/specs/architecture/decisions/ADR-07.md
//! [FR-DG-02]: ../../../docs/specs/requirements/FR-DG-02.md
//! [FR-DG-03]: ../../../docs/specs/requirements/FR-DG-03.md
//! [FR-DG-04]: ../../../docs/specs/requirements/FR-DG-04.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
//! [resolution-engine]: ../../../docs/specs/architecture/components/resolution-engine.md

use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use crate::model::{EdgeKind, LogosSymbol, NodeKind, RefForm};
use crate::plugin::LanguagePlugin;

use super::symbol::{build_symbol, descriptor_for, path_segments};
use super::{EdgeFact, Facts, FileInput, NodeFact, RefFact, SymbolContext};

/// The fallback slug for a heading whose text slugifies to the empty string
/// (e.g. a heading made only of punctuation), so its identity stays a valid,
/// stable, non-empty descriptor.
const EMPTY_SLUG: &str = "section";

/// Extract one documentation file with an explicit plugin, allocating a fresh
/// parser. The documentation analogue of [`super::extract`].
pub fn extract_doc(input: &FileInput, plugin: &dyn LanguagePlugin, ctx: &SymbolContext) -> Facts {
    let mut parser = Parser::new();
    extract_one_doc(&mut parser, input, plugin, ctx)
}

/// Core single-file documentation extraction, reusing the caller's parser.
///
/// Routed here from [`super::extract_one`] when `plugin.is_documentation()` is
/// true, so it shares the per-rayon-worker parser of the parallel driver.
pub(super) fn extract_one_doc(
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
    // Markdown rarely fails to parse, but record partial extraction honestly if
    // the block grammar reports an error (same posture as the code path).
    if tree.root_node().has_error() {
        facts.partial = true;
        facts
            .warnings
            .push("syntax error(s) present; partial extraction".to_string());
    }

    let source = input.source.as_bytes();
    let segments: Vec<&str> = path_segments(&input.path);

    // The DocFile root: the file path rendered as a namespace (empty scope
    // chain). Built first so it is the Contains parent of every top-level
    // heading section.
    let doc_file_symbol = match build_symbol(ctx, &segments, &[]) {
        Ok(sym) => sym,
        Err(err) => {
            facts
                .warnings
                .push(format!("could not build the doc-file symbol: {err}"));
            return facts;
        }
    };
    facts.nodes.push(NodeFact {
        symbol: doc_file_symbol.clone(),
        kind: NodeKind::DocFile,
        name: doc_file_name(&segments),
        start_line: 1,
        end_line: input.source.lines().count().max(1) as u32,
        metrics: None,
        exported: false,
        fingerprint: None,
        test_evidence: false,
        // The DocFile is the structural root; its searchable prose lives in the
        // DocSection nodes beneath it (FR-DG-05), so the file node has no body.
        body: None,
        // Documentation nodes are not functions — no CR-005 structural facts.
        max_nesting_depth: None,
        shingles: Vec::new(),
    });

    // A parser bound to the markdown *inline* grammar, for the per-`inline`-block
    // re-parse that surfaces links and code spans (S-035). `None` when the inline
    // grammar is unavailable (no `lang-markdown`) — then only the section tree is
    // produced, never a bogus ref.
    let mut inline = make_inline_parser();

    // Walk the block grammar's nested `section` tree, materialising one
    // DocSection per heading and a Contains edge from its parent, and collecting
    // each section's outgoing doc references (S-035).
    let walk = DocWalk {
        ctx,
        segments: &segments,
        source,
    };
    walk_sections(
        tree.root_node(),
        &[],
        &doc_file_symbol,
        &walk,
        &mut inline,
        &mut facts,
    );

    // Canonical output ordering (NFR-RA-06) — the shared sort the code path also
    // applies, so the doc facts are byte-stable regardless of traversal order.
    super::sort_facts(&mut facts);
    super::dedup_sort_refs(&mut facts.refs);
    facts
}

/// A [`Parser`] bound to the markdown *inline* grammar, or `None` when the
/// `lang-markdown` grammar is not linked.
///
/// The block grammar (the doc plugin's `language()`) parses headings and
/// paragraphs but leaves inline content — links, code spans — as opaque
/// `inline` leaves; the inline grammar parses those leaves' text. The two are
/// separate `tree_sitter_md` grammars, so reference extraction needs this
/// second parser.
#[cfg(feature = "lang-markdown")]
fn make_inline_parser() -> Option<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_md::INLINE_LANGUAGE.into())
        .ok()?;
    Some(parser)
}

/// Without the markdown grammar there is no inline parser — and no doc file
/// reaches this module in the first place.
#[cfg(not(feature = "lang-markdown"))]
fn make_inline_parser() -> Option<Parser> {
    None
}

/// The immutable per-file context threaded through the section walk: the symbol
/// builder's [`SymbolContext`], the file's path segments, and the raw source
/// bytes. Bundled so [`walk_sections`] stays within a sane argument count.
struct DocWalk<'a> {
    ctx: &'a SymbolContext,
    segments: &'a [&'a str],
    source: &'a [u8],
}

/// Recursively materialise the `section` children of `container` (a `document`
/// or a `section`) as `DocSection` nodes under `parent_symbol`.
///
/// `parent_chain` is the pre-rendered descriptor chain of the enclosing heading
/// sections (outermost-first). The per-call `ordinals` map disambiguates sibling
/// sections that slugify to the same value, in document order ([ADR-07]).
fn walk_sections(
    container: Node<'_>,
    parent_chain: &[String],
    parent_symbol: &LogosSymbol,
    walk: &DocWalk<'_>,
    inline: &mut Option<Parser>,
    facts: &mut Facts,
) {
    let DocWalk {
        ctx,
        segments,
        source,
    } = *walk;
    // Sibling slug → next ordinal, scoped to THIS parent (ADR-07: ordinal within
    // parent scope). Document order is the canonical sort for sections, which the
    // grammar's child order already gives us.
    let mut ordinals: HashMap<String, u32> = HashMap::new();
    let mut cursor = container.walk();
    for child in container.named_children(&mut cursor) {
        if child.kind() != "section" {
            continue;
        }
        let Some(heading_text) = heading_text_of(child, source) else {
            // A section with no heading is the leading preamble (content before
            // the first heading); it holds no sub-sections, but its links and
            // code spans are real references attributed to the enclosing
            // file/section (S-035).
            collect_section_refs(child, parent_symbol, source, inline, facts);
            continue;
        };

        let slug = heading_slug(&heading_text);
        let ordinal = *ordinals.get(&slug).unwrap_or(&0);
        *ordinals.entry(slug.clone()).or_insert(0) += 1;

        let mut chain = parent_chain.to_vec();
        chain.push(descriptor_for(NodeKind::DocSection, &slug, ordinal));

        let symbol = match build_symbol(ctx, segments, &chain) {
            Ok(sym) => sym,
            Err(err) => {
                facts.warnings.push(format!(
                    "could not build doc-section symbol for heading '{heading_text}': {err}"
                ));
                continue;
            }
        };

        // The section's own prose (paragraphs, lists, code) beneath the heading,
        // excluding nested sub-sections — those are body-indexed under their own
        // DocSection. Empty → None, so a heading-only section carries no body.
        let body = section_body_text(child, source);

        facts.nodes.push(NodeFact {
            symbol: symbol.clone(),
            kind: NodeKind::DocSection,
            // The raw heading text is the FTS-indexed, human-facing name; the
            // slug lives only in the identity.
            name: heading_text.trim().to_string(),
            start_line: child.start_position().row as u32 + 1,
            end_line: child.end_position().row as u32 + 1,
            metrics: None,
            exported: false,
            fingerprint: None,
            test_evidence: false,
            // The FTS-indexed body (FR-DG-05): a phrase appearing only here is
            // findable through `search` even when the heading does not name it.
            body,
            // Documentation nodes are not functions — no CR-005 structural facts.
            max_nesting_depth: None,
            shingles: Vec::new(),
        });
        facts.edges.push(EdgeFact {
            source: parent_symbol.clone(),
            target: symbol.clone(),
            kind: EdgeKind::Contains,
        });

        // This section's own links/code spans (its direct content, not its
        // sub-sections') are references attributed to it (S-035).
        collect_section_refs(child, &symbol, source, inline, facts);

        // Recurse: deeper headings nest as `section` children of this section.
        walk_sections(child, &chain, &symbol, walk, inline, facts);
    }
}

/// The heading text of a `section`, or `None` when the section carries no
/// heading (a preamble). Reads the `heading_content` field of the section's
/// first `atx_heading`/`setext_heading` child — the inline text without the
/// `#`/underline markers.
fn heading_text_of(section: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = section.walk();
    let heading = section
        .named_children(&mut cursor)
        .find(|c| matches!(c.kind(), "atx_heading" | "setext_heading"))?;
    let content = heading.child_by_field_name("heading_content")?;
    let text = content.utf8_text(source).ok()?;
    Some(text.trim().to_string())
}

/// The FTS-indexed body of a `DocSection` ([FR-DG-05], S-037): the raw source
/// text of the section's **own** direct block children — paragraphs, lists,
/// tables, code blocks — joined by blank lines, excluding the heading itself and
/// any nested sub-sections (those are body-indexed under their own
/// [`NodeKind::DocSection`], mirroring the [`collect_section_refs`] boundary).
///
/// Returns `None` when the section has no prose of its own (a heading-only
/// scaffold section), so the `nodes.body` column stays `NULL` rather than
/// holding an empty string — an honest "no searchable body here".
fn section_body_text(section: Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = section.walk();
    let mut parts: Vec<&str> = Vec::new();
    for child in section.named_children(&mut cursor) {
        match child.kind() {
            // A sub-section's prose belongs to the sub-section, not here.
            "section" => continue,
            // The heading is the section's `name`, not its body.
            "atx_heading" | "setext_heading" => continue,
            _ => {
                if let Ok(text) = child.utf8_text(source) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed);
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// The human-facing name of a `DocFile`: the file's basename (the last path
/// segment), or `document` for a pathological empty path.
fn doc_file_name(segments: &[&str]) -> String {
    segments
        .last()
        .map(|s| (*s).to_string())
        .unwrap_or_else(|| "document".to_string())
}

/// The stable anchor slug of a heading: [`slugify`] with the [`EMPTY_SLUG`]
/// fallback applied, so a punctuation-only heading still has a non-empty slug.
///
/// The single source of truth for the `#heading-slug` half of doc identity: the
/// section's own id uses it (via [`walk_sections`]) and the resolver matches a
/// link's `#anchor` against it ([`crate::resolve`], S-035), so a link and its
/// target agree by construction.
pub(crate) fn heading_slug(heading: &str) -> String {
    let slug = slugify(heading);
    if slug.is_empty() {
        EMPTY_SLUG.to_string()
    } else {
        slug
    }
}

/// Recognised source-file extensions for an *explicit repo file-path* code
/// reference (FR-DG-04) — a code-span or link target like
/// `logos-core/src/lib.rs`. A path ending in one of these is admitted as a
/// path-form [`EdgeKind::DocReference`]; the resolver binds it to the file's
/// node (or leaves it unresolved). `md`/`markdown` are included so a path to
/// another doc resolves to its [`NodeKind::DocFile`].
const REPO_PATH_EXTENSIONS: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "mjs", "cjs", "go", "java", "md", "markdown",
];

/// Collect a heading section's *own* outgoing references (S-035): the links and
/// code spans in its direct content, not in its sub-sections (those are walked
/// separately and attributed to their own [`NodeKind::DocSection`]).
///
/// Iterates the section's direct block children, skipping the heading itself and
/// nested `section`s, and descends each into [`collect_block_refs`].
fn collect_section_refs(
    section: Node<'_>,
    source_symbol: &LogosSymbol,
    source: &[u8],
    inline: &mut Option<Parser>,
    facts: &mut Facts,
) {
    let mut cursor = section.walk();
    for child in section.named_children(&mut cursor) {
        match child.kind() {
            // A sub-section's content belongs to the sub-section, not here.
            "section" => continue,
            // The heading text is the section's name, not a reference.
            "atx_heading" | "setext_heading" => continue,
            _ => collect_block_refs(child, source, source_symbol, inline, facts),
        }
    }
}

/// Recursively scan a block subtree for reference-bearing leaves — `inline`
/// runs (links, code spans) and code blocks — attributing each to
/// `source_symbol`. Stops at a nested `section` so a reference is never
/// attributed across a heading boundary.
fn collect_block_refs(
    node: Node<'_>,
    source: &[u8],
    source_symbol: &LogosSymbol,
    inline: &mut Option<Parser>,
    facts: &mut Facts,
) {
    match node.kind() {
        // Defensive: block children of a section never contain a nested section,
        // but never cross the boundary even if a future grammar nests one.
        "section" => {}
        "inline" => collect_inline_refs(node, source, source_symbol, inline, facts),
        "fenced_code_block" | "indented_code_block" => {
            collect_code_block_ref(node, source, source_symbol, facts);
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_block_refs(child, source, source_symbol, inline, facts);
            }
        }
    }
}

/// `true` for the inline node kinds whose subtree is link/image *anchor text*
/// (display styling), not freestanding content. Their inner code spans are not
/// code references, so the walk never descends into them ([FR-DG-04]).
fn is_link_container(kind: &str) -> bool {
    matches!(
        kind,
        "inline_link"
            | "image"
            | "full_reference_link"
            | "collapsed_reference_link"
            | "shortcut_link"
    )
}

/// Re-parse one `inline` block's text with the inline grammar and emit a ref for
/// every markdown link (`[text](dest)`) and code span (`` `token` ``) it
/// contains. A no-op when the inline parser is unavailable.
fn collect_inline_refs(
    inline_node: Node<'_>,
    source: &[u8],
    source_symbol: &LogosSymbol,
    inline: &mut Option<Parser>,
    facts: &mut Facts,
) {
    let Some(parser) = inline.as_mut() else {
        return;
    };
    let Ok(text) = inline_node.utf8_text(source) else {
        return;
    };
    let Some(tree) = parser.parse(text, None) else {
        return;
    };
    // Inner node positions are relative to the inline text; offset by the block's
    // row to recover the source line (metadata only — refs sort by content).
    let base_row = inline_node.start_position().row as u32;
    let bytes = text.as_bytes();
    let mut stack = vec![tree.root_node()];
    while let Some(n) = stack.pop() {
        // A link/image container's text is display styling, not a freestanding
        // code mention: emit the link's destination, but do NOT descend into its
        // text — a code span inside `[`foo`](bar.md)` is link styling, not a
        // `foo` reference the author wrote ([FR-DG-04], never fabricate a
        // candidacy).
        if is_link_container(n.kind()) {
            if n.kind() == "inline_link" {
                if let Some(dest) = n
                    .named_children(&mut n.walk())
                    .find(|c| c.kind() == "link_destination")
                {
                    if let Ok(href) = dest.utf8_text(bytes) {
                        emit_link_ref(
                            href,
                            source_symbol,
                            base_row + n.start_position().row as u32 + 1,
                            facts,
                        );
                    }
                }
            }
            continue; // never descend into link/image anchor text
        }
        if n.kind() == "code_span" {
            if let Ok(span) = n.utf8_text(bytes) {
                emit_code_token_ref(
                    strip_code_delimiters(span),
                    source_symbol,
                    base_row + n.start_position().row as u32 + 1,
                    facts,
                );
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

/// Emit a code reference for a single-token code block (`` ``` `` fence or an
/// indented block whose whole content is one token, e.g. a `symbol_name` on its
/// own line). A multi-line/multi-word code sample is not a single reference and
/// is left alone — [`emit_code_token_ref`] rejects anything with whitespace.
fn collect_code_block_ref(
    block: Node<'_>,
    source: &[u8],
    source_symbol: &LogosSymbol,
    facts: &mut Facts,
) {
    // A fenced block's payload is its `code_fence_content`; an indented block
    // has none, so the node text is the payload.
    let content = block
        .named_children(&mut block.walk())
        .find(|c| c.kind() == "code_fence_content")
        .unwrap_or(block);
    if let Ok(text) = content.utf8_text(source) {
        emit_code_token_ref(
            text,
            source_symbol,
            block.start_position().row as u32 + 1,
            facts,
        );
    }
}

/// Strip the leading/trailing backtick run of a `code_span`'s raw text, leaving
/// the code between the delimiters.
fn strip_code_delimiters(span: &str) -> &str {
    span.trim_matches('`')
}

/// Emit a doc→doc / doc→code *link* reference for a markdown link destination.
///
/// External links (a URL scheme, a protocol-relative `//…`) are skipped — only
/// in-repository links are graph references. The destination is recorded
/// verbatim (path plus any `#anchor`); the resolver splits and resolves it
/// relative to the source doc ([FR-DG-03]).
fn emit_link_ref(dest: &str, source_symbol: &LogosSymbol, line: u32, facts: &mut Facts) {
    let dest = dest.trim();
    if dest.is_empty() || is_external_link(dest) {
        return;
    }
    facts.refs.push(RefFact {
        source: source_symbol.clone(),
        target: dest.to_string(),
        alias: None,
        form: RefForm::Path,
        kind: EdgeKind::DocReference,
        line,
        relation: None,
    });
}

/// `true` for a link destination that points outside the repository: a
/// `scheme://…` URL, a `mailto:`/`tel:` URI, or a protocol-relative `//host/…`.
fn is_external_link(dest: &str) -> bool {
    dest.starts_with("//")
        || dest.contains("://")
        || dest.starts_with("mailto:")
        || dest.starts_with("tel:")
}

/// Emit a doc→code reference for a code token (an inline-code span or a
/// single-token code block), or nothing if the token is not a plausible code
/// reference ([FR-DG-04], conservative by design).
///
/// - a multi-word snippet (any whitespace) is a code *sample*, not a token —
///   skipped;
/// - an explicit repo file-path (`a/b/c.rs`) → a path-form ref, resolved to the
///   file's node;
/// - otherwise the last `::`/`.`-separated segment, when it is a plain
///   identifier, is a name-form ref resolved to the one code symbol it names; a
///   non-identifier (an operator, a flag, a sentence fragment) is skipped so
///   prose-in-backticks never becomes a candidate.
fn emit_code_token_ref(raw: &str, source_symbol: &LogosSymbol, line: u32, facts: &mut Facts) {
    let token = raw.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return;
    }
    // Drop cosmetic call/macro suffixes: `foo()`, `foo!`, `foo!()` all name `foo`.
    let token = token.strip_suffix("()").unwrap_or(token);
    let token = token.strip_suffix('!').unwrap_or(token);
    if token.is_empty() {
        return;
    }

    // A file-path-shaped token (`a/b/c.rs`, or a bare `README.md`) is a
    // path-form reference resolved to the file's node. Checked before the name
    // branch so a filename's extension is never mistaken for a bare name.
    if is_repo_path(token) {
        facts.refs.push(RefFact {
            source: source_symbol.clone(),
            target: token.to_string(),
            alias: None,
            form: RefForm::Path,
            kind: EdgeKind::DocReference,
            line,
            relation: None,
        });
        return;
    }
    // A non-path token containing `/` (e.g. `net/http`) is not a code reference.
    if token.contains('/') {
        return;
    }

    // A name token: bind by the last path segment's bare name (`Vec::new` →
    // `new`, `mod.thing` → `thing`, bare `run` → `run`).
    let after_colons = token.rsplit("::").next().unwrap_or(token);
    let name = after_colons.rsplit('.').next().unwrap_or(after_colons);
    if is_plain_ident(name) {
        facts.refs.push(RefFact {
            source: source_symbol.clone(),
            target: name.to_string(),
            alias: None,
            form: RefForm::Method,
            kind: EdgeKind::DocReference,
            line,
            relation: None,
        });
    }
}

/// `true` if `token` looks like an explicit repository file-path: it contains a
/// path separator and its basename carries a recognised source extension.
fn is_repo_path(token: &str) -> bool {
    let base = token.rsplit('/').next().unwrap_or(token);
    match base.rsplit_once('.') {
        Some((stem, ext)) => !stem.is_empty() && REPO_PATH_EXTENSIONS.contains(&ext),
        None => false,
    }
}

/// `true` if `name` is a plain identifier (`[A-Za-z_][A-Za-z0-9_]*`) — the only
/// shape admitted as a doc→code name reference, so backticked prose, operators,
/// and flags are never link candidates ([FR-DG-04]).
fn is_plain_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Slugify a heading into a GitHub-style anchor fragment: lowercased, with each
/// run of non-alphanumeric characters collapsed to a single `-`, and leading and
/// trailing `-` trimmed.
///
/// Pure function of the heading text — the determinism the `path#heading-slug`
/// identity rests on ([NFR-RA-06]). `char::is_alphanumeric`/`to_lowercase` are
/// Unicode-aware, so an ASCII heading yields a `[a-z0-9-]` slug that
/// [`super::symbol::escape_name`] emits verbatim, while a non-ASCII heading (e.g.
/// `Café`) yields a slug `escape_name` backtick-escapes — both produce a valid,
/// canonical descriptor (`LogosSymbol::parse` round-trips either).
fn slugify(heading: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in heading.trim().chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(ch.to_lowercase());
        } else {
            // Any non-alphanumeric run (spaces, punctuation, markdown markup)
            // becomes a single separator — but only emitted once a following
            // alphanumeric char confirms it is interior, so trailing runs drop.
            pending_dash = true;
        }
    }
    out
}

/// swe-skills typed-node enrichment — the additive promotion of convention
/// artifacts to typed `Requirement`/`Adr`/`Story` nodes (S-039, [FR-DG-07]).
///
/// [FR-DG-07]: ../../../../docs/specs/requirements/FR-DG-07.md
pub(crate) mod enrich;

#[cfg(all(test, feature = "lang-markdown"))]
mod tests;
