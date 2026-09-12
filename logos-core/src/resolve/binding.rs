//! **Configuration-bound operand resolution** — a committed configuration value
//! admitted as evidence (S-382, [CR-121], [FR-WS-19], [FR-WS-08], [ADR-64]).
//!
//! The arm normalizers ([`super::http_client_call`], the broker arm) reduce a
//! call site to a target *only when the site itself proves one*. This module is
//! the one step further [ADR-64] licenses: where the site names a **configuration
//! key**, the value that key is committed to — in a version-controlled file
//! inside the reading module — is read and admitted, and everything else stays
//! refused.
//!
//! # What is evidence, and what is a guess ([ADR-64])
//!
//! > **A value the repository commits is evidence. A value assembled at runtime
//! > is not.**
//!
//! The test has three parts and all three must hold: the value is **committed**
//! (a version-controlled file, so the commit that set it can be named), **within
//! reach of the reading module** (so the join is a name lookup, not a search of
//! the estate), and **read, not reconstructed** (where the bytes cannot be read
//! exactly, nothing is yielded — never an approximation).
//!
//! The third part is the one that is easy to state and easy to lose, and the
//! corpus S-380 promoted fixes its direction: a quoted scalar carrying an escape
//! the flattener cannot decode yields **no pair at all** rather than a truncated
//! one, because two sources committing *different* values would otherwise be
//! recorded as committing the *same* one — a fabricated agreement assembled
//! entirely out of committed bytes. Every refusal below points the same way.
//! **Under-reading is the safe direction; over-reading is fabrication.**
//!
//! # Overlay disagreement is represented, never averaged and never refused
//!
//! This is the one rule that differs from the superseded S-366 reading, and it
//! is [ADR-64] decision point 3: where overlays define a key differently,
//! **every** value is retained with its profile set, and the consumer carries
//! the profiles that produce it. A single resolved value is simply wrong on an
//! estate that varies hosts and topics per deployment, and picking the
//! unprofiled one is the default-profile guess the decision exists to refuse.
//!
//! So [`Agreement::Divergent`] is an *outcome*, not a refusal: it admits, and it
//! admits **all** of it. The refusals are the four rows below.
//!
//! # What stays refused ([ADR-64] "What stays refused")
//!
//! | Refused | Reported as |
//! |---------|-------------|
//! | An environment variable with no committed default, a `System.getenv`/`process.env` read | [`ValueRefusal::Uncommitted`] |
//! | A config server, Consul, etcd, a secret store, a Kubernetes ConfigMap/Secret | [`ValueRefusal::MissingKey`] — refused **structurally**, by the discovery gate: no such file is a configuration source, so the key is committed nowhere the corpus admits |
//! | A value that is itself an unresolved placeholder | [`ValueRefusal::PlaceholderValue`] |
//! | A key no committed source defines | [`ValueRefusal::MissingKey`] |
//! | An accessor that resolves to no key at all | [`Refusal`] — nine distinct faults, never a bucket labelled "other" |
//!
//! **Five refusal fixtures, three reasons, and the arithmetic is stated rather
//! than hidden.** [FR-WS-08]'s criterion names five populations that must keep
//! refusing; they map onto three [`ValueRefusal`] variants, because the config
//! server / secret store / ConfigMap row and the undefined-key row are refused by
//! the *same* mechanism (the key reaches the corpus from no admitted source) and
//! a fourth variant for them would advertise a distinction this module cannot
//! observe — the advertised-but-empty capability [NFR-CC-04] disfavours. What
//! differs between those two is *why* no source defines the key, which is proved
//! by the discovery gate, not by this module. `tests.rs` carries one case per
//! population regardless, because the populations are what must not regress.
//!
//! # Provenance is mandatory ([NFR-CC-04], [ADR-64])
//!
//! > **An admitted value must never be indistinguishable from an observed one.**
//!
//! Every admitted value travels as a [`ConfigBound`] — the key, the defining
//! sources and the profile set — inside a [`Provenance`] that a directly-observed
//! literal also carries, as [`Provenance::Literal`]. The distinction is
//! structural: a consumer cannot render one without the other being expressible,
//! because they are the same type.
//!
//! [`Provenance`] has **three** states, not two, and the third is not a
//! convenience: a reference that *names* a key the sources do not admit
//! ([`Provenance::ConfigUnresolved`]) proved only an indirection, so filing it as
//! a literal would report unresolved text as observed evidence — the same
//! over-claim in the opposite direction.
//!
//! [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
//! [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
//! [FR-WS-08]: ../../../docs/specs/requirements/FR-WS-08.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::extract::config::corpus::canonical_key;
use crate::graph_store::ConfigDefinition;

