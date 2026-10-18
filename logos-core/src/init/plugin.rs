//! The Claude Code plugin seen from `init` and `doctor` ([FR-IN-12], [ADR-81]).
//!
//! The plugin ([FR-IN-11]) carries the agent side: the `logos` MCP server and
//! the session-start readout. `init` keeps the repository side. This module is
//! the seam between the two:
//!
//! - **Detection.** Whether the plugin is enabled for a project, read from the
//!   three Claude Code settings scopes the host itself reads ([`enabled`]).
//! - **Enrolment.** The committed `.claude/settings.json` entries that make the
//!   host prompt every teammate to install the plugin ([`enrol`]), and their
//!   removal on uninstall ([`unenrol`]).
//! - **Doctor findings.** A `logos` server declared both by `.mcp.json` and by
//!   the plugin, and a binary older than the plugin's declared minimum
//!   ([`findings`]).
//!
//! Every function takes the user's Claude Code directory as a parameter; only
//! [`claude_config_dir`] reads the process environment, so the rules are
//! testable without touching it — the [`crate::config::user_config_dir`]
//! pattern.
//!
//! [FR-IN-11]: ../../../docs/specs/requirements/FR-IN-11.md
//! [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
//! [ADR-81]: ../../../docs/specs/architecture/decisions/ADR-81.md

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Map, Value};

use super::{read_optional, step, write_json};
use crate::models::pipeline::{InitAction, InitStep};

/// The plugin's id as the host keys it in `enabledPlugins`: plugin `logos`
/// from the marketplace `logos` (the repository's `.claude-plugin/marketplace.json`).
pub(crate) const PLUGIN_ID: &str = "logos@logos";

/// The marketplace name, the key of its `extraKnownMarketplaces` entry.
const MARKETPLACE: &str = "logos";

/// The repository that publishes the marketplace.
const MARKETPLACE_REPO: &str = "raffaelecamanzo/logos";

/// The variable Claude Code reads to relocate its user directory (`~/.claude`).
const CLAUDE_CONFIG_DIR_ENV: &str = "CLAUDE_CONFIG_DIR";

/// The project settings the enrolment is committed into, repo-relative.
const SETTINGS_REL: &str = crate::hooks::SETTINGS_REL;

/// The per-developer project settings, repo-relative. Never committed, and
/// read only: `init` writes nothing here.
const SETTINGS_LOCAL_REL: &str = ".claude/settings.local.json";

/// The plugin manifest inside a plugin root.
const MANIFEST_REL: &str = ".claude-plugin/plugin.json";

/// The plugin root inside the repository that publishes it.
const PUBLISHED_PLUGIN_REL: &str = "plugins/logos";

/// Whether the Claude Code plugin counts as enabled for an `init` run
/// ([FR-IN-12]).
///
/// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClaudePlugin {
    /// Read it from the Claude Code settings when `init` runs ([`enabled`]),
    /// the user scope from [`claude_config_dir`]. What the CLI passes.
    #[default]
    Detect,
    /// The plugin is not enabled: `init -i` writes today's files.
    Absent,
    /// The plugin is enabled: `init -i` enrols it and retires the legacy files.
    Enabled,
}

impl ClaudePlugin {
    /// Resolve to a yes/no for the project at `root`.
    pub(crate) fn is_enabled(self, root: &Path) -> bool {
        match self {
            Self::Detect => enabled(root, claude_config_dir().as_deref()),
            Self::Absent => false,
            Self::Enabled => true,
        }
    }
}

/// The user's Claude Code directory: `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`.
pub(crate) fn claude_config_dir() -> Option<PathBuf> {
    claude_config_dir_from(|name| std::env::var_os(name))
}

/// [`claude_config_dir`] over an injected environment lookup. An unset, empty
/// or relative value is skipped.
fn claude_config_dir_from(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| lookup(name).map(PathBuf::from).filter(|p| p.is_absolute());
    absolute(CLAUDE_CONFIG_DIR_ENV).or_else(|| absolute("HOME").map(|home| home.join(".claude")))
}

