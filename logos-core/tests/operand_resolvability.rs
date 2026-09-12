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
//! captured path argument into operands and classifies each operand into the
//! taxonomy [S-355]'s acceptance criterion names: `literal`, `same-unit
//! constant`, `injected`, `configuration lookup`, `method return`, `other`.
//!
//! A site is **newly admissible** under [FR-WS-18] AC1's reading when its
//! leading operand folds, the template it folds to is an absolute path (a
//! trailing non-foldable operand being the `{}` placeholder a route template
//! already expresses), and the arm does not already admit it. The stricter
//! reading — *every* operand folds — is measured and reported alongside.
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
//! - resolution recurses through a same-unit binding's initialiser to
//!   [`FOLD_DEPTH`] rather than stopping at the first indirection, and
//!   [`classify`] and [`folded_text`] reach exactly the same distance;
//! - a qualified access reduces to its last segment and is resolved against
//!   this unit's bindings even when its qualifier names another unit;
//! - the headline count uses AC1's placeholder reading, not the strict one.
//!
//! The one place the bias is inverted is deliberate and reported: a name the
//! unit binds to **two different** literals folds to neither, because the
//! source does not prove which the call site sees. Such sites are listed under
//! "foldable by classification but not reducible to one proven template" rather
//! than silently dropped.
//!
//! # Recorded finding (2026-09-05, `~/source/pec-services`, 90 members)
//!
//! **CRA-01 is FALSIFIED.** Over the corpus the arm actually considers — a
//! verb-anchored query match inside a ledger-gate-admitted file:
//!
//! ```text
//! language     files  gated  sites  emitted  literal  same-u  config  inject  return  other
//! go             262     75     98        0       51       6       7       0       0     34
//! java          2447     26     98        0        0       1      81       0       0     16
//! php            161      0      0        0        0       0       0       0       0      0
//! python          55      1      2        0        0       0       0       0       0      2
//! tsx              5      0      0        0        0       0       0       0       0      0
//! typescript     656     12      1        1        1       0       0       0       0      0
//! ```
//!
//! **Constant folding would newly admit ZERO sites, in every language**, under
//! both readings. The Java corpus CRA-01 is about resolves 81 of 98 sites to a
//! *configuration lookup* (`someApiProperties.getUriX()` on a cross-unit
//! `@ConfigurationProperties` bean whose value lives in `application.yml`, not
//! in source), 16 to `other` (9 `uriBuilder -> …path(<that same getter>)`
//! lambdas, 4 `.uri(uri, …)` method parameters, 3 incidental list accesses),
//! and exactly **1** to a same-unit constant — `JSESSIONID_COOKIE_NAME`, a
//! cookie name, not a path.
//!
//! Go's 6 same-unit constants are likewise not paths: they are HTTP **header
//! names** (`Origin`, `AccessControlRequestMethod`, `HeaderXForwardedHost`)
//! read through `header.Get(…)` inside a `net/http` file — the documented
//! [ADR-54] file-grained gate ceiling, not outbound calls.
//!
//! Ignoring the ledger gate entirely raises the ceiling to **3** sites
//! workspace-wide, all `WebTestClient` in-process test calls in two test files
//! — a service calling its own routes, not cross-service coupling. That figure
//! is computed and printed by the run, not asserted in prose.
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
//! The classifier's own rules are pinned by fixture tests that always run, one
//! per language the corpus contains, so no reported column rests on an
//! unexercised code path. A `LOGOS_REF_WORKSPACE` that is set but does not
//! resolve to a directory **panics** rather than skipping — a green run that
//! measured nothing is the one outcome this file must never produce.
//!
//! # This harness is deliberately language-specific
//!
//! [`CONST_MARKERS`], [`BINDING_KINDS`], [`looks_like_configuration`]'s needle
//! list, the per-grammar node-kind tests, **and every Spring-specific table in
//! the `configuration_agreement` submodule** — `HEADER_PUBLISH_QUERY`,
//! `BASE_URL_METHODS` and `names_topic_header` — are exactly the kind of table
//! `resolve::framework::tests::jvm_parity::no_language_specific_composition_code_exists`
//! forbids under `logos-core/src/resolve/`. They are legitimate *here*, in a
//! measurement over a fixed corpus, and must not be lifted into the resolver.
//!
//! Two entries left this list in Sprint 67, and by promotion rather than by
//! exception: the relaxed-binding rules are `extract::config::corpus` (S-380)
//! and the `@ConfigurationProperties` index is `extract::config::binding`
//! (S-381). Neither took its Spring vocabulary along — that lives in each
//! plugin's `queries/properties.scm` and `[properties]` descriptor table, which
//! is where the prohibition above says a real arm's capture belongs.
//!
//! This list is **open, not closed**: anything of that kind added to this
//! harness or its submodules is covered by the same carve-out and the same
//! prohibition. The fitness function cannot enforce it — it scans
//! `src/resolve/` only — so extending the enumeration when the surface grows is
//! the whole guard.
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

/// S-365's extension: configuration-key resolvability and profile agreement,
/// over this harness's client-call corpus **and** the broker-publish corpus.
/// One binary, one walk, one report — see its module docs for why the two arms
/// share a gate.
///
/// `#[path]`-attached: a plain `tests/configuration_agreement.rs` would be
/// auto-discovered by cargo as a *second* test target, which is precisely what
/// the story forbids — one gate, not two measurements drifting apart. A
/// directory alongside the root file carries no target.
#[path = "operand_resolvability/configuration_agreement.rs"]
mod configuration_agreement;

/// S-384's identity gate — its own module, so the deploy-corpus walk and the
/// pair judgement do not co-edit the file the configuration arm owns. Reads
/// this module's `measurement` and the promoted `ConfigCorpus`; adds no symbol
/// to either.
///
/// `#[path]`-attached for the same reason `configuration_agreement` is: a plain
/// `tests/identity.rs` would become a second cargo test target and walk the
/// estate a second time.
#[path = "operand_resolvability/identity.rs"]
mod identity;

/// S-374's recorded verdict, reproduced by
/// [`measure_recorded_client_call_refusals_over_the_reference_workspace`] and
/// printed by it.
///
/// `include_str!` rather than a doc link, following
/// [`configuration_agreement::RECORDED_FINDING`]: a file the build embeds cannot
/// be deleted or renamed without breaking compilation, so the artifact and the
/// run that produced it cannot drift apart silently.
const RECORDED_REFUSAL_FINDING: &str =
    include_str!("operand_resolvability/client_call_refusal_finding.txt");

// ── The taxonomy ────────────────────────────────────────────────────────────

/// The operand taxonomy [S-355]'s first acceptance criterion names (and
/// [CR-113] §3.2/§3.3 describe in prose), ordered **most resolvable first** so
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

/// Named child kinds that carry a literal's **content** rather than its
/// delimiters. `operands` pushes these as literal fragments of an interpolated
/// string, so `static_literal` must read them verbatim: they have no quotes to
/// unwrap, and running the delimiter-stripping fallback over them corrupts the
/// text (`` `${BASE}rest/v1` `` would lose its leading `r`).
const LITERAL_FRAGMENT_KINDS: [&str; 7] = [
    "string_content",
    "string_fragment",
    "escape_sequence",
    "interpreted_string_literal_content",
    "raw_string_literal_content",
    "string_literal_content",
    "raw_string_content",
];

/// Named child kinds that are a literal's delimiters and carry no content.
const LITERAL_DELIMITER_KINDS: [&str; 4] =
    ["string_start", "string_end", "raw_string_start", "raw_string_end"];