/// The opening delimiter of a configuration placeholder (`${key}`).
const PLACEHOLDER_OPEN: &str = "${";

/// The most compositions one profile may prove for a template before it proves
/// **none** ([`substitutions`]).
///
/// Not a truncation limit: past it the profile composes nothing at all, because
/// admitting an arbitrary prefix would quietly falsify [ADR-64] decision point
/// 3's "every value is retained". Sized far above the reference estate's worst
/// case (one placeholder, one value) so that reaching it means the corpus is a
/// shape this resolver was never measured on.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
const MAX_COMPOSITIONS: usize = 64;

/// Separates a placeholder's key from its inline default (`${a.b:fallback}`).
///
/// The default is **read, never used**: a committed default proves what the
/// source falls back to, not what the deployment holds, and admitting it would
/// resolve an uncommitted environment variable to its fallback — which is the
/// first row of the refusal table. The key is everything before the first `:`.
const PLACEHOLDER_DEFAULT: char = ':';

// ── How an operand reached its key ──────────────────────────────────────────

/// How an operand reached the configuration key it names (S-365, promoted).
///
/// The distinction is not bookkeeping: [`Environment`](KeySource::Environment) is
/// refused *whatever the committed sources say*, because
/// [`canonical_key`] lower-cases and strips separators, so a `getenv("BASE_URL")`
/// would otherwise collide with a yaml `base-url` and be admitted as though the
/// repository proved it ([ADR-64]).
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeySource {
    /// A getter on a configuration-bound (`@ConfigurationProperties`) bean,
    /// resolved through `PropertiesIndex::bind`
    /// ([`crate::extract::config::binding`], S-381).
    Properties,
    /// An annotation naming the key at the use site (`@Value("${key}")`).
    ValueAnnotation,
    /// A `${key}` placeholder written into the operand's own literal.
    Placeholder,
    /// Not configuration at all: a literal or same-unit constant the call site
    /// itself proves. Carries the empty key.
    CallSite,
    /// An environment read. Resolved so a census can name the variable, but
    /// **never** admitted ([ADR-64]).
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    Environment,
}

impl KeySource {
    /// The census word for this source.
    pub fn label(self) -> &'static str {
        match self {
            Self::Properties => "a configuration-bound bean",
            Self::ValueAnnotation => "a value annotation",
            Self::Placeholder => "a placeholder in the operand",
            Self::CallSite => "the call site",
            Self::Environment => "the environment",
        }
    }

    /// Whether a key reached this way can ever be admitted.
    ///
    /// `false` for [`Environment`](Self::Environment) alone, and asked *before*
    /// agreement rather than after it.
    pub fn is_committed(self) -> bool {
        !matches!(self, Self::Environment)
    }
}

/// Why an accessor did **not** resolve to a configuration key (S-365, promoted).
///
/// Each variant is a distinct, countable fault so the residue is a diagnosis
/// rather than a bucket labelled "other" ([NFR-CC-04]).
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// A getter chained on another call — `a.getB().getC()`. Resolving it needs
    /// the nested type's own binding, which is never guessed.
    NestedAccessor,
    /// The operand is a method parameter: its value originates one call frame
    /// away, beyond the one-hop bound [FR-WS-23] measures rather than assumes.
    ///
    /// [FR-WS-23]: ../../../docs/specs/requirements/FR-WS-23.md
    MethodParameter,
    /// A name the compilation unit does not bind at all.
    UnboundName,
    /// A name the unit binds to two different configuration accessors. The
    /// source does not prove which one the call site sees, so neither is used.
    AmbiguousBinding,
    /// The receiver resolves, but the member read is not an accessor.
    NotAGetter,
    /// The receiver's declared type is not visible in the compilation unit.
    ReceiverTypeUnknown,
    /// The declared type names no configuration-bound class in the corpus — the
    /// bean is external, or bound some other way.
    NoPropertiesClass,
    /// The class is indexed, but declares no property matching the accessor.
    PropertyNotDeclared,
    /// A configuration-shaped operand in none of the recognised forms.
    UnrecognisedAccessor,
}