/// Is the plugin enabled for the project at `root`?
///
/// The host's own precedence, most specific first: the project's
/// `.claude/settings.local.json`, its `.claude/settings.json`, then the user's
/// `settings.json` in `claude_dir`. The first scope whose `enabledPlugins`
/// holds a boolean for [`PLUGIN_ID`] decides, so a project that sets it `false`
/// overrides a user who enabled it. A scope that is absent, unreadable, not
/// JSON, or silent about the plugin defers to the next.
pub(crate) fn enabled(root: &Path, claude_dir: Option<&Path>) -> bool {
    let scopes = [
        Some(root.join(SETTINGS_LOCAL_REL)),
        Some(root.join(SETTINGS_REL)),
        claude_dir.map(|dir| dir.join("settings.json")),
    ];
    scopes
        .into_iter()
        .flatten()
        .find_map(|path| {
            read_json(&path)?
                .get("enabledPlugins")?
                .get(PLUGIN_ID)?
                .as_bool()
        })
        .unwrap_or(false)
}

/// The `extraKnownMarketplaces` entry the enrolment writes.
fn marketplace_entry() -> Value {
    json!({ "source": { "source": "github", "repo": MARKETPLACE_REPO } })
}

/// Every `(container, key, value)` the enrolment writes — the one list both
/// [`enrol`] and [`unenrol`] walk, so an entry added here is removed by an
/// uninstall too.
fn enrolment() -> [(&'static str, &'static str, Value); 2] {
    [
        ("extraKnownMarketplaces", MARKETPLACE, marketplace_entry()),
        ("enabledPlugins", PLUGIN_ID, Value::Bool(true)),
    ]
}

/// Enrol the plugin in the committed `.claude/settings.json` ([FR-IN-12]):
/// `extraKnownMarketplaces.logos` and `enabledPlugins."logos@logos": true`,
/// so the host prompts every teammate to install it.
///
/// Non-clobbering, like the `.mcp.json` injection: a key already present
/// keeps its value (a teammate's directory-source marketplace is theirs), the
/// rest of the file is preserved, and a file that is not a JSON object, or
/// holds either container as a non-object, is `Skipped` with the reason.
///
/// # Errors
/// Only on an I/O failure writing the settings file.
///
/// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
pub(crate) fn enrol(root: &Path) -> Result<InitStep> {
    let path = root.join(SETTINGS_REL);
    let existed = path.exists();
    let mut settings = match settings_object(&path)? {
        Ok(settings) => settings,
        Err(reason) => return Ok(step(SETTINGS_REL, InitAction::Skipped, reason)),
    };
    let mut added = Vec::new();
    for (container, key, value) in enrolment() {
        let Some(map) = settings
            .entry(container)
            .or_insert_with(|| json!({}))
            .as_object_mut()
        else {
            return Ok(step(
                SETTINGS_REL,
                InitAction::Skipped,
                format!("existing {SETTINGS_REL} `{container}` is not an object — left untouched"),
            ));
        };
        if !map.contains_key(key) {
            map.insert(key.to_string(), value);
            added.push(format!("{container}.{key}"));
        }
    }
    // A present `false` is the team's decision and is kept; the row says so
    // rather than claiming an enrolment the file does not hold.
    let disabled = settings
        .get("enabledPlugins")
        .and_then(|plugins| plugins.get(PLUGIN_ID))
        .is_some_and(|value| *value != Value::Bool(true));
    let note = if disabled {
        format!("; enabledPlugins.{PLUGIN_ID} is not `true` here — left as is")
    } else {
        String::new()
    };
    if added.is_empty() {
        let detail = if disabled {
            note.trim_start_matches("; ").to_string()
        } else {
            format!("the {PLUGIN_ID} plugin is already enrolled")
        };
        return Ok(step(SETTINGS_REL, InitAction::Unchanged, detail));
    }
    write_settings(root, &settings)?;
    let what = if disabled { "entries added" } else { "enrolled" };
    Ok(step(
        SETTINGS_REL,
        if existed { InitAction::Updated } else { InitAction::Created },
        format!("Claude Code plugin {PLUGIN_ID} {what} ({}){note}", added.join(", ")),
    ))
}