/// The literal text of a fully static string node, or `None` when the node is
/// not a string or carries an interpolation.
///
/// A mirror of `extract::static_string_literal`, which is a private `fn` in
/// `logos-core` and therefore unreachable from an integration test. The mirror
/// is **pinned, not trusted**: the fixtures below assert it against the exact
/// shapes production handles (including the trailing trim and empty-rejection
/// that this function previously omitted), and the corpus test cross-checks it
/// against what the real pass actually emitted.
fn static_literal(node: Node<'_>, src: &[u8]) -> Option<String> {
    // A bare content fragment: verbatim, no delimiters to strip, no trim (its
    // whitespace is real content of the surrounding template).
    if LITERAL_FRAGMENT_KINDS.contains(&node.kind()) {
        return node.utf8_text(src).ok().map(str::to_string);
    }
    if !node.kind().contains("string") {
        return None;
    }
    let mut content = String::new();
    let mut saw_child = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        saw_child = true;
        let kind = child.kind();
        if LITERAL_FRAGMENT_KINDS.contains(&kind) {
            content.push_str(child.utf8_text(src).ok()?);
        } else if LITERAL_DELIMITER_KINDS.contains(&kind) {
            // Carries no content — skip without disqualifying the literal.
        } else {
            // An interpolation / template substitution / expansion → dynamic.
            return None;
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
    // The production tail: an empty or whitespace-only literal is refused, not
    // returned as an empty path (`extract::static_string_literal`).
    let content = content.trim().to_string();
    (!content.is_empty()).then_some(content)
}

/// Reduce an operand node to the unit-level **name** it references, or `None`
/// when it is not a name.
///
/// The single spelling of this reduction — `classify`, `folded_text` and
/// `strictly_const` all consult it, so they cannot drift apart the way the
/// first draft's three separate copies did.
///
/// Qualified accesses (`this.BASE`, `self.BASE`, `$this->base`, `self::BASE`,
/// `c.basePath`, `Routes.BASE`) reduce to their **last** segment and are then
/// resolved against the unit's own bindings. That is deliberately generous:
/// a qualifier naming another compilation unit resolves only if this file
/// happens to bind the same name, and every such site is printed in the census
/// so the generosity is auditable rather than hidden.
fn operand_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let text = node.utf8_text(src).ok()?.trim();
    let kind = node.kind();
    let bare = |s: &str| -> Option<String> {
        let s = s.trim().trim_start_matches('$').trim();
        (!s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')).then(|| s.to_string())
    };
    match kind {
        "identifier" | "type_identifier" | "variable_name" | "simple_identifier" | "name"
        | "field_identifier" | "property_identifier" => bare(text),
        // Python `attribute`, Go `selector_expression`, Java `field_access`,
        // TS `member_expression`, PHP `member_access_expression` /
        // `class_constant_access_expression` / `scoped_property_access_expression`.
        _ if kind == "attribute"
            || kind.contains("field")
            || kind.contains("member")
            || kind.contains("selector")
            || kind.contains("scoped")
            || kind.contains("class_constant") =>
        {
            let (_, tail) = split_qualified(text)?;
            bare(tail)
        }
        _ => None,
    }
}

/// Split a qualified access on its **last** separator — `.`, `->` or `::`.
fn split_qualified(text: &str) -> Option<(&str, &str)> {
    let (at, len) = ["::", "->", "."]
        .iter()
        .filter_map(|sep| text.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)?;
    Some((&text[..at], &text[at + len..]))
}

// ── Same-unit bindings ──────────────────────────────────────────────────────

/// One same-file binding of a name: its initialiser (when the unit shows one),
/// the kind of node that bound it, and the head of the declaration that
/// introduced it (for annotation reading).
#[derive(Clone)]
struct Binding<'t> {
    value: Option<Node<'t>>,
    /// The binding node's own kind (`variable_declarator`, `formal_parameter`,
    /// `const_spec`, …) — what decides parameter-vs-field, never the text.
    bind_kind: String,
    /// The kind of the declaration statement the binding sits in.
    decl_kind: String,
    /// The declaration's **head** — everything before the first `=`. Scoped
    /// deliberately: reading the whole declaration would, for a parameter, pull
    /// in the entire enclosing method body and let an incidental `config` in
    /// unrelated code classify the parameter as a configuration lookup.
    decl_head: String,
    /// Whether the declaration head carries a const/final marker.
    is_const: bool,
    /// The binding's **declared type**, when the grammar field-names one
    /// (S-365). Read from the node rather than from `decl_head`, so a Java
    /// `private final MailServerConfigurationApi mailServerConfigurationApi;`
    /// yields the type it declares and not the first token of its modifiers.
    decl_type: Option<String>,
}

/// Every name the compilation unit binds, with each binding kept — a name bound
/// twice keeps both, so classification can take the most resolvable.
struct Unit<'t> {
    bindings: BTreeMap<String, Vec<Binding<'t>>>,
}

impl Unit<'_> {
    /// The **simple** type name a binding of `name` declares, when the unit
    /// shows one (S-365). The first binding that carries a type wins; a name
    /// the unit declares twice with different types is a shadowing the source
    /// itself does not disambiguate at this layer, and the resulting key is
    /// reported per-site so it can be audited.
    fn declared_type(&self, name: &str) -> Option<&str> {
        self.bindings
            .get(name)?
            .iter()
            .find_map(|b| b.decl_type.as_deref())
            .map(simple_type_name)
    }
}

/// The simple name of a possibly-generic, possibly-qualified type:
/// `com.acme.Props<String>` → `Props`.
fn simple_type_name(declared: &str) -> &str {
    let head = declared.split(['<', '[']).next().unwrap_or(declared).trim();
    head.rsplit(['.', ':']).next().unwrap_or(head)
}

/// Declaration keywords that mark a binding **immutable** across the supported
/// grammars (`val` covers Kotlin, `readonly` C#).
///
/// `static` is deliberately **absent**: a Java `private static String` is
/// reassignable, so admitting it would make the "strict const-only" counter —
/// the independent lower bound the falsification leans on — not actually
/// const-only.
const CONST_MARKERS: [&str; 4] = ["final", "const", "readonly", "val"];

/// How many same-unit binding hops resolution follows. Shared by [`classify`]
/// and [`folded_text`] so classification and folding can never disagree about
/// what is reachable. Also the cycle guard: `a = b; b = a` terminates here.
const FOLD_DEPTH: usize = 4;

/// Node kinds that genuinely **bind a name to a value** in the supported
/// grammars.
///
/// An allowlist, not a "has a `name` field" rule. Every call, class, method and
/// Python keyword argument also carries a `name` field: under the field rule a
/// `helper(path="/injected")` keyword argument registered a binding for `path`,
/// which then folded an unrelated *parameter* of the same name to a stranger's
/// literal. An allowlist cannot do that.
const BINDING_KINDS: [&str; 15] = [
    // Java / C# / TypeScript / JavaScript
    "variable_declarator",
    // Java / C# parameters (value-less bindings, so a parameter resolves to
    // `Other` rather than to whatever else in the file shares its name)
    "formal_parameter",
    "spread_parameter",
    "catch_formal_parameter",
    "parameter",
    // Go
    "const_spec",
    "var_spec",
    "short_var_declaration",
    // Python
    "assignment",
    // Java / C# / TypeScript / PHP reassignment — a second binding of a name
    // already declared, which is what makes a mutable field unfoldable.
    "assignment_expression",
    // PHP
    "property_element",
    "const_element",
    "simple_parameter",
    "property_promotion_parameter",
    // Kotlin
    "property_declaration",
];

/// Binding node kinds that are a **parameter** — never a same-unit constant,
/// whatever the enclosing declaration's text happens to mention.
fn is_parameter_kind(kind: &str) -> bool {
    kind.contains("parameter")
}

impl<'t> Unit<'t> {
    fn build(root: Node<'t>, src: &[u8]) -> Self {
        let mut bindings: BTreeMap<String, Vec<Binding<'t>>> = BTreeMap::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));

            if !BINDING_KINDS.contains(&node.kind()) {
                continue;
            }

            let (name_nodes, value_nodes) = binding_parts(node);
            if name_nodes.is_empty() {
                continue;
            }

            let decl = declaration_of(node);
            let decl_head = decl
                .utf8_text(src)
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect::<String>()
                .split('=')
                .next()
                .unwrap_or_default()
                .to_string();
            let is_const = CONST_MARKERS.iter().any(|m| {
                decl_head
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|tok| tok == *m)
            });
            // The binding's own `type` field first (a parameter declares its
            // own), then the declaration's (a Java field declares the type once
            // for all its declarators).
            let decl_type = node
                .child_by_field_name("type")
                .or_else(|| decl.child_by_field_name("type"))
                .and_then(|n| n.utf8_text(src).ok())
                .map(str::to_string);

            for (i, name_node) in name_nodes.iter().enumerate() {
                let Some(name) = operand_name(*name_node, src) else {
                    continue;
                };
                // Pair positionally when the arities agree; otherwise the source
                // does not prove which value is which, so bind no value.
                let value = (value_nodes.len() == name_nodes.len())
                    .then(|| value_nodes.get(i).copied())
                    .flatten();
                bindings.entry(name).or_default().push(Binding {
                    value,
                    bind_kind: node.kind().to_string(),
                    decl_kind: decl.kind().to_string(),
                    decl_head: decl_head.clone(),
                    is_const,
                    decl_type: decl_type.clone(),
                });
            }
        }
        Self { bindings }
    }
}

/// The names a binding node binds, and the values it binds them to.
///
/// Three grammar shapes have to be reconciled, and getting any of them wrong
/// silently classifies a real same-unit constant as unresolvable:
///
/// - **fields** (`name`/`value`, `left`/`right`, PHP's `default_value`) — the
///   common case;
/// - **Go's `expression_list`** — `const B = "/x"` binds the value through a
///   list node, which is not a string node, so folding must descend into it;
/// - **positional children** — PHP's `const_element` field-names neither part,
///   and Go's `const A, B = …` field-names only the *first* identifier.
fn binding_parts<'t>(node: Node<'t>) -> (Vec<Node<'t>>, Vec<Node<'t>>) {
    // `children_by_field_name` yields the anonymous separator tokens too
    // (Go's `const A, B` reports `["A", ",", "B"]`), so keep only named nodes —
    // otherwise the name/value arities never match and the binding is dropped.
    let mut cursor = node.walk();
    let mut names: Vec<Node<'t>> = node
        .children_by_field_name("name", &mut cursor)
        .filter(Node::is_named)
        .collect();
    drop(cursor);
    if names.is_empty() {
        if let Some(left) = node.child_by_field_name("left") {
            names = expression_list_items(left);
        }
    }

    let values = node
        .child_by_field_name("value")
        .or_else(|| node.child_by_field_name("default_value"))
        .or_else(|| node.child_by_field_name("right"))
        .map(expression_list_items);

    let mut kids_cursor = node.walk();
    let kids: Vec<Node<'t>> = node.named_children(&mut kids_cursor).collect();

    match values {
        Some(values) => {
            // Recover the names a grammar left positional (Go's `const A, B`).
            if names.len() < values.len() {
                let leading: Vec<Node<'t>> = kids
                    .iter()
                    .copied()
                    .take_while(|n| matches!(n.kind(), "identifier" | "field_identifier"))
                    .collect();
                if leading.len() == values.len() {
                    names = leading;
                }
            }
            (names, values)
        }
        // No field-named value at all: a fully positional binding
        // (PHP `const_element` → `name`, `string`).
        None if names.is_empty() && kids.len() >= 2 => {
            (vec![kids[0]], vec![kids[kids.len() - 1]])
        }
        None => (names, Vec::new()),
    }
}

