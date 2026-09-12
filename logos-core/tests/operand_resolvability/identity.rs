//! **S-384 — service-identity resolvability across the deploy corpus**
//! ([CR-121] CRA-02 and CRA-03, [FR-WS-20], [FR-WS-22], [FR-CG-09],
//! [NFR-CC-04]).
//!
//! This is a **blocking measurement gate**. [CR-121] §6 promises that "on the
//! reference workspace, resolved identity-based cross-service REST edges rise
//! from 0 to at least 31", and records the join that would produce them as its
//! *"single largest unvalidated inference"* (CRA-02, CRA-03). Eight stories —
//! [S-385] through [S-390], [S-395] and [S-396] — stay unplanned unless the
//! measured figure clears a floor declared **before** the run.
//!
//! # The mechanism under test, in the estate's own terms
//!
//! A consumer's `application.yml` commits a *local* base URL and a set of path
//! keys under one prefix:
//!
//! ```yaml
//! mailbox-aggregate:
//!   api:
//!     base-url: http://localhost:9000            # overridden at deploy time
//!     uri-get-mailbox: /v1/users/{userId}/mailboxes/{mailboxId}
//! ```
//!
//! and its **deploy** evidence — a Helm values file, which the configuration
//! corpus does not admit — overrides that base URL with the address of the
//! service it actually calls:
//!
//! ```yaml
//! configmap:
//!   MAILBOXAGGREGATE_API_BASEURL: http://mailbox-api.pec-services.svc.cluster.local:9000
//! ```
//!
//! The first DNS label, `mailbox-api`, is a workspace member. So the call site
//! that reads `mailbox-aggregate.api.uri-get-mailbox` does carry, in committed
//! sources, **which provider it meant** — [FR-CG-09]'s ambiguity ceiling note
//! executed literally. Measuring whether that holds across 84 members, and
//! whether it yields pairs path-only matching would not already have bound, is
//! this module's whole job.
//!
//! # What decides the gate
//!
//! Only [FR-WS-22]'s headline population: **net-new** identity-resolved
//! consumer→provider pairs. A pair that path-only matching would already have
//! bound on its own is reported, but it is **not evidence for identity** and is
//! excluded from the figure the floor is read against. The floor is recorded in
//! `.pending/S-384-T1-floor.txt`, written before any of this code existed.
//!
//! # Read-only, and against the estate SOURCE
//!
//! Nothing here opens an `Engine`, indexes anything, or writes a byte under the
//! reference workspace — the pattern [S-380] established for `ConfigCorpus`.
//! The persisted `.logos` stores under the estate are a 1.4.7-generation index
//! and are deliberately **not** read: every figure below is computed from
//! committed source text on this run.
//!
//! # The recorded finding
//!
//! Lives once, in `operand_resolvability/identity_finding.txt`, embedded with
//! `include_str!` so it cannot be deleted or renamed without breaking
//! compilation and there is no second hand-maintained copy to rot — the same
//! discipline `client_call_refusal_finding.txt` follows.
//!
//! [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
//! [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
//! [FR-WS-22]: ../../../docs/specs/requirements/FR-WS-22.md
//! [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
//! [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
//! [S-380]: ../../../docs/planning/journal.md#s-380-configuration-values-are-indexed-with-their-profile
//! [S-385]: ../../../docs/planning/journal.md#s-385-deploy-manifests-yield-member-identity-facts
//! [S-390]: ../../../docs/planning/journal.md#s-390-confidence-tiers-and-their-evidence-reach-every-surface
//! [S-395]: ../../../docs/planning/journal.md#s-395-the-workspace-service-graph-on-cli-mcp-and-the-web-surface
//! [S-396]: ../../../docs/planning/journal.md#s-396-gateway-configuration-routing-is-an-egress-recogniser

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use logos_core::extract::{self, FileInput, SymbolContext};
use logos_core::plugin::LanguageRegistry;
use logos_core::resolve::route_template::normalize_template;

use super::configuration_agreement::{parse_yaml, ConfigCorpus, Tree};

/// The recorded verdict, reproduced by the run and printed by it.
pub const RECORDED_FINDING: &str = include_str!("identity_finding.txt");

/// The materiality floor, declared before the run in
/// `docs/planning/sprints/.pending/S-384-T1-floor.txt` and reproduced here so
/// the assertion and the declaration cannot drift.
///
/// Derived from [CR-121] §6's hand-computed "at least 31" edges: one half of
/// the claim, rounded up. Below it the hand computation is wrong by more than a
/// factor of two — the failure mode [CR-121] §7 names as "exactly how CR-113
/// died" — and the dependent stories stay unplanned.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
pub const NET_NEW_FLOOR: usize = 16;

// ── Identity ────────────────────────────────────────────────────────────────

/// [FR-WS-20]'s tiered source list, most decisive first.
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// (1) A Kubernetes `Service` name, Helm chart name or `fullnameOverride`.
    Deploy,
    /// (2) A Compose service key.
    ///
    /// [FR-WS-20] also names the "Dockerfile image name" at this tier, and this
    /// walk does not read it — deliberately, and stated rather than left as a
    /// silent gap. A plain Dockerfile commits only `FROM <base>`, which
    /// identifies the **base image**, not the member; nothing in it says what
    /// the built image is called. On this estate the 88 `FROM` lines name
    /// `openjdk`, `maven`, `python`, `node` and one mock server — reading them
    /// would put `openjdk` into the decisive tier of 40 members at once.
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    Container,
    /// (3) The member's own directory name.
    Directory,
    /// (4) A framework application name (`spring.application.name`).
    Application,
    /// (5) A build-file artifact id — **recorded but never decisive**
    /// ([FR-WS-20] AC6).
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    Artifact,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Self::Deploy => "1 deploy",
            Self::Container => "2 container",
            Self::Directory => "3 directory",
            Self::Application => "4 application",
            Self::Artifact => "5 artifact",
        }
    }

    /// Whether a label at this tier may decide identity at all. Tier 5 never
    /// does: on this estate one member declares another's name and two declare
    /// the same generic id ([FR-WS-20] AC6).
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn is_decisive(self) -> bool {
        self != Self::Artifact
    }

    pub const ALL: [Self; 5] =
        [Self::Deploy, Self::Container, Self::Directory, Self::Application, Self::Artifact];
}

/// One identity claim: a member, a tier, a label, and the file that proves it.
///
/// Built only through [`Corpus::claim`], which refuses an empty or templated
/// label: 34 of this estate's 84 members commit `fullnameOverride: ""`, and
/// admitting it would invent one tier-1 collision spanning 40% of the workspace
/// and hide every real collision behind it.
#[derive(Debug, Clone)]
pub struct Claim {
    pub member: String,
    pub tier: Tier,
    pub label: String,
    pub evidence: String,
}

/// One target-host reference read out of the deploy corpus: a configuration
/// value that parses as a URL with a host ([FR-WS-20] AC3).
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
#[derive(Debug, Clone)]
pub struct TargetRef {
    /// The member whose deploy evidence carries this reference — the consumer.
    pub member: String,
    /// The deploy overlay it lives in (the values file's directory), so a
    /// per-overlay split is a group-by rather than a second walk.
    pub overlay: String,
    /// The **first DNS label** of the host, matched exactly: no suffix
    /// stripping ([FR-WS-20] AC4).
    pub label: String,
    pub scheme: String,
    pub port: Option<String>,
    /// The configuration key this value was read from, reduced to the
    /// separator-free form Spring's environment binding equates.
    pub via_flat: String,
    /// The key as written in the deploy file, for the census.
    pub via_key: String,
    pub file: String,
}

impl TargetRef {
    /// Whether this reference came from **deploy** evidence rather than from
    /// application configuration.
    ///
    /// The two populations must not be conflated. Every consumer here commits a
    /// localhost default for its own base-URL key, so a "did anything define
    /// this key?" test that admitted application sources would answer yes for
    /// essentially every site and report a vacuous zero residue.
    pub fn is_deploy(&self) -> bool {
        !self.overlay.starts_with(APPLICATION_OVERLAY)
    }
}

/// Overlay prefix marking a target read from application configuration rather
/// than from deploy evidence.
const APPLICATION_OVERLAY: &str = "application:";

