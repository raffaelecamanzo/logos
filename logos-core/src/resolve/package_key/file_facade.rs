//! The facade class through which another language of a family reaches a
//! file's top-level functions (S-560, S-832, [FR-RS-31]): Java's `UtilsKt.f()`
//! for Kotlin's `Utils.kt`, or `Slugs.slug()` for a file declaring
//! `@file:JvmName("Slugs")`.
//!
//! Two facts name it, kept together so that every reader of
//! [`PackageLayout::file_facade`](super::PackageLayout::file_facade) — the
//! binder's facade classes and the reference classes' name universe
//! ([FR-RS-49]) — reads one name:
//!
//! - each extension's **suffix**, which its plugin declares (`file_facade =
//!   "Kt"`), and which makes the language a facade language at all;
//! - the name a file's own annotation **declares**, by path, which the store
//!   records per file (`files.facade`, migration 47) and which replaces the
//!   name the file's stem derives.
//!
//! [FR-RS-31]: ../../../../docs/specs/requirements/FR-RS-31.md
//! [FR-RS-49]: ../../../../docs/specs/requirements/FR-RS-49.md

use std::collections::HashMap;
use std::path::Path;

/// What names each file's facade class: the declared suffixes by extension and
/// the declared names by path. Built from either half and merged into a
/// layout by [`PackageLayout::with_file_facades`](super::PackageLayout::with_file_facades),
/// which a [`HashMap`] of suffixes converts into ([`From`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacades {
    /// Normalised extension → the suffix the stem-derived name ends in.
    suffixes: HashMap<String, String>,
    /// File path → the facade class its own annotation names (S-832).
    declared: HashMap<String, String>,
}

impl FileFacades {
    /// The facades the files at these paths declare (S-832), as
    /// [`GraphStore::file_facades`](crate::graph_store::GraphStore::file_facades)
    /// reads them: `(path, name)`. A name counts only for a file whose
    /// language declares a suffix.
    pub fn declared(facades: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            suffixes: HashMap::new(),
            declared: facades.into_iter().collect(),
        }
    }

    /// `other`'s suffixes and declared names added to these, a later name for
    /// one path or extension replacing an earlier one.
    pub(super) fn merge(&mut self, other: Self) {
        super::extend_normalised_map(&mut self.suffixes, other.suffixes);
        self.declared.extend(other.declared);
    }

    /// Whether the language of the file at `path` declares file facades.
    pub(super) fn declares(&self, path: &str) -> bool {
        super::extension(path).is_some_and(|ext| self.suffixes.contains_key(&ext))
    }

    /// The facade class of the file at `path`, `None` when its language
    /// declares none: the name the file declares, else its stem — every
    /// character that cannot spell a class name replaced by `_` and the first
    /// letter upper-cased — then the declared suffix.
    pub(super) fn of(&self, path: &str) -> Option<String> {
        let suffix = self.suffixes.get(&super::extension(path)?)?;
        if let Some(name) = self.declared.get(path) {
            return Some(name.clone());
        }
        let stem = Path::new(path).file_stem()?.to_str()?;
        let mut chars = stem.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' });
        let first = chars.next()?;
        Some(first.to_uppercase().chain(chars).chain(suffix.chars()).collect())
    }
}

/// Each extension's suffix (S-560; the map
/// [`LanguageRegistry::file_facades`](crate::plugin::LanguageRegistry::file_facades)
/// returns), no file declaring a name.
impl From<HashMap<String, String>> for FileFacades {
    fn from(suffixes: HashMap<String, String>) -> Self {
        Self {
            suffixes,
            declared: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::PackageLayout;
    use super::*;

    const SUFFIX: (&str, &str) = (".KT", "Kt");

    fn layout() -> PackageLayout {
        PackageLayout::default().with_file_facades(HashMap::from([(SUFFIX.0.to_string(), SUFFIX.1.to_string())]))
    }

    /// FR-RS-31 (S-832): a file's declared facade replaces the name its stem
    /// derives; a file declaring none keeps `<Stem>Kt`; and a name declared for
    /// a file of a language without facades names nothing.
    #[test]
    fn a_declared_facade_replaces_the_stem_derived_name_of_its_file_only() {
        let layout = layout().with_file_facades(FileFacades::declared([
            ("src/app/text/Strings.kt".to_string(), "Slugs".to_string()),
            ("src/app/Boot.java".to_string(), "Boot2".to_string()),
        ]));
        let cases = [
            ("src/app/text/Strings.kt", Some("Slugs")),
            ("src/app/text/Plain.kt", Some("PlainKt")),
            ("src/app/text/strings.kt", Some("StringsKt")),
            ("src/app/Boot.java", None),
        ];
        for (path, facade) in cases {
            assert_eq!(layout.file_facade(path).as_deref(), facade, "{path}");
        }
        assert!(layout.declares_file_facades("src/app/text/Strings.kt"));
        assert!(!layout.declares_file_facades("src/app/Boot.java"));
    }

    /// The two halves merge in either order: the declared names survive a later
    /// suffix merge, and a later declaration of one path replaces the earlier.
    #[test]
    fn suffixes_and_declared_names_merge_in_either_order() {
        let declared_first = PackageLayout::default()
            .with_file_facades(FileFacades::declared([("a/Strings.kt".to_string(), "One".to_string())]))
            .with_file_facades(HashMap::from([(SUFFIX.0.to_string(), SUFFIX.1.to_string())]))
            .with_file_facades(FileFacades::declared([("a/Strings.kt".to_string(), "Two".to_string())]));
        assert_eq!(declared_first.file_facade("a/Strings.kt").as_deref(), Some("Two"));
        assert_eq!(declared_first.file_facade("a/Other.kt").as_deref(), Some("OtherKt"));
    }
}