/// The items of a Go `expression_list`, or the node itself when it is not one.
fn expression_list_items(node: Node<'_>) -> Vec<Node<'_>> {
    if node.kind() == "expression_list" {
        let mut cursor = node.walk();
        return node.named_children(&mut cursor).collect();
    }
    vec![node]
}

/// The declaration statement owning a binding node — the node whose head
/// carries the modifiers and annotations (`@Value`, `final`, `const`).
///
/// A parameter is its own declaration: climbing from one reaches the enclosing
/// `method_declaration`, whose text is the whole method.
fn declaration_of(node: Node<'_>) -> Node<'_> {
    if is_parameter_kind(node.kind()) {
        return node;
    }
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

    // A name, or a qualified access reducible to one. A qualified access whose
    // own spelling names a configuration source is one without needing to
    // resolve it (`cch.configuration.STS.Endpoint`, `process.env.API_HOST`).
    let Some(name) = operand_name(node, src) else {
        return OperandKind::Other;
    };
    let is_qualified = split_qualified(text).is_some();
    if is_qualified && looks_like_configuration(text) {
        return OperandKind::ConfigurationLookup;
    }

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
    // Declared but never initialised in the unit. A parameter is decided by its
    // own node kind FIRST: it is never a configuration lookup, whatever the
    // declaration around it happens to spell.
    if is_parameter_kind(&binding.bind_kind) || is_parameter_kind(&binding.decl_kind) {
        return OperandKind::Other;
    }
    if binding.decl_head.contains("@Value")
        || binding.decl_head.contains("@ConfigurationProperties")
        || looks_like_configuration(&binding.decl_head)
    {
        return OperandKind::ConfigurationLookup;
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
///
/// The verb gate is applied in Rust rather than as a query predicate because it
/// consults `invocation_methods` — a per-plugin normalizer table, not a text
/// match. (`tree_sitter` 0.25 does evaluate `#eq?`/`#match?`/`#any-of?`; the
/// S-365 submodule's note says so and this is the same situation.)
///
/// **One stated divergence:** production additionally drops a site whose anchor
/// has no attributable enclosing symbol; this does not. The omission can only
/// *add* sites, so it biases in favour of CRA-01 like every other judgement
/// call here (see the module docs).
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
    /// The template folding would produce when **every** operand folds — the
    /// strict reading of [CR-113] §3.2.
    folded: Option<String>,
    /// The template folding would produce under [FR-WS-18] AC1's reading, where
    /// a trailing non-foldable operand is the `{}` placeholder a route template
    /// already expresses. The leading operand must still fold: "what changes is
    /// only whether the *prefix* may come from a folded constant" ([CR-113]
    /// §3.2), and AC3 keeps a configuration / injected / method-return base
    /// refused.
    placeholder_folded: Option<String>,
    gate_admitted: bool,
    /// S-365: what each operand's configuration accessor resolved to, `None`
    /// for an operand that is not a configuration lookup.
    key_outcomes: Vec<Option<configuration_agreement::KeyOutcome>>,
    /// S-365: what [CR-115] §3.4's agreement rule does with this site.
    cr115: configuration_agreement::Verdict,
}

impl Site {
    fn foldable(&self) -> bool {
        !self.kinds.is_empty() && self.kinds.iter().all(|k| k.is_foldable())
    }

    /// A single static literal is what the arm admits **today**; folding adds
    /// nothing here.
    fn already_static_literal(&self) -> bool {
        is_already_static_literal(&self.kinds)
    }

    /// Newly admissible under the **strict** reading: every operand folds, to
    /// an absolute path template the arm does not already see. An absolute
    /// *URL* (`http://host/p`) stays refused for the independent reason that
    /// its route prefix is external ([FR-WS-08] AC2), so it is not counted.
    fn newly_admissible(&self) -> bool {
        self.foldable()
            && !self.already_static_literal()
            && self.folded.as_deref().is_some_and(|t| t.starts_with('/'))
    }

    /// Newly admissible under [FR-WS-18] AC1's reading — the **headline**
    /// count, because it is the more generous of the two and AC1 names
    /// `CONST + "/literal/" + param` as admissible outright.
    fn newly_admissible_with_placeholders(&self) -> bool {
        !self.already_static_literal()
            && self
                .placeholder_folded
                .as_deref()
                .is_some_and(|t| t.starts_with('/'))
    }

    /// Foldable by classification, yet folding produced no single proven
    /// template. Never silently dropped — reported so a human can audit it.
    fn foldable_but_unfolded(&self) -> bool {
        self.foldable() && self.folded.is_none()
    }
}

/// Whether a site is already admitted by the arm today.
///
/// A free function because S-365's `judge` needs the same predicate and had
/// spelled it out a second time: the parent's newly-admissible counts and the
/// S-365 `literal` column both rest on it, so two copies could disagree about
/// what "already admitted" means and each publish a different figure.
fn is_already_static_literal(kinds: &[OperandKind]) -> bool {
    kinds == [OperandKind::Literal]
}

/// Classify one captured path argument: its operand kinds, the strict folded
/// template, the [FR-WS-18] AC1 placeholder template, and whether every
/// non-literal operand resolves through a const-marked binding.
///
/// The **single** path from a captured argument to a verdict — the corpus walk
/// and every fixture go through it, so a fixture cannot pass over logic the
/// measurement does not use.
fn classify_site(
    arg: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
) -> (Vec<OperandKind>, Option<String>, Option<String>, bool) {
    let mut nodes = Vec::new();
    operands(arg, src, &mut nodes);
    let kinds: Vec<OperandKind> = nodes
        .iter()
        .map(|n| classify(*n, src, unit, FOLD_DEPTH))
        .collect();
    let folded = kinds
        .iter()
        .all(|k| k.is_foldable())
        .then(|| {
            nodes
                .iter()
                .map(|n| folded_text(*n, src, unit, FOLD_DEPTH))
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.concat().trim().to_string())
                .filter(|t| !t.is_empty())
        })
        .flatten();
    let placeholder = placeholder_template(&nodes, src, unit, FOLD_DEPTH);
    let strictly = strictly_const(&nodes, src, unit);
    (kinds, folded, placeholder, strictly)
}

/// The template a composition folds to under [FR-WS-18] AC1: the leading
/// operand must fold, and each later operand either folds or becomes the `{}`
/// placeholder a route template already expresses.
///
/// Delegates to `configuration_agreement::compose` with no configuration values
/// supplied. The two were separate implementations of one rule — this one
/// produced the S-355 headline and that one the S-365 headline — so a change to
/// the placeholder rule in either would silently have made the two figures mean
/// different things.
fn placeholder_template(
    nodes: &[Node<'_>],
    src: &[u8],
    unit: &Unit<'_>,
    depth: usize,
) -> Option<String> {
    debug_assert_eq!(depth, FOLD_DEPTH, "compose folds at FOLD_DEPTH");
    let none: Vec<Option<configuration_agreement::Agreement>> =
        std::iter::repeat_with(|| None).take(nodes.len()).collect();
    configuration_agreement::compose(nodes, &none, src, unit, true)
}

// ── The measurement ─────────────────────────────────────────────────────────

#[derive(Default, Debug)]
struct LangStats {
    files_scanned: usize,
    files_gate_admitted: usize,
    emitted_today: usize,
    sites: Vec<Site>,
    /// S-374: what the arm actually *records*, read from the production
    /// `extract` pass rather than from this harness's mirrored site walk —
    /// keyed by source tree, because a refusal in an IT test class is not the
    /// coupling the acceptance criterion is written over.
    ///
    /// The grain is the **ledger row**, i.e. one per `(declaration, relation)`
    /// after `dedup_sort_refs`, which is the grain `workspace status` counts and
    /// therefore the only grain the ~111-row criterion can be checked at. Two
    /// composed calls in one method are one row.
    recorded: BTreeMap<(configuration_agreement::Tree, RowKind), usize>,
    /// Files carrying at least one recorded refusal, per tree — the "how spread
    /// out is it" figure a row count alone cannot give.
    refusal_files: BTreeMap<configuration_agreement::Tree, usize>,
}

/// Which population one `http-client-call` ledger row belongs to (S-374).
///
/// The arm writes exactly two shapes and they are told apart by the target, the
/// same way `federation::coverage::client_call_refusal` tells them apart: a
/// keyless row is a recorded `base-url-runtime` refusal, a keyed row is a
/// reference that named a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RowKind {
    /// A keyless row — the recorded refusal (`base-url-runtime`).
    Refusal,
    /// A keyed row — a `"METHOD /template"` reference.
    Reference,
}

#[derive(Default)]
struct Measurement {
    per_language: BTreeMap<String, LangStats>,
    /// Sites admitted under the strict reading (`final`/`const` bindings only).
    strict_const_admits: usize,
    /// S-365: the committed configuration sources and what they prove.
    config: configuration_agreement::ConfigCorpus,
    /// S-365: the `@ConfigurationProperties` classes the corpus declares.
    properties: configuration_agreement::PropertiesIndex,
    /// S-365: the broker-publish arm, keyed by language.
    broker: BTreeMap<String, configuration_agreement::BrokerStats>,
    /// S-365: the `.baseUrl(…)` sites [CR-115] is titled after, keyed by
    /// language. Reported separately from the path operands the headline
    /// counts — see `report_base_urls` for why the two must not be merged.
    base_urls: BTreeMap<String, Vec<configuration_agreement::BaseUrlSite>>,
}

/// Everything a file scan needs besides the file itself. A struct rather than
/// six more parameters: S-365 added the configuration corpus, the properties
/// index and the module scope to a signature that was already at its limit.
struct ScanCtx<'a> {
    plugin: &'a dyn LanguagePlugin,
    symbols: &'a SymbolContext,
    config: &'a configuration_agreement::ConfigCorpus,
    properties: &'a configuration_agreement::PropertiesIndex,
    /// The module root the file belongs to — the scope [CR-115] §3.4's
    /// agreement is taken over (see the S-365 module docs for why the
    /// workspace scope is reported alongside rather than instead).
    module: String,
}

impl<'a> ScanCtx<'a> {
    fn resolver(&'a self) -> configuration_agreement::Resolver<'a> {
        configuration_agreement::Resolver {
            corpus: self.config,
            props: self.properties,
            module: &self.module,
        }
    }
}

fn scan_file(rel: &str, source: &str, ctx: &ScanCtx<'_>, stats: &mut LangStats, strict: &mut usize) {
    let plugin = ctx.plugin;
    let Some(query) = plugin.query("invocations") else {
        return;
    };
    stats.files_scanned += 1;

    let facts = extract::extract(&FileInput::new(rel, source), plugin, ctx.symbols);
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
        .filter(|r| {
            r.relation == Some(ArtifactRelation::HttpClientCall) && !r.target.is_empty()
        })
        .count();

    // S-374: the arm's own recorded output, straight from the production pass.
    // Read here (not from the mirrored site walk below) so the reported figure is
    // what `workspace status` would count, not what this harness thinks it should.
    let tree = configuration_agreement::Tree::of(rel);
    let mut refusals_here = 0;
    for r in facts
        .refs
        .iter()
        .filter(|r| r.relation == Some(ArtifactRelation::HttpClientCall))
    {
        let kind = if r.target.trim().is_empty() {
            refusals_here += 1;
            RowKind::Refusal
        } else {
            RowKind::Reference
        };
        *stats.recorded.entry((tree, kind)).or_default() += 1;
    }
    if refusals_here > 0 {
        *stats.refusal_files.entry(tree).or_default() += 1;
    }

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
        let (kinds, folded, placeholder_folded, strictly) = classify_site(arg, src, &unit);
        // S-365 rides the same captured argument: `operands` is deterministic,
        // so re-decomposing costs a walk of one expression and keeps the S-355
        // path — `classify_site` — untouched.
        let mut nodes = Vec::new();
        operands(arg, src, &mut nodes);
        let judgement =
            configuration_agreement::judge(&nodes, &kinds, src, &unit, ctx.resolver(), true);
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
            placeholder_folded,
            gate_admitted: gate,
            key_outcomes: judgement.outcomes,
            cr115: judgement.verdict,
        };
        if gate && site.newly_admissible_with_placeholders() && strictly {
            *strict += 1;
        }
        stats.sites.push(site);
    }
}

