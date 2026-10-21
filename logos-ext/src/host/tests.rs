//! The host's activation rules ([ADR-75]) and its refusals, over a local test
//! extension. The surfaces' tests prove each contribution reaches its surface;
//! these prove which extensions are asked at all.
//!
//! [ADR-75]: ../../../docs/specs/architecture/decisions/ADR-75.md

use super::*;
use crate::ui::{UiManifest, UiNavGroup, UiNavItem, UiNavScope};
use crate::ToolClass;
use std::sync::atomic::{AtomicUsize, Ordering};

const PROBE_TOOL: ExtensionTool = ExtensionTool::new("host_test_probe", ToolClass::Navigation);

/// An extension contributing one of each aggregated point, optionally gated.
struct Probe {
    id: &'static str,
    requires: Option<&'static str>,
    tools: &'static [ExtensionTool],
}

impl Probe {
    fn open(id: &'static str) -> Self {
        Self { id, requires: None, tools: &[] }
    }

    fn gated(id: &'static str, feature: &'static str) -> Self {
        Self { id, requires: Some(feature), tools: &[] }
    }
}

impl Extension for Probe {
    fn id(&self) -> &'static str {
        self.id
    }

    fn required_feature(&self) -> Option<&'static str> {
        self.requires
    }

    fn config_tables(&self) -> &'static [&'static str] {
        &["probe"]
    }

    fn validate_config_table(&self, table: &str, value: &crate::TomlValue) -> Result<(), String> {
        match value {
            crate::TomlValue::Table(_) => Ok(()),
            _ => Err(format!("`{table}` must be a table, checked by `{}`", self.id)),
        }
    }

    fn secret_keys(&self) -> &'static [&'static str] {
        &["probe.key"]
    }

    fn ui_manifest(&self) -> Option<UiManifest> {
        Some(UiManifest {
            nav: vec![UiNavItem {
                id: "probe",
                label: "Probe",
                path: "/probe",
                group: UiNavGroup::A,
                scope: UiNavScope::Member,
                replaced_in_workspace_by: Some("probe-workspace"),
            }],
            views: vec!["/probe", "/probe/detail"],
            config_panels: Vec::new(),
            dashboard_cards: Vec::new(),
        })
    }

    fn telemetry_tools(&self) -> &'static [ExtensionTool] {
        self.tools
    }
}

/// A gate enabling a fixed set of features.
struct Fixed(&'static [&'static str]);

impl FeatureGate for Fixed {
    fn enabled(&self) -> BTreeSet<String> {
        self.0.iter().map(|f| f.to_string()).collect()
    }
}

fn active_ids(host: &ExtensionHost) -> Vec<&'static str> {
    host.active().iter().map(|e| e.id()).collect()
}

#[test]
fn a_host_without_a_gate_is_empty_and_enables_no_feature() {
    let host = ExtensionHost::new();
    assert!(host.active().is_empty());
    assert!(host.enabled_features().is_empty());
    assert!(host.config_tables().is_empty() && host.ui_manifests().is_empty());
}

#[test]
fn an_extension_naming_no_feature_is_active_without_a_gate() {
    let host = ExtensionHost::new().register(Probe::open("open")).expect("registers");
    assert_eq!(active_ids(&host), ["open"]);
    assert!(host.is_active("open"));
    assert_eq!(host.config_tables(), [("open", "probe")]);
    assert_eq!(host.secret_keys(), [("open", "probe.key")]);
    let manifests = host.ui_manifests();
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].0, "open", "each manifest names the extension it came from");
}

#[test]
fn without_a_gate_a_gated_extension_contributes_nothing() {
    // BR-63: with no gate no feature is enabled, so an extension that names one
    // is simply not there.
    // It declares a telemetry tool, so "no telemetry tool" below is the gate's
    // doing rather than an empty declaration.
    let gated = Probe { tools: &[PROBE_TOOL], ..Probe::gated("gated", "feature-x") };
    let host = ExtensionHost::new().register(gated).expect("registers");
    assert_eq!(host.registered().count(), 1, "registered");
    assert!(active_ids(&host).is_empty(), "but not active");
    assert!(!host.is_active("gated"));
    assert!(host.config_tables().is_empty());
    assert!(host.secret_keys().is_empty());
    assert!(host.ui_manifests().is_empty());
    assert!(host.mcp_tools().is_empty() && host.cli_commands().is_empty());
    assert!(host.init_steps().is_empty() && host.telemetry_tools().is_empty());
}

