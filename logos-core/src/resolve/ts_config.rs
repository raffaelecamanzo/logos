//! The `tsconfig.json`/`jsconfig.json` and workspace packages a tree declares —
//! what a bare TypeScript/JavaScript specifier binds through (S-531, [CR-173]
//! §3.2 (1), [FR-RS-02]). The twin of [`super::go_module`] for the TS family.
//!
//! A bare specifier (`@/features/auth`, `@acme/ui`, `react`) names no file by
//! itself. Two things can make it name a workspace file, and both are read off
//! the files the TypeScript compiler itself reads:
//!
//! - the **nearest** `tsconfig.json` (else `jsconfig.json`) above the importing
//!   file, with its `extends` chain followed inside the repository: its
//!   `compilerOptions.paths` and `compilerOptions.baseUrl` rewrite a matching
//!   specifier into a root-relative path, which then resolves the way a
//!   relative specifier does (extensions and `index` included). Each package of
//!   a monorepo is governed by its own config, so `@/` means a different `src/`
//!   per app;
//! - a **workspace package** — a `package.json` whose `name` is the specifier,
//!   or its prefix up to a `/` — resolves to that package's entry, tried in this
//!   order: `exports` (its `import`, `types` and `default` conditions), then
//!   `types`, `module`, `main`, and finally the package's `index`.
//!
//! What this module returns is an ordered list of **candidate paths**, never a
//! binding: the binder tries each against the indexed files and the first that
//! names exactly one decides ([NFR-RA-05]). A specifier nothing here rewrites —
//! an external package such as `react` — has no candidate and stays unbound.
//!
//! Only the configs and manifests that sit in a directory of an indexed file
//! of an opted-in language (`bare_specifiers = "tsconfig"`) are opened, plus
//! the files their `extends` chains name inside the repository; a tree with no
//! such file reads nothing. Everything unreadable or malformed is treated as
//! contributing nothing — losing an alias only leaves an import unbound, never
//! mis-bound.
//!
//! [CR-173]: ../../../docs/requests/CR-173-typescript-resolves-aliases-barrels-defaults-relations-and-injected-calls.md
//! [FR-RS-02]: ../../../docs/specs/requirements/FR-RS-02.md
//! [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use serde_json::Value;

use super::rel_path::{dir_segments, extension_of, fold_path, parent_of};

/// The config file names probed in each directory, in precedence order: the
/// compiler reads a `tsconfig.json` over a `jsconfig.json` beside it.
const CONFIG_NAMES: [&str; 2] = ["tsconfig.json", "jsconfig.json"];

/// How deep an `extends` chain is followed; a deeper (or cyclic) chain
/// contributes only what was read above it.
const MAX_EXTENDS_DEPTH: usize = 8;

/// The package entry fields read after `exports`, in the order they are tried.
const ENTRY_FIELDS: [&str; 3] = ["types", "module", "main"];

/// The `exports` conditions honoured, in the order they are tried.
const EXPORT_CONDITIONS: [&str; 3] = ["import", "types", "default"];

/// Everything a bare specifier of the opted-in languages resolves through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TsResolution {
    /// The extensions of the files governed here (the plugins declaring
    /// `bare_specifiers = "tsconfig"`). Empty: nothing is rewritten.
    extensions: HashSet<String>,
    /// Directory (`""` at the root) → the effective settings of the config
    /// found there — only configs that are the nearest of some indexed file.
    configs: BTreeMap<String, TsConfig>,
    /// The workspace packages, sorted by name then root.
    packages: Vec<WorkspacePackage>,
}

/// One config's effective resolution settings, its `extends` chain applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TsConfig {
    /// `compilerOptions.baseUrl`, root-relative (`""` at the root).
    base_url: Option<String>,
    /// `compilerOptions.paths`, in declaration order.
    paths: Vec<PathPattern>,
    /// The directory the `paths` substitutions are relative to: the effective
    /// `baseUrl`, else the directory of the config that declared `paths`.
    paths_base: String,
}

impl TsConfig {
    /// What this config rewrites `spec` into, in order: each substitution of
    /// the `paths` pattern it matches best (the one `*` taking the captured
    /// text), then `baseUrl`.
    fn rewrites(&self, spec: &str) -> Vec<String> {
        let mut out: Vec<String> = best_match(&self.paths, spec)
            .map(|(pattern, captured)| {
                pattern
                    .substitutions
                    .iter()
                    .map(|sub| join(&self.paths_base, &sub.replacen('*', captured, 1)))
                    .collect()
            })
            .unwrap_or_default();
        out.extend(self.base_url.iter().map(|base| join(base, spec)));
        out
    }
}