/// The text an operand folds to, or `None` when it does not fold.
///
/// Recurses through same-unit bindings to the **same depth** [`classify`] uses,
/// and via the same [`operand_name`] reduction. The first draft resolved
/// exactly one hop with its own ad-hoc name handling, so a chained constant
/// (`ALIAS = BASE; BASE = "/api"`) classified as foldable yet folded to
/// nothing, and was silently dropped from the admissible count.
///
/// When a name is bound to **two different** literals in the same unit, this
/// returns `None`: the source does not prove which one the call site sees, and
/// picking whichever the traversal reached first would fold to an arbitrary
/// template. Such a site is `foldable()` but unfolded, and is reported
/// explicitly rather than quietly dropped (see [`Measurement::unfoldable`]).
fn folded_text(node: Node<'_>, src: &[u8], unit: &Unit<'_>, depth: usize) -> Option<String> {
    if let Some(text) = static_literal(node, src) {
        return Some(text);
    }
    if depth == 0 {
        return None;
    }
    let name = operand_name(node, src)?;
    let bindings = unit.bindings.get(&name)?;
    let candidates: std::collections::BTreeSet<String> = bindings
        .iter()
        .filter_map(|b| b.value)
        .filter_map(|v| folded_text(v, src, unit, depth - 1))
        .collect();
    match candidates.len() {
        1 => candidates.into_iter().next(),
        _ => None,
    }
}

/// Whether every non-literal operand resolves to a binding that carries a
/// const/final marker — the strict reading of [CR-113] §3.2 ("a reference to a
/// **constant**"), reported alongside the generous count as an independent
/// lower bound.
fn strictly_const(nodes: &[Node<'_>], src: &[u8], unit: &Unit<'_>) -> bool {
    nodes.iter().all(|n| {
        if static_literal(*n, src).is_some() {
            return true;
        }
        let Some(name) = operand_name(*n, src) else {
            return false;
        };
        unit.bindings
            .get(&name)
            .is_some_and(|bs| bs.iter().any(|b| b.is_const && b.value.is_some()))
    })
}

/// The configured corpus, or `None` when none is configured.
///
/// A variable that is **set but does not resolve to a directory** panics rather
/// than skipping: the two cases are indistinguishable to a reader of a green
/// test run, and a typo'd or un-checked-out corpus path would otherwise report
/// success while measuring nothing.
fn corpus_root() -> Option<PathBuf> {
    let raw = std::env::var("LOGOS_REF_WORKSPACE").ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = match raw.strip_prefix("~/") {
        Some(rest) => PathBuf::from(&home).join(rest),
        None if raw == "~" => PathBuf::from(&home),
        None => PathBuf::from(&raw),
    };
    assert!(
        expanded.is_dir(),
        "LOGOS_REF_WORKSPACE={raw} does not resolve to a directory (expanded: {}) — \
         refusing to report a green run that measured nothing",
        expanded.display(),
    );
    Some(expanded)
}

/// The corpus measurement, computed **once** per test binary.
///
/// S-355's test and S-365's read the same walk. `LOGOS_REF_WORKSPACE` is read
/// once per process, so the two callers always pass the same `root`; a second
/// root would silently reuse the first, which is why nothing else may call
/// this.
fn measurement(root: &Path) -> &'static Measurement {
    static ONCE: std::sync::OnceLock<Measurement> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| measure(root))
}

fn measure(root: &Path) -> Measurement {
    let registry = LanguageRegistry::load(root).expect("plugin registry loads");
    let symbols = SymbolContext::default();

    // S-365, pass one: the committed configuration sources, the module
    // partition, and the `@ConfigurationProperties` classes they bind. Both
    // must exist before a single call site is judged, so this is a separate
    // (cheap — parse-free except for the annotated classes) traversal rather
    // than a second walk of the source corpus.
    let config = configuration_agreement::ConfigCorpus::discover(root);
    // Every linked plugin that declares the `properties` capability indexes the
    // corpus's bound classes — the roster is the registry's, not a Java literal,
    // which is what makes a second language's binding vocabulary a descriptor
    // change rather than a harness change (S-381).
    let properties = configuration_agreement::PropertiesIndex::build(root, &config, &registry);
    let mut m = Measurement { config, properties, ..Measurement::default() };

    // `parents(false)` matches production's `admission_walk_builder`
    // (containment: never read ignore files above the root). `git_global` and
    // `.ignore` are switched OFF so the corpus is the same on every machine —
    // a developer's `~/.gitignore_global` must not quietly change a published
    // measurement.
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
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
        // S-365 widened the admission from `invocations` alone: a language that
        // ships `brokers` and not `invocations` carries the broker arm's corpus
        // and must be walked. `scan_file` still counts only files whose plugin
        // ships `invocations`, so the S-355 denominators are unchanged.
        if plugin.query("invocations").is_none() && plugin.query("brokers").is_none() {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let lang = plugin.name().to_string();
        let ctx = ScanCtx {
            plugin,
            symbols: &symbols,
            config: &m.config,
            properties: &m.properties,
            module: m.config.module_of(&rel).to_string(),
        };
        let stats = m.per_language.entry(lang.clone()).or_default();
        scan_file(&rel, &source, &ctx, stats, &mut m.strict_const_admits);
        let broker = m.broker.entry(lang.clone()).or_default();
        scan_broker_file(&rel, &source, &ctx, broker);
        let base_urls = scan_base_urls(&rel, &source, &ctx);
        if !base_urls.is_empty() {
            m.base_urls.entry(lang).or_default().extend(base_urls);
        }
    }
    m
}

