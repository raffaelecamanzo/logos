//! The **package-shaped** module key ([CR-149], [FR-RS-01]) — the one place a
//! package is derived from a file path.
//!
//! The binder's default module model is Rust's ([`module_key_for_file`]): the
//! directory before the last `src/` names the crate, every later directory is a
//! module. A language whose plugin declares `[package_modules]`
//! ([`PackageModules`](crate::plugin::PackageModules)) is keyed instead by the
//! path *after* its source root, so `mailbox-core/src/main/java/com/x/Svc.java`
//! is `("mailbox_core", [com, x, Svc])` — the fully-qualified name its imports
//! spell — where the default model made it `[main, java, com, x, Svc]`.
//!
//! **This is the single source of FQN derivation.** Anything that needs the
//! package a file declares, or the fully-qualified name of a type it declares,
//! asks [`PackageLayout::package_of`] / [`PackageLayout::type_fqn`] — never a
//! second split of the path (the sprint-81 risk register names the divergence a
//! re-derivation would cause).
//!
//! [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
//! [FR-RS-01]: ../../../docs/specs/requirements/FR-RS-01.md

use std::collections::HashMap;
use std::path::Path;

use crate::plugin::{LanguagePlugin, LanguageRegistry};

/// A module identity: `(crate name, module path segments)` — the binder's
/// `ModKey`.
pub type ModuleKey = (String, Vec<String>);

/// Which file extensions are package-shaped, and under which source roots.
///
/// Empty ([`Default`]) is the default module model for every file, which is
/// what a synthetic graph with no registry behind it is resolved with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageLayout {
    /// Normalised extension → its language's source roots, each split into path
    /// segments, in declaration order.
    roots_by_ext: HashMap<String, Vec<Vec<String>>>,
}

