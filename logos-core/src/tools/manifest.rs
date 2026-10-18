//! The compiled-in **manifest** ([FR-SC-21], [ADR-88] §2): per tool and per
//! platform, the pinned version, the source URL, the SHA-256 and the archive
//! layout. A Python scanner (Semgrep, Bandit, Checkov) has no artifact of its
//! own: it carries [hash-pinned requirements](PythonPackage) that the managed
//! [`uv`](Manifest::uv) installs into an environment of its own, on the
//! [managed Python](Manifest::python) (S-745, [FR-SC-23]). Dependency-Check
//! and CodeQL are [opt-in](OptIn): never part of the default set, installed
//! whole from a [tree](Layout::Tree) archive, Dependency-Check with the
//! [managed Java runtime](Manifest::java) and CodeQL only once its
//! [licence](CODEQL_LICENCE) is acknowledged (S-746).
//!
//! [`Manifest::builtin`] is what every shipped surface installs from. Its pins
//! travel with the Logos release: a newer upstream version is not used until a
//! release pins it, and an upstream asset that changes after pinning fails
//! verification. Each SHA-256 below was taken from the publisher's checksum
//! file for that release when the version was pinned, and the archive layout
//! from listing the macOS arm64 archive.
//!
//! The fields are public so that a test can build its own manifest — fixture
//! artifacts behind a stub `curl` — and hand it to
//! [`Installer::with_manifest`](super::Installer::with_manifest). Nothing reads
//! a manifest from a file or the environment.
//!
//! [FR-SC-21]: ../../../../docs/specs/requirements/FR-SC-21.md
//! [FR-SC-23]: ../../../../docs/specs/requirements/FR-SC-23.md
//! [ADR-88]: ../../../../docs/specs/architecture/decisions/ADR-88.md

use std::path::PathBuf;
use std::sync::OnceLock;

/// A platform the manifest can carry an artifact for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacosArm64,
    MacosX86_64,
    LinuxX86_64,
    LinuxArm64,
}

impl Platform {
    /// Every platform, in manifest order.
    pub const ALL: [Platform; 4] = [
        Platform::MacosArm64,
        Platform::MacosX86_64,
        Platform::LinuxX86_64,
        Platform::LinuxArm64,
    ];

    /// The platform this binary was built for, or `None` where managed installs
    /// are not available (Windows, until CR-220's work adds it).
    pub fn current() -> Option<Platform> {
        Self::for_target(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// The platform for a target's `std::env::consts::{OS, ARCH}` spelling.
    pub fn for_target(os: &str, arch: &str) -> Option<Platform> {
        match (os, arch) {
            ("macos", "aarch64") => Some(Platform::MacosArm64),
            ("macos", "x86_64") => Some(Platform::MacosX86_64),
            ("linux", "x86_64") => Some(Platform::LinuxX86_64),
            ("linux", "aarch64") => Some(Platform::LinuxArm64),
            _ => None,
        }
    }
}

/// How the tool's binary sits in its artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    /// A gzip-compressed tar archive; `member` is the binary's path inside it.
    TarGz { member: String },
    /// The artifact is the binary itself.
    Binary,
    /// An archive unpacked whole, for a tool that needs more than its binary
    /// (a JVM's libraries, CodeQL's query packs): the directory `root` inside
    /// it becomes the tool's `<tool>/<version>/` directory, and the tool's
    /// [`binary`](ToolEntry::binary) is a path inside that (S-746).
    Tree { archive: Archive, root: String },
}

/// The format of a [`Layout::Tree`] archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    /// Unpacked by the system `tar`.
    TarGz,
    /// Unpacked by the system `unzip`.
    Zip,
}