/// The `.baseUrl(…)` sites of one file (S-365, [CR-115]'s other half).
fn scan_base_urls(
    rel: &str,
    source: &str,
    ctx: &ScanCtx<'_>,
) -> Vec<configuration_agreement::BaseUrlSite> {
    let plugin = ctx.plugin;
    let Some(query) = configuration_agreement::base_url_query(plugin.language()) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let unit = Unit::build(tree.root_node(), src);
    configuration_agreement::collect_base_urls(
        &query,
        tree.root_node(),
        src,
        &unit,
        rel,
        ctx.resolver(),
    )
}

/// The broker-publish arm of the same walk (S-365, [CR-117] §3.3).
///
/// Two readings of one file: what the **real** `brokers.scm` captures today
/// (the denominator of what the arm already emits), and the message-header
/// publish form it cannot see, whose topic operand is classified and judged by
/// exactly the same configuration rule the client arm uses.
fn scan_broker_file(
    rel: &str,
    source: &str,
    ctx: &ScanCtx<'_>,
    stats: &mut configuration_agreement::BrokerStats,
) {
    let plugin = ctx.plugin;
    let Some(brokers) = plugin.query("brokers") else {
        return;
    };
    stats.files_scanned += 1;

    let mut parser = Parser::new();
    if parser.set_language(plugin.language()).is_err() {
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        return;
    };
    let src = source.as_bytes();
    configuration_agreement::count_broker_captures(brokers, tree.root_node(), src, stats);

    let Some(header_query) = configuration_agreement::header_publish_query(plugin.language())
    else {
        return;
    };
    stats.header_form_supported = true;
    let unit = Unit::build(tree.root_node(), src);
    stats.sites.extend(configuration_agreement::collect_header_publishes(
        &header_query,
        tree.root_node(),
        src,
        &unit,
        rel,
        ctx.resolver(),
    ));
}

/// Print the measurement and return the headline newly-admitted count.
fn report(m: &Measurement) -> usize {
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
    }

    println!("\n--- what constant folding would NEWLY admit, per language ---");
    println!(
        "Headline reading is [FR-WS-18] AC1's: the prefix must fold, a trailing\n\
         non-foldable operand becomes the `{{}}` placeholder a route template already\n\
         expresses. The strict reading (every operand folds) is reported beside it.\n"
    );
    let mut total_ceiling = 0usize;
    for (lang, stats) in &m.per_language {
        let new: Vec<&Site> = stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.newly_admissible_with_placeholders())
            .collect();
        let strict_new = stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.newly_admissible())
            .count();
        let gated_sites = stats.sites.iter().filter(|s| s.gate_admitted).count();
        // The ceiling a widened ledger gate would expose — the figure the
        // recorded finding quotes, computed here so a re-run reproduces it.
        let ceiling: Vec<&Site> = stats
            .sites
            .iter()
            .filter(|s| !s.gate_admitted && s.newly_admissible_with_placeholders())
            .collect();
        total_new += new.len();
        total_ceiling += ceiling.len();
        println!(
            "{lang:<12} newly admitted: {:>3}  (strict reading: {strict_new})  \
             of {gated_sites} gate-admitted sites; {} behind the ledger gate, of which \
             {} would fold",
            new.len(),
            stats.sites.len() - gated_sites,
            ceiling.len(),
        );
        for site in new.iter().chain(ceiling.iter()) {
            println!(
                "               {} {}:{}  {}  ->  {}",
                if site.gate_admitted { "+" } else { "(behind gate)" },
                site.file,
                site.line,
                site.text,
                site.placeholder_folded.as_deref().unwrap_or("<unfolded>"),
            );
        }
    }
    println!(
        "\nTOTAL newly admitted, gate-admitted corpus (FR-WS-18 AC1 reading): {total_new}\
         \nTOTAL newly admitted (strict const-only reading): {}\
         \nTOTAL behind the ledger gate that would fold (ceiling): {total_ceiling}",
        m.strict_const_admits
    );

    // Never silently dropped: a site classified foldable that folding could not
    // reduce to one proven template is named, so it can be audited by hand.
    let unfolded: Vec<(&String, &Site)> = m
        .per_language
        .iter()
        .flat_map(|(lang, s)| s.sites.iter().map(move |site| (lang, site)))
        .filter(|(_, site)| site.foldable_but_unfolded())
        .collect();
    println!(
        "\n--- foldable by classification but not reducible to one proven template: {} ---",
        unfolded.len()
    );
    for (lang, site) in unfolded {
        println!("{lang}  {}:{}  {}", site.file, site.line, site.text);
    }

    println!("\n--- site census over the gate-admitted corpus (every site, auditable) ---");
    for (lang, stats) in &m.per_language {
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            let kinds: Vec<&str> = site.kinds.iter().map(|k| k.label()).collect();
            println!("{lang}  {}:{}  [{}]  {}", site.file, site.line, kinds.join(" + "), site.text);
        }
    }
    total_new
}

/// S-381 AC5: the descriptor-driven binding index over the reference workspace.
///
/// # Why a FLOOR on one figure and an EQUALITY on the other two
///
/// The three numbers are asserted differently on purpose. The class-name count
/// is a floor, because the estate grows and a later commit that binds one more
/// class is not a regression. The collision count is pinned exactly, because it
/// is the **refusal** half: a collision that quietly stops being one is a class
/// resolving to a guess, which is the failure [NFR-RA-05] is about and the
/// failure this index exists to prevent. It cost eleven false refusals when it
/// was wrong once already (see `PropertiesIndex`'s docs), so it is pinned in
/// both directions.
///
/// `prefixless` is pinned exactly too, and it is pinned because a floor alone is
/// not a guard: `len()` counts **distinct bound class names**, so a regression
/// that binds five new ones while dropping five to `prefixless` nets zero and
/// passes. That is not hypothetical — the prefix-scoping defect this story fixed
/// did exactly that, moving `prefixless` 0 → 9 while the class count fell, and
/// only a run that printed all three figures caught it.
///
/// # What this measured when S-381 promoted the index
///
/// `69 classes, 0 prefixless, 13 simple-name collisions` — **identical, class by
/// class**, to what the Java-grammar walk it replaced produced over the same
/// estate. That was checked by running both indexes side by side over
/// `~/source/pec-services` and comparing the full
/// `(name, module, file) → (prefix, properties)` map, not just the totals; the
/// two maps were equal and neither held a class the other did not. The probe was
/// a one-off — keeping a copy of the deleted walk beside its replacement is the
/// twin that drifts — and its result is recorded in the S-381 implementation
/// notes.
///
/// [NFR-RA-05]: ../../docs/specs/requirements/NFR-RA-05.md
#[test]
fn measure_configuration_binding_over_the_reference_workspace() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-381 configuration-binding measurement."
        );
        return;
    };
    let m = measurement(&root);
    let props = &m.properties;
    println!(
        "\nS-381 — configuration binding over {}\n  \
         {} classes indexed, {} annotated but prefixless, {} simple-name collisions refused",
        root.display(),
        props.len(),
        props.prefixless,
        props.collisions.len(),
    );
    for name in &props.collisions {
        println!("  collision: {name} (resolves to nothing outside its own module)");
    }

    assert!(
        props.len() >= 69,
        "expected at least 69 distinct bound class NAMES over {}, got {} — the \
         descriptor vocabulary or the `properties` query has stopped matching \
         what the Java-grammar walk this replaced matched. (Names, not \
         declarations: 13 of them are declared more than once.)",
        root.display(),
        props.len(),
    );
    assert_eq!(
        props.prefixless, 0,
        "every bound class on this estate carries a readable literal prefix; a \
         non-zero count means a prefix reading regressed — an annotation's \
         argument stopped being recognised, or a sibling annotation's argument \
         started competing with it",
    );
    assert_eq!(
        props.collisions.len(),
        13,
        "the estate's 13 known simple-name collisions must each resolve to \
         nothing; a lower count is a class resolving to a guess, a higher one is \
         a reading that has started splitting declarations that agree: {:?}",
        props.collisions,
    );
    for name in &props.collisions {
        assert!(
            props.get(name, "no-such-module").is_none(),
            "{name} is a collision but still resolves from outside its own module",
        );
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
    let m = measurement(&root);
    let total_new = report(m);

    // Cross-check the harness against the real pass: every reference the arm
    // actually emitted must correspond to a site the harness classified as a
    // single static literal. A drift in the mirrored literal reading fails here
    // instead of silently skewing the numbers.
    let mut engaged = false;
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
        engaged |= stats.emitted_today > 0;
    }
    // `0 <= 0` holds for any implementation, so a corpus where nothing was
    // emitted anywhere has not cross-checked the mirror at all. Say so rather
    // than banking a vacuous pass — the always-run `static_literal` fixtures are
    // what pin the mirror; this is the corpus-level confirmation.
    assert!(
        engaged,
        "the mirror cross-check never engaged: no language emitted a single \
         `http-client-call` reference over {}, so `0 <= 0` is all that was \
         asserted. Point LOGOS_REF_WORKSPACE at a corpus that binds at least \
         one client call, or treat this run as unverified.",
        root.display(),
    );
    assert!(
        !m.per_language.is_empty(),
        "the corpus at {} yielded no file in any language shipping `invocations`",
        root.display(),
    );

    // The recorded finding itself. Printing a table is not a test: without this
    // a classifier regression that flipped the verdict would still pass, and the
    // verdict is what blocks CR-113.
    assert_eq!(
        total_new, 0,
        "S-355's recorded finding is that constant folding newly admits ZERO \
         client-call sites across the reference workspace; this run found \
         {total_new}. Re-open CR-113 §8.1 and re-decide the CR before changing \
         this assertion — do not relax it to make the suite green.",
    );
}