impl Refusal {
    /// The census word for this refusal.
    pub fn label(self) -> &'static str {
        match self {
            Self::NestedAccessor => "nested accessor",
            Self::MethodParameter => "method parameter (one call frame away)",
            Self::UnboundName => "name unbound in this unit",
            Self::AmbiguousBinding => "name bound to two different accessors",
            Self::NotAGetter => "not a getter",
            Self::ReceiverTypeUnknown => "receiver type unknown",
            Self::NoPropertiesClass => "no configuration-bound class",
            Self::PropertyNotDeclared => "property not declared on the class",
            Self::UnrecognisedAccessor => "unrecognised accessor shape",
        }
    }

    /// Every variant, so a census enumerates the residue rather than sampling it.
    pub const ALL: [Self; 9] = [
        Self::NestedAccessor,
        Self::MethodParameter,
        Self::UnboundName,
        Self::AmbiguousBinding,
        Self::NotAGetter,
        Self::ReceiverTypeUnknown,
        Self::NoPropertiesClass,
        Self::PropertyNotDeclared,
        Self::UnrecognisedAccessor,
    ];
}

/// Why a key that *was* named admitted no committed value ([ADR-64]).
///
/// Disagreement is deliberately absent: it is no longer a refusal — see
/// [`Agreement::Divergent`] and this module's docs.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueRefusal {
    /// The value is supplied at runtime by something the repository does not
    /// commit — an environment variable with no committed default, a
    /// `System.getenv`/`process.env` read. Decided from the
    /// [`KeySource`] **before** any agreement is taken.
    Uncommitted,
    /// At least one defining source's value is itself an unresolved `${…}`
    /// indirection: it proves the indirection, not the value.
    PlaceholderValue,
    /// The committed sources prove no value for the operand.
    ///
    /// **Three mechanisms reach it, and they are one reason on purpose.** No
    /// committed source defines the key at all; or the only sources that would
    /// are not configuration sources under the discovery gate (a config server,
    /// Consul/etcd, a secret store, a Kubernetes ConfigMap), so nothing they
    /// hold reaches the corpus; or — for a multi-key template — every key admits
    /// individually but **no single profile proves a value for all of them at
    /// once**, so no committed composition exists. In each case the repository
    /// proves nothing for what the site reads, which is the distinction that
    /// matters to a consumer.
    ///
    /// Splitting the third into its own variant was considered and rejected:
    /// it occurs **zero** times on the reference estate, and a reason with no
    /// real producer is the advertised-but-empty capability [NFR-CC-04]
    /// disfavours — the test S-378 removed `schema-mismatch` for failing.
    ///
    /// Kept **distinct from disagreement** so the two are never conflated in a
    /// count ([ADR-64]) — and disagreement is not a refusal at all.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    MissingKey,
}

impl ValueRefusal {
    /// The census word for this refusal.
    pub fn label(self) -> &'static str {
        match self {
            Self::Uncommitted => "not committed by the repository",
            Self::PlaceholderValue => "the committed value is itself a placeholder",
            Self::MissingKey => "no committed source defines it",
        }
    }

    /// Every variant. Unlike [`Refusal::ALL`] this feeds no census — the
    /// coverage tier converts to its own vocabulary and never enumerates this
    /// enum — so its one job is to make the distinct-label guard in `tests.rs`
    /// exhaustive: a fourth variant does not compile until it is listed here.
    pub const ALL: [Self; 3] = [Self::Uncommitted, Self::PlaceholderValue, Self::MissingKey];
}

// ── What the committed sources prove ────────────────────────────────────────

/// One committed value of a key, with the profiles and files that prove it
/// ([ADR-64], [FR-WS-19]).
///
/// The unprofiled source is reported by [`unprofiled`](Self::unprofiled) rather
/// than by a fabricated profile name: inventing a `"default"` label would make an
/// estate that genuinely declares a `default` profile indistinguishable from one
/// that does not.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfiledValue {
    /// The committed literal, exactly as the source proves it.
    pub value: String,
    /// The profiles whose sources prove this value, sorted and deduplicated.
    pub profiles: Vec<String>,
    /// Whether the **unprofiled** source proves it — stated even when `false`,
    /// so a reader never infers it from an empty profile list ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub unprofiled: bool,
    /// The project-relative files proving it, sorted and deduplicated — the
    /// "defining sources" half of the provenance [FR-WS-19] requires.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    pub sources: Vec<String>,
}