/// Why a tool is not in the default set: `install --all` and Install & scan
/// never install it, and it is installed only when named ([FR-SC-23]).
///
/// [FR-SC-23]: ../../../../docs/specs/requirements/FR-SC-23.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptIn {
    /// It runs on the [managed Java runtime](Manifest::java), installed with
    /// it and removed once no installed tool needs it (Dependency-Check).
    Java,
    /// It installs only once the user has acknowledged its licence (CodeQL).
    Licence(Licence),
}

/// A licence the user must acknowledge before a tool is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Licence {
    /// The `logos security install` flag that acknowledges it.
    pub flag: &'static str,
    /// The terms, as the refusal shows them.
    pub terms: &'static str,
}

/// The CodeQL CLI's licence, as `logos security install codeql` shows it.
pub const CODEQL_LICENCE: Licence = Licence {
    flag: "--accept-codeql-licence",
    terms: "the CodeQL CLI is licensed under the GitHub CodeQL Terms and Conditions \
            (https://github.com/github/codeql-cli-binaries/blob/main/LICENSE.md): it may be used \
            to analyse open-source codebases and for academic research; any other use, such as \
            analysing a closed-source codebase, needs a licence from GitHub",
};

/// One downloadable artifact of a tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub platform: Platform,
    /// An `https://` URL; its last path segment is the file name looked for in
    /// an offline `--from` directory.
    pub url: String,
    /// The artifact's SHA-256, 64 lowercase hex digits.
    pub sha256: String,
    /// The artifact's size in bytes, as published.
    pub size: u64,
    pub layout: Layout,
}

/// One tool the installer can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEntry {
    /// The registry name of the scanner this tool is.
    pub tool: String,
    /// The installed executable's file name.
    pub binary: String,
    /// The pinned version: the one installed, and the directory it lives in.
    pub version: String,
    /// The lowest version the registry's command lines were tested against.
    pub min_version: String,
    /// A release-binary tool's artifacts, one per platform; empty for a
    /// [Python](Self::python) tool.
    pub artifacts: Vec<Artifact>,
    /// A Python tool's package, installed by the managed `uv` instead of from
    /// an artifact.
    pub python: Option<PythonPackage>,
    /// Why the tool is installed only when named; `None` for the default set.
    pub opt_in: Option<OptIn>,
}

impl ToolEntry {
    /// The artifact for `platform`, if this tool has one there.
    pub fn artifact(&self, platform: Platform) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.platform == platform)
    }

    /// Whether `install --all` and Install & scan install it.
    pub fn is_default(&self) -> bool {
        self.opt_in.is_none()
    }

    /// The licence the tool installs under, if it needs one acknowledged.
    pub fn licence(&self) -> Option<&Licence> {
        match &self.opt_in {
            Some(OptIn::Licence(licence)) => Some(licence),
            _ => None,
        }
    }

    /// Whether the tool runs on the [managed Java runtime](Manifest::java).
    pub fn needs_java(&self) -> bool {
        self.opt_in == Some(OptIn::Java)
    }

    /// The executable's path inside the tool's `<tool>/<version>/` directory:
    /// the binary itself (a path inside the tree, for a [`Layout::Tree`]
    /// tool), or a Python environment's `bin/<binary>`.
    pub fn executable(&self) -> PathBuf {
        match self.python {
            Some(_) => PathBuf::from("bin").join(&self.binary),
            None => PathBuf::from(&self.binary),
        }
    }

    /// What installing the tool on `platform` downloads, in bytes; `None`
    /// where it cannot be installed. A Python tool's figure is its own
    /// packages only: `uv` and the managed Python are separate lines.
    pub fn download_size(&self, platform: Platform) -> Option<u64> {
        match &self.python {
            Some(package) => package.size(platform),
            None => self.artifact(platform).map(|a| a.size),
        }
    }
}

