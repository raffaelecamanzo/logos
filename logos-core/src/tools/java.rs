//! The managed Java runtime ([FR-SC-23], [ADR-88] §7, S-746): what the
//! opt-in Dependency-Check runs on, so no system Java is used or needed.
//!
//! It is [`Manifest::java`], a pinned and SHA-256-verified release artifact
//! installed by the release route into `java/<version>/` — never a tool of
//! its own. The first [Java tool](super::ToolEntry::needs_java) of an install
//! installs it; the consent names it as a line of its own while it is
//! missing; and it is removed once no installed tool needs it. A Java tool's
//! managed copy [resolves](super::resolve) only while the runtime does, and
//! runs with [`java_env`] pointing at it, whatever the user's own `JAVA_HOME`
//! says.
//!
//! [FR-SC-23]: ../../../docs/specs/requirements/FR-SC-23.md
//! [ADR-88]: ../../../docs/specs/architecture/decisions/ADR-88.md

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{resolve_entry, Installer, Manifest, Platform, ToolEntry};

impl Installer {
    /// The Java runtime's consent line, while it is not installed: `(name,
    /// version, host, download size)`.
    pub(crate) fn java_pending(&self, platform: Platform) -> Option<(String, String, String, u64)> {
        self.support_pending(&self.manifest.java, platform)
    }

    /// When no tool that needs it is left, remove the Java runtime.
    pub(super) fn reclaim_java_runtime(&self) {
        self.reclaim_support(ToolEntry::needs_java, &[self.manifest.java.tool.as_str()]);
    }
}

/// The managed Java runtime's home in `tools_dir` — the directory holding
/// `bin/java` — when it is installed.
pub fn managed_java_home(tools_dir: &Path, manifest: &Manifest) -> Option<PathBuf> {
    let java = resolve_entry(tools_dir, manifest, &manifest.java)?;
    Some(java.binary.parent()?.parent()?.to_path_buf())
}

/// The variables a Java tool's managed copy runs with: `JAVA_HOME` and
/// `JAVACMD` both at the runtime in `home`. Dependency-Check's launcher reads
/// `JAVACMD` first and `JAVA_HOME` next, so neither a user's own setting nor
/// a missing system Java can change which runtime it starts.
pub fn java_env(home: &Path) -> [(&'static str, OsString); 2] {
    [
        ("JAVA_HOME", home.as_os_str().to_os_string()),
        ("JAVACMD", home.join("bin").join("java").into_os_string()),
    ]
}