/// Spring equates `MAILBOXAGGREGATE_API_BASEURL`, `mailboxaggregate.api.baseurl`
/// and `mailbox-aggregate.api.base-url` under relaxed binding. Reducing a key to
/// its separator-free lower-case form is that equality, computed once for both
/// sides of the join.
///
/// Deliberately **not** `canonical_key`: that one keeps `.` as a segment
/// boundary, which is exactly the information an environment-variable spelling
/// has already destroyed. The cost is that two genuinely different keys can
/// flatten together, so [`Corpus::flat_collisions`] measures how often that
/// happens rather than assuming it does not.
pub fn flat_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '-' && *c != '_' && *c != '.')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether a path is deploy evidence rather than application configuration,
/// and which kind.
///
/// Helm charts on this estate live under a **hidden** `.helm/` directory, which
/// the corpus walk skips by design — so a walk that admits deploy evidence
/// cannot reuse `ConfigCorpus::discover`'s `hidden(true)` setting, and says so
/// here rather than silently measuring nothing.
///
/// The [`DeployRole::Manifest`] arm is deliberately broad — any YAML that is
/// not an `application*` source — because a raw Kubernetes `Service` manifest
/// has no naming convention to match on. It is read for one key and discarded
/// if that key is absent.
fn deploy_role(rel: &str) -> Option<DeployRole> {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    let lower = name.to_ascii_lowercase();
    if name == "Chart.yaml" {
        return Some(DeployRole::Chart);
    }
    if lower.starts_with("values") && (lower.ends_with(".yaml") || lower.ends_with(".yml")) {
        return Some(DeployRole::Values);
    }
    if lower.starts_with("docker-compose") && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
    {
        return Some(DeployRole::Compose);
    }
    // Any other YAML may be a raw Kubernetes manifest. [FR-WS-20] names the
    // `Service` name as a tier-1 source, so a walk that read only Helm charts
    // would be leaving a tier-1 source unmeasured and reporting the gap as an
    // estate fact. `application*` is excluded because that is the OTHER
    // corpus — `ConfigCorpus` owns it, and admitting it here would double-count
    // every key it already proves.
    if (lower.ends_with(".yaml") || lower.ends_with(".yml")) && !lower.starts_with("application") {
        return Some(DeployRole::Manifest);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeployRole {
    Chart,
    Values,
    Compose,
    /// A raw Kubernetes manifest — read only for a `kind: Service` name.
    Manifest,
}

// ── The deploy corpus ───────────────────────────────────────────────────────

/// Everything the deploy walk collects, computed once per test binary.
#[derive(Default)]
pub struct Corpus {
    /// Workspace members, by directory name.
    pub members: BTreeSet<String>,
    /// Every identity claim, at every tier.
    pub claims: Vec<Claim>,
    /// Every target-host reference read out of deploy or application evidence.
    pub targets: Vec<TargetRef>,
    /// Deploy files admitted, for the census denominator.
    pub deploy_files: usize,
    /// Members carrying at least one deploy file.
    pub members_with_deploy: BTreeSet<String>,
    /// Raw `kind: Service` manifests that yielded one unambiguous name.
    pub service_manifests: usize,
    /// Raw `kind: Service` manifests refused as ambiguous — a multi-document
    /// file whose `kind` or `metadata.name` is not unique.
    pub service_manifests_ambiguous: usize,
    /// Values that are URL-shaped but whose host establishes **nothing** — a
    /// template expression, a placeholder, an IP literal, an unparsable
    /// authority — keyed by the reason, with one example each.
    ///
    /// [FR-WS-22] AC2 asks for a host reference that resolves to a member, to
    /// an external label, **or to nothing**. Dropping these before they became
    /// references would have left that third bucket permanently empty and made
    /// the external count read as the whole residue.
    ///
    /// [FR-WS-22]: ../../../docs/specs/requirements/FR-WS-22.md
    pub unresolvable_hosts: BTreeMap<&'static str, (usize, String)>,
    /// Spring keys whose separator-free forms collide, i.e. where [`flat_key`]
    /// equates two keys `canonical_key` keeps apart. The honest cost of the
    /// environment-variable join.
    pub flat_collisions: BTreeMap<String, BTreeSet<String>>,
    /// Every canonical Spring key the application corpus defines, by flat form.
    pub spring_by_flat: BTreeMap<String, BTreeSet<String>>,
}

impl Corpus {
    /// Record a URL-shaped value whose host establishes nothing, keeping one
    /// example per reason so the bucket is evidence rather than a bare count
    /// ([NFR-CC-04]).
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    fn unresolvable(&mut self, reason: &'static str, value: &str, file: &str) {
        let entry =
            self.unresolvable_hosts.entry(reason).or_insert_with(|| (0, String::new()));
        entry.0 += 1;
        if entry.1.is_empty() {
            entry.1 = format!("{value}  in {file}");
        }
    }

    /// Record an identity claim, refusing a label that establishes nothing: an
    /// empty string, or one carrying template syntax a deploy tool would have
    /// substituted (`{{ include "x.fullname" . }}`).
    fn claim(&mut self, member: &str, tier: Tier, label: &str, evidence: &str) {
        let label = label.trim();
        if label.is_empty() || label.contains(['{', '}', '$']) {
            return;
        }
        self.claims.push(Claim {
            member: member.to_string(),
            tier,
            label: label.to_string(),
            evidence: evidence.to_string(),
        });
    }

    /// Labels claimed at one tier, and by which members — the collision census
    /// ([FR-WS-20] AC2).
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn labels_at(&self, tier: Tier) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut out: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for c in self.claims.iter().filter(|c| c.tier == tier) {
            out.entry(c.label.as_str()).or_default().insert(c.member.as_str());
        }
        out
    }

    /// The member a host label identifies, or `None` — either because no member
    /// claims it at a decisive tier, or because two do at the same tier and a
    /// same-tier collision resolves to **nothing** rather than to a guess
    /// ([FR-WS-20] AC2, [NFR-RA-05]).
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    pub fn member_for(&self, label: &str) -> Option<&str> {
        for tier in Tier::ALL.into_iter().filter(|t| t.is_decisive()) {
            let claimants: BTreeSet<&str> = self
                .claims
                .iter()
                .filter(|c| c.tier == tier && c.label == label)
                .map(|c| c.member.as_str())
                .collect();
            match claimants.len() {
                0 => continue,
                1 => return claimants.into_iter().next(),
                _ => return None, // same-tier collision: neither
            }
        }
        None
    }

    /// The member a host label identifies under a **fall-through** reading of
    /// [FR-WS-20] AC2: a tier at which the label collides is skipped and the
    /// next tier is consulted, instead of the whole label resolving to nothing.
    ///
    /// This is **not** the requirement as written, and it is not the headline.
    /// It exists because the literal reading has one expensive consequence on
    /// this estate — a fork repository committing the original's chart name
    /// costs two consumers every edge to `archive-api` — and the gate is more
    /// useful if it says how much that reading costs than if it silently
    /// absorbs it. Both figures are reported; the literal one decides.
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn member_for_fallthrough(&self, label: &str) -> Option<&str> {
        for tier in Tier::ALL.into_iter().filter(|t| t.is_decisive()) {
            let claimants: BTreeSet<&str> = self
                .claims
                .iter()
                .filter(|c| c.tier == tier && c.label == label)
                .map(|c| c.member.as_str())
                .collect();
            if claimants.len() == 1 {
                return claimants.into_iter().next();
            }
        }
        None
    }

    /// The tier a label resolves at, for the census.
    pub fn tier_for(&self, label: &str) -> Option<Tier> {
        Tier::ALL.into_iter().filter(|t| t.is_decisive()).find(|&tier| {
            self.claims
                .iter()
                .filter(|c| c.tier == tier && c.label == label)
                .map(|c| c.member.as_str())
                .collect::<BTreeSet<_>>()
                .len()
                == 1
        })
    }
}

/// The immediate child keys of a top-level YAML mapping, **verbatim**.
///
/// [`parse_yaml`] cannot serve this: it canonicalises every key (dropping `-`
/// and `_`, lower-casing) and records only scalar leaves, so a Compose service
/// named `mailbox-api` reaches it as `mailboxapi` and a service with no scalar
/// child does not reach it at all. An identity label is matched by **exact**
/// first DNS label ([FR-WS-20] AC4), so it must be read unchanged.
///
/// Deliberately minimal: one indent level under one named top-level key, which
/// is all a Compose `services:` block is. Anything deeper is not a service name.
fn top_level_child_keys(text: &str, parent: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside: Option<usize> = None;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim().is_empty() || trimmed.trim_start().starts_with('#') {
            continue;
        }
        let indent = trimmed.len() - trimmed.trim_start().len();
        match inside {
            None => {
                if indent == 0 && trimmed.trim_start().starts_with(&format!("{parent}:")) {
                    inside = Some(usize::MAX);
                }
            }
            Some(child_indent) => {
                if indent == 0 {
                    inside = None;
                    continue;
                }
                let depth = if child_indent == usize::MAX { indent } else { child_indent };
                if indent != depth {
                    continue;
                }
                inside = Some(depth);
                let Some((key, _)) = trimmed.trim_start().split_once(':') else { continue };
                let key = key.trim().trim_matches(['"', '\'']);
                if !key.is_empty() && !key.contains(char::is_whitespace) {
                    out.push(key.to_string());
                }
            }
        }
    }
    out
}