/// A Python scanner's package: what the managed `uv` installs, into an
/// environment of its own, with `--require-hashes` ([NFR-SE-10]).
///
/// [NFR-SE-10]: ../../../../docs/specs/requirements/NFR-SE-10.md
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonPackage {
    /// A requirements file: the package at the pinned version and every
    /// dependency, each `name==version` with its `--hash=sha256:` lines, as
    /// `uv pip compile --generate-hashes` writes it.
    pub requirements: String,
    /// The platforms the pinned packages have wheels for, each with the
    /// download size there, in bytes.
    pub sizes: Vec<(Platform, u64)>,
}

impl PythonPackage {
    /// The download size on `platform`, if the package installs there.
    pub fn size(&self, platform: Platform) -> Option<u64> {
        self.sizes
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, size)| *size)
    }

    /// Why these requirements cannot be installed as written, if they cannot:
    /// checked before anything is fetched. Each requirement must pin one
    /// version with `==` and carry at least one SHA-256, and nothing else may
    /// appear — no option (`-e`, `--index-url`, `-r`), URL or path that would
    /// let a requirement escape the hash check. `package` at `version` must be
    /// among them.
    pub fn defect(&self, package: &str, version: &str) -> Option<String> {
        let mut found = false;
        for requirement in logical_lines(&self.requirements) {
            let mut words = requirement.split_whitespace();
            let spec = words.next().unwrap_or_default();
            let Some((name, pinned)) = spec.split_once("==") else {
                return Some(format!("{spec:?} does not pin a version with =="));
            };
            let bare = name.split('[').next().unwrap_or_default();
            let plain = |s: &str| {
                !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            };
            let extras_ok = match name.split_once('[') {
                None => true,
                Some((_, rest)) => rest
                    .strip_suffix(']')
                    .is_some_and(|e| e.split(',').all(plain)),
            };
            if !plain(bare) || !extras_ok || !plain(pinned) {
                return Some(format!("{spec:?} is not a plain name==version requirement"));
            }
            // An environment marker sits between the pin and the hashes.
            let mut hashes = 0;
            let mut in_marker = false;
            for word in words {
                if let Some(hash) = word.strip_prefix("--hash=sha256:") {
                    if !is_sha256(hash) {
                        return Some(format!("{spec}: {hash:?} is not a SHA-256 in lowercase hex"));
                    }
                    hashes += 1;
                    in_marker = false;
                } else if word == ";" && hashes == 0 {
                    in_marker = true;
                } else if !(in_marker && hashes == 0 && !word.starts_with('-')) {
                    return Some(format!("{spec}: {word:?} is not a hash"));
                }
            }
            if hashes == 0 {
                return Some(format!("{spec} carries no --hash=sha256:"));
            }
            found |= bare.eq_ignore_ascii_case(package) && pinned == version;
        }
        (!found).then(|| format!("the requirements do not pin {package}=={version}"))
    }
}

/// The requirements' logical lines: `\`-continued lines joined, comments and
/// blank lines dropped.
fn logical_lines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.split(" #").next().unwrap_or_default();
        if line.trim_start().starts_with('#') {
            continue;
        }
        match line.trim_end().strip_suffix('\\') {
            Some(head) => {
                current.push_str(head);
                current.push(' ');
            }
            None => {
                current.push_str(line);
                if !current.trim().is_empty() {
                    lines.push(current.trim().to_string());
                }
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        lines.push(current.trim().to_string());
    }
    lines
}

fn is_sha256(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The managed Python the [`Manifest::uv`] provisions for every Python tool
/// (python-build-standalone, fetched and checked by `uv` itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPython {
    /// The exact version `uv python install` is asked for.
    pub version: String,
    /// Where `uv` downloads it from, for the consent line.
    pub host: String,
    /// The download size per platform, in bytes.
    pub sizes: Vec<(Platform, u64)>,
    /// The package index the Python tools are installed from.
    pub index: String,
}

impl ManagedPython {
    /// The download size on `platform`, if there is a build for it.
    pub fn size(&self, platform: Platform) -> Option<u64> {
        self.sizes
            .iter()
            .find(|(p, _)| *p == platform)
            .map(|(_, size)| *size)
    }