/// **S-374 acceptance: the recorded-refusal figure, reconciled against the real
/// `pec-services` estate at a dedup-proof grain, with the production/test split
/// disclosed.** ([CR-120] §6, [FR-WS-08] AC2.)
///
/// [CR-120]'s criterion is "approximately **111 additional unbound rows** with
/// reason `base-url-runtime`, against a prior count of zero". Its CRA-01 sources
/// the 111 from `configuration_agreement_finding.txt`'s `client-call/main`
/// denominator — which is **this harness's own corpus of composed operands**,
/// not the population the shipped arm records. The two differ, and the whole
/// point of measuring here is to say by how much and why, rather than to assert
/// a number carried over from a different denominator.
///
/// # The grain, chosen so dedup cannot move it
///
/// The reported figure is the **ledger row**: one per `(declaration, relation)`
/// after `dedup_sort_refs`, which keys on `(source, target, form, kind,
/// relation)` and ignores `line`. Two composed calls in one method are one row,
/// and `workspace status` counts rows — so the row count is the only grain the
/// acceptance criterion can be checked at, and it is immune to the dedup by
/// construction. `refusal_files` is reported beside it so a row count
/// concentrated in a handful of classes cannot read as a broad one.
///
/// Every figure comes from the **production** `extract` pass, not from this
/// file's mirrored site walk. That matters here more than anywhere else in the
/// harness: the criterion is about what the arm records, and a mirror that
/// over-counted would inflate the very number the CR is graded on.
///
/// # Recorded finding (2026-09-08, `~/source/pec-services`, 84 members)
///
/// The per-language table is **not** copied here. It lives once, in
/// `operand_resolvability/client_call_refusal_finding.txt`, which this file
/// embeds with `include_str!` and this test prints — so it cannot be deleted or
/// renamed without breaking compilation, and there is no second hand-maintained
/// copy to rot out of step with it.
///
/// **The criterion holds on the production population: 115 against ~111.** The
/// whole-workspace figure an index run would print is **131**, because indexing
/// does not split main from test; both are stated and neither is the other. The
/// agreement at 115 is not one measurement arriving twice — 111 was a *site*
/// count over the composed-operand corpus, 115 is a *row* count over what the arm
/// records, and two offsetting effects put them within four rows: Java's 94 sites
/// barely collapse (one `.uri(…)` chain per method, and all 94 refuse — the same
/// 94 the S-375 receiver gate recorded), while Go's 98 collapse to 36.
///
/// The full reasoning, the three excluded populations, and why PHP's and TSX's
/// zeros are honest absence rather than a regression are recorded in
/// `operand_resolvability/client_call_refusal_finding.txt` — the durable artifact
/// beside `configuration_agreement_finding.txt`, kept in the same form for the
/// same reason.
///
/// # What is asserted, and what is only reported
///
/// Asserted: refusals exist at all — the prior count was zero, so a run that
/// records none has not delivered the story. Reported without assertion: the
/// counts themselves, because they are a property of the corpus rather than of
/// the code, and pinning a corpus figure in an assertion is how a measurement
/// becomes a thing to be made green. (Row *shape* — keyless, inert, one per
/// declaration — is pinned by fixtures that run without a corpus, in
/// `extract::tests` and `xservice_http_client_call`; nothing here rests on the
/// corpus for that.)
///
/// [CR-120]: ../../docs/requests/CR-120-invocation-arms-report-their-own-refusals.md
/// [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
#[test]
fn measure_recorded_client_call_refusals_over_the_reference_workspace() {
    use configuration_agreement::Tree;

    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-374 recorded-refusal measurement."
        );
        return;
    };
    let m = measurement(&root);

    println!("\nS-374 — recorded HTTP client-call refusals ({})", root.display());
    println!("  grain: LEDGER ROW (one per declaration; `dedup_sort_refs` ignores line)\n");
    println!(
        "  {:<12} {:>6} {:>6} {:>9} {:>9} {:>7} {:>7}",
        "language", "files", "gated", "refuse/main", "refuse/test", "refs", "sites*"
    );
    let mut totals: BTreeMap<(Tree, RowKind), usize> = BTreeMap::new();
    let mut total_files: BTreeMap<Tree, usize> = BTreeMap::new();
    for (lang, stats) in &m.per_language {
        let row = |tree: Tree, kind: RowKind| {
            stats.recorded.get(&(tree, kind)).copied().unwrap_or(0)
        };
        for (key, count) in &stats.recorded {
            *totals.entry(*key).or_default() += count;
        }
        for (tree, count) in &stats.refusal_files {
            *total_files.entry(*tree).or_default() += count;
        }
        println!(
            "  {:<12} {:>6} {:>6} {:>9} {:>9} {:>7} {:>7}",
            lang,
            stats.files_scanned,
            stats.files_gate_admitted,
            row(Tree::Main, RowKind::Refusal),
            row(Tree::Test, RowKind::Refusal),
            row(Tree::Main, RowKind::Reference) + row(Tree::Test, RowKind::Reference),
            stats.sites.iter().filter(|s| s.gate_admitted).count(),
        );
    }
    let refusals = |tree: Tree| totals.get(&(tree, RowKind::Refusal)).copied().unwrap_or(0);
    let references = |tree: Tree| totals.get(&(tree, RowKind::Reference)).copied().unwrap_or(0);
    let files = |tree: Tree| total_files.get(&tree).copied().unwrap_or(0);
    println!(
        "\n  TOTAL refusal rows: main {} (in {} files), test {} (in {} files)",
        refusals(Tree::Main),
        files(Tree::Main),
        refusals(Tree::Test),
        files(Tree::Test),
    );
    println!(
        "  TOTAL references:   main {}, test {}",
        references(Tree::Main),
        references(Tree::Test),
    );
    println!(
        "  * `sites` is THIS harness's mirrored, gate-admitted query-match count — \n             reported for context only. A site refused at QUERY-MATCH time (a stated\n             capture ceiling, a receiver the S-375 rule declines) is in neither column:\n             it leaves no site, so it can carry no reason. See\n             `extract::capture_http_client_call_arm` for the enumeration.\n"
    );

    println!("--- recorded finding ---\n{RECORDED_REFUSAL_FINDING}");

    // The one thing that must hold whatever the corpus contains: the prior count
    // was zero, so a run recording nothing has not delivered the story.
    assert!(
        refusals(Tree::Main) + refusals(Tree::Test) > 0,
        "the arm recorded NO refusal anywhere over {} — before S-374 the count was \
         zero and the whole story is that it no longer is. Either the corpus holds \
         no composed client call (check the `sites` column) or the refusal path \
         regressed.",
        root.display(),
    );
}

// ── Classifier fixtures ─────────────────────────────────────────────────────
//
// These pin the classifier's rules on every `cargo test` run, corpus or no
// corpus. They matter more than usual: the measurement above skips without a
// corpus, so without these the file would pin nothing in CI — and the numbers
// it produces are what block [CR-113].
//
// Every fixture goes through `classify_site`, the same entry point the corpus
// walk uses, so a fixture cannot pass over logic the measurement does not run.

/// One analysed call site, produced from a whole compilation unit.
struct Analysed {
    kinds: Vec<OperandKind>,
    folded: Option<String>,
    placeholder: Option<String>,
    strictly_const: bool,
    site: Site,
}

/// Parse `source` as `filename`, run that language's **real** `invocations`
/// query, and classify the single call site it must contain.
fn analyse(filename: &str, source: &str) -> Analysed {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry loads");
    let plugin = registry
        .for_path(filename)
        .unwrap_or_else(|| panic!("no plugin for {filename}"));
    let query = plugin
        .query("invocations")
        .unwrap_or_else(|| panic!("{filename}: plugin ships no `invocations` query"));
    let mut parser = Parser::new();
    parser.set_language(plugin.language()).expect("language");
    let tree = parser.parse(source, None).expect("parse");
    let src = source.as_bytes();
    let unit = Unit::build(tree.root_node(), src);
    let sites = collect_sites(
        query,
        tree.root_node(),
        src,
        &plugin.semantics().invocation_methods,
    );
    assert_eq!(
        sites.len(),
        1,
        "fixture must yield exactly one client-call site, got {} in:\n{source}",
        sites.len(),
    );
    let (line, arg) = sites[0];
    let (kinds, folded, placeholder, strictly) = classify_site(arg, src, &unit);
    Analysed {
        site: Site {
            file: filename.to_string(),
            line,
            text: arg.utf8_text(src).unwrap_or_default().to_string(),
            kinds: kinds.clone(),
            folded: folded.clone(),
            placeholder_folded: placeholder.clone(),
            gate_admitted: true,
            // S-355's fixtures assert folding, not configuration resolution;
            // the S-365 fixtures drive `judge` through its own entry point.
            key_outcomes: Vec::new(),
            cr115: configuration_agreement::Verdict::NotConfigurationBound,
        },
        kinds,
        folded,
        placeholder,
        strictly_const: strictly,
    }
}

