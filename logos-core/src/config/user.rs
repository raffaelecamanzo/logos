//! The **user-level** config: one file per user, never per repository
//! ([NFR-SE-08], [ADR-80], S-665).
//!
//! Everything in `.logos/config.toml` travels with a clone, so a setting that
//! chooses what Logos *runs* cannot live there. It lives here instead, in a file
//! no repository ships:
//!
//! - `[security_scan.<scanner>]` — the binary path and the extra flags for one
//!   registered scanner ([FR-SC-05]).
//!
//! # Location
//! [`user_config_dir`] resolves the directory, first match wins:
//!
//! 1. `$LOGOS_CONFIG_HOME` — the explicit override (tests point it at a temp dir);
//! 2. `$XDG_CONFIG_HOME/logos`;
//! 3. `$HOME/.config/logos`;
//! 4. `%APPDATA%\logos` (Windows).
//!
//! A variable that is unset, empty or relative is skipped, as the XDG base
//! directory specification asks. The file is [`USER_CONFIG_FILE`] in that
//! directory. The directory is the home for every later piece of user-level
//! state, such as the scan consent record (S-667): it is resolved once here so
//! each consumer adds a file or a table instead of a second lookup.
//!
//! The user-level **tools directory** ([`user_tools_dir`], S-743) is resolved
//! beside it by the same rules, from `$LOGOS_DATA_HOME`, `$XDG_DATA_HOME` and
//! `$HOME`.
//!
//! A missing directory or file is not a fault: it reads as no overrides. A file
//! that exists is parsed strictly (`deny_unknown_fields`, exit 2), like the
//! repository config.
//!
//! # Scan consent
//! The same directory holds [`SCAN_CONSENT_FILE`], the record of which scanners
//! the user has allowed to reach the network in which project ([FR-SC-07],
//! modelled on [NFR-SE-07]'s first-use consent). It is state Logos writes, not
//! config the user edits, so it is a sibling file rather than a table in
//! [`USER_CONFIG_FILE`]: recording a consent never rewrites the user's own
//! config. A project is keyed by its canonical root path, never by anything the
//! repository says about itself, so a repository cannot claim another
//! project's consent. Nothing is ever written inside the repository.
//!
//! # Install consent
//! Beside it, [`INSTALL_CONSENT_FILE`] records which tool versions the user
//! allowed Logos to download and install (S-744, [FR-SC-22]): per user and per
//! tool version, covering every project. It is a separate file because it
//! answers a separate question — the scan consent is per project and about the
//! scanners' own network use. The CodeQL licence acknowledgement is recorded
//! in it too, per tool version, with the install consent it came with (S-746).
//!
//! [FR-SC-22]: ../../../../docs/specs/requirements/FR-SC-22.md
//!
//! [NFR-SE-08]: ../../../../docs/specs/requirements/NFR-SE-08.md
//! [NFR-SE-07]: ../../../../docs/specs/requirements/NFR-SE-07.md
//! [FR-SC-05]: ../../../../docs/specs/requirements/FR-SC-05.md
//! [FR-SC-07]: ../../../../docs/specs/requirements/FR-SC-07.md
//! [ADR-80]: ../../../../docs/specs/architecture/decisions/ADR-80.md

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::ConfigError;
use super::settings::validate_scanner_names;

/// The variable that overrides the user-level config directory.
pub const USER_CONFIG_HOME_ENV: &str = "LOGOS_CONFIG_HOME";

/// The user-level config file's name inside [`user_config_dir`].
pub const USER_CONFIG_FILE: &str = "config.toml";

/// The user-level config directory, resolved from the process environment
/// (see the [module docs](self) for the order), or `None` when no candidate
/// variable is usable.
pub fn user_config_dir() -> Option<PathBuf> {
    user_config_dir_from(|name| std::env::var_os(name))
}