/// One `paths` entry or `exports` subpath key: a pattern with at most one
/// `*`, and its substitutions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PathPattern {
    /// The text before the `*` (the whole pattern when it has none).
    prefix: String,
    /// The text after the `*`, when the pattern has one.
    suffix: Option<String>,
    /// The substitutions, as written (`./src/*`), tried in order.
    substitutions: Vec<String>,
}

/// One workspace package: a `package.json` with a `name` inside the tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct WorkspacePackage {
    /// The declared `name` (`@acme/ui`).
    name: String,
    /// The root-relative directory holding the `package.json`.
    root: String,
    /// The entry candidates, root-relative, in the order they are tried.
    entry: Vec<String>,
    /// The `exports` subpath keys (`./button` → `button`, `./*` → `*`), each
    /// with its root-relative candidates — none for a `null` target, which
    /// excludes the subpath. `None` when `exports` is absent or not a subpath
    /// map — a subpath then resolves as a path under the package root.
    subpaths: Option<Vec<PathPattern>>,
}

impl TsResolution {
    /// Whether nothing here can rewrite a specifier.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.configs.is_empty() && self.packages.is_empty()
    }

    /// The root-relative paths a bare `spec` written in `source_file` may
    /// name, in the order they are tried — `None` when `source_file` is not a
    /// file of an opted-in language. The paths are unfolded (`a/./src/x`), may
    /// carry the entry's own extension (`packages/ui/src/index.ts`), and may
    /// name a directory whose `index` is meant; the binder folds and matches
    /// each ([`fold_path`]).
    ///
    /// The order is the compiler's: the best-matching `paths` pattern's
    /// substitutions (an exact pattern over a wildcard one, then the longest
    /// prefix), then `baseUrl`, then the workspace package the specifier names.
    pub(crate) fn candidates(&self, source_file: &str, spec: &str) -> Option<Vec<String>> {
        if !self.extensions.contains(&extension_of(source_file)) {
            return None;
        }
        let mut out = self.nearest(source_file).map(|c| c.rewrites(spec)).unwrap_or_default();
        out.extend(self.package_candidates(spec));
        Some(out)
    }

    /// The config governing `file`: the one in its own directory or the
    /// nearest ancestor that holds one.
    fn nearest(&self, file: &str) -> Option<&TsConfig> {
        let mut dir = parent_of(file);
        loop {
            if let Some(config) = self.configs.get(dir) {
                return Some(config);
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent_of(dir);
        }
    }

    /// The candidates of the workspace package `spec` names — by its whole
    /// name, or by a name followed by a `/subpath`. The longest such name wins;
    /// a name two packages declare names neither ([NFR-RA-05]).
    ///
    /// [NFR-RA-05]: ../../../docs/specs/requirements/NFR-RA-05.md
    fn package_candidates(&self, spec: &str) -> Vec<String> {
        let (package, subpath) = match self.named_package(spec) {
            Some(found) => found,
            None => return Vec::new(),
        };
        let Some(subpath) = subpath else {
            return package.entry.clone();
        };
        match &package.subpaths {
            None => vec![join(&package.root, subpath)],
            // The key the resolver picks — an exact key, else the pattern with
            // the longest prefix — decides, even when it lists no target: a
            // `null` target excludes the subpath. Every `*` of an `exports`
            // target takes the captured text.
            Some(subpaths) => best_match(subpaths, subpath)
                .map(|(key, captured)| key.substitutions.iter().map(|t| t.replace('*', captured)).collect())
                .unwrap_or_default(),
        }
    }

    /// The one package `spec` names, with the subpath after its name.
    fn named_package<'s>(&self, spec: &'s str) -> Option<(&WorkspacePackage, Option<&'s str>)> {
        let best = self
            .packages
            .iter()
            .filter(|p| spec == p.name || spec.starts_with(&format!("{}/", p.name)))
            .map(|p| p.name.len())
            .max()?;
        let mut named = self.packages.iter().filter(|p| p.name.len() == best && spec.starts_with(&p.name));
        let package = named.next()?;
        if named.next().is_some() {
            return None;
        }
        let subpath = spec[best..].strip_prefix('/').filter(|s| !s.is_empty());
        Some((package, subpath))
    }
}

