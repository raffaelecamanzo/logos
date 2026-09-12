//! **S-365 — configuration-key resolvability and profile agreement**
//! ([CR-115] CRA-01, [CR-117] CRA-01, [FR-WS-08], [FR-WS-10], [FR-SY-11]).
//!
//! The S-355 measurement in the parent module ended where the values stop being
//! in the code: **81 of 98** Java client-call sites resolve to a *configuration
//! lookup* — a getter on a cross-unit `@ConfigurationProperties` bean whose
//! value lives in `application.yml`. [CR-115] proposes reading those sources.
//! [CR-117] §3.3 proposes the same mechanism for a broker publish site's topic.
//! Both are gated on the same question, and this module answers it:
//!
//! > Of the sites that resolve to a configuration key, how many resolve to a
//! > key on whose value **every committed source agrees**?
//!
//! [CR-115] §3.4 makes agreement the admission rule, not a tie-break: a key
//! several sources define differently is **refused**, naming the key and the
//! conflicting files, rather than defaulted to the unprofiled value. So the
//! newly-admitted count is bounded by agreement, and agreement is what is
//! measured here.
//!
//! # One gate, two arms
//!
//! The two corpora are measured by one run, reported **separately and
//! combined, never averaged** — a material figure for one arm and an immaterial
//! figure for the other is a real outcome, and [CR-115]/[CR-117] are then
//! decided differently from each other. The arms share this module's key
//! resolution and agreement rule verbatim, which is the point: two measurements
//! drifting to two answers about one `@ConfigurationProperties` mechanism is
//! exactly what [CR-117] §3.3 asks to be avoided.
//!
//! | arm | corpus (the denominator) | admitted today |
//! |-----|--------------------------|----------------|
//! | client call | a verb-anchored `invocations` match inside a ledger-gate-admitted file (S-355's corpus) whose least-resolvable operand is a configuration lookup | a single static literal |
//! | broker publish | a **message-header publish form** — `setHeader(KafkaHeaders.TOPIC, <operand>)` — in a file of a language shipping `brokers` | a single static literal: since S-370 the real `brokers.scm` recognises the header form and admits a literal operand there on its own. A configuration-bound operand is still refused, and that is what this measurement counts |
//!
//! The denominators differ and are printed separately for that reason. The
//! broker arm has **no ledger gate** (`brokers.scm` is not detector-gated) and
//! no already-admitted subset, so its ratio is not comparable to the client
//! arm's by construction.
//!
//! ## Why the broker arm is the header form only
//!
//! `brokers.scm`'s other publish pattern — `send`/`convertAndSend`/`publish`
//! with a literal first argument — was **deliberately not** widened to
//! non-literal first arguments for this measurement. Lifting the literal
//! constraint matches every `.send(x)` in the corpus, including
//! `kafkaTemplate.send(message)` one line below a header-form publish, and the
//! resulting denominator would be noise. [CR-117] §3.3 scopes the capture work
//! to the header form; the measurement scopes to the same thing, and says so
//! rather than reporting a bigger number over a corpus it cannot defend.
//!
//! # What "resolves to a configuration key" means
//!
//! Spring's binding is a two-hop lookup and each hop can fail, so each failure
//! is a named [`Refusal`] rather than a silent drop ([NFR-CC-04]):
//!
//! ```text
//! mailServerConfigurationApi.getUriGetArchive()
//!   │                        └─ getter → property `uriGetArchive`
//!   └─ field of declared type `MailServerConfigurationApi`
//!        └─ @ConfigurationProperties(prefix = "mailserver.api")
//!             → key `mailserver.api.uri-get-archive`
//! ```
//!
//! A `@Value("${key:default}")`-annotated name resolves in one hop. An
//! environment read (`System.getenv`, `process.env`, `os.Getenv`) resolves to
//! the variable's name — deliberately, so it can be looked up and reported as
//! *not defined by any committed source* rather than as an unrecognised shape.
//!
//! Keys are compared under Spring's **relaxed binding**: each `.`-segment is
//! lower-cased with `-` and `_` removed, so `uri-get-archive`, `uriGetArchive`
//! and `URI_GET_ARCHIVE` are one key.
//!
//! # Which sources count
//!
//! `application.yml`, `application.yaml`, `application.properties` and every
//! `application-<profile>` variant of those three extensions, discovered by the
//! same [FR-SY-11] admission walk the parent module's corpus scan uses
//! (`.gitignore` honoured, nested-git boundaries pruned, no machine-local
//! ignore files). Test resources are **not** excluded: they are committed
//! sources that define the key, and [CR-115] §3.4's rule says *every discovered
//! source*.
//!
//! ## Two scopes, both reported
//!
//! Agreement is computed twice and the difference is itself a finding:
//!
//! - **module scope** (the headline) — sources under the call site's nearest
//!   ancestor holding a build descriptor (`pom.xml`, `build.gradle{,.kts}`,
//!   `package.json`, `go.mod`). This approximates the classpath one deployable
//!   actually assembles, which is the scope [CR-115]'s base-URL rule is about.
//! - **workspace scope** — every source in the corpus. This is the scope
//!   [CR-117]'s *canonical topic identity* needs, because a publish in one
//!   member must meet a subscribe in another.
//!
//! # Which tables stay here, and which have left
//!
//! `HEADER_PUBLISH_QUERY`, `BASE_URL_METHODS` and [`names_topic_header`] are
//! Spring-API-coupled measurement tables. They fall under the parent harness's
//! carve-out (see its module docs) and **must not be lifted into
//! `logos-core/src/resolve/`**, which
//! `resolve::framework::tests::jvm_parity::no_language_specific_composition_code_exists`
//! forbids. A measurement is allowed to know what a framework looks like, a
//! resolver is not.
//!
//! The other two named here until Sprint 67 have **left, by the route that
//! paragraph prescribes** rather than in spite of it: the relaxed-binding rules
//! went to `extract::config::corpus` (S-380) and the `@ConfigurationProperties`
//! index to `extract::config::binding` (S-381), and neither carried its Spring
//! vocabulary with it. The vocabulary now lives in
//! `plugins/<lang>/queries/properties.scm` and the descriptor's `[properties]`
//! table as pure plugin data ([ADR-54]), which is exactly what "the real arm's
//! capture belongs in the plugin" asked for — so this is a promotion under the
//! rule, not an exception to it.
//!
//! [ADR-54]: ../../docs/specs/architecture/decisions/ADR-54.md
//!
//! # Materiality is declared before the run, not after it
//!
//! [CR-113] closed because folding admitted zero. "Immaterial" must not be a
//! number chosen once the number is known, so the floor is a constant here:
//! a mechanism that recovers fewer than [`MATERIAL_FLOOR_PCT`]% of the sites it
//! targets, or fewer than [`MATERIAL_FLOOR_SITES`] sites outright, repeats
//! [CR-113]'s outcome and the change request should close the same way.
//!
//! # Recorded finding (2026-09-07, `~/source/pec-services`, 84 members)
//!
//! **[CR-115] CRA-01 holds. [CR-117] CRA-01 is falsified on production source.**
//! The two arms answer differently, and the split that decides them is one
//! level deeper than the arms: **production source versus test source**. The
//! broker arm admits 38 of 54 overall — and every one of the 38 is an IT test
//! class, while all 13 of its production publish sites are refused.
//!
//! [`RECORDED_FINDING`] carries the full text and the tables; it is
//! `include_str!`-ed so the run prints exactly what the documents quote, rather
//! than a second hand-maintained copy that can drift from it. The verdict —
//! per arm, read off the production row — is asserted in
//! [`measure_configuration_agreement_over_the_reference_workspace`].
//!
//! [CR-113]: ../../docs/requests/CR-113-constant-folded-base-url-composition.md
//! [CR-115]: ../../docs/requests/CR-115-configuration-bound-base-url-resolution.md
//! [CR-117]: ../../docs/requests/CR-117-broker-publish-capture-and-the-topic-key-namespace.md
//! [FR-SY-11]: ../../docs/specs/requirements/FR-SY-11.md
//! [FR-WS-08]: ../../docs/specs/requirements/FR-WS-08.md
//! [FR-WS-10]: ../../docs/specs/requirements/FR-WS-10.md
//! [NFR-CC-04]: ../../docs/specs/requirements/NFR-CC-04.md

use std::collections::{BTreeMap, BTreeSet};

use tree_sitter::Node;

// The promoted corpus (S-380): the flattener, the canonical key and the profile
// rule this harness grew, now production code under `extract::config::corpus`.
pub use logos_core::extract::config::corpus::{
    canonical_key, config_profile, parse_properties, parse_yaml, ConfigCorpus, ConfigSource,
};
pub use logos_core::graph_store::ConfigDefinition;

use super::{folded_text, operand_name, static_literal, OperandKind, Unit, FOLD_DEPTH};

// ── Declared thresholds ─────────────────────────────────────────────────────

/// Percent of an arm's own denominator below which the mechanism is immaterial.
pub const MATERIAL_FLOOR_PCT: usize = 10;

/// Absolute site count below which the mechanism is immaterial whatever the
/// percentage says — 3 of 8 is a large share of nothing.
pub const MATERIAL_FLOOR_SITES: usize = 5;

/// The recorded verdict, reproduced by the run and pinned by its assertion.
pub const RECORDED_FINDING: &str = include_str!("configuration_agreement_finding.txt");

// ── Configuration sources ───────────────────────────────────────────────────
//
// The corpus itself — `ConfigSource`, `ConfigCorpus`, the YAML/properties
// flatteners, the relaxed-binding canonical key and the profile rule — was
// PROMOTED into production by S-380 and now lives at
// `logos_core::extract::config::corpus`, with the 22 unit tests that covered it.
// It is imported above rather than reimplemented: this harness must measure the
// code that ships, not a copy of it.
//
// `Agreement` followed it in S-382, together with `Resolver`, `Refusal` and
// `KeySource`, so nothing of the resolution substrate is reimplemented here any
// more. What remains below is the part that is genuinely Java-shaped — reading a
// tree-sitter expression to find which key an operand names — which is the
// harness's own carve-out and not a candidate for promotion.

/// The promoted resolution substrate (S-382): `Agreement`, `KeySource`,
/// `Refusal` and `Resolver` now live at `logos_core::resolve::binding`, with the
/// ADR-64 rule change baked in — overlay disagreement is `Agreement::Divergent`,
/// which **admits every value with its profile set** instead of refusing.
///
/// This harness keeps measuring S-365's question under S-365's rule (a key whose
/// sources disagree is refused, [CR-115] §3.4), because the finding it reproduces
/// was recorded under that rule and a recorded measurement that silently changes
/// its own admission rule stops being reproducible. The S-382 reading of the SAME
/// run is reported beside it by `report_s382`, so both rules are visible and
/// neither is inferred from the other.
pub use logos_core::resolve::binding::{
    Agreement, ConfigLookup, KeySource, ProfiledValue, Refusal, Resolver,
};

/// A module-scoped view of a discovered [`ConfigCorpus`], as the promoted
/// [`ConfigLookup`] seam.
///
/// The production tier reads one member's own store, which is already scoped; this
/// harness walks a multi-module repository in one pass, so the scope is applied
/// here — the classpath one deployable assembles. Both are [ADR-64]'s "within
/// reach of the reading module"; they differ only in what a module is in each
/// setting, which is why the scope is a parameter of the lookup rather than a
/// property of the corpus.
pub struct CorpusLookup<'a>(pub &'a ConfigCorpus);

impl ConfigLookup for CorpusLookup<'_> {
    fn definitions(&self, key: &str, module: &str) -> Vec<ConfigDefinition> {
        definitions_in(self.0, key, Some(module))
    }
}

/// Every committed definition of one **canonical** key in `corpus`, optionally
/// narrowed to one module — the single `ConfigSource` → [`ConfigDefinition`]
/// conversion this harness performs.
///
/// One function rather than two, because the module-scoped and workspace-wide
/// readings differ only in that filter, and the sort below is load-bearing for
/// both: the store returns `(path, value)`-ordered rows, [`Agreement`] groups by
/// value, and the profile lists it produces would differ between the two seams
/// if either ordering drifted. Two copies of that meant the warning guarded one
/// of them.
fn definitions_in(
    corpus: &ConfigCorpus,
    canonical: &str,
    module: Option<&str>,
) -> Vec<ConfigDefinition> {
    let mut out = Vec::new();
    for source in &corpus.sources {
        if module.is_some_and(|m| source.module != m) {
            continue;
        }
        let Some(values) = source.values.get(canonical) else {
            continue;
        };
        for value in values {
            out.push(ConfigDefinition {
                path: source.path.clone(),
                profile: source.profile.clone(),
                value: value.clone(),
            });
        }
    }
    out.sort_by(|a, b| (&a.path, &a.value).cmp(&(&b.path, &b.value)));
    out
}

/// The value S-365's rule admits: the one every committed source agrees on, and
/// **nothing** when they disagree ([CR-115] §3.4).
///
/// The promoted [`Agreement`] admits a divergent key too ([ADR-64] decision point
/// 3, S-382's rule change); this function is where the recorded measurement keeps
/// reading it under the rule it was recorded with, so the finding stays
/// reproducible. `report_s382` reads the same run under the new rule.
pub fn agreed_value(agreement: &Agreement) -> Option<&str> {
    match agreement {
        Agreement::Agreed(value) => Some(value.value.as_str()),
        Agreement::Divergent(_) | Agreement::Placeholder { .. } | Agreement::Missing => None,
    }
}

/// An agreement proven by the **call site itself** rather than by a configuration
/// source — the sentinel `report_base_urls` reads as "proven at the call site".
///
/// Zero defining sources and no profile, because there is no configuration source
/// behind it at all; `census_detail` renders exactly that rather than the
/// promoted `detail()`'s "agreed across 0 source(s)".
pub fn proven_at_the_call_site(value: String) -> Agreement {
    Agreement::Agreed(ProfiledValue {
        value,
        profiles: Vec::new(),
        unprofiled: false,
        sources: Vec::new(),
    })
}

/// [`Agreement::detail`], with the call-site sentinel spelled the way the census
/// has always spelled it.
pub fn census_detail(agreement: &Agreement) -> String {
    match agreement {
        Agreement::Agreed(v) if v.sources.is_empty() => {
            format!("proven at the call site: {:?}", v.value)
        }
        other => other.detail(),
    }
}