/// What a key's committed sources prove about its value.
///
/// [`Agreed`](Self::Agreed) and [`Divergent`](Self::Divergent) both **admit**;
/// the other two refuse. That asymmetry is [ADR-64] decision point 3 and is the
/// single rule this module changes relative to the superseded S-366 reading.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Agreement {
    /// Exactly one value across every source that defines the key.
    Agreed(ProfiledValue),
    /// Two or more distinct values. **Every one is retained** with its profile
    /// set; none is preferred and none is dropped. Ordered by value so the
    /// answer is stable across runs ([NFR-RA-06]).
    ///
    /// [NFR-RA-06]: ../../../docs/specs/requirements/NFR-RA-06.md
    Divergent(Vec<ProfiledValue>),
    /// Defined, but at least one source's value is itself a `${…}` indirection,
    /// so the sources do not prove a value at all.
    Placeholder { sources: usize },
    /// No committed source defines the key.
    Missing,
}

impl Agreement {
    /// What `definitions` prove — every definition of one canonical key, as
    /// [`GraphStore::config_definitions`](crate::graph_store::GraphStore::config_definitions)
    /// returns them.
    ///
    /// The caller has already scoped the definitions to the reading module; this
    /// function judges what it is given and scopes nothing itself.
    pub fn of(definitions: &[ConfigDefinition]) -> Self {
        if definitions.is_empty() {
            return Self::Missing;
        }
        if definitions.iter().any(|d| d.value.contains(PLACEHOLDER_OPEN)) {
            return Self::Placeholder { sources: definitions.len() };
        }
        let mut by_value: BTreeMap<&str, (BTreeSet<&str>, bool, BTreeSet<&str>)> = BTreeMap::new();
        for def in definitions {
            let entry = by_value.entry(def.value.as_str()).or_default();
            match def.profile.as_deref() {
                Some(profile) => {
                    entry.0.insert(profile);
                }
                None => entry.1 = true,
            }
            entry.2.insert(def.path.as_str());
        }
        let values: Vec<ProfiledValue> = by_value
            .into_iter()
            .map(|(value, (profiles, unprofiled, sources))| ProfiledValue {
                value: value.to_string(),
                profiles: profiles.into_iter().map(str::to_string).collect(),
                unprofiled,
                sources: sources.into_iter().map(str::to_string).collect(),
            })
            .collect();
        match <[ProfiledValue; 1]>::try_from(values) {
            Ok([only]) => Self::Agreed(only),
            Err(values) => Self::Divergent(values),
        }
    }

    /// The census word for this outcome.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Agreed(_) => "agreed",
            Self::Divergent(_) => "profile-divergent",
            Self::Placeholder { .. } => "placeholder value",
            Self::Missing => "missing key",
        }
    }

    /// The full statement [FR-WS-19] asks for: how many sources define the key
    /// **and** what they prove. `label()` alone answers only the second half, so
    /// an agreed census line never said how much evidence stood behind it.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    pub fn detail(&self) -> String {
        match self {
            Self::Agreed(v) => {
                format!("agreed across {} source(s): {:?}", v.sources.len(), v.value)
            }
            Self::Divergent(values) => format!(
                "{} distinct values across {} source(s), every one retained with its profile set",
                values.len(),
                values.iter().map(|v| v.sources.len()).sum::<usize>(),
            ),
            Self::Placeholder { sources } => {
                format!("defined by {sources} source(s), but the value is a `${{…}}` indirection")
            }
            Self::Missing => "no committed source defines it".to_string(),
        }
    }

}

// ── Provenance ──────────────────────────────────────────────────────────────

/// The provenance of an **admitted** configuration-resolved value: the key, the
/// defining sources and the profile set ([FR-WS-19], [NFR-CC-04], [ADR-64]).
///
/// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConfigBound {
    /// The canonical key the value was read from ([`canonical_key`]).
    pub key: String,
    /// How the operand reached that key.
    pub source: KeySource,
    /// One entry per distinct committed value. More than one is an overlay
    /// divergence, retained rather than averaged ([ADR-64]).
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    pub values: Vec<ProfiledValue>,
}

impl ConfigBound {
    /// Every profile that proves any of this key's values, sorted — the
    /// "profile set" half of the provenance.
    pub fn profiles(&self) -> Vec<&str> {
        let set: BTreeSet<&str> =
            self.values.iter().flat_map(|v| v.profiles.iter().map(String::as_str)).collect();
        set.into_iter().collect()
    }

