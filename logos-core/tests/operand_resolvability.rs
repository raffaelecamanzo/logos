//! **S-355 — operand resolvability across the composed client-call corpus**
//! ([CR-113] CRA-01, [FR-WS-18], [FR-WS-08], [ADR-54]).
//!
//! [CR-113] proposes admitting a client-call path composed entirely from
//! compile-time constants. Its whole case rests on **CRA-01**: *"a material
//! share of the 124 Java `.uri(…)` sites compose from a constant declared in the
//! same compilation unit"*, recorded as **not validated** with the instruction
//! *"measure this before implementing"*. [S-341] proved zero of those sites are
//! single static literals but never classified the composed ones by operand
//! resolvability. This harness is that classification.
//!
//! # What it measures
//!
//! For every file in a reference workspace whose language ships the
//! `invocations` capability, it runs the **real** compiled grammar, the **real**
//! per-language `invocations.scm`, and the **real** `extract::extract` pass (for
//! the ledger gate and for what the arm emits today), then decomposes each
//! captured path argument into operands and classifies each operand into
//! [CR-113] §8's taxonomy: `literal`, `same-unit constant`, `injected`,
//! `configuration lookup`, `method return`, `other`.
//!
//! A site is **newly admissible under folding** when every operand is a literal
//! or a same-unit constant, the folded template is an absolute path, and the arm
//! does not already admit it.
//!
//! # The measurement is biased *in favour* of CRA-01, deliberately
//!
//! Every judgement call is resolved the way that maximises the newly-admissible
//! count, so a small result is a robust falsification rather than an artefact of
//! a strict reading:
//!
//! - an operand name with several same-file bindings takes its **most**
//!   resolvable one, not its least;
//! - a same-unit name initialised from a string literal counts as a constant
//!   whether or not it is declared `final` / `const` (the strict, const-only
//!   count is reported alongside as [`Measurement::strict_const_admits`]);
//! - resolution recurses through a same-unit binding's initialiser rather than
//!   stopping at the first indirection.
//!
//! # Recorded finding (2026-09-05, `~/source/pec-services`, 90 members)
//!
//! **CRA-01 is FALSIFIED.** Over the corpus the arm actually considers — a
//! verb-anchored query match inside a ledger-gate-admitted file:
//!
//! ```text
//! language     files  gated  sites  emitted  literal  same-u  config  inject  return  other
//! go             262     75     98        0       51       0       7       0       0     40
//! java          2447     26     98        0        0       1      85       0       0     12
//! php            161      0      0        0        0       0       0       0       0      0
//! python          55      1      2        0        0       0       0       0       0      2
//! tsx              5      0      0        0        0       0       0       0       0      0
//! typescript     656     12      1        1        1       0       0       0       0      0
//! ```
//!
//! **Constant folding would newly admit ZERO sites, in every language.** The
//! Java corpus CRA-01 is about resolves 85 of 98 sites to a *configuration
//! lookup* (`someApiProperties.getUriX()` on a cross-unit
//! `@ConfigurationProperties` bean whose value lives in `application.yml`, not
//! in source), 10 more to a `uriBuilder -> …path(<the same getter>)` lambda,
//! 3 to incidental list accesses, and exactly **1** to a same-unit constant —
//! `JSESSIONID_COOKIE_NAME`, a cookie name, not a path.
//!
//! Ignoring the ledger gate entirely raises the ceiling to **3** sites
//! workspace-wide (0.5 % of 606), all `WebTestClient` in-process test calls in
//! two test files — not outbound cross-service coupling at all.
//!
//! See `docs/planning/sprints/sprint-impl-64.md` for the full reasoning and the
//! consequence for [CR-113].
//!
//! # Running it
//!
//! The corpus is not in this repository, so the measurement test **skips** unless
//! `LOGOS_REF_WORKSPACE` points at a checkout:
//!
//! ```text
//! LOGOS_REF_WORKSPACE=~/source/pec-services \
//!   cargo test -p logos-core --test operand_resolvability -- --nocapture
//! ```
//!
//! The classifier's own rules are pinned by fixture tests that always run.
//!
//! [S-341]: ../../docs/planning/journal.md
//! [CR-113]: ../../docs/requests/CR-113-constant-folded-base-url-composition.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-18]: ../../docs/specs/requirements/FR-WS-18.md
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use logos_core::extract::{self, FileInput, SymbolContext};
use logos_core::model::ArtifactRelation;
use logos_core::plugin::{LanguagePlugin, LanguageRegistry};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