impl PackageLayout {
    /// A layout from `extension → source roots` (`"src/main/java"` form), the
    /// shape [`LanguageRegistry::package_source_roots`] returns.
    pub fn new(roots_by_ext: HashMap<String, Vec<String>>) -> Self {
        let roots_by_ext = roots_by_ext
            .into_iter()
            .map(|(ext, roots)| {
                let split = roots
                    .iter()
                    .map(|r| {
                        r.split('/')
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .collect();
                (ext.to_ascii_lowercase(), split)
            })
            .collect();
        Self { roots_by_ext }
    }

    /// The layout the loaded plugins declare.
    pub fn from_registry(registry: &LanguageRegistry) -> Self {
        Self::new(registry.package_source_roots())
    }

    /// The layout one plugin declares — [`from_registry`](Self::from_registry)
    /// narrowed to a single language, for the single-file
    /// [`extract`](crate::extract::extract) entry point, which is handed a
    /// plugin and no registry. For every extension of that plugin it is the
    /// registry's layout exactly.
    pub fn from_plugin(plugin: &dyn LanguagePlugin) -> Self {
        let Some(pm) = plugin.semantics().package_modules.as_ref() else {
            return Self::default();
        };
        Self::new(
            plugin
                .extensions()
                .iter()
                .map(|ext| (ext.trim_start_matches('.').to_string(), pm.source_roots.clone()))
                .collect(),
        )
    }

    /// `true` when `path`'s language declares a package-shaped module path.
    pub fn is_package_shaped(&self, path: &str) -> bool {
        self.roots_of(path).is_some()
    }

    /// The module key of the file at the project-relative `path`.
    ///
    /// For a file of a package-shaped language under one of its source roots:
    /// the crate is the directory before the root (normalised `-` → `_`,
    /// `crate` when the root starts the path), and the modules are the
    /// directories after it plus the file stem. A file outside every root, and
    /// every file of any other language, keeps the default model's key — for a
    /// package-shaped language only the file stem is always kept, because a
    /// type is never the `mod`/`lib`/`main` marker the Rust rule folds away.
    ///
    /// The **rightmost** match of any root wins, as the default model takes the
    /// last `src/`, so a nested module's own root is the one that counts.
    pub fn module_key(&self, path: &str) -> ModuleKey {
        let (crate_name, mut mods, stem) = match self.roots_of(path) {
            Some(roots) => rooted(path, roots).unwrap_or_else(|| default_layout(path)),
            None => return module_key_for_file(path),
        };
        if let Some(stem) = stem {
            mods.push(stem);
        }
        (crate_name, mods)
    }

    /// The package the file at `path` declares by its location — its module key
    /// without the file stem — or `None` when its language is not
    /// package-shaped. `src/main/java/com/x/Svc.java` → `[com, x]`.
    pub fn package_of(&self, path: &str) -> Option<Vec<String>> {
        self.roots_of(path)?;
        let (_, mut mods) = self.module_key(path);
        mods.pop();
        Some(mods)
    }

    /// The fully-qualified name of a top-level type named `type_name` declared in
    /// the file at `path` — its [`package_of`](Self::package_of) plus the name —
    /// or `None` when the file's language is not package-shaped.
    pub fn type_fqn(&self, path: &str, type_name: &str) -> Option<Vec<String>> {
        let mut fqn = self.package_of(path)?;
        fqn.push(type_name.to_string());
        Some(fqn)
    }

    /// The source root the file at `path` sits under — the root whose match
    /// keyed it ([`module_key`](Self::module_key)), as its path segments — or
    /// `None` when its language is not package-shaped or no root precedes it.
    /// `svc/src/test/java/com/x/SvcTest.java` → `[src, test, java]`.
    pub fn source_root(&self, path: &str) -> Option<&[String]> {
        let roots = self.roots_of(path)?;
        let (_, _, idx) = matched_root(path, roots)?;
        Some(&roots[idx])
    }

    fn roots_of(&self, path: &str) -> Option<&[Vec<String>]> {
        let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
        self.roots_by_ext.get(&ext).map(Vec::as_slice)
    }
}

/// `(crate, package directories, stem)` of `path` under the rightmost match of
/// any of `roots`, or `None` when no root precedes the file name.
fn rooted(path: &str, roots: &[Vec<String>]) -> Option<(String, Vec<String>, Option<String>)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (file, dirs) = segs.split_last()?;
    let (start, len, _) = matched_root(path, roots)?;
    let crate_name = match start {
        0 => "crate".to_string(),
        i => normalize_crate(dirs[i - 1]),
    };
    let package = dirs[start + len..]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    Some((crate_name, package, Some(file_stem(file))))
}

/// `(start, length, root index)` of the rightmost match of any of `roots`
/// among `path`'s directories — the one match both [`rooted`] and
/// [`PackageLayout::source_root`] read, so the root that keys a file and the
/// root reported for it can never differ. Of two roots matching at one start,
/// the longer wins, then the later-declared.
fn matched_root(path: &str, roots: &[Vec<String>]) -> Option<(usize, usize, usize)> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (_, dirs) = segs.split_last()?;
    roots
        .iter()
        .enumerate()
        .filter(|(_, r)| !r.is_empty() && r.len() <= dirs.len())
        .flat_map(|(idx, r)| {
            (0..=dirs.len() - r.len())
                .filter(move |&i| dirs[i..i + r.len()].iter().zip(r).all(|(d, s)| *d == s))
                .map(move |i| (i, r.len(), idx))
        })
        .max()
}

/// Derive a file's module identity from its project-relative path, by the
/// **default** (Rust) module model.
///
/// The segment before the last `src/` names the crate (normalised `-` → `_`,
/// `crate` when there is none); segments after it are modules, with the
/// `mod`/`lib`/`main` stems naming their enclosing module rather than adding a
/// segment. `logos-core/src/extract/mod.rs` → `("logos_core", ["extract"])`.
///
/// A package-shaped language's files are keyed by
/// [`PackageLayout::module_key`] instead ([CR-149]), which falls back to this
/// for every other file. Rust's model and the package model live side by side
/// in this module, so there is one home for path → module key.
///
/// [CR-149]: ../../../docs/requests/CR-149-java-imports-and-type-relations-never-bind.md
pub(crate) fn module_key_for_file(path: &str) -> ModuleKey {
    let (crate_name, mut mods, stem) = default_layout(path);
    if let Some(stem) = stem.filter(|s| !matches!(s.as_str(), "mod" | "lib" | "main")) {
        mods.push(stem);
    }
    (crate_name, mods)
}

/// The default model's parts of `path`: the crate, the module directories after
/// its last `src/`, and the file stem (`None` when nothing follows the crate
/// root) — before the `mod`/`lib`/`main` fold [`module_key_for_file`] applies.
/// Shared with [`PackageLayout::module_key`] so the two models agree on every
/// file outside a package root.
fn default_layout(path: &str) -> (String, Vec<String>, Option<String>) {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let (crate_name, mods_start) = match segs.iter().rposition(|s| *s == "src") {
        Some(0) => ("crate".to_string(), 1),
        Some(pos) => (normalize_crate(segs[pos - 1]), pos + 1),
        None => ("crate".to_string(), 0),
    };
    let mut mods: Vec<String> = segs
        .get(mods_start..)
        .unwrap_or_default()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let stem = mods.pop().map(|last| file_stem(&last));
    (crate_name, mods, stem)
}