#[test]
fn a_gate_activates_exactly_the_features_it_enables() {
    let enabled = Probe { tools: &[PROBE_TOOL], ..Probe::gated("enabled", "feature-x") };
    let host = ExtensionHost::new()
        .register(enabled)
        .and_then(|h| h.register(Probe::gated("disabled", "feature-y")))
        .and_then(|h| h.register(Probe::open("open")))
        .expect("registers")
        .with_gate(Fixed(&["feature-x"]));
    assert_eq!(active_ids(&host), ["enabled", "open"]);
    assert_eq!(host.enabled_features(), BTreeSet::from(["feature-x".to_string()]));
    // Every contribution point follows the gate, not just the active list.
    assert_eq!(host.config_tables(), [("enabled", "probe"), ("open", "probe")]);
    assert_eq!(host.secret_keys(), [("enabled", "probe.key"), ("open", "probe.key")]);
    let manifests: Vec<_> = host.ui_manifests().into_iter().map(|(id, _)| id).collect();
    assert_eq!(manifests, ["enabled", "open"]);
    assert_eq!(host.telemetry_tools(), [PROBE_TOOL]);
    let (tables, secrets) = host.config_claims();
    assert_eq!(tables.iter().map(|t| t.owner()).collect::<Vec<_>>(), ["enabled", "open"]);
    assert_eq!(secrets.iter().map(|c| c.owner).collect::<Vec<_>>(), ["enabled", "open"]);
}

#[test]
fn a_gate_enabling_nothing_activates_no_gated_extension() {
    let host = ExtensionHost::new()
        .register(Probe::gated("gated", "feature-x"))
        .and_then(|h| h.register(Probe::open("open")))
        .expect("registers")
        .with_gate(Fixed(&[]));
    assert_eq!(active_ids(&host), ["open"]);
}

#[test]
fn a_duplicate_id_is_refused() {
    let err = ExtensionHost::new()
        .register(Probe::open("same"))
        .and_then(|h| h.register(Probe::open("same")))
        .expect_err("a second `same` is refused");
    assert_eq!(err, HostError::DuplicateId("same"));
}

#[test]
fn a_telemetry_tool_declared_twice_is_refused() {
    let first = Probe { tools: &[PROBE_TOOL], ..Probe::open("first") };
    let second = Probe { tools: &[PROBE_TOOL], ..Probe::open("second") };
    let err = ExtensionHost::new()
        .register(first)
        .and_then(|h| h.register(second))
        .expect_err("the second declaration is refused");
    assert_eq!(err, HostError::DuplicateTelemetryTool("host_test_probe"));
}

/// The manifest serialises in the shape the SPA's registries read: a navigation
/// entry carries every `NavItem` field (snake case), the group and scope as the
/// SPA spells them, the replacement only when there is one, and the view routes.
#[test]
fn a_ui_manifest_serialises_in_the_shape_the_spa_registries_read() {
    let manifest = Probe::open("probe").ui_manifest().expect("a manifest");
    assert_eq!(
        serde_json::to_value(&manifest).expect("serialises"),
        serde_json::json!({
            "nav": [{
                "id": "probe",
                "label": "Probe",
                "path": "/probe",
                "group": "A",
                "scope": "member",
                "replaced_in_workspace_by": "probe-workspace"
            }],
            "views": ["/probe", "/probe/detail"],
            "config_panels": []
        })
    );
    let mut without = manifest.nav[0].clone();
    without.replaced_in_workspace_by = None;
    let value = serde_json::to_value(&without).expect("serialises");
    assert!(value.get("replaced_in_workspace_by").is_none(), "omitted when None: {value}");
    assert_eq!(serde_json::to_value(UiNavScope::App).expect("serialises"), "app");
}

/// A gate enabling `feature-x` on its first evaluation and nothing on any later
/// one — a gate whose answer changes between two evaluations.
struct Closing(Arc<AtomicUsize>);