/// Whether the root-relative file `rel` is one a bare specifier can bind
/// through, so a sync handed it re-binds the TypeScript imports although it
/// may index no such file (`pipeline::sync`, as for a `go.mod`). That is any
/// `.json` file: a `package.json`, a `tsconfig.json`/`jsconfig.json`, and an
/// `extends` base, which may be named anything (`configs/base.json`).
pub(crate) fn is_descriptor(rel: &str) -> bool {
    rel.rsplit('/').next().unwrap_or(rel).to_ascii_lowercase().ends_with(".json")
}

/// Every config and workspace package governing the files under `root` whose
/// extension is one of `extensions` (S-531); `files` is every indexed file.
///
/// Each directory is probed at most once for each kind of file. A config is
/// the nearest one above each governed file, its `extends` chain followed
/// inside the repository. A package is any `package.json` with a `name` in a
/// directory holding — or above — **any** indexed file, so a package of
/// configs only (`@repo/config` with its `base.json`) is found through its own
/// indexed `package.json` and can be extended. Without a governed file nothing
/// is read.
pub(crate) fn discover<'p>(
    root: &Path,
    extensions: HashSet<String>,
    files: impl IntoIterator<Item = &'p str>,
) -> TsResolution {
    if extensions.is_empty() {
        return TsResolution::default();
    }
    let files: Vec<&str> = files.into_iter().collect();
    let governed: Vec<&str> = files.iter().copied().filter(|f| extensions.contains(&extension_of(f))).collect();
    if governed.is_empty() {
        return TsResolution::default();
    }
    let reader = Reader { root };
    // Every directory holding an indexed file, and each above one: where a
    // workspace package may sit. An ancestor already present had its own
    // ancestors added with it.
    let mut dirs: HashSet<String> = HashSet::new();
    for file in &files {
        let mut dir = parent_of(file);
        while dirs.insert(dir.to_string()) && !dir.is_empty() {
            dir = parent_of(dir);
        }
    }
    let mut nearest: HashSet<String> = HashSet::new();
    let mut probed: HashMap<String, Option<String>> = HashMap::new();
    for file in governed {
        let mut dir = parent_of(file);
        loop {
            let config = probed
                .entry(dir.to_string())
                .or_insert_with(|| {
                    CONFIG_NAMES
                        .iter()
                        .map(|name| join(dir, name))
                        .find(|rel| root.join(rel).is_file())
                })
                .clone();
            if let Some(config) = config {
                nearest.insert(config);
                break; // the nearest config owns the file
            }
            if dir.is_empty() {
                break;
            }
            dir = parent_of(dir);
        }
    }
    let mut packages: Vec<WorkspacePackage> = dirs
        .iter()
        .filter_map(|dir| reader.package(dir))
        .collect();
    packages.sort();
    let mut resolution = TsResolution {
        extensions,
        configs: BTreeMap::new(),
        packages,
    };
    let configs: BTreeMap<String, TsConfig> = nearest
        .into_iter()
        .map(|rel| {
            let raw = reader.config(&rel, &resolution, &mut HashSet::new(), 0);
            (parent_of(&rel).to_string(), raw.effective())
        })
        .collect();
    resolution.configs = configs;
    resolution
}

/// One config's settings before its effective `paths` base is decided.
#[derive(Debug, Clone, Default)]
struct RawConfig {
    /// `baseUrl`, root-relative.
    base_url: Option<String>,
    /// `paths`, with the directory of the config that declared them.
    paths: Option<(String, Vec<PathPattern>)>,
}

impl RawConfig {
    /// `over`'s settings replace this one's, each key wholesale, as an
    /// extending config's do its base's.
    fn overlay(&mut self, over: RawConfig) {
        if over.base_url.is_some() {
            self.base_url = over.base_url;
        }
        if over.paths.is_some() {
            self.paths = over.paths;
        }
    }

    fn effective(self) -> TsConfig {
        let (declared_in, paths) = self.paths.unwrap_or_default();
        TsConfig {
            paths_base: self.base_url.clone().unwrap_or(declared_in),
            base_url: self.base_url,
            paths,
        }
    }
}