// ── The taxonomy ────────────────────────────────────────────────────────────

/// [CR-113] §8's operand taxonomy, ordered **most resolvable first** so
/// `min()` over a set of candidate classifications picks the reading most
/// favourable to CRA-01.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OperandKind {
    /// A string literal present at the call site.
    Literal,
    /// A name bound in the same compilation unit to a string literal — the
    /// operand kind CRA-01 is about.
    SameUnitConstant,
    /// A value read from configuration: `@Value`, a `*Properties` /
    /// `*Configuration*` bean, `process.env`, `os.environ`, `System.getenv`.
    ConfigurationLookup,
    /// A field supplied by dependency injection (declared, never initialised in
    /// the unit).
    Injected,
    /// The return value of a method/function call.
    MethodReturn,
    /// A parameter, an unresolvable name, a lambda, anything else.
    Other,
}

impl OperandKind {
    /// The label used in the reported table.
    fn label(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::SameUnitConstant => "same-unit constant",
            Self::ConfigurationLookup => "configuration lookup",
            Self::Injected => "injected",
            Self::MethodReturn => "method return",
            Self::Other => "other",
        }
    }

    /// Whether folding can resolve this operand from the source alone
    /// ([CR-113] §3.2: a literal, or a constant whose initialiser is a literal
    /// reachable in the same compilation unit).
    fn is_foldable(self) -> bool {
        matches!(self, Self::Literal | Self::SameUnitConstant)
    }

    const ALL: [Self; 6] = [
        Self::Literal,
        Self::SameUnitConstant,
        Self::ConfigurationLookup,
        Self::Injected,
        Self::MethodReturn,
        Self::Other,
    ];
}

// ── Static-literal reading (mirrors `extract::static_string_literal`) ────────

/// The literal text of a fully static string node, or `None` when the node is
/// not a string or carries an interpolation.
///
/// A deliberate mirror of the private `extract::static_string_literal`: the
/// harness must be able to *say why* a site is refused, which the production
/// function does not report. The corpus test cross-checks the mirror against
/// what the real pass actually emitted, so a drift shows up as a failure rather
/// than as a quietly wrong number.
fn static_literal(node: Node<'_>, src: &[u8]) -> Option<String> {
    if !node.kind().contains("string") {
        return None;
    }
    let mut content = String::new();
    let mut saw_child = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        saw_child = true;
        match child.kind() {
            "string_content"
            | "string_fragment"
            | "escape_sequence"
            | "interpreted_string_literal_content"
            | "raw_string_literal_content"
            | "string_literal_content"
            | "raw_string_content" => content.push_str(child.utf8_text(src).ok()?),
            "string_start" | "string_end" | "raw_string_start" | "raw_string_end" => {}
            _ => return None,
        }
    }
    if !saw_child {
        let raw = node.utf8_text(src).ok()?;
        let body = raw.trim_start_matches(['r', 'b', '#', '@']);
        let first = body.chars().next();
        let unwrapped = match (first, body.chars().next_back()) {
            (Some(open @ ('"' | '\'' | '`')), Some(close))
                if open == close && body.chars().count() >= 2 =>
            {
                &body[open.len_utf8()..body.len() - close.len_utf8()]
            }
            _ => body.trim_matches(['"', '\'', '`', '#']),
        };
        content.push_str(unwrapped);
    }
    Some(content)
}

// ── Same-unit bindings ──────────────────────────────────────────────────────

/// One same-file binding of a name: its initialiser (when the unit shows one)
/// and the text of the declaration that introduced it (for annotation reading).
#[derive(Clone)]
struct Binding<'t> {
    value: Option<Node<'t>>,
    decl_kind: String,
    decl_text: String,
    /// Whether the declaration carries a const/final marker before its `=`.
    is_const: bool,
}