// ── Taxonomy invariants (no grammar needed) ─────────────────────────────────

/// Only literals and same-unit constants fold. The boundary [CR-113] §3.2
/// draws, stated as a list rather than as the implementation's own expression.
#[test]
fn foldability_is_exactly_literal_and_same_unit_constant() {
    let foldable: Vec<OperandKind> = OperandKind::ALL
        .iter()
        .copied()
        .filter(|k| k.is_foldable())
        .collect();
    assert_eq!(
        foldable,
        vec![OperandKind::Literal, OperandKind::SameUnitConstant],
    );
}

/// The taxonomy is ordered most-resolvable-first, which is what makes `min()`
/// over candidate bindings the reading most favourable to CRA-01.
#[test]
fn the_taxonomy_is_ordered_most_resolvable_first() {
    let mut sorted = OperandKind::ALL;
    sorted.sort();
    assert_eq!(sorted, OperandKind::ALL);
    assert!(OperandKind::Literal < OperandKind::Other);
}

// ── Java ────────────────────────────────────────────────────────────────────

#[cfg(feature = "lang-java")]
fn java(prelude: &str, expr: &str) -> Analysed {
    analyse(
        "Calls.java",
        &format!(
            "package com.example;\n\
             import org.springframework.web.reactive.function.client.WebClient;\n\
             public class Calls {{\n{prelude}\n  \
             void call(String id) {{ client.get().uri({expr}); }}\n}}\n"
        ),
    )
}