/// [`user_config_dir`] over an injected environment lookup, so the resolution
/// order is testable without mutating the process environment.
pub fn user_config_dir_from(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    // An unset, empty or relative value is skipped (XDG base directory spec).
    let absolute = |name: &str| {
        lookup(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    absolute(USER_CONFIG_HOME_ENV)
        .or_else(|| absolute("XDG_CONFIG_HOME").map(|p| p.join("logos")))
        .or_else(|| absolute("HOME").map(|p| p.join(".config").join("logos")))
        .or_else(|| absolute("APPDATA").map(|p| p.join("logos")))
}

/// The variable that overrides the user-level tools directory (S-743).
pub const USER_DATA_HOME_ENV: &str = "LOGOS_DATA_HOME";

/// The user-level **tools directory** the [tool installer](crate::tools) keeps
/// managed tools in ([FR-SC-21]), resolved from the process environment, or
/// `None` when no candidate variable is usable. First match wins:
///
/// 1. `$LOGOS_DATA_HOME` — the directory itself (tests point it at a temp dir);
/// 2. `$XDG_DATA_HOME/logos/tools`;
/// 3. `$HOME/.local/share/logos/tools`.
///
/// Resolved here, beside [`user_config_dir`] and by the same rules (an unset,
/// empty or relative value is skipped), because it is the same kind of state:
/// one per user, shared by every project, never inside a repository. There is
/// no Windows fallback: managed installs are not available there yet.
///
/// [FR-SC-21]: ../../../../docs/specs/requirements/FR-SC-21.md
pub fn user_tools_dir() -> Option<PathBuf> {
    user_tools_dir_from(|name| std::env::var_os(name))
}

/// [`user_tools_dir`] over an injected environment lookup.
pub fn user_tools_dir_from(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        lookup(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    absolute(USER_DATA_HOME_ENV)
        .or_else(|| absolute("XDG_DATA_HOME").map(|p| p.join("logos").join("tools")))
        .or_else(|| {
            absolute("HOME").map(|p| p.join(".local").join("share").join("logos").join("tools"))
        })
}

/// The parsed user-level config file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    /// Per-scanner binary-path and extra-flag overrides.
    #[serde(default)]
    pub security_scan: UserSecurityScan,
}

/// The user-level `[security_scan]` table: one `[security_scan.<scanner>]`
/// sub-table per scanner the user overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserSecurityScan(pub BTreeMap<String, ScannerOverride>);

/// One scanner's user-level overrides. Both keys are optional.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScannerOverride {
    /// The scanner binary to run instead of the first match on `PATH`. Must be
    /// absolute.
    pub path: Option<PathBuf>,
    /// Flags inserted at the template's fixed extra-flags position, each one
    /// argv element (no shell splitting).
    #[serde(default)]
    pub extra_args: Vec<String>,
}

impl UserSecurityScan {
    /// The overrides for `scanner`, if the user set any.
    pub fn get(&self, scanner: &str) -> Option<&ScannerOverride> {
        self.0.get(scanner)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        validate_scanner_names("security_scan", self.0.keys())?;
        for (name, scanner) in &self.0 {
            if let Some(path) = scanner.path.as_ref().filter(|p| !p.is_absolute()) {
                return Err(ConfigError::InvalidValue {
                    key: format!("security_scan.{name}.path"),
                    message: format!("{} must be an absolute path", path.display()),
                });
            }
            if scanner.extra_args.iter().any(String::is_empty) {
                return Err(ConfigError::InvalidValue {
                    key: format!("security_scan.{name}.extra_args"),
                    message: "an extra argument must not be empty".to_string(),
                });
            }
        }
        Ok(())
    }
}

/// Parse and validate a user-level config from already-read `text`; `path` is
/// for error attribution only.
///
/// # Errors
/// [`ConfigError::Parse`] on invalid TOML or an unknown key, or
/// [`ConfigError::InvalidValue`] for an unregistered scanner name, a relative
/// binary path or an empty extra argument.
pub fn parse_user_config(text: &str, path: &Path) -> Result<UserConfig, ConfigError> {
    let config: UserConfig = toml::from_str(text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    config.security_scan.validate()?;
    Ok(config)
}

/// Load the user-level config from `dir`'s [`USER_CONFIG_FILE`], or
/// [`UserConfig::default`] when `dir` is `None` or the file is absent.
///
/// # Errors
/// [`ConfigError::Io`] if the file exists but cannot be read, else as
/// [`parse_user_config`].
pub fn load_user_config(dir: Option<&Path>) -> Result<UserConfig, ConfigError> {
    let Some(path) = dir.map(|d| d.join(USER_CONFIG_FILE)) else {
        return Ok(UserConfig::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => parse_user_config(&text, &path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserConfig::default()),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// The scan-consent record's file name inside [`user_config_dir`].
pub const SCAN_CONSENT_FILE: &str = "security-consent.toml";

/// Every project's recorded scan consent ([FR-SC-07]): one `[project."<root>"]`
/// table per project, naming the scanners the user allowed to reach the
/// network there.
///
/// [FR-SC-07]: ../../../../docs/specs/requirements/FR-SC-07.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanConsent {
    /// Keyed by the project's canonical root path.
    #[serde(default)]
    project: BTreeMap<String, ProjectConsent>,
}

/// One project's consent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectConsent {
    scanners: BTreeSet<String>,
}

impl ScanConsent {
    /// The scanners consented to for the project rooted at `root`, in name
    /// order; empty when none were.
    pub fn consented(&self, root: &Path) -> BTreeSet<String> {
        self.project
            .get(&project_key(root))
            .map(|p| p.scanners.clone())
            .unwrap_or_default()
    }
}

/// The key a project's consent is filed under: its canonical root path, so two
/// spellings of one checkout share a record and no repository content can name
/// another project's.
fn project_key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Load the scan-consent record from `dir`, or an empty record when `dir` is
/// `None` or holds none.
///
/// # Errors
/// [`ConfigError::Io`] if the file exists but cannot be read, or
/// [`ConfigError::Parse`] if it is not a record Logos wrote.
pub fn load_scan_consent(dir: Option<&Path>) -> Result<ScanConsent, ConfigError> {
    let Some(path) = dir.map(|d| d.join(SCAN_CONSENT_FILE)) else {
        return Ok(ScanConsent::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse { path, source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ScanConsent::default()),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// Add `scanners` to the consent recorded for the project at `root`, in the
/// record under `dir` (created if absent), and return the project's whole
/// consented set. The file is replaced atomically through
/// [`publish`](super::atomic::publish).
///
/// # Errors
/// [`ConfigError::InvalidValue`] when there is no user-level directory to
/// record in, else as [`load_scan_consent`], or [`ConfigError::Write`] when
/// the record cannot be written.
pub fn record_scan_consent<S: AsRef<str>>(
    dir: Option<&Path>,
    root: &Path,
    scanners: &[S],
) -> Result<BTreeSet<String>, ConfigError> {
    let Some(dir) = dir else {
        return Err(ConfigError::InvalidValue {
            key: "consent".to_string(),
            message: format!(
                "there is no user-level directory to record it in; set ${USER_CONFIG_HOME_ENV} \
                 or $HOME"
            ),
        });
    };
    let mut record = load_scan_consent(Some(dir))?;
    let entry = record.project.entry(project_key(root)).or_default();
    entry
        .scanners
        .extend(scanners.iter().map(|s| s.as_ref().to_string()));
    let consented = entry.scanners.clone();
    let path = dir.join(SCAN_CONSENT_FILE);
    let text = toml::to_string(&record).expect("a consent record always serialises");
    fs::create_dir_all(dir)
        .and_then(|()| super::atomic::publish(&path, text.as_bytes(), None))
        .map_err(|source| ConfigError::Write { path, source })?;
    Ok(consented)
}

/// The install-consent record's file name inside [`user_config_dir`], beside
/// [`SCAN_CONSENT_FILE`].
pub const INSTALL_CONSENT_FILE: &str = "security-install-consent.toml";

/// The user's recorded **install consent** ([FR-SC-22], S-744): per tool, the
/// versions the user allowed Logos to download, install and run. One
/// `[tool]` table, `<tool> = ["<version>", …]`, and — once a licence has been
/// acknowledged ([FR-SC-23], S-746) — a `[licence]` table of the same shape:
/// the versions of each tool whose licence terms the user accepted.
///
/// Per user and per tool version, never per project: a consent covers that
/// version in every project, and a newly pinned version is asked for again.
/// Like [`ScanConsent`] it is state Logos writes, kept out of
/// [`USER_CONFIG_FILE`] and out of every repository.
///
/// [FR-SC-22]: ../../../../docs/specs/requirements/FR-SC-22.md
/// [FR-SC-23]: ../../../../docs/specs/requirements/FR-SC-23.md
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallConsent {
    #[serde(default)]
    tool: BTreeMap<String, BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    licence: BTreeMap<String, BTreeSet<String>>,
}

impl InstallConsent {
    /// Whether the user consented to installing `version` of `tool`.
    pub fn covers(&self, tool: &str, version: &str) -> bool {
        self.tool.get(tool).is_some_and(|v| v.contains(version))
    }

    /// Whether the user accepted the licence of `version` of `tool`.
    pub fn licence_accepted(&self, tool: &str, version: &str) -> bool {
        self.licence.get(tool).is_some_and(|v| v.contains(version))
    }

    /// The versions of `tool` whose licence the user accepted, in version
    /// string order.
    pub fn licences(&self, tool: &str) -> Vec<String> {
        self.licence
            .get(tool)
            .map(|v| v.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Load the install-consent record from `dir`, or an empty record when `dir`
/// is `None` or holds none.
///
/// # Errors
/// [`ConfigError::Io`] if the file exists but cannot be read, or
/// [`ConfigError::Parse`] if it is not a record Logos wrote.
pub fn load_install_consent(dir: Option<&Path>) -> Result<InstallConsent, ConfigError> {
    let Some(path) = dir.map(|d| d.join(INSTALL_CONSENT_FILE)) else {
        return Ok(InstallConsent::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse { path, source }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(InstallConsent::default()),
        Err(source) => Err(ConfigError::Io { path, source }),
    }
}

/// Add each `(tool, version)` of `lines` to the install consent under `dir`
/// (created if absent), and each of `licences` to its accepted licences, in
/// one write. The file is replaced atomically through
/// [`publish`](super::atomic::publish), as [`record_scan_consent`] does.
///
/// # Errors
/// [`ConfigError::InvalidValue`] when there is no user-level directory to
/// record in, else as [`load_install_consent`], or [`ConfigError::Write`] when
/// the record cannot be written.
pub fn record_install_consent<T: AsRef<str>, V: AsRef<str>>(
    dir: Option<&Path>,
    lines: &[(T, V)],
    licences: &[(&str, &str)],
) -> Result<InstallConsent, ConfigError> {
    let Some(dir) = dir else {
        return Err(ConfigError::InvalidValue {
            key: "consent".to_string(),
            message: format!(
                "there is no user-level directory to record the install consent in; set \
                 ${USER_CONFIG_HOME_ENV} or $HOME"
            ),
        });
    };
    let mut record = load_install_consent(Some(dir))?;
    for (tool, version) in lines {
        record
            .tool
            .entry(tool.as_ref().to_string())
            .or_default()
            .insert(version.as_ref().to_string());
    }
    for (tool, version) in licences {
        record
            .licence
            .entry((*tool).to_string())
            .or_default()
            .insert((*version).to_string());
    }
    let path = dir.join(INSTALL_CONSENT_FILE);
    let text = toml::to_string(&record).expect("a consent record always serialises");
    fs::create_dir_all(dir)
        .and_then(|()| super::atomic::publish(&path, text.as_bytes(), None))
        .map_err(|source| ConfigError::Write { path, source })?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_consent_is_per_tool_version_and_accumulates_beside_the_scan_consent() {
        let user = tempfile::TempDir::new().unwrap();
        assert!(!load_install_consent(Some(user.path())).unwrap().covers("trivy", "0.74.0"));

        record_install_consent(Some(user.path()), &[("trivy", "0.74.0")], &[]).unwrap();
        let record = record_install_consent(
            Some(user.path()),
            &[("trivy", "0.75.0"), ("gosec", "2.21.4")],
            &[],
        )
        .unwrap();
        assert_eq!(record, load_install_consent(Some(user.path())).unwrap());
        assert!(record.covers("trivy", "0.74.0"), "a later consent adds to the earlier one");
        assert!(record.covers("trivy", "0.75.0") && record.covers("gosec", "2.21.4"));
        assert!(!record.covers("gosec", "2.22.0"), "a consent names one version");
        assert!(!record.covers("gitleaks", "8.18.4"));
        // Its own file, beside the scan consent's, and no temporary left behind.
        let files: Vec<_> = fs::read_dir(user.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(files, [INSTALL_CONSENT_FILE]);
        assert!(load_scan_consent(Some(user.path())).unwrap().consented(user.path()).is_empty());
        assert!(matches!(
            record_install_consent(None, &[("trivy", "0.74.0")], &[]),
            Err(ConfigError::InvalidValue { .. })
        ));
        fs::write(user.path().join(INSTALL_CONSENT_FILE), "all = true\n").unwrap();
        assert!(matches!(
            load_install_consent(Some(user.path())),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn a_licence_acknowledgement_is_recorded_per_version_with_the_install_consent() {
        let user = tempfile::TempDir::new().unwrap();
        // A record without one writes no `[licence]` table.
        record_install_consent(Some(user.path()), &[("trivy", "0.74.0")], &[]).unwrap();
        let text = fs::read_to_string(user.path().join(INSTALL_CONSENT_FILE)).unwrap();
        assert!(!text.contains("licence"), "{text}");

        let record =
            record_install_consent(Some(user.path()), &[("codeql", "2.27.2")], &[("codeql", "2.27.2")])
                .unwrap();
        assert_eq!(record, load_install_consent(Some(user.path())).unwrap());
        assert!(record.covers("codeql", "2.27.2") && record.covers("trivy", "0.74.0"));
        assert!(record.licence_accepted("codeql", "2.27.2"));
        assert!(!record.licence_accepted("codeql", "2.28.0"), "an acknowledgement names one version");
        assert!(!record.licence_accepted("trivy", "0.74.0"), "an install consent is not one");
        assert_eq!(record.licences("codeql"), ["2.27.2"]);
        assert!(record.licences("trivy").is_empty());
        let text = fs::read_to_string(user.path().join(INSTALL_CONSENT_FILE)).unwrap();
        assert!(text.contains("[licence]"), "{text}");
    }

    #[test]
    fn consent_is_recorded_per_project_outside_the_repository_and_accumulates() {
        let user = tempfile::TempDir::new().unwrap();
        let (a, b) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());
        let none: [&str; 0] = [];
        assert!(load_scan_consent(Some(user.path())).unwrap().consented(a.path()).is_empty());

        let first = record_scan_consent(Some(user.path()), a.path(), &["semgrep"]).unwrap();
        assert_eq!(first, BTreeSet::from(["semgrep".to_string()]));
        let second = record_scan_consent(Some(user.path()), a.path(), &["trivy"]).unwrap();
        assert_eq!(second.len(), 2, "a later consent adds to the earlier one: {second:?}");

        let record = load_scan_consent(Some(user.path())).unwrap();
        assert_eq!(record.consented(a.path()), second);
        // Another spelling of the same checkout reads the same consent.
        let link = user.path().join("project-link");
        std::os::unix::fs::symlink(a.path(), &link).unwrap();
        assert_eq!(record.consented(&link), second, "keyed by the canonical root");
        fs::remove_file(&link).unwrap();
        assert!(record.consented(b.path()).is_empty(), "consent is per project");
        // Nothing lands in either project, and no temporary file is left behind.
        for project in [a.path(), b.path()] {
            assert_eq!(fs::read_dir(project).unwrap().count(), 0);
        }
        let files: Vec<_> = fs::read_dir(user.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(files, [SCAN_CONSENT_FILE]);
        assert!(matches!(
            record_scan_consent(None, a.path(), &none),
            Err(ConfigError::InvalidValue { .. })
        ));
    }

    #[test]
    fn a_consent_record_logos_did_not_write_is_rejected() {
        let user = tempfile::TempDir::new().unwrap();
        fs::write(user.path().join(SCAN_CONSENT_FILE), "[project.\"/x\"]\nscanners = [\"semgrep\"]\nall = true\n").unwrap();
        assert!(matches!(
            load_scan_consent(Some(user.path())),
            Err(ConfigError::Parse { .. })
        ));
    }

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_directory_resolves_override_then_xdg_then_home_skipping_relative_values() {
        let all = [
            ("LOGOS_CONFIG_HOME", "/o"),
            ("XDG_CONFIG_HOME", "/x"),
            ("HOME", "/h"),
            ("APPDATA", "/a"),
        ];
        assert_eq!(user_config_dir_from(env(&all)), Some(PathBuf::from("/o")));
        assert_eq!(
            user_config_dir_from(env(&all[1..])),
            Some(PathBuf::from("/x/logos"))
        );
        assert_eq!(
            user_config_dir_from(env(&all[2..])),
            Some(PathBuf::from("/h/.config/logos"))
        );
        assert_eq!(
            user_config_dir_from(env(&all[3..])),
            Some(PathBuf::from("/a/logos"))
        );
        assert_eq!(user_config_dir_from(env(&[])), None);
        // Empty and relative values are skipped, never joined onto the cwd.
        let skipped = [
            ("LOGOS_CONFIG_HOME", ""),
            ("XDG_CONFIG_HOME", "relative/dir"),
            ("HOME", "/h"),
        ];
        assert_eq!(
            user_config_dir_from(env(&skipped)),
            Some(PathBuf::from("/h/.config/logos"))
        );
    }

    #[test]
    fn the_tools_directory_resolves_override_then_xdg_data_then_home() {
        let all = [
            ("LOGOS_DATA_HOME", "/d"),
            ("XDG_DATA_HOME", "/x"),
            ("HOME", "/h"),
            ("APPDATA", "/a"),
        ];
        assert_eq!(user_tools_dir_from(env(&all)), Some(PathBuf::from("/d")));
        assert_eq!(
            user_tools_dir_from(env(&all[1..])),
            Some(PathBuf::from("/x/logos/tools"))
        );
        assert_eq!(
            user_tools_dir_from(env(&all[2..])),
            Some(PathBuf::from("/h/.local/share/logos/tools"))
        );
        // No Windows fallback, and the config overrides are not consulted.
        assert_eq!(user_tools_dir_from(env(&all[3..])), None);
        assert_eq!(
            user_tools_dir_from(env(&[("LOGOS_CONFIG_HOME", "/o"), ("XDG_CONFIG_HOME", "/c")])),
            None
        );
        let skipped = [
            ("LOGOS_DATA_HOME", ""),
            ("XDG_DATA_HOME", "relative/dir"),
            ("HOME", "/h"),
        ];
        assert_eq!(
            user_tools_dir_from(env(&skipped)),
            Some(PathBuf::from("/h/.local/share/logos/tools"))
        );
    }

    #[test]
    fn a_missing_directory_or_file_reads_as_no_overrides() {
        assert_eq!(load_user_config(None).unwrap(), UserConfig::default());
        let tmp = tempfile::TempDir::new().unwrap();
        assert_eq!(
            load_user_config(Some(tmp.path())).unwrap(),
            UserConfig::default()
        );
    }
}