/// Every name the compilation unit binds, with each binding kept — a name bound
/// twice keeps both, so classification can take the most resolvable.
struct Unit<'t> {
    bindings: BTreeMap<String, Vec<Binding<'t>>>,
}

/// Declaration keywords that mark a binding immutable across the supported
/// grammars (`val` covers Kotlin, `readonly` C#).
const CONST_MARKERS: [&str; 5] = ["final", "const", "readonly", "val", "static"];

impl<'t> Unit<'t> {
    fn build(root: Node<'t>, src: &[u8]) -> Self {
        let mut bindings: BTreeMap<String, Vec<Binding<'t>>> = BTreeMap::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));

            // `name`/`value` (Java, C#, TS declarators; Go `const_spec`) and
            // `left`/`right` (Python assignment, Go short declaration).
            let named = node
                .child_by_field_name("name")
                .map(|n| (n, node.child_by_field_name("value")))
                .or_else(|| {
                    // `left`/`right` is a *binding* only for an assignment or a
                    // Go short declaration — a `binary_expression` carries the
                    // same field names and must never be read as one.
                    let k = node.kind();
                    if !(k.contains("assignment") || k == "short_var_declaration") {
                        return None;
                    }
                    let left = node.child_by_field_name("left")?;
                    Some((left, node.child_by_field_name("right")))
                });
            let Some((name_node, value)) = named else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(src) else {
                continue;
            };
            let name = name.trim_start_matches('$'); // PHP variables
            if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let decl = declaration_of(node);
            let decl_text = decl
                .utf8_text(src)
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect::<String>();
            let head = decl_text.split('=').next().unwrap_or_default().to_string();
            let is_const = CONST_MARKERS.iter().any(|m| {
                head.split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|tok| tok == *m)
            });
            bindings.entry(name.to_string()).or_default().push(Binding {
                value,
                decl_kind: decl.kind().to_string(),
                decl_text,
                is_const,
            });
        }
        Self { bindings }
    }
}

/// The declaration statement owning a binding node — the node whose text
/// carries the modifiers and annotations (`@Value`, `final`, `const`).
fn declaration_of(node: Node<'_>) -> Node<'_> {
    let mut current = node;
    for _ in 0..4 {
        let Some(parent) = current.parent() else { break };
        let k = parent.kind();
        if k.ends_with("_declaration")
            || k.ends_with("_statement")
            || k.ends_with("_spec")
            || k == "field_declaration"
            || k == "property_declaration"
        {
            return parent;
        }
        current = parent;
    }
    node
}

/// `true` when a receiver/expression spelling names a configuration source.
///
/// Deliberately a **name** rule, not a type rule: the corpus's configuration
/// beans are `@ConfigurationProperties` classes reached through fields called
/// `…ApiProperties` / `…ConfigurationApi`, and no type information is available
/// at this layer. Every classified site is printed with its source text so the
/// rule's verdict is auditable rather than asserted ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md
fn looks_like_configuration(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["propert", "config", "getenv", "environ", "process.env", "settings", "viper."]
        .iter()
        .any(|needle| lower.contains(needle))
}

// ── Operand decomposition and classification ────────────────────────────────

/// Split a captured path argument into its composition operands: a
/// concatenation into its terms, an interpolated string into its fragments and
/// substitutions, anything else into itself.
fn operands<'t>(node: Node<'t>, src: &[u8], out: &mut Vec<Node<'t>>) {
    let kind = node.kind();
    if kind == "parenthesized_expression" {
        let mut cursor = node.walk();
        let inner = node.named_children(&mut cursor).next();
        drop(cursor);
        if let Some(inner) = inner {
            operands(inner, src, out);
            return;
        }
    }
    // A concatenation (`+` everywhere, `.` in PHP) decomposes into its terms.
    if let (Some(left), Some(right)) = (
        node.child_by_field_name("left"),
        node.child_by_field_name("right"),
    ) {
        let op = node
            .child_by_field_name("operator")
            .and_then(|n| n.utf8_text(src).ok())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let between = &src[left.end_byte()..right.start_byte()];
                String::from_utf8_lossy(between).trim().to_string()
            });
        if op == "+" || op == "." {
            operands(left, src, out);
            operands(right, src, out);
            return;
        }
    }
    if kind.contains("string") && static_literal(node, src).is_none() {
        // An interpolated / templated string: fragments are literal operands,
        // substitutions decompose into the expressions they interpolate.
        let mut cursor = node.walk();
        let children: Vec<Node<'t>> = node.named_children(&mut cursor).collect();
        if !children.is_empty() {
            for child in children {
                match child.kind() {
                    "string_start" | "string_end" | "raw_string_start" | "raw_string_end" => {}
                    k if k.contains("content") || k.contains("fragment") || k == "escape_sequence" => {
                        out.push(child)
                    }
                    _ => {
                        let mut inner = child.walk();
                        let expr = child.named_children(&mut inner).next();
                        match expr {
                            Some(e) => operands(e, src, out),
                            None => out.push(child),
                        }
                    }
                }
            }
            return;
        }
    }
    out.push(node);
}