    /// The index's host, for the consent line.
    pub fn index_host(&self) -> &str {
        let rest = self.index.split_once("://").map_or("", |(_, r)| r);
        rest.split(['/', '?', '#']).next().unwrap_or("")
    }
}

/// Every tool the installer knows, with its pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub tools: Vec<ToolEntry>,
    /// The `uv` that installs the Python tools: a release binary, fetched and
    /// verified as every other one is, but never a tool of its own — it is
    /// installed with the first Python tool and removed with the last.
    pub uv: ToolEntry,
    /// The Python the Python tools run on.
    pub python: ManagedPython,
    /// The Java runtime the [`OptIn::Java`] tools run on: like [`uv`](Self::uv),
    /// a release artifact that is never a tool of its own — installed with the
    /// first tool that needs it and removed with the last (S-746).
    pub java: ToolEntry,
}

impl Manifest {
    /// The manifest compiled into this release.
    pub fn builtin() -> &'static Manifest {
        BUILTIN.get_or_init(|| Manifest {
            tools: builtin_tools(),
            uv: builtin_uv(),
            python: builtin_python(),
            java: builtin_java(),
        })
    }

    /// The entry for `tool`, by registry name.
    pub fn tool(&self, tool: &str) -> Option<&ToolEntry> {
        self.tools.iter().find(|t| t.tool == tool)
    }

    /// Whether a copy of `tool` at `version` meets the minimum tested version
    /// ([FR-SC-05]): the one rule detection and install share. A version that
    /// could not be read does not; a tool this manifest does not list has no
    /// minimum.
    ///
    /// [FR-SC-05]: ../../../../docs/specs/requirements/FR-SC-05.md
    pub fn meets_minimum(&self, tool: &str, version: Option<&str>) -> bool {
        self.tool(tool).is_none_or(|entry| {
            version.is_some_and(|v| version_at_least(v, &entry.min_version))
        })
    }
}

impl Artifact {
    /// The file name the artifact is saved as: the URL's last path segment.
    /// `None` for a URL that is not `https://` or whose last segment could
    /// name anything but a plain file in the staging directory.
    pub fn file_name(&self) -> Option<&str> {
        let rest = self.url.strip_prefix("https://")?;
        let path = rest.split(['?', '#']).next()?;
        let (_, name) = path.rsplit_once('/')?;
        let plain = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.starts_with('-')
            && !name.contains('\\');
        plain.then_some(name)
    }

    /// The URL's host, for the install event. Never the path or the query.
    pub fn host(&self) -> &str {
        let rest = self.url.strip_prefix("https://").unwrap_or("");
        rest.split(['/', '?', '#']).next().unwrap_or("")
    }

    /// Why this artifact cannot be installed as written, if it cannot: checked
    /// before anything is fetched.
    pub fn defect(&self) -> Option<String> {
        if self.file_name().is_none() {
            return Some(format!("{} is not an https URL naming a file", self.url));
        }
        if !is_sha256(&self.sha256) {
            return Some(format!(
                "{:?} is not a SHA-256 in lowercase hex",
                self.sha256
            ));
        }
        let member = match &self.layout {
            Layout::TarGz { member } => member,
            Layout::Tree { root, .. } => root,
            Layout::Binary => return None,
        };
        (!is_relative_member(member))
            .then(|| format!("{member:?} is not a relative archive member"))
    }
}

/// Whether `member` names a path inside an archive's extraction directory:
/// relative, every component a plain name, and never read as an option.
pub(crate) fn is_relative_member(member: &str) -> bool {
    !(member.is_empty()
        || member.starts_with('/')
        || member.starts_with('-')
        || member
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == ".."))
}

