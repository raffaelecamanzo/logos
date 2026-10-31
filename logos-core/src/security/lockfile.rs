//! The dependency scope of a lockfile finding ([FR-SC-30], [ADR-79], S-810):
//! `dev` when the lockfile itself marks the finding's package dev-only, and
//! otherwise `production`. Computed each time a finding is read, from the
//! lockfile as it stands, and never stored.
//!
//! The lockfile is the only evidence. A package is dev-only when
//!
//! - **`package-lock.json` / `npm-shrinkwrap.json`** — every entry of that
//!   name and version carries `"dev": true` (`packages` of lockfile v2/v3, or
//!   the nested `dependencies` of v1). `devOptional` is not `dev`.
//! - **`pnpm-lock.yaml`** — every `packages` entry of that name and version
//!   carries `dev: true`.
//!
//! Everything else reads `production`: another lockfile or ecosystem, a
//! finding whose package its message does not name, a package the lockfile does
//! not hold (or holds once without the flag, as a hoisted copy that something
//! shipped depends on), an unreadable or unparseable lockfile. The scope
//! never guesses.
//!
//! The package comes from the finding's message, the one place both lockfile
//! scanners put it: OSV-Scanner's ``Package 'name@version' is vulnerable …``
//! and Trivy's `Package: name` beside `Installed Version: version`. A finding
//! reads its lockfile from the project root; a path outside it, a `..`
//! segment, a symbolic link (the file itself, or a directory on the way to it
//! that leads out of the root) or a lockfile over [`MAX_LOCKFILE_BYTES`] is
//! `production`.
//!
//! [FR-SC-30]: ../../../docs/specs/requirements/FR-SC-30.md
//! [ADR-79]: ../../../docs/specs/architecture/decisions/ADR-79.md

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path};

use crate::history::SecurityFinding;

/// The largest lockfile read: a bigger one is not parsed and reads `production`.
pub const MAX_LOCKFILE_BYTES: u64 = 64 * 1024 * 1024;

/// Which lockfile format a project-relative path names, by its file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// `package-lock.json`, `npm-shrinkwrap.json`.
    Npm,
    /// `pnpm-lock.yaml`.
    Pnpm,
}

impl Format {
    fn of(path: &str) -> Option<Self> {
        match path.rsplit('/').next()? {
            "package-lock.json" | "npm-shrinkwrap.json" => Some(Self::Npm),
            "pnpm-lock.yaml" => Some(Self::Pnpm),
            _ => None,
        }
    }
}

/// Whether the packages of one lockfile are dev-only: per `(name, version)`,
/// `true` only when every entry the lockfile holds for it is marked `dev`.
#[derive(Debug, Default)]
struct DevPackages(BTreeMap<(String, String), bool>);

impl DevPackages {
    fn record(&mut self, name: &str, version: &str, dev: bool) {
        self.0
            .entry((name.to_string(), version.to_string()))
            .and_modify(|all_dev| *all_dev &= dev)
            .or_insert(dev);
    }

    fn is_dev(&self, name: &str, version: &str) -> bool {
        self.0.get(&(name.to_string(), version.to_string())).copied().unwrap_or(false)
    }
}

/// The lockfiles read so far in one classification, each at most once: a
/// project reports hundreds of advisories against one `package-lock.json`.
#[derive(Debug, Default)]
pub struct LockfileCache(HashMap<String, Option<DevPackages>>);

impl LockfileCache {
    /// Whether `finding`, located at the project-relative `location`, is on a
    /// package the lockfile at `root`/`location` marks dev-only.
    pub fn is_dev(&mut self, root: &Path, location: &str, finding: &SecurityFinding) -> bool {
        let Some(format) = Format::of(location) else { return false };
        // The one reading of both message forms, which the ingest keys a
        // dependency finding by (S-808).
        let Some(package) = crate::history::security::message_package(&finding.message) else { return false };
        self.0
            .entry(location.to_string())
            .or_insert_with(|| read(root, location, format))
            .as_ref()
            .is_some_and(|packages| packages.is_dev(&package.name, &package.version))
    }
}

/// The packages of the lockfile at `root`/`location`, or `None` for one that
/// cannot be safely read or parsed.
fn read(root: &Path, location: &str, format: Format) -> Option<DevPackages> {
    let relative = Path::new(location);
    if relative.is_absolute() || relative.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    let path = root.join(relative);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_LOCKFILE_BYTES {
        return None;
    }
    // `symlink_metadata` judged the last component alone: a linked directory on
    // the way must not lead out of the project either.
    if !path.canonicalize().ok()?.starts_with(root.canonicalize().ok()?) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    match format {
        Format::Npm => parse_npm(&text),
        Format::Pnpm => Some(parse_pnpm(&text)),
    }
}