/// Classify one operand, recursing through same-unit bindings up to `depth`.
fn classify(node: Node<'_>, src: &[u8], unit: &Unit<'_>, depth: usize) -> OperandKind {
    if static_literal(node, src).is_some() {
        return OperandKind::Literal;
    }
    let text = node.utf8_text(src).unwrap_or_default().trim();
    let kind = node.kind();

    // A call: configuration lookup when its receiver names a configuration
    // source, otherwise a method return.
    if kind.contains("call") || kind.contains("invocation") {
        let receiver = node
            .child_by_field_name("object")
            .or_else(|| node.child_by_field_name("function"))
            .and_then(|n| n.utf8_text(src).ok())
            .unwrap_or(text);
        return if looks_like_configuration(receiver) || looks_like_configuration(text) {
            OperandKind::ConfigurationLookup
        } else {
            OperandKind::MethodReturn
        };
    }
    if kind.contains("lambda") || kind.contains("arrow") || kind.contains("closure") {
        return OperandKind::Other;
    }

    // A name, or a field access reducible to one.
    let name = match kind {
        "identifier" | "type_identifier" | "variable_name" | "simple_identifier" => {
            text.trim_start_matches('$').to_string()
        }
        _ if kind.contains("field") || kind.contains("member") || kind.contains("selector") => {
            if looks_like_configuration(text) {
                return OperandKind::ConfigurationLookup;
            }
            // `this.FOO` / `self.FOO` reduces to the unit-level name `FOO`;
            // `Other.FOO` does not (it is another compilation unit).
            let (head, tail) = text.rsplit_once('.').unwrap_or(("", text));
            let head = head.trim();
            if head.is_empty() || head == "this" || head == "self" {
                tail.trim().to_string()
            } else {
                return OperandKind::Other;
            }
        }
        _ => return OperandKind::Other,
    };

    if depth == 0 {
        return OperandKind::Other;
    }
    let Some(bindings) = unit.bindings.get(&name) else {
        return OperandKind::Other;
    };
    // Biased in favour of CRA-01: the most resolvable binding wins.
    bindings
        .iter()
        .map(|b| classify_binding(b, src, unit, depth - 1))
        .min()
        .unwrap_or(OperandKind::Other)
}

fn classify_binding(binding: &Binding<'_>, src: &[u8], unit: &Unit<'_>, depth: usize) -> OperandKind {
    if let Some(value) = binding.value {
        if static_literal(value, src).is_some() {
            return OperandKind::SameUnitConstant;
        }
        return classify(value, src, unit, depth);
    }
    // Declared but never initialised in the unit.
    if binding.decl_text.contains("@Value")
        || binding.decl_text.contains("@ConfigurationProperties")
        || looks_like_configuration(&binding.decl_text)
    {
        return OperandKind::ConfigurationLookup;
    }
    if binding.decl_kind.contains("parameter") {
        return OperandKind::Other;
    }
    if binding.decl_kind.contains("field") || binding.decl_kind.contains("property") {
        return OperandKind::Injected;
    }
    OperandKind::Other
}

// ── Site collection (mirrors the arm's verb gate) ───────────────────────────

const DECLARED_METHOD_PREFIX: &str = "invoke.http.method.";
const HTTP_METHODS: [&str; 7] = ["get", "post", "put", "delete", "patch", "head", "options"];