/// Whether `version` is at or above `minimum`, both dotted numeric versions
/// (`0.74.0`, `8.18`), compared component by component with missing
/// components read as `0`. A version that is not dotted numeric is never at
/// or above anything: an unknown version cannot be shown to meet a minimum.
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    match (version_key(version), version_key(minimum)) {
        (Some(mut v), Some(mut m)) => {
            let len = v.len().max(m.len());
            v.resize(len, 0);
            m.resize(len, 0);
            v >= m
        }
        _ => false,
    }
}

/// A dotted numeric version's components, or `None` for anything else.
pub(crate) fn version_key(version: &str) -> Option<Vec<u64>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

/// A `.tar.gz` artifact whose binary sits at the archive root.
fn tar_gz(platform: Platform, url: String, sha256: &str, size: u64, member: &str) -> Artifact {
    Artifact {
        platform,
        url,
        sha256: sha256.to_string(),
        size,
        layout: Layout::TarGz {
            member: member.to_string(),
        },
    }
}

fn binary(platform: Platform, url: String, sha256: &str, size: u64) -> Artifact {
    Artifact {
        platform,
        url,
        sha256: sha256.to_string(),
        size,
        layout: Layout::Binary,
    }
}

/// The compiled-in manifest, built on its first [`Manifest::builtin`] call.
/// The build is a call in that function's body, never a `LazyLock`
/// initializer: dead-code reachability runs over calls from callables, so
/// only from a body does it reach [`builtin_tools`] and the artifact helpers.
static BUILTIN: OnceLock<Manifest> = OnceLock::new();