/// Whether an entry's `dev` field is the boolean `true`. A field of any other type
/// (a string `"dev"`) reads as not dev instead of failing the whole file.
fn is_dev(flag: &Option<serde_json::Value>) -> bool {
    flag.as_ref() == Some(&serde_json::Value::Bool(true))
}

/// A lockfile v2/v3 `packages` entry, with the three fields that are read.
/// Parsing into this keeps memory proportional to the entries, where a
/// `serde_json::Value` tree of a lockfile held about twenty-five times its size:
/// every other field (`resolved`, `integrity`, the entry's `dependencies`, which
/// are version ranges here) is skipped unbuilt. Each read field is a `Value` so
/// an unexpected type reads as absent rather than failing the file.
#[derive(serde::Deserialize)]
struct PackageEntry {
    name: Option<serde_json::Value>,
    version: Option<serde_json::Value>,
    dev: Option<serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct PackagesLock {
    /// Keyed by install path.
    packages: Option<std::collections::BTreeMap<String, PackageEntry>>,
}

/// A lockfile v1 entry: the same fields, and the entries installed beneath it.
#[derive(serde::Deserialize)]
struct NestedEntry {
    version: Option<serde_json::Value>,
    dev: Option<serde_json::Value>,
    dependencies: Option<std::collections::BTreeMap<String, NestedEntry>>,
}

#[derive(serde::Deserialize)]
struct NestedLock {
    dependencies: Option<std::collections::BTreeMap<String, NestedEntry>>,
}

/// An npm lockfile: v2/v3 `packages` (keyed by install path) or, when there is
/// none, v1's nested `dependencies` (a second pass, which only a v1 file pays).
fn parse_npm(text: &str) -> Option<DevPackages> {
    let lock: PackagesLock = serde_json::from_str(text).ok()?;
    let mut out = DevPackages::default();
    if let Some(packages) = &lock.packages {
        for (key, entry) in packages {
            // The root project is the empty key; a linked workspace has no
            // `node_modules/` in its key and no claim to make.
            let Some(installed) = key.rsplit_once("node_modules/").map(|(_, name)| name) else { continue };
            let name = entry.name.as_ref().and_then(|n| n.as_str()).unwrap_or(installed);
            if let Some(version) = entry.version.as_ref().and_then(|v| v.as_str()) {
                out.record(name, version, is_dev(&entry.dev));
            }
        }
    } else if let Some(dependencies) = serde_json::from_str::<NestedLock>(text).ok()?.dependencies {
        walk_v1(&dependencies, &mut out);
    }
    Some(out)
}

fn walk_v1(dependencies: &std::collections::BTreeMap<String, NestedEntry>, out: &mut DevPackages) {
    for (name, entry) in dependencies {
        if let Some(version) = entry.version.as_ref().and_then(|v| v.as_str()) {
            out.record(name, version, is_dev(&entry.dev));
        }
        if let Some(nested) = &entry.dependencies {
            walk_v1(nested, out);
        }
    }
}

/// A pnpm lockfile's `packages:` section, line by line: the file is generated,
/// its entries sit at a fixed indent (`  key:` then `    field: value`), so no
/// YAML parser is needed for the one flag read.
fn parse_pnpm(text: &str) -> DevPackages {
    let mut out = DevPackages::default();
    let mut in_packages = false;
    let mut current: Option<(String, String)> = None;
    let mut dev = false;
    let mut flush = |current: &mut Option<(String, String)>, dev: &mut bool| {
        if let Some((name, version)) = current.take() {
            out.record(&name, &version, *dev);
        }
        *dev = false;
    };
    for line in text.lines() {
        if !line.starts_with(' ') && !line.is_empty() && !line.starts_with('#') {
            // A top-level key: the section either starts or ends here.
            flush(&mut current, &mut dev);
            in_packages = line.trim_end() == "packages:";
            continue;
        }
        if !in_packages {
            continue;
        }
        if let Some(key) = line.strip_prefix("  ").filter(|k| !k.starts_with(' ')) {
            flush(&mut current, &mut dev);
            current = pnpm_key(key);
        } else if line.trim_end() == "    dev: true" {
            dev = true;
        }
    }
    flush(&mut current, &mut dev);
    out
}

/// `(name, version)` of a pnpm `packages` key, in the three spellings of
/// lockfile v5 (`/name/1.0.0_peer`), v6 (`/name@1.0.0(peer)`) and v9
/// (`name@1.0.0`); `None` for a line that is not an entry key.
fn pnpm_key(line: &str) -> Option<(String, String)> {
    let key = line.trim_end().strip_suffix(':')?.trim_matches(|c| c == '\'' || c == '"');
    let key = key.strip_prefix('/').unwrap_or(key);
    // A peer suffix: v6 `(…)`, v5 `_…` after the version.
    let key = key.split('(').next()?;
    // v5 puts a version after the last `/`, its peer suffix after a `_` — and
    // that suffix may hold an `@`. A v6/v9 tail is a name: `name@version`.
    if let Some((name, tail)) = key.rsplit_once('/') {
        let version = tail.split('_').next()?;
        if version.starts_with(|c: char| c.is_ascii_digit()) && version.contains('.') && !version.contains('@') {
            return (!name.is_empty()).then(|| (name.to_string(), version.to_string()));
        }
    }
    let at = key.rfind('@').filter(|&at| at > 0)?;
    Some((key[..at].to_string(), key[at + 1..].to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::security::FindingBinding;
    use crate::history::{FRESHNESS_FRESH, ScannerConfidence, Severity};

    fn finding(message: &str, report_path: &str) -> SecurityFinding {
        SecurityFinding {
            scanner_name: "osv-scanner".into(),
            scanner_version: "2.7.0".into(),
            head_sha: "0".repeat(40),
            fingerprint: "f".repeat(64),
            rule_id: "GHSA-xxxx".into(),
            severity: Severity::High,
            confidence: ScannerConfidence::Unstated,
            cwes: Vec::new(),
            message: message.into(),
            redacted: false,
            binding: FindingBinding::Unmatched,
            report_path: Some(report_path.into()),
            path: None,
            start_line: None,
            start_column: None,
            end_line: None,
            end_column: None,
            commits: Vec::new(),
            legacy_fingerprints: Vec::new(),
            anchor: None,
            freshness: FRESHNESS_FRESH,
            reporters: Vec::new(),
        }
    }

    fn osv(package: &str) -> String {
        format!("Package '{package}' is vulnerable to 'GHSA-xxxx' (also known as 'CVE-2024-1').")
    }

    const NPM_V3: &str = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": {"name": "app", "version": "1.0.0"},
        "node_modules/vite": {"version": "5.0.0", "dev": true},
        "node_modules/vitest": {"version": "1.0.0", "dev": true},
        "node_modules/vitest/node_modules/tinypool": {"version": "0.8.0", "dev": true},
        "node_modules/react": {"version": "18.2.0"},
        "node_modules/@scope/dev-tool": {"version": "2.0.0", "dev": true},
        "node_modules/@scope/shared": {"version": "3.0.0", "devOptional": true},
        "node_modules/shared": {"version": "1.0.0", "dev": true},
        "node_modules/a/node_modules/shared": {"version": "1.0.0"},
        "node_modules/aliased": {"name": "real-name", "version": "4.0.0", "dev": true}
      }
    }"#;

    fn npm_project(lock: &str) -> (tempfile::TempDir, LockfileCache) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package-lock.json"), lock).unwrap();
        (dir, LockfileCache::default())
    }