/// A single static literal — what the arm already admits; folding adds nothing.
#[cfg(feature = "lang-java")]
#[test]
fn a_static_literal_is_already_admitted_not_newly() {
    let a = java("", r#""/users/{id}""#);
    assert_eq!(a.kinds, vec![OperandKind::Literal]);
    assert!(a.site.already_static_literal());
    assert!(!a.site.newly_admissible());
    assert!(!a.site.newly_admissible_with_placeholders());
}

/// [CR-113]'s motivating shape, and the **positive control** for the whole
/// measurement: if this did not come out admissible, a zero corpus result would
/// prove nothing about the corpus.
#[cfg(feature = "lang-java")]
#[test]
fn a_same_unit_constant_prefix_is_newly_admissible() {
    let a = java(r#"  private static final String BASE = "/api/v1";"#, r#"BASE + "/soggetti""#);
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/v1/soggetti"));
    assert!(a.site.newly_admissible());
    assert!(a.site.newly_admissible_with_placeholders());
    assert!(a.strictly_const);
}

/// [FR-WS-18] AC1's exact shape — `CONST + "/literal/" + param`. The statement
/// says a parameter interpolation "is not an obstacle: it is the `{id}`
/// placeholder a route template already expresses", so it must be admissible
/// under the headline reading even though `id` itself never folds.
#[cfg(feature = "lang-java")]
#[test]
fn the_fr_ws_18_ac1_parameter_shape_is_newly_admissible() {
    let a = java(
        r#"  private static final String BASE = "/api/v1";"#,
        r#"BASE + "/soggetti/" + id"#,
    );
    assert_eq!(
        a.kinds,
        vec![
            OperandKind::SameUnitConstant,
            OperandKind::Literal,
            OperandKind::Other,
        ],
    );
    // The strict reading refuses it (not every operand folds) …
    assert!(!a.site.newly_admissible());
    // … while AC1's reading admits it, with the parameter as a placeholder.
    assert_eq!(a.placeholder.as_deref(), Some("/api/v1/soggetti/{}"));
    assert!(a.site.newly_admissible_with_placeholders());
}

/// A bare same-unit constant reference — the degenerate one-operand
/// composition, and the only foldable shape the reference workspace actually
/// contributes.
#[cfg(feature = "lang-java")]
#[test]
fn a_bare_same_unit_constant_folds_to_its_literal() {
    let a = java(r#"  private static final String PATH = "/v1/alerts";"#, "PATH");
    assert_eq!(a.kinds, vec![OperandKind::SameUnitConstant]);
    assert_eq!(a.folded.as_deref(), Some("/v1/alerts"));
    assert!(a.site.newly_admissible());
}

/// Folding follows a chain of same-unit bindings. `classify` and `folded_text`
/// must agree about how far they reach: the first draft classified this
/// foldable and then folded it to nothing, silently dropping the site.
#[cfg(feature = "lang-java")]
#[test]
fn folding_and_classification_reach_the_same_distance() {
    let a = java(
        "  private static final String ROOT = \"/api\";\n  \
         private static final String BASE = ROOT;",
        r#"BASE + "/x""#,
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/x"));
    assert!(
        !a.site.foldable_but_unfolded(),
        "a foldable site must fold, or be reported as unfoldable",
    );
}

/// An absolute *URL* folds, but stays refused: its route prefix is external
/// ([FR-WS-08] AC2 / `classify_client_call`'s leading-`/` rule, which [CR-113]
/// §3.2 does not propose changing).
#[cfg(feature = "lang-java")]
#[test]
fn an_absolute_url_folds_but_is_not_newly_admissible() {
    let a = java(
        r#"  private static final String BASE = "http://pec-anagrafica/api/v1";"#,
        r#"BASE + "/soggetti""#,
    );
    assert!(a.site.foldable());
    assert_eq!(a.folded.as_deref(), Some("http://pec-anagrafica/api/v1/soggetti"));
    assert!(!a.site.newly_admissible());
    assert!(!a.site.newly_admissible_with_placeholders());
}

/// A relative folded template is likewise refused — the route prefix is
/// composed elsewhere.
#[cfg(feature = "lang-java")]
#[test]
fn a_relative_folded_template_is_not_newly_admissible() {
    let a = java(r#"  private static final String BASE = "v1";"#, r#"BASE + "/users""#);
    assert!(a.site.foldable());
    assert!(!a.site.newly_admissible_with_placeholders());
}

/// A `@Value`-injected base is a configuration lookup — refused, and the
/// boundary [CR-113] §3.3 exists to preserve.
#[cfg(feature = "lang-java")]
#[test]
fn a_value_injected_base_is_a_configuration_lookup() {
    let a = java(
        "  @Value(\"${service.base}\")\n  private String base;",
        r#"base + "/users""#,
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::ConfigurationLookup, OperandKind::Literal],
    );
    assert!(!a.site.newly_admissible_with_placeholders());
}

/// A configuration-properties bean's getter — the reference workspace's
/// dominant shape, 81 of its 98 gate-admitted Java sites.
#[cfg(feature = "lang-java")]
#[test]
fn a_properties_bean_getter_is_a_configuration_lookup() {
    let a = java(
        "  private final MailboxApiProperties mailboxApiProperties;",
        "mailboxApiProperties.getUriGetMailbox()",
    );
    assert_eq!(a.kinds, vec![OperandKind::ConfigurationLookup]);
}

/// A plain helper-method call is a method return, not a foldable operand.
#[cfg(feature = "lang-java")]
#[test]
fn a_helper_method_call_is_a_method_return() {
    assert_eq!(
        java("", "buildCreateMailboxUrl()").kinds,
        vec![OperandKind::MethodReturn],
    );
}

/// An injected collaborator field, never initialised in the unit.
#[cfg(feature = "lang-java")]
#[test]
fn an_uninitialised_field_is_injected() {
    assert_eq!(
        java("  private String basePath;", "basePath").kinds,
        vec![OperandKind::Injected],
    );
}

/// [FR-WS-18] AC4: folding never recurses into a value the source does not
/// prove. A constant initialised from a **plain** call — one no name rule can
/// rescue — must not fold; this fails if recursion refusal is removed.
#[cfg(feature = "lang-java")]
#[test]
fn a_constant_initialised_from_a_call_does_not_fold() {
    let a = java(
        "  private static final String BASE = buildBase();",
        r#"BASE + "/users""#,
    );
    assert_eq!(a.kinds, vec![OperandKind::MethodReturn, OperandKind::Literal]);
    assert!(a.folded.is_none());
    assert!(!a.site.newly_admissible_with_placeholders());
}

/// A constant initialised from an environment read is a configuration lookup.
#[cfg(feature = "lang-java")]
#[test]
fn a_getenv_initialised_constant_is_a_configuration_lookup() {
    let a = java(
        "  private static final String BASE = System.getenv(\"BASE\");",
        r#"BASE + "/users""#,
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::ConfigurationLookup, OperandKind::Literal],
    );
}

/// A method **parameter** is `Other`, whatever the enclosing method's body
/// happens to mention. Pins the corpus correction that moved four `.uri(uri, …)`
/// sites out of the `configuration lookup` column: reading the whole enclosing
/// declaration let an incidental `properties` local reclassify a parameter.
#[cfg(feature = "lang-java")]
#[test]
fn a_parameter_is_other_even_in_a_method_mentioning_properties() {
    let a = analyse(
        "Calls.java",
        "package com.example;\n\
         import org.springframework.web.reactive.function.client.WebClient;\n\
         public class Calls {\n  \
         void call(String uri) {\n    \
         String properties = \"config settings\";\n    \
         client.get().uri(uri);\n  }\n}\n",
    );
    assert_eq!(a.kinds, vec![OperandKind::Other]);
}

/// A `uriBuilder -> …` lambda is `Other` — 9 of the corpus's Java sites.
#[cfg(feature = "lang-java")]
#[test]
fn a_uri_builder_lambda_is_other() {
    let a = java("", "builder -> builder.path(\"/x\").build()");
    assert_eq!(a.kinds, vec![OperandKind::Other]);
}

/// A name the unit does not bind is `Other` — never guessed.
#[cfg(feature = "lang-java")]
#[test]
fn an_unbound_name_is_other() {
    assert_eq!(java("", "unknownVar").kinds, vec![OperandKind::Other]);
}

/// The strict reading requires an actual const marker. `static` alone does not
/// make a Java field immutable, so it must not satisfy the strict counter.
#[cfg(feature = "lang-java")]
#[test]
fn the_strict_reading_requires_a_const_marker() {
    assert!(java(r#"  private static final String B = "/api";"#, "B").strictly_const);
    assert!(!java(r#"  private static String B = "/api";"#, "B").strictly_const);
    assert!(!java(r#"  private String b = "/api";"#, "b").strictly_const);
}

/// A name bound to two different literals folds to neither: the source does not
/// prove which one the call site sees.
#[cfg(feature = "lang-java")]
#[test]
fn a_name_bound_to_two_literals_does_not_fold() {
    let a = analyse(
        "Calls.java",
        "package com.example;\n\
         import org.springframework.web.reactive.function.client.WebClient;\n\
         public class Calls {\n  \
         private static String b = \"/api/v1\";\n  \
         void mutate() { b = \"/other\"; }\n  \
         void call() { client.get().uri(b); }\n}\n",
    );
    assert!(a.folded.is_none(), "folded to {:?}", a.folded);
    assert!(!a.site.newly_admissible_with_placeholders());
    assert!(
        a.site.foldable_but_unfolded(),
        "such a site must be reported, not silently dropped",
    );
}

// ── The mirrored literal reading ────────────────────────────────────────────

/// `static_literal` must reproduce `extract::static_string_literal`'s tail:
/// an empty or whitespace-only literal is **refused**, and a padded one is
/// trimmed. The first draft returned `Some("")` and untrimmed text, which both
/// inflated the `literal` column and could suppress an admissible site.
#[cfg(feature = "lang-java")]
#[test]
fn the_mirrored_literal_reading_trims_and_rejects_empty() {
    let registry = LanguageRegistry::load(std::env::temp_dir()).expect("registry");
    let plugin = registry.for_path("Calls.java").expect("java plugin");
    let mut parser = Parser::new();
    parser.set_language(plugin.language()).expect("language");

    let cases: [(&str, Option<&str>); 4] = [
        (r#""""#, None),
        (r#""   ""#, None),
        (r#""  /users  ""#, Some("/users")),
        (r#""/users/{id}""#, Some("/users/{id}")),
    ];
    for (literal, expected) in cases {
        let source = format!("class C {{ String s = {literal}; }}");
        let tree = parser.parse(&source, None).expect("parse");
        let src = source.as_bytes();
        // The declarator's value is the string node.
        let mut found = None;
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if node.kind().contains("string") && node.kind() != "string_fragment" {
                found = Some(node);
                break;
            }
            let mut c = node.walk();
            stack.extend(node.named_children(&mut c));
        }
        let node = found.expect("a string literal node");
        assert_eq!(
            static_literal(node, src).as_deref(),
            expected,
            "literal {literal}",
        );
    }
}

// ── Go — the arm whose constants the first draft could not resolve ──────────

/// Go binds a `const`/`var` through an `expression_list`, not directly. The
/// first draft read `child_by_field_name("value")`, got the list, failed the
/// string test and classified **every** Go same-unit constant as `Other` —
/// invalidating the Go column of a published table.
#[cfg(feature = "lang-go")]
#[test]
fn a_go_const_resolves_as_a_same_unit_constant() {
    let a = analyse(
        "client.go",
        "package main\n\nimport \"net/http\"\n\n\
         const BasePath = \"/api/v1\"\n\n\
         func call() { http.Get(BasePath + \"/users\") }\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/v1/users"));
}

/// The same for a `var`, and for a `:=` short declaration.
#[cfg(feature = "lang-go")]
#[test]
fn a_go_var_and_short_declaration_resolve() {
    let a = analyse(
        "client.go",
        "package main\n\nimport \"net/http\"\n\n\
         var VarBase = \"/api/v2\"\n\n\
         func call() { http.Get(VarBase + \"/things\") }\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );

    let b = analyse(
        "client.go",
        "package main\n\nimport \"net/http\"\n\n\
         func call() {\n\tbase := \"/api/v3\"\n\thttp.Get(base + \"/things\")\n}\n",
    );
    assert_eq!(
        b.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
}

/// A multi-name `const A, B = "x", "y"` pairs positionally rather than binding
/// every name to the first value.
#[cfg(feature = "lang-go")]
#[test]
fn a_go_multi_name_const_pairs_positionally() {
    let a = analyse(
        "client.go",
        "package main\n\nimport \"net/http\"\n\n\
         const A, B = \"/first\", \"/second\"\n\n\
         func call() { http.Get(B) }\n",
    );
    assert_eq!(a.kinds, vec![OperandKind::SameUnitConstant]);
    assert_eq!(a.folded.as_deref(), Some("/second"));
}

// ── Python ──────────────────────────────────────────────────────────────────

/// Python spells a qualified access `attribute`, which matched none of the
/// first draft's kind tests, so `self.BASE` classified as `Other`.
#[cfg(feature = "lang-python")]
#[test]
fn a_python_self_attribute_resolves() {
    let a = analyse(
        "client.py",
        "import requests\n\n\
         class C:\n    BASE = \"/api/v1\"\n\n    \
         def call(self):\n        requests.get(self.BASE + \"/users\")\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
}

/// An f-string decomposes into its fragments and substitutions.
#[cfg(feature = "lang-python")]
#[test]
fn a_python_f_string_decomposes() {
    let a = analyse(
        "client.py",
        "import requests\n\n\
         BASE = \"/api/v1\"\n\n\
         def call():\n    requests.get(f\"{BASE}/users\")\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/v1/users"));
}

/// A keyword argument is **not** a binding. Under a "any node with a `name`
/// field" rule, `helper(path="/injected")` registered a binding for `path` and
/// folded an unrelated parameter of the same name to it — a fabricated
/// admission, the one direction a measurement must never err in.
#[cfg(feature = "lang-python")]
#[test]
fn a_python_keyword_argument_does_not_bind_a_name() {
    let a = analyse(
        "client.py",
        "import requests\n\n\
         def call(path):\n    helper(path=\"/injected/literal\")\n    \
         requests.get(path + \"/tail\")\n",
    );
    assert_eq!(a.kinds, vec![OperandKind::Other, OperandKind::Literal]);
    assert!(!a.site.newly_admissible_with_placeholders());
}

// ── TypeScript ──────────────────────────────────────────────────────────────

/// A template string with a substitution decomposes; one without is a single
/// literal the arm already admits.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_typescript_template_string_decomposes() {
    let a = analyse(
        "client.ts",
        "import axios from 'axios';\n\
         const BASE = '/api/v1';\n\
         export async function call() { await axios.get(`${BASE}/users`); }\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/v1/users"));

    let b = analyse(
        "client.ts",
        "import axios from 'axios';\n\
         export async function call() { await axios.get(`/api`); }\n",
    );
    assert_eq!(b.kinds, vec![OperandKind::Literal]);
    assert!(
        b.site.already_static_literal(),
        "a no-substitution template literal is admitted TODAY, so folding adds nothing",
    );
}

/// `process.env` is a configuration lookup wherever it appears.
#[cfg(feature = "lang-typescript")]
#[test]
fn a_typescript_process_env_base_is_a_configuration_lookup() {
    let a = analyse(
        "client.ts",
        "import axios from 'axios';\n\
         export async function call() { await axios.get(`${process.env.API_HOST}/contracts`); }\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::ConfigurationLookup, OperandKind::Literal],
    );
    assert!(!a.site.newly_admissible_with_placeholders());
}

// ── PHP ─────────────────────────────────────────────────────────────────────

/// PHP concatenates with `.`, spells a class constant `self::BASE`, and gives a
/// property's initialiser the field name `default_value` — three shapes the
/// first draft classified as `Other`, leaving its PHP row vacuous rather than
/// confirming.
#[cfg(feature = "lang-php")]
#[test]
fn a_php_class_constant_concatenation_resolves() {
    let a = analyse(
        "Client.php",
        "<?php\nuse GuzzleHttp\\Client;\n\
         class C {\n  const BASE = '/api/v1';\n  \
         function call($client) { $client->get(self::BASE . '/users'); }\n}\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
    assert_eq!(a.folded.as_deref(), Some("/api/v1/users"));
}

/// A property initialiser (`default_value`) is a same-unit constant, not an
/// injected field.
#[cfg(feature = "lang-php")]
#[test]
fn a_php_property_initialiser_is_not_injected() {
    let a = analyse(
        "Client.php",
        "<?php\nuse GuzzleHttp\\Client;\n\
         class C {\n  private $base = '/legacy';\n  \
         function call($client) { $client->get($this->base . '/users'); }\n}\n",
    );
    assert_eq!(
        a.kinds,
        vec![OperandKind::SameUnitConstant, OperandKind::Literal],
    );
}