/// What the whole workspace's sources prove about `key`, ignoring module scope —
/// the census reading [CR-115] §3.4's words take literally.
pub fn workspace_agreement(corpus: &ConfigCorpus, key: &str) -> Agreement {
    Agreement::of(&definitions_in(corpus, &canonical_key(key), None))
}

// ── The configuration-binding index (S-381) ────────────────────────────────
//
// PROMOTED. `PropertiesClass`, `PropertiesIndex` and the four Java-grammar
// readers under them (`annotation_prefix`, `declared_properties`,
// `annotation_named`, `child_of_kind`) lived here until S-381 and walked
// tree-sitter Java nodes directly — `class_declaration`, `modifiers`,
// `annotation`, `element_value_pair`, `field_declaration`, `formal_parameter`.
// That is the Java-shaped substrate CR-121 §5.1 asks to be replaced, and it is
// now `extract::config::binding`: a generic interpreter over a plugin's
// `properties` query and its `[properties]` descriptor table, with the
// vocabulary in `plugins/java/plugin.toml` and the tree shapes in
// `plugins/java/queries/properties.scm`.
//
// The behaviour this harness measures is unchanged by construction — the
// collision rule, the module-first lookup and the accessor name transformation
// moved with their semantics intact — and the reference-workspace figures below
// are the check on that claim.
pub use logos_core::extract::config::binding::PropertiesIndex;

/// The harness's resolution context: the promoted [`Resolver`] plus the
/// configuration-bound class index it deliberately does **not** carry.
///
/// `Resolver` was promoted without a `props` field because the shipped resolver
/// never reads one — finding *which key* an operand names is the tree-sitter,
/// language-shaped half that stays here ([CR-121] §5.1). That half needs the
/// index, so the harness pairs the two itself rather than making production
/// carry a field only this file reads.
#[derive(Clone, Copy)]
pub struct Judge<'a> {
    pub resolver: Resolver<'a>,
    pub props: &'a PropertiesIndex,
}

impl Judge<'_> {
    /// What the committed sources prove about `key`, in this judge's scope.
    fn agreement(&self, key: &str) -> Agreement {
        self.resolver.agreement(key)
    }
}

/// The plugin whose accessor convention `resolve_getter` judges by. A literal
/// here and nowhere in `logos-core`: this harness is deliberately
/// language-specific (see the parent module's carve-out), and the expression
/// shapes it reads are Java's.
const JAVA_PLUGIN: &str = "java";

// ── Key resolution ──────────────────────────────────────────────────────────

/// What one configuration-lookup operand resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    Resolved {
        key: String,
        source: KeySource,
        /// The file declaring the `@ConfigurationProperties` class the key came
        /// from, so a census line names the evidence rather than asserting it
        /// ([NFR-CC-04]). `None` for a `@Value` or environment read, which
        /// carry their key at the use site.
        declared_in: Option<String>,
    },
    Unresolved(Refusal),
}

impl KeyOutcome {
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Resolved { key, .. } => Some(key),
            Self::Unresolved(_) => None,
        }
    }
}

/// Resolve one configuration-lookup operand to the configuration key it reads.
///
/// Recursion through same-unit bindings is bounded by [`FOLD_DEPTH`], the same
/// constant — and for the same reason — as `classify` and `folded_text` in the
/// parent module: it is the cycle guard. `a = b; b = a` binds two distinct AST
/// nodes, so an identity check alone does not terminate.
pub fn resolve_key(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    judge_ctx: Judge<'_>,
) -> KeyOutcome {
    resolve_key_at(node, src, unit, judge_ctx, FOLD_DEPTH)
}

fn resolve_key_at(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    judge_ctx: Judge<'_>,
    depth: usize,
) -> KeyOutcome {
    if let Some(key) = environment_key(node, src) {
        return KeyOutcome::Resolved { key, source: KeySource::Environment, declared_in: None };
    }
    let kind = node.kind();
    if kind.contains("call") || kind.contains("invocation") {
        return resolve_getter(node, src, unit, judge_ctx);
    }
    // A bare (or qualified) name: `@Value("${…}")` is the only one-hop form.
    if let Some(name) = operand_name(node, src) {
        // A parameter is decided FIRST, before any annotation is read — the
        // same rule `classify_binding` applies in the parent module.
        //
        // `Unit` is file-scoped, not scope-aware, so a `@Value`-annotated field
        // and a method parameter that shadows it share one entry, and the
        // field's key answered for the parameter. Testing "are ALL bindings
        // parameters?" does not settle it either, because the field binding
        // makes that false. So resolve it by SCOPE: does the method actually
        // enclosing this operand declare a parameter of that name? Field /
        // parameter collision is idiomatic Spring, and both readings of it are
        // wrong in a different direction.
        if encloses_parameter_named(node, &name, src) {
            return KeyOutcome::Unresolved(Refusal::MethodParameter);
        }
        let all_parameters = unit.bindings.get(&name).is_some_and(|bs| {
            bs.iter().all(|b| {
                super::is_parameter_kind(&b.bind_kind) || super::is_parameter_kind(&b.decl_kind)
            })
        });
        if all_parameters {
            return KeyOutcome::Unresolved(Refusal::MethodParameter);
        }
        if let Some(key) = value_annotation_key(&name, unit) {
            return KeyOutcome::Resolved {
                key,
                source: KeySource::ValueAnnotation,
                declared_in: None,
            };
        }
        // A name bound to a configuration accessor one hop away resolves
        // through that accessor — the same reach `classify` uses when it calls
        // an operand a configuration lookup in the first place.
        let Some(bindings) = unit.bindings.get(&name) else {
            return KeyOutcome::Unresolved(Refusal::UnboundName);
        };
        // Collect EVERY binding's key, not the first that resolves. `folded_text`
        // in the parent refuses a name bound to two different literals because
        // "the source does not prove which the call site sees"; the same is true
        // of a name bound to two different accessors, and `Unit::build` fills
        // this vec in DFS order, so "first" is not even source order.
        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut resolved: Option<KeyOutcome> = None;
        for binding in bindings {
            let Some(value) = binding.value else { continue };
            if value.id() == node.id() || depth == 0 {
                continue;
            }
            let outcome = resolve_key_at(value, src, unit, judge_ctx, depth - 1);
            if let Some(key) = outcome.key() {
                keys.insert(key.to_string());
                resolved.get_or_insert(outcome);
            }
        }
        match keys.len() {
            1 => return resolved.unwrap_or(KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor)),
            n if n > 1 => return KeyOutcome::Unresolved(Refusal::AmbiguousBinding),
            _ => {}
        }
    }
    KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor)
}

/// Whether a method/constructor/lambda enclosing `node` declares a parameter
/// called `name` — i.e. whether the operand refers to a parameter rather than
/// to a same-named field of the class.
fn encloses_parameter_named(node: Node<'_>, name: &str, src: &[u8]) -> bool {
    let mut current = node.parent();
    while let Some(scope) = current {
        if matches!(
            scope.kind(),
            "method_declaration" | "constructor_declaration" | "lambda_expression"
        ) {
            if let Some(params) = scope.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                let declared = params.named_children(&mut cursor).any(|p| {
                    p.child_by_field_name("name")
                        .and_then(|n| n.utf8_text(src).ok())
                        .is_some_and(|n| n == name)
                });
                if declared {
                    return true;
                }
            }
        }
        current = scope.parent();
    }
    false
}

/// `System.getenv("X")`, `os.Getenv("X")`, `process.env.X`, `os.environ["X"]`.
fn environment_key(node: Node<'_>, src: &[u8]) -> Option<String> {
    let text = node.utf8_text(src).ok()?.trim();
    if let Some(rest) = text.strip_prefix("process.env.") {
        return (!rest.is_empty() && rest.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .then(|| rest.to_string());
    }
    // Case-SENSITIVE needles. Lower-casing the operand first made
    // `configApi.getEnv("A")` — an ordinary bean getter — read as an
    // environment read and pre-empt the properties lookup. `getenv` and
    // `Getenv` are the actual spellings Java/PHP and Go use; `getEnv` is not
    // one of them.
    let (arg_start, close) = ["getenv(", "Getenv(", "environ[", "environ.get("]
        .iter()
        .find_map(|needle| {
            let at = text.find(needle)?;
            Some((at + needle.len(), if needle.contains('[') { ']' } else { ')' }))
        })?;
    let arg = text.get(arg_start..)?;
    let end = arg.find(close)?;
    // The operand must BE the environment read, not merely contain one.
    // `Optional.ofNullable(System.getenv("HOST")).orElse("/x")` is an
    // expression whose value the environment does not decide; reading it as
    // `HOST` silently dropped the fallback.
    if arg_start + end + 1 != text.len() {
        return None;
    }
    let name = arg[..end].trim().trim_matches(['"', '\'']);
    (!name.is_empty() && !name.contains(['(', ' ', '+'])).then(|| name.to_string())
}

/// The `${key}` of a `@Value` annotation on a same-unit binding of `name`.
fn value_annotation_key(name: &str, unit: &Unit<'_>) -> Option<String> {
    let bindings = unit.bindings.get(name)?;
    bindings
        .iter()
        .filter(|b| {
            // A parameter's `decl_head` is its own declaration, but a caller
            // supplies its value; an annotation elsewhere on a same-named field
            // does not describe it.
            !super::is_parameter_kind(&b.bind_kind) && !super::is_parameter_kind(&b.decl_kind)
        })
        .find_map(|b| {
        let at = b.decl_head.find("@Value")?;
        let head = &b.decl_head[at..];
        let open = head.find("${")?;
        let close = head[open..].find('}')? + open;
        let inner = &head[open + 2..close];
        // `${key:default}` — the default is not a source, so only the key.
        let key = inner.split(':').next().unwrap_or(inner).trim();
        (!key.is_empty()).then(|| key.to_string())
    })
}

/// `receiver.getProperty()` → the key its `@ConfigurationProperties` class binds.
fn resolve_getter(
    node: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    judge_ctx: Judge<'_>,
) -> KeyOutcome {
    let Some(function) = node
        .child_by_field_name("name")
        .or_else(|| node.child_by_field_name("function"))
    else {
        return KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor);
    };
    let Some(receiver) = node.child_by_field_name("object") else {
        return KeyOutcome::Unresolved(Refusal::UnrecognisedAccessor);
    };
    if receiver.kind().contains("call") || receiver.kind().contains("invocation") {
        return KeyOutcome::Unresolved(Refusal::NestedAccessor);
    }
    let method = function.utf8_text(src).unwrap_or_default().trim();
    // The accessor SHAPE question, asked before the receiver is resolved so the
    // refusal order this census reports is unchanged: a member read that is not
    // an accessor at all is `NotAGetter`, whatever its receiver turns out to be.
    // The convention itself is the plugin descriptor's `[properties]
    // accessor_prefixes`, never a `get`/`is` literal here (S-381).
    //
    // Asked of JAVA specifically, not of every language the index was declared
    // over. This function reads a Java bean-getter call shape and nothing else,
    // so Java is the language whose convention decides it — and asking the index
    // as a whole would be worse than imprecise: a language declaring the empty
    // prefix (direct property access, which Kotlin does) makes EVERY name an
    // accessor, and `NotAGetter` would stop being reachable at all. Naming the
    // language here is the parent harness's stated carve-out, not a leak of one
    // into `logos-core`.
    if !judge_ctx.props.names_an_accessor(JAVA_PLUGIN, method) {
        return KeyOutcome::Unresolved(Refusal::NotAGetter);
    }
    let Some(receiver_name) = operand_name(receiver, src) else {
        return KeyOutcome::Unresolved(Refusal::ReceiverTypeUnknown);
    };
    let Some(declared) = unit.declared_type(&receiver_name) else {
        return KeyOutcome::Unresolved(Refusal::ReceiverTypeUnknown);
    };
    let Some(class) = judge_ctx.props.get(declared, judge_ctx.resolver.module) else {
        return KeyOutcome::Unresolved(Refusal::NoPropertiesClass);
    };
    match judge_ctx.props.bind(class, method) {
        Ok(binding) => KeyOutcome::Resolved {
            key: binding.key,
            source: KeySource::Properties,
            declared_in: Some(binding.file),
        },
        // `NotAnAccessor` cannot arrive here — the shape test above already
        // returned on it — and `AmbiguousProperty` is UNREACHABLE under Java's
        // shipped `["get", "is"]` vocabulary, because neither prefix is a prefix
        // of the other, so at most one can ever strip a given name. That is what
        // lets both fold into this census's single "the class does not prove one
        // property for this accessor" variant without the mapping ever being
        // able to misreport: S-381 does not widen `Refusal`, which belongs to
        // S-382. Pinned by
        // `javas_vocabulary_cannot_produce_an_ambiguous_accessor`.
        Err(_) => KeyOutcome::Unresolved(Refusal::PropertyNotDeclared),
    }
}

// ── Per-site verdict ────────────────────────────────────────────────────────

/// What [CR-115] §3.4's agreement rule does with one site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every operand folds from the source alone, so configuration has no part
    /// in the site — S-355's territory, outside this measurement's reach.
    NotConfigurationBound,
    /// Already admitted today: the arm emits it without this change.
    AlreadyAdmitted,
    /// Newly admitted under the agreement rule, resolving to this value.
    NewlyAdmitted { resolved: String },
    /// Every key resolved and agreed, but the composition is still not a route
    /// the arm can bind (client-call arm only).
    NotARoute { resolved: String },
    /// At least one key's sources disagree.
    Disagreement,
    /// At least one key is defined by no committed source.
    MissingKey,
    /// At least one key's value is itself a `${…}` indirection.
    PlaceholderValue,
    /// At least one accessor does not resolve to a key.
    NoKey(Refusal),
    /// The value comes from an environment variable, which is not a committed
    /// source — out of scope for [CR-115] §3.3 whatever the sources say.
    OutOfScopeSource,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Self::NotConfigurationBound => "not configuration-bound",
            Self::AlreadyAdmitted => "already admitted",
            Self::NewlyAdmitted { .. } => "NEWLY ADMITTED",
            Self::NotARoute { .. } => "agreed, but not a route",
            Self::Disagreement => "refused: disagreement",
            Self::MissingKey => "refused: missing key",
            Self::PlaceholderValue => "refused: placeholder value",
            Self::NoKey(_) => "refused: no key",
            Self::OutOfScopeSource => "refused: environment variable, not a committed source",
        }
    }

    pub fn is_newly_admitted(&self) -> bool {
        matches!(self, Self::NewlyAdmitted { .. })
    }

    /// The value the site resolved to, for the verdicts that carry one.
    pub fn resolved(&self) -> Option<&str> {
        match self {
            Self::NewlyAdmitted { resolved } | Self::NotARoute { resolved } => Some(resolved),
            _ => None,
        }
    }
}