    fn dev_in(dir: &tempfile::TempDir, cache: &mut LockfileCache, location: &str, message: &str) -> bool {
        cache.is_dev(dir.path(), location, &finding(message, location))
    }

    /// The acceptance criterion: `"dev": true` reads dev, a package without the
    /// flag reads production — and every shape of name the lockfile spells.
    #[test]
    fn an_npm_package_marked_dev_reads_dev_and_one_without_the_flag_reads_production() {
        let (dir, mut cache) = npm_project(NPM_V3);
        let dev = |c: &mut LockfileCache, m: String| dev_in(&dir, c, "package-lock.json", &m);
        assert!(dev(&mut cache, osv("vite@5.0.0")), "a top-level dev package");
        assert!(dev(&mut cache, osv("tinypool@0.8.0")), "a nested dev package: named by the last node_modules/");
        assert!(dev(&mut cache, osv("@scope/dev-tool@2.0.0")), "a scoped name keeps its leading @");
        assert!(dev(&mut cache, osv("real-name@4.0.0")), "an aliased entry is its own `name`");
        assert!(!dev(&mut cache, osv("react@18.2.0")), "no flag is production");
        assert!(!dev(&mut cache, osv("@scope/shared@3.0.0")), "devOptional is not dev");
        assert!(!dev(&mut cache, osv("vite@4.0.0")), "another version of a dev package is not the dev one");
        assert!(!dev(&mut cache, osv("absent@1.0.0")), "a package the lockfile does not hold never guesses");
        assert!(!dev(&mut cache, osv("shared@1.0.0")), "one production copy of the version makes it production");
    }