// One line per artifact, so a release bump reads as one row per platform.
#[rustfmt::skip]
fn builtin_tools() -> Vec<ToolEntry> {
    use Platform::{LinuxArm64, LinuxX86_64, MacosArm64, MacosX86_64};
    let trivy = |asset: &str| {
        format!("https://github.com/aquasecurity/trivy/releases/download/v0.74.0/trivy_0.74.0_{asset}.tar.gz")
    };
    let gitleaks = |asset: &str| {
        format!("https://github.com/gitleaks/gitleaks/releases/download/v8.18.4/gitleaks_8.18.4_{asset}.tar.gz")
    };
    let osv = |asset: &str| {
        format!("https://github.com/google/osv-scanner/releases/download/v1.9.2/osv-scanner_{asset}")
    };
    let gosec = |asset: &str| {
        format!("https://github.com/securego/gosec/releases/download/v2.21.4/gosec_2.21.4_{asset}.tar.gz")
    };
    vec![
        ToolEntry {
            tool: "trivy".to_string(),
            binary: "trivy".to_string(),
            version: "0.74.0".to_string(),
            min_version: "0.74.0".to_string(),
            artifacts: vec![
                tar_gz(MacosArm64, trivy("macOS-ARM64"), "1caada5e0e2091909357c7525d3aa76f4b660b13821bc143b190c7483e31cc11", 47_694_894, "trivy"),
                tar_gz(MacosX86_64, trivy("macOS-64bit"), "472816f6888dda689d075c30254d4210b4d1035acf365aa72332f584c2f60485", 51_440_811, "trivy"),
                tar_gz(LinuxX86_64, trivy("Linux-64bit"), "2ae6fe3ee734b7fdf11335663e18c75ea12dccc76062f09f164a3b0f8be4371a", 50_419_107, "trivy"),
                tar_gz(LinuxArm64, trivy("Linux-ARM64"), "b94ce1976bbf3c15b514b605ee88be7c6d94a29be2302847ff01cb794d47aad5", 45_424_969, "trivy"),
            ],
            python: None,
            opt_in: None,
        },
        ToolEntry {
            tool: "gitleaks".to_string(),
            binary: "gitleaks".to_string(),
            version: "8.18.4".to_string(),
            min_version: "8.18.0".to_string(),
            artifacts: vec![
                tar_gz(MacosArm64, gitleaks("darwin_arm64"), "a480d8593acd8215b22402cf0f3f88b01dcd3610c63b5391db640f7767e62104", 2_790_298, "gitleaks"),
                tar_gz(MacosX86_64, gitleaks("darwin_x64"), "1a69e5666b13cd374889cbcb1939ed1573b63b551251283d5d2329a53cf58e2f", 2_956_128, "gitleaks"),
                tar_gz(LinuxX86_64, gitleaks("linux_x64"), "ba6dbb656933921c775ee5a2d1c13a91046e7952e9d919f9bac4cec61d628e7d", 2_903_447, "gitleaks"),
                tar_gz(LinuxArm64, gitleaks("linux_arm64"), "bf5f7f466ebfade1296c8bd32cf7d3f592c2aa78836aa9980ffbe2cadca7a861", 2_678_206, "gitleaks"),
            ],
            python: None,
            opt_in: None,
        },
        ToolEntry {
            tool: "osv-scanner".to_string(),
            binary: "osv-scanner".to_string(),
            version: "1.9.2".to_string(),
            min_version: "1.9.0".to_string(),
            artifacts: vec![
                binary(MacosArm64, osv("darwin_arm64"), "393f2c7089d9431bd26a3804d6e46d417b1c05abd5d49c41c7dfc174c520acf0", 31_840_978),
                binary(MacosX86_64, osv("darwin_amd64"), "487ab433b2c2a8c80b737c0bd428a80e6d2e211b4adf775a52a6964163fa3249", 32_986_512),
                binary(LinuxX86_64, osv("linux_amd64"), "d6af4b67fa5de658598bd2d445efb99e90d1734b3146962418719c4350ecb74b", 32_116_888),
                binary(LinuxArm64, osv("linux_arm64"), "9c6160afb26c79449a1f1b667323b989a57dda8fc19f22936c9ff920fd97ddfa", 30_802_072),
            ],
            python: None,
            opt_in: None,
        },
        ToolEntry {
            tool: "gosec".to_string(),
            binary: "gosec".to_string(),
            version: "2.21.4".to_string(),
            min_version: "2.21.0".to_string(),
            artifacts: vec![
                tar_gz(MacosArm64, gosec("darwin_arm64"), "32fa76e1142b81696c53066970c53d221b41bac03c2edb8f452cb993ade89881", 14_448_698, "gosec"),
                tar_gz(MacosX86_64, gosec("darwin_amd64"), "ac58986182c99ee74f4a01283aae1bb22b8a904573887aeecf51854c789757a3", 15_086_215, "gosec"),
                tar_gz(LinuxX86_64, gosec("linux_amd64"), "9229dbfdc092b176e628b9ea6e4210757373b819f47365cedd9f9e12d3b2c173", 15_127_867, "gosec"),
                tar_gz(LinuxArm64, gosec("linux_arm64"), "7e5b0aa5052c2559d78e2ef60100285ddcd95b19b2229b114ed25dfdf89a88c8", 14_226_257, "gosec"),
            ],
            python: None,
            opt_in: None,
        },
        // The Python scanners. Each size is the wheels `uv` picks for Python
        // 3.12 on that platform, summed from PyPI's metadata when pinned.
        python_tool("semgrep", "1.85.0", "1.85.0", include_str!("requirements/semgrep.txt"),
            [34_492_227, 34_512_126, 35_014_877, 34_970_069]),
        // Bandit writes SARIF only through its `sarif` extra, which is pinned.
        python_tool("bandit", "1.7.10", "1.7.10", include_str!("requirements/bandit.txt"),
            [3_132_964, 3_141_054, 3_766_861, 3_734_107]),
        python_tool("checkov", "3.2.533", "3.2.0", include_str!("requirements/checkov.txt"),
            [46_631_798, 51_981_581, 55_366_037, 53_947_390]),
        // The opt-in pair (S-746): installed only when named. Dependency-Check
        // is one zip for every platform, run on the managed Java runtime; the
        // minimum is the pin, because each earlier line has broken on a change
        // to the NVD data it reads and the registry's flags were checked
        // against 13.0.0. Each SHA-256 is the GitHub asset digest.
        ToolEntry {
            tool: "dependency-check".to_string(),
            binary: "bin/dependency-check.sh".to_string(),
            version: "13.0.0".to_string(),
            min_version: "13.0.0".to_string(),
            artifacts: Platform::ALL.into_iter().map(|p| {
                tree(p, "https://github.com/dependency-check/DependencyCheck/releases/download/v13.0.0/dependency-check-13.0.0-release.zip".to_string(), "44d920d1ec03e948df862a253f0912782a31b9beee8a7c8895b9cb95760176ed", 29_411_949, Archive::Zip, "dependency-check")
            }).collect(),
            python: None,
            opt_in: Some(OptIn::Java),
        },
        // The CodeQL bundle carries the CLI and its standard query packs, so
        // an analysis fetches nothing. The macOS bundle is one universal
        // build. Each SHA-256 is the publisher's `.checksum.txt`, cross-checked
        // against the GitHub asset digest.
        ToolEntry {
            tool: "codeql".to_string(),
            binary: "codeql".to_string(),
            version: "2.27.2".to_string(),
            min_version: "2.19.0".to_string(),
            artifacts: vec![
                tree(MacosArm64, codeql("osx64"), "bdd42bab471ed16277048b35b9be7264d07bd19b7391c37d30c74e9555e61e92", 1_257_204_497, Archive::TarGz, "codeql"),
                tree(MacosX86_64, codeql("osx64"), "bdd42bab471ed16277048b35b9be7264d07bd19b7391c37d30c74e9555e61e92", 1_257_204_497, Archive::TarGz, "codeql"),
                tree(LinuxX86_64, codeql("linux64"), "f002864be6dd8d5d7bdb123aaf7291ec8e291012bd9f70b6d2362f730db52aeb", 692_996_129, Archive::TarGz, "codeql"),
                tree(LinuxArm64, codeql("linux-arm64"), "c1779ce6335606ef86fce4aae0db2ac4d7da869527d6a67bf055d9135695473c", 675_372_649, Archive::TarGz, "codeql"),
            ],
            python: None,
            opt_in: Some(OptIn::Licence(CODEQL_LICENCE)),
        },
    ]
}