/// One site's verdict, and the per-operand key outcomes it was reached from —
/// kept so a census line can show the evidence, not just the conclusion.
pub struct Judgement {
    /// Parallel to the site's operands; `None` where the operand folds from the
    /// source and needs no configuration.
    pub outcomes: Vec<Option<KeyOutcome>>,
    pub verdict: Verdict,
}

/// The judgement both arms share: resolve every operand that does not fold,
/// take the agreement of each resolved key, and compose what the site would
/// resolve to.
///
/// `route_required` is the one arm-specific input, and it decides two things:
/// a client call must compose a route the arm can bind ([FR-WS-08]) and may
/// spend a trailing unresolvable operand as the `{}` a route template already
/// expresses, while a broker topic is admitted on its value alone
/// ([FR-WS-10]) and every one of its operands must resolve — a topic with a
/// `{}` in it is not a topic.
pub fn judge(
    nodes: &[Node<'_>],
    kinds: &[OperandKind],
    src: &[u8],
    unit: &Unit<'_>,
    judge_ctx: Judge<'_>,
    route_required: bool,
) -> Judgement {
    // Resolution is attempted on every operand that does **not** fold — not
    // only on the ones S-355's taxonomy labelled `configuration lookup`.
    //
    // That label is a *name* heuristic (`looks_like_configuration`'s needle
    // list), and on this corpus it under-reads by a wide margin: the broker
    // arm's beans are called `KafkaTopics`, reached through a field called
    // `kafkaTopics`, which contains none of the needles — so a first pass over
    // 54 header-form publish sites offered a denominator of 3. Resolution here
    // is **type-driven** (does the receiver's declared type name an indexed
    // `@ConfigurationProperties` class?), which is stronger evidence than the
    // spelling of a field. The taxonomy's own subset is still reported, since
    // it is the denominator [CR-115]'s acceptance criterion names.
    let mut outcomes: Vec<Option<KeyOutcome>> = Vec::with_capacity(nodes.len());
    for (node, kind) in nodes.iter().zip(kinds) {
        outcomes.push((!kind.is_foldable()).then(|| resolve_key(*node, src, unit, judge_ctx)));
    }
    // Already admitted: a single static literal needs nothing from this change.
    // Shares the parent's predicate rather than restating it — two spellings of
    // "already admitted" could disagree and each publish a different figure.
    if super::is_already_static_literal(kinds) {
        return Judgement { outcomes, verdict: Verdict::AlreadyAdmitted };
    }
    if outcomes.iter().all(Option::is_none) {
        return Judgement { outcomes, verdict: Verdict::NotConfigurationBound };
    }

    // An operand that resolves to no key at all is fatal only where it MUST
    // resolve: the leading one always (an unknown prefix is not a resolved
    // site, [CR-113] §3.2 inherited), and every one of them on the broker arm
    // (a topic with a `{}` in it is not a topic). A *trailing* one on the
    // client arm is the `{}` placeholder a route template already expresses —
    // refusing it would have under-counted `getUriX() + id` sites, which is
    // what the fixture below caught.
    let fatal = outcomes.iter().enumerate().find_map(|(i, outcome)| match outcome {
        Some(KeyOutcome::Unresolved(refusal)) if i == 0 || !route_required => Some(*refusal),
        _ => None,
    });
    if let Some(refusal) = fatal {
        return Judgement { outcomes, verdict: Verdict::NoKey(refusal) };
    }

    // An operand that DOES resolve to a key must agree, wherever it sits. A
    // conflicting configuration value is not a route parameter: turning it into
    // `{}` because it happens to be trailing would be the default-profile guess
    // [CR-115] §3.4 exists to refuse, wearing a different hat.
    // An environment variable is not a committed source, so its scope is
    // decided before its agreement: [CR-115] §3.3 excludes it whatever the
    // sources say, and `canonical_key` lower-cases and strips separators, so
    // `getenv("BASE_URL")` would otherwise collide with a yml `base-url` and be
    // counted in the headline as though the repository proved it.
    if outcomes
        .iter()
        .flatten()
        .any(|o| matches!(o, KeyOutcome::Resolved { source: KeySource::Environment, .. }))
    {
        return Judgement { outcomes, verdict: Verdict::OutOfScopeSource };
    }

    let agreements: Vec<Option<Agreement>> = outcomes
        .iter()
        .map(|o| o.as_ref().and_then(KeyOutcome::key).map(|k| judge_ctx.agreement(k)))
        .collect();
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Missing)) {
        return Judgement { outcomes, verdict: Verdict::MissingKey };
    }
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Placeholder { .. })) {
        return Judgement { outcomes, verdict: Verdict::PlaceholderValue };
    }
    // S-365's rule: a key whose sources disagree is REFUSED. S-382 changed that
    // for production (ADR-64 decision point 3 retains every profile-tagged value);
    // this measurement keeps the rule it was recorded under, and `report_s382`
    // reads the same run under the new one.
    if agreements.iter().flatten().any(|a| matches!(a, Agreement::Divergent(_))) {
        return Judgement { outcomes, verdict: Verdict::Disagreement };
    }
    // Nothing resolved through configuration: the composition folds from the
    // source alone, or from a trailing placeholder. Either way S-355 already
    // measured it and this arm must not re-count it as its own recovery.
    if !outcomes.iter().flatten().any(|o| o.key().is_some()) {
        // ...unless a configuration accessor was recognised and simply could
        // not be read. Position decided this before: a LEADING unresolved
        // lookup was fatal and counted, a TRAILING one fell through to
        // `NotConfigurationBound`, which `Tally::add` drops from `denominator`
        // and `report_census` hides. The same refusal was in or out of the
        // published denominator purely by where it sat in the composition —
        // shrinking the denominator and so inflating the recovered share the
        // verdict turns on. [CR-115] AC1 also requires every such site be
        // reported with its reason.
        let unread_lookup = kinds
            .iter()
            .zip(&outcomes)
            .find_map(|(kind, outcome)| match outcome {
                Some(KeyOutcome::Unresolved(refusal))
                    if *kind == OperandKind::ConfigurationLookup =>
                {
                    Some(*refusal)
                }
                _ => None,
            });
        let verdict = match unread_lookup {
            Some(refusal) => Verdict::NoKey(refusal),
            None => Verdict::NotConfigurationBound,
        };
        return Judgement { outcomes, verdict };
    }

    // Every configuration operand agrees. Compose what the site resolves to:
    // a configuration value, a folded same-unit constant or literal, or the
    // `{}` placeholder a route template already expresses.
    let Some(resolved) = compose(nodes, &agreements, src, unit, route_required) else {
        return Judgement { outcomes, verdict: Verdict::NoKey(Refusal::UnrecognisedAccessor) };
    };
    let verdict = if !route_required || binds_a_route(&resolved) {
        Verdict::NewlyAdmitted { resolved }
    } else {
        Verdict::NotARoute { resolved }
    };
    Judgement { outcomes, verdict }
}

/// The text the site resolves to. Returns `None` when the **leading** operand
/// resolves to nothing — a composition whose prefix is unknown is not resolved
/// at all, whatever its tail says ([CR-113] §3.2, inherited by [CR-115]).
///
/// Shared with the parent module's S-355 headline, which passes an all-`None`
/// `agreements` slice: with no configuration values in play this is exactly
/// [FR-WS-18] AC1's placeholder rule. The two headlines rest on one
/// implementation of that rule so a change to it cannot make them mean
/// different things.
pub(super) fn compose(
    nodes: &[Node<'_>],
    agreements: &[Option<Agreement>],
    src: &[u8],
    unit: &Unit<'_>,
    allow_placeholders: bool,
) -> Option<String> {
    let mut out = String::new();
    for (i, node) in nodes.iter().enumerate() {
        let text = agreements
            .get(i)
            .and_then(Option::as_ref)
            .and_then(agreed_value)
            .map(str::to_string)
            .or_else(|| folded_text(*node, src, unit, FOLD_DEPTH));
        match text {
            Some(text) => out.push_str(&text),
            None if i == 0 => return None,
            None if allow_placeholders => out.push_str("{}"),
            None => return None,
        }
    }
    let out = out.trim().to_string();
    (!out.is_empty()).then_some(out)
}

/// Whether a resolved client-call template names a route the arm can bind: an
/// absolute path, or an absolute URL carrying one.
///
/// [CR-115] §3.4 is explicit that the *host* need not resolve —
/// `http://pec-anagrafica/api/v1` names a service-discovery target and binding
/// matches on the portable route key — so an absolute URL is admitted here,
/// unlike in the S-355 folding measurement where no configuration value existed
/// to supply one.
pub fn binds_a_route(template: &str) -> bool {
    if template.starts_with('/') {
        return true;
    }
    let Some((scheme, rest)) = template.split_once("://") else {
        return false;
    };
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return false;
    }
    rest.split_once('/').is_some_and(|(host, path)| !host.is_empty() && !path.is_empty())
}

// ── The broker-publish arm ──────────────────────────────────────────────────

/// The message-header publish form, as a harness-local query.
///
/// Deliberately shape-only: the header constant and the method name are
/// filtered in Rust rather than by query predicates.
///
/// Not because predicates do not work — `tree_sitter` 0.25 *does* evaluate the
/// text predicates (`#eq?`, `#match?`, `#any-of?`) inside `QueryMatches`, which
/// is why `count_broker_captures` can rely on the real `brokers.scm`'s
/// `#any-of?` firing. The reason is that this arm's filter is not a text
/// predicate at all: it accepts four different spellings of the topic header
/// (qualified, statically imported, and the wire name) and must stay readable
/// beside `names_topic_header`, which the fixtures pin directly.
const HEADER_PUBLISH_QUERY: &str = r"
(method_invocation
  name: (identifier) @publish.method
  arguments: (argument_list
    . (_) @publish.header
    . (_) @publish.topic))
";

/// The Spring Kafka topic header, in the two spellings the corpus uses: the
/// constant (qualified or statically imported) and its wire name.
fn names_topic_header(text: &str) -> bool {
    let text = text.trim();
    text == "KafkaHeaders.TOPIC"
        || text.ends_with(".KafkaHeaders.TOPIC")
        || text == "TOPIC"
        || text.trim_matches('"') == "kafka_topic"
}

/// One classified broker publish site.
#[derive(Debug, Clone)]
pub struct BrokerSite {
    pub file: String,
    pub line: u32,
    pub text: String,
    pub kinds: Vec<OperandKind>,
    pub outcomes: Vec<Option<KeyOutcome>>,
    pub verdict: Verdict,
    /// The topic operand is already a static literal at the header-form site.
    /// Recognising the form ([CR-117] §3.2 / S-370) admits it on its own; the
    /// configuration rule is not what unlocks it, so it is excluded from this
    /// measurement's newly-admitted count and reported separately.
    pub literal_topic: bool,
}

/// Per-language broker figures.
#[derive(Debug, Default)]
pub struct BrokerStats {
    pub files_scanned: usize,
    /// What the real `brokers.scm` captures today, for the denominator.
    pub publish_literals_today: usize,
    pub subscribe_literals_today: usize,
    /// Whether the header form is even expressible in this language's grammar.
    pub header_form_supported: bool,
    pub sites: Vec<BrokerSite>,
}

/// Compile the header-form query against a language, or `None` when the
/// grammar has no such node shape (every non-Java grammar in the set).
pub fn header_publish_query(language: &tree_sitter::Language) -> Option<tree_sitter::Query> {
    tree_sitter::Query::new(language, HEADER_PUBLISH_QUERY).ok()
}

/// Count what the real `brokers.scm` captures in this file — the arm's own
/// output, not the harness's reading of it.
pub fn count_broker_captures(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    stats: &mut BrokerStats,
) {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        for cap in m.captures {
            match names[cap.index as usize] {
                "broker.publish.topic" => stats.publish_literals_today += 1,
                "broker.subscribe.topic" => stats.subscribe_literals_today += 1,
                _ => {}
            }
        }
    }
}

/// Collect and judge every message-header publish site in one file.
pub fn collect_header_publishes(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    rel: &str,
    judge_ctx: Judge<'_>,
) -> Vec<BrokerSite> {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method = None;
        let mut header = None;
        let mut topic = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "publish.method" => method = Some(cap.node),
                "publish.header" => header = Some(cap.node),
                "publish.topic" => topic = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method), Some(header), Some(topic)) = (method, header, topic) else {
            continue;
        };
        if method.utf8_text(src).unwrap_or_default().trim() != "setHeader" {
            continue;
        }
        if !names_topic_header(header.utf8_text(src).unwrap_or_default()) {
            continue;
        }
        let mut nodes = Vec::new();
        super::operands(topic, src, &mut nodes);
        let kinds: Vec<OperandKind> =
            nodes.iter().map(|n| super::classify(*n, src, unit, FOLD_DEPTH)).collect();
        let literal_topic = static_literal(topic, src).is_some();
        let judgement = judge(&nodes, &kinds, src, unit, judge_ctx, false);
        out.push(BrokerSite {
            file: rel.to_string(),
            line: method.start_position().row as u32 + 1,
            text: topic
                .utf8_text(src)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            kinds,
            outcomes: judgement.outcomes,
            verdict: judgement.verdict,
            literal_topic,
        });
    }
    out
}

// ── The base-URL half of CR-115 ─────────────────────────────────────────────

/// Builder methods that set the base a client's paths compose against.
const BASE_URL_METHODS: [&str; 4] = ["baseUrl", "baseURL", "setBaseUrl", "rootUri"];