/// Do the committed project settings enable the plugin — `enabledPlugins`
/// holding `true` for [`PLUGIN_ID`] in `.claude/settings.json`? What
/// [`enrol`] is meant to leave behind, and the condition for removing the
/// committed legacy registrations it replaces.
pub(crate) fn committed(root: &Path) -> bool {
    read_json(&root.join(SETTINGS_REL))
        .and_then(|settings| settings.get("enabledPlugins")?.get(PLUGIN_ID)?.as_bool())
        .unwrap_or(false)
}

/// The row for a legacy registration left in place because the committed
/// settings do not enable the plugin ([`committed`]).
pub(crate) fn legacy_kept(target: &str) -> InitStep {
    step(
        target,
        InitAction::Skipped,
        format!(
            "the committed {SETTINGS_REL} does not enable {PLUGIN_ID}, so teammates would lose \
             this registration — left in place"
        ),
    )
}

/// Remove the enrolment [`enrol`] wrote, for `init --uninstall`: each entry
/// only while it still holds the value `enrol` writes, so a teammate's own
/// marketplace source or an explicit `false` survives. A container this
/// emptied goes with it, and a settings file this emptied is deleted.
///
/// # Errors
/// Only on an I/O failure writing or removing the settings file.
pub(crate) fn unenrol(root: &Path) -> Result<InitStep> {
    let path = root.join(SETTINGS_REL);
    if !path.exists() {
        return Ok(step(SETTINGS_REL, InitAction::Unchanged, "no plugin enrolment"));
    }
    let mut settings = match settings_object(&path)? {
        Ok(settings) => settings,
        Err(reason) => return Ok(step(SETTINGS_REL, InitAction::Skipped, reason)),
    };
    let mut removed = Vec::new();
    for (container, key, value) in enrolment() {
        let Some(map) = settings.get_mut(container).and_then(Value::as_object_mut) else {
            continue;
        };
        if map.get(key) == Some(&value) {
            map.shift_remove(key);
            removed.push(format!("{container}.{key}"));
            if map.is_empty() {
                settings.shift_remove(container);
            }
        }
    }
    if removed.is_empty() {
        return Ok(step(SETTINGS_REL, InitAction::Unchanged, "no plugin enrolment"));
    }
    let detail = format!("plugin enrolment removed ({})", removed.join(", "));
    if settings.is_empty() {
        fs::remove_file(&path)?;
        return Ok(step(SETTINGS_REL, InitAction::Removed, format!("{detail}; the file held nothing else")));
    }
    write_settings(root, &settings)?;
    Ok(step(SETTINGS_REL, InitAction::Updated, detail))
}