fn is_http_method(name: &str) -> bool {
    HTTP_METHODS.iter().any(|m| name.eq_ignore_ascii_case(m))
}

/// The path-argument nodes of every site the arm's dispatch would consider —
/// the same capture names and the same HTTP-verb gate `collect_invocation_sites`
/// applies, so the harness counts the arm's corpus, not a grep's.
fn collect_sites<'t>(
    query: &Query,
    root: Node<'t>,
    src: &'t [u8],
    invocation_methods: &BTreeMap<String, String>,
) -> Vec<(u32, Node<'t>)> {
    let names = query.capture_names();
    let mut sites = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method_node = None;
        let mut declared = None;
        let mut arg_node = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "invoke.http.method" => method_node = Some(cap.node),
                "invoke.http.arg" => arg_node = Some(cap.node),
                other => {
                    if let Some(verb) = other.strip_prefix(DECLARED_METHOD_PREFIX) {
                        declared.get_or_insert((verb, cap.node));
                    }
                }
            }
        }
        let Some(arg_node) = arg_node else { continue };
        let method = match method_node {
            Some(node) => {
                let Ok(text) = node.utf8_text(src) else { continue };
                let text = text.trim();
                if invocation_methods.is_empty() {
                    text.to_string()
                } else {
                    match invocation_methods.get(text) {
                        Some(m) => m.clone(),
                        None => continue,
                    }
                }
            }
            None => match declared {
                Some((verb, _)) => verb.to_string(),
                None => continue,
            },
        };
        if !is_http_method(&method) {
            continue;
        }
        let anchor = method_node
            .or(declared.map(|(_, n)| n))
            .unwrap_or(arg_node);
        sites.push((anchor.start_position().row as u32 + 1, arg_node));
    }
    sites
}

/// Mirrors `resolve::matches_detector` (crate-private): a canonical reference
/// target equals a detector or extends it by whole `::` segments.
fn matches_detector(target: &str, detector: &str) -> bool {
    target == detector
        || target
            .strip_prefix(detector)
            .is_some_and(|rest| rest.starts_with("::"))
}

// ── One classified site ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Site {
    file: String,
    line: u32,
    text: String,
    kinds: Vec<OperandKind>,
    /// The template folding would produce, when every operand folds.
    folded: Option<String>,
    gate_admitted: bool,
}

impl Site {
    fn foldable(&self) -> bool {
        !self.kinds.is_empty() && self.kinds.iter().all(|k| k.is_foldable())
    }

    /// A single static literal is what the arm admits **today**; folding adds
    /// nothing here.
    fn already_static_literal(&self) -> bool {
        self.kinds == [OperandKind::Literal]
    }

    /// Newly admissible: folds to an absolute path template the arm does not
    /// already see. An absolute *URL* (`http://host/p`) stays refused for the
    /// independent reason that its route prefix is external ([FR-WS-08] AC2),
    /// so it is not counted.
    fn newly_admissible(&self) -> bool {
        self.foldable()
            && !self.already_static_literal()
            && self.folded.as_deref().is_some_and(|t| t.starts_with('/'))
    }
}

// ── The measurement ─────────────────────────────────────────────────────────

#[derive(Default, Debug)]
struct LangStats {
    files_scanned: usize,
    files_gate_admitted: usize,
    emitted_today: usize,
    sites: Vec<Site>,
}

#[derive(Default)]
struct Measurement {
    per_language: BTreeMap<String, LangStats>,
    /// Sites admitted under the strict reading (`final`/`const` bindings only).
    strict_const_admits: usize,
}