/// The name a raw Kubernetes manifest registers as a `Service`, or `None`.
///
/// One `kind` and one `metadata.name` only. [`parse_yaml`] accumulates across
/// the documents of a multi-document file, so a file holding a `Deployment`
/// beside a `Service` cannot say which name belongs to which — and guessing
/// there is exactly what [NFR-RA-05] forbids. Refusing the ambiguous file is
/// the safe direction; the refusals are counted so they are visible rather than
/// silent.
///
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
fn service_name(values: &BTreeMap<String, BTreeSet<String>>) -> Option<String> {
    let one = |k: &str| values.get(k).filter(|v| v.len() == 1).and_then(|v| v.iter().next());
    match (one("kind"), one("metadata.name")) {
        (Some(kind), Some(name)) if kind == "Service" => Some(name.clone()),
        _ => None,
    }
}

/// The first `<artifactId>` a POM declares **outside** its `<parent>` block —
/// the module's own id, not the id it inherits from.
///
/// A tier-5 fact ([FR-WS-20] AC6), never decisive, so a deliberately shallow
/// read: this exists to *report* that build ids disagree with deploy names, not
/// to resolve anything with them.
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
fn pom_artifact_id(text: &str) -> Option<String> {
    let mut depth_in_parent = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("<parent>") {
            depth_in_parent = true;
        } else if t.starts_with("</parent>") {
            depth_in_parent = false;
        } else if !depth_in_parent {
            if let Some(rest) = t.strip_prefix("<artifactId>") {
                if let Some(id) = rest.strip_suffix("</artifactId>") {
                    return Some(id.trim().to_string());
                }
            }
        }
    }
    None
}

/// The first DNS label, scheme and port of a value that parses as a URL with a
/// host, or `None` ([FR-WS-20] AC3).
///
/// Matched **exactly**: no suffix stripping, so `apicurio-registry-service`
/// yields `apicurio-registry-service` and resolves to no member rather than to
/// a fabricated `apicurio` ([FR-WS-20] AC4).
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
pub fn url_target(value: &str) -> Option<(String, String, Option<String>)> {
    url_target_or_reason(value).ok()
}

/// Why a URL-shaped value established no host label. The refusal reasons are
/// named rather than collapsed to `None` so the "resolves to nothing" bucket
/// can say what it is made of.
pub fn url_target_or_reason(
    value: &str,
) -> Result<(String, String, Option<String>), &'static str> {
    let Some((scheme, rest)) = value.split_once("://") else { return Err("not a URL") };
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+') {
        return Err("not a URL");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.is_empty() {
        return Err("empty authority");
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, Some(p.to_string()))
        }
        _ => (authority, None),
    };
    let host = host.trim_matches(['[', ']']);
    // A ':' still in the host means the port was not a number (`http://h:xxxx`),
    // so the authority was never parsed and nothing about it is established.
    if host.is_empty() || host.contains(':') {
        return Err("port is not a number");
    }
    // An IP literal is an address, not an identity: `192.168.54.134` would
    // otherwise yield the "first DNS label" 192 and be reported as a host label
    // no member claims, padding the external census with four false entries.
    if host.split('.').all(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_digit())) {
        return Err("IP literal, not a name");
    }
    let label = host.split('.').next().unwrap_or(host);
    // A templated or placeholder host establishes nothing.
    if label.is_empty() {
        return Err("empty host label");
    }
    if label.contains(['{', '}', '$', '(', ')', '*']) {
        return Err("templated host");
    }
    Ok((label.to_string(), scheme.to_string(), port))
}

/// Walk the estate for deploy evidence and identity claims.
///
/// `hidden(false)`, unlike `ConfigCorpus::discover` and the parent module's
/// `measure`: Helm charts on this estate live under `.helm/`, and a walk that
/// skipped hidden directories would report a confident zero. `.git` is excluded
/// explicitly instead — the one hidden directory that carries no evidence and
/// costs the whole walk if entered.
fn walk_deploy(root: &Path, corpus: &mut Corpus) {
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .ignore(false)
        .parents(false)
        .filter_entry(|e| e.file_name() != std::ffi::OsStr::new(".git"))
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let Some(member) = rel.split('/').next().map(str::to_string) else { continue };
        if !corpus.members.contains(&member) {
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);

        if name == "pom.xml" {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if let Some(id) = pom_artifact_id(&text) {
                    corpus.claim(&member, Tier::Artifact, &id, &rel);
                }
            }
            continue;
        }

        let Some(role) = deploy_role(&rel) else { continue };
        let Ok(text) = std::fs::read_to_string(entry.path()) else { continue };
        corpus.deploy_files += 1;
        corpus.members_with_deploy.insert(member.clone());

        match role {
            DeployRole::Compose => {
                for service in top_level_child_keys(&text, "services") {
                    corpus.claim(&member, Tier::Container, &service, &rel);
                }
            }
            DeployRole::Manifest => {
                // Cheap reject before parsing: the overwhelming majority of the
                // estate's 3,351 YAML files are not Service manifests.
                if !text.contains("kind: Service") {
                    continue;
                }
                match service_name(&parse_yaml(&text)) {
                    Some(name) => {
                        corpus.service_manifests += 1;
                        corpus.claim(&member, Tier::Deploy, &name, &rel);
                    }
                    None => corpus.service_manifests_ambiguous += 1,
                }
            }
            DeployRole::Chart | DeployRole::Values => {
                let values = parse_yaml(&text);
                // Tier 1: the chart's own name, and any `fullnameOverride`.
                let tier1_keys: &[&str] = match role {
                    DeployRole::Chart => &["name"],
                    _ => &["fullnameoverride"],
                };
                for key in tier1_keys {
                    for v in values.get(*key).into_iter().flatten() {
                        corpus.claim(&member, Tier::Deploy, v, &rel);
                    }
                }
                if role == DeployRole::Values {
                    let overlay = overlay_of(&rel, &member);
                    for (key, vals) in &values {
                        for v in vals {
                            let (label, scheme, port) = match url_target_or_reason(v) {
                                Ok(t) => t,
                                Err("not a URL") => continue,
                                Err(reason) => {
                                    corpus.unresolvable(reason, v, &rel);
                                    continue;
                                }
                            };
                            let leaf = key.rsplit('.').next().unwrap_or(key);
                            corpus.targets.push(TargetRef {
                                member: member.clone(),
                                overlay: overlay.clone(),
                                label,
                                scheme,
                                port,
                                via_flat: flat_key(leaf),
                                via_key: key.clone(),
                                file: rel.clone(),
                            });
                        }
                    }
                }
            }
        }
    }
}

/// The deploy overlay a values file belongs to: its directory, member-relative.
/// `.helm/values.yaml` and `deploy-coll-bp/values.yaml` are two overlays of one
/// member, and [FR-WS-22] AC4 requires every edge attributable to the overlay
/// that produces it.
///
/// [FR-WS-22]: ../../../docs/specs/requirements/FR-WS-22.md
fn overlay_of(rel: &str, member: &str) -> String {
    let tail = rel.strip_prefix(member).unwrap_or(rel).trim_start_matches('/');
    match tail.rfind('/') {
        Some(i) => tail[..i].to_string(),
        None => String::new(),
    }
}

// ── Providers ───────────────────────────────────────────────────────────────

/// The routes each member serves, as the framework-promotion pass would read
/// them: one entry per `(normalized template, method)` it registers.
#[derive(Default)]
pub struct Providers {
    /// member → normalized template → the methods registered for it.
    pub by_member: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub files_scanned: usize,
    pub files_gated: usize,
    pub routes_seen: usize,
    pub routes_normalized: usize,
    /// A **ceiling**, not an extraction: mapping annotations textually present
    /// in the ledger-gated production files, per member.
    ///
    /// This exists because of what the run found on the provider side. The
    /// framework query captures a **string-literal** path; `pecserver-facade`
    /// registers
    /// `value = "/mailboxes/{" + EMAIL_ADDRESS_PARAMETER_NAME + "}/size"`, a
    /// concatenation, which the query does not capture at all — so the route is
    /// not refused, it is invisible. That is the provider-side mirror of the
    /// consumer-side composition problem [CR-113] and [CR-115] are about, and a
    /// measurement that reported `serving = 0` without saying so would be
    /// reporting a gap in the reader as a fact about the estate.
    ///
    /// Counted textually and deliberately: it is an upper bound on how many
    /// registrations exist, never a second route extractor. The number that
    /// matters is the **difference** between it and `routes_seen`.
    ///
    /// [CR-113]: ../../../docs/requests/CR-113-constant-folded-base-url-composition.md
    /// [CR-115]: ../../../docs/requests/CR-115-configuration-bound-base-url-resolution.md
    pub registrations_ceiling: BTreeMap<String, usize>,
}

/// Mapping-annotation spellings counted for [`Providers::registrations_ceiling`].
const MAPPING_ANNOTATIONS: [&str; 6] = [
    "@GetMapping",
    "@PostMapping",
    "@PutMapping",
    "@DeleteMapping",
    "@PatchMapping",
    "@RequestMapping",
];