    /// Whether this key's sources disagree — `true` when more than one distinct
    /// value is retained.
    pub fn is_divergent(&self) -> bool {
        self.values.len() > 1
    }
}

/// Whether a value was **observed** at the call site or **admitted** from
/// committed configuration ([NFR-CC-04], [ADR-64], [BR-52]).
///
/// The one field that keeps an admitted value from ever being indistinguishable
/// from a directly-observed literal. Both cases are the same type, so no surface
/// can render one and silently omit the other.
///
/// Internally tagged, so the wire form of a literal is `{"provenance":"literal"}`
/// and of an admitted value
/// `{"provenance":"config-bound","bound":[{"key":…,"source":…,"values":[…]}]}`
/// — one key a consumer switches on, never a nullable sibling field it must
/// infer from. The evidence is nested under `bound`, one entry per key, and NOT
/// flattened onto the row: it was flattened at this story's first commit, and
/// the two readers written against that shape outlived it.
///
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
/// [BR-52]: ../../../docs/specs/software-spec.md#327-workspace-federation
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "provenance", rename_all = "kebab-case")]
pub enum Provenance {
    /// Written at the call site and read verbatim — the pre-S-382 case, and
    /// still the only one that needs no configuration at all.
    Literal,
    /// Read from committed configuration, carrying its evidence — **one entry
    /// per configuration key the target names**, in the order the target names
    /// them.
    ///
    /// A `Vec`, not a single [`ConfigBound`], and the difference is the whole
    /// requirement rather than ergonomics: [FR-WS-19] AC6 asks that *every*
    /// admitted value carry the key, the defining sources and the profile set,
    /// and a two-placeholder target such as `${svc.host}${svc.path}/orders` is
    /// two keys, two source sets and two profile sets. Carrying only the first
    /// would leave the second key's evidence nameable from no surface at all.
    ///
    /// [FR-WS-19]: ../../../docs/specs/requirements/FR-WS-19.md
    ConfigBound { bound: Vec<ConfigBound> },
    /// The target **names** configuration keys and the committed sources admit
    /// no value for them, so nothing was read at all — neither an observed
    /// literal nor an admitted value.
    ///
    /// A third state rather than a reuse of [`Literal`](Self::Literal), because
    /// the two are different claims: a literal says *the call site proves this
    /// text*, and this says *the call site proves only an indirection*. Filing a
    /// refused configuration reference as a literal would report unresolved text
    /// as observed evidence — the same over-claim in the opposite direction to
    /// the one [ADR-64] forbids.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    ConfigUnresolved {
        /// The canonical keys the target names, in source order.
        keys: Vec<String>,
        /// Why they admitted nothing.
        refusal: ValueRefusal,
    },
}

impl Provenance {
    /// The wire word, so a human renderer need not match on the variant.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::ConfigBound { .. } => "config-bound",
            Self::ConfigUnresolved { .. } => "config-unresolved",
        }
    }

    /// The evidence, for the admitted case: one entry per key the target names.
    pub fn config_bound(&self) -> Option<&[ConfigBound]> {
        match self {
            Self::ConfigBound { bound } => Some(bound),
            Self::Literal | Self::ConfigUnresolved { .. } => None,
        }
    }
}

// ── The lookup seam ─────────────────────────────────────────────────────────

/// The committed-configuration lookup one resolution runs against.
///
/// A trait rather than a concrete store handle for the reason every other
/// federation seam is one: resolution is exercisable without standing up an
/// on-disk engine, and the measurement harness drives the same code the
/// production pipeline does instead of a copy of it.
///
/// The key passed in is **canonical** ([`canonical_key`]) — this is a lookup,
/// not a binder.
pub trait ConfigLookup {
    /// Every committed definition of one canonical key **within `module`**, in a
    /// stable order.
    ///
    /// `module` is [`Resolver::module`], passed through rather than baked into
    /// the implementation, so the reading module's scope lives in exactly one
    /// place and a resolver and its corpus cannot disagree about it.
    ///
    /// # What "the reading module" is differs by setting, and is stated rather
    /// than assumed
    ///
    /// [ADR-64]'s committed-evidence line requires the value to be *within reach
    /// of the reading module*, and what bounds that reach depends on what the
    /// implementation holds:
    ///
    /// - A **member-local** corpus (the federated coverage tier) is already one
    ///   deployable's own store, so the member **is** the scope and `module` is
    ///   `""` there — a narrower scope would need the build-module partition,
    ///   which a single member's store does not carry.
    /// - A **whole-corpus** view (the measurement harness, which walks a
    ///   multi-module repository) is not, so it filters on `module`: the
    ///   classpath one deployable assembles.
    ///
    /// An implementation that cannot read returns **empty**, which resolves as
    /// [`ValueRefusal::MissingKey`]: an unreadable corpus admits nothing rather
    /// than admitting a guess.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    fn definitions(&self, key: &str, module: &str) -> Vec<ConfigDefinition>;
}

