//! The `logos.workspace.toml` manifest — schema and fail-loud parse
//! ([federation component], [FR-WS-01], [ADR-52]).
//!
//! A manifest at a **parent folder** declares an *application workspace*: a set
//! of member git repositories that together form one system. It is the only
//! on-disk artefact federation reads — the overlay itself is never persisted
//! ([ADR-52]). This module owns the schema ([`Manifest`]) and the parse
//! ([`parse`]); the up-tree location + member resolution live in the parent
//! [`super`] module.
//!
//! # Failure posture
//! Parsing mirrors the checked-in policy files ([config component], [FR-CF-01]):
//! `#[serde(deny_unknown_fields)]` so a typo'd key fails **loud** rather than
//! being silently ignored, and a malformed manifest is a [`ConfigError`] the
//! surfaces map to exit code 2. A *missing* manifest is **not** a fault — it is
//! the single-root case, handled one level up as `Ok(None)`.
//!
//! [federation component]: ../../../docs/specs/architecture/components/federation.md
//! [config component]: ../../../docs/specs/architecture/components/config.md
//! [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
//! [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
//! [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::models::pipeline::{InitAction, InitStep};

/// The manifest filename discovered by the up-tree walk ([`super::discover`]).
///
/// Named to avoid collision with the git-worktree resolution module
/// (`workspace.rs`) and `TargetClass::Workspace`; the user-facing term is
/// "workspace" ([ADR-52] Notes).
///
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
pub const MANIFEST_FILENAME: &str = "logos.workspace.toml";

/// The parsed `logos.workspace.toml` — the declared workspace, before member
/// resolution ([FR-WS-01]).
///
/// This is the *raw* manifest shape; [`super::discover`] turns it into a
/// [`super::Federation`] by resolving and validating each member against the
/// filesystem. Unknown top-level keys are rejected ([`serde(deny_unknown_fields)`]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The `[workspace]` section — name, members, default, and the optional
    /// autodiscover toggle.
    pub workspace: WorkspaceSection,
    /// User-asserted cross-service edges (`[[links]]`), carried through verbatim
    /// for the contract bridge to consume ([FR-WS-04]); labelled `asserted`,
    /// never fabricated. Empty when the manifest declares none.
    ///
    /// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,

    /// The `[governance]` workspace rule family ([FR-WS-13], [ADR-56]) — the
    /// cross-service policies [`super::governance`] evaluates over bridge
    /// matches. A **separate family** from the per-repo `.logos/rules.toml`
    /// ([FR-GV-01]): it lives in a different file, compiles to a different
    /// violation type, and is reported at the workspace level without ever
    /// touching a member's gated signal.
    ///
    /// Defaults to empty — a manifest declaring no `[governance]` produces **no**
    /// workspace governance output at all ([NFR-CC-04] honest empty).
    ///
    /// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
    /// [FR-GV-01]: ../../../docs/specs/requirements/FR-GV-01.md
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    /// [ADR-56]: ../../../docs/specs/architecture/decisions/ADR-56.md
    #[serde(default, skip_serializing_if = "Governance::is_empty")]
    pub governance: Governance,
}

/// The `[governance]` table — the workspace-level rule family ([FR-WS-13]).
///
/// Every field is an optional array-of-tables, so a manifest may declare any
/// subset. [`is_empty`](Self::is_empty) is the **honest-empty predicate**: when
/// it holds, [`super::governance::workspace_governance`] returns `None` and no
/// report is produced — an undeclared policy is never reported as a passing one
/// ([NFR-CC-04]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Governance {
    /// Named service layers over the **member set** (`[[governance.service_layers]]`)
    /// — the workspace analogue of `rules.toml`'s path-glob `[[layers]]`, except a
    /// band here is a set of *services*, not a set of files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_layers: Vec<ServiceLayer>,

    /// Forbidden cross-service calls between named layers
    /// (`[[governance.boundaries]]`), e.g. `edge` → `core`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub boundaries: Vec<ServiceBoundary>,

    /// Providers that must have **no** cross-service callers
    /// (`[[governance.no_cross_service_callers]]`) — the "deprecated endpoint"
    /// contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_cross_service_callers: Vec<NoCrossServiceCallers>,
}