    #[test]
    fn a_trivy_message_names_its_package_on_two_lines() {
        let (dir, mut cache) = npm_project(NPM_V3);
        let trivy = |name: &str, version: &str| {
            format!("Package: {name}\nInstalled Version: {version}\nVulnerability CVE-2024-1\nSeverity: HIGH")
        };
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &trivy("vitest", "1.0.0")));
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", &trivy("react", "18.2.0")));
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", "Package: vitest\nVulnerability CVE-2024-1"), "no version");
    }

    #[test]
    fn an_npm_v1_lockfile_reads_its_nested_dependencies() {
        let v1 = r#"{"lockfileVersion":1,"dependencies":{
            "jest":{"version":"29.0.0","dev":true,"dependencies":{"chalk":{"version":"4.1.2","dev":true}}},
            "lodash":{"version":"4.17.20"}}}"#;
        let (dir, mut cache) = npm_project(v1);
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("jest@29.0.0")));
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("chalk@4.1.2")), "nested");
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", &osv("lodash@4.17.20")));
    }

    /// An entry whose fields have an unexpected type reads as not dev and does
    /// not fail the file: a string `"dev"`, a numeric `version`, a `name` that is
    /// not text, and fields the reader never looks at.
    #[test]
    fn an_entry_of_an_unexpected_shape_does_not_fail_the_lockfile() {
        let odd = r#"{"lockfileVersion":3,"packages":{
            "node_modules/a":{"version":"1.0.0","dev":"true"},
            "node_modules/b":{"version":2,"dev":true},
            "node_modules/c":{"name":7,"version":"3.0.0","dev":true},
            "node_modules/d":{"version":"4.0.0","dev":true,"resolved":"https://x","integrity":"sha512-y","dependencies":{"e":"^1"},"engines":{"node":">=18"}}}}"#;
        let (dir, mut cache) = npm_project(odd);
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", &osv("a@1.0.0")), "a string dev is not dev");
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", &osv("b@2")), "a numeric version names nothing");
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("c@3.0.0")), "a non-text name falls back to the install path");
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("d@4.0.0")), "fields it never reads are skipped");
    }

    #[test]
    fn a_shrinkwrap_and_a_nested_lockfile_are_npm_lockfiles_too() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("web")).unwrap();
        std::fs::write(dir.path().join("web/npm-shrinkwrap.json"), NPM_V3).unwrap();
        let mut cache = LockfileCache::default();
        assert!(dev_in(&dir, &mut cache, "web/npm-shrinkwrap.json", &osv("vite@5.0.0")));
    }

    const PNPM_V6: &str = "\
lockfileVersion: '6.0'

dependencies:
  react:
    specifier: ^18.0.0
    version: 18.2.0

packages:

  /@babel/core@7.0.0:
    resolution: {integrity: sha512-aaa}
    dev: true

  /react@18.2.0:
    resolution: {integrity: sha512-bbb}
    dev: false

  '/@quoted/tool@2.0.0':
    resolution: {integrity: sha512-qqq}
    dev: true

  /vite@5.0.0(@types/node@20.0.0):
    resolution: {integrity: sha512-ccc}
    dev: true

snapshots:

  /late@1.0.0:
    dev: true
";

    const PNPM_V5: &str = "\
lockfileVersion: 5.4

packages:

  /@scope/tool/1.2.3_peer@1.0.0:
    resolution: {integrity: sha512-ddd}
    dev: true

  /plain/2.0.0:
    resolution: {integrity: sha512-eee}
    dev: false

  /3d_graph@4.0.0:
    resolution: {integrity: sha512-fff}
    dev: true