/// Reads configs and manifests under one root.
struct Reader<'r> {
    root: &'r Path,
}

impl Reader<'_> {
    /// The JSON (a leading byte-order mark, comments and trailing commas
    /// tolerated, as the compiler and package managers read it) of the
    /// root-relative file `rel`, or `None` when it is absent, unreadable or
    /// malformed.
    fn json(&self, rel: &str) -> Option<Value> {
        let text = std::fs::read_to_string(self.root.join(rel)).ok()?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        serde_json::from_str(&strip_jsonc(text)).ok()
    }

    /// The workspace package whose `package.json` sits in `dir`, if it names one.
    fn package(&self, dir: &str) -> Option<WorkspacePackage> {
        let json = self.json(&join(dir, "package.json"))?;
        let name = json.get("name")?.as_str()?.trim();
        if name.is_empty() {
            return None;
        }
        let at = |target: &str| join(dir, target);
        let (mut entry, subpaths) = json.get("exports").map(|e| read_exports(e, dir)).unwrap_or_default();
        entry.extend(
            ENTRY_FIELDS
                .iter()
                .filter_map(|field| json.get(*field)?.as_str())
                .map(at),
        );
        entry.push(dir.to_string()); // the package's own `index`
        Some(WorkspacePackage {
            name: name.to_string(),
            root: dir.to_string(),
            entry,
            subpaths,
        })
    }

    /// The settings of the config at `rel`, its `extends` chain applied.
    fn config(&self, rel: &str, packages: &TsResolution, visiting: &mut HashSet<String>, depth: usize) -> RawConfig {
        let mut settings = RawConfig::default();
        let Some(json) = self.json(rel) else {
            return settings; // present but unreadable: it owns the file, and adds nothing
        };
        visiting.insert(rel.to_string());
        let dir = parent_of(rel);
        let bases: Vec<&str> = match json.get("extends") {
            Some(Value::String(base)) => vec![base.as_str()],
            Some(Value::Array(bases)) => bases.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for base in bases {
            let Some(base_rel) = extends_target(rel, base, packages) else { continue };
            if depth < MAX_EXTENDS_DEPTH && !visiting.contains(&base_rel) {
                settings.overlay(self.config(&base_rel, packages, visiting, depth + 1));
            }
        }
        visiting.remove(rel);
        let options = json.get("compilerOptions");
        let base_url = options
            .and_then(|o| o.get("baseUrl"))
            .and_then(Value::as_str)
            .and_then(|b| fold_path(&dir_segments(rel), b));
        let paths = options.and_then(|o| o.get("paths")).and_then(Value::as_object).map(|map| {
            let patterns = map
                .iter()
                .filter_map(|(pattern, subs)| {
                    let substitutions: Vec<String> = subs
                        .as_array()?
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                    PathPattern::new(pattern, substitutions)
                })
                .collect();
            (dir.to_string(), patterns)
        });
        settings.overlay(RawConfig { base_url, paths });
        settings
    }
}

impl PathPattern {
    /// A pattern with at most one `*`; any other is ignored, as the compiler
    /// rejects it.
    fn new(pattern: &str, substitutions: Vec<String>) -> Option<PathPattern> {
        let (prefix, suffix) = match pattern.split_once('*') {
            Some((_, post)) if post.contains('*') => return None,
            Some((pre, post)) => (pre.to_string(), Some(post.to_string())),
            None => (pattern.to_string(), None),
        };
        Some(PathPattern {
            prefix,
            suffix,
            substitutions,
        })
    }
}

/// The pattern `spec` matches best, with the text its `*` captured: an exact
/// pattern first, else the wildcard pattern with the longest prefix — the
/// first declared on a tie. The rule the compiler applies to `paths` and the
/// resolver to `exports` subpath keys alike.
fn best_match<'a, 's>(patterns: &'a [PathPattern], spec: &'s str) -> Option<(&'a PathPattern, &'s str)> {
    if let Some(exact) = patterns.iter().find(|p| p.suffix.is_none() && p.prefix == spec) {
        return Some((exact, ""));
    }
    let mut best: Option<(&PathPattern, &str)> = None;
    for pattern in patterns {
        let Some(suffix) = &pattern.suffix else { continue };
        let Some(captured) = spec
            .strip_prefix(pattern.prefix.as_str())
            .and_then(|rest| rest.strip_suffix(suffix.as_str()))
        else {
            continue;
        };
        if best.is_none_or(|(b, _)| pattern.prefix.len() > b.prefix.len()) {
            best = Some((pattern, captured));
        }
    }
    best
}