/// A call with at least one argument. The method name is filtered in Rust, as
/// everywhere else here, because query predicates are not evaluated.
const BASE_URL_QUERY: &str = r"
(method_invocation
  name: (identifier) @base.method
  arguments: (argument_list . (_) @base.arg))
";

/// One `.baseUrl(…)` site and what its operand resolves to.
#[derive(Debug, Clone)]
pub struct BaseUrlSite {
    pub file: String,
    pub line: u32,
    pub text: String,
    pub outcome: KeyOutcome,
    pub agreement: Agreement,
}

pub fn base_url_query(language: &tree_sitter::Language) -> Option<tree_sitter::Query> {
    tree_sitter::Query::new(language, BASE_URL_QUERY).ok()
}

/// Collect the base-URL sites of one file.
///
/// Why this is measured at all: [CR-115] is titled *base-URL* resolution, and
/// its §3.4 worked example is a host that varies per profile. The client arm's
/// headline counts **path** operands, because that is what [FR-WS-08] binds a
/// route key on — §3.4 says in as many words that the host need not resolve.
/// Those are two different questions and the second one is the one most likely
/// to disagree, so it is reported rather than folded into the first.
pub fn collect_base_urls(
    query: &tree_sitter::Query,
    root: Node<'_>,
    src: &[u8],
    unit: &Unit<'_>,
    rel: &str,
    judge_ctx: Judge<'_>,
) -> Vec<BaseUrlSite> {
    use tree_sitter::{QueryCursor, StreamingIterator};
    let names = query.capture_names();
    let mut out = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, src);
    while let Some(m) = matches.next() {
        let mut method = None;
        let mut arg = None;
        for cap in m.captures {
            match names[cap.index as usize] {
                "base.method" => method = Some(cap.node),
                "base.arg" => arg = Some(cap.node),
                _ => {}
            }
        }
        let (Some(method), Some(arg)) = (method, arg) else { continue };
        let name = method.utf8_text(src).unwrap_or_default().trim();
        if !BASE_URL_METHODS.contains(&name) {
            continue;
        }
        // A base URL the unit itself proves — a literal, or a same-unit
        // constant folding to one — needs no configuration source and cannot
        // disagree with one. `sources: 0` records that it was proven at the
        // call site rather than by the corpus.
        let folded = folded_text(arg, src, unit, FOLD_DEPTH);
        let (outcome, agreement) = match folded {
            Some(value) => (
                // An empty key is the sentinel `report_base_urls` reads as
                // "proven at the call site". `KeySource::CallSite` names it
                // honestly: recording it as `@Value` was simply false.
                KeyOutcome::Resolved {
                    key: String::new(),
                    source: KeySource::CallSite,
                    declared_in: None,
                },
                proven_at_the_call_site(value),
            ),
            None => {
                let outcome = resolve_key(arg, src, unit, judge_ctx);
                let agreement = match &outcome {
                    KeyOutcome::Resolved { key, .. } => judge_ctx.agreement(key),
                    KeyOutcome::Unresolved(_) => Agreement::Missing,
                };
                (outcome, agreement)
            }
        };
        out.push(BaseUrlSite {
            file: rel.to_string(),
            line: method.start_position().row as u32 + 1,
            text: arg
                .utf8_text(src)
                .unwrap_or_default()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            outcome,
            agreement,
        });
    }
    out
}

// ── Reporting ───────────────────────────────────────────────────────────────

/// One arm's tally, in the shape both arms and the combined line share.
#[derive(Debug, Default, Clone, Copy)]
pub struct Tally {
    /// **The materiality denominator**: sites the arm refuses today whose
    /// composition needs a value the source does not hold — every site this
    /// mechanism could conceivably admit, however the operand is spelled.
    pub denominator: usize,
    /// **The acceptance-criterion denominator**: the subset of those sites
    /// S-355's taxonomy labelled a *configuration lookup*. Reported because
    /// [CR-115]'s criterion is phrased over it, and because the gap between the
    /// two is itself a finding — the taxonomy is a name heuristic and the
    /// broker arm's beans are named in a way it does not catch.
    pub config_labelled: usize,
    /// Newly admitted **within** that labelled subset — the figure
    /// [CR-115]'s acceptance criterion asks for, as distinct from the total.
    pub newly_admitted_labelled: usize,
    pub newly_admitted: usize,
    pub disagreement: usize,
    pub missing_key: usize,
    pub placeholder: usize,
    pub no_key: usize,
    pub not_a_route: usize,
    /// Resolved, but from an environment variable — [CR-115] §3.3 out of scope.
    pub out_of_scope: usize,
    pub already_admitted: usize,
}

impl Tally {
    fn add(&mut self, verdict: &Verdict, kinds: &[OperandKind]) {
        if matches!(verdict, Verdict::AlreadyAdmitted) {
            self.already_admitted += 1;
            return;
        }
        if matches!(verdict, Verdict::NotConfigurationBound) {
            return;
        }
        self.denominator += 1;
        if kinds.contains(&OperandKind::ConfigurationLookup) {
            self.config_labelled += 1;
        }
        let labelled = kinds.contains(&OperandKind::ConfigurationLookup);
        match verdict {
            Verdict::NewlyAdmitted { .. } => {
                self.newly_admitted += 1;
                if labelled {
                    self.newly_admitted_labelled += 1;
                }
            }
            Verdict::Disagreement => self.disagreement += 1,
            Verdict::MissingKey => self.missing_key += 1,
            Verdict::PlaceholderValue => self.placeholder += 1,
            Verdict::NoKey(_) => self.no_key += 1,
            Verdict::NotARoute { .. } => self.not_a_route += 1,
            Verdict::OutOfScopeSource => self.out_of_scope += 1,
            Verdict::AlreadyAdmitted | Verdict::NotConfigurationBound => unreachable!(),
        }
    }

    fn merge(&mut self, other: &Self) {
        self.denominator += other.denominator;
        self.config_labelled += other.config_labelled;
        self.newly_admitted_labelled += other.newly_admitted_labelled;
        self.newly_admitted += other.newly_admitted;
        self.disagreement += other.disagreement;
        self.missing_key += other.missing_key;
        self.placeholder += other.placeholder;
        self.no_key += other.no_key;
        self.not_a_route += other.not_a_route;
        self.out_of_scope += other.out_of_scope;
        self.already_admitted += other.already_admitted;
    }

    /// Whether the mechanism recovers enough of its own denominator to be worth
    /// building, against the floors declared before the run.
    pub fn is_material(&self) -> bool {
        self.newly_admitted >= MATERIAL_FLOOR_SITES
            && self.denominator > 0
            && self.newly_admitted * 100 >= self.denominator * MATERIAL_FLOOR_PCT
    }

    /// The recovered share of the arm's own denominator, in whole percent.
    pub fn percent(&self) -> usize {
        (self.newly_admitted * 100).checked_div(self.denominator).unwrap_or(0)
    }

    fn header() -> String {
        format!(
            "{:<12} {:>6} {:>7} {:>6} {:>8} {:>7} {:>8} {:>6} {:>9} {:>4} {:>6}",
            "language",
            "denom",
            "cfg-lbl",
            "NEW",
            "disagree",
            "missing",
            "placehld",
            "no-key",
            "not-route",
            "env",
            "literal",
        )
    }

    fn row(&self, label: &str) -> String {
        format!(
            "{:<12} {:>6} {:>7} {:>6} {:>8} {:>7} {:>8} {:>6} {:>9} {:>4} {:>6}",
            label,
            self.denominator,
            self.config_labelled,
            self.newly_admitted,
            self.disagreement,
            self.missing_key,
            self.placeholder,
            self.no_key,
            self.not_a_route,
            self.out_of_scope,
            self.already_admitted,
        )
    }
}

/// Whether a site lives in production source or in a test tree.
///
/// This exists because the arm split alone hid an inversion. The broker arm
/// reported 38 of 54 admitted — and **every one of the 38 was an IT test
/// class**, while all 13 sites in `src/main` were refused. A cross-service
/// coupling edge derived from a test fixture is not the coupling [CR-117] is
/// about, and [CR-117] §6's acceptance criterion is written over the
/// main-source population specifically. The story's own rule — "a material
/// figure for one and an immaterial figure for the other must not be averaged
/// away" — applies one level deeper than the two arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tree {
    Main,
    Test,
}

impl Tree {
    /// Classify by path. Deliberately broad: Maven/Gradle `src/test`, a
    /// top-level `test`/`tests` directory, Go's `_test.go`, and the `IT`/`Test`
    /// class-name suffixes the corpus uses.
    pub fn of(path: &str) -> Self {
        let lower = path.to_ascii_lowercase();
        let in_test_dir = lower.contains("/src/test/")
            || lower.starts_with("src/test/")
            || lower.contains("/test/")
            || lower.contains("/tests/")
            || lower.starts_with("test/")
            || lower.starts_with("tests/");
        let test_named = lower.ends_with("_test.go")
            || lower.ends_with("it.java")
            || lower.ends_with("test.java")
            || lower.ends_with("tests.java")
            || lower.ends_with(".test.ts")
            || lower.ends_with(".spec.ts");
        if in_test_dir || test_named {
            Self::Test
        } else {
            Self::Main
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Test => "test",
        }
    }
}

/// Both arms' figures, separately and combined — the object the verdict is read
/// off, so the two are never averaged into one. Each arm is additionally split
/// by [`Tree`], because a yield that lives entirely in test source is a
/// different verdict from the same yield in production source.
#[derive(Debug, Default)]
pub struct Verdicts {
    pub client: BTreeMap<String, Tally>,
    pub broker: BTreeMap<String, Tally>,
    /// Per arm ("client-call" / "broker"), per tree.
    pub by_tree: BTreeMap<(&'static str, Tree), Tally>,
}

impl Verdicts {
    pub fn client_total(&self) -> Tally {
        let mut total = Tally::default();
        for t in self.client.values() {
            total.merge(t);
        }
        total
    }

    pub fn broker_total(&self) -> Tally {
        let mut total = Tally::default();
        for t in self.broker.values() {
            total.merge(t);
        }
        total
    }

    /// The combined figure. Reported **alongside** the two arms, never instead
    /// of them: [CR-115] and [CR-117] are decided on their own arm.
    pub fn combined(&self) -> Tally {
        let mut total = self.client_total();
        total.merge(&self.broker_total());
        total
    }
}

/// Print the S-365 measurement and return both arms' figures.
pub fn report(m: &super::Measurement) -> Verdicts {
    println!(
        "\n=== S-365: configuration-key resolvability and profile agreement ===\n\
         \nOne gate, two arms. CR-115 §3.4's rule is applied verbatim to both: a\
         \nkey is resolved only when EVERY committed source that defines it agrees\
         \non the value. The arms are reported separately and combined — never\
         \naveraged — because CR-115 and CR-117 are decided on their own figure.\n"
    );
    report_sources(m);
    let mut verdicts = Verdicts::default();
    report_client_arm(m, &mut verdicts);
    report_broker_arm(m, &mut verdicts);
    report_newly_admitted(m);
    report_base_urls(m);
    let (workspace_agreed, headline_total) = workspace_scope_headline(m);
    report_totals(&verdicts);
    println!(
        "  workspace-scope check: of the {headline_total} sites the module-scoped rule admits, \
         {workspace_agreed} also agree\n  across EVERY source in the workspace — CR-115 §3.4 read \
         strictly admits {workspace_agreed}, not {headline_total}."
    );
    report_refusals(m);
    report_s382(m);
    report_census(m);
    verdicts
}

fn report_sources(m: &super::Measurement) {
    let corpus = &m.config;
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for source in &corpus.sources {
        let ext = source.path.rsplit('.').next().unwrap_or("?");
        *by_kind.entry(ext).or_default() += 1;
    }
    let keys: BTreeSet<&String> =
        corpus.sources.iter().flat_map(|s| s.values.keys()).collect();
    let profiles = corpus.profiles();
    println!("--- committed configuration sources (FR-SY-11 admission) ---");
    println!(
        "{} sources, {} distinct keys, {} unprofiled + {} profiled",
        corpus.sources.len(),
        keys.len(),
        corpus.sources.iter().filter(|s| s.profile.is_none()).count(),
        corpus.sources.iter().filter(|s| s.profile.is_some()).count(),
    );
    for (ext, count) in &by_kind {
        println!("  application*.{ext:<12} {count:>4}");
    }
    println!(
        "  profiles discovered: {}",
        if profiles.is_empty() {
            "none".to_string()
        } else {
            profiles.into_iter().collect::<Vec<_>>().join(", ")
        },
    );
    println!(
        "  @ConfigurationProperties classes indexed: {} ({} annotated but prefixless, \
         {} simple-name collisions refused)",
        m.properties.len(),
        m.properties.prefixless,
        m.properties.collisions.len(),
    );
}

fn report_client_arm(m: &super::Measurement, verdicts: &mut Verdicts) {
    println!(
        "\n--- ARM 1: client-call sites (CR-115) ---\n\
         `denom`   every gate-admitted `invocations` site the arm refuses today whose\n\
         .         composition needs a value the source does not hold. The materiality\n\
         .         denominator: what this mechanism could conceivably admit.\n\
         `cfg-lbl` the subset S-355's taxonomy labelled `configuration lookup` — the\n\
         .         denominator CR-115's acceptance criterion is phrased over (its 81\n\
         .         Java sites). The two differ, so both are printed.\n\
         `literal` sites already a single static literal, excluded from `denom`.\n\
         `not-route` keys all agreed, but the composition still names no bindable\n\
         .         route (FR-WS-08 AC2).\n"
    );
    println!("{}", Tally::header());
    for (lang, stats) in &m.per_language {
        let mut tally = Tally::default();
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            tally.add(&site.cr115, &site.kinds);
            verdicts
                .by_tree
                .entry(("client-call", Tree::of(&site.file)))
                .or_default()
                .add(&site.cr115, &site.kinds);
        }
        println!("{}", tally.row(lang));
        verdicts.client.insert(lang.clone(), tally);
    }
    println!("{}", verdicts.client_total().row("ALL"));
}