fn scan_file(
    rel: &str,
    source: &str,
    plugin: &dyn LanguagePlugin,
    ctx: &SymbolContext,
    stats: &mut LangStats,
    strict: &mut usize,
) {
    let Some(query) = plugin.query("invocations") else {
        return;
    };
    stats.files_scanned += 1;

    let facts = extract::extract(&FileInput::new(rel, source), plugin, ctx);
    let detectors = &plugin.semantics().http_client_detectors;
    let gate = !detectors.is_empty()
        && facts
            .refs
            .iter()
            .any(|r| detectors.iter().any(|d| matches_detector(&r.target, d)));
    if gate {
        stats.files_gate_admitted += 1;
    }
    stats.emitted_today += facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
        .count();

    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        return;
    };
    let src = source.as_bytes();
    let unit = Unit::build(tree.root_node(), src);

    for (line, arg) in collect_sites(
        query,
        tree.root_node(),
        src,
        &plugin.semantics().invocation_methods,
    ) {
        let mut nodes = Vec::new();
        operands(arg, src, &mut nodes);
        let kinds: Vec<OperandKind> = nodes
            .iter()
            .map(|n| classify(*n, src, &unit, 4))
            .collect();
        let folded = kinds
            .iter()
            .all(|k| k.is_foldable())
            .then(|| {
                nodes
                    .iter()
                    .map(|n| folded_text(*n, src, &unit))
                    .collect::<Option<Vec<_>>>()
                    .map(|parts| parts.concat())
            })
            .flatten();
        let site = Site {
            file: rel.to_string(),
            line,
            text: arg
                .utf8_text(src)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            kinds,
            folded,
            gate_admitted: gate,
        };
        if gate && site.newly_admissible() && strictly_const(&nodes, src, &unit) {
            *strict += 1;
        }
        stats.sites.push(site);
    }
}

/// The text an operand folds to, or `None` when it does not fold.
fn folded_text(node: Node<'_>, src: &[u8], unit: &Unit<'_>) -> Option<String> {
    if let Some(text) = static_literal(node, src) {
        return Some(text);
    }
    let name = node
        .utf8_text(src)
        .ok()?
        .trim()
        .trim_start_matches("this.")
        .trim_start_matches("self.")
        .trim_start_matches('$')
        .to_string();
    unit.bindings
        .get(&name)?
        .iter()
        .find_map(|b| b.value.and_then(|v| static_literal(v, src)))
}

/// Whether every non-literal operand's binding carries a const/final marker —
/// the strict reading of [CR-113] §3.2 ("a reference to a **constant**").
fn strictly_const(nodes: &[Node<'_>], src: &[u8], unit: &Unit<'_>) -> bool {
    nodes.iter().all(|n| {
        if static_literal(*n, src).is_some() {
            return true;
        }
        let Ok(text) = n.utf8_text(src) else {
            return false;
        };
        let name = text
            .trim()
            .trim_start_matches("this.")
            .trim_start_matches("self.")
            .trim_start_matches('$');
        unit.bindings
            .get(name)
            .is_some_and(|bs| bs.iter().any(|b| b.is_const && b.value.is_some()))
    })
}

fn corpus_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").ok()?).join(rest),
        None => PathBuf::from(raw),
    };
    expanded.is_dir().then_some(expanded)
}

fn measure(root: &Path) -> Measurement {
    let registry = LanguageRegistry::load(root).expect("plugin registry loads");
    let ctx = SymbolContext::default();
    let mut m = Measurement::default();

    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .parents(false)
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        let Some(plugin) = registry.for_path(&rel) else {
            continue;
        };
        if plugin.query("invocations").is_none() {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let lang = plugin.name().to_string();
        let stats = m.per_language.entry(lang).or_default();
        scan_file(
            &rel,
            &source,
            plugin,
            &ctx,
            stats,
            &mut m.strict_const_admits,
        );
    }
    m
}

