//! **S-471's blocking measurement gate** — would a cross-member type overlay
//! ([CR-152]) admit enough of the reference estate's Java/Kotlin imports, bind
//! them precisely enough, and explain enough of the build relation, to be worth
//! building?
//!
//! Three halves, declared in [`cross_member_type_refs_floor.txt`] **before this
//! module existed** and parsed out of it by
//! [`the_bounds_are_the_ones_declared_before_the_run`](tests::the_bounds_are_the_ones_declared_before_the_run).
//! The constants are [`REACH_FLOOR`], [`EXPLANATION_FLOOR`] and
//! [`PRECISION_CEILING`]; no figure is restated in prose here, because a number
//! in a module header is guarded by nothing.
//!
//!   - **Reach** — distinct main-tree `(consumer, provider, type)` triples bound
//!     exactly-one between a build pair or a collision-backed pair.
//!   - **Explanation** — the share of non-platform `dependency` build pairs at
//!     least one Reach triple explains.
//!   - **Precision ceiling** — the share of cross-member import rows whose type
//!     has more than one owner. A ceiling: it fails *above* its bound.
//!
//! All three must hold for [S-472]..[S-474] to be planned. Below a floor, or
//! above the ceiling, [CR-152] is recorded FALSIFIED in
//! [`cross_member_type_refs_finding.txt`] and [ADR-70] is superseded in place.
//!
//! # What is the product's and what is the harness's
//!
//! The rows are the product's: each member's `unresolved_refs`, read through the
//! shipped [`GraphStore`] over [`SqliteGraphStore::open_readonly`]. The build
//! relation is the product's: the shipped [`join`] over the same stores'
//! build-manifest facts and the manifest's declared kinds — never a pom re-parse.
//! Whether a member *references* a colliding coordinate is the product's filing
//! too ([`collision_references`] asks the join, one member at a time). The file
//! roster is the shipped [`ConfigCorpus`] walk, and "members committing
//! Java/Kotlin source" is S-411's [`members_with_source`], called rather than
//! copied.
//!
//! What the product does **not** have yet — a type's fully-qualified name — the
//! harness derives, and only the harness ([CR-152] §7: the gate may derive names
//! independently; nothing in `logos-core/src` gains a second rule). A source
//! type's name is its path under `src/main/{java,kotlin}/`, refused when the
//! file's declared `package` disagrees ([`source_path_name`],
//! [`declared_package`]); an Avro type's name is its schema's namespace + name
//! ([`avro_types`]).
//!
//! # The census's rule, run beside the product's
//!
//! [CR-152] §2.1's census is an upper bound from a permissive owner rule. The
//! gate runs that rule too ([`Rule::Census`]: the package check off) over the
//! same rows, so every difference between the census and the verdict is printed
//! by name with the mechanism that produced it ([`reconcile`]).
//!
//! # A private copy only — and the gate refuses anything else
//!
//! [`open_member_store`] is the one way this module opens a member database,
//! and it is the shipped read-only open, which refuses a store at any schema
//! version other than this binary's rather than migrating it. The harness
//! never calls `SqliteGraphStore::open` on a member store and never starts an
//! `Engine` over a member (the fixtures build their own throwaway stores). A
//! refused store fails the run naming its member.
//!
//! [ADR-70]: ../../../docs/specs/architecture/decisions/ADR-70.md
//! [CR-152]: ../../../docs/requests/CR-152-cross-member-type-references-overlay.md
//! [S-472]: ../../../docs/planning/journal.md#s-472-members-record-the-types-they-declare-from-source-and-from-avro-schemas
//! [S-474]: ../../../docs/planning/journal.md#s-474-cross-member-type-references-on-the-cli-mcp-and-the-xservice-queries
//! [`cross_member_type_refs_floor.txt`]: ./cross_member_type_refs_floor.txt
//! [`cross_member_type_refs_finding.txt`]: ./cross_member_type_refs_finding.txt
//! [`members_with_source`]: super::config_declared_coupling::members_with_source

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::federation::build_deps::{join, MemberBuildFacts};
use logos_core::federation::{
    discover, BuildDependencyRelation, BuildEdgeKind, Member, MemberKind,
};
use logos_core::graph_store::{BuildManifestRow, GraphStore, SqliteGraphStore};
use logos_core::model::EdgeKind;

use super::config_declared_coupling::members_with_source;
use super::configuration_agreement::ConfigCorpus;

/// **The bounds, as declared before the run.** Embedded, following
/// [`super::vendored_spec_contracts::DECLARED_FLOOR`]: a file the build embeds
/// cannot be deleted or renamed without breaking compilation.
pub const DECLARED_FLOOR: &str = include_str!("cross_member_type_refs_floor.txt");

/// The recorded verdict, reproduced by the estate run and asserted against it.
pub const RECORDED_FINDING: &str = include_str!("cross_member_type_refs_finding.txt");

/// The three half names, verbatim from CR-152 §3.2 A1's first column — the
/// strings the bound lines, the report and the finding all carry.
pub const HALF_REACH: &str = "Reach";
pub const HALF_EXPLANATION: &str = "Explanation";
pub const HALF_PRECISION: &str = "Precision ceiling";

/// Reach's floor, parsed out of [`DECLARED_FLOOR`] by the always-run test.
pub const REACH_FLOOR: Bound = Bound { op: Op::AtLeast, value: 300, percent: false };
/// Explanation's floor, a share of the measured denominator.
pub const EXPLANATION_FLOOR: Bound = Bound { op: Op::AtLeast, value: 75, percent: true };
/// The precision ceiling — the half fails above it.
pub const PRECISION_CEILING: Bound = Bound { op: Op::AtMost, value: 10, percent: true };

/// The four non-build pairs CR-152 §2.1 names, reconciled by name.
pub const CENSUS_NON_BUILD_PAIRS: [(&str, &str); 4] = [
    ("mailbox-api", "mailbox-domain"),
    ("mailbox-manager", "mailbox-domain"),
    ("mailbox-unread-mail-batch", "mailbox-domain"),
    ("official-log-legal-storage-batch", "postel-cpx-creation-lib"),
];

/// CR-152 §2.1's census figures — reconciled against, never asserted as floors.
pub const CENSUS_MAIN_TRIPLES: usize = 437;
pub const CENSUS_PAIRS: usize = 81;

// ── Recorded figures ───────────────────────────────────────────────────────
//
// Pinned so a drift in the rows, the owner rule, the build relation or the
// classification is a failure rather than a quietly different number — a `>=`
// cannot fail upward (S-411's lesson).

/// Import rows read (`imports`, unresolved, `.java`/`.kt`) across the members.
pub const RECORDED_ROWS: usize = 24_732;
/// Reach triples measured.
pub const RECORDED_REACH: usize = 434;
/// `(source, avro, both)` backing of the Reach triples.
pub const RECORDED_REACH_BACKING: (usize, usize, usize) = (294, 140, 0);
/// Main-tree exactly-one triples by pair: `(build, build into a platform,
/// collision-backed, type-only)`.
pub const RECORDED_MAIN_TRIPLE_PAIRS: (usize, usize, usize, usize) = (379, 31, 24, 3);
/// `(explained, denominator)` of the Explanation half.
pub const RECORDED_EXPLANATION: (usize, usize) = (52, 52);
/// `(ambiguous, cross-member)` rows of the precision ceiling.
pub const RECORDED_PRECISION: (usize, usize) = (131, 2_492);
/// Source files refused for a package/directory disagreement.
pub const RECORDED_PACKAGE_REFUSALS: usize = 0;
/// Main-tree exactly-one triples under the census's own rule.
pub const RECORDED_CENSUS_RULE_MAIN_TRIPLES: usize = 437;

// ── The declaration ────────────────────────────────────────────────────────

/// Which side of its bound a half must sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    AtLeast,
    AtMost,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => ">=",
            Self::AtMost => "<=",
        }
    }
}

/// One declared bound: `>= 300`, `>= 75 %`, `<= 10 %`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    pub op: Op,
    pub value: usize,
    /// A share of a denominator rather than a count.
    pub percent: bool,
}

impl Bound {
    /// Whether `measured` (over `denominator`, for a share) sits on the right
    /// side. Decided in integers, never rounded; a share over a zero
    /// denominator never holds — nothing measured is not a pass.
    pub fn holds(self, measured: usize, denominator: usize) -> bool {
        if !self.percent {
            return match self.op {
                Op::AtLeast => measured >= self.value,
                Op::AtMost => measured <= self.value,
            };
        }
        if denominator == 0 {
            return false;
        }
        match self.op {
            Op::AtLeast => 100 * measured >= self.value * denominator,
            Op::AtMost => 100 * measured <= self.value * denominator,
        }
    }

    fn render(self) -> String {
        let unit = if self.percent { " %" } else { "" };
        format!("{} {}{unit}", self.op.symbol(), self.value)
    }
}

/// The bound the declaration states for `half`, on exactly ONE line of the form
/// `>= NN <half>…`, `>= NN % <half>…` or `<= NN % <half>…` — S-411's rule: a
/// declaration that states its own floor twice has none.
pub fn declared_bound(text: &str, half: &str) -> Result<Bound, String> {
    let hits: Vec<Bound> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (op, rest) = match (line.strip_prefix(">= "), line.strip_prefix("<= ")) {
                (Some(rest), _) => (Op::AtLeast, rest),
                (_, Some(rest)) => (Op::AtMost, rest),
                _ => return None,
            };
            let (n, tail) = rest.split_once(' ')?;
            let value = n.parse().ok()?;
            let (percent, tail) = match tail.strip_prefix("% ") {
                Some(tail) => (true, tail),
                None => (false, tail),
            };
            let named = tail.strip_prefix(half).is_some_and(|r| r.is_empty() || r.starts_with(' '));
            named.then_some(Bound { op, value, percent })
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        _ => Err(format!(
            "the declaration must state `>=|<= NN [%] {half}` on exactly one line; found {hits:?}"
        )),
    }
}