impl Providers {
    /// The members registering a route at this normalized template, under any
    /// method.
    ///
    /// Method-agnostic, and that is a **disclosed bias in favour of
    /// [CR-121]**: adding the method to the comparison can only shrink the
    /// candidate set, which makes path-only matching *more* likely to reach the
    /// exactly-one rule on its own, which in turn moves pairs out of the
    /// net-new column. The consumer-side method is not recoverable from the
    /// composed-operand corpus this harness reads, so the figure produced here
    /// is an **upper bound** on net-new — below the floor it falsifies
    /// robustly; above it, the true figure may still be lower.
    ///
    /// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    pub fn serving(&self, template: &str) -> BTreeSet<&str> {
        self.by_member
            .iter()
            .filter(|(_, routes)| routes.contains_key(template))
            .map(|(m, _)| m.as_str())
            .collect()
    }
}

/// Scan the estate's production source for the routes each member serves.
///
/// The ledger gate is [FR-FW-04]'s, computed the way the parent module computes
/// the client-call gate: the real `extract` pass, then the plugin's own
/// `framework_detectors`. A file whose refs name no framework is not a
/// candidate, so an annotation-shaped call in a plain library cannot contribute
/// a route the promotion pass would never promote.
///
/// Test source is excluded: a controller in `src/test` is not a service this
/// estate deploys, and [CR-117]'s gate is the precedent for keeping the two
/// populations apart rather than averaging them.
///
/// [FR-FW-04]: ../../../docs/specs/requirements/FR-FW-04.md
/// [CR-117]: ../../../docs/requests/CR-117-canonical-topic-identity.md
fn scan_providers(root: &Path, members: &BTreeSet<String>) -> Providers {
    let mut out = Providers::default();
    let Ok(registry) = LanguageRegistry::load(root) else { return out };
    let symbols = SymbolContext::default();
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
        let Ok(rel) = entry.path().strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if Tree::of(&rel) == Tree::Test {
            continue;
        }
        let Some(member) = rel.split('/').next().map(str::to_string) else { continue };
        if !members.contains(&member) {
            continue;
        }
        let Some(plugin) = registry.for_path(&rel) else { continue };
        if plugin.query("frameworks").is_none() {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(entry.path()) else { continue };
        out.files_scanned += 1;

        let detectors = &plugin.semantics().framework_detectors;
        if detectors.is_empty() {
            continue;
        }
        let facts = extract::extract(&FileInput::new(&rel, &source), plugin, &symbols);
        let gated = facts
            .refs
            .iter()
            .any(|r| detectors.iter().any(|d| r.target == *d || r.target.starts_with(&**d)));
        if !gated {
            continue;
        }
        out.files_gated += 1;
        let ceiling: usize =
            MAPPING_ANNOTATIONS.iter().map(|a| source.matches(a).count()).sum();
        *out.registrations_ceiling.entry(member.clone()).or_default() += ceiling;

        for route in logos_core::resolve::framework::routes_in_source(plugin, &source) {
            out.routes_seen += 1;
            let Some(norm) = normalize_template(&route.path) else { continue };
            out.routes_normalized += 1;
            out.by_member
                .entry(member.clone())
                .or_default()
                .entry(norm)
                .or_default()
                .insert(route.method);
        }
    }
    out
}

// ── Pairs ───────────────────────────────────────────────────────────────────

/// What identity does to one consumer call site that resolved a path template
/// and a target identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairClass {
    /// Two or more members register this normalized template, so the
    /// exactly-one rule refuses on path alone and identity is what binds it —
    /// [FR-CG-09]'s ambiguity ceiling, discharged.
    ///
    /// [FR-CG-09]: ../../../docs/specs/requirements/FR-CG-09.md
    NetNewAmbiguous,
    /// Exactly one member registers this template and it **is** the identity
    /// target, so path-only matching would already have bound it. Reported, and
    /// deliberately **not** counted as evidence for identity.
    AlreadyBoundByPath,
    /// The identity target registers no route at this template, so identity
    /// produces no edge here whatever path-only matching would have done.
    TargetServesNothing,
}

impl PairClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::NetNewAmbiguous => "NET-NEW (path-only ties)",
            Self::AlreadyBoundByPath => "already bound by path alone",
            Self::TargetServesNothing => "identity target serves no such route",
        }
    }
}

/// One judged consumer→provider candidate.
#[derive(Debug, Clone)]
pub struct Pair {
    pub consumer: String,
    pub provider: String,
    pub overlay: String,
    pub template: String,
    pub normalized: String,
    pub via_key: String,
    pub label: String,
    pub serving: usize,
    pub class: PairClass,
    pub site: String,
}

/// Everything the run measures, so the report and the assertions read one
/// object rather than recomputing.
pub struct Findings {
    pub corpus: Corpus,
    pub providers: Providers,
    pub pairs: Vec<Pair>,
    /// The same judgement under [`Resolution::FallThrough`] — reported beside
    /// the headline, never instead of it.
    pub pairs_fallthrough: Vec<Pair>,
    /// Consumer sites that resolved a template but whose base-URL sibling key
    /// no deploy file overrides — the residue AC2 asks to be enumerated.
    pub sites_without_target: BTreeSet<String>,
    pub sites_considered: usize,
    /// Every `(member, flat base-URL key)` a resolved consumer call site reads,
    /// collected while judging and **independently of whether a deploy file
    /// overrides it**. AC2's "the via-key resolves to a call site" is a test
    /// against this set; deriving it from the matched pairs instead would make
    /// the answer true by construction.
    pub call_site_base_keys: BTreeSet<(String, String)>,
}

/// One cross-service REST edge at the grain [CR-121] §6 counts:
/// `(consumer member, normalized template, provider member)`.
///
/// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
type Edge<'a> = (&'a str, &'a str, &'a str);

impl Findings {
    /// Distinct cross-service REST edges at the grain [CR-121] §6 counts: one
    /// per `(consumer member, normalized template, provider member)`, so two
    /// call sites in one class reading one key are one edge.
    ///
    /// [CR-121]: ../../../docs/requests/CR-121-caller-to-callee-and-producer-to-consumer-across-services.md
    fn edges(&self, class: PairClass) -> BTreeSet<Edge<'_>> {
        self.pairs
            .iter()
            .filter(|p| p.class == class)
            .map(|p| (p.consumer.as_str(), p.normalized.as_str(), p.provider.as_str()))
            .collect()
    }

    /// **The figure the gate is read off**: net-new identity-resolved edges.
    pub fn net_new(&self) -> usize {
        self.edges(PairClass::NetNewAmbiguous).len()
    }

    /// Net-new under the fall-through counterfactual — the sensitivity of the
    /// headline to [FR-WS-20] AC2's same-tier collision rule.
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    pub fn net_new_fallthrough(&self) -> usize {
        self.pairs_fallthrough
            .iter()
            .filter(|p| p.class == PairClass::NetNewAmbiguous)
            .map(|p| (p.consumer.as_str(), p.normalized.as_str(), p.provider.as_str()))
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn already_bound(&self) -> usize {
        self.edges(PairClass::AlreadyBoundByPath).len()
    }

    /// Distinct member→member couplings among the resolved edges.
    pub fn member_pairs(&self) -> BTreeSet<(&str, &str)> {
        self.pairs
            .iter()
            .filter(|p| p.class != PairClass::TargetServesNothing)
            .map(|p| (p.consumer.as_str(), p.provider.as_str()))
            .collect()
    }
}

/// How a host label is resolved to a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// [FR-WS-20] AC2 as written: a same-tier collision resolves to nothing.
    ///
    /// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
    Literal,
    /// The counterfactual: a colliding tier is skipped and the next consulted.
    FallThrough,
}

/// What one pass of [`judge`] produced.
struct Judged {
    pairs: Vec<Pair>,
    sites_without_target: BTreeSet<String>,
    sites_considered: usize,
    call_site_base_keys: BTreeSet<(String, String)>,
}