fn report(m: &Measurement) {
    println!("\n=== S-355: operand resolvability across the composed client-call corpus ===\n");
    println!(
        "The corpus is the set of sites the arm actually considers: a query match \n\
         with an HTTP verb, inside a file the `http_client_detectors` ledger gate \n\
         admits. Sites in non-gate-admitted files are counted separately as the \n\
         ceiling a widened gate would expose — the arm never reaches them today.\n"
    );
    println!(
        "{:<12} {:>7} {:>6} {:>7} {:>8} {:>8} {:>7} {:>7} {:>7} {:>7} {:>6}",
        "language",
        "files",
        "gated",
        "sites",
        "emitted",
        "literal",
        "same-u",
        "config",
        "inject",
        "return",
        "other",
    );
    let mut total_new = 0usize;
    for (lang, stats) in &m.per_language {
        let mut counts: BTreeMap<OperandKind, usize> = BTreeMap::new();
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            // A site is counted under its *least* resolvable operand — the one
            // that decides admissibility.
            let kind = site.kinds.iter().copied().max().unwrap_or(OperandKind::Other);
            *counts.entry(kind).or_default() += 1;
        }
        let gated_sites = stats.sites.iter().filter(|s| s.gate_admitted).count();
        println!(
            "{:<12} {:>7} {:>6} {:>7} {:>8} {:>8} {:>7} {:>7} {:>7} {:>7} {:>6}",
            lang,
            stats.files_scanned,
            stats.files_gate_admitted,
            gated_sites,
            stats.emitted_today,
            counts.get(&OperandKind::Literal).copied().unwrap_or(0),
            counts.get(&OperandKind::SameUnitConstant).copied().unwrap_or(0),
            counts.get(&OperandKind::ConfigurationLookup).copied().unwrap_or(0),
            counts.get(&OperandKind::Injected).copied().unwrap_or(0),
            counts.get(&OperandKind::MethodReturn).copied().unwrap_or(0),
            counts.get(&OperandKind::Other).copied().unwrap_or(0),
        );
        total_new += stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.newly_admissible())
            .count();
    }

    println!("\n--- what constant folding would NEWLY admit, per language ---");
    for (lang, stats) in &m.per_language {
        let new: Vec<&Site> = stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.newly_admissible())
            .collect();
        let gated_sites = stats.sites.iter().filter(|s| s.gate_admitted).count();
        println!(
            "{lang:<12} newly admitted: {:>3}   (of {gated_sites} gate-admitted sites; \
             {} more behind the ledger gate)",
            new.len(),
            stats.sites.len() - gated_sites,
        );
        for site in new {
            println!(
                "               + {}:{}  {}  ->  {}",
                site.file,
                site.line,
                site.text,
                site.folded.as_deref().unwrap_or("<unfolded>"),
            );
        }
    }
    println!(
        "\nTOTAL newly admitted (generous reading): {total_new}\
         \nTOTAL newly admitted (strict const-only reading): {}",
        m.strict_const_admits
    );

    println!("\n--- site census over the gate-admitted corpus (every site, auditable) ---");
    for (lang, stats) in &m.per_language {
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            let kinds: Vec<&str> = site.kinds.iter().map(|k| k.label()).collect();
            println!("{lang}  {}:{}  [{}]  {}", site.file, site.line, kinds.join(" + "), site.text);
        }
    }
}

/// The measurement itself. Skips — loudly — when no corpus is configured, so
/// `cargo test --workspace` stays green on a machine without one.
#[test]
fn measure_operand_resolvability_over_the_reference_workspace() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-355 measurement (see this file's module docs for the recorded finding)."
        );
        return;
    };
    let m = measure(&root);
    report(&m);

    // Cross-check the harness against the real pass: every reference the arm
    // actually emitted must correspond to a site the harness classified as a
    // single static literal. A drift in the mirrored literal reading fails here
    // instead of silently skewing the numbers.
    for (lang, stats) in &m.per_language {
        let static_literal_sites = stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.already_static_literal())
            .count();
        assert!(
            stats.emitted_today <= static_literal_sites,
            "{lang}: the arm emitted {} references but the harness sees only {} \
             single-static-literal sites in gate-admitted files — the mirrored \
             literal reading has drifted from `extract::static_string_literal`",
            stats.emitted_today,
            static_literal_sites,
        );
    }
    assert!(
        !m.per_language.is_empty(),
        "the corpus at {} yielded no file in any language shipping `invocations`",
        root.display(),
    );
}

// ── Classifier fixtures (always run) ────────────────────────────────────────

#[cfg(feature = "lang-java")]
mod fixtures {
    use super::*;

