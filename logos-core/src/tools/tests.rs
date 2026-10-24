//! Unit tests for the tool installer: the fixed `curl` argv, the hash, the
//! builtin manifest's shape, and the platform and selection refusals. The
//! install flows over fixture artifacts are in `tests/tools_install.rs`.

use std::ffi::OsString;
use std::path::Path;

use super::*;
use crate::security::registry::REGISTRY;

#[test]
fn curl_runs_with_a_fixed_https_only_argv_that_ignores_curlrc() {
    let args = curl_args(
        "https://example.test/a/tool.tar.gz",
        Path::new("/stage/tool.tar.gz"),
    );
    let expected: Vec<OsString> = [
        "-q",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--fail",
        "--location",
        "--max-redirs",
        "10",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "30",
        "--output",
        "/stage/tool.tar.gz",
        "https://example.test/a/tool.tar.gz",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    assert_eq!(args, expected);
}

#[test]
fn the_hash_is_sha256_in_lowercase_hex() {
    let tmp = tempfile::TempDir::new().unwrap();
    let file = tmp.path().join("abc");
    std::fs::write(&file, b"abc").unwrap();
    // FIPS 180-2's "abc" test vector.
    assert_eq!(
        sha256_file(&file).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn every_builtin_tool_is_a_registered_scanner_pinned_for_all_four_platforms() {
    let manifest = Manifest::builtin();
    let names: Vec<&str> = manifest.tools.iter().map(|t| t.tool.as_str()).collect();
    assert_eq!(
        names,
        [
            "trivy",
            "gitleaks",
            "osv-scanner",
            "gosec",
            "semgrep",
            "bandit",
            "checkov",
            "dependency-check",
            "codeql"
        ]
    );
    // The opt-in pair (S-746) is outside the default set, each for its own
    // reason; every other tool is in it.
    let opt_in: Vec<(&str, bool, Option<&str>)> = manifest
        .tools
        .iter()
        .filter(|t| !t.is_default())
        .map(|t| (t.tool.as_str(), t.needs_java(), t.licence().map(|l| l.flag)))
        .collect();
    assert_eq!(
        opt_in,
        [
            ("dependency-check", true, None),
            ("codeql", false, Some("--accept-codeql-licence"))
        ]
    );
    assert!(CODEQL_LICENCE.terms.contains("open-source codebases"));
    let release = |entry: &ToolEntry| {
        let platforms: Vec<Platform> = entry.artifacts.iter().map(|a| a.platform).collect();
        assert_eq!(platforms, Platform::ALL, "{}", entry.tool);
        for artifact in &entry.artifacts {
            assert_eq!(artifact.defect(), None, "{}: {}", entry.tool, artifact.url);
            assert_eq!(artifact.host(), "github.com");
            assert!(
                artifact.url.contains(&entry.version),
                "{}: the URL is the pinned release",
                artifact.url
            );
            assert!(artifact.size > 0);
        }
    };
    for entry in &manifest.tools {
        let scanner = REGISTRY
            .iter()
            .find(|s| s.name == entry.tool)
            .expect("a registered scanner");
        // A tree tool's binary is a path inside its tree; its file name is
        // what detection looks for on `PATH`.
        let file_name = Path::new(&entry.binary).file_name().unwrap().to_str().unwrap();
        assert!(
            scanner.binaries.contains(&file_name),
            "{}: the managed binary is a name detection looks for",
            entry.tool
        );
        assert!(
            version_key(&entry.version) >= version_key(&entry.min_version),
            "{}: the pin is at or above the tested minimum",
            entry.tool
        );
        match &entry.python {
            None => release(entry),
            Some(package) => {
                assert!(entry.artifacts.is_empty(), "{}", entry.tool);
                assert_eq!(package.defect(&entry.tool, &entry.version), None, "{}", entry.tool);
                for platform in Platform::ALL {
                    assert!(package.size(platform).is_some_and(|s| s > 0), "{}", entry.tool);
                }
                assert_eq!(
                    entry.executable(),
                    std::path::Path::new("bin").join(&entry.binary)
                );
            }
        }
    }
    // `uv` and the managed Python are support for the Python tools, never a
    // tool of their own.
    assert!(manifest.tool(&manifest.uv.tool).is_none());
    assert!(manifest.uv.python.is_none());
    release(&manifest.uv);
    // The Java runtime likewise, for the Java tools: on every platform its
    // tree's root holds `bin/java`, which is what its home is found by.
    assert!(manifest.tool(&manifest.java.tool).is_none());
    assert!(manifest.java.python.is_none() && manifest.java.is_default());
    release(&manifest.java);
    assert_eq!(manifest.java.executable(), Path::new("bin").join("java"));
    for artifact in &manifest.java.artifacts {
        assert!(matches!(&artifact.layout, Layout::Tree { archive: Archive::TarGz, .. }));
    }
    for platform in Platform::ALL {
        assert!(manifest.python.size(platform).is_some_and(|s| s > 0));
    }
    assert_eq!(manifest.python.index_host(), "pypi.org");
    assert!(manifest.python.index.starts_with("https://"));
}

/// One requirement, `uv pip compile --generate-hashes` style.
const PIN: &str = "bandit==1.7.10 \\\n    --hash=sha256:665721d7bebbb4485a339c55161ac0eedde27d51e638000d91c8c2d68343ad02\n";
const DEP: &str = "colorama==0.4.6 ; sys_platform == 'win32' \\\n    --hash=sha256:4f1d9991f5acc0ca119f9d443620b77f9d6b33703e51011c16baf57afb285fc6\n";

#[test]
fn python_requirements_must_pin_and_hash_every_package_and_name_nothing_else() {
    let check = |text: &str| {
        PythonPackage {
            requirements: text.to_string(),
            sizes: Vec::new(),
        }
        .defect("bandit", "1.7.10")
    };
    assert_eq!(check(&format!("# a comment\n{PIN}{DEP}")), None);
    assert_eq!(
        check(&PIN.replace("bandit==", "bandit[sarif]==")),
        None,
        "an extra is a plain name"
    );
    let near_misses = [
        // The near miss of each accepted shape.
        (PIN.replace("==", ">="), "an unpinned version"),
        (PIN.replace("==1.7.10", "===1.7.10"), "an arbitrary-equality pin"),
        ("bandit==1.7.10\n".to_string(), "no hash"),
        (PIN.replace("sha256:6", "sha256:G"), "a hash that is not hex"),
        (PIN.replace("sha256:", "md5:"), "a hash that is not SHA-256"),
        (PIN.replace("ad02", "ad0"), "a 63-digit hash"),
        (format!("{PIN}-e git+https://example.invalid/x\n"), "an editable"),
        (format!("{PIN}--index-url https://example.invalid/simple\n"), "an index option"),
        (format!("{PIN}-r other.txt\n"), "an included file"),
        (format!("{PIN}x @ https://example.invalid/x.whl --hash=sha256:{}\n", "0".repeat(64)), "a URL requirement"),
        (PIN.replace("bandit==1.7.10", "bandit[sarif==1.7.10"), "a broken extra"),
        (PIN.replace(" \\\n    --hash", " --index-url=x --hash"), "an option among the hashes"),
        (PIN.replace("1.7.10", "1.7.9"), "the tool at another version"),
        (PIN.replace("bandit==", "bandit2=="), "another package named like the tool"),
        (DEP.to_string(), "the tool missing"),
    ];
    for (text, why) in near_misses {
        assert!(check(&text).is_some(), "{why}: {text:?}");
    }
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap()).collect()
}

#[test]
fn an_artifact_that_could_escape_staging_or_skip_https_is_a_defect() {
    let base = Manifest::builtin().tools[0].artifacts[0].clone();
    let with = |f: &dyn Fn(&mut Artifact)| {
        let mut a = base.clone();
        f(&mut a);
        a.defect()
    };
    assert_eq!(with(&|_| {}), None);
    for url in [
        "http://github.com/x/tool.tar.gz",
        "file:///etc/passwd",
        "https://github.com/x/",
        "https://github.com/x/..",
        "https://github.com/x/-o",
        "https://host-only",
    ] {
        assert!(with(&|a| a.url = url.to_string()).is_some(), "{url}");
    }
    assert!(with(&|a| a.sha256 = "ABC".to_string()).is_some());
    assert!(
        with(&|a| a.sha256 = base.sha256.to_uppercase()).is_some(),
        "uppercase hex"
    );
    for member in ["", "/trivy", "../trivy", "a/../trivy", "-trivy", "a//trivy"] {
        assert!(
            with(&|a| a.layout = Layout::TarGz {
                member: member.to_string()
            })
            .is_some(),
            "{member:?}"
        );
    }
    assert_eq!(
        with(&|a| a.layout = Layout::TarGz {
            member: "dir/trivy".to_string()
        }),
        None
    );
    // The query string is not part of the file name, and never part of the host.
    let mut q = base.clone();
    q.url = "https://github.com/x/tool.tar.gz?token=1".to_string();
    assert_eq!(q.file_name(), Some("tool.tar.gz"));
    assert_eq!(q.host(), "github.com");
}

#[test]
fn a_platform_without_managed_installs_reports_unavailable_and_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tools = tmp.path().join("tools");
    let report = Installer::new(tools.clone())
        .with_platform(None)
        .with_search_path(None)
        .install(&["trivy", "gosec"], &Source::Network)
        .unwrap();
    assert_eq!(report.tools.len(), 2);
    for tool in &report.tools {
        assert_eq!(
            tool.outcome,
            InstallOutcome::Unavailable {
                reason: MANAGED_UNAVAILABLE
            },
            "{}",
            tool.tool
        );
    }
    assert_eq!(
        MANAGED_UNAVAILABLE,
        "managed install not available on this platform"
    );
    assert!(!tools.join("trivy").exists() && !tools.join(STAGING_DIR).exists());
}

/// On Windows the manifest has no platform, so every tool is unavailable.
#[cfg(windows)]
#[test]
fn windows_reports_managed_install_not_available_on_this_platform() {
    assert_eq!(Platform::current(), None);
    let tmp = tempfile::TempDir::new().unwrap();
    let report = Installer::new(tmp.path().to_path_buf())
        .install(&["trivy"], &Source::Network)
        .unwrap();
    assert_eq!(
        report.tools[0].outcome,
        InstallOutcome::Unavailable {
            reason: "managed install not available on this platform"
        }
    );
}

#[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
#[test]
fn the_platforms_this_suite_runs_on_have_managed_installs() {
    assert!(Platform::current().is_some());
}

#[test]
fn an_unknown_tool_is_refused_before_anything_is_fetched() {
    let tmp = tempfile::TempDir::new().unwrap();
    let installer = Installer::new(tmp.path().join("tools")).with_search_path(None);
    let err = installer
        .install(&["trivy", "nmap"], &Source::Network)
        .unwrap_err();
    assert!(
        matches!(&err, ConfigError::InvalidValue { message, .. } if message.contains("\"nmap\"") && message.contains("osv-scanner")),
        "{err:?}"
    );
    for not_a_tool in ["java", "uv"] {
        assert!(
            installer.uninstall(&[not_a_tool]).is_err(),
            "{not_a_tool} is not a tool Logos installs"
        );
    }
    assert!(!tmp.path().join("tools").exists(), "nothing was created");
}

#[test]
fn the_install_report_serialises_one_tagged_outcome_per_tool() {
    let report = InstallReport {
        tools: vec![ToolInstall {
            tool: "trivy".to_string(),
            version: "0.74.0".to_string(),
            outcome: InstallOutcome::Failed {
                stage: FailStage::Checksum,
                reason: "checksum mismatch".to_string(),
            },
        }],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({"tools": [{
            "tool": "trivy", "version": "0.74.0", "outcome": "failed",
            "stage": "checksum", "reason": "checksum mismatch"
        }]})
    );
}

/// ADR-88's fitness function: only the tool installer spawns `curl`. Every
/// source file in the workspace that names `"curl"` is listed here with why,
/// so a new mention fails until it is classified.
#[test]
fn only_the_tool_installer_names_curl_as_a_program() {
    const ALLOWED: [(&str, &str); 3] = [
        ("logos-core/src/tools/mod.rs", "the installer's download"),
        ("logos-core/src/tools/tests.rs", "this guard"),
        (
            "logos-core/src/hooks/quality_report.rs",
            "a test asserting the hook names no network tool",
        ),
    ];
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut found = Vec::new();
    for member in std::fs::read_dir(workspace).unwrap().flatten() {
        let src = member.path().join("src");
        if member.path().join("Cargo.toml").is_file() && src.is_dir() {
            collect_curl_mentions(workspace, &src, &mut found);
        }
    }
    found.sort();
    let mut allowed: Vec<String> = ALLOWED.iter().map(|(f, _)| f.to_string()).collect();
    allowed.sort();
    assert_eq!(
        found, allowed,
        "a new source names \"curl\": classify it here"
    );
}

fn collect_curl_mentions(workspace: &Path, dir: &Path, found: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_curl_mentions(workspace, &path, found);
        } else if path.extension().is_some_and(|e| e == "rs")
            && std::fs::read_to_string(&path).is_ok_and(|t| t.contains("\"curl\""))
        {
            let relative = path.strip_prefix(workspace).unwrap();
            found.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn from_process_needs_a_platform_with_managed_installs_then_a_tools_directory() {
    let invalid = |r: Result<Installer, ConfigError>| match r {
        Err(ConfigError::InvalidValue { message, .. }) => message,
        other => panic!("{other:?}"),
    };
    let platform = Some(Platform::LinuxX86_64);
    assert_eq!(
        invalid(Installer::from_env(Some("/t".into()), None)),
        MANAGED_UNAVAILABLE,
        "Windows names the platform, not the directory"
    );
    assert_eq!(
        invalid(Installer::from_env(None, None)),
        MANAGED_UNAVAILABLE
    );
    let missing = invalid(Installer::from_env(None, platform));
    assert!(
        missing.contains("no user-level tools directory") && missing.contains("$LOGOS_DATA_HOME"),
        "{missing}"
    );
    let installer = Installer::from_env(Some("/t".into()), platform).unwrap();
    assert_eq!(installer.tools_dir(), Path::new("/t"));
}

/// Every target maps by its `std::env::consts` spelling; Windows and every
/// other target have no managed installs. Runs on every host, so the Windows
/// row is checked without a Windows runner.
#[test]
fn each_target_maps_to_its_platform_and_windows_to_none() {
    let rows = [
        ("macos", "aarch64", Some(Platform::MacosArm64)),
        ("macos", "x86_64", Some(Platform::MacosX86_64)),
        ("linux", "x86_64", Some(Platform::LinuxX86_64)),
        ("linux", "aarch64", Some(Platform::LinuxArm64)),
        ("windows", "x86_64", None),
        ("windows", "aarch64", None),
        ("linux", "x86", None),
        ("freebsd", "x86_64", None),
    ];
    for (os, arch, expected) in rows {
        assert_eq!(Platform::for_target(os, arch), expected, "{os}/{arch}");
    }
    assert_eq!(
        Platform::current(),
        Platform::for_target(std::env::consts::OS, std::env::consts::ARCH)
    );
}

#[test]
fn a_version_meets_a_minimum_component_by_component_and_an_unknown_one_never_does() {
    use super::version_at_least;
    for (version, minimum) in [("0.74.0", "0.74.0"), ("0.75.0", "0.74.0"), ("0.74.1", "0.74"), ("1.0", "0.99.9"), ("8.18.10", "8.18.9")] {
        assert!(version_at_least(version, minimum), "{version} >= {minimum}");
    }
    // `8.18.10` is above `8.18.9` numerically, though not as a string.
    for (version, minimum) in [("0.73.9", "0.74.0"), ("8.18.9", "8.18.10"), ("0.74", "0.74.1"), ("", "0.1"), ("v0.74.0", "0.1"), ("0.74.0-rc1", "0.1")] {
        assert!(!version_at_least(version, minimum), "{version:?} < {minimum}");
    }
}

#[cfg(unix)]
#[test]
fn resolve_prefers_the_pin_then_the_newest_older_copy_at_or_above_the_minimum() {
    use std::os::unix::fs::PermissionsExt;
    let tools = tempfile::TempDir::new().unwrap();
    let mut manifest = Manifest::builtin().clone();
    let entry = manifest.tools.iter_mut().find(|t| t.tool == "trivy").unwrap();
    (entry.version, entry.min_version) = ("0.80.0".to_string(), "0.74.0".to_string());
    let install = |version: &str| {
        let binary = tools.path().join("trivy").join(version).join("trivy");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        binary
    };
    let found = |manifest: &Manifest| resolve(tools.path(), manifest, "trivy").map(|m| m.version);

    install("0.73.0");
    assert_eq!(found(&manifest), None, "below the minimum is not a copy");
    install("0.74.0");
    install("0.9.0");
    install("not-a-version");
    assert_eq!(found(&manifest), Some("0.74.0".to_string()), "the newest usable older copy");
    install("0.80.0");
    assert_eq!(found(&manifest), Some("0.80.0".to_string()), "the pin wins");
    assert_eq!(found(Manifest::builtin()), Some("0.74.0".to_string()));
}

#[test]
fn a_tree_root_that_could_escape_the_extraction_directory_is_a_defect() {
    let base = Manifest::builtin().tool("dependency-check").unwrap().artifacts[0].clone();
    assert_eq!(base.defect(), None);
    let with_root = |root: &str| {
        let mut a = base.clone();
        a.layout = Layout::Tree {
            archive: Archive::Zip,
            root: root.to_string(),
        };
        a.defect()
    };
    assert_eq!(with_root("jdk-21.0.12.1+1-jre/Contents/Home"), None);
    for near_miss in ["", "/codeql", "../codeql", "codeql/../..", "codeql//x", "./codeql", "-x", "codeql/."] {
        assert!(with_root(near_miss).is_some(), "{near_miss:?} is a defect");
    }
}

#[test]
fn a_java_tool_runs_with_both_launcher_variables_at_the_managed_runtime() {
    let home = Path::new("/tools/java/21.0.12.1");
    let vars = java_env(home);
    assert_eq!(
        vars,
        [
            ("JAVA_HOME", OsString::from("/tools/java/21.0.12.1")),
            ("JAVACMD", OsString::from("/tools/java/21.0.12.1/bin/java")),
        ],
        "JAVACMD is what the launcher reads first, so the user's own cannot win"
    );
}

/// A lock file its last holder removed is no lock. An install that opened the
/// file before a sweep removed it, and locks it after, holds nothing the next
/// sweep would see: the lock is taken again on the file now at the path, so
/// that sweep leaves the install's staging alone (S-759).
#[cfg(unix)]
#[test]
fn a_lock_on_a_removed_lock_file_is_not_held_and_is_taken_on_the_current_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join(STAGING_DIR);
    let entry = root.join("trivy-0.74.0");
    fs::create_dir_all(&root).unwrap();
    // The install has opened the lock file; a sweep takes it, finds no entry,
    // removes the file and releases it.
    let opened = open_lock_file(&lock_path(&entry)).unwrap();
    sweep_stale_staging(&root);
    assert!(!lock_path(&entry).exists(), "the sweep removed the lock file");
    assert!(matches!(hold(opened, &lock_path(&entry)).unwrap(), Hold::Unlinked), "the removed file is no lock");

    let held = StagingLock::take(&entry).unwrap().expect("the entry is free");
    fs::create_dir_all(&entry).unwrap();
    fs::write(entry.join("trivy.tar.gz"), "partial").unwrap();
    sweep_stale_staging(&root);
    assert!(entry.join("trivy.tar.gz").is_file(), "a sweep leaves a held entry alone");
    drop(held);
}

/// The managed coverage tools (S-748, FR-CV-17): each a registered coverage
/// tool, never a scanner — so no security surface sees it — pinned for all
/// four platforms as a `.tar.gz` holding the binary the coverage registry
/// runs, at its root. Only [`Manifest::entry`] and [`Manifest::coverage_tool`]
/// find it, and the installer accepts it by name.
#[test]
fn the_managed_coverage_tool_is_cargo_llvm_cov_and_never_a_scanner() {
    let manifest = Manifest::builtin();
    let names: Vec<&str> = manifest.coverage.iter().map(|t| t.tool.as_str()).collect();
    assert_eq!(names, ["cargo-llvm-cov"]);
    let entry = &manifest.coverage[0];
    assert!(crate::coverage::registry::tool(&entry.tool).is_some(), "a registered coverage tool");
    assert!(REGISTRY.iter().all(|s| s.name != entry.tool), "not a scanner");
    assert!(manifest.tool(&entry.tool).is_none(), "the security surfaces' lookup never finds it");
    assert_eq!(manifest.entry(&entry.tool), Some(entry));
    assert_eq!(manifest.coverage_tool(&entry.tool), Some(entry));
    assert_eq!(manifest.entry("trivy"), manifest.tool("trivy"), "a scanner still resolves as one");
    assert_eq!(manifest.coverage_tool("trivy"), None);
    assert!(entry.is_default() && entry.python.is_none());
    assert!(version_key(&entry.version) >= version_key(&entry.min_version));
    assert!(manifest.meets_minimum(&entry.tool, Some(&entry.min_version)));
    assert!(!manifest.meets_minimum(&entry.tool, Some("0.5.9")), "its minimum counts");
    let platforms: Vec<Platform> = entry.artifacts.iter().map(|a| a.platform).collect();
    assert_eq!(platforms, Platform::ALL);
    for artifact in &entry.artifacts {
        assert_eq!(artifact.defect(), None, "{}", artifact.url);
        assert_eq!(artifact.host(), "github.com");
        assert!(artifact.url.contains(&format!("/v{}/", entry.version)), "{}", artifact.url);
        assert_eq!(artifact.layout, Layout::TarGz { member: entry.binary.clone() });
        assert!(artifact.size > 0);
    }
    assert!(
        Installer::new(PathBuf::from("/nonexistent")).entries(&["cargo-llvm-cov"]).is_ok(),
        "the installer installs it by name"
    );
}
