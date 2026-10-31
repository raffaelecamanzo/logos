//! Pure, root-relative path helpers the resolution pass shares (S-531): the
//! binder's doc-link and specifier folds, and the Go module, `tsconfig` and
//! load-path (S-566) discovery — and the descriptor-name test extraction shares
//! with it (`names_suffixed_file`). A leaf — it depends on nothing in `resolve` — so each of them
//! can use it without depending on another.

use std::path::Path;

/// Fold a link `path_part`'s `.`/`..`/leading-`/` against `base_dir` (the
/// directory segments to resolve relative to) into a normalised
/// project-relative path, or `None` if it escapes the repository root.
///
/// Pure and deterministic — the link and its target agree because both sides
/// share this normalisation and the binder's `heading_slug`.
pub(super) fn fold_path(base_dir: &[&str], path_part: &str) -> Option<String> {
    // A leading `/` is repo-root-relative; otherwise resolve against `base_dir`.
    let mut segs: Vec<&str> = if path_part.starts_with('/') {
        Vec::new()
    } else {
        base_dir.to_vec()
    };
    for seg in path_part.split('/').filter(|s| !s.is_empty()) {
        match seg {
            "." => {}
            ".." => {
                // An escape above the repo root is not a resolvable target.
                segs.pop()?;
            }
            other => segs.push(other),
        }
    }
    Some(segs.join("/"))
}

/// The lower-cased extension of `path` (`""` when it has none) — the key the
/// path-specifier maps are built on ([`crate::plugin::LanguageRegistry::specifier_target_extensions`]).
pub(super) fn extension_of(path: &str) -> String {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// The directory segments of `file` (the path with its file name dropped).
pub(super) fn dir_segments(file: &str) -> Vec<&str> {
    let mut dir: Vec<&str> = file.split('/').collect();
    dir.pop(); // drop the file name, keep the directory
    dir.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Whether the project-relative `rel` names a file whose basename ends in
/// `suffix` and has a stem before it — exact and case-sensitive, as a
/// toolchain's own `*<suffix>` glob is: `x.avsc` and `rack.gemspec`, never
/// `x.avsc.bak`, `x.AVSC` or a bare `.gemspec`.
pub(crate) fn names_suffixed_file(rel: &str, suffix: &str) -> bool {
    let base = rel.rsplit('/').next().unwrap_or(rel);
    base.len() > suffix.len() && base.ends_with(suffix)
}

/// The directory part of a project-relative path (`""` for a root-level one).
pub(super) fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}