/// Judge every consumer call site the client-call arm resolved a template for.
fn judge(root: &Path, corpus: &Corpus, providers: &Providers, mode: Resolution) -> Judged {
    let m = crate::measurement(root);
    let mut pairs = Vec::new();
    let mut sites_without_target = BTreeSet::new();
    let mut sites_considered = 0usize;
    let mut call_site_base_keys = BTreeSet::new();

    for stats in m.per_language.values() {
        for site in stats.sites.iter().filter(|s| s.gate_admitted) {
            if Tree::of(&site.file) == Tree::Test {
                continue;
            }
            let Some(template) = site.cr115.resolved() else { continue };
            let Some(normalized) = normalize_template(template) else { continue };
            let Some(consumer) = site.file.split('/').next().map(str::to_string) else { continue };
            sites_considered += 1;

            // The base URL of a path key is its sibling under the same prefix:
            // "the same configuration that supplies the path also supplies the
            // base URL" (CR-121 §3.2). Every key the site read is tried, so a
            // composition reading two keys is not silently dropped.
            let mut matched_deploy = false;
            for key in site.key_outcomes.iter().flatten().filter_map(|o| o.key()) {
                let Some((prefix, _)) = key.rsplit_once('.') else { continue };
                let base_flat = flat_key(&format!("{prefix}.base-url"));
                call_site_base_keys.insert((consumer.clone(), base_flat.clone()));
                for target in
                    corpus.targets.iter().filter(|t| t.member == consumer && t.via_flat == base_flat)
                {
                    matched_deploy |= target.is_deploy();
                    let provider = match mode {
                        Resolution::Literal => corpus.member_for(&target.label),
                        Resolution::FallThrough => corpus.member_for_fallthrough(&target.label),
                    };
                    let Some(provider) = provider else { continue };
                    let serving = providers.serving(&normalized);
                    let class = if !serving.contains(provider) {
                        PairClass::TargetServesNothing
                    } else if serving.len() == 1 {
                        PairClass::AlreadyBoundByPath
                    } else {
                        PairClass::NetNewAmbiguous
                    };
                    pairs.push(Pair {
                        consumer: consumer.clone(),
                        provider: provider.to_string(),
                        overlay: target.overlay.clone(),
                        template: template.to_string(),
                        normalized: normalized.clone(),
                        via_key: target.via_key.clone(),
                        label: target.label.clone(),
                        serving: serving.len(),
                        class,
                        site: format!("{}:{}", site.file, site.line),
                    });
                }
            }
            if !matched_deploy {
                sites_without_target.insert(format!("{}:{}", site.file, site.line));
            }
        }
    }
    Judged { pairs, sites_without_target, sites_considered, call_site_base_keys }
}

/// The measurement, computed once per test binary.
fn findings(root: &Path) -> &'static Findings {
    static ONCE: std::sync::OnceLock<Findings> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let mut corpus = Corpus::default();
        for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
            if entry.path().is_dir() && entry.path().join(".git").exists() {
                corpus.members.insert(entry.file_name().to_string_lossy().to_string());
            }
        }
        // Tier 3 is unconditional: "a member with none still yields its tier-3
        // directory name" (FR-WS-20 AC1).
        for member in corpus.members.clone() {
            let evidence = format!("{member}/");
            corpus.claim(&member, Tier::Directory, &member, &evidence);
        }
        walk_deploy(root, &mut corpus);

        // Tier 4 and the Spring key census come from the production corpus.
        let app = ConfigCorpus::discover(root);
        for source in &app.sources {
            let member = source.path.split('/').next().unwrap_or("").to_string();
            if !corpus.members.contains(&member) {
                continue;
            }
            for (key, values) in &source.values {
                corpus
                    .spring_by_flat
                    .entry(flat_key(key))
                    .or_default()
                    .insert(key.clone());
                if key == "spring.application.name" {
                    for v in values {
                        corpus.claim(&member, Tier::Application, v, &source.path);
                    }
                }
                // Application-committed URLs are target references too, and
                // reporting them is how the deploy corpus earns its place: on
                // this estate they are localhost and infrastructure, not peers.
                for v in values {
                    match url_target_or_reason(v) {
                        Err("not a URL") => {}
                        Err(reason) => corpus.unresolvable(reason, v, &source.path),
                        Ok((label, scheme, port)) => corpus.targets.push(TargetRef {
                            member: member.clone(),
                            overlay: format!(
                                "{APPLICATION_OVERLAY}{}",
                                source.profile.as_deref().unwrap_or("<none>")
                            ),
                            label,
                            scheme,
                            port,
                            via_flat: flat_key(key),
                            via_key: key.clone(),
                            file: source.path.clone(),
                        }),
                    }
                }
            }
        }
        corpus.flat_collisions =
            corpus.spring_by_flat.iter().filter(|(_, k)| k.len() > 1).map(|(f, k)| (f.clone(), k.clone())).collect();

        let providers = scan_providers(root, &corpus.members);
        let literal = judge(root, &corpus, &providers, Resolution::Literal);
        let fallthrough = judge(root, &corpus, &providers, Resolution::FallThrough);
        Findings {
            corpus,
            providers,
            pairs: literal.pairs,
            pairs_fallthrough: fallthrough.pairs,
            sites_without_target: literal.sites_without_target,
            sites_considered: literal.sites_considered,
            call_site_base_keys: literal.call_site_base_keys,
        }
    })
}

// ── The report ──────────────────────────────────────────────────────────────