";

    #[test]
    fn a_pnpm_package_marked_dev_reads_dev_in_each_key_spelling() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pnpm-lock.yaml"), PNPM_V6).unwrap();
        let mut cache = LockfileCache::default();
        let mut dev = |m: String| dev_in(&dir, &mut cache, "pnpm-lock.yaml", &m);
        assert!(dev(osv("@babel/core@7.0.0")), "v6 scoped key");
        assert!(dev(osv("vite@5.0.0")), "v6 key with a peer suffix");
        assert!(!dev(osv("react@18.2.0")), "dev: false");
        assert!(dev(osv("@quoted/tool@2.0.0")), "a single-quoted key");
        assert!(!dev(osv("late@1.0.0")), "a `snapshots:` entry is not a package entry");

        std::fs::write(dir.path().join("pnpm-lock.yaml"), PNPM_V5).unwrap();
        let mut cache = LockfileCache::default();
        assert!(dev_in(&dir, &mut cache, "pnpm-lock.yaml", &osv("@scope/tool@1.2.3")), "v5 slash key with a peer suffix");
        assert!(!dev_in(&dir, &mut cache, "pnpm-lock.yaml", &osv("plain@2.0.0")));
        assert!(dev_in(&dir, &mut cache, "pnpm-lock.yaml", &osv("3d_graph@4.0.0")), "a name that starts with a digit and holds a `_` is no v5 version");
    }

    #[test]
    fn every_other_lockfile_and_every_unreadable_one_reads_production() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["Cargo.lock", "go.mod", "pom.xml", "yarn.lock", "requirements.txt"] {
            std::fs::write(dir.path().join(name), r#"{"packages":{"node_modules/vite":{"version":"5.0.0","dev":true}}}"#).unwrap();
            let mut cache = LockfileCache::default();
            assert!(!dev_in(&dir, &mut cache, name, &osv("vite@5.0.0")), "{name}");
        }
        // Not JSON, missing, a message with no package.
        std::fs::write(dir.path().join("package-lock.json"), "not json").unwrap();
        let mut cache = LockfileCache::default();
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", &osv("vite@5.0.0")), "unparseable");
        assert!(!dev_in(&dir, &mut cache, "sub/package-lock.json", &osv("vite@5.0.0")), "missing");
        let (dir, mut cache) = npm_project(NPM_V3);
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", "hardcoded secret in config"), "no package named");
        assert!(!dev_in(&dir, &mut cache, "package-lock.json", "Package 'vite5.0.0' is vulnerable to 'X'"), "no @version");
    }

    /// A location that steps out of the root, or a symbolic link, is never read.
    #[test]
    fn a_lockfile_outside_the_root_or_behind_a_symlink_reads_production() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(outer.path().join("package-lock.json"), NPM_V3).unwrap();
        let mut cache = LockfileCache::default();
        let probe = finding(&osv("vite@5.0.0"), "../package-lock.json");
        assert!(!cache.is_dev(&root, "../package-lock.json", &probe));
        assert!(!cache.is_dev(&root, "/etc/package-lock.json", &probe));
        std::os::unix::fs::symlink(outer.path().join("package-lock.json"), root.join("package-lock.json")).unwrap();
        assert!(!cache.is_dev(&root, "package-lock.json", &probe), "a symbolic link");
        // A linked directory on the way out of the root: the file itself is a regular file.
        let ext = outer.path().join("ext");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("package-lock.json"), NPM_V3).unwrap();
        std::os::unix::fs::symlink(&ext, root.join("web")).unwrap();
        let through = finding(&osv("vite@5.0.0"), "web/package-lock.json");
        assert!(!LockfileCache::default().is_dev(&root, "web/package-lock.json", &through), "a directory link out of the root");
        // One that stays inside the root is the project's own file.
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/package-lock.json"), NPM_V3).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).unwrap();
        let inside = finding(&osv("vite@5.0.0"), "alias/package-lock.json");
        assert!(LockfileCache::default().is_dev(&root, "alias/package-lock.json", &inside), "a directory link inside the root");
    }

    /// A lockfile over the cap is never read, whatever it says; one at the cap is.
    #[test]
    fn a_lockfile_over_the_size_cap_reads_production() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package-lock.json");
        let lock = r#"{"lockfileVersion":3,"packages":{"node_modules/vite":{"version":"5.0.0","dev":true}}}"#;
        // Trailing whitespace pads a valid document to the size under test.
        let mut text = lock.to_string();
        text.push_str(&" ".repeat(usize::try_from(MAX_LOCKFILE_BYTES).unwrap() - lock.len()));
        std::fs::write(&path, &text).unwrap();
        assert!(dev_in(&dir, &mut LockfileCache::default(), "package-lock.json", &osv("vite@5.0.0")), "at the cap");
        text.push(' ');
        std::fs::write(&path, &text).unwrap();
        assert!(!dev_in(&dir, &mut LockfileCache::default(), "package-lock.json", &osv("vite@5.0.0")), "one byte over");
    }

    /// One lockfile is parsed once however many findings name it.
    #[test]
    fn a_lockfile_is_read_once_per_classification() {
        let (dir, mut cache) = npm_project(NPM_V3);
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("vite@5.0.0")));
        std::fs::remove_file(dir.path().join("package-lock.json")).unwrap();
        assert!(dev_in(&dir, &mut cache, "package-lock.json", &osv("vitest@1.0.0")), "served from the cache");
        assert_eq!(cache.0.len(), 1);
    }
}