/// The plugin findings `doctor` reports ([FR-IN-12]); empty unless the plugin
/// is enabled for `root`.
///
/// - **Duplicate registration**: the project `.mcp.json` declares a `logos`
///   server too, so the host lists every tool twice (`mcp__logos__*` and
///   `mcp__plugin_logos_logos__*`).
/// - **Version skew**: `binary_version` is older than the `minBinaryVersion` a
///   plugin manifest declares. The manifests read are the installed copies the
///   host recorded for this project or user (`plugins/installed_plugins.json`
///   in `claude_dir`), and `plugins/logos/.claude-plugin/plugin.json` when
///   `root` is the repository that publishes the plugin. A manifest that
///   cannot be read, or a version that does not parse, yields no finding:
///   `doctor` does not manufacture one from a misread.
///
/// [FR-IN-12]: ../../../docs/specs/requirements/FR-IN-12.md
pub(crate) fn findings(root: &Path, claude_dir: Option<&Path>, binary_version: &str) -> Vec<String> {
    if !enabled(root, claude_dir) {
        return Vec::new();
    }
    let mut findings = Vec::new();
    let declared = read_json(&root.join(".mcp.json"))
        .is_some_and(|mcp| mcp.get("mcpServers").and_then(|s| s.get("logos")).is_some());
    if declared {
        findings.push(format!(
            "duplicate registration: a `logos` MCP server is declared both by .mcp.json and by \
             the enabled Claude Code plugin {PLUGIN_ID}, so every tool is listed twice \
             (mcp__logos__* and mcp__plugin_logos_logos__*); run `logos init -i` to remove the \
             legacy .mcp.json entry, or remove it by hand if it was edited (`init` removes only \
             the entry it wrote)"
        ));
    }
    for manifest in plugin_manifests(root, claude_dir) {
        let Some(minimum) = read_json(&manifest)
            .and_then(|m| m.get("metadata")?.get("minBinaryVersion")?.as_str().map(str::to_string))
        else {
            continue;
        };
        if older_than(binary_version, &minimum) {
            findings.push(format!(
                "version skew: the logos binary is {binary_version}, older than the {minimum} the \
                 Claude Code plugin needs (minBinaryVersion in {}); upgrade logos",
                manifest.display()
            ));
        }
    }
    findings
}

/// Every plugin manifest that applies to `root`: the installed copies the
/// host recorded at user scope or for this project, then the published tree
/// when `root` carries it. Each one that exists, once, as its canonical path —
/// an install of the published tree itself is one manifest, not two.
fn plugin_manifests(root: &Path, claude_dir: Option<&Path>) -> Vec<PathBuf> {
    let project = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let installed = claude_dir
        .and_then(|dir| read_json(&dir.join("plugins/installed_plugins.json")))
        .and_then(|registry| registry.get("plugins")?.get(PLUGIN_ID)?.as_array().cloned())
        .unwrap_or_default();
    let mut manifests: Vec<PathBuf> = installed
        .iter()
        .filter(|install| {
            install.get("scope").and_then(Value::as_str) == Some("user")
                || install
                    .get("projectPath")
                    .and_then(Value::as_str)
                    .is_some_and(|p| fs::canonicalize(p).is_ok_and(|p| p == project))
        })
        .filter_map(|install| install.get("installPath")?.as_str())
        .map(|install| Path::new(install).join(MANIFEST_REL))
        .collect();
    manifests.push(root.join(PUBLISHED_PLUGIN_REL).join(MANIFEST_REL));
    let mut seen = Vec::new();
    manifests
        .into_iter()
        .filter_map(|manifest| fs::canonicalize(manifest).ok())
        .filter(|manifest| {
            let first = !seen.contains(manifest);
            seen.push(manifest.clone());
            first
        })
        .collect()
}

/// Is `binary` older than `minimum`? A pre-release or build suffix is dropped
/// first (`1.20.0-dev` reads as 1.20.0, as the plugin's own handshake reads
/// it), and the comparison is [`crate::tools::version_at_least`]'s, the one
/// dotted-version comparison here. Both directions are asked because that
/// function answers `false` for a version that does not parse: "not at least
/// the minimum, yet the minimum is at least it" holds only when both parse.
fn older_than(binary: &str, minimum: &str) -> bool {
    let core = |version: &str| version.trim().split(['-', '+']).next().unwrap_or_default().to_string();
    let (binary, minimum) = (core(binary), core(minimum));
    !crate::tools::version_at_least(&binary, &minimum) && crate::tools::version_at_least(&minimum, &binary)
}

/// The JSON at `path`, or `None` when it is absent, unreadable or not JSON.
fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// The settings file as an object to edit: `{}` when absent or blank, the
/// object when it parses as one, and the reason it must be left alone
/// otherwise.
fn settings_object(path: &Path) -> Result<std::result::Result<Map<String, Value>, String>> {
    let text = match read_optional(path) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(Ok(Map::new())),
        // A shared file: unreadable is a reason to leave it, not a failure.
        Err(err) => return Ok(Err(format!("existing {SETTINGS_REL} could not be read ({err:#}) — left untouched"))),
    };
    Ok(crate::hooks::parse_settings(Some(&text)).map_err(|shape| shape.reason(SETTINGS_REL)))
}