impl Governance {
    /// Whether the workspace declares **no** governance rules at all — the
    /// honest-empty predicate ([NFR-CC-04]).
    ///
    /// `service_layers` alone does not count as a rule: layers are *vocabulary*,
    /// not policy. A manifest that names layers but forbids nothing has declared
    /// no contract to check, so it still produces no report.
    ///
    /// [NFR-CC-04]: ../../../docs/specs/requirements/NFR-CC-04.md
    pub fn is_empty(&self) -> bool {
        self.boundaries.is_empty() && self.no_cross_service_callers.is_empty()
    }
}

/// A `[[governance.service_layers]]` band: a named layer over workspace members
/// ([FR-WS-13]).
///
/// Where a `rules.toml` `[[layers]]` assigns *files* to a band by path glob, a
/// service layer assigns *members* to a band by name. A member named by two
/// bands takes the **first declaration** (the same first-wins tiebreak the
/// per-repo layer matcher uses); a member named by none is **unlayered** and no
/// boundary rule can classify it — so it is never a violation, never fabricated
/// ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceLayer {
    /// The layer name (e.g. `edge`, `core`) — the vocabulary
    /// [`ServiceBoundary`] refers to.
    pub name: String,
    /// The member names ([`Member::name`](super::Member::name)) in this layer.
    pub members: Vec<String>,
}

/// A `[[governance.boundaries]]` entry: a forbidden cross-service call from one
/// service layer to another ([FR-WS-13]).
///
/// Read over **bridge matches**, not stored edges: a [`BridgeEdge`](super::BridgeEdge)
/// whose consumer (`from`) member sits in layer `from` and whose provider (`to`)
/// member sits in layer `to` violates the boundary. An edge the bridge never
/// matched cannot violate anything — the rules quantify over what was actually
/// bound ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceBoundary {
    /// The calling layer that may not reach `to`.
    pub from: String,
    /// The forbidden callee layer.
    pub to: String,
    /// Human-readable rationale surfaced in the violation message (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A `[[governance.no_cross_service_callers]]` entry: a provider that must have
/// **no** cross-service callers ([FR-WS-13]) — the "deprecated endpoint" contract.
///
/// [`symbol`](Self::symbol) is a **glob** matched against the provider endpoint's
/// canonical [`LogosSymbol`](crate::model::LogosSymbol) string (compiled once via
/// the same `globset` matcher every `rules.toml` family uses), so a whole
/// deprecated namespace fences in one rule. [`member`](Self::member), when given,
/// additionally scopes the rule to providers owned by that member.
///
/// Any [`BridgeEdge`](super::BridgeEdge) whose **provider** endpoint matches is a
/// violation — the edge *is* the cross-service caller. The rule reads the bridge;
/// it never synthesises a caller set ([NFR-RA-05]).
///
/// [FR-WS-13]: ../../../docs/specs/requirements/FR-WS-13.md
/// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NoCrossServiceCallers {
    /// Glob matched against the provider endpoint's canonical symbol string.
    pub symbol: String,
    /// Scope the rule to providers owned by this member (optional; omit to match
    /// the symbol glob in **any** member).
    #[serde(default)]
    pub member: Option<String>,
    /// Human-readable rationale surfaced in the violation message (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

/// The `[workspace]` table.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    /// The workspace name (`[workspace] name`) — required.
    pub name: String,
    /// Member repository paths, **relative to the manifest directory**
    /// (`members = [...]`). Resolved and validated by [`super::discover`];
    /// absent ⇒ rely solely on [`autodiscover`](Self::autodiscover).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
    /// The optional default member (`[workspace] default`) — a member path as
    /// written in [`members`](Self::members) (or an autodiscovered directory
    /// name). Carried through; validated against the resolved set by
    /// [`super::discover`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The optional `[workspace.autodiscover]` toggle. Present ⇒ immediate child
    /// directories that are git roots (or already carry `.logos/logos.db`) are
    /// unioned with the explicit [`members`](Self::members) ([FR-WS-01]).
    ///
    /// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autodiscover: Option<Autodiscover>,
}