/// Everything configuration resolution needs besides the operand itself: the
/// committed definitions and the module scope they are read in (S-365,
/// promoted).
///
/// # It carries no properties index, and that is the promotion doing its job
///
/// The harness's `Resolver` also held a `&PropertiesIndex`, because its
/// `resolve_key` walks a tree-sitter expression to find *which key* an operand
/// names. That half is language-shaped and stayed in the harness ([CR-121] §5.1),
/// so the shipped resolver never reads the field — and a promoted struct
/// carrying a field its own methods never touch is baggage rather than a
/// contract. It cost a throwaway `PropertiesIndex::default()` per resolved row
/// until it was removed. The harness keeps its index beside this struct.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
#[derive(Clone, Copy)]
pub struct Resolver<'a> {
    /// The committed configuration this resolution reads.
    pub corpus: &'a dyn ConfigLookup,
    /// The module root the use site sits in; `""` is the corpus root.
    pub module: &'a str,
}

impl Resolver<'_> {
    /// What the committed sources prove about `key`, which may be spelled in the
    /// source's own relaxed form — it is canonicalised here.
    pub fn agreement(&self, key: &str) -> Agreement {
        Agreement::of(&self.corpus.definitions(&canonical_key(key), self.module))
    }

    /// Admit `key`'s committed value(s), or name why not.
    ///
    /// `source` is consulted **first**: an environment read is refused whatever
    /// the committed sources say, because [`canonical_key`] would otherwise let
    /// `getenv("BASE_URL")` collide with a yaml `base-url` and be admitted as
    /// though the repository proved it ([ADR-64]).
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    pub fn resolve(&self, key: &str, source: KeySource) -> Result<ConfigBound, ValueRefusal> {
        if !source.is_committed() {
            return Err(ValueRefusal::Uncommitted);
        }
        let canonical = canonical_key(key);
        match self.agreement(&canonical) {
            Agreement::Agreed(value) => Ok(ConfigBound {
                key: canonical,
                source,
                values: vec![value],
            }),
            Agreement::Divergent(values) => Ok(ConfigBound { key: canonical, source, values }),
            Agreement::Placeholder { .. } => Err(ValueRefusal::PlaceholderValue),
            Agreement::Missing => Err(ValueRefusal::MissingKey),
        }
    }

    /// Resolve every `${…}` placeholder in `template` against the committed
    /// corpus, yielding one candidate per profile that proves a distinct
    /// composition ([ADR-64] decision point 3).
    ///
    /// Returns [`None`] when the template carries no placeholder at all — the
    /// caller then holds a literal, and nothing here applies to it.
    ///
    /// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
    pub fn resolve_template(
        &self,
        template: &str,
    ) -> Option<Result<ResolvedTemplate, ValueRefusal>> {
        let keys = placeholder_keys(template)?;
        Some(self.compose_template(template, &keys))
    }

    /// The body of [`resolve_template`](Self::resolve_template) for a template
    /// already known to carry `keys`.
    fn compose_template(
        &self,
        template: &str,
        keys: &[String],
    ) -> Result<ResolvedTemplate, ValueRefusal> {
        let mut bound = Vec::with_capacity(keys.len());
        for key in keys {
            bound.push(self.resolve(key, KeySource::Placeholder)?);
        }
        let candidates = profile_candidates(template, &bound);
        // **No profile composes the whole template.** Every key admits on its
        // own, but no single profile — and not the unprofiled base — proves a
        // value for *all* of them at once, so there is no committed composition
        // to admit. Reachable with two keys committed under disjoint profiles.
        //
        // Refused as [`ValueRefusal::MissingKey`] rather than returned as an
        // empty `Ok`: an `Ok` carrying no composition would travel to the
        // surfaces as `config-bound` provenance — an admitted value for a
        // template that was never composed, which is precisely the over-read
        // this module exists to refuse.
        if candidates.is_empty() {
            return Err(ValueRefusal::MissingKey);
        }
        Ok(ResolvedTemplate { candidates, bound })
    }
}