fn write_settings(root: &Path, settings: &Map<String, Value>) -> Result<()> {
    let path = root.join(SETTINGS_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json(&path, &Value::Object(settings.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn enabled_json(value: bool) -> String {
        format!(r#"{{"enabledPlugins":{{"{PLUGIN_ID}":{value}}}}}"#)
    }

    #[test]
    fn the_claude_directory_is_the_override_else_home_dot_claude() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| OsString::from(v))
        };
        assert_eq!(
            claude_config_dir_from(env(&[(CLAUDE_CONFIG_DIR_ENV, "/c"), ("HOME", "/h")])),
            Some(PathBuf::from("/c"))
        );
        assert_eq!(claude_config_dir_from(env(&[("HOME", "/h")])), Some(PathBuf::from("/h/.claude")));
        // A relative override is skipped, as the XDG rules skip one.
        assert_eq!(
            claude_config_dir_from(env(&[(CLAUDE_CONFIG_DIR_ENV, "rel"), ("HOME", "/h")])),
            Some(PathBuf::from("/h/.claude"))
        );
        assert_eq!(claude_config_dir_from(env(&[])), None);
    }

    /// Each scope enables on its own, and the most specific scope that names
    /// the plugin decides — the host's precedence.
    #[test]
    fn enablement_follows_the_host_scope_precedence() {
        let project = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let (root, home) = (project.path(), user.path());
        assert!(!enabled(root, Some(home)), "nothing anywhere: not enabled");

        write(home, "settings.json", &enabled_json(true));
        assert!(enabled(root, Some(home)), "the user scope enables it");
        assert!(!enabled(root, None), "…and without a user directory it is not read");

        write(root, ".claude/settings.json", &enabled_json(false));
        assert!(!enabled(root, Some(home)), "the project's explicit false overrides the user");

        write(root, ".claude/settings.local.json", &enabled_json(true));
        assert!(enabled(root, Some(home)), "the local scope overrides the project");
    }

    /// Near misses: another plugin of the same marketplace, the same plugin of
    /// another marketplace, a non-boolean value and a broken file enable
    /// nothing, and the broken file defers to the next scope.
    #[test]
    fn only_the_exact_plugin_id_with_a_boolean_enables() {
        let project = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let root = project.path();
        for near_miss in [
            r#"{"enabledPlugins":{"logos-review@logos":true}}"#,
            r#"{"enabledPlugins":{"logos@other":true}}"#,
            r#"{"enabledPlugins":{"logos@logos":"true"}}"#,
            r#"{"enabledPlugins":["logos@logos"]}"#,
        ] {
            write(root, ".claude/settings.json", near_miss);
            assert!(!enabled(root, None), "{near_miss} must not enable the plugin");
        }
        write(root, ".claude/settings.json", "{ not json");
        write(user.path(), "settings.json", &enabled_json(true));
        assert!(enabled(root, Some(user.path())), "a broken project file defers to the user scope");
    }

    #[test]
    fn versions_compare_numerically_and_drop_suffixes() {
        assert!(older_than("1.3.0", "1.20.0"), "numeric, not lexical");
        assert!(!older_than("1.100.0", "1.20.0"));
        assert!(!older_than("1.20.0", "1.20.0"), "equal is not older");
        assert!(!older_than("1.20.0-dev", "1.20.0"), "a pre-release counts as its release");
        assert!(older_than("1.19.9+build.7", "1.20.0"));
        assert!(older_than("1", "1.0.1"), "a missing part is 0");
        for bad in ["", "v1.2.3", "1.x.0", "logos 1.2.3"] {
            assert!(!older_than(bad, "1.20.0") && !older_than("1.0.0", bad), "{bad:?} never yields a skew");
        }
    }

    /// Enrolment preserves foreign keys and a teammate's own marketplace
    /// source, is idempotent, and refuses a container it cannot extend.
    #[test]
    fn enrolment_is_non_clobbering_and_idempotent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let created = enrol(root).unwrap();
        assert_eq!(created.action, InitAction::Created);
        let settings = read_json(&root.join(SETTINGS_REL)).unwrap();
        assert_eq!(settings["enabledPlugins"][PLUGIN_ID], true);
        assert_eq!(settings["extraKnownMarketplaces"][MARKETPLACE], marketplace_entry());
        assert!(enabled(root, None), "what enrol writes is what detection reads");
        let again = enrol(root).unwrap();
        assert_eq!(again.action, InitAction::Unchanged);

        let theirs = r#"{"model":"x","extraKnownMarketplaces":{"logos":{"source":{"source":"directory","path":"/src/logos"}}}}"#;
        write(root, SETTINGS_REL, theirs);
        let merged = enrol(root).unwrap();
        assert_eq!(merged.action, InitAction::Updated);
        let settings = read_json(&root.join(SETTINGS_REL)).unwrap();
        assert_eq!(settings["model"], "x");
        assert_eq!(
            settings["extraKnownMarketplaces"]["logos"]["source"]["source"], "directory",
            "a present marketplace source is the teammate's"
        );

        // A file `init` cannot edit — a container of the wrong shape, a
        // non-object, not JSON at all — is left byte for byte, both ways.
        for foreign in [r#"{"enabledPlugins":[]}"#, "[1]", "{ not json"] {
            write(root, SETTINGS_REL, foreign);
            assert_eq!(enrol(root).unwrap().action, InitAction::Skipped, "{foreign}");
            assert_eq!(fs::read_to_string(root.join(SETTINGS_REL)).unwrap(), foreign);
            let left = unenrol(root).unwrap().action;
            assert!(matches!(left, InitAction::Skipped | InitAction::Unchanged), "{foreign}: {left:?}");
            assert_eq!(fs::read_to_string(root.join(SETTINGS_REL)).unwrap(), foreign);
        }
    }

    /// Unenrol removes exactly what enrol wrote: a file it created goes, a
    /// teammate's source and other plugins stay, and a second run is a no-op.
    #[test]
    fn unenrol_removes_only_what_enrol_wrote() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        enrol(root).unwrap();
        assert_eq!(unenrol(root).unwrap().action, InitAction::Removed);
        assert!(!root.join(SETTINGS_REL).exists(), "a file enrol created and nothing else uses is deleted");
        assert_eq!(unenrol(root).unwrap().action, InitAction::Unchanged);

        let theirs = r#"{"enabledPlugins":{"other@x":true},"extraKnownMarketplaces":{"logos":{"source":{"source":"directory","path":"/p"}}}}"#;
        write(root, SETTINGS_REL, theirs);
        enrol(root).unwrap();
        assert_eq!(unenrol(root).unwrap().action, InitAction::Updated);
        let left = read_json(&root.join(SETTINGS_REL)).unwrap();
        assert_eq!(left, serde_json::from_str::<Value>(theirs).unwrap(), "back to exactly theirs");

        // A container this emptied goes without reordering the user's keys.
        write(root, SETTINGS_REL, r#"{"a":1,"b":2,"c":3}"#);
        enrol(root).unwrap();
        write(root, SETTINGS_REL, r#"{"a":1,"enabledPlugins":{"logos@logos":true},"b":2,"c":3}"#);
        unenrol(root).unwrap();
        let keys: Vec<String> = read_json(&root.join(SETTINGS_REL)).unwrap().as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["a", "b", "c"]);
    }

    fn manifest(min: &str) -> String {
        format!(r#"{{"name":"logos","version":"9.9.9","metadata":{{"minBinaryVersion":"{min}"}}}}"#)
    }

    #[test]
    fn findings_are_empty_unless_the_plugin_is_enabled() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, ".mcp.json", r#"{"mcpServers":{"logos":{"command":"logos"}}}"#);
        write(root, "plugins/logos/.claude-plugin/plugin.json", &manifest("99.0.0"));
        assert!(findings(root, None, "1.0.0").is_empty(), "no plugin, no duplicate and no skew");
    }

    #[test]
    fn findings_name_the_duplicate_registration() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(root, SETTINGS_REL, &enabled_json(true));
        assert!(findings(root, None, "1.0.0").is_empty(), "the plugin alone is clean");
        write(root, ".mcp.json", r#"{"mcpServers":{"other":{"command":"x"}}}"#);
        assert!(findings(root, None, "1.0.0").is_empty(), "another server is not a duplicate");
        write(root, ".mcp.json", r#"{"mcpServers":{"logos":{"command":"logos"}}}"#);
        let found = findings(root, None, "1.0.0");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("duplicate registration"), "{found:?}");
        assert!(found[0].contains("mcp__plugin_logos_logos__"), "{found:?}");
        assert!(found[0].contains("by hand if it was edited"), "the remedy covers an entry init keeps: {found:?}");
    }

    /// The skew reads the published manifest and the installed copies the
    /// host recorded — the user-scope one and this project's, never another
    /// project's — and compares numerically.
    #[test]
    fn findings_name_a_version_skew_from_each_applicable_manifest() {
        let tmp = TempDir::new().unwrap();
        let other = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();
        let (root, home) = (tmp.path(), user.path());
        write(root, SETTINGS_REL, &enabled_json(true));

        write(root, "plugins/logos/.claude-plugin/plugin.json", &manifest("1.20.0"));
        assert!(findings(root, None, "1.20.0").is_empty(), "equal is not older");
        assert!(findings(root, None, "1.100.0").is_empty(), "1.100.0 is newer than 1.20.0");
        assert!(findings(root, None, "not-a-version").is_empty(), "an unreadable binary version is no finding");
        let skew = findings(root, None, "1.3.0");
        assert_eq!(skew.len(), 1, "{skew:?}");
        assert!(skew[0].starts_with("version skew") && skew[0].contains("1.3.0") && skew[0].contains("1.20.0"), "{skew:?}");

        let cache = |name: &str, min: &str| {
            let dir = home.join("plugins/cache").join(name);
            write(&dir, MANIFEST_REL, &manifest(min));
            dir.display().to_string()
        };
        let registry = json!({ "version": 2, "plugins": { PLUGIN_ID: [
            { "scope": "user", "installPath": cache("user", "2.0.0") },
            { "scope": "project", "projectPath": root.display().to_string(), "installPath": cache("mine", "3.0.0") },
            { "scope": "project", "projectPath": other.path().display().to_string(), "installPath": cache("theirs", "4.0.0") },
        ] } });
        write(home, "plugins/installed_plugins.json", &registry.to_string());
        let skew = findings(root, Some(home), "1.25.0");
        assert_eq!(skew.len(), 2, "the user and this project's copy, not another project's: {skew:?}");
        assert!(skew.iter().any(|f| f.contains("2.0.0")) && skew.iter().any(|f| f.contains("3.0.0")), "{skew:?}");
        assert!(!skew.iter().any(|f| f.contains("4.0.0")), "{skew:?}");

        // The user installed the published tree itself: one manifest, one line.
        let published = root.join("plugins/logos").display().to_string();
        let registry = json!({ "plugins": { PLUGIN_ID: [
            { "scope": "user", "installPath": published },
            { "scope": "user", "installPath": format!("{published}/.") },
        ] } });
        write(home, "plugins/installed_plugins.json", &registry.to_string());
        let skew = findings(root, Some(home), "1.0.0");
        assert_eq!(skew.len(), 1, "one manifest reached three ways is one finding: {skew:?}");

        write(root, "plugins/logos/.claude-plugin/plugin.json", r#"{"name":"logos"}"#);
        write(home, "plugins/installed_plugins.json", "{}");
        assert!(findings(root, Some(home), "0.1.0").is_empty(), "no declared minimum, no finding");
    }
}