/// The one verdict line — rendered identically in the report and the finding,
/// so the finding is checked against the run by string equality.
pub fn verdict_line(half: &str, bound: Bound, measured: usize, denominator: usize) -> String {
    let verdict = if bound.holds(measured, denominator) { "HOLDS" } else { "FALSIFIED" };
    let (what, figure) = match (bound.percent, bound.op) {
        (false, _) => ("floor", format!("{measured}")),
        (true, op) => (
            if op == Op::AtMost { "ceiling" } else { "floor" },
            format!("{measured} of {denominator} ({})", share(measured, denominator)),
        ),
    };
    format!("{half}: measured {figure}, {what} {} — {verdict}", bound.render())
}

/// A display share with one decimal — display only; [`Bound::holds`] decides.
pub fn share(n: usize, of: usize) -> String {
    if of == 0 {
        return "n/a".into();
    }
    format!("{:.1} %", 100.0 * n as f64 / of as f64)
}

// ── Consumer trees and declared names ──────────────────────────────────────

/// Where an importing file sits in its member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tree {
    Main,
    Test,
    Other,
}

impl Tree {
    pub fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Test => "test",
            Self::Other => "other",
        }
    }
}

/// The byte offset just past `segment` when `path` starts with it or holds it
/// after a `/` — a path-segment match, never a substring one (`xsrc/main/` is
/// not `src/main/`).
fn after_segment(path: &str, segment: &str) -> Option<usize> {
    if path.starts_with(segment) {
        return Some(segment.len());
    }
    path.find(&format!("/{segment}")).map(|i| i + 1 + segment.len())
}

/// A member-relative path's consumer tree.
pub fn consumer_tree(path: &str) -> Tree {
    if after_segment(path, "src/main/").is_some() {
        Tree::Main
    } else if after_segment(path, "src/test/").is_some() {
        Tree::Test
    } else {
        Tree::Other
    }
}

/// The two main-tree source roots a declaring file must sit under.
pub const SOURCE_ROOTS: [&str; 2] = ["src/main/java/", "src/main/kotlin/"];

/// `(package, simple name)` a main-tree `.java`/`.kt` file declares by its path,
/// or `None` when the file is not a main-tree Java/Kotlin source.
pub fn source_path_name(path: &str) -> Option<(String, String)> {
    let stem = path.strip_suffix(".java").or_else(|| path.strip_suffix(".kt"))?;
    let start = SOURCE_ROOTS.iter().filter_map(|root| after_segment(stem, root)).min()?;
    let rest = &stem[start..];
    let (dirs, name) = rest.rsplit_once('/').unwrap_or(("", rest));
    if name.is_empty() {
        return None;
    }
    Some((dirs.replace('/', "."), name.to_string()))
}

/// `text` with every comment removed and every string or character literal's
/// contents dropped (its quotes kept), newlines preserved — scanned left to
/// right, so a `/*` inside a `//` comment or a string opens nothing (a
/// `//*****` licence banner is one line comment, not a block).
fn code_only(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = '\0';
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && d == '/' {
                        break;
                    }
                    prev = d;
                }
                out.push(' ');
            }
            '"' | '\'' => {
                out.push(c);
                let mut escaped = false;
                for d in chars.by_ref() {
                    if d == '\n' {
                        out.push('\n');
                        break;
                    }
                    if escaped {
                        escaped = false;
                    } else if d == '\\' {
                        escaped = true;
                    } else if d == c {
                        out.push(c);
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The package a Java/Kotlin file declares — its first code line, when that is
/// a `package` statement — or `None` for the default package.
///
/// Comments are skipped ([`code_only`]) and so are annotation lines (Kotlin's
/// `@file:JvmName`, Java's `package-info` annotations), which may precede the
/// statement. Kotlin's backtick escapes are dropped.
pub fn declared_package(text: &str) -> Option<String> {
    for line in code_only(text).lines() {
        let code = line.trim();
        if code.is_empty() || code.starts_with('@') {
            continue;
        }
        let rest = code.strip_prefix("package")?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let name = rest.split(';').next().unwrap_or("").trim().replace('`', "");
        return (!name.is_empty()).then_some(name);
    }
    None
}

/// Every record/enum full name an `.avsc` document declares, or the parse
/// error that refuses it.
///
/// The Avro naming rule: a `name` containing a `.` is already full; otherwise
/// it takes the object's own `namespace`, else the nearest enclosing named
/// type's. An empty `namespace` is the null namespace — it inherits nothing,
/// per the Avro specification. A top-level array (a union) is read element by element, and named
/// types nested in `fields`, `type`, `items` and `values` are read too.
pub fn avro_types(text: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    walk_avro(&value, None, &mut out);
    Ok(out)
}

fn walk_avro(value: &serde_json::Value, namespace: Option<&str>, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                walk_avro(item, namespace, out);
            }
        }
        serde_json::Value::Object(map) => {
            let own = match map.get("namespace").and_then(serde_json::Value::as_str) {
                Some("") => None,
                Some(ns) => Some(ns),
                None => namespace,
            };
            let kind = map.get("type").and_then(serde_json::Value::as_str);
            let name = map.get("name").and_then(serde_json::Value::as_str);
            let mut enclosing = own.map(str::to_string);
            if let (Some("record" | "enum"), Some(name)) = (kind, name) {
                let full = match own {
                    _ if name.contains('.') => name.to_string(),
                    Some(ns) => format!("{ns}.{name}"),
                    None => name.to_string(),
                };
                enclosing = full.rsplit_once('.').map(|(ns, _)| ns.to_string());
                out.push(full);
            }
            for key in ["fields", "type", "items", "values"] {
                if let Some(child) = map.get(key) {
                    walk_avro(child, enclosing.as_deref(), out);
                }
            }
        }
        _ => {}
    }
}

// ── Declarations and the owner index ───────────────────────────────────────

/// One main-tree Java/Kotlin source file, as its path and its text name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    pub member: String,
    pub path: String,
    pub path_package: String,
    pub name: String,
    pub declared_package: Option<String>,
}

impl SourceFile {
    /// The path-derived fully-qualified name.
    pub fn path_fqn(&self) -> String {
        if self.path_package.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.path_package, self.name)
        }
    }

    /// The declared package equals the directory's (no statement agreeing only
    /// with the default package).
    pub fn agrees(&self) -> bool {
        self.declared_package.as_deref().unwrap_or("") == self.path_package
    }
}

/// One `.avsc` file and what it declares, or why it was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvroFile {
    pub member: String,
    pub path: String,
    pub types: Result<Vec<String>, String>,
}

/// Every declaring file in the workspace.
#[derive(Debug, Clone, Default)]
pub struct Declarations {
    pub sources: Vec<SourceFile>,
    pub avro: Vec<AvroFile>,
}

impl Declarations {
    /// Source files refused for a package/directory disagreement.
    pub fn package_refusals(&self) -> Vec<&SourceFile> {
        self.sources.iter().filter(|s| !s.agrees()).collect()
    }

    /// `.avsc` files that did not parse.
    pub fn avro_refusals(&self) -> Vec<&AvroFile> {
        self.avro.iter().filter(|a| a.types.is_err()).collect()
    }
}

/// The owner rule in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The declared floor's rule: a package/directory disagreement is refused.
    Product,
    /// CR-152 §2.1's census rule: the path alone decides.
    Census,
}

/// How a provider declares a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Backing {
    Source,
    Avro,
    Both,
}

impl Backing {
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Avro => "avro",
            Self::Both => "both",
        }
    }

    fn with(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            Self::Both
        }
    }
}

/// Owners of a fully-qualified name, by member.
pub type Owners = BTreeMap<String, Backing>;

/// Fully-qualified type name → its owners.
#[derive(Debug, Clone, Default)]
pub struct OwnerIndex(pub BTreeMap<String, Owners>);

impl OwnerIndex {
    pub fn build(decl: &Declarations, rule: Rule) -> Self {
        let mut index: BTreeMap<String, Owners> = BTreeMap::new();
        let mut add = |fqn: String, member: &str, backing: Backing| {
            let owners = index.entry(fqn).or_default();
            let merged = owners.get(member).map_or(backing, |b| b.with(backing));
            owners.insert(member.to_string(), merged);
        };
        for s in &decl.sources {
            if rule == Rule::Product && !s.agrees() {
                continue;
            }
            add(s.path_fqn(), &s.member, Backing::Source);
        }
        for a in &decl.avro {
            for t in a.types.iter().flatten() {
                add(t.clone(), &a.member, Backing::Avro);
            }
        }
        Self(index)
    }

    pub fn owners(&self, fqn: &str) -> Option<&Owners> {
        self.0.get(fqn).filter(|o| !o.is_empty())
    }
}

/// How a row's target reached its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Naming {
    Exact,
    /// The enclosing name: a static-member import or a nested type.
    Enclosing,
}

/// The type a row's target names — exactly, else by its enclosing name one
/// level up — with its owners, or `None` when neither has an owner.
pub fn resolve<'a>(target: &str, index: &'a OwnerIndex) -> Option<(String, Naming, &'a Owners)> {
    if let Some(owners) = index.owners(target) {
        return Some((target.to_string(), Naming::Exact, owners));
    }
    let (enclosing, _) = target.rsplit_once('.')?;
    index.owners(enclosing).map(|owners| (enclosing.to_string(), Naming::Enclosing, owners))
}