fn report_broker_arm(m: &super::Measurement, verdicts: &mut Verdicts) {
    println!(
        "\n--- ARM 2: broker publish sites (CR-117 §3.3) ---\n\
         Denominator: every message-header publish site — `setHeader(KafkaHeaders.TOPIC,\n\
         …)` — whose topic operand is not a literal. A DIFFERENT denominator from arm 1\n\
         and not comparable to it: this arm has no ledger gate, and the real\n\
         `brokers.scm` recognises the header form not at all, so nothing here is\n\
         admitted today. Note how far `cfg-lbl` falls below `denom`: the corpus's topic\n\
         beans are called `KafkaTopics`, a name S-355's `looks_like_configuration`\n\
         heuristic does not catch, which is why resolution here is type-driven.\n"
    );
    println!("{}", Tally::header());
    for (lang, stats) in &m.broker {
        let mut tally = Tally::default();
        for site in &stats.sites {
            tally.add(&site.verdict, &site.kinds);
            verdicts
                .by_tree
                .entry(("broker", Tree::of(&site.file)))
                .or_default()
                .add(&site.verdict, &site.kinds);
        }
        println!("{}", tally.row(lang));
        verdicts.broker.insert(lang.clone(), tally);
    }
    println!("{}", verdicts.broker_total().row("ALL"));
    println!("\nwhat the real `brokers.scm` sees today, and what the header form adds:");
    for (lang, stats) in &m.broker {
        let literal_header_sites = stats.sites.iter().filter(|s| s.literal_topic).count();
        println!(
            "{lang:<12} {:>5} files; today publish/subscribe literals {}/{}; \
             header-form sites {} ({} with a literal topic, admitted by recognition alone); \
             header form expressible in this grammar: {}",
            stats.files_scanned,
            stats.publish_literals_today,
            stats.subscribe_literals_today,
            stats.sites.len(),
            literal_header_sites,
            stats.header_form_supported,
        );
    }
}

/// The strict [CR-115] §3.4 reading — "every discovered source **in the
/// workspace** that defines the key agrees" — recomputed over the same sites.
///
/// The headline is module-scoped, because a Maven module is the classpath one
/// deployable actually assembles and two members may legitimately bind one key
/// differently. But §3.4's words are workspace-wide, and on this corpus the two
/// differ, so the report must state which produced the headline and what the
/// other would have given.
fn workspace_scope_headline(m: &super::Measurement) -> (usize, usize) {
    let client = m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.gate_admitted && s.cr115.is_newly_admitted())
        .map(|s| &s.key_outcomes);
    let broker = m
        .broker
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.verdict.is_newly_admitted())
        .map(|s| &s.outcomes);

    let mut total = 0usize;
    let mut agreed = 0usize;
    for outcomes in client.chain(broker) {
        total += 1;
        let holds = outcomes.iter().flatten().all(|outcome| match outcome.key() {
            Some(key) => matches!(workspace_agreement(&m.config, key), Agreement::Agreed(_)),
            None => true,
        });
        if holds {
            agreed += 1;
        }
    }
    (agreed, total)
}

fn report_totals(verdicts: &Verdicts) {
    let client = verdicts.client_total();
    let broker = verdicts.broker_total();
    let combined = verdicts.combined();
    println!(
        "\n--- both arms, separately and combined (never averaged) ---\n{}",
        Tally::header()
    );
    println!("{}", client.row("client-call"));
    println!("{}", broker.row("broker"));
    println!("{}", combined.row("COMBINED"));
    println!(
        "\n--- each arm split by PRODUCTION vs TEST source ---\n\
         The arm split alone is not enough. A yield that lives entirely in test fixtures is\n\
         a different verdict from the same yield in production code, and CR-117 §6's\n\
         acceptance criterion is written over the main-source population specifically.\n"
    );
    println!("{}", Tally::header());
    for arm in ["client-call", "broker"] {
        for tree in [Tree::Main, Tree::Test] {
            let tally = verdicts.by_tree.get(&(arm, tree)).copied().unwrap_or_default();
            println!("{}", tally.row(&format!("{arm}/{}", tree.label())));
        }
    }
    println!(
        "\nSCOPE: the headline is MODULE-scoped — agreement is taken over the sources under\n\
         the call site's own build module, which is the classpath one deployable assembles.\n\
         CR-115 §3.4's words are workspace-wide; that stricter reading is printed beside it\n\
         so the CR is decided on the reading it means."
    );
    println!(
        "\nmateriality floor, declared before the run: >= {MATERIAL_FLOOR_SITES} sites AND \
         >= {MATERIAL_FLOOR_PCT}% of the arm's own denominator"
    );
    for (name, tally, cr) in [
        ("client-call", client, "CR-115"),
        ("broker", broker, "CR-117"),
    ] {
        let main = verdicts.by_tree.get(&(name, Tree::Main)).copied().unwrap_or_default();
        println!(
            "  {name:<12} {} newly admitted / {} refused-today sites = {}%  \
             [taxonomy-labelled subset: {} of {}]",
            tally.newly_admitted,
            tally.denominator,
            tally.percent(),
            tally.newly_admitted_labelled,
            tally.config_labelled,
        );
        println!(
            "  {:<12}   of which in PRODUCTION source: {} of {} = {}%  ->  {cr} CRA-01 {}",
            "",
            main.newly_admitted,
            main.denominator,
            main.percent(),
            if main.is_material() { "HOLDS" } else { "FALSIFIED" },
        );
        if tally.is_material() != main.is_material() {
            println!(
                "  {:<12}   ** the whole-arm figure and the production figure DISAGREE. \
                 The production one decides {cr}: an edge derived from a test fixture is not \
                 the coupling it is about. **",
                "",
            );
        }
    }
}

/// The headline figure, listed site by site. A count nobody can check is not
/// evidence, and this is the count both change requests turn on.
fn report_newly_admitted(m: &super::Measurement) {
    println!("\n--- the headline: every site the agreement rule would NEWLY admit ---");
    let mut listed = 0usize;
    for (lang, stats) in &m.per_language {
        for site in stats.sites.iter().filter(|s| s.gate_admitted && s.cr115.is_newly_admitted()) {
            listed += 1;
            println!("client  {lang}  {}:{}  {}  ->  {}", site.file, site.line, site.text, site.cr115.resolved().unwrap_or_default());
        }
    }
    for (lang, stats) in &m.broker {
        for site in stats.sites.iter().filter(|s| s.verdict.is_newly_admitted()) {
            listed += 1;
            println!("broker  {lang}  {}:{}  {}  ->  {}", site.file, site.line, site.text, site.verdict.resolved().unwrap_or_default());
        }
    }
    if listed == 0 {
        println!("  (none)");
    }
}

/// [CR-115]'s other half: the base the admitted paths compose against.
fn report_base_urls(m: &super::Measurement) {
    println!(
        "\n--- CR-115's OTHER half: the base URL those paths compose against ---\n\
         The headline above counts PATH operands, because FR-WS-08 binds a route key on\n\
         the path and CR-115 §3.4 states the host need not resolve. Whether the host\n\
         *agrees* is a different question, and on this corpus it is the one that fails.\n\
         Both figures are given so the CR is decided on the reading it actually means.\n"
    );
    let sites: Vec<&BaseUrlSite> = m.base_urls.values().flatten().collect();
    fn agreed(site: &BaseUrlSite) -> bool {
        matches!(site.agreement, Agreement::Agreed(_))
    }
    let resolved_to_a_key = sites
        .iter()
        .filter(|s| s.outcome.key().is_some_and(|k| !k.is_empty()))
        .count();
    let proven_at_the_call_site =
        sites.iter().filter(|s| s.outcome.key() == Some("")).count();
    println!(
        "{} base-URL sites; {resolved_to_a_key} resolved to a configuration key, \
         {proven_at_the_call_site} proven at the call site;\nagreed {}; disagreed {}; \
         no source defines the key {}; accessor unresolved {}",
        sites.len(),
        sites.iter().filter(|s| agreed(s)).count(),
        sites.iter().filter(|s| matches!(s.agreement, Agreement::Divergent(_))).count(),
        sites
            .iter()
            .filter(|s| matches!(s.agreement, Agreement::Missing) && s.outcome.key().is_some())
            .count(),
        sites.iter().filter(|s| matches!(s.outcome, KeyOutcome::Unresolved(_))).count(),
    );
    let agreeing_files: BTreeSet<&str> =
        sites.iter().filter(|s| agreed(s)).map(|s| s.file.as_str()).collect();
    let base_url_files: BTreeSet<&str> = sites.iter().map(|s| s.file.as_str()).collect();
    let (mut with, mut against, mut none) = (0usize, 0usize, 0usize);
    for site in m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.gate_admitted && s.cr115.is_newly_admitted())
    {
        if agreeing_files.contains(site.file.as_str()) {
            with += 1;
        } else if base_url_files.contains(site.file.as_str()) {
            against += 1;
        } else {
            none += 1;
        }
    }
    println!(
        "\nof the newly-admitted client-call sites: {with} sit in a unit whose base URL \
         ALSO agrees,\n{against} in a unit whose base URL does not resolve or does not \
         agree, and\n{none} in a unit that sets no base URL of its own (it is configured \
         elsewhere).\n\
         \nSo the STRICTER reading of CR-115 — the whole absolute URL must be proven — \
         admits {with},\nand the FR-WS-08 route-key reading admits the headline figure. \
         Both are on the record."
    );
    for site in sites.iter().filter(|s| !agreed(s)) {
        // An unresolved accessor is named by its refusal, not lumped under
        // `missing key`: "we could not read it" and "no source defines it" are
        // different faults and only the second is CR-115's rule biting.
        let why = match &site.outcome {
            KeyOutcome::Unresolved(refusal) => refusal.label(),
            KeyOutcome::Resolved { .. } => site.agreement.label(),
        };
        println!("  {}:{}  {}  ->  {why}", site.file, site.line, site.text);
    }
}

/// The **S-382 reading** of the same run: what [ADR-64]'s committed-evidence rule
/// admits, as against what [CR-115] §3.4's rule admitted (S-382 AC5).
///
/// The two rules differ in exactly one place — a key whose overlays disagree.
/// S-365 refused it; [ADR-64] decision point 3 retains **every** value with its
/// profile set. So this reading is the S-365 one plus the disagreeing sites,
/// each contributing one profile-tagged value per overlay rather than a refusal.
///
/// Reported as its own block rather than by re-scoring the table above, because
/// the table reproduces a recorded finding and a recorded measurement that
/// silently changes its own admission rule stops being reproducible.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct S382Reading {
    /// Production client-call sites the arm refuses today (the S-365
    /// denominator, production tree only).
    pub denominator: usize,
    /// Sites carrying a resolved template under the agreed rule — unchanged from
    /// S-365, since agreement was never the part that changed.
    pub resolved: usize,
    /// Sites whose key diverges across overlays. Each is now **admitted**, and
    /// [`divergent_values`](Self::divergent_values) says with how many values.
    pub divergent: usize,
    /// The profile-tagged values those divergent sites emit, summed. Two overlays
    /// disagreeing on one key is two values, and every one of them reaches the
    /// consumer of the resolution ([ADR-64]).
    pub divergent_values: usize,
    /// Sites whose operand resolves to **no key at all** — refused under
    /// [`Refusal`], a reason distinct from every value-level one.
    pub no_key: usize,
}