/// One committed composition of a template, and the profiles that produce it
/// ([ADR-64] decision point 3: "edges carry the profiles that produce them").
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfiledTemplate {
    /// The template with every placeholder replaced by the value that profile
    /// commits.
    pub template: String,
    /// The profiles proving this composition, sorted.
    pub profiles: Vec<String>,
    /// Whether the **unprofiled** sources alone prove it.
    pub unprofiled: bool,
}

/// What a placeholder-bearing template resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedTemplate {
    /// One entry per distinct committed composition — more than one is an
    /// overlay divergence, and **every one** reaches the consumer.
    ///
    /// Non-empty whenever a [`ResolvedTemplate`] exists, because
    /// [`Resolver::resolve_template`] refuses rather than returning an empty
    /// set: a key can admit on its own and still compose nothing, when no single
    /// profile proves every key of the template. That is the refusal, not an
    /// empty success — see [`ValueRefusal::MissingKey`].
    pub candidates: Vec<ProfiledTemplate>,
    /// The provenance of each placeholder, in the order the template names them.
    pub bound: Vec<ConfigBound>,
}

/// Compose one candidate per profile that proves a distinct substitution.
///
/// **Per profile, not per combination of profiles.** The profile is the dimension
/// [ADR-64] names, and enumerating it is linear in the corpus: a deployment runs
/// under one profile at a time, so profiles are never crossed with each other.
///
/// Within one profile the keys ARE crossed, because a key a multi-document file
/// defines twice proves both values and dropping one would be a silent choice.
/// That product is bounded by [`MAX_COMPOSITIONS`] — see [`substitutions`] — so
/// the "linear in the corpus" claim above holds of the profile axis only, which
/// is the axis the decision is about.
///
/// A key with no value under profile `P` falls back to its **unprofiled** value,
/// which is what an overlay means: the profiled file overrides the base file and
/// is silent about everything else. A key that `P` and the base file **both**
/// leave undefined composes nothing under `P` — [`Resolver::resolve`] proves each
/// key has *some* value, never that it has one under *this* profile — and
/// [`substitutions`] states that rule where it is enforced.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
fn profile_candidates(template: &str, bound: &[ConfigBound]) -> Vec<ProfiledTemplate> {
    let profiles: BTreeSet<&str> =
        bound.iter().flat_map(|b| b.values.iter()).flat_map(|v| v.profiles.iter().map(String::as_str)).collect();
    // Composition → the profiles proving it. A BTreeMap because two profiles
    // that agree on every key produce ONE candidate, not two identical ones.
    let mut by_composition: BTreeMap<String, (BTreeSet<&str>, bool)> = BTreeMap::new();
    for profile in profiles.iter().copied().map(Some).chain(std::iter::once(None)) {
        for composition in substitutions(template, bound, profile) {
            let entry = by_composition.entry(composition).or_default();
            match profile {
                Some(p) => {
                    entry.0.insert(p);
                }
                None => entry.1 = true,
            }
        }
    }
    by_composition
        .into_iter()
        .map(|(template, (profiles, unprofiled))| ProfiledTemplate {
            template,
            profiles: profiles.into_iter().map(str::to_string).collect(),
            unprofiled,
        })
        .collect()
}

/// Every substitution of `template` under `profile` — one per value that profile
/// proves for a key, so an in-profile duplicate (a multi-document file defining
/// one key twice) is represented rather than silently resolved to one of them.
///
/// `profile` of [`None`] is the unprofiled base.
///
/// # The product is bounded, and the bound refuses rather than truncates
///
/// Crossing the keys of one profile is a product, not a sum: eight placeholders
/// each proving four in-profile values is 65 536 compositions, which is a real
/// (if exotic) allocation cliff rather than a theoretical one. Past
/// [`MAX_COMPOSITIONS`] this profile composes **nothing**, so the site refuses
/// instead of admitting an arbitrary prefix of its own candidate set — a
/// truncated set would make "every value reaches the consumer" ([ADR-64]
/// decision point 3) false without saying so.
///
/// The reference estate's worst case is **one** placeholder proving one value,
/// so the cap is unreached there; it exists for the corpus that is not this one.
///
/// [ADR-64]: ../../../docs/specs/architecture/decisions/ADR-64.md
fn substitutions(
    template: &str,
    bound: &[ConfigBound],
    profile: Option<&str>,
) -> Vec<String> {
    let mut out = vec![template.to_string()];
    for entry in bound {
        let values = values_under(entry, profile);
        if values.is_empty() {
            // This profile proves nothing for this key and neither does the
            // base file, so it composes nothing at all under this profile.
            return Vec::new();
        }
        if out.len().saturating_mul(values.len()) > MAX_COMPOSITIONS {
            return Vec::new();
        }
        out = out
            .iter()
            .flat_map(|partial| {
                values.iter().map(move |value| replace_key(partial, &entry.key, value))
            })
            .collect();
    }
    out
}