/// A row's class, decided in the declaration's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowClass {
    SelfOwned,
    ExactlyOne { provider: String, backing: Backing },
    Ambiguous { owners: Vec<String> },
    NoOwner,
}

pub fn classify(consumer: &str, owners: Option<&Owners>) -> RowClass {
    let Some(owners) = owners else { return RowClass::NoOwner };
    if owners.contains_key(consumer) {
        return RowClass::SelfOwned;
    }
    match owners.iter().collect::<Vec<_>>().as_slice() {
        [] => RowClass::NoOwner,
        [(provider, backing)] => {
            RowClass::ExactlyOne { provider: (*provider).clone(), backing: **backing }
        }
        many => RowClass::Ambiguous { owners: many.iter().map(|(m, _)| (*m).clone()).collect() },
    }
}

// ── The build relation, as the pair restriction ────────────────────────────

/// How the build relation relates an exactly-one row's two members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairClass {
    /// A `builds-against(A → B)` edge of any kind; `platform` when B is a
    /// declared platform (the edges counted apart in the build headline).
    Build {
        platform: bool,
    },
    CollisionBacked,
    TypeOnly,
}

impl PairClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Build { platform: false } => "build",
            Self::Build { platform: true } => "build (platform)",
            Self::CollisionBacked => "collision-backed",
            Self::TypeOnly => "type-only",
        }
    }

    /// Whether a triple of this pair counts toward Reach.
    pub fn admitted(self) -> bool {
        !matches!(self, Self::TypeOnly)
    }
}

/// The pair evidence the three halves read off the build relation.
#[derive(Debug, Clone, Default)]
pub struct BuildPairs {
    /// Directed build pairs → whether their edges are into a platform.
    pub build: BTreeMap<(String, String), bool>,
    /// `(A, B)` where A references a colliding coordinate B produces.
    pub collision: BTreeSet<(String, String)>,
    /// Explanation's denominator: pairs with a non-platform `dependency` edge.
    pub dependency: BTreeSet<(String, String)>,
}

impl BuildPairs {
    /// Read the relation's edges and collisions. `referenced` is each member's
    /// set of colliding coordinates it references ([`collision_references`]).
    pub fn from_relation(
        relation: &BuildDependencyRelation,
        referenced: &BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        let mut out = Self::default();
        for e in &relation.edges {
            let key = (e.from.clone(), e.to.clone());
            if e.kind == BuildEdgeKind::Dependency && !e.platform {
                out.dependency.insert(key.clone());
            }
            *out.build.entry(key).or_default() |= e.platform;
        }
        for (member, artifacts) in referenced {
            for c in relation.headline.collisions.iter().filter(|c| artifacts.contains(&c.artifact))
            {
                for producer in c.producers.iter().filter(|p| *p != member) {
                    out.collision.insert((member.clone(), producer.clone()));
                }
            }
        }
        out
    }

    pub fn class(&self, a: &str, b: &str) -> PairClass {
        let key = (a.to_string(), b.to_string());
        match self.build.get(&key) {
            Some(&platform) => PairClass::Build { platform },
            None if self.collision.contains(&key) => PairClass::CollisionBacked,
            None => PairClass::TypeOnly,
        }
    }
}

/// Each member's colliding coordinates it references, decided by the product's
/// own filing: the shipped [`join`] run with only that member's references kept
/// (every member's produced facts stay, so the collisions are the same) reports
/// a collision with a non-zero reference count exactly when that member's
/// references reach it — refused keys, project references and build plugins
/// already filed elsewhere, as the join files them.
pub fn collision_references(
    roster: &[Member],
    kinds: &BTreeMap<String, MemberKind>,
    facts: &[MemberBuildFacts],
) -> BTreeMap<String, BTreeSet<String>> {
    let produced_only = |rows: &[BuildManifestRow]| -> Vec<BuildManifestRow> {
        rows.iter()
            .map(|r| BuildManifestRow {
                artifacts: r.artifacts.iter().filter(|a| a.role == "produced").cloned().collect(),
                ..r.clone()
            })
            .collect()
    };
    let mut out = BTreeMap::new();
    for (member, _) in facts {
        let isolated: Vec<MemberBuildFacts> = facts
            .iter()
            .map(|(m, rows)| {
                (m.clone(), if m == member { rows.clone() } else { produced_only(rows) })
            })
            .collect();
        let relation = join(roster, kinds, &isolated, &[]);
        let hit: BTreeSet<String> = relation
            .headline
            .collisions
            .iter()
            .filter(|c| c.references > 0)
            .map(|c| c.artifact.clone())
            .collect();
        if !hit.is_empty() {
            out.insert(member.clone(), hit);
        }
    }
    out
}

// ── The judgement ──────────────────────────────────────────────────────────

/// One import row as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub consumer: String,
    pub path: String,
    pub line: Option<i64>,
    /// The target with `::` read as `.`.
    pub target: String,
}

/// One row, judged.
#[derive(Debug, Clone)]
pub struct Judged {
    pub tree: Tree,
    /// The type named and how, when it has an owner.
    pub named: Option<(String, Naming)>,
    pub class: RowClass,
    /// The pair class, for an exactly-one row.
    pub pair: Option<PairClass>,
}

/// `(consumer, provider, type)`.
pub type Triple = (String, String, String);

/// Every row judged under one owner rule.
#[derive(Debug, Clone, Default)]
pub struct Judgement {
    pub rows: Vec<(Row, Judged)>,
    /// Explanation's denominator, carried from the pair evidence.
    pub dependency: BTreeSet<(String, String)>,
}

impl Judgement {
    /// Exactly-one rows as `(triple, tree, pair class, backing, naming)`.
    pub fn exactly_one(
        &self,
    ) -> impl Iterator<Item = (Triple, Tree, PairClass, Backing, Naming)> + '_ {
        self.rows.iter().filter_map(|(row, j)| match (&j.class, j.pair, &j.named) {
            (RowClass::ExactlyOne { provider, backing }, Some(pair), Some((t, naming))) => Some((
                (row.consumer.clone(), provider.clone(), t.clone()),
                j.tree,
                pair,
                *backing,
                *naming,
            )),
            _ => None,
        })
    }

    /// Distinct main-tree exactly-one triples with their pair class — every
    /// pair class, the census's grain.
    pub fn main_triples(&self) -> BTreeMap<Triple, PairClass> {
        self.exactly_one()
            .filter(|(_, tree, ..)| *tree == Tree::Main)
            .map(|(t, _, p, ..)| (t, p))
            .collect()
    }

    /// **Reach**: distinct main-tree exactly-one triples of an admitted pair.
    pub fn reach(&self) -> BTreeSet<Triple> {
        self.main_triples().into_iter().filter(|(_, p)| p.admitted()).map(|(t, _)| t).collect()
    }

    /// Reach triples by their provider's backing: `(source, avro, both)`.
    pub fn reach_backing(&self) -> (usize, usize, usize) {
        let backing: BTreeMap<Triple, Backing> =
            self.exactly_one().map(|(t, _, _, b, _)| (t, b)).collect();
        let reach = self.reach();
        let n = |want| reach.iter().filter(|t| backing[*t] == want).count();
        (n(Backing::Source), n(Backing::Avro), n(Backing::Both))
    }

    /// Main-tree exactly-one triples by pair: `(build, build into a platform,
    /// collision-backed, type-only)`.
    pub fn main_triple_pairs(&self) -> (usize, usize, usize, usize) {
        let main = self.main_triples();
        let n = |want| main.values().filter(|p| **p == want).count();
        (
            n(PairClass::Build { platform: false }),
            n(PairClass::Build { platform: true }),
            n(PairClass::CollisionBacked),
            n(PairClass::TypeOnly),
        )
    }

    /// **Explanation**: `(explained, denominator)`, and the unexplained pairs.
    pub fn explanation(&self) -> (usize, usize, Vec<(String, String)>) {
        let explaining: BTreeSet<(String, String)> =
            self.reach().into_iter().map(|(a, b, _)| (a, b)).collect();
        let unexplained: Vec<_> =
            self.dependency.iter().filter(|p| !explaining.contains(*p)).cloned().collect();
        (self.dependency.len() - unexplained.len(), self.dependency.len(), unexplained)
    }

    /// **Precision ceiling**: `(ambiguous rows, cross-member rows)`, every tree.
    pub fn precision(&self) -> (usize, usize) {
        let mut ambiguous = 0;
        let mut cross = 0;
        for (_, j) in &self.rows {
            match j.class {
                RowClass::ExactlyOne { .. } => cross += 1,
                RowClass::Ambiguous { .. } => {
                    ambiguous += 1;
                    cross += 1;
                }
                _ => {}
            }
        }
        (ambiguous, cross)
    }

    /// Rows by `(class label, tree)`.
    pub fn row_classes(&self) -> BTreeMap<(&'static str, Tree), usize> {
        let mut out = BTreeMap::new();
        for (_, j) in &self.rows {
            let label = match j.class {
                RowClass::SelfOwned => "self-owned",
                RowClass::ExactlyOne { .. } => "exactly-one",
                RowClass::Ambiguous { .. } => "ambiguous",
                RowClass::NoOwner => "no owner",
            };
            *out.entry((label, j.tree)).or_default() += 1;
        }
        out
    }
}