fn codeql(asset: &str) -> String {
    format!("https://github.com/github/codeql-action/releases/download/codeql-bundle-v2.27.2/codeql-bundle-{asset}.tar.gz")
}

/// An archive unpacked whole, its `root` directory becoming the tool's.
fn tree(platform: Platform, url: String, sha256: &str, size: u64, archive: Archive, root: &str) -> Artifact {
    Artifact {
        platform,
        url,
        sha256: sha256.to_string(),
        size,
        layout: Layout::Tree {
            archive,
            root: root.to_string(),
        },
    }
}

/// A Python scanner; `sizes` in [`Platform::ALL`] order.
fn python_tool(tool: &str, version: &str, min_version: &str, requirements: &str, sizes: [u64; 4]) -> ToolEntry {
    ToolEntry {
        tool: tool.to_string(),
        binary: tool.to_string(),
        version: version.to_string(),
        min_version: min_version.to_string(),
        artifacts: Vec::new(),
        python: Some(PythonPackage {
            requirements: requirements.to_string(),
            sizes: Platform::ALL.into_iter().zip(sizes).collect(),
        }),
        opt_in: None,
    }
}

// The Linux builds are the static musl ones, so they run on any distribution.
// Each SHA-256 is the publisher's `.sha256` file, cross-checked against the
// GitHub asset digest.
#[rustfmt::skip]
fn builtin_uv() -> ToolEntry {
    use Platform::{LinuxArm64, LinuxX86_64, MacosArm64, MacosX86_64};
    let uv = |target: &str| {
        format!("https://github.com/astral-sh/uv/releases/download/0.12.21/uv-{target}.tar.gz")
    };
    let member = |target: &str| format!("uv-{target}/uv");
    ToolEntry {
        tool: "uv".to_string(),
        binary: "uv".to_string(),
        version: "0.12.21".to_string(),
        min_version: "0.12.21".to_string(),
        artifacts: vec![
            tar_gz(MacosArm64, uv("aarch64-apple-darwin"), "b88bda573e566ef9bced66b155fe0408626fbbc053aee1c30ba686f0728c9447", 17_001_427, &member("aarch64-apple-darwin")),
            tar_gz(MacosX86_64, uv("x86_64-apple-darwin"), "2b336763b396ec6afa20c5a8b083538ca7402445b868311979d740a4344c17d8", 20_741_769, &member("x86_64-apple-darwin")),
            tar_gz(LinuxX86_64, uv("x86_64-unknown-linux-musl"), "d69d543a55ec9cdf9d3d9f2648b0a161847e3dbddc477e3be6b5813a6d46f639", 22_761_587, &member("x86_64-unknown-linux-musl")),
            tar_gz(LinuxArm64, uv("aarch64-unknown-linux-musl"), "67389a674e62adffa5a395d9a3b80688731c4aa7b33a6def3e62d00f7fec821f", 21_211_593, &member("aarch64-unknown-linux-musl")),
        ],
        python: None,
        opt_in: None,
    }
}