/// The `[workspace.autodiscover]` sub-table ([FR-WS-01]).
///
/// Its mere presence opts a workspace into auto-discovery of child repositories;
/// [`enabled`](Self::enabled) defaults to `true` so a bare `[workspace.autodiscover]`
/// section turns it on, while `enabled = false` keeps the section documented but
/// inert.
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Autodiscover {
    /// Whether auto-discovery is active (default `true`).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// serde default for [`Autodiscover::enabled`].
fn default_true() -> bool {
    true
}

/// A user-asserted cross-service edge (`[[links]]`) — the escape hatch for
/// couplings the static bridge cannot see (dynamic URLs, computed topics),
/// declared explicitly and labelled `asserted`, **never** fabricated
/// ([FR-WS-04], [ADR-52]).
///
/// Endpoints are free-form portable identifiers (a member-qualified symbol or
/// portable key); this foundation module parses and carries them, and the
/// contract bridge ([FR-WS-04]) interprets them. Kept minimal and stable so the
/// bridge story can grow the interpretation without a manifest-schema churn.
///
/// [FR-WS-04]: ../../../docs/specs/requirements/FR-WS-04.md
/// [ADR-52]: ../../../docs/specs/architecture/decisions/ADR-52.md
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// The asserted relation kind (e.g. `"http_call"`, `"grpc_call"`,
    /// `"publishes"`).
    pub relation: String,
    /// The producing/consuming endpoint the edge starts at.
    pub from: String,
    /// The endpoint the edge points to.
    pub to: String,
}