/// Judge every row under `index`, reading the pair restriction off `pairs`.
pub fn judge(rows: &[Row], index: &OwnerIndex, pairs: &BuildPairs) -> Judgement {
    let judged = rows
        .iter()
        .map(|row| {
            let resolved = resolve(&row.target, index);
            let class = classify(&row.consumer, resolved.as_ref().map(|(_, _, o)| *o));
            let pair = match &class {
                RowClass::ExactlyOne { provider, .. } => Some(pairs.class(&row.consumer, provider)),
                _ => None,
            };
            let judged = Judged {
                tree: consumer_tree(&row.path),
                named: resolved.map(|(t, n, _)| (t, n)),
                class,
                pair,
            };
            (row.clone(), judged)
        })
        .collect();
    Judgement { rows: judged, dependency: pairs.dependency.clone() }
}

/// Why a census-rule main-tree triple is not a Reach triple, or why a Reach
/// triple is not a census-rule one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mechanism {
    /// The product rule gives the same exactly-one triple, of a type-only pair.
    TypeOnlyPair,
    /// A package/directory refusal changed the type's owners — the refused
    /// files named.
    PackageRefusal(Vec<String>),
    /// No mechanism this gate names; printed, never absorbed.
    Unattributed,
}

/// The census-vs-product differences, by name.
#[derive(Debug, Clone, Default)]
pub struct Reconciliation {
    /// Census-rule main-tree triples not in Reach, with their mechanism.
    pub missing: Vec<(Triple, Mechanism)>,
    /// Reach triples the census rule does not count, with their mechanism.
    pub extra: Vec<(Triple, Mechanism)>,
}

/// Reconcile the census rule's main-tree triples against Reach.
pub fn reconcile(product: &Judgement, census: &Judgement, decl: &Declarations) -> Reconciliation {
    let mut refused_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in decl.package_refusals() {
        refused_by_name.entry(s.path_fqn()).or_default().push(format!("{}/{}", s.member, s.path));
    }
    let product_main = product.main_triples();
    let reach = product.reach();
    let census_main = census.main_triples();
    let mechanism = |(_, _, t): &Triple| match refused_by_name.get(t) {
        Some(files) => Mechanism::PackageRefusal(files.clone()),
        None => Mechanism::Unattributed,
    };
    let mut out = Reconciliation::default();
    for triple in census_main.keys().filter(|t| !reach.contains(*t)) {
        let why = match product_main.get(triple) {
            Some(PairClass::TypeOnly) => Mechanism::TypeOnlyPair,
            _ => mechanism(triple),
        };
        out.missing.push((triple.clone(), why));
    }
    for triple in reach.iter().filter(|t| !census_main.contains_key(*t)) {
        out.extra.push((triple.clone(), mechanism(triple)));
    }
    out
}

// ── The estate read ────────────────────────────────────────────────────────

/// Open one member store the only way this module does: the shipped read-only
/// open, which refuses — never migrates — a store at any schema version other
/// than this binary's.
pub fn open_member_store(db: &Path) -> Result<SqliteGraphStore, String> {
    SqliteGraphStore::open_readonly(db).map_err(|e| format!("{e:#}"))
}

/// Whether a ledger path is a Java or Kotlin file.
pub fn is_jvm_source(path: &str) -> bool {
    path.ends_with(".java") || path.ends_with(".kt")
}

/// One member's import rows, read off an open store.
pub fn import_rows(member: &str, store: &SqliteGraphStore) -> Result<Vec<Row>, String> {
    let paths: BTreeMap<i64, String> = store
        .indexed_files()
        .map_err(|e| format!("{member}: files: {e:#}"))?
        .into_iter()
        .map(|f| (f.id, f.path))
        .collect();
    let refs = store.unresolved_refs().map_err(|e| format!("{member}: ledger: {e:#}"))?;
    Ok(refs
        .into_iter()
        .filter(|r| r.kind == EdgeKind::Imports && !r.resolved)
        .filter_map(|r| {
            let path = paths.get(&r.file_id?)?;
            is_jvm_source(path).then(|| Row {
                consumer: member.to_string(),
                path: path.clone(),
                line: r.line,
                target: r.target.replace("::", "."),
            })
        })
        .collect())
}

/// Every declaring file in the corpus roster, member-attributed.
pub fn declarations(
    root: &Path,
    members: &BTreeSet<String>,
    config: &ConfigCorpus,
) -> Declarations {
    let mut out = Declarations::default();
    for rel in config.files() {
        let Some((member, path)) = rel.split_once('/') else { continue };
        if !members.contains(member) {
            continue;
        }
        let read = || std::fs::read_to_string(root.join(rel));
        if let Some((path_package, name)) = source_path_name(path) {
            let declared_package = read().ok().as_deref().and_then(declared_package);
            out.sources.push(SourceFile {
                member: member.into(),
                path: path.into(),
                path_package,
                name,
                declared_package,
            });
        } else if path.ends_with(".avsc") {
            let types = read().map_err(|e| e.to_string()).and_then(|t| avro_types(&t));
            out.avro.push(AvroFile { member: member.into(), path: path.into(), types });
        }
    }
    out
}

/// What the estate read produced.
#[derive(Debug)]
pub struct Estate {
    pub members: Vec<String>,
    pub stores_read: usize,
    pub rows: Vec<Row>,
    pub decl: Declarations,
    pub relation: BuildDependencyRelation,
    pub pairs: BuildPairs,
    /// S-411's `members_with_source`, narrowed to the Java and Kotlin plugins.
    pub jvm_members: BTreeSet<String>,
}

/// Read the estate: every member store read-only, the file roster, the build
/// relation. Panics — the run fails — naming a member whose store is absent or
/// refused, or whose build facts the join reports unread.
pub fn read_estate(root: &Path) -> Estate {
    let federation = discover(root).expect("the workspace manifest parses").unwrap_or_else(|| {
        panic!("{} is not a Logos workspace — this gate never enrols one", root.display())
    });
    let names: BTreeSet<String> = federation.members.iter().map(|m| m.name.clone()).collect();

    let mut rows = Vec::new();
    let mut facts: Vec<MemberBuildFacts> = Vec::new();
    let mut not_extracted = Vec::new();
    let mut refused = Vec::new();
    for member in &federation.members {
        let db = member.root.join(".logos").join("logos.db");
        if !db.is_file() {
            refused.push(format!("{}: no store at {}", member.name, db.display()));
            continue;
        }
        let store = match open_member_store(&db) {
            Ok(store) => store,
            Err(e) => {
                refused.push(format!("{}: {e}", member.name));
                continue;
            }
        };
        rows.extend(import_rows(&member.name, &store).unwrap_or_else(|e| panic!("{e}")));
        let extracted = store.build_facts_extracted().unwrap_or(false);
        if extracted {
            let manifests = store
                .build_manifests()
                .unwrap_or_else(|e| panic!("{}: build manifests: {e:#}", member.name));
            facts.push((member.name.clone(), manifests));
        } else {
            not_extracted.push(member.name.clone());
        }
    }
    assert!(
        refused.is_empty(),
        "{} member store(s) were refused or absent — the run never migrates a store and never \
         measures a partial workspace. Run on a private copy whose stores this binary's schema \
         version matches:\n  {}",
        refused.len(),
        refused.join("\n  ")
    );

    let relation = join(&federation.members, &federation.member_kinds, &facts, &not_extracted);
    assert!(
        relation.headline.members.unread.is_empty(),
        "the build relation left members unread: {:?}",
        relation.headline.members.unread_reasons
    );
    let referenced = collision_references(&federation.members, &federation.member_kinds, &facts);
    let pairs = BuildPairs::from_relation(&relation, &referenced);

    let config = ConfigCorpus::discover(root);
    let decl = declarations(root, &names, &config);
    let jvm_members =
        members_with_source(root, &names, &config, |p| matches!(p.name(), "java" | "kotlin"));

    Estate {
        members: federation.members.iter().map(|m| m.name.clone()).collect(),
        stores_read: federation.members.len(),
        rows,
        decl,
        relation,
        pairs,
        jvm_members,
    }
}

// ── The report ─────────────────────────────────────────────────────────────

fn count_by<K: Ord, I: IntoIterator<Item = K>>(items: I) -> BTreeMap<K, usize> {
    let mut out = BTreeMap::new();
    for k in items {
        *out.entry(k).or_default() += 1;
    }
    out
}

fn render_counts<K: std::fmt::Display>(counts: &BTreeMap<K, usize>) -> String {
    counts.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(" · ")
}