    /// Classify a Java expression written as the argument of a `.uri(…)` call,
    /// with `prelude` supplying the compilation unit's declarations.
    fn kinds_of(prelude: &str, expr: &str) -> Vec<OperandKind> {
        let source = format!(
            "package com.example;\n\
             import org.springframework.web.reactive.function.client.WebClient;\n\
             public class Calls {{\n{prelude}\n  void call() {{ client.get().uri({expr}); }}\n}}\n"
        );
        let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry");
        let plugin = registry.for_path("Calls.java").expect("java plugin");
        let query = plugin.query("invocations").expect("invocations query");
        let mut parser = Parser::new();
        parser.set_language(plugin.language()).expect("language");
        let tree = parser.parse(&source, None).expect("parse");
        let src = source.as_bytes();
        let unit = Unit::build(tree.root_node(), src);
        let sites = collect_sites(
            query,
            tree.root_node(),
            src,
            &plugin.semantics().invocation_methods,
        );
        assert_eq!(sites.len(), 1, "fixture must yield exactly one site: {expr}");
        let mut nodes = Vec::new();
        operands(sites[0].1, src, &mut nodes);
        nodes.iter().map(|n| classify(*n, src, &unit, 4)).collect()
    }

    /// A single static literal — what the arm already admits; folding adds
    /// nothing.
    #[test]
    fn a_static_literal_is_a_literal_operand() {
        assert_eq!(kinds_of("", r#""/users/{id}""#), vec![OperandKind::Literal]);
    }

    /// [CR-113]'s motivating shape: a same-unit literal-initialised constant
    /// concatenated with a literal path and a parameter.
    #[test]
    fn a_same_unit_constant_prefix_folds() {
        let prelude = r#"  private static final String BASE = "/api/v1";"#;
        assert_eq!(
            kinds_of(prelude, r#"BASE + "/soggetti""#),
            vec![OperandKind::SameUnitConstant, OperandKind::Literal],
        );
    }

    /// A bare same-unit constant reference — the degenerate one-operand
    /// composition, and the only shape the reference workspace actually
    /// contributes.
    #[test]
    fn a_bare_same_unit_constant_is_resolvable() {
        let prelude = r#"  private static final String PATH = "/v1/alerts";"#;
        assert_eq!(kinds_of(prelude, "PATH"), vec![OperandKind::SameUnitConstant]);
    }

    /// A `@Value`-injected base is a configuration lookup — refused, and the
    /// boundary [CR-113] §3.3 exists to preserve.
    #[test]
    fn a_value_injected_base_is_a_configuration_lookup() {
        let prelude = "  @Value(\"${service.base}\")\n  private String base;";
        assert_eq!(
            kinds_of(prelude, r#"base + "/users""#),
            vec![OperandKind::ConfigurationLookup, OperandKind::Literal],
        );
    }

    /// A configuration-properties bean's getter — the reference workspace's
    /// dominant shape.
    #[test]
    fn a_properties_bean_getter_is_a_configuration_lookup() {
        let prelude = "  private final MailboxApiProperties mailboxApiProperties;";
        assert_eq!(
            kinds_of(prelude, "mailboxApiProperties.getUriGetMailbox()"),
            vec![OperandKind::ConfigurationLookup],
        );
    }

    /// A plain helper-method call is a method return, not a foldable operand.
    #[test]
    fn a_helper_method_call_is_a_method_return() {
        assert_eq!(
            kinds_of("", "buildCreateMailboxUrl()"),
            vec![OperandKind::MethodReturn],
        );
    }

    /// An injected collaborator field, never initialised in the unit.
    #[test]
    fn an_uninitialised_field_is_injected() {
        let prelude = "  private String basePath;";
        assert_eq!(kinds_of(prelude, "basePath"), vec![OperandKind::Injected]);
    }

    /// Folding never recurses into a value the source does not prove
    /// ([FR-WS-18] AC4): a constant initialised from another call stays refused.
    #[test]
    fn a_constant_initialised_from_a_call_does_not_fold() {
        let prelude = "  private static final String BASE = System.getenv(\"BASE\");";
        let kinds = kinds_of(prelude, r#"BASE + "/users""#);
        assert_eq!(kinds[0], OperandKind::ConfigurationLookup);
        assert!(!kinds[0].is_foldable());
    }

    /// Only literals and same-unit constants fold.
    #[test]
    fn foldability_is_exactly_literal_and_same_unit_constant() {
        for kind in OperandKind::ALL {
            assert_eq!(
                kind.is_foldable(),
                matches!(kind, OperandKind::Literal | OperandKind::SameUnitConstant),
                "{kind:?}",
            );
        }
    }
}