/// The values `entry` proves under `profile`, falling back to the unprofiled
/// base when the profile itself is silent about the key.
fn values_under<'a>(entry: &'a ConfigBound, profile: Option<&str>) -> Vec<&'a str> {
    let profiled: Vec<&str> = match profile {
        Some(p) => entry
            .values
            .iter()
            .filter(|v| v.profiles.iter().any(|q| q == p))
            .map(|v| v.value.as_str())
            .collect(),
        None => Vec::new(),
    };
    if !profiled.is_empty() {
        return profiled;
    }
    entry.values.iter().filter(|v| v.unprofiled).map(|v| v.value.as_str()).collect()
}

/// Replace every `${key}` (with or without an inline default) naming `key` with
/// `value`, comparing keys **canonically** so the source's relaxed spelling
/// matches the canonical key the corpus stores.
fn replace_key(template: &str, key: &str, value: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((before, inner, after)) = next_placeholder(rest) {
        out.push_str(before);
        if canonical_key(placeholder_key(inner)) == key {
            out.push_str(value);
        } else {
            out.push_str(PLACEHOLDER_OPEN);
            out.push_str(inner);
            out.push('}');
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The canonical keys a template's `${…}` placeholders name, in source order and
/// deduplicated, or [`None`] when it carries no placeholder at all.
///
/// **A route parameter is not a placeholder.** `/users/{id}` carries no `$`, so
/// it is untouched and keeps its positional-normalization meaning — the one
/// near-miss that would otherwise turn every route template into a
/// configuration lookup.
///
/// An **unterminated** `${` yields no key for that occurrence and the text is
/// left verbatim: reading past it would fabricate a key out of the rest of the
/// path. If that is the template's only `${`, the answer is [`None`] and the
/// caller treats it as a literal — which the arm then refuses on its own terms,
/// since `${` is not an absolute path.
pub fn placeholder_keys(template: &str) -> Option<Vec<String>> {
    let mut keys: Vec<String> = Vec::new();
    let mut rest = template;
    while let Some((_, inner, after)) = next_placeholder(rest) {
        let key = canonical_key(placeholder_key(inner));
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
        rest = after;
    }
    (!keys.is_empty()).then_some(keys)
}

/// Split at the next complete `${…}`: the text before it, its inner text, and
/// the text after its closing brace. [`None`] when no complete placeholder
/// remains.
///
/// A `${` with no `}` after it terminates the scan — see [`placeholder_keys`].
fn next_placeholder(text: &str) -> Option<(&str, &str, &str)> {
    let open = text.find(PLACEHOLDER_OPEN)?;
    let body = &text[open + PLACEHOLDER_OPEN.len()..];
    let close = body.find('}')?;
    let inner = &body[..close];
    // A NESTED opener inside the braces means the first `}` closed the inner
    // placeholder, not this one, so `inner` is a fragment rather than a key:
    // `${a${b}}` would otherwise yield the key `a${b`, which is a key name
    // assembled by this scanner rather than written by the source. It refuses no
    // *value* (no such key is committed, so it was always going to be missing) —
    // but the fabricated string reached `ConfigUnresolved { keys }` and any key
    // census verbatim, and a census naming a key nobody wrote is the same
    // over-read in a smaller place. Treated exactly like the unterminated case.
    if inner.contains(PLACEHOLDER_OPEN) {
        return None;
    }
    Some((&text[..open], inner, &body[close + 1..]))
}

/// A placeholder's key: everything before its inline default separator.
fn placeholder_key(inner: &str) -> &str {
    inner.split(PLACEHOLDER_DEFAULT).next().unwrap_or(inner).trim()
}

#[cfg(test)]
mod tests;