fn report(root: &Path, e: &Estate, product: &Judgement, census: &Judgement, rec: &Reconciliation) {
    println!(
        "S-471 / CR-152 §3.2 A1 over {} — {} manifest members, {} stores read (read-only), \
         {} committing Java/Kotlin source; {} import rows (imports, unresolved, .java/.kt)",
        root.display(),
        e.members.len(),
        e.stores_read,
        e.jvm_members.len(),
        e.rows.len(),
    );
    let consumers: BTreeSet<&str> = e.rows.iter().map(|r| r.consumer.as_str()).collect();
    let outside: Vec<&&str> = consumers.iter().filter(|m| !e.jvm_members.contains(**m)).collect();
    println!(
        "  consumers holding a row: {} (outside the Java/Kotlin members: {outside:?})",
        consumers.len()
    );
    println!("  build relation: {}", e.relation.headline.summary);
    println!(
        "  declarations: {} main-tree source files ({} refused: package ≠ directory), {} .avsc \
         ({} refused: unparseable), {} Avro types; owner index {} types (census rule {})",
        e.decl.sources.len(),
        e.decl.package_refusals().len(),
        e.decl.avro.len(),
        e.decl.avro_refusals().len(),
        e.decl.avro.iter().flat_map(|a| a.types.iter().flatten()).count(),
        OwnerIndex::build(&e.decl, Rule::Product).0.len(),
        OwnerIndex::build(&e.decl, Rule::Census).0.len(),
    );

    for (label, j) in [("product rule", product), ("census rule", census)] {
        let classes = j.row_classes();
        println!("\n  ROWS ({label}) — class × tree:");
        for class in ["self-owned", "exactly-one", "ambiguous", "no owner"] {
            let per: Vec<String> = [Tree::Main, Tree::Test, Tree::Other]
                .iter()
                .map(|t| {
                    format!("{} {}", t.label(), classes.get(&(class, *t)).copied().unwrap_or(0))
                })
                .collect();
            let total: usize = [Tree::Main, Tree::Test, Tree::Other]
                .iter()
                .map(|t| classes.get(&(class, *t)).copied().unwrap_or(0))
                .sum();
            println!("    {class:<12} {total:>6}  ({})", per.join(" · "));
        }
        let one: Vec<_> = j.exactly_one().collect();
        println!(
            "    exactly-one by backing: {} · by naming: {}",
            render_counts(&count_by(one.iter().map(|(_, _, _, b, _)| b.label()))),
            render_counts(&count_by(
                one.iter().map(|(_, _, _, _, n)| format!("{n:?}").to_lowercase())
            )),
        );
    }

    // ── Reach
    let reach = product.reach();
    let main = product.main_triples();
    let one: Vec<_> = product.exactly_one().collect();
    let backing_of: BTreeMap<Triple, Backing> =
        one.iter().map(|(t, _, _, b, _)| (t.clone(), *b)).collect();
    println!("\n  {}", verdict_line(HALF_REACH, REACH_FLOOR, reach.len(), 0));
    println!(
        "    main-tree triples by pair: {}",
        render_counts(&count_by(main.values().map(|p| p.label())))
    );
    let off_build = count_by(
        main.iter()
            .filter(|(_, p)| !matches!(p, PairClass::Build { .. }))
            .map(|((a, b, _), p)| format!("{a} → {b} ({})", p.label())),
    );
    for (pair, n) in &off_build {
        println!("      not a build pair: {pair}: {n} triple(s)");
    }
    println!(
        "    Reach by backing: {}",
        render_counts(&count_by(reach.iter().map(|t| backing_of[t].label())))
    );
    let test_triples: BTreeSet<Triple> = one
        .iter()
        .filter(|(_, tree, p, ..)| *tree == Tree::Test && p.admitted())
        .map(|(t, ..)| t.clone())
        .collect();
    println!(
        "    test-tree triples of an admitted pair (never Reach): {} ({} not also main-tree)",
        test_triples.len(),
        test_triples.iter().filter(|t| !reach.contains(*t)).count()
    );
    let reach_pairs = count_by(reach.iter().map(|(a, b, _)| (a.as_str(), b.as_str())));
    println!("    Reach pairs: {} directed", reach_pairs.len());
    let providers = count_by(reach.iter().map(|(_, b, _)| b.as_str()));
    println!("    Reach by provider: {}", render_counts(&providers));

    // ── Explanation
    let (explained, denominator, unexplained) = product.explanation();
    println!("\n  {}", verdict_line(HALF_EXPLANATION, EXPLANATION_FLOOR, explained, denominator));
    for (a, b) in &unexplained {
        println!("    unexplained: {a} → {b}");
    }

    // ── Precision
    let (ambiguous, cross) = product.precision();
    println!("\n  {}", verdict_line(HALF_PRECISION, PRECISION_CEILING, ambiguous, cross));
    let mut ambiguous_types: BTreeMap<(String, Vec<String>), usize> = BTreeMap::new();
    for (_, j) in &product.rows {
        if let (RowClass::Ambiguous { owners }, Some((t, _))) = (&j.class, &j.named) {
            *ambiguous_types.entry((t.clone(), owners.clone())).or_default() += 1;
        }
    }
    let mut by_rows: Vec<_> = ambiguous_types.into_iter().collect();
    by_rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    println!("    {} ambiguous types; the 15 with most rows:", by_rows.len());
    for ((t, owners), n) in by_rows.iter().take(15) {
        println!("      {n:>4}  {t}  owners {owners:?}");
    }
    println!(
        "    ambiguous rows by tree: {}",
        render_counts(&count_by(product.rows.iter().filter_map(|(_, j)| {
            matches!(j.class, RowClass::Ambiguous { .. }).then_some(j.tree.label())
        })))
    );

    // ── Refusals
    println!("\n  REFUSALS");
    for s in e.decl.package_refusals() {
        println!(
            "    package ≠ directory: {}/{} declares `{}`, path says `{}`",
            s.member,
            s.path,
            s.declared_package.as_deref().unwrap_or("(default)"),
            s.path_package
        );
    }
    for a in e.decl.avro_refusals() {
        println!(
            "    unparseable .avsc: {}/{}: {}",
            a.member,
            a.path,
            a.types.as_ref().unwrap_err()
        );
    }

    // ── Reconciliation
    let census_main = census.main_triples();
    let census_pairs: BTreeSet<(String, String)> =
        census.exactly_one().map(|((a, b, _), ..)| (a, b)).collect();
    let census_main_pairs: BTreeSet<(String, String)> =
        census_main.keys().map(|(a, b, _)| (a.clone(), b.clone())).collect();
    println!("\n  RECONCILIATION against CR-152 §2.1");
    println!(
        "    census-rule main-tree triples: {} (CR-152 census {CENSUS_MAIN_TRIPLES}); directed pairs \
         {} ({} with main-tree evidence; census {CENSUS_PAIRS})",
        census_main.len(),
        census_pairs.len(),
        census_main_pairs.len()
    );
    println!(
        "    census-rule main-tree triples by pair: {}",
        render_counts(&count_by(census_main.values().map(|p| p.label())))
    );
    let non_build: BTreeSet<&(String, String)> = census_pairs
        .iter()
        .filter(|(a, b)| !matches!(e.pairs.class(a, b), PairClass::Build { .. }))
        .collect();
    println!("    census-rule pairs NOT a build pair: {}", non_build.len());
    for (a, b) in &non_build {
        let named = CENSUS_NON_BUILD_PAIRS.contains(&(a.as_str(), b.as_str()));
        println!(
            "      {a} → {b}: {}{}",
            e.pairs.class(a, b).label(),
            if named { "  (named by CR-152 §2.1)" } else { "  (NOT named by CR-152 §2.1)" }
        );
    }
    for (a, b) in CENSUS_NON_BUILD_PAIRS {
        if !non_build.iter().any(|(x, y)| x == a && y == b) {
            println!(
                "      named by CR-152 §2.1 but not a census-rule non-build pair here: {a} → {b}"
            );
        }
    }
    println!(
        "    census-rule main-tree triples not in Reach: {} — by mechanism: {}",
        rec.missing.len(),
        render_counts(&count_by(rec.missing.iter().map(|(_, m)| mechanism_label(m))))
    );
    for (t, m) in rec.missing.iter().filter(|(_, m)| *m != Mechanism::TypeOnlyPair) {
        println!("      {} → {} : {}  [{m:?}]", t.0, t.1, t.2);
    }
    let type_only = count_by(
        rec.missing
            .iter()
            .filter(|(_, m)| *m == Mechanism::TypeOnlyPair)
            .map(|((a, b, _), _)| format!("{a} → {b}")),
    );
    for (pair, n) in &type_only {
        println!("      type-only pair {pair}: {n} triple(s)");
    }
    println!("    Reach triples the census rule does not count: {}", rec.extra.len());
    for (t, m) in &rec.extra {
        println!("      {} → {} : {}  [{m:?}]", t.0, t.1, t.2);
    }
}

fn mechanism_label(m: &Mechanism) -> &'static str {
    match m {
        Mechanism::TypeOnlyPair => "type-only pair",
        Mechanism::PackageRefusal(_) => "package/directory refusal",
        Mechanism::Unattributed => "UNATTRIBUTED",
    }
}

// ── The estate gate ────────────────────────────────────────────────────────

/// Measure the three halves over the reference estate and hold the result to
/// the recorded finding.
///
/// Skips without `LOGOS_REF_WORKSPACE`, like every estate gate in this binary —
/// which is why every classifier deciding a half is pinned by the always-run
/// fixtures in [`tests`], and why this run must be shown explicitly: it is
/// invisible to `gate.sh` and to CI.
#[test]
fn measure_cross_member_type_refs_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to a private copy of the reference workspace> \
             to run the S-471 cross-member type-reference gate (see \
             cross_member_type_refs_finding.txt for the recorded verdicts)."
        );
        return;
    };
    let estate = read_estate(&root);
    let product =
        judge(&estate.rows, &OwnerIndex::build(&estate.decl, Rule::Product), &estate.pairs);
    let census = judge(&estate.rows, &OwnerIndex::build(&estate.decl, Rule::Census), &estate.pairs);
    let rec = reconcile(&product, &census, &estate.decl);
    report(&root, &estate, &product, &census, &rec);

    assert!(
        rec.missing.iter().chain(&rec.extra).all(|(_, m)| *m != Mechanism::Unattributed),
        "a census difference has no named mechanism — explain it before recording a verdict"
    );

    let (explained, denominator, _) = product.explanation();
    let (ambiguous, cross) = product.precision();
    for line in [
        verdict_line(HALF_REACH, REACH_FLOOR, product.reach().len(), 0),
        verdict_line(HALF_EXPLANATION, EXPLANATION_FLOOR, explained, denominator),
        verdict_line(HALF_PRECISION, PRECISION_CEILING, ambiguous, cross),
    ] {
        assert!(
            RECORDED_FINDING.contains(&line),
            "the recorded finding does not carry this run's verdict line `{line}` — record the \
             new figures in cross_member_type_refs_finding.txt (and supersede ADR-70 on a \
             falsified half), never bend a classifier to reproduce the old ones",
        );
    }

    assert_eq!(estate.rows.len(), RECORDED_ROWS, "import rows drifted");
    assert_eq!(product.reach().len(), RECORDED_REACH, "Reach drifted");
    assert_eq!(product.reach_backing(), RECORDED_REACH_BACKING, "Reach's backing split drifted");
    assert_eq!(product.main_triple_pairs(), RECORDED_MAIN_TRIPLE_PAIRS, "the pair split drifted");
    assert_eq!((explained, denominator), RECORDED_EXPLANATION, "Explanation drifted");
    assert_eq!((ambiguous, cross), RECORDED_PRECISION, "precision rows drifted");
    assert_eq!(
        estate.decl.package_refusals().len(),
        RECORDED_PACKAGE_REFUSALS,
        "package/directory refusals drifted"
    );
    assert_eq!(
        census.main_triples().len(),
        RECORDED_CENSUS_RULE_MAIN_TRIPLES,
        "the census rule's main-tree triples drifted — the reconciliation moved"
    );
}