impl FeatureGate for Closing {
    fn enabled(&self) -> BTreeSet<String> {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => BTreeSet::from(["feature-x".to_string()]),
            _ => BTreeSet::new(),
        }
    }
}

/// `active` is one gate evaluation, and the gate is asked again on the next
/// call: the host caches no answer.
#[test]
fn the_gate_is_asked_once_per_evaluation_and_again_on_the_next() {
    let reads = Arc::new(AtomicUsize::new(0));
    let host = ExtensionHost::new()
        .register(Probe::gated("gated", "feature-x"))
        .expect("registers")
        .with_gate(Closing(Arc::clone(&reads)));
    assert_eq!(active_ids(&host), ["gated"]);
    assert_eq!(reads.load(Ordering::SeqCst), 1, "the gate ran once");
    // The next evaluation sees the change.
    assert!(host.active().is_empty());
    assert_eq!(reads.load(Ordering::SeqCst), 2, "and again on the next call");
}

#[test]
fn the_config_claims_are_the_active_extensions_and_validate_through_their_owner() {
    let host = ExtensionHost::new()
        .register(Probe::open("open"))
        .and_then(|h| h.register(Probe::gated("gated", "feature-x")))
        .expect("registers");
    let (tables, secrets) = host.config_claims();
    let owned: Vec<_> = tables.iter().map(|t| (t.owner(), t.table())).collect();
    assert_eq!(owned, [("open", "probe")], "only the active extension claims");
    assert_eq!(secrets, [logos_core::config::SecretClaim { owner: "open", key: "probe.key" }]);
    assert_eq!(tables[0].validate(&crate::TomlValue::Table(Default::default())), Ok(()));
    assert_eq!(
        tables[0].validate(&crate::TomlValue::Integer(1)),
        Err("`probe` must be a table, checked by `open`".to_string()),
        "the owner's own validation, for its own table"
    );
}

/// The process-wide registration asks the gate once: the telemetry tools and
/// the config claims it registers come from one active set, so a gate that
/// changes mid-registration can never register an extension's tools but not its
/// tables, or the reverse.
#[test]
fn the_process_registration_asks_the_gate_once() {
    let reads = Arc::new(AtomicUsize::new(0));
    let gated = Probe { tools: &[PROBE_TOOL], ..Probe::gated("gated", "feature-x") };
    let host = ExtensionHost::new()
        .register(gated)
        .expect("registers")
        .with_gate(Closing(Arc::clone(&reads)));
    host.register_with_process().expect("registers with the process");
    assert_eq!(reads.load(Ordering::SeqCst), 1, "one gate evaluation");
    // Registered under that one (active) reading: the table is the extension's.
    logos_core::config::parse_config("[probe]\nk = 1\n", std::path::Path::new("config.toml"))
        .expect("[probe] was claimed under the same reading as the tools");
}

/// An extension whose one contribution is a skill name, optionally gated.
struct SkillOwner {
    id: &'static str,
    requires: Option<&'static str>,
    skill: &'static str,
}

impl Extension for SkillOwner {
    fn id(&self) -> &'static str {
        self.id
    }

    fn required_feature(&self) -> Option<&'static str> {
        self.requires
    }

    fn owned_skills(&self) -> Vec<String> {
        vec![self.skill.to_string()]
    }
}

/// The skill names are the one contribution read from every REGISTERED extension,
/// so an extension the gate leaves inactive still keeps its skill from `init`'s
/// retirement (HF-10); the extensions that name no skill add nothing.
#[test]
fn the_owned_skills_are_every_registered_extensions_active_or_not() {
    let host = ExtensionHost::new()
        .register(SkillOwner { id: "open", requires: None, skill: "open-skill" })
        .unwrap()
        .register(SkillOwner { id: "off", requires: Some("off-feature"), skill: "off-skill" })
        .unwrap()
        .register(Probe::open("quiet"))
        .unwrap()
        .with_gate(Fixed(&["another-feature"]));
    assert_eq!(active_ids(&host), ["open", "quiet"], "the gated one is inactive");
    assert_eq!(host.owned_skills(), ["open-skill", "off-skill"], "yet its skill is owned");
    assert!(ExtensionHost::new().owned_skills().is_empty(), "no extension, no skill");
}