/// Read the production client-call arm of `m` under the S-382 rule.
pub fn s382_reading(m: &super::Measurement) -> S382Reading {
    let mut out = S382Reading::default();
    for site in m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter())
        .filter(|s| s.gate_admitted && Tree::of(&s.file) == Tree::Main)
    {
        // The same denominator `Tally::add` builds, by the same two exclusions.
        if matches!(site.cr115, Verdict::AlreadyAdmitted | Verdict::NotConfigurationBound) {
            continue;
        }
        out.denominator += 1;
        match &site.cr115 {
            Verdict::NewlyAdmitted { .. } => out.resolved += 1,
            Verdict::NoKey(_) => out.no_key += 1,
            Verdict::Disagreement => {
                out.divergent += 1;
                // Re-read the site's own keys under the promoted rule. The module
                // is the site's, not the workspace's — the scope the headline is
                // taken over, and the one ADR-64's "within reach of the reading
                // module" names.
                let module = m.config.module_of(&site.file).to_string();
                for key in site.key_outcomes.iter().flatten().filter_map(KeyOutcome::key) {
                    let agreement = Agreement::of(&definitions_in(
                        &m.config,
                        &canonical_key(key),
                        Some(&module),
                    ));
                    if let Agreement::Divergent(values) = agreement {
                        out.divergent_values += values.len();
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn report_s382(m: &super::Measurement) {
    let r = s382_reading(m);
    println!(
        "\n--- THE S-382 READING OF THE SAME RUN (ADR-64) ---\n\
         Production client-call sites only. The rule differs from the table above in\n\
         exactly one place: a key whose overlays disagree is ADMITTED, retaining every\n\
         value with its profile set, where CR-115 §3.4 refused it.\n\
         \n\
         \x20 denominator (production, refused today)  {}\n\
         \x20 resolved template (agreed key)           {}\n\
         \x20 resolved template (divergent key)        {}  emitting {} profile-labelled values\n\
         \x20 refused: operand resolves to no key      {}\n",
        r.denominator, r.resolved, r.divergent, r.divergent_values, r.no_key,
    );
    println!(
        "  THE .properties GAP, STATED RATHER THAN ABSORBED. These figures are measured\n\
         \x20 through `ConfigCorpus::discover`, which walks the filesystem and DOES read\n\
         \x20 `.properties`. Production INGESTION does not: `source_facts` is reached only\n\
         \x20 for a file the plugin registry claims, no descriptor claims the extension, and\n\
         \x20 no story owns the artifact plugin that would. On this estate that is 31 of the\n\
         \x20 174 discovered sources. A key committed ONLY in a `.properties` file therefore\n\
         \x20 refuses in production as `config-key-missing` while resolving here — so these\n\
         \x20 figures are an UPPER BOUND on what the shipped pipeline admits, not a\n\
         \x20 measurement of it.\n"
    );
}

fn report_refusals(m: &super::Measurement) {
    let mut counts: BTreeMap<Refusal, usize> = BTreeMap::new();
    let outcomes = m
        .per_language
        .values()
        .flat_map(|s| s.sites.iter().filter(|s| s.gate_admitted))
        .flat_map(|s| s.key_outcomes.iter())
        .chain(m.broker.values().flat_map(|s| s.sites.iter()).flat_map(|s| s.outcomes.iter()));
    for outcome in outcomes.flatten() {
        if let KeyOutcome::Unresolved(refusal) = outcome {
            *counts.entry(*refusal).or_default() += 1;
        }
    }
    println!("\n--- why an accessor did not resolve to a key (both arms) ---");
    for refusal in Refusal::ALL {
        println!("  {:<38} {:>4}", refusal.label(), counts.get(&refusal).copied().unwrap_or(0));
    }
}

fn report_census(m: &super::Measurement) {
    println!("\n--- client-call census: every configuration-bound site, auditable ---");
    for (lang, stats) in &m.per_language {
        for site in stats
            .sites
            .iter()
            .filter(|s| s.gate_admitted && s.cr115 != Verdict::NotConfigurationBound)
        {
            println!(
                "{lang}  {}:{}  {}  ->  {}{}",
                site.file,
                site.line,
                site.text,
                site.cr115.label(),
                describe_keys(&site.key_outcomes, &m.config, m.config.module_of(&site.file)),
            );
        }
    }
    println!("\n--- broker census: every message-header publish site, auditable ---");
    for (lang, stats) in &m.broker {
        for site in &stats.sites {
            let kinds: Vec<&str> = site.kinds.iter().map(|k| k.label()).collect();
            println!(
                "{lang}  {}:{}  [{}]  {}  ->  {}{}",
                site.file,
                site.line,
                kinds.join(" + "),
                site.text,
                site.verdict.label(),
                describe_keys(&site.outcomes, &m.config, m.config.module_of(&site.file)),
            );
        }
    }
}

/// The per-operand key trace appended to a census line: the key, the scope's
/// agreement, and — when the sources disagree — the conflicting files by name,
/// which is what [CR-115] §3.4's refusal is required to report.
fn describe_keys(outcomes: &[Option<KeyOutcome>], corpus: &ConfigCorpus, module: &str) -> String {
    let mut out = String::new();
    for outcome in outcomes.iter().flatten() {
        match outcome {
            KeyOutcome::Unresolved(refusal) => {
                out.push_str(&format!("\n      <unresolved: {}>", refusal.label()));
            }
            KeyOutcome::Resolved { key, declared_in, source } => {
                let scoped = Agreement::of(&definitions_in(corpus, &canonical_key(key), Some(module)));
                let workspace = workspace_agreement(corpus, key);
                out.push_str(&format!(
                    "\n      {key}  via {}  module: {}  workspace: {}{}",
                    source.label(),
                    census_detail(&scoped),
                    census_detail(&workspace),
                    declared_in
                        .as_deref()
                        .map(|f| format!("  declared in {f}"))
                        .unwrap_or_default(),
                ));
                if let Agreement::Divergent(values) = &scoped {
                    for value in values {
                        out.push_str(&format!(
                            "\n        {:?} <- {}",
                            value.value,
                            value.sources.join(", ")
                        ));
                    }
                }
            }
        }
    }
    out
}

// ── The measurement ─────────────────────────────────────────────────────────

/// S-365 itself. Skips — loudly — when no corpus is configured, exactly as the
/// S-355 measurement it extends does.
#[test]
fn measure_configuration_agreement_over_the_reference_workspace() {
    let Some(root) = super::corpus_root() else {
        // **A skip that reads as a pass is a false green, so it is named on
        // stdout AND in the test name's own terms.** With `LOGOS_REF_WORKSPACE`
        // unset this test reports `ok` — not `ignored` — and nothing in this
        // repository sets the variable, so the S-382 AC5 assertions below
        // (79 / 2 / 4 profile-labelled values) do not run in any default `cargo
        // test`, nor in the gate. Review found exactly that: a mutation that
        // zeroed every AC5 figure still exited 0.
        //
        // Printed rather than `panic!`-ed because the reference workspace is a
        // developer-local checkout that CI does not have, and failing without
        // it would make the suite unrunnable off this machine. What the skip
        // must not do is stay invisible: the sprint's evidence has to say that
        // AC5 was verified by an env-set run, and this line is what a reader
        // greps for to check that claim.
        println!(
            "SKIPPED: LOGOS_REF_WORKSPACE is unset, so the S-365 measurement and the \
             S-382 AC5 assertions did NOT run. This test reports `ok` having measured \
             NOTHING. Run `LOGOS_REF_WORKSPACE=<path> cargo test -p logos-core \
             --features lang-java --test operand_resolvability` to measure."
        );
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-365 measurement and the S-382 AC5 assertions (see this module's docs for \
             the recorded finding). This run measured nothing."
        );
        return;
    };
    let m = super::measurement(&root);
    let verdicts = report(m);
    let client = verdicts.client_total();
    let broker = verdicts.broker_total();
    println!("\n--- recorded finding ---\n{RECORDED_FINDING}");

    assert!(
        !m.config.sources.is_empty(),
        "the corpus at {} yielded no application.{{yml,yaml,properties}} source, so no \
         agreement was measured — refusing to report a green run that measured nothing",
        root.display(),
    );
    // The materiality floor (5 sites / 10%) cannot notice a collapsed harness:
    // a broker arm that regressed to 6 sites of which 5 admit still reads
    // "material". These guard the ORDER OF MAGNITUDE the finding was recorded
    // at, loosely enough to survive corpus churn.
    assert!(
        client.denominator >= 100 && broker.denominator >= 40,
        "the recorded finding measured 140 client / 54 broker refused-today sites; this run \
         saw {} / {}. A collapsed denominator is a broken harness, not a new finding.",
        client.denominator,
        broker.denominator,
    );
    assert!(
        !m.base_urls.is_empty(),
        "the CR-115 base-URL arm found no `.baseUrl(…)` site, so half of what CR-115 is \
         titled after was not measured at all",
    );
    assert!(
        m.config.profiles().len() >= 2,
        "a corpus with fewer than two profiles cannot exercise profile disagreement, which \
         is the rule CR-115 §3.4 is judged on — this run measured agreement that could not \
         have failed",
    );
    assert!(
        m.per_language
            .values()
            .flat_map(|s| s.sites.iter())
            .any(|s| s.gate_admitted && s.cr115 == Verdict::Disagreement),
        "no site anywhere was refused for disagreement, so the agreement rule's refusal path \
         was never taken. A run that never refuses is not evidence the rule works — and a \
         regression that stopped producing Disagreed would make every arm look MORE material",
    );
    assert!(
        !m.properties.is_empty(),
        "the corpus at {} declares no @ConfigurationProperties class, so the accessor \
         hop was never exercised and every client-call refusal would be \
         `no @ConfigurationProperties class` by construction",
        root.display(),
    );

    // The verdict is what blocks CR-115 and CR-117, so it is asserted rather
    // than printed. Both arms are pinned independently: a change that flipped
    // one and not the other must fail here, not average out.
    //
    // The verdict is read off PRODUCTION source, per arm. The whole-arm figure
    // is printed beside it but does not decide anything: the broker arm's 38
    // admits are all IT test classes and its 13 production sites yield zero, so
    // a whole-arm reading would have recorded CR-117 CRA-01 as holding on the
    // strength of test fixtures.
    let client_main =
        verdicts.by_tree.get(&("client-call", Tree::Main)).copied().unwrap_or_default();
    let broker_main = verdicts.by_tree.get(&("broker", Tree::Main)).copied().unwrap_or_default();

    assert!(
        client_main.is_material(),
        "S-365's recorded finding is that CR-115 CRA-01 HOLDS: the agreement rule newly \
         admits a MATERIAL number of PRODUCTION client-call sites. This run admitted {} of \
         {} ({}%), below the floor of {MATERIAL_FLOOR_SITES} sites and {MATERIAL_FLOOR_PCT}% \
         declared before the measurement was taken. That is a falsification, not a broken \
         test: mark CR-115 CRA-01 falsified with this run's evidence and date, leave S-366 \
         and S-367 unplanned, and re-decide the CR before changing this assertion.",
        client_main.newly_admitted,
        client_main.denominator,
        client_main.percent(),
    );
    assert!(
        !broker_main.is_material(),
        "S-365's recorded finding is that CR-117 CRA-01 is FALSIFIED on production source: \
         all {} of the broker arm's production publish sites are refused, and every site the \
         rule admits is an IT test class. This run admitted {} of {} production sites ({}%), \
         which CLEARS the declared floor — the corpus or the harness has changed. Re-read \
         CR-117 §8 CRA-01 and re-decide the CR before changing this assertion; a genuine \
         production yield would make S-371 plannable again.",
        broker_main.denominator,
        broker_main.newly_admitted,
        broker_main.denominator,
        broker_main.percent(),
    );

    // ── S-382 AC5, pinned on the same run ───────────────────────────────
    //
    // The acceptance criterion names three figures over the production
    // client-call arm, and each is asserted rather than printed: 79 of 111 sites
    // carry a resolved template, the 2 disagreeing sites emit two
    // profile-labelled values each, and the 30 no-key sites refuse under a
    // distinct reason. They are pinned exactly, not as a floor, because the
    // criterion is a reproduction claim: a run that produced different numbers
    // has either changed the rule or changed the corpus, and both need a human.
    let s382 = s382_reading(m);
    // **The denominator has drifted from the recorded finding, and that is stated
    // here rather than absorbed into a looser assertion.** S-382 AC5 is written
    // over S-365's recorded figures — 79 of **111** production client-call sites,
    // 2 divergent, **30** no-key. This run reads 79 of **108**, 2 divergent, **27**
    // no-key: three sites have left the arm's denominator and all three were
    // `no-key` refusals. The drift is **not** this story's: measured on the same
    // reference workspace at the merge base (2026-09-12, before any S-382 change),
    // the run already read 79 of 108 / 137 whole-arm against the finding's 111 /
    // 140. S-365's own guard is `denominator >= 100`, so a three-site drift was
    // invisible to it by construction.
    //
    // Pinned at what the run produces rather than at what the criterion quotes,
    // because a test asserting 111 would fail on a corpus nothing in this
    // repository controls; the three figures the criterion is actually *about* —
    // 79 resolved, 2 divergent, each emitting two profile-labelled values — are
    // reproduced exactly.
    assert_eq!(
        (s382.denominator, s382.resolved, s382.divergent, s382.no_key),
        (108, 79, 2, 27),
        "S-382 AC5 names 79 production client-call sites resolved and 2 divergent, over \
         a denominator S-365 recorded as 111 (30 no-key) and this repository now \
         measures as 108 (27 no-key) — a corpus drift that predates S-382. This run \
         read {s382:?}. Re-measure against the reference workspace's recorded 1.4.7 \
         baseline before changing this assertion.",
    );
    assert_eq!(
        s382.divergent_values, 4,
        "each of the 2 disagreeing sites must emit TWO profile-labelled values — that \
         is ADR-64 decision point 3, and a site emitting one would be the \
         default-profile guess the decision refuses. This run emitted {}.",
        s382.divergent_values,
    );
    assert_eq!(
        s382.denominator,
        s382.resolved + s382.divergent + s382.no_key,
        "the three S-382 populations must partition the denominator exactly; a \
         remainder means a verdict is counted in neither, which is how a coverage \
         figure acquires a denominator nobody can reconstruct",
    );

    // The verdicts are pinned per arm and never on the combined figure: a
    // material arm must not rescue an immaterial one. The fixture
    // `an_immaterial_arm_is_not_rescued_by_a_material_one` pins that property
    // of `is_material` itself; this is the corpus-level application of it.
    assert!(
        verdicts.combined().newly_admitted == client.newly_admitted + broker.newly_admitted,
        "the combined figure must be the sum of the arms, not an average of them",
    );
}

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// These run on every `cargo test`, corpus or no corpus. They matter for the
// same reason the parent module's do: the measurement above skips without a
// corpus, so without them this file would pin nothing in CI — and the numbers
// it produces are what unblock (or close) [CR-115] and [CR-117].
//
// Every fixture drives the real Java grammar and goes through `judge`, the same
// entry point both corpus arms use.

#[cfg(test)]
mod fixtures {
    use super::*;
    use logos_core::plugin::LanguageRegistry;
    use tree_sitter::Parser;

    /// A corpus of exactly one Java compilation unit plus the configuration
    /// sources it is bound from.
    struct Fixture {
        corpus: ConfigCorpus,
        props: PropertiesIndex,
    }

    impl Fixture {
        /// `sources` are `(filename, body)` pairs placed at the corpus root, so
        /// every one of them is in the same (root) module scope.
        fn new(classes: &[&str], sources: &[(&str, &str)]) -> Self {
            let mut corpus = ConfigCorpus::default();
            for (name, body) in sources {
                let values = if name.ends_with(".properties") {
                    parse_properties(body)
                } else {
                    parse_yaml(body)
                };
                corpus.sources.push(ConfigSource {
                    path: (*name).to_string(),
                    profile: config_profile(name).flatten(),
                    module: String::new(),
                    values,
                });
            }
            let mut props = PropertiesIndex::for_plugins(&[java_plugin()]);
            for (i, class) in classes.iter().enumerate() {
                props.absorb_source(java_plugin(), &format!("Props{i}.java"), "", class);
            }
            props.seal();
            Self { corpus, props }
        }

        /// Judge the single `.uri(…)` argument of a Java unit against this
        /// fixture's configuration.
        fn judge_uri(&self, unit_source: &str) -> Verdict {
            self.judge_expression(unit_source, true)
        }

        /// Judge it as a broker topic instead — no route requirement.
        fn judge_topic(&self, unit_source: &str) -> Verdict {
            self.judge_expression(unit_source, false)
        }

        fn judge_expression(&self, unit_source: &str, route_required: bool) -> Verdict {
            let language = java_language();
            let mut parser = Parser::new();
            parser.set_language(&language).expect("java language");
            let tree = parser.parse(unit_source, None).expect("parse");
            let src = unit_source.as_bytes();
            let unit = Unit::build(tree.root_node(), src);
            let arg = sole_probe_argument(tree.root_node(), src);
            let mut nodes = Vec::new();
            super::super::operands(arg, src, &mut nodes);
            let kinds: Vec<OperandKind> = nodes
                .iter()
                .map(|n| super::super::classify(*n, src, &unit, FOLD_DEPTH))
                .collect();
            let judge_ctx = Judge {
                resolver: Resolver { corpus: &CorpusLookup(&self.corpus), module: "" },
                props: &self.props,
            };
            judge(&nodes, &kinds, src, &unit, judge_ctx, route_required).verdict
        }
    }

    /// The loaded registry, built once per test binary: every fixture below
    /// needs the Java plugin's `properties` query and its `[properties]` table,
    /// not just its grammar, and compiling the whole query set once beats
    /// compiling it per fixture.
    fn registry() -> &'static LanguageRegistry {
        static ONCE: std::sync::OnceLock<LanguageRegistry> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            LanguageRegistry::load(std::env::temp_dir()).expect("registry loads")
        })
    }

    /// The Java plugin — the descriptor + query pair the binding index reads its
    /// whole vocabulary from (S-381).
    fn java_plugin() -> &'static dyn logos_core::plugin::LanguagePlugin {
        registry().for_path("Probe.java").expect("java plugin")
    }

    fn java_language() -> tree_sitter::Language {
        java_plugin().language().clone()
    }

    /// The argument of the single `probe(…)` call a fixture unit must contain.
    /// A dedicated marker rather than `.uri(…)`, so a fixture never depends on
    /// the client-call query's verb gate to find its own expression.
    fn sole_probe_argument<'t>(root: tree_sitter::Node<'t>, src: &[u8]) -> tree_sitter::Node<'t> {
        let mut found = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
            drop(cursor);
            if node.kind() != "method_invocation" {
                continue;
            }
            let is_probe = node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src).ok())
                .is_some_and(|n| n == "probe");
            if !is_probe {
                continue;
            }
            let args = node.child_by_field_name("arguments").expect("argument list");
            let mut cursor = args.walk();
            let first = args.named_children(&mut cursor).next().expect("one argument");
            drop(cursor);
            found.push(first);
        }
        assert_eq!(found.len(), 1, "a fixture unit must contain exactly one probe(…) call");
        found[0]
    }

    const PROPS: &str = r#"
        @ConfigurationProperties(prefix = "mailserver.api")
        public class MailServerConfigurationApi {
            private String baseUrl;
            private String uriGetArchive;
        }
    "#;

    const TOPICS: &str = r#"
        @ConfigurationProperties(prefix = "spring.kafka.topics")
        public class KafkaTopics {
            private String archiveCommands;
        }
    "#;

    fn unit(body: &str) -> String {
        format!("public class Client {{\n{body}\n}}\n")
    }

    // ── relaxed binding and value canonicalisation ──────────────────────────
    //
    // The flattener, canonical-key and profile tests that lived here moved with
    // the code they cover into `logos_core::extract::config::corpus` (S-380,
    // AC2). What remains below is what they always sat beside: the BINDING
    // fixtures, which need this harness's `Fixture`/`judge` machinery and belong
    // to the resolution story rather than to the corpus.

    #[test]
    fn a_binding_cycle_terminates_instead_of_recursing_forever() {
        // `a = b; b = a` binds two DISTINCT ast nodes, so the identity check
        // alone does not terminate — FOLD_DEPTH is what does.
        let f = Fixture::new(&[PROPS], &[("application.yml", "x: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { String a = b; String b = a; probe(a); }"
            )),
            Verdict::NoKey(Refusal::UnrecognisedAccessor),
        );
    }

    #[test]
    fn a_binding_chain_still_reaches_the_configuration_accessor_behind_it() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert!(f
            .judge_uri(&unit(
                "private MailServerConfigurationApi mailServerConfigurationApi;\n\
                 void go() { String p = mailServerConfigurationApi.getUriGetArchive(); probe(p); }"
            ))
            .is_newly_admitted());
    }

    // ── Regression fixtures: every defect the S-365 review found ───────────
    //
    // Each of these failed before its fix. They are grouped because they share
    // a property: all were invisible to the 46 fixtures that preceded them, and
    // most moved a published number. The flattener half of this group moved to
    // `extract::config::corpus` with the code it covers (S-380); these are the
    // binding-side ones.

    #[test]
    fn a_value_annotated_field_does_not_answer_for_a_parameter_that_shadows_it() {
        // Was: `Unit` is file-scoped, so the field's @Value key resolved for the
        // PARAMETER of the same name and the site counted as admitted. Field /
        // parameter name collision is idiomatic Spring.
        let f = Fixture::new(&[], &[("application.yml", "app:\n  path: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "@Value(\"${app.path}\") private String path;\n\
                 void go(String path) { probe(path); }"
            )),
            Verdict::NoKey(Refusal::MethodParameter),
            "the operand is the parameter, whose value a caller supplies",
        );
    }

    #[test]
    fn a_bean_getter_named_get_env_is_not_an_environment_read() {
        // Was: the needle `getenv(` was matched against the lower-cased whole
        // operand, so `configApi.getEnv("A")` was misread as an environment
        // read and pre-empted the properties lookup.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getEnv(\"A\")); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::PropertyNotDeclared),
            "this is a bean getter, not System.getenv",
        );
    }

    #[test]
    fn an_environment_read_nested_in_an_expression_does_not_fabricate_a_key() {
        // Was: the delimiter search anchored on the FIRST `(` in the operand,
        // so this yielded the key `System.getenv("HOST` — a fabricated key
        // replacing a real refusal reason.
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        let verdict = f.judge_uri(&unit(
            "void go() { probe(Optional.ofNullable(System.getenv(\"HOST\")).orElse(\"/x\")); }",
        ));
        assert!(
            matches!(verdict, Verdict::NoKey(_) | Verdict::NotConfigurationBound),
            "expected a refusal, got {verdict:?}",
        );
    }

    #[test]
    fn an_environment_variable_is_never_admitted_even_when_a_key_agrees() {
        // CR-115 §3.3 puts environment variables out of scope. `canonical_key`
        // lower-cases and strips separators, so getenv("BASE_URL") collides
        // with a yml `base-url`; without the scope rule that collision counted
        // in the headline as though the repository proved it.
        let f = Fixture::new(&[], &[("application.yml", "base-url: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(System.getenv(\"BASE_URL\")); }")),
            Verdict::OutOfScopeSource,
        );
    }

    // Five index unit cases that lived here until S-381 are **not** missing —
    // they moved to `extract::config::binding`'s own test file with the code
    // they cover, the way S-380's 22 corpus cases did. What stays here is what
    // exercises the HARNESS: this one (a class body, where the unit suite covers
    // a record), the own-module preference across two modules, and everything
    // that goes through `judge`/`resolve_getter`.

    #[test]
    fn a_method_parameter_of_a_class_is_not_a_declared_property() {
        // Was: the record-component arm fired for ANY formal_parameter in the
        // class body, so a helper's parameter became a declared property —
        // loosening PropertyNotDeclared and, where a source defined the same
        // key, admitting the site outright.
        let mut props = PropertiesIndex::for_plugins(&[java_plugin()]);
        let body = r#"
            @ConfigurationProperties(prefix = "api")
            public class P { private String a; public void helper(String uriGetArchive) {} }
        "#;
        props.absorb_source(java_plugin(), "P.java", "", body);
        props.seal();
        let class = props.get("P", "").expect("indexed");
        assert!(class.properties.contains(&canonical_key("a")));
        assert!(
            !class.properties.contains(&canonical_key("uriGetArchive")),
            "a method parameter is not a bound property",
        );
    }

    #[test]
    fn a_name_bound_to_two_different_accessors_resolves_to_neither() {
        // `folded_text` in the parent refuses a name bound to two literals for
        // exactly this reason; resolution used to pick whichever the DFS
        // reached first, which is not even source order.
        let f = Fixture::new(
            &[PROPS, TOPICS],
            &[(
                "application.yml",
                "mailserver:\n  api:\n    uri-get-archive: /a\nspring:\n  kafka:\n                     topics:\n      archive-commands: ac\n",
            )],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "private MailServerConfigurationApi mailServerConfigurationApi;\n\
                 private KafkaTopics kafkaTopics;\n\
                 void go() { String u = mailServerConfigurationApi.getUriGetArchive(); \
                 u = kafkaTopics.getArchiveCommands(); probe(u); }"
            )),
            Verdict::NoKey(Refusal::AmbiguousBinding),
        );
    }

    #[test]
    fn a_refused_configuration_lookup_counts_wherever_it_sits_in_the_composition() {
        // Was: a LEADING unresolved lookup was fatal and counted; a TRAILING
        // one fell through to NotConfigurationBound, which Tally::add drops
        // from the denominator and the census hides. The same refusal was in
        // or out of the published denominator purely by position — shrinking
        // the denominator and so inflating the recovered share.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        let decl = "private MailServerConfigurationApi mailServerConfigurationApi;";
        let leading = f.judge_uri(&unit(&format!(
            "{decl}\nvoid go() {{ probe(mailServerConfigurationApi.getUriPutArchive() + \"/x\"); }}"
        )));
        let trailing = f.judge_uri(&unit(&format!(
            "{decl}\nvoid go() {{ probe(\"/x\" + mailServerConfigurationApi.getUriPutArchive()); }}"
        )));
        assert_eq!(leading, Verdict::NoKey(Refusal::PropertyNotDeclared));
        assert_eq!(
            trailing, leading,
            "position must not decide whether a refusal is counted",
        );
    }

    // ── the agreement rule (CR-115 §3.4) ────────────────────────────────────

    #[test]
    fn one_source_defining_the_key_agrees_with_itself() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NewlyAdmitted { resolved: "/a/{id}".to_string() },
        );
    }

    #[test]
    fn several_sources_agreeing_still_admits() {
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
            ],
        );
        assert!(f
            .judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                              private MailServerConfigurationApi mailServerConfigurationApi;"))
            .is_newly_admitted());
    }

    #[test]
    fn profile_disagreement_refuses_rather_than_defaulting_to_the_unprofiled_value() {
        // The rule CR-115 §3.4 is judged on: no default-profile fallback.
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/{id}\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    uri-get-archive: /b/{id}\n"),
            ],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::Disagreement,
        );
    }

    #[test]
    fn a_key_no_source_defines_is_a_missing_key_not_a_disagreement() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "other: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::MissingKey,
        );
    }

    #[test]
    fn a_value_that_is_itself_a_placeholder_proves_nothing() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: ${ARCHIVE_URI}\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::PlaceholderValue,
        );
    }

    #[test]
    fn a_properties_file_and_a_yaml_file_disagreeing_is_still_a_disagreement() {
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n"),
                ("application.properties", "mailserver.api.uri-get-archive=/b\n"),
            ],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::Disagreement,
        );
    }

    // ── accessor resolution, and each way it fails ──────────────────────────

    #[test]
    fn the_getter_suffix_binds_the_relaxed_key() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        // Declared as `uriGetArchive` on the class, spelled `uri-get-archive`
        // in the yml, read as `getUriGetArchive()` at the call site.
        assert!(f
            .judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                              private MailServerConfigurationApi mailServerConfigurationApi;"))
            .is_newly_admitted());
    }

    #[test]
    fn a_qualified_this_receiver_resolves_like_a_bare_one() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert!(f
            .judge_uri(&unit(
                "void go() { probe(this.mailServerConfigurationApi.getUriGetArchive()); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            ))
            .is_newly_admitted());
    }

    #[test]
    fn a_getter_for_a_property_the_class_does_not_declare_is_refused() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-put-archive: /a\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriPutArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::PropertyNotDeclared),
        );
    }

    #[test]
    fn a_receiver_whose_type_declares_no_properties_class_is_refused() {
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(someBean.getUriGetArchive()); }\n\
                               private SomeBean someBean;")),
            Verdict::NoKey(Refusal::NoPropertiesClass),
        );
    }

    #[test]
    fn a_receiver_the_unit_never_declares_is_refused_for_its_type_not_its_class() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(undeclared.getUriGetArchive()); }")),
            Verdict::NoKey(Refusal::ReceiverTypeUnknown),
        );
    }

    #[test]
    fn a_chained_getter_is_refused_as_a_nested_accessor_not_guessed_through() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(config.getApi().getUriGetArchive()); }\n\
                               private MailServerConfigurationApi config;")),
            Verdict::NoKey(Refusal::NestedAccessor),
        );
    }

    #[test]
    fn a_non_getter_member_call_is_refused_as_such() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.resolve()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NoKey(Refusal::NotAGetter),
        );
    }

    #[test]
    fn a_method_parameter_is_refused_as_one_call_frame_away() {
        // CR-117 CRA-04's residue, counted rather than mistaken for a defect.
        let f = Fixture::new(&[TOPICS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_topic(&unit("void send(String topic) { probe(topic); }")),
            Verdict::NoKey(Refusal::MethodParameter),
        );
    }

    #[test]
    fn a_value_annotated_field_resolves_in_one_hop_and_drops_its_default() {
        let f = Fixture::new(&[], &[("application.yml", "app:\n  path: /from-yml\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { probe(path); }\n\
                 @Value(\"${app.path:/fallback}\") private String path;"
            )),
            Verdict::NewlyAdmitted { resolved: "/from-yml".to_string() },
        );
    }

    #[test]
    fn an_environment_read_resolves_to_its_variable_and_is_reported_out_of_scope() {
        // Resolved on purpose — the census can then name the variable rather
        // than shrug at an "unreadable shape" — but never admitted: CR-115 §3.3
        // excludes environment variables because they are not committed.
        let f = Fixture::new(&[], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(System.getenv(\"API_HOST\")); }")),
            Verdict::OutOfScopeSource,
        );
    }

    // ── composition and the route rule ──────────────────────────────────────

    #[test]
    fn a_configuration_prefix_composes_with_a_literal_suffix() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "void go() { probe(mailServerConfigurationApi.getUriGetArchive() + \"/sub\"); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            )),
            Verdict::NewlyAdmitted { resolved: "/a/sub".to_string() },
        );
    }

    #[test]
    fn a_trailing_unresolvable_operand_becomes_the_placeholder_a_route_already_expresses() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a/\n")],
        );
        assert_eq!(
            f.judge_uri(&unit(
                "void go(String id) { probe(mailServerConfigurationApi.getUriGetArchive() + id); }\n\
                 private MailServerConfigurationApi mailServerConfigurationApi;"
            )),
            Verdict::NewlyAdmitted { resolved: "/a/{}".to_string() },
        );
    }

    #[test]
    fn a_leading_unresolvable_operand_refuses_the_whole_composition() {
        // CR-113 §3.2's rule, inherited: an unknown prefix is not a resolved
        // site whatever its tail says.
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: /a\n")],
        );
        let verdict = f.judge_uri(&unit(
            "void go(String base) { probe(base + mailServerConfigurationApi.getUriGetArchive()); }\n\
             private MailServerConfigurationApi mailServerConfigurationApi;",
        ));
        assert_eq!(verdict, Verdict::NoKey(Refusal::MethodParameter));
    }

    #[test]
    fn an_absolute_url_binds_a_route_because_the_host_need_not_resolve() {
        // CR-115 §3.4 is explicit: `http://pec-anagrafica/api/v1` names a
        // service-discovery target and binding matches the portable route key.
        assert!(binds_a_route("http://pec-anagrafica/api/v1"));
        assert!(binds_a_route("https://host/p"));
        assert!(binds_a_route("/relative-to-root"));
        assert!(!binds_a_route("relative"));
        assert!(!binds_a_route("http://host-with-no-path"));
        assert!(!binds_a_route("://p"));
    }

    #[test]
    fn a_resolved_value_that_names_no_route_is_reported_as_such_not_as_admitted() {
        let f = Fixture::new(
            &[PROPS],
            &[("application.yml", "mailserver:\n  api:\n    uri-get-archive: relative\n")],
        );
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(mailServerConfigurationApi.getUriGetArchive()); }\n\
                               private MailServerConfigurationApi mailServerConfigurationApi;")),
            Verdict::NotARoute { resolved: "relative".to_string() },
        );
    }

    #[test]
    fn the_broker_arm_admits_a_topic_that_names_no_route() {
        // The one place the arms differ: FR-WS-10 binds a topic on its value,
        // FR-WS-08 binds a client call on a route.
        let f = Fixture::new(
            &[TOPICS],
            &[("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")],
        );
        assert_eq!(
            f.judge_topic(&unit("void go() { probe(kafkaTopics.getArchiveCommands()); }\n\
                                 private KafkaTopics kafkaTopics;")),
            Verdict::NewlyAdmitted { resolved: "ac".to_string() },
        );
    }

    #[test]
    fn a_topic_bean_named_without_a_configuration_needle_still_resolves() {
        // The defect the first corpus run exposed: S-355's taxonomy labels an
        // operand by the SPELLING of its receiver, and `kafkaTopics` contains
        // none of its needles. Resolution here is type-driven, so the label
        // does not gate it — a 54-site corpus must not report a denominator
        // of 3.
        let f = Fixture::new(
            &[TOPICS],
            &[("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")],
        );
        assert!(!looks_like_configuration_needle("kafkaTopics"), "premise of this test");
        assert!(f
            .judge_topic(&unit("void go() { probe(kafkaTopics.getArchiveCommands()); }\n\
                                private KafkaTopics kafkaTopics;"))
            .is_newly_admitted());
    }

    /// The parent module's name heuristic, reached through a local shim so the
    /// test above states its premise instead of assuming it.
    fn looks_like_configuration_needle(text: &str) -> bool {
        super::super::looks_like_configuration(text)
    }

    #[test]
    fn a_site_whose_every_operand_folds_is_outside_this_measurement() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit(
                "static final String P = \"/a\";\nvoid go() { probe(P + \"/b\"); }"
            )),
            Verdict::NotConfigurationBound,
            "S-355 already measured folding; S-365 must not re-count it",
        );
    }

    #[test]
    fn a_single_static_literal_is_already_admitted_not_newly() {
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go() { probe(\"/a\"); }")),
            Verdict::AlreadyAdmitted,
        );
    }

    // ── the module scope, and the collision it exists to prevent ────────────

    #[test]
    fn a_use_site_prefers_the_properties_class_its_own_module_declares() {
        // The corpus defect: two members declare `MailServerConfigurationApi`
        // under one prefix with different property sets, and a workspace-wide
        // "first wins" index reported eleven false `property not declared`
        // refusals against the wrong member's class.
        let mut props = PropertiesIndex::for_plugins(&[java_plugin()]);
        let narrow = r#"
            @ConfigurationProperties(prefix = "mailserver.api")
            public class MailServerConfigurationApi { private String uriGetArchive; }
        "#;
        let wide = r#"
            @ConfigurationProperties(prefix = "mailserver.api")
            public class MailServerConfigurationApi {
                private String uriGetArchive;
                private String uriUpdateArchive;
            }
        "#;
        for (module, body) in [("archive-api", narrow), ("archive-manager", wide)] {
            props.absorb_source(java_plugin(), &format!("{module}/C.java"), module, body);
        }
        props.seal();

        assert!(
            props.collisions.contains("MailServerConfigurationApi"),
            "declarations that disagree must be recorded as a collision",
        );
        let own = props.get("MailServerConfigurationApi", "archive-manager").expect("own module");
        assert!(own.properties.contains(&canonical_key("uriUpdateArchive")));
        assert!(
            props.get("MailServerConfigurationApi", "unrelated-member").is_none(),
            "a colliding name must resolve to nothing rather than to a guess",
        );
    }

    // ── the two arms are never averaged ─────────────────────────────────────

    // ── The accounting itself, which every published number passes through ─

    #[test]
    fn the_tally_counts_each_verdict_into_exactly_one_bucket() {
        // `Tally::add` was reached only by the corpus walk: both materiality
        // fixtures built `Tally` literals instead. A one-line change letting
        // `AlreadyAdmitted` fall through to `denominator += 1` would move every
        // percentage in the finding with nothing failing.
        let cfg = [OperandKind::ConfigurationLookup];
        let lit = [OperandKind::Literal];
        let other = [OperandKind::Other];
        let mut t = Tally::default();
        t.add(&Verdict::AlreadyAdmitted, &lit);
        t.add(&Verdict::NotConfigurationBound, &lit);
        t.add(&Verdict::NewlyAdmitted { resolved: "/a".into() }, &cfg);
        t.add(&Verdict::NewlyAdmitted { resolved: "/b".into() }, &other);
        t.add(&Verdict::Disagreement, &cfg);
        t.add(&Verdict::MissingKey, &cfg);
        t.add(&Verdict::PlaceholderValue, &cfg);
        t.add(&Verdict::NoKey(Refusal::UnboundName), &other);
        t.add(&Verdict::NotARoute { resolved: "x".into() }, &cfg);
        t.add(&Verdict::OutOfScopeSource, &cfg);

        assert_eq!(t.already_admitted, 1);
        assert_eq!(
            t.denominator, 8,
            "AlreadyAdmitted and NotConfigurationBound are excluded from the denominator",
        );
        assert_eq!(
            t.newly_admitted + t.disagreement + t.missing_key + t.placeholder + t.no_key
                + t.not_a_route + t.out_of_scope,
            t.denominator,
            "the buckets must partition the denominator exactly",
        );
        assert_eq!(t.config_labelled, 6);
        assert_eq!(t.newly_admitted, 2);
        assert_eq!(t.newly_admitted_labelled, 1, "the labelled subset is CR-115's criterion");
    }

    #[test]
    fn merge_carries_every_field() {
        let a = Tally {
            denominator: 1,
            config_labelled: 2,
            newly_admitted_labelled: 3,
            newly_admitted: 4,
            disagreement: 5,
            missing_key: 6,
            placeholder: 7,
            no_key: 8,
            not_a_route: 9,
            out_of_scope: 10,
            already_admitted: 11,
        };
        let mut b = a;
        b.merge(&a);
        assert_eq!(
            (
                b.denominator,
                b.config_labelled,
                b.newly_admitted_labelled,
                b.newly_admitted,
                b.disagreement,
                b.missing_key,
                b.placeholder,
                b.no_key,
                b.not_a_route,
                b.out_of_scope,
                b.already_admitted,
            ),
            (2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22),
            "a dropped line in `merge` silently under-reports a bucket",
        );
    }

    #[test]
    fn every_refusal_variant_is_listed_in_all_with_a_distinct_label() {
        // `report_refusals` iterates `ALL`; a variant added without extending
        // it disappears from the published refusal census silently.
        let labels: BTreeSet<&str> = Refusal::ALL.iter().map(|r| r.label()).collect();
        assert_eq!(labels.len(), Refusal::ALL.len(), "two refusals share a label");
        // An exhaustive match: adding a variant fails to compile until `ALL`
        // and this list are both extended.
        for refusal in Refusal::ALL {
            match refusal {
                Refusal::NestedAccessor
                | Refusal::MethodParameter
                | Refusal::UnboundName
                | Refusal::AmbiguousBinding
                | Refusal::NotAGetter
                | Refusal::ReceiverTypeUnknown
                | Refusal::NoPropertiesClass
                | Refusal::PropertyNotDeclared
                | Refusal::UnrecognisedAccessor => {}
            }
        }
    }

    #[test]
    fn a_name_the_unit_never_binds_is_refused_as_unbound() {
        // The largest single refusal bucket in the published finding (41 of the
        // client arm's 59) had no fixture at all.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_topic(&unit("void go() { probe(neverBoundAnywhere); }")),
            Verdict::NoKey(Refusal::UnboundName),
        );
    }

    #[test]
    fn a_topic_with_a_trailing_unresolvable_operand_is_refused_because_it_is_not_a_topic() {
        // The one place the arms genuinely differ, and every broker fixture
        // passed a SINGLE operand, so `i == 0 || !route_required` was never
        // exercised on the `!route_required` half. Mutating it to `i == 0`
        // left every fixture green while turning refused multi-operand topics
        // into admitted ones — inflating exactly the number CR-117 turns on.
        let sources =
            [("application.yml", "spring:\n  kafka:\n    topics:\n      archive-commands: ac\n")];
        let f = Fixture::new(&[TOPICS], &sources);
        let source = unit(
            "private KafkaTopics kafkaTopics;\n\
             void send(String suffix) { probe(kafkaTopics.getArchiveCommands() + suffix); }",
        );
        assert_eq!(
            f.judge_topic(&source),
            Verdict::NoKey(Refusal::MethodParameter),
            "FR-WS-10 binds a topic on its value: a topic with a `{{}}` in it is not a topic",
        );
        assert_eq!(
            f.judge_uri(&source),
            Verdict::NotARoute { resolved: "ac{}".to_string() },
            "the client arm SPENDS the same trailing operand as a `{{}}` placeholder and \
             composes a template — it is then judged on the route shape, not refused at the \
             operand. (`ac{{}}` is no route, so this one still ends refused; what differs is \
             WHY.)",
        );
    }

    #[test]
    fn refusal_precedence_is_worst_fault_first_across_several_operands() {
        // Every previous fixture had exactly one configuration operand, so the
        // Missing -> Placeholder -> Disagreed ordering was never exercised.
        let f = Fixture::new(
            &[PROPS],
            &[
                ("application.yml", "mailserver:\n  api:\n    base-url: /a\n"),
                ("application-prod.yml", "mailserver:\n  api:\n    base-url: /b\n"),
            ],
        );
        // `uriGetArchive` is declared on the class but defined by no source
        // (missing); `baseUrl` is defined by two sources that disagree.
        assert_eq!(
            f.judge_uri(&unit(
                "private MailServerConfigurationApi c;\n\
                 void go() { probe(c.getBaseUrl() + c.getUriGetArchive()); }"
            )),
            Verdict::MissingKey,
            "a missing key is a deeper fault than a disagreement and must be reported first",
        );
    }

    #[test]
    fn a_literal_plus_a_parameter_is_still_s355_territory() {
        // The guard exists for exactly this shape: nothing is fatal, nothing
        // resolved to a key, and without it the site composes to `/a/{}`,
        // clears `binds_a_route` and inflates the CR-115 headline.
        let f = Fixture::new(&[PROPS], &[("application.yml", "a: 1\n")]);
        assert_eq!(
            f.judge_uri(&unit("void go(String id) { probe(\"/a/\" + id); }")),
            Verdict::NotConfigurationBound,
        );
    }

    #[test]
    fn the_production_and_test_trees_are_told_apart() {
        // The split that inverted CR-117's verdict.
        for main in [
            "archive-api/src/main/java/com/x/KafkaProducer.java",
            "internal/flow/handler.go",
            "web/src/app.ts",
        ] {
            assert_eq!(Tree::of(main), Tree::Main, "{main}");
        }
        for test in [
            "archive-api/src/test/java/com/x/KafkaProducerIT.java",
            "archive-api/src/main/java/com/x/FooTest.java",
            "internal/flow/handler_test.go",
            "tests/e2e/probe.java",
        ] {
            assert_eq!(Tree::of(test), Tree::Test, "{test}");
        }
    }

    #[test]
    fn an_immaterial_arm_is_not_rescued_by_a_material_one() {
        // The acceptance criterion this test exists for: "a material figure for
        // one and an immaterial figure for the other is a real possible outcome
        // and must not be averaged away".
        let mut verdicts = Verdicts::default();
        verdicts.client.insert(
            "java".to_string(),
            Tally { denominator: 100, newly_admitted: 90, ..Tally::default() },
        );
        verdicts.broker.insert(
            "java".to_string(),
            Tally { denominator: 100, newly_admitted: 1, ..Tally::default() },
        );
        assert!(verdicts.client_total().is_material());
        assert!(!verdicts.broker_total().is_material());
        assert!(
            verdicts.combined().is_material(),
            "the combined figure would clear the floor — which is exactly why the \
             per-arm verdicts, not the combined one, decide the change requests",
        );
    }

    #[test]
    fn the_materiality_floor_needs_both_a_share_and_a_count() {
        let tiny = Tally { denominator: 4, newly_admitted: 4, ..Tally::default() };
        assert!(!tiny.is_material(), "4 of 4 is 100% of almost nothing");
        let thin = Tally { denominator: 1000, newly_admitted: 50, ..Tally::default() };
        assert!(!thin.is_material(), "50 sites is 5% — below the declared share");
        let real = Tally { denominator: 100, newly_admitted: 50, ..Tally::default() };
        assert!(real.is_material());
    }

    #[test]
    fn a_zero_denominator_is_never_material() {
        assert!(!Tally::default().is_material());
        assert_eq!(Tally::default().percent(), 0);
    }
}