/// Eclipse Temurin 21.0.12.1+1, the JRE: Dependency-Check 13 runs on Java 11
/// and later. On macOS the runtime is the bundle's `Contents/Home`, so the
/// executable is `bin/java` on every platform. Each SHA-256 is the
/// publisher's `.sha256.txt`, cross-checked against the GitHub asset digest.
#[rustfmt::skip]
fn builtin_java() -> ToolEntry {
    use Platform::{LinuxArm64, LinuxX86_64, MacosArm64, MacosX86_64};
    let jre = |asset: &str| {
        format!("https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_{asset}_hotspot_21.0.12.1_1.tar.gz")
    };
    let (mac, linux) = ("jdk-21.0.12.1+1-jre/Contents/Home", "jdk-21.0.12.1+1-jre");
    ToolEntry {
        tool: "java".to_string(),
        binary: "bin/java".to_string(),
        version: "21.0.12.1".to_string(),
        min_version: "21.0.12.1".to_string(),
        artifacts: vec![
            tree(MacosArm64, jre("aarch64_mac"), "dec50fc6f9fcd4fe3ae8cabf5a5fa68f6afc48841f7698e468e9aa5d54beed84", 48_144_965, Archive::TarGz, mac),
            tree(MacosX86_64, jre("x64_mac"), "6717ec641fd9ce0bb209ca083ee23b42202ac68cb6fcc5753496e0e4a0f41989", 42_131_876, Archive::TarGz, mac),
            tree(LinuxX86_64, jre("x64_linux"), "2413149700df0f7d440500a84a8f764c535f21e5a5e87d38328b64eec2c5b500", 52_059_408, Archive::TarGz, linux),
            tree(LinuxArm64, jre("aarch64_linux"), "14be1f35ebdbd1f6e8d57eb911a3ffb74d6d9aa255abc5daf2b1302002cf2cf2", 51_149_460, Archive::TarGz, linux),
        ],
        python: None,
        opt_in: None,
    }
}

/// CPython 3.12.14, python-build-standalone 20260929 `install_only_stripped`:
/// the newest 3.12 the pinned `uv` knows, which checks the archive's SHA-256
/// against its own list. Sizes are the release assets'.
fn builtin_python() -> ManagedPython {
    ManagedPython {
        version: "3.12.14".to_string(),
        host: "github.com".to_string(),
        sizes: Platform::ALL
            .into_iter()
            .zip([25_017_608, 24_729_143, 34_277_962, 29_203_166])
            .collect(),
        index: "https://pypi.org/simple".to_string(),
    }
}