/// Parse a `logos.workspace.toml` at `path` ([FR-WS-01]).
///
/// # Errors
/// - [`ConfigError::Io`] if the file cannot be read (it was located by the
///   up-tree walk, so a read failure is a real fault, not "no manifest").
/// - [`ConfigError::Parse`] if the TOML is syntactically invalid or contains an
///   unknown key (`deny_unknown_fields`) — surfaced as exit code 2 ([FR-CF-01]).
///
/// [FR-WS-01]: ../../../docs/specs/requirements/FR-WS-01.md
/// [FR-CF-01]: ../../../docs/specs/requirements/FR-CF-01.md
pub fn parse(path: &Path) -> Result<Manifest, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Create or incrementally update the manifest at `root` from the approved
/// member-name set (`logos init --workspace`, [FR-WS-02]): a fresh manifest is
/// created with `name` and `members`; an existing one keeps its `name`,
/// `default`, `autodiscover`, and `links` untouched and only has `members`
/// upserted (sorted, de-duplicated). A result byte-identical to what's already
/// on disk reports [`InitAction::Unchanged`] without writing — the
/// write-if-different extension of [FR-IN-01]'s write-if-absent posture,
/// applied to the one field this command owns.
///
/// # Errors
/// [`ConfigError::Io`]/[`ConfigError::Parse`] reading a malformed existing
/// manifest; [`ConfigError::Write`] if the write itself fails.
///
/// [FR-WS-02]: ../../../docs/specs/requirements/FR-WS-02.md
/// [FR-IN-01]: ../../../docs/specs/requirements/FR-IN-01.md
pub fn upsert(root: &Path, name: &str, members: &[String]) -> Result<InitStep, ConfigError> {
    let path = root.join(MANIFEST_FILENAME);
    let mut members = members.to_vec();
    members.sort();
    members.dedup();

    let existing = if path.is_file() {
        Some(parse(&path)?)
    } else {
        None
    };

    let manifest = Manifest {
        workspace: WorkspaceSection {
            name: existing
                .as_ref()
                .map_or_else(|| name.to_string(), |m| m.workspace.name.clone()),
            members,
            default: existing.as_ref().and_then(|m| m.workspace.default.clone()),
            autodiscover: existing.as_ref().and_then(|m| m.workspace.autodiscover.clone()),
        },
        links: existing.as_ref().map_or_else(Vec::new, |m| m.links.clone()),
        // Like `links`, the workspace rule family is user-authored policy this
        // command does not own: `upsert` rebuilds the whole struct, so a section
        // not carried across here would be silently dropped on the next
        // `logos init --workspace` ([FR-WS-13]).
        governance: existing
            .as_ref()
            .map_or_else(Governance::default, |m| m.governance.clone()),
    };

    // The struct holds no maps/floats — every field is a String, Vec<String>,
    // Option<String>, Option<Autodiscover>, or a Vec of Link/governance tables of
    // the same — so TOML serialisation cannot fail in practice.
    let text = toml::to_string_pretty(&manifest)
        .expect("Manifest holds only TOML-representable scalar/table fields");

    if existing.is_some() {
        let current = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.clone(),
            source,
        })?;
        if current == text {
            return Ok(InitStep {
                target: MANIFEST_FILENAME.to_string(),
                action: InitAction::Unchanged,
                detail: String::new(),
            });
        }
    }

    std::fs::write(&path, &text).map_err(|source| ConfigError::Write {
        path: path.clone(),
        source,
    })?;

    Ok(InitStep {
        target: MANIFEST_FILENAME.to_string(),
        action: if existing.is_none() {
            InitAction::Created
        } else {
            InitAction::Updated
        },
        detail: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use tempfile::TempDir;

    /// Write `body` to `<tmp>/logos.workspace.toml` and return its path.
    fn write_manifest(tmp: &TempDir, body: &str) -> std::path::PathBuf {
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(&path, body).unwrap();
        path
    }

    /// A full manifest parses: name, members, default, autodiscover, and links.
    #[test]
    fn parses_a_full_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            r#"
            [workspace]
            name = "shop"
            members = ["web", "api"]
            default = "api"

            [workspace.autodiscover]
            enabled = true

            [[links]]
            relation = "http_call"
            from = "web::fetchCart"
            to = "api::get_cart"
            "#,
        );
        let m = parse(&path).expect("valid manifest parses");
        assert_eq!(m.workspace.name, "shop");
        assert_eq!(m.workspace.members, ["web", "api"]);
        assert_eq!(m.workspace.default.as_deref(), Some("api"));
        assert!(m.workspace.autodiscover.expect("present").enabled);
        assert_eq!(m.links.len(), 1);
        assert_eq!(m.links[0].relation, "http_call");
        assert_eq!(m.links[0].from, "web::fetchCart");
        assert_eq!(m.links[0].to, "api::get_cart");
    }

    /// The minimal manifest is just a name: members/default/autodiscover/links
    /// all default to empty/absent.
    #[test]
    fn parses_a_minimal_manifest() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nname = \"solo\"\n");
        let m = parse(&path).expect("a name-only manifest is valid");
        assert_eq!(m.workspace.name, "solo");
        assert!(m.workspace.members.is_empty());
        assert!(m.workspace.default.is_none());
        assert!(m.workspace.autodiscover.is_none());
        assert!(m.links.is_empty());
    }

    /// A bare `[workspace.autodiscover]` section turns discovery on (enabled
    /// defaults to true).
    #[test]
    fn bare_autodiscover_section_defaults_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.autodiscover]\n",
        );
        let m = parse(&path).unwrap();
        assert!(m.workspace.autodiscover.expect("present").enabled);
    }

    /// `enabled = false` keeps the section but disables discovery.
    #[test]
    fn autodiscover_can_be_disabled() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\n\n[workspace.autodiscover]\nenabled = false\n",
        );
        let m = parse(&path).unwrap();
        assert!(!m.workspace.autodiscover.expect("present").enabled);
    }

    /// An unknown key fails loud (`deny_unknown_fields`) — the FR-CF-01 posture,
    /// surfaced as a parse error (exit 2), never silently ignored.
    #[test]
    fn unknown_key_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(
            &tmp,
            "[workspace]\nname = \"a\"\nmembrs = [\"typo\"]\n",
        );
        let err = parse(&path).expect_err("an unknown key must fail loud");
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert_eq!(err.exit_code(), 2);
    }

    /// A missing `name` is a parse error — `[workspace] name` is required.
    #[test]
    fn missing_name_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "[workspace]\nmembers = [\"x\"]\n");
        assert!(matches!(parse(&path), Err(ConfigError::Parse { .. })));
    }

    /// Syntactically invalid TOML is a parse error, not a panic.
    #[test]
    fn invalid_toml_is_a_parse_error() {
        let tmp = TempDir::new().unwrap();
        let path = write_manifest(&tmp, "this is not toml = = =");
        assert!(matches!(parse(&path), Err(ConfigError::Parse { .. })));
    }

    /// A path that does not exist is an I/O error carrying the offending path.
    #[test]
    fn a_missing_file_is_an_io_error() {
        let tmp = TempDir::new().unwrap();
        let ghost = tmp.path().join(MANIFEST_FILENAME);
        match parse(&ghost) {
            Err(ConfigError::Io { path, .. }) => assert_eq!(path, ghost),
            other => panic!("expected an Io error, got {other:?}"),
        }
    }

    // ── upsert (FR-WS-02) ─────────────────────────────────────────────────

    /// No manifest yet: `upsert` creates one with the given name and members,
    /// sorted and de-duplicated.
    #[test]
    fn upsert_creates_a_fresh_manifest() {
        let tmp = TempDir::new().unwrap();
        let step = upsert(tmp.path(), "shop", &["web".into(), "api".into(), "api".into()])
            .expect("creates");
        assert_eq!(step.action, InitAction::Created);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.name, "shop");
        assert_eq!(m.workspace.members, ["api", "web"], "sorted + de-duplicated");
        assert!(m.workspace.default.is_none());
        assert!(m.workspace.autodiscover.is_none());
        assert!(m.links.is_empty());
    }

    /// An existing manifest keeps its name/default/autodiscover/links
    /// untouched; only `members` is upserted.
    #[test]
    fn upsert_preserves_hand_written_sections_on_an_existing_manifest() {
        let tmp = TempDir::new().unwrap();
        write_manifest(
            &tmp,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\ndefault = \"api\"\n\n\
             [workspace.autodiscover]\nenabled = false\n\n\
             [[links]]\nrelation = \"http_call\"\nfrom = \"web::c\"\nto = \"api::h\"\n",
        );

        let step = upsert(tmp.path(), "ignored-name", &["api".into(), "web".into()])
            .expect("updates");
        assert_eq!(step.action, InitAction::Updated);

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.name, "shop", "name is never overwritten by a re-run");
        assert_eq!(m.workspace.members, ["api", "web"]);
        assert_eq!(m.workspace.default.as_deref(), Some("api"));
        assert!(!m.workspace.autodiscover.unwrap().enabled, "preserved verbatim");
        assert_eq!(m.links.len(), 1, "links carried through untouched");
    }

    /// Re-running `upsert` with the same member set is a no-op — `Unchanged`,
    /// no write.
    #[test]
    fn upsert_is_unchanged_on_an_identical_rerun() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        let before = fs::read_to_string(&path).unwrap();

        let step = upsert(tmp.path(), "shop", &["api".into()]).expect("no-op");
        assert_eq!(step.action, InitAction::Unchanged);
        assert_eq!(fs::read_to_string(&path).unwrap(), before, "byte-identical, no rewrite");
    }

    /// A member dropped from the approved set on a re-run is pruned from
    /// `members` — the incremental-prune half of FR-WS-02.
    #[test]
    fn upsert_prunes_a_member_no_longer_in_the_approved_set() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into(), "web".into()]).unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();

        let m = parse(&tmp.path().join(MANIFEST_FILENAME)).unwrap();
        assert_eq!(m.workspace.members, ["api"], "web was pruned");
    }

    // ── the `[governance]` workspace rule family (S-258, FR-WS-13) ────────

    /// The `[governance]` table parses into the rule family: named service
    /// layers, forbidden boundaries, and no-cross-service-callers contracts.
    #[test]
    fn parses_the_governance_rule_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"web\", \"api\"]\n\n\
             [[governance.service_layers]]\nname = \"edge\"\nmembers = [\"web\"]\n\n\
             [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n\n\
             [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\nreason = \"gateway only\"\n\n\
             [[governance.no_cross_service_callers]]\nsymbol = \"*legacy*\"\nmember = \"api\"\n",
        )
        .unwrap();

        let m = parse(&path).unwrap();
        assert_eq!(m.governance.service_layers.len(), 2);
        assert_eq!(m.governance.service_layers[0].name, "edge");
        assert_eq!(m.governance.service_layers[0].members, ["web"]);
        assert_eq!(m.governance.boundaries.len(), 1);
        assert_eq!(m.governance.boundaries[0].from, "edge");
        assert_eq!(m.governance.boundaries[0].reason.as_deref(), Some("gateway only"));
        assert_eq!(m.governance.no_cross_service_callers.len(), 1);
        assert_eq!(m.governance.no_cross_service_callers[0].symbol, "*legacy*");
        assert_eq!(
            m.governance.no_cross_service_callers[0].member.as_deref(),
            Some("api")
        );
        assert!(!m.governance.is_empty(), "a declared boundary is a rule");
    }

    /// A manifest with no `[governance]` table parses to the empty family — the
    /// honest-empty predicate holds, so no report is ever produced ([NFR-CC-04]).
    #[test]
    fn a_manifest_without_governance_is_the_empty_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(&path, "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n").unwrap();

        let m = parse(&path).unwrap();
        assert!(m.governance.is_empty(), "no [governance] ⇒ nothing to check");
    }

    /// Service layers ALONE are vocabulary, not policy: the family still reads as
    /// empty, so naming bands without forbidding anything produces no report.
    #[test]
    fn service_layers_alone_are_still_the_empty_family() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n\n\
             [[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n",
        )
        .unwrap();

        let m = parse(&path).unwrap();
        assert!(
            m.governance.is_empty(),
            "layers declare vocabulary; only a boundary/no-callers rule is a contract",
        );
    }

    /// `upsert` rebuilds the whole manifest, so it must carry the user-authored
    /// rule family across — a re-run of `logos init --workspace` must never
    /// silently drop `[governance]` (the same guarantee `[[links]]` has).
    #[test]
    fn upsert_preserves_the_governance_family() {
        let tmp = TempDir::new().unwrap();
        upsert(tmp.path(), "shop", &["api".into()]).unwrap();

        let path = tmp.path().join(MANIFEST_FILENAME);
        let existing = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            format!(
                "{existing}\n[[governance.service_layers]]\nname = \"core\"\nmembers = [\"api\"]\n\n\
                 [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\n"
            ),
        )
        .unwrap();

        // A re-run that adds a member must not clobber the rules.
        upsert(tmp.path(), "shop", &["api".into(), "web".into()]).unwrap();

        let m = parse(&path).unwrap();
        assert_eq!(m.workspace.members, ["api", "web"], "the new member landed");
        assert_eq!(
            m.governance.boundaries.len(),
            1,
            "the user's workspace rules survived the re-run",
        );
        assert_eq!(m.governance.service_layers[0].name, "core");
    }

    /// An unknown key inside `[governance]` fails loud, like every other section
    /// (`deny_unknown_fields`) — a typo'd rule must never be silently ignored.
    #[test]
    fn an_unknown_governance_key_fails_loud() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(MANIFEST_FILENAME);
        fs::write(
            &path,
            "[workspace]\nname = \"shop\"\nmembers = [\"api\"]\n\n\
             [[governance.boundaries]]\nfrom = \"edge\"\nto = \"core\"\nseverity = \"warn\"\n",
        )
        .unwrap();

        let err = parse(&path).expect_err("an unknown rule key must fail loud");
        assert_eq!(err.exit_code(), 2);
    }
}