// ── Always-run classifier fixtures ─────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use logos_core::graph_store::BuildArtifactRow;

    fn src(member: &str, path: &str, declared: Option<&str>) -> SourceFile {
        let (path_package, name) = source_path_name(path).expect("fixture is a main-tree source");
        SourceFile {
            member: member.into(),
            path: path.into(),
            path_package,
            name,
            declared_package: declared.map(str::to_string),
        }
    }

    fn avsc(member: &str, types: &[&str]) -> AvroFile {
        AvroFile {
            member: member.into(),
            path: "src/main/avro/x.avsc".into(),
            types: Ok(types.iter().map(|t| (*t).to_string()).collect()),
        }
    }

    fn row(consumer: &str, path: &str, target: &str) -> Row {
        Row { consumer: consumer.into(), path: path.into(), line: Some(1), target: target.into() }
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.into(), b.into())
    }

    // ── The declaration ─────────────────────────────────────────────────────

    #[test]
    fn the_bounds_are_the_ones_declared_before_the_run() {
        // Parses the DECLARATION rather than comparing a constant with itself.
        for (half, constant) in [
            (HALF_REACH, REACH_FLOOR),
            (HALF_EXPLANATION, EXPLANATION_FLOOR),
            (HALF_PRECISION, PRECISION_CEILING),
        ] {
            let declared = declared_bound(DECLARED_FLOOR, half).unwrap();
            assert_eq!(
                declared, constant,
                "`{half}` bound constant is {constant:?} but the declaration says {declared:?} — \
                 change the constant only by re-deciding CR-152, never to clear a run"
            );
        }
        assert!(
            DECLARED_FLOOR.contains("2026-09-28T15:35:58Z"),
            "the declaration must carry the UTC timestamp that makes it a floor"
        );
    }

    #[test]
    fn a_bound_stated_twice_or_not_at_all_is_no_bound() {
        let twice = ">= 300 Reach triples\n>= 250 Reach triples\n";
        assert!(declared_bound(twice, HALF_REACH).is_err());
        assert!(declared_bound("Reach is 300\n", HALF_REACH).is_err());
        // A half whose name is a prefix of another word is not that half.
        assert!(declared_bound(">= 300 Reachable things\n", HALF_REACH).is_err());
        assert_eq!(
            declared_bound("  <= 10 % Precision ceiling of rows\n", HALF_PRECISION),
            Ok(PRECISION_CEILING)
        );
    }

    #[test]
    fn a_floor_holds_at_its_bound_and_fails_one_below_and_a_ceiling_fails_one_above() {
        assert!(REACH_FLOOR.holds(300, 0));
        assert!(!REACH_FLOOR.holds(299, 0));
        // 75 % of 52 is 39: 39/52 holds, 38/52 does not.
        assert!(EXPLANATION_FLOOR.holds(39, 52));
        assert!(!EXPLANATION_FLOOR.holds(38, 52));
        // 10 % of 2,492 is 249.2: 249 holds, 250 does not.
        assert!(PRECISION_CEILING.holds(249, 2492));
        assert!(!PRECISION_CEILING.holds(250, 2492));
        // At exactly the bound, both hold: 10 % of 2,490 is 249, 75 % of 52 is 39.
        assert!(PRECISION_CEILING.holds(249, 2490));
        // Nothing measured is never a pass, for a floor or a ceiling.
        assert!(!EXPLANATION_FLOOR.holds(0, 0));
        assert!(!PRECISION_CEILING.holds(0, 0));
    }

    #[test]
    fn a_verdict_line_names_its_half_its_figure_and_its_bound() {
        assert_eq!(
            verdict_line(HALF_REACH, REACH_FLOOR, 312, 0),
            "Reach: measured 312, floor >= 300 — HOLDS"
        );
        assert_eq!(
            verdict_line(HALF_PRECISION, PRECISION_CEILING, 131, 2492),
            "Precision ceiling: measured 131 of 2492 (5.3 %), ceiling <= 10 % — HOLDS"
        );
        assert_eq!(
            verdict_line(HALF_EXPLANATION, EXPLANATION_FLOOR, 38, 52),
            "Explanation: measured 38 of 52 (73.1 %), floor >= 75 % — FALSIFIED"
        );
    }

    // ── Trees and names ─────────────────────────────────────────────────────

    #[test]
    fn a_consumer_tree_is_a_path_segment_match_never_a_substring() {
        assert_eq!(consumer_tree("src/main/java/a/B.java"), Tree::Main);
        assert_eq!(consumer_tree("module-x/src/main/kotlin/a/B.kt"), Tree::Main);
        assert_eq!(consumer_tree("src/test/java/a/BTest.java"), Tree::Test);
        assert_eq!(consumer_tree("mod/src/test/java/a/BTest.java"), Tree::Test);
        // The near misses: one character from a tree.
        assert_eq!(consumer_tree("xsrc/main/java/a/B.java"), Tree::Other);
        assert_eq!(consumer_tree("src/mainline/java/a/B.java"), Tree::Other);
        assert_eq!(consumer_tree("buildSrc/src/it/java/a/B.java"), Tree::Other);
    }

    #[test]
    fn a_source_name_is_its_path_under_a_main_source_root() {
        assert_eq!(
            source_path_name("src/main/java/com/x/Svc.java"),
            Some(("com.x".into(), "Svc".into()))
        );
        assert_eq!(
            source_path_name("core/src/main/kotlin/com/x/Model.kt"),
            Some(("com.x".into(), "Model".into()))
        );
        assert_eq!(
            source_path_name("src/main/java/Root.java"),
            Some((String::new(), "Root".into()))
        );
        // Test trees, resources, other extensions and near-miss roots declare nothing.
        assert_eq!(source_path_name("src/test/java/com/x/SvcTest.java"), None);
        assert_eq!(source_path_name("src/main/resources/com/x/Svc.java.txt"), None);
        assert_eq!(source_path_name("src/main/java/com/x/Svc.groovy"), None);
        assert_eq!(source_path_name("xsrc/main/java/com/x/Svc.java"), None);
        assert_eq!(source_path_name("src/main/javax/com/x/Svc.java"), None);
    }

    #[test]
    fn the_declared_package_is_the_first_code_line_past_comments_and_annotations() {
        let java = "/*\n * Licence\n * package not.this;\n */\n// package nor.this;\npackage com.x.y;\n\nimport a.B;\n";
        assert_eq!(declared_package(java).as_deref(), Some("com.x.y"));
        let kotlin = "@file:JvmName(\"Util\")\npackage com.x.`in`\n\nfun f() = 1\n";
        assert_eq!(declared_package(kotlin).as_deref(), Some("com.x.in"));
        assert_eq!(declared_package("/* a */ package com.x; /* b */").as_deref(), Some("com.x"));
        // A file whose first code line is not `package` is the default package.
        assert_eq!(declared_package("import a.B;\nclass C {}\npackage late;\n"), None);
        // A `/*` inside a line comment or a string opens no block: the licence
        // banners `//*****` and a path like `docs/*` in a comment.
        let banner = "//*****************************\n// Copyright ACME\n//*****************************\npackage com.x.y;\n";
        assert_eq!(declared_package(banner).as_deref(), Some("com.x.y"));
        assert_eq!(declared_package("// see docs/* for details\npackage com.x.y;\n").as_deref(), Some("com.x.y"));
        assert_eq!(
            declared_package("@file:JvmName(\"a/*b\")\npackage com.x.k\n").as_deref(),
            Some("com.x.k")
        );
        // …and a `//` inside a block comment ends nothing.
        assert_eq!(declared_package("/* http://x.y\n package no; */\npackage com.z;").as_deref(), Some("com.z"));
        // `packageX` is not a package statement.
        assert_eq!(declared_package("packages.foo;\n"), None);
        assert_eq!(declared_package(""), None);
    }

    #[test]
    fn a_disagreeing_package_is_refused_by_the_product_rule_and_kept_by_the_census() {
        let decl = Declarations {
            sources: vec![
                src("lib", "src/main/java/com/x/Good.java", Some("com.x")),
                src("lib", "src/main/java/com/x/Moved.java", Some("com.old")),
                src("lib", "src/main/java/Root.java", None),
                src("lib", "src/main/java/com/x/NoStmt.java", None),
            ],
            avro: vec![],
        };
        assert_eq!(decl.package_refusals().len(), 2, "Moved and NoStmt disagree");
        let product = OwnerIndex::build(&decl, Rule::Product);
        let census = OwnerIndex::build(&decl, Rule::Census);
        assert!(product.owners("com.x.Good").is_some());
        assert!(product.owners("Root").is_some(), "the default package agrees with the root");
        assert!(product.owners("com.x.Moved").is_none());
        assert!(
            product.owners("com.old.Moved").is_none(),
            "refused, never re-read to its declared name"
        );
        assert!(product.owners("com.x.NoStmt").is_none());
        assert!(census.owners("com.x.Moved").is_some());
        assert!(census.owners("com.x.NoStmt").is_some());
    }

    #[test]
    fn avro_names_follow_the_namespace_rule_nested_and_in_unions() {
        let schema = r#"[
          {"type":"record","name":"Mail","namespace":"com.m.payload","fields":[
             {"name":"state","type":{"type":"enum","name":"State","symbols":["A"]}},
             {"name":"att","type":{"type":"array","items":
                {"type":"record","name":"Att","namespace":"com.m.att","fields":[]}}},
             {"name":"meta","type":{"type":"map","values":
                {"type":"record","name":"org.q.Meta","fields":[
                   {"name":"k","type":{"type":"enum","name":"Kind","symbols":["K"]}}]}}},
             {"name":"plain","type":"string","default":{"type":"record","name":"NotAType"}},
             {"name":"root","type":{"type":"record","name":"Rooted","namespace":"","fields":[
                {"name":"r","type":{"type":"enum","name":"Leaf","symbols":["L"]}}]}}
          ]},
          {"type":"enum","name":"Bare","symbols":["X"]},
          {"type":"fixed","name":"Hash","namespace":"com.m","size":16}
        ]"#;
        let mut got = avro_types(schema).unwrap();
        got.sort();
        assert_eq!(
            got,
            [
                "Bare",
                "Leaf",
                "Rooted",
                "com.m.att.Att",
                "com.m.payload.Mail",
                "com.m.payload.State",
                "org.q.Kind",
                "org.q.Meta"
            ]
        );
        assert!(
            avro_types("{ not json").is_err(),
            "an unparseable schema is refused, never skipped"
        );
    }

    #[test]
    fn a_member_declaring_a_type_by_source_and_by_avro_backs_it_with_both() {
        let decl = Declarations {
            sources: vec![src("models", "src/main/java/com/m/Mail.java", Some("com.m"))],
            avro: vec![
                avsc("models", &["com.m.Mail", "com.m.Only"]),
                avsc("other", &["com.m.Only"]),
            ],
        };
        let index = OwnerIndex::build(&decl, Rule::Product);
        assert_eq!(index.owners("com.m.Mail").unwrap().get("models"), Some(&Backing::Both));
        let only = index.owners("com.m.Only").unwrap();
        assert_eq!(only.len(), 2);
        assert_eq!(only.get("other"), Some(&Backing::Avro));
    }

    // ── Rows ────────────────────────────────────────────────────────────────

    fn index() -> OwnerIndex {
        OwnerIndex::build(
            &Declarations {
                sources: vec![
                    src("lib", "src/main/java/com/l/Svc.java", Some("com.l")),
                    src("app", "src/main/java/com/a/Own.java", Some("com.a")),
                    src("lib", "src/main/java/com/s/Shared.java", Some("com.s")),
                    src("app", "src/main/java/com/s/Shared.java", Some("com.s")),
                    src("lib", "src/main/java/com/d/Dup.java", Some("com.d")),
                    src("util", "src/main/java/com/d/Dup.java", Some("com.d")),
                ],
                avro: vec![avsc("models", &["com.m.Mail"])],
            },
            Rule::Product,
        )
    }

    #[test]
    fn a_target_names_its_type_exactly_else_its_enclosing_type_one_level_up() {
        let index = index();
        let (t, n, _) = resolve("com.l.Svc", &index).unwrap();
        assert_eq!((t.as_str(), n), ("com.l.Svc", Naming::Exact));
        // A static-member import and a nested type both fall back one level.
        let (t, n, _) = resolve("com.l.Svc.CONSTANT", &index).unwrap();
        assert_eq!((t.as_str(), n), ("com.l.Svc", Naming::Enclosing));
        // Never two levels, and never a sibling in the same package.
        assert!(resolve("com.l.Svc.Inner.X", &index).is_none());
        assert!(resolve("com.l.Other", &index).is_none());
        assert!(resolve("Svc", &index).is_none());
    }

    #[test]
    fn rows_classify_self_owned_first_then_exactly_one_ambiguous_or_no_owner() {
        let index = index();
        let class = |consumer: &str, target: &str| {
            classify(consumer, resolve(target, &index).map(|(_, _, o)| o))
        };
        assert_eq!(
            class("app", "com.l.Svc"),
            RowClass::ExactlyOne { provider: "lib".into(), backing: Backing::Source }
        );
        assert_eq!(
            class("app", "com.m.Mail"),
            RowClass::ExactlyOne { provider: "models".into(), backing: Backing::Avro }
        );
        assert_eq!(class("app", "com.a.Own"), RowClass::SelfOwned);
        // The consumer among several owners is self-owned, never ambiguous.
        assert_eq!(class("app", "com.s.Shared"), RowClass::SelfOwned);
        assert_eq!(
            class("app", "com.d.Dup"),
            RowClass::Ambiguous { owners: vec!["lib".into(), "util".into()] }
        );
        assert_eq!(class("app", "org.springframework.Bean"), RowClass::NoOwner);
        // The provider importing its own type is self-owned.
        assert_eq!(class("lib", "com.l.Svc"), RowClass::SelfOwned);
    }

    /// Pair evidence stated as its fields, never rebuilt by a copy of
    /// [`BuildPairs::from_relation`]'s loop — that reading is pinned against the
    /// product's join by [`pair_evidence_is_read_off_the_products_join`].
    fn relation_pairs() -> BuildPairs {
        BuildPairs {
            build: [
                (pair("app", "lib"), false),
                (pair("app", "parent"), false),
                (pair("app", "common"), true),
                (pair("web", "lib"), false),
            ]
            .into_iter()
            .collect(),
            collision: [pair("app", "models")].into_iter().collect(),
            dependency: [pair("app", "lib")].into_iter().collect(),
        }
    }

    #[test]
    fn a_pair_is_build_of_any_kind_then_collision_backed_then_type_only() {
        let pairs = relation_pairs();
        assert_eq!(pairs.class("app", "lib"), PairClass::Build { platform: false });
        assert_eq!(pairs.class("app", "parent"), PairClass::Build { platform: false });
        assert_eq!(pairs.class("web", "lib"), PairClass::Build { platform: false });
        assert_eq!(pairs.class("app", "common"), PairClass::Build { platform: true });
        assert_eq!(pairs.class("app", "models"), PairClass::CollisionBacked);
        assert_eq!(pairs.class("app", "util"), PairClass::TypeOnly);
        // Direction matters: the relation is directed.
        assert_eq!(pairs.class("lib", "app"), PairClass::TypeOnly);
        assert!(!PairClass::TypeOnly.admitted());
        assert!(PairClass::CollisionBacked.admitted());
        assert!(PairClass::Build { platform: true }.admitted());
    }

    /// The pair evidence read off a real relation: `join` over fixture facts, so
    /// the platform flag, the dependency denominator and the collision producers
    /// are the product's.
    #[test]
    fn pair_evidence_is_read_off_the_products_join() {
        let manifest = |produced: (&str, &str), refs: &[(&str, &str, &str)]| BuildManifestRow {
            path: "pom.xml".into(),
            format: "maven".into(),
            content_hash: None,
            status: "read".into(),
            detail: None,
            artifacts: std::iter::once(artifact("produced", None, produced.0, produced.1))
                .chain(refs.iter().map(|(kind, g, a)| artifact("referenced", Some(kind), g, a)))
                .collect(),
        };
        let facts: Vec<MemberBuildFacts> = vec![
            (
                "app".into(),
                vec![manifest(
                    ("g", "app"),
                    &[
                        ("dependency", "g", "lib"),
                        ("dependency", "g", "common"),
                        ("dependency", "g", "domain"),
                    ],
                )],
            ),
            ("lib".into(), vec![manifest(("g", "lib"), &[])]),
            ("common".into(), vec![manifest(("g", "common"), &[])]),
            ("domain".into(), vec![manifest(("g", "domain"), &[])]),
            // A producer of the colliding coordinate that also references it.
            ("legacy".into(), vec![manifest(("g", "domain"), &[("dependency", "g", "domain")])]),
            ("web".into(), vec![manifest(("g", "web"), &[("managed", "g", "lib")])]),
        ];
        let roster: Vec<Member> = facts
            .iter()
            .map(|(m, _)| Member { name: m.clone(), root: std::path::PathBuf::from(m) })
            .collect();
        let kinds: BTreeMap<String, MemberKind> =
            [("common".to_string(), MemberKind::Platform)].into_iter().collect();
        let relation = join(&roster, &kinds, &facts, &[]);
        let referenced = collision_references(&roster, &kinds, &facts);
        assert_eq!(
            referenced,
            [
                ("app".to_string(), ["g:domain".to_string()].into_iter().collect()),
                ("legacy".to_string(), ["g:domain".to_string()].into_iter().collect()),
            ]
            .into_iter()
            .collect(),
            "app and legacy reference the colliding g:domain; web and lib do not"
        );
        let pairs = BuildPairs::from_relation(&relation, &referenced);
        assert_eq!(pairs.class("app", "lib"), PairClass::Build { platform: false });
        assert_eq!(pairs.class("app", "common"), PairClass::Build { platform: true });
        assert_eq!(pairs.class("app", "domain"), PairClass::CollisionBacked);
        assert_eq!(pairs.class("app", "legacy"), PairClass::CollisionBacked);
        assert_eq!(pairs.class("web", "lib"), PairClass::Build { platform: false });
        assert_eq!(pairs.class("web", "domain"), PairClass::TypeOnly);
        // A producer referencing its own colliding coordinate is backed toward
        // the other producer, never toward itself.
        assert_eq!(pairs.class("legacy", "domain"), PairClass::CollisionBacked);
        assert_eq!(pairs.class("legacy", "legacy"), PairClass::TypeOnly);
        assert_eq!(
            pairs.dependency,
            [pair("app", "lib")].into_iter().collect(),
            "the denominator is non-platform dependency pairs only"
        );
    }

    fn artifact(role: &str, kind: Option<&str>, group: &str, id: &str) -> BuildArtifactRow {
        BuildArtifactRow {
            role: role.into(),
            kind: kind.map(str::to_string),
            group_id: Some(group.into()),
            artifact_id: Some(id.into()),
            version: Some("1".into()),
            scope: None,
            project_path: None,
            resolution: "resolved".into(),
            reason: None,
        }
    }

    // ── The three halves ────────────────────────────────────────────────────

    fn fixture_rows() -> Vec<Row> {
        vec![
            // Reach: two main-tree rows of one triple count once.
            row("app", "src/main/java/com/a/A.java", "com.l.Svc"),
            row("app", "src/main/java/com/a/B.java", "com.l.Svc"),
            // A static member of the same type: the same triple.
            row("app", "src/main/java/com/a/C.java", "com.l.Svc.CONST"),
            // Avro-backed, collision-backed pair.
            row("app", "src/main/java/com/a/A.java", "com.m.Mail"),
            // Test tree: exactly-one, admitted pair, never Reach.
            row("web", "src/test/java/com/w/T.java", "com.l.Svc"),
            // Type-only pair: exactly-one, not Reach.
            row("other", "src/main/java/com/o/O.java", "com.l.Svc"),
            // Ambiguous, self-owned and no-owner rows.
            row("app", "src/main/java/com/a/A.java", "com.d.Dup"),
            row("app", "src/main/java/com/a/A.java", "com.a.Own"),
            row("app", "src/main/java/com/a/A.java", "java.util.List"),
        ]
    }

    #[test]
    fn reach_counts_distinct_main_tree_triples_of_admitted_pairs_only() {
        let j = judge(&fixture_rows(), &index(), &relation_pairs());
        let reach = j.reach();
        assert_eq!(
            reach,
            [
                ("app".into(), "lib".into(), "com.l.Svc".into()),
                ("app".into(), "models".into(), "com.m.Mail".into()),
            ]
            .into_iter()
            .collect(),
        );
        assert_eq!(j.reach_backing(), (1, 1, 0), "Svc is source-backed, Mail Avro-backed");
        assert_eq!(j.main_triple_pairs(), (1, 0, 1, 1), "build, platform, collision, type-only");
        // The type-only triple is a main-tree triple the census grain keeps.
        assert_eq!(
            j.main_triples().get(&("other".into(), "lib".into(), "com.l.Svc".into())),
            Some(&PairClass::TypeOnly)
        );
    }

    #[test]
    fn explanation_is_the_share_of_dependency_pairs_a_reach_triple_explains() {
        let j = judge(&fixture_rows(), &index(), &relation_pairs());
        let (explained, denominator, unexplained) = j.explanation();
        assert_eq!((explained, denominator), (1, 1), "app → lib is explained");
        assert!(unexplained.is_empty());
        // Without the Reach row, the same pair is unexplained — and a test-tree
        // triple of the same pair never explains it.
        let rows: Vec<Row> = fixture_rows().into_iter().filter(|r| r.consumer != "app").collect();
        let j = judge(&rows, &index(), &relation_pairs());
        let (explained, denominator, unexplained) = j.explanation();
        assert_eq!((explained, denominator), (0, 1));
        assert_eq!(unexplained, vec![pair("app", "lib")]);
    }

    #[test]
    fn the_ceiling_is_ambiguous_rows_over_every_cross_member_row_and_excludes_self_and_no_owner() {
        let j = judge(&fixture_rows(), &index(), &relation_pairs());
        // Cross-member rows: 4 app→lib/models + web + other (exactly-one) + 1 ambiguous.
        assert_eq!(j.precision(), (1, 7));
        let classes = j.row_classes();
        assert_eq!(classes.get(&("self-owned", Tree::Main)), Some(&1));
        assert_eq!(classes.get(&("no owner", Tree::Main)), Some(&1));
        assert_eq!(classes.get(&("exactly-one", Tree::Test)), Some(&1));
    }

    #[test]
    fn every_census_difference_is_attributed_to_a_named_mechanism() {
        let decl = Declarations {
            sources: vec![
                src("lib", "src/main/java/com/l/Svc.java", Some("com.l")),
                // Refused under the product rule: its triple leaves Reach.
                src("lib", "src/main/java/com/l/Moved.java", Some("com.elsewhere")),
                // Two owners under the census, one under the product rule.
                src("lib", "src/main/java/com/l/Twice.java", Some("com.l")),
                src("util", "src/main/java/com/l/Twice.java", Some("com.wrong")),
            ],
            avro: vec![],
        };
        let rows = vec![
            row("app", "src/main/java/A.java", "com.l.Svc"),
            row("app", "src/main/java/A.java", "com.l.Moved"),
            row("app", "src/main/java/A.java", "com.l.Twice"),
            row("other", "src/main/java/O.java", "com.l.Svc"),
        ];
        let pairs = relation_pairs();
        let product = judge(&rows, &OwnerIndex::build(&decl, Rule::Product), &pairs);
        let census = judge(&rows, &OwnerIndex::build(&decl, Rule::Census), &pairs);
        let rec = reconcile(&product, &census, &decl);
        let t = |a: &str, b: &str, ty: &str| (a.to_string(), b.to_string(), ty.to_string());
        assert_eq!(
            rec.missing,
            vec![
                (
                    t("app", "lib", "com.l.Moved"),
                    Mechanism::PackageRefusal(vec!["lib/src/main/java/com/l/Moved.java".into()])
                ),
                (t("other", "lib", "com.l.Svc"), Mechanism::TypeOnlyPair),
            ]
        );
        assert_eq!(
            rec.extra,
            vec![(
                t("app", "lib", "com.l.Twice"),
                Mechanism::PackageRefusal(vec!["util/src/main/java/com/l/Twice.java".into()])
            )],
            "a refusal that removes a co-owner makes an ambiguous type exactly-one"
        );
    }

    // ── The store guard ─────────────────────────────────────────────────────

    fn user_version(db: &Path) -> i64 {
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    fn store_at(dir: &Path, offset: i64) -> (std::path::PathBuf, i64) {
        let db = dir.join("logos.db");
        drop(SqliteGraphStore::open(&db).expect("a fresh store migrates"));
        let latest = user_version(&db);
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "user_version", latest + offset).unwrap();
        drop(conn);
        (db, latest)
    }

    #[test]
    fn a_store_at_another_schema_version_is_refused_and_left_unmigrated() {
        for offset in [-1, 1] {
            let tmp = tempfile::tempdir().unwrap();
            let (db, latest) = store_at(tmp.path(), offset);
            let Err(err) = open_member_store(&db) else {
                panic!("a store at another version is refused")
            };
            assert!(err.contains("schema"), "the refusal names the schema mismatch: {err}");
            assert_eq!(user_version(&db), latest + offset, "the refused store was not migrated");
        }
    }

    #[test]
    fn a_store_at_this_binarys_version_opens_and_only_its_unresolved_jvm_imports_are_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let (db, latest) = store_at(tmp.path(), 0);
        let imports = EdgeKind::Imports as i64;
        let other_kind = if imports == 1 { 2 } else { 1 };
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO files (id, path) VALUES \
               (1, 'src/main/java/com/a/A.java'), (2, 'src/lib.rs'), (3, 'src/test/kotlin/K.kt');
             INSERT INTO unresolved_refs (file_id, source_symbol, target, form, kind, line, resolved) VALUES \
               (1, 's1', 'com::l::Svc', 1, {imports}, 3, 0),
               (1, 's2', 'com::l::Bound', 1, {imports}, 4, 1),
               (1, 's3', 'com::l::call', 1, {other_kind}, 5, 0),
               (2, 's4', 'crate::x::Y', 1, {imports}, 1, 0),
               (3, 's5', 'com::m::Mail', 1, {imports}, 2, 0),
               (NULL, 's6', 'com::l::Orphan', 1, {imports}, 1, 0);"
        ))
        .unwrap();
        drop(conn);

        let store = open_member_store(&db).expect("a current store opens read-only");
        assert_eq!(
            import_rows("m", &store).unwrap(),
            vec![
                Row {
                    consumer: "m".into(),
                    path: "src/main/java/com/a/A.java".into(),
                    line: Some(3),
                    target: "com.l.Svc".into()
                },
                Row {
                    consumer: "m".into(),
                    path: "src/test/kotlin/K.kt".into(),
                    line: Some(2),
                    target: "com.m.Mail".into()
                },
            ],
            "a resolved row, a non-import row, a non-JVM file and a file-less row are not rows"
        );
        assert_eq!(user_version(&db), latest);
    }

    #[test]
    fn only_java_and_kotlin_files_hold_import_rows() {
        assert!(is_jvm_source("src/main/java/A.java"));
        assert!(is_jvm_source("src/main/kotlin/A.kt"));
        assert!(!is_jvm_source("src/main/kotlin/build.kts"));
        assert!(!is_jvm_source("src/main/java/A.javax"));
        assert!(!is_jvm_source("lib.rs"));
    }
}