/// The root-relative config an `extends` entry in the config `config_rel` names:
/// a relative path (with `.json` added when it has no extension), or a file of
/// a workspace package (`@acme/tsconfig/base.json`, or the package's
/// `tsconfig.json` when only its name is given). Anything else — a package
/// outside the repository — contributes nothing.
fn extends_target(config_rel: &str, base: &str, packages: &TsResolution) -> Option<String> {
    let with_json = |path: String| {
        if path.ends_with(".json") {
            path
        } else {
            format!("{path}.json")
        }
    };
    if base.starts_with("./") || base.starts_with("../") {
        return fold_path(&dir_segments(config_rel), base).map(with_json);
    }
    let (package, file) = packages.named_package(base)?;
    let root = package.root.as_str();
    Some(match file {
        Some(file) => with_json(join(root, file)),
        None => join(root, "tsconfig.json"),
    })
}

/// A package's `exports` read from its directory `dir`: the root-relative
/// candidates of its `.` entry, and its subpath keys when it is a subpath map
/// (a key starting with `.`) — `None` for a bare target or a top-level
/// condition object, which declare no subpath.
fn read_exports(exports: &Value, dir: &str) -> (Vec<String>, Option<Vec<PathPattern>>) {
    let targets_of = |value: &Value| -> Vec<String> {
        let mut targets = Vec::new();
        condition_targets(value, &mut targets);
        targets.iter().map(|t| join(dir, t)).collect()
    };
    let Some(map) = exports.as_object().filter(|m| m.keys().any(|k| k.starts_with('.'))) else {
        return (targets_of(exports), None);
    };
    let entry = map.get(".").map(targets_of).unwrap_or_default();
    let subpaths = map
        .iter()
        .filter_map(|(key, value)| PathPattern::new(key.strip_prefix("./")?, targets_of(value)))
        .collect();
    (entry, Some(subpaths))
}

/// The targets of an `exports` value under the honoured conditions, in order:
/// a string, a fallback array, or a condition object (nested ones included).
/// Its nesting is bounded by the parser's own depth limit.
fn condition_targets(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(target) => out.push(target.clone()),
        Value::Array(values) => values.iter().for_each(|v| condition_targets(v, out)),
        Value::Object(map) => {
            for condition in EXPORT_CONDITIONS {
                if let Some(v) = map.get(condition) {
                    condition_targets(v, out);
                }
            }
        }
        _ => {}
    }
}

/// `text` with JSONC's `//` and `/* */` comments and its trailing commas
/// removed, string literals left intact — the dialect `tsconfig.json` is
/// written in. Two linear passes: comments first, so a comment between a
/// trailing comma and its closing bracket hides nothing from the second.
fn strip_jsonc(text: &str) -> String {
    strip_trailing_commas(&strip_comments(text))
}

/// `text` with its `//` and `/* */` comments removed (a block comment leaves
/// one space), string literals copied whole.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('"', _) => copy_string(&mut chars, &mut out),
            ('/', Some('/')) => {
                while chars.next_if(|&n| n != '\n').is_some() {}
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = '\0';
                let _ = chars.by_ref().find(|&n| std::mem::replace(&mut last, n) == '*' && n == '/');
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

/// `text` with every comma whose next significant character closes a
/// container dropped, string literals copied whole.
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => copy_string(&mut chars, &mut out),
            ',' if matches!(chars.clone().find(|n| !n.is_whitespace()), Some('}' | ']')) => {}
            _ => out.push(c),
        }
    }
    out
}

/// Copy one string literal whose opening `"` was just read: the quote, its
/// text (escapes kept with what they escape) and its closing `"`.
fn copy_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    out.push('"');
    while let Some(c) = chars.next() {
        out.push(c);
        match c {
            '\\' => out.extend(chars.next()),
            '"' => return,
            _ => {}
        }
    }
}

/// `rest` under the root-relative directory `base`.
fn join(base: &str, rest: &str) -> String {
    if base.is_empty() {
        rest.to_string()
    } else {
        format!("{base}/{rest}")
    }
}

#[cfg(test)]
#[path = "ts_config_tests.rs"]
mod tests;