/// Print the full census. Every acceptance criterion is a section, in order, so
/// the printed output can be read against the story without a decoder.
fn report(f: &Findings) {
    let c = &f.corpus;
    println!("\n=== S-384 · service-identity resolvability across the deploy corpus ===");
    println!(
        "members {} · deploy files {} · members carrying deploy evidence {}",
        c.members.len(),
        c.deploy_files,
        c.members_with_deploy.len(),
    );

    // ── AC1 ────────────────────────────────────────────────────────────────
    println!(
        "raw kind:Service manifests: {} named, {} refused as ambiguous",
        c.service_manifests, c.service_manifests_ambiguous,
    );
    println!("\n--- AC1 · self identity, per tier ---");
    println!("{:<14} {:>8} {:>8} {:>10}", "tier", "members", "labels", "collisions");
    for tier in Tier::ALL {
        let at = c.labels_at(tier);
        let members: BTreeSet<&str> =
            c.claims.iter().filter(|x| x.tier == tier).map(|x| x.member.as_str()).collect();
        let collisions = at.values().filter(|m| m.len() > 1).count();
        println!(
            "{:<14} {:>8} {:>8} {:>10}{}",
            tier.label(),
            members.len(),
            at.len(),
            collisions,
            if tier.is_decisive() { "" } else { "   (never decisive)" },
        );
        for (label, claimants) in at.iter().filter(|(_, m)| m.len() > 1) {
            let shown: Vec<&str> = claimants.iter().take(5).copied().collect();
            println!(
                "      COLLISION at {}: {label:?} claimed by {} members {shown:?}{}",
                tier.label(),
                claimants.len(),
                if claimants.len() > shown.len() { " …" } else { "" },
            );
        }
    }
    let decided: BTreeSet<&str> = c
        .claims
        .iter()
        .filter(|x| x.tier.is_decisive() && c.member_for(&x.label) == Some(x.member.as_str()))
        .map(|x| x.member.as_str())
        .collect();
    println!(
        "members yielding at least one decisive self identity: {} of {}",
        decided.len(),
        c.members.len(),
    );
    println!("\n  per-member best decisive tier (first 12 and any member with none):");
    let mut none_count = 0;
    for (i, member) in c.members.iter().enumerate() {
        let best = Tier::ALL
            .into_iter()
            .filter(|t| t.is_decisive())
            .find(|&t| {
                c.claims
                    .iter()
                    .any(|x| x.tier == t && x.member == *member && c.member_for(&x.label) == Some(member.as_str()))
            });
        let evidence = best.and_then(|t| {
            c.claims
                .iter()
                .find(|x| x.tier == t && x.member == *member && c.member_for(&x.label) == Some(member.as_str()))
                .map(|x| format!("{} <- {}", x.label, x.evidence))
        });
        match best {
            None => {
                none_count += 1;
                let collided: BTreeSet<&str> = c
                    .claims
                    .iter()
                    .filter(|x| x.member == *member && x.tier.is_decisive())
                    .map(|x| x.label.as_str())
                    .collect();
                println!("    {member:<44} NONE   (claims, all colliding: {collided:?})");
            }
            Some(tier) if i < 12 => {
                println!(
                    "    {member:<44} {}   {}",
                    tier.label(),
                    evidence.unwrap_or_default(),
                );
            }
            Some(_) => {}
        }
    }
    println!("  members with no decisive self identity at all: {none_count}");

    // ── AC2 ────────────────────────────────────────────────────────────────
    println!("\n--- AC2 · target host references and their via-keys ---");
    let deploy_targets: Vec<&TargetRef> = c.targets.iter().filter(|t| t.is_deploy()).collect();
    let app_targets = c.targets.len() - deploy_targets.len();
    println!(
        "target host references: {} total — {} from deploy evidence, {} from application config",
        c.targets.len(),
        deploy_targets.len(),
        app_targets,
    );

    let call_site_keys = &f.call_site_base_keys;
    let mut to_member = 0;
    let mut to_external = 0;
    let mut with_call_site = 0;
    let mut unmatched: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for t in &deploy_targets {
        if c.member_for(&t.label).is_some() {
            to_member += 1;
        } else {
            to_external += 1;
            unmatched.entry(t.label.as_str()).or_default().insert(t.member.as_str());
        }
        if call_site_keys.contains(&(t.member.clone(), t.via_flat.clone())) {
            with_call_site += 1;
        }
    }
    let to_nothing: usize = c.unresolvable_hosts.values().map(|(n, _)| n).sum();
    println!("  via-key resolves to a consumer call site: {with_call_site} of {}", deploy_targets.len());
    println!("  host label resolves to a member:          {to_member}");
    println!("  host label resolves to an external label: {to_external}");
    println!("  host establishes NOTHING:                 {to_nothing}  (URL-shaped, no usable host)");
    for (reason, (count, example)) in &c.unresolvable_hosts {
        println!("      {count:>4}  {reason:<24} e.g. {example}");
    }
    println!(
        "  every external here is INFERRED: no workspace manifest declares one, so the \
         declared/inferred split FR-WS-20 AC5 requires has only its inferred side on this estate",
    );
    println!(
        "  distinct labels: {} member / {} external",
        deploy_targets
            .iter()
            .filter(|t| c.member_for(&t.label).is_some())
            .map(|t| t.label.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        unmatched.len(),
    );
    println!("\n  MATCHED LABELS, with the tier that decided them:");
    for label in deploy_targets
        .iter()
        .filter(|t| c.member_for(&t.label).is_some())
        .map(|t| t.label.as_str())
        .collect::<BTreeSet<_>>()
    {
        println!(
            "    {label:<46} -> {} at tier {}",
            c.member_for(label).unwrap_or("?"),
            c.tier_for(label).map_or("-", Tier::label),
        );
    }
    println!("\n  UNMATCHED LABELS, enumerated for human adjudication:");
    for (label, claimers) in &unmatched {
        let sample = deploy_targets
            .iter()
            .find(|t| t.label == *label)
            .map(|t| {
                format!(
                    "{}://{}{}  via {}  in {}",
                    t.scheme,
                    t.label,
                    t.port.as_deref().map(|p| format!(":{p}")).unwrap_or_default(),
                    t.via_key,
                    t.file,
                )
            })
            .unwrap_or_default();
        println!("    {label:<46} referenced by {claimers:?}\n        e.g. {sample}");
    }
    println!(
        "\n  application-config target labels (reported, not joined): {:?}",
        c.targets
            .iter()
            .filter(|t| !t.is_deploy())
            .map(|t| t.label.as_str())
            .collect::<BTreeSet<_>>(),
    );
    println!(
        "  Spring keys whose separator-free forms collide: {} (the cost of the env-var join)",
        c.flat_collisions.len(),
    );
    for (flat, keys) in c.flat_collisions.iter().take(10) {
        println!("    {flat:<40} {keys:?}");
    }

    // ── Providers ──────────────────────────────────────────────────────────
    let p = &f.providers;
    println!("\n--- providers (production source only) ---");
    println!(
        "framework-capable files {} · ledger-gated {} · routes {} · normalized {} · members serving routes {}",
        p.files_scanned, p.files_gated, p.routes_seen, p.routes_normalized, p.by_member.len(),
    );
    let ceiling: usize = p.registrations_ceiling.values().sum();
    println!(
        "mapping annotations textually present in those files: {ceiling} — so {} registrations \
         yielded no readable route",
        ceiling.saturating_sub(p.routes_seen),
    );
    println!(
        "  (the ceiling counts Spring annotation spellings only, so a member on another \
         framework dialect shows 0 annotations beside a non-zero route count — that is the \
         ceiling being narrower than the extractor, not a negative gap)",
    );
    println!("  {:<32} {:>12} {:>8} {:>8}", "member", "annotations", "routes", "unread");
    for (member, ceil) in &p.registrations_ceiling {
        let found = p.by_member.get(member).map_or(0, |r| r.values().map(BTreeSet::len).sum());
        println!(
            "  {member:<32} {ceil:>12} {found:>8} {:>8}",
            ceil.saturating_sub(found),
        );
    }

    // ── AC3 ────────────────────────────────────────────────────────────────
    println!("\n--- AC3 · identity-resolved pairs, split by what path-only would have done ---");
    println!(
        "consumer call sites with a resolved template (production): {} across members {:?}",
        f.sites_considered,
        f.pairs.iter().map(|p| p.consumer.as_str()).collect::<BTreeSet<_>>(),
    );
    println!(
        "  of which NO DEPLOY file overrides their base-URL sibling key: {} \
         (application sources are excluded from this test: every consumer commits a localhost \
         default for its own key, so admitting them would answer yes for every site)",
        f.sites_without_target.len(),
    );
    for class in
        [PairClass::NetNewAmbiguous, PairClass::AlreadyBoundByPath, PairClass::TargetServesNothing]
    {
        let edges = f.edges(class);
        let sites = f.pairs.iter().filter(|x| x.class == class).count();
        println!("  {:<34} edges {:>4}   sites {:>4}", class.label(), edges.len(), sites);
    }
    println!("\n  every judged pair:");
    let mut seen = BTreeSet::new();
    for pair in &f.pairs {
        if !seen.insert((
            pair.consumer.clone(),
            pair.normalized.clone(),
            pair.provider.clone(),
            pair.class,
        )) {
            continue;
        }
        println!(
            "    {:<26} -> {:<26} serving={:<3} {:<34} {}",
            pair.consumer,
            pair.provider,
            pair.serving,
            pair.class.label(),
            pair.normalized,
        );
        println!(
            "        via {} -> host {} · template {} · site {}",
            pair.via_key, pair.label, pair.template, pair.site,
        );
    }
    println!(
        "\n  HEADLINE: net-new {} · already bound by path alone {} · member->member couplings {}",
        f.net_new(),
        f.already_bound(),
        f.member_pairs().len(),
    );
    println!(
        "  SENSITIVITY · same-tier collisions: net-new would be {} if a colliding tier fell \
         through to the next instead of resolving to nothing (FR-WS-20 AC2 as written is the \
         headline; this is the counterfactual)",
        f.net_new_fallthrough(),
    );
    println!(
        "  SENSITIVITY · method-agnostic matching: adding the consumer's HTTP verb to the \
         comparison can only shrink the candidate set, so it can only move pairs OUT of \
         net-new. The headline is an upper bound."
    );
    let unreadable: usize = f
        .providers
        .registrations_ceiling
        .values()
        .sum::<usize>()
        .saturating_sub(f.providers.routes_seen);
    // Computed, not claimed: how many of the unresolved pairs sit at a template
    // no member serves at all. For those, supplying the missing provider makes
    // the template serving=1 — already bound by path alone, never net-new.
    let silent: Vec<&Pair> =
        f.pairs.iter().filter(|p| p.class == PairClass::TargetServesNothing).collect();
    let silent_at_zero = silent.iter().filter(|p| p.serving == 0).count();
    println!(
        "  SENSITIVITY · provider coverage: {unreadable} mapping annotations yielded no readable \
         route. {silent_at_zero} of the {} unresolved pairs sit at a template NO member serves, \
         so supplying the missing provider makes them serving=1 — already bound by path alone, \
         not net-new. The gap moves net-new only where it would give a SECOND provider to a \
         template that already has one.",
        silent.len(),
    );
    for (a, b) in f.member_pairs() {
        println!("    {a} -> {b}");
    }

    // ── AC4 ────────────────────────────────────────────────────────────────
    println!("\n--- AC4 · pairs per deploy overlay ---");
    let mut by_overlay: BTreeMap<(&str, &str), BTreeSet<Edge<'_>>> = BTreeMap::new();
    for pair in f.pairs.iter().filter(|x| x.class != PairClass::TargetServesNothing) {
        by_overlay
            .entry((pair.consumer.as_str(), pair.overlay.as_str()))
            .or_default()
            .insert((pair.consumer.as_str(), pair.normalized.as_str(), pair.provider.as_str()));
    }
    println!("{:<30} {:<26} {:>6}", "member", "overlay", "edges");
    for ((member, overlay), edges) in &by_overlay {
        println!("{member:<30} {overlay:<26} {:>6}", edges.len());
    }
    // The union/intersection is taken **per member**: two members' overlays are
    // not alternatives to each other, and folding them together would report
    // every edge as overlay-specific whatever the overlays actually say.
    let mut union_total = 0;
    let mut common_total = 0;
    let mut differing_members = 0;
    for member in f.pairs.iter().map(|p| p.consumer.as_str()).collect::<BTreeSet<_>>() {
        let overlays: Vec<&BTreeSet<Edge<'_>>> =
            by_overlay.iter().filter(|((m, _), _)| *m == member).map(|(_, e)| e).collect();
        if overlays.is_empty() {
            continue;
        }
        let union: BTreeSet<_> = overlays.iter().flat_map(|s| s.iter()).copied().collect();
        let common: BTreeSet<_> = overlays
            .iter()
            .skip(1)
            .fold(overlays[0].clone(), |a, s| a.intersection(s).copied().collect());
        union_total += union.len();
        common_total += common.len();
        if union.len() != common.len() {
            differing_members += 1;
        }
        println!(
            "  {member:<30} overlays {} · union {} · in every overlay {} · overlay-specific {}",
            overlays.len(),
            union.len(),
            common.len(),
            union.len() - common.len(),
        );
    }
    println!(
        "workspace-wide: union {union_total} · in every overlay {common_total} · \
         overlay-specific {} across {differing_members} member(s)",
        union_total - common_total,
    );
}


// ── The measurement ─────────────────────────────────────────────────────────

/// **S-384's blocking gate.** Skips — loudly — when no corpus is configured, so
/// `cargo test --workspace` stays green on a machine without one.
///
/// The verdict is an assertion, not a printed table: without one, a regression
/// that flipped the finding would still pass, and the finding is what blocks
/// eight stories.
#[test]
fn measure_service_identity_resolvability_over_the_reference_workspace() {
    let Some(root) = crate::corpus_root() else {
        eprintln!(
            "SKIPPED: set LOGOS_REF_WORKSPACE=<path to the reference workspace> to run the \
             S-384 identity gate (see this file's module docs and identity_finding.txt \
             for the recorded finding)."
        );
        return;
    };
    let f = findings(&root);
    report(f);
    println!("\n{RECORDED_FINDING}");

    // The corpus must have engaged at all. A run that walked nothing reports
    // "0 net-new" exactly like a run that walked everything and found none, and
    // only this separates them.
    assert!(
        f.corpus.members.len() >= 2,
        "the workspace at {} yielded {} members — point LOGOS_REF_WORKSPACE at the \
         reference estate rather than at a single repository",
        root.display(),
        f.corpus.members.len(),
    );
    assert!(
        f.providers.files_gated > 0,
        "no file in the estate passed the FR-FW-04 ledger gate, so the provider side \
         of every pair is empty and the net-new figure is vacuous",
    );
    assert!(
        f.sites_considered > 0,
        "no consumer call site resolved a path template, so there was nothing to \
         judge — the client-call arm has drifted or the corpus is not the estate",
    );

    let net_new = f.net_new();
    println!(
        "\nVERDICT: net-new identity-resolved edges {net_new} against a floor of \
         {NET_NEW_FLOOR} declared before the run  =>  {}",
        if net_new >= NET_NEW_FLOOR { "HOLDS" } else { "FALSIFIED" },
    );

    // The recorded finding. S-384 measured this as FALSIFIED; the assertion
    // pins the verdict so a change that flips it has to be decided rather than
    // absorbed. Re-open CR-121 §8 and re-decide the CR before relaxing this —
    // do not edit it to make the suite green.
    assert!(
        net_new < NET_NEW_FLOOR,
        "S-384's recorded finding is that identity yields {} net-new edges, below the \
         floor of {NET_NEW_FLOOR} declared before the run; this run found {net_new}. \
         If that is real, CR-121's gate has re-opened: re-decide CR-121 §8 and plan \
         S-385..S-390, S-395 and S-396 — do not relax this assertion.",
        RECORDED_NET_NEW,
    );
    assert_eq!(
        net_new, RECORDED_NET_NEW,
        "the net-new figure moved from the recorded {RECORDED_NET_NEW} to {net_new} \
         without the finding being re-recorded",
    );
    // The split is the criterion, so both halves are pinned: a change that moved
    // pairs from one column to the other while leaving the total alone would
    // otherwise pass silently, and it is exactly the change that would matter.
    assert_eq!(
        f.already_bound(),
        RECORDED_ALREADY_BOUND,
        "the already-bound-by-path half of AC3's split moved from {RECORDED_ALREADY_BOUND} \
         to {}",
        f.already_bound(),
    );
    assert_eq!(
        f.net_new_fallthrough(),
        RECORDED_NET_NEW_FALLTHROUGH,
        "the same-tier-collision sensitivity moved from {RECORDED_NET_NEW_FALLTHROUGH} to {}; \
         the finding turns on it staying below the floor too",
        f.net_new_fallthrough(),
    );
    const {
        assert!(
            RECORDED_NET_NEW_FALLTHROUGH < NET_NEW_FLOOR,
            "the recorded counterfactual must itself be below the floor, or the falsification \
             rests on the reading of FR-WS-20 AC2 rather than on the measurement",
        );
    }

    // Census floors, not equalities: these can only grow as the estate grows,
    // and a re-clone must not redden the run. The verdict figures above are the
    // equalities.
    assert!(
        f.corpus.members.len() >= 80,
        "the estate yielded {} members, fewer than the 84 recorded",
        f.corpus.members.len(),
    );
    assert!(
        f.providers.routes_seen >= 80,
        "provider extraction yielded {} routes, well under the 94 recorded — the provider \
         side of every pair has shrunk and the split is not comparable",
        f.providers.routes_seen,
    );
}

/// The other half of AC3's split, pinned for the reason the assertion gives.
pub const RECORDED_ALREADY_BOUND: usize = 10;

/// Net-new under the fall-through reading of [FR-WS-20] AC2 — the sensitivity
/// that has to stay below the floor for the falsification to rest on the
/// measurement rather than on a reading of the requirement.
///
/// [FR-WS-20]: ../../../docs/specs/requirements/FR-WS-20.md
pub const RECORDED_NET_NEW_FALLTHROUGH: usize = 14;

/// The net-new figure this harness measured, pinned so a drift in either the
/// identity join or the provider scan is a failure rather than a quietly
/// different number.
pub const RECORDED_NET_NEW: usize = 12;

// ── Fixtures ────────────────────────────────────────────────────────────────
//
// The estate measurement runs only where a corpus is configured, so every rule
// it depends on is pinned here too, and each matcher is probed with the near
// miss it must reject rather than only with the case it must accept.

#[cfg(test)]
mod fixtures {
    use super::*;

    fn corpus_of(claims: &[(&str, Tier, &str)]) -> Corpus {
        let mut c = Corpus::default();
        for (member, tier, label) in claims {
            c.members.insert((*member).to_string());
            c.claim(member, *tier, label, "fixture");
        }
        c
    }

    #[test]
    fn a_k8s_dns_name_yields_its_first_label_with_scheme_and_port() {
        assert_eq!(
            url_target("http://mailbox-api.pec-services.svc.cluster.local:9000"),
            Some(("mailbox-api".into(), "http".into(), Some("9000".into()))),
        );
    }

    #[test]
    fn the_first_dns_label_is_matched_whole_and_never_suffix_stripped() {
        // FR-WS-20 AC4 names both of these: the rule exists so that a label
        // ending in a service-ish suffix resolves to nothing rather than to a
        // member whose name is a prefix of it.
        assert_eq!(
            url_target("http://apicurio-registry-service:8080").map(|t| t.0),
            Some("apicurio-registry-service".into()),
        );
        assert_eq!(url_target("http://keycloak-http").map(|t| t.0), Some("keycloak-http".into()));
    }

    #[test]
    fn a_host_whose_port_is_not_a_number_establishes_nothing() {
        // `http://localhost:xxxx` is a placeholder. Reading the whole authority
        // as a host would publish the label `localhost:xxxx`.
        assert_eq!(url_target("http://localhost:xxxx"), None);
        assert_eq!(url_target_or_reason("http://localhost:xxxx"), Err("port is not a number"));
    }

    #[test]
    fn a_refusal_says_which_kind_it_is_so_the_nothing_bucket_can_be_read() {
        // AC2's third bucket is only as good as these reasons. A refusal that
        // collapsed to one reason would report 18 unresolvable hosts without
        // saying that 13 are IP literals and 5 are templated ports.
        assert_eq!(url_target_or_reason("https://192.168.154.41/x"), Err("IP literal, not a name"));
        assert_eq!(url_target_or_reason("http://{{ .Values.host }}"), Err("templated host"));
        assert_eq!(url_target_or_reason("http://"), Err("empty authority"));
        // "not a URL" is the one refusal that is NOT a finding: it is every
        // ordinary configuration value, and counting it would drown the bucket.
        assert_eq!(url_target_or_reason("changeit"), Err("not a URL"));
        assert_eq!(url_target_or_reason("/v1/users"), Err("not a URL"));
        assert!(url_target_or_reason("http://mailbox-api:9000").is_ok());
    }

    #[test]
    fn an_ip_literal_is_an_address_not_an_identity() {
        assert_eq!(url_target("https://192.168.54.134:444/postedoc-ws"), None);
        assert_eq!(url_target("http://127.0.0.1"), None);
        // A name that merely begins with digits is still a name.
        assert_eq!(url_target("http://192abc.example.com").map(|t| t.0), Some("192abc".into()));
    }

    #[test]
    fn a_templated_or_malformed_host_establishes_nothing() {
        assert_eq!(url_target("http://{{ .Values.host }}/api"), None);
        assert_eq!(url_target("http://${SERVICE_HOST}"), None);
        assert_eq!(url_target("/v1/users"), None);
        assert_eq!(url_target("changeit"), None);
        assert_eq!(url_target("http://"), None);
    }

    #[test]
    fn relaxed_binding_equates_the_three_spellings_of_one_key() {
        let env = flat_key("MAILBOXAGGREGATE_API_BASEURL");
        assert_eq!(env, flat_key("mailbox-aggregate.api.base-url"));
        assert_eq!(env, flat_key("mailboxAggregate.api.baseUrl"));
        assert_eq!(env, "mailboxaggregateapibaseurl");
    }

    #[test]
    fn the_flat_form_still_separates_two_genuinely_different_keys() {
        // The join's whole risk is over-matching, so the near miss is pinned:
        // one extra character must keep the keys apart.
        assert_ne!(flat_key("archive-aggregate.api.base-url"), flat_key("archive-aggregate.api.base-url2"));
        assert_ne!(flat_key("mailbox.api.base-url"), flat_key("mailbox.api.base-urls"));
    }

    #[test]
    fn compose_service_keys_are_read_verbatim_and_only_one_level_down() {
        let text = "\
version: '3'
services:
  mailbox-api:
    image: x
    ports:
      - 9000
  kafka:
    image: y
volumes:
  data:
";
        assert_eq!(top_level_child_keys(text, "services"), vec!["mailbox-api", "kafka"]);
        // `image` and `ports` are the service's children, not services; `data`
        // belongs to a different top-level key entirely.
        assert!(!top_level_child_keys(text, "services").contains(&"image".to_string()));
        assert!(!top_level_child_keys(text, "services").contains(&"data".to_string()));
    }

    #[test]
    fn a_pom_declares_its_own_artifact_id_not_the_one_it_inherits() {
        let text = "\
<project>
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
  </parent>
  <artifactId>mailbox-api</artifactId>
</project>
";
        assert_eq!(pom_artifact_id(text).as_deref(), Some("mailbox-api"));
    }

    #[test]
    fn deploy_evidence_is_recognised_by_name_and_nothing_else_is() {
        assert_eq!(deploy_role("archive-api/.helm/Chart.yaml"), Some(DeployRole::Chart));
        assert_eq!(deploy_role("archive-api/.helm/values.yaml"), Some(DeployRole::Values));
        assert_eq!(deploy_role("m/deploy-coll/values-bp.yml"), Some(DeployRole::Values));
        assert_eq!(deploy_role("m/docker-compose.override.yml"), Some(DeployRole::Compose));
        // An application source is the OTHER corpus; admitting it here would
        // double-count every key ConfigCorpus already owns.
        assert_eq!(deploy_role("m/.helm/templates/service.yaml"), Some(DeployRole::Manifest));
        // The application corpus is ConfigCorpus's, not this walk's: admitting
        // it here would double-count every key it already proves.
        assert_eq!(deploy_role("m/src/main/resources/application.yml"), None);
        assert_eq!(deploy_role("m/src/main/resources/application-local.yaml"), None);
        assert_eq!(deploy_role("m/README.md"), None);
        assert_eq!(deploy_role("m/pom.xml"), None);
    }

    #[test]
    fn a_service_manifest_yields_its_name_and_an_ambiguous_one_yields_nothing() {
        let one = "\
apiVersion: v1
kind: Service
metadata:
  name: mailbox-api
";
        assert_eq!(service_name(&parse_yaml(one)).as_deref(), Some("mailbox-api"));

        // A multi-document file: parse_yaml accumulates across documents, so
        // neither name can be attributed. Refuse rather than guess.
        //
        // Two Service documents, so `kind` is unambiguously "Service" and ONLY
        // the name is in doubt. Written this way deliberately: a Deployment +
        // Service fixture is refused by the `kind` test and would leave the
        // name test unprobed — it passed in both states when the guard was
        // removed. Here, dropping the uniqueness guard picks the
        // alphabetically-first name and the assertion fails, which is what
        // makes this test worth having.
        let two = "\
apiVersion: v1
kind: Service
metadata:
  name: a-mailbox-api
---
apiVersion: v1
kind: Service
metadata:
  name: z-mailbox-api
";
        assert_eq!(service_name(&parse_yaml(two)), None);

        // And the mixed file, which the `kind` test refuses.
        let mixed = "\
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mailbox-api-deploy
---
apiVersion: v1
kind: Service
metadata:
  name: mailbox-api
";
        assert_eq!(service_name(&parse_yaml(mixed)), None);

        // Not a Service at all.
        let other = "\
kind: ConfigMap
metadata:
  name: mailbox-api
";
        assert_eq!(service_name(&parse_yaml(other)), None);

        // A Helm-templated name establishes nothing; `service_name` returns it
        // and `Corpus::claim` is what refuses it, so both halves are pinned.
        let templated = "\
kind: Service
metadata:
  name: '{{ include \"archive-api.fullname\" . }}'
";
        let name = service_name(&parse_yaml(templated));
        assert!(name.is_some(), "the reader returns it: {name:?}");
        let mut c = Corpus::default();
        c.claim("archive-api", Tier::Deploy, &name.expect("some"), "fixture");
        assert!(c.claims.is_empty(), "the claim guard refuses it: {:?}", c.claims);
    }

    #[test]
    fn an_empty_or_templated_label_is_never_an_identity() {
        // 34 of the reference estate's members commit `fullnameOverride: ""`.
        let c = corpus_of(&[
            ("a", Tier::Deploy, ""),
            ("b", Tier::Deploy, "   "),
            ("c", Tier::Deploy, "{{ include \"c.fullname\" . }}"),
        ]);
        assert!(c.claims.is_empty(), "claims: {:?}", c.claims);
    }

    #[test]
    fn two_members_claiming_one_label_at_one_tier_resolve_to_neither() {
        // FR-WS-20 AC2, and the reason `archive-api` resolves to nothing on the
        // reference estate: a fork repository commits the original's chart name.
        let c = corpus_of(&[
            ("archive-api", Tier::Deploy, "archive-api"),
            ("archive-api-logiclens-fork", Tier::Deploy, "archive-api"),
            ("archive-api", Tier::Directory, "archive-api"),
        ]);
        assert_eq!(c.member_for("archive-api"), None);
        // And the collision is not repaired by a lower tier: falling through
        // would let tier 3 break a tier-1 tie.
        assert_eq!(c.member_for_fallthrough("archive-api"), Some("archive-api"));
    }

    #[test]
    fn a_label_one_member_claims_resolves_at_its_best_tier() {
        let c = corpus_of(&[
            ("mailbox-api", Tier::Deploy, "mailbox-api"),
            ("mailbox-api", Tier::Directory, "mailbox-api"),
        ]);
        assert_eq!(c.member_for("mailbox-api"), Some("mailbox-api"));
        assert_eq!(c.tier_for("mailbox-api"), Some(Tier::Deploy));
    }

    #[test]
    fn a_build_artifact_id_never_decides_identity() {
        // FR-WS-20 AC6. Seven members of the reference estate declare the
        // artifact id `api`; one declares another member's name.
        let c = corpus_of(&[("mailbox-api", Tier::Artifact, "api")]);
        assert_eq!(c.member_for("api"), None);
        assert_eq!(c.member_for_fallthrough("api"), None);
        assert!(!Tier::Artifact.is_decisive());
        assert_eq!(Tier::ALL.iter().filter(|t| t.is_decisive()).count(), 4);
    }

    #[test]
    fn a_target_knows_whether_it_came_from_deploy_or_application_evidence() {
        let t = |overlay: &str| TargetRef {
            member: "m".into(),
            overlay: overlay.into(),
            label: "x".into(),
            scheme: "http".into(),
            port: None,
            via_flat: "k".into(),
            via_key: "k".into(),
            file: "f".into(),
        };
        assert!(t(".helm").is_deploy());
        assert!(t("deploy-coll-bp").is_deploy());
        assert!(!t("application:<none>").is_deploy());
        assert!(!t("application:local").is_deploy());
        // The near miss: a deploy overlay directory whose name merely begins
        // with the word must still be deploy evidence.
        assert!(t("application-overlays").is_deploy());
    }

    #[test]
    fn the_overlay_of_a_values_file_is_its_directory_within_the_member() {
        assert_eq!(overlay_of("m/.helm/values.yaml", "m"), ".helm");
        assert_eq!(overlay_of("m/deploy-coll-bp/values.yaml", "m"), "deploy-coll-bp");
        assert_eq!(overlay_of("m/values.yaml", "m"), "");
    }

    #[test]
    fn the_floor_is_the_one_declared_before_the_run() {
        // The floor lives in .pending/S-384-T1-floor.txt, written before this
        // file existed. If someone edits the constant to make a future run
        // clear it, this states what it was.
        const {
            assert!(NET_NEW_FLOOR == 16, "half of CR-121 section 6's hand-computed 31, rounded up");
            assert!(
                RECORDED_NET_NEW < NET_NEW_FLOOR,
                "the recorded finding is FALSIFIED; if that changes, re-decide CR-121 section 8",
            );
        }
    }
}