/// Normalise a crate directory name to its extern-path form (`-` → `_`) —
/// shared by both models, and by the binder's crate-name path heads.
pub(crate) fn normalize_crate(name: &str) -> String {
    name.replace('-', "_")
}

/// A file name without its extension (`Svc.java` → `Svc`; the name itself when
/// it has none) — the module stem both models append.
fn file_stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
        .to_string()
}


// The fixtures are Maven paths, so the layout is the one the loaded Java
// descriptor declares — read from the registry, never spelled here: this
// directory holds no language id (the `jvm_parity` guard).
#[cfg(all(test, feature = "lang-java"))]
mod tests {
    use super::*;

    fn java() -> PackageLayout {
        let tmp = tempfile::tempdir().expect("tempdir");
        PackageLayout::from_registry(&LanguageRegistry::load(tmp.path()).expect("registry loads"))
    }

    /// `(crate, modules)` from a `/`-joined module path (`""` for none).
    fn key(c: &str, mods: &str) -> ModuleKey {
        (
            c.to_string(),
            mods.split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        )
    }

    #[test]
    fn a_file_under_a_source_root_is_keyed_by_its_package() {
        let l = java();
        assert_eq!(
            l.module_key("src/main/java/com/x/Svc.java"),
            key("crate", "com/x/Svc")
        );
        assert_eq!(
            l.module_key("src/test/java/com/x/SvcTest.java"),
            key("crate", "com/x/SvcTest")
        );
        // A multi-module Maven repository keeps each module's crate.
        assert_eq!(
            l.module_key("mailbox-core/src/main/java/com/x/Svc.java"),
            key("mailbox_core", "com/x/Svc")
        );
        // The default package: no directory after the root.
        assert_eq!(
            l.module_key("src/main/java/Main.java"),
            key("crate", "Main")
        );
        assert_eq!(l.package_of("src/main/java/Main.java"), Some(vec![]));
    }

    #[test]
    fn a_package_that_contains_a_src_directory_keeps_its_module_crate() {
        // The default model takes the LAST `src/` as the crate boundary, which
        // would make `com` the crate here; the root match does not.
        assert_eq!(
            java().module_key("app/src/main/java/com/src/Foo.java"),
            key("app", "com/src/Foo")
        );
    }

    #[test]
    fn a_file_outside_every_root_keeps_the_default_key() {
        let l = java();
        // A flat layout and a `src/`-rooted one are already package-shaped.
        assert_eq!(l.module_key("com/x/Svc.java"), key("crate", "com/x/Svc"));
        assert_eq!(
            l.module_key("src/com/x/Svc.java"),
            key("crate", "com/x/Svc")
        );
        assert_eq!(
            l.package_of("src/com/x/Svc.java"),
            Some(vec!["com".into(), "x".into()])
        );
        // A root is matched as whole segments, never as a substring.
        assert_eq!(
            l.module_key("mysrc/main/java/com/Svc.java"),
            key("crate", "mysrc/main/java/com/Svc")
        );
    }

    #[test]
    fn every_other_language_keeps_the_default_key_byte_for_byte() {
        let l = java();
        for path in [
            "logos-core/src/extract/mod.rs",
            "src/main.rs",
            "src/lib.rs",
            "a/src/main/java/b.rs",
            "src/main/java/x.py",
        ] {
            assert_eq!(l.module_key(path), module_key_for_file(path), "{path}");
            assert_eq!(l.package_of(path), None, "{path}");
        }
        assert_eq!(
            l.module_key("logos-core/src/extract/mod.rs"),
            key("logos_core", "extract")
        );
        // …and an empty layout is the default model for Java too.
        let none = PackageLayout::default();
        assert_eq!(
            none.module_key("src/main/java/com/x/Svc.java"),
            key("crate", "main/java/com/x/Svc")
        );
        assert!(!none.is_package_shaped("src/main/java/com/x/Svc.java"));
    }

    #[test]
    fn a_type_fqn_is_its_files_package_plus_its_own_name() {
        let l = java();
        assert_eq!(
            l.type_fqn("svc/src/main/java/com/x/Svc.java", "Extra"),
            Some(vec!["com".into(), "x".into(), "Extra".into()])
        );
        assert_eq!(l.type_fqn("src/lib.rs", "Extra"), None);
    }
}
