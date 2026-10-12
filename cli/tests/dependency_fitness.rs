//! No core crate depends on an extension crate (S-645, CR-213, [ADR-74] §3).
//!
//! Chat and the wiki are extensions a build can leave out (CR-214 builds on that). That
//! holds only while no core crate names them, so this is the guard: `logos-core`, `logos-ext`,
//! `mcp`, `web` and the `cli` **library** reach none of `logos-wiki`, `logos-chat`,
//! `chat-agent`, `wiki-agent` or `agent-core`. A failure names the offending edge.
//!
//! # Read from `cargo metadata`, never from a list of edges
//!
//! The dependency graph is `cargo metadata --no-deps`' declared dependencies of the
//! workspace members. Every edge kind counts at the first hop — a `dev` edge from
//! `web` to `logos-wiki` would make the core crate's own tests need the extension
//! — and `normal`/`build` edges onward, transitively, so `web → X → logos-wiki`
//! fails as surely as `web → logos-wiki`. An optional edge counts too: one feature
//! away from linked is linked.
//!
//! # The `cli` library, which shares a package with the binary
//!
//! `logos_cli` (the library) and `logos` (the binary) are one package, and cargo
//! declares dependencies per package, not per target: `cargo metadata` cannot say
//! which target uses an edge. The package legitimately depends on the extension
//! crates — the binary's composition root registers them — so the graph check would
//! either fail on that or have to exempt the whole package, library included.
//!
//! So the library is held to the rule at the level where the distinction exists, its
//! source, still located through `cargo metadata`: the package's `lib` target root and
//! every module it declares, minus `#[cfg(test)]` modules (the unit tests read the
//! binary's composition, as the surface snapshots do). No library module may name an
//! extension crate — by its library name, or by any name the package imports it
//! under. The binary's own modules (`main.rs` → `composition.rs`) are the one place
//! that may, and the scan of them is this check's proof it can see a name at all.
//! Splitting the library into its own package would make the graph check sufficient
//! alone; that is a packaging change for CR-214, not a test's.
//!
//! The package's edges to the extension crates must also stay optional, so a build
//! without their features links none of them (`--no-default-features --features
//! lang-all,ui`, the no-extension build).
//!
//! [ADR-74]: ../../docs/specs/architecture/decisions/ADR-74.md

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The extension crates: chat, the wiki, and the agent crates only they use.
const EXTENSION_CRATES: &[&str] = &["logos-wiki", "logos-chat", "chat-agent", "wiki-agent", "agent-core"];

/// The core crates, which must reach none of them.
const CORE_CRATES: &[&str] = &["logos-core", "logos-ext", "mcp", "web"];

/// The package holding the `cli` library and the `logos` binary.
const CLI_PACKAGE: &str = "logos";

/// One declared dependency of a workspace member.
#[derive(Debug, Clone)]
struct Edge {
    to: String,
    /// `normal`, `dev` or `build`.
    kind: String,
    optional: bool,
    /// The name the dependent imports it under, when renamed.
    rename: Option<String>,
}

impl Edge {
    fn describe(&self) -> String {
        format!("{} ({}{})", self.to, self.kind, if self.optional { ", optional" } else { "" })
    }
}

struct Target {
    name: String,
    kinds: Vec<String>,
    src_path: PathBuf,
}

struct Package {
    edges: Vec<Edge>,
    targets: Vec<Target>,
}

/// The workspace members and their declared dependencies on one another.
struct Workspace(HashMap<String, Package>);

impl Workspace {
    fn from_metadata(metadata: &Value) -> Self {
        let packages = metadata["packages"].as_array().expect("cargo metadata lists packages");
        let names: BTreeSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
        let mut members = HashMap::new();
        for package in packages {
            let edges = package["dependencies"]
                .as_array()
                .expect("a dependency list")
                .iter()
                .filter(|d| names.contains(d["name"].as_str().expect("a dependency name")))
                .map(|d| Edge {
                    to: d["name"].as_str().expect("a name").to_string(),
                    kind: d["kind"].as_str().unwrap_or("normal").to_string(),
                    optional: d["optional"].as_bool().expect("an optional flag"),
                    rename: d["rename"].as_str().map(str::to_string),
                })
                .collect();
            let targets = package["targets"]
                .as_array()
                .expect("a target list")
                .iter()
                .map(|t| Target {
                    name: t["name"].as_str().expect("a target name").to_string(),
                    kinds: t["kind"].as_array().expect("kinds").iter().filter_map(Value::as_str).map(str::to_string).collect(),
                    src_path: PathBuf::from(t["src_path"].as_str().expect("a source path")),
                })
                .collect();
            members.insert(package["name"].as_str().expect("a package name").to_string(), Package { edges, targets });
        }
        Workspace(members)
    }

    fn package(&self, name: &str) -> &Package {
        self.0.get(name).unwrap_or_else(|| panic!("`{name}` is not a workspace member"))
    }

    /// The first path from `root` to an extension crate, as edges: any kind at the
    /// first hop, `normal`/`build` after (a dependency's dev edges are not inherited).
    fn paths_to_extensions(&self, root: &str) -> Vec<Vec<(String, Edge)>> {
        let mut found = Vec::new();
        let mut queue: VecDeque<(String, Vec<(String, Edge)>)> = VecDeque::from([(root.to_string(), Vec::new())]);
        let mut seen = BTreeSet::from([root.to_string()]);
        while let Some((from, path)) = queue.pop_front() {
            for edge in &self.package(&from).edges {
                if !path.is_empty() && edge.kind == "dev" {
                    continue;
                }
                let mut next = path.clone();
                next.push((from.clone(), edge.clone()));
                if EXTENSION_CRATES.contains(&edge.to.as_str()) {
                    found.push(next);
                } else if seen.insert(edge.to.clone()) {
                    queue.push_back((edge.to.clone(), next));
                }
            }
        }
        found
    }

    fn target(&self, package: &str, kind: &str) -> &Target {
        self.package(package)
            .targets
            .iter()
            .find(|t| t.kinds.iter().any(|k| k == kind))
            .unwrap_or_else(|| panic!("`{package}` has a {kind} target"))
    }
}

fn render_path(path: &[(String, Edge)]) -> String {
    let mut out = path[0].0.clone();
    for (_, edge) in path {
        out.push_str(&format!(" → {}", edge.describe()));
    }
    out
}

fn metadata() -> Value {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("`cargo metadata` runs");
    assert!(output.status.success(), "`cargo metadata` failed:\n{}", String::from_utf8_lossy(&output.stderr));
    serde_json::from_slice(&output.stdout).expect("cargo metadata is JSON")
}

/// Every edge from a core crate to an extension crate, rendered as its path.
fn core_violations(workspace: &Workspace) -> Vec<String> {
    CORE_CRATES.iter().flat_map(|root| workspace.paths_to_extensions(root)).map(|p| render_path(&p)).collect()
}

#[test]
fn no_core_crate_depends_on_an_extension_crate() {
    let workspace = Workspace::from_metadata(&metadata());
    // Every name the rule speaks of is a real member, or the rule is vacuous for it.
    for name in EXTENSION_CRATES.iter().chain(CORE_CRATES).chain([&CLI_PACKAGE]) {
        workspace.package(name);
    }
    let violations = core_violations(&workspace);
    assert!(
        violations.is_empty(),
        "a core crate depends on an extension crate (CR-213, ADR-74 §3) — the edge runs the \
         wrong way, and a build could no longer leave the extension out:\n  {}",
        violations.join("\n  ")
    );
}

/// The check fires on a real graph: the extension crates' own edges, read by the same
/// search, reach one another (`logos-chat → agent-core`, `wiki-agent → logos-wiki`).
#[test]
fn the_search_finds_the_extensions_own_edges() {
    let workspace = Workspace::from_metadata(&metadata());
    for (root, expected) in [
        ("logos-chat", "logos-chat → agent-core (normal)"),
        ("wiki-agent", "wiki-agent → logos-wiki (normal)"),
        (CLI_PACKAGE, "logos → logos-chat (normal, optional)"),
    ] {
        let paths: Vec<String> = workspace.paths_to_extensions(root).iter().map(|p| render_path(p)).collect();
        assert!(paths.contains(&expected.to_string()), "{root}: {paths:?}");
    }
}

/// A transitive edge and a dev edge are both caught, and named hop by hop — on a
/// graph that differs from the real one by exactly one declared dependency.
#[test]
fn a_transitive_or_dev_edge_is_named_hop_by_hop() {
    let mut metadata = metadata();
    let add_edge = |metadata: &mut Value, from: &str, to: &str, kind: Option<&str>| {
        let package = metadata["packages"]
            .as_array_mut()
            .expect("packages")
            .iter_mut()
            .find(|p| p["name"] == from)
            .expect("the package");
        package["dependencies"].as_array_mut().expect("deps").push(serde_json::json!({
            "name": to, "kind": kind, "optional": false, "rename": null,
        }));
    };
    add_edge(&mut metadata, "logos-ext", "wiki-agent", None);
    add_edge(&mut metadata, "logos-core", "logos-chat", Some("dev"));
    let violations = core_violations(&Workspace::from_metadata(&metadata));
    assert!(violations.contains(&"logos-ext → wiki-agent (normal)".to_string()), "{violations:?}");
    assert!(violations.contains(&"web → logos-ext (normal) → wiki-agent (normal)".to_string()), "{violations:?}");
    assert!(violations.contains(&"logos-core → logos-chat (dev)".to_string()), "{violations:?}");
    // A dependency's dev edge is not inherited: `mcp → logos-core` does not reach it.
    assert!(!violations.iter().any(|v| v.starts_with("mcp → logos-core (normal) → logos-chat")), "{violations:?}");
}

// ── The cli library ─────────────────────────────────────────────────────────

/// The identifiers the `cli` package could name an extension crate by: each
/// extension's library name, and any rename the package imports one under.
fn extension_identifiers(workspace: &Workspace) -> BTreeSet<String> {
    let mut names: BTreeSet<String> =
        EXTENSION_CRATES.iter().map(|c| workspace.target(c, "lib").name.clone()).collect();
    for edge in &workspace.package(CLI_PACKAGE).edges {
        if let (true, Some(rename)) = (EXTENSION_CRATES.contains(&edge.to.as_str()), &edge.rename) {
            names.insert(rename.replace('-', "_"));
        }
    }
    names
}

/// `source` with every comment blanked and, when `strings` is set, the inside of
/// every string and char literal too — byte for byte, newlines kept, so an offset or
/// a line number in the result is the source's. Comments are found outside literals
/// only, so `"https://…"` hides nothing after it; literals are blanked for the
/// structural searches (`{`, `;`, `#[cfg(test)]`) a brace inside a string would fool.
fn blank(source: &str, strings: bool) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let fill = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let end = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
                fill(&mut out, i, end);
                i = end;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let end = source[i + 2..].find("*/").map_or(bytes.len(), |n| i + 2 + n + 2);
                fill(&mut out, i, end);
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                if strings {
                    fill(&mut out, i + 1, j.min(bytes.len()));
                }
                i = j + 1;
            }
            // A char literal (`'"'`, `'\''`, `'{'`), never a lifetime (`'a`).
            b'\'' if bytes.get(i + 2) == Some(&b'\'') || (bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\'')) => {
                let end = if bytes[i + 1] == b'\\' { i + 4 } else { i + 3 };
                if strings {
                    fill(&mut out, i + 1, end - 1);
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    String::from_utf8(out).expect("only ASCII bytes were replaced, and only whole literals or comments")
}

/// `source` without comments and without `#[cfg(test)]` items — an inline test
/// module's balanced body, or an out-of-line `mod x;` — line numbers kept.
fn production_code(source: &str) -> String {
    let code = blank(source, false);
    let structure = blank(source, true);
    let mut out = String::new();
    let mut from = 0;
    while let Some(found) = structure[from..].find("#[cfg(test)]") {
        let at = from + found;
        out.push_str(&code[from..at]);
        let after = &structure[at..];
        // The attribute's item: an inline module's balanced body is dropped; any
        // other item (an out-of-line `mod x;` among them) ends at its `;`.
        let end = match (after.find('{'), after.find(';')) {
            (Some(b), Some(s)) if b < s => {
                let mut depth = 0usize;
                after[b..]
                    .char_indices()
                    .find_map(|(i, c)| match c {
                        '{' => {
                            depth += 1;
                            None
                        }
                        '}' => {
                            depth -= 1;
                            (depth == 0).then_some(b + i + 1)
                        }
                        _ => None,
                    })
                    .expect("a balanced test module")
            }
            (_, Some(s)) => s + 1,
            _ => after.len(),
        };
        // Keep the line count, so a reported line number is the file's.
        out.push_str(&"\n".repeat(after[..end].matches('\n').count()));
        from = at + end;
    }
    out.push_str(&code[from..]);
    out
}

/// A module declared out of line by `line` (`mod x;`, after any attributes and any
/// visibility on the same line): its name, and a `#[path]` given on that line.
fn mod_declaration(line: &str) -> Option<(&str, Option<&str>)> {
    let mut rest = line.trim();
    let mut path = None;
    while let Some(attr) = rest.strip_prefix("#[") {
        let close = attr.find(']')?;
        path = path.or_else(|| attr[..close].strip_prefix("path = \"").and_then(|p| p.strip_suffix('"')));
        rest = attr[close + 1..].trim_start();
    }
    if let Some(vis) = rest.strip_prefix("pub") {
        rest = match vis.trim_start().strip_prefix('(') {
            Some(scoped) => scoped[scoped.find(')')? + 1..].trim_start(),
            None => vis.trim_start(),
        };
    }
    let name = rest.strip_prefix("mod ")?.trim().strip_suffix(';')?.trim();
    name.chars().all(|c| c.is_alphanumeric() || c == '_').then_some((name, path))
}

/// The files of the module tree rooted at `root`: `mod x;` declarations followed to
/// `x.rs` / `x/mod.rs` (or a `#[path]`), `#[cfg(test)]` declarations skipped.
fn module_tree(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.to_path_buf()];
    let mut i = 0;
    while i < files.len() {
        let file = files[i].clone();
        i += 1;
        let source = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
        let code = production_code(&source);
        let dir = file.parent().expect("a directory");
        let is_mod_root = matches!(file.file_name().and_then(|n| n.to_str()), Some("lib.rs" | "main.rs" | "mod.rs"));
        let lines: Vec<&str> = code.lines().map(str::trim).collect();
        for (at, line) in lines.iter().enumerate() {
            let Some((name, same_line_path)) = mod_declaration(line) else {
                continue;
            };
            let path_attr = same_line_path.or_else(|| {
                lines[..at]
                    .iter()
                    .rev()
                    .take_while(|l| l.starts_with("#["))
                    .find_map(|l| l.strip_prefix("#[path = \"").and_then(|p| p.strip_suffix("\"]")))
            });
            let base = if is_mod_root { dir.to_path_buf() } else { dir.join(file.file_stem().expect("a stem")) };
            let resolved = match path_attr {
                Some(path) => dir.join(path),
                None if base.join(format!("{name}.rs")).exists() => base.join(format!("{name}.rs")),
                None => base.join(name).join("mod.rs"),
            };
            assert!(resolved.exists(), "{}: `mod {name};` resolves to {}", file.display(), resolved.display());
            files.push(resolved);
        }
    }
    files
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files
}

/// Each `file:line` of `files` whose production code names one of `identifiers` as a
/// whole word.
fn names_in(files: &[PathBuf], identifiers: &BTreeSet<String>) -> Vec<String> {
    let mut hits = Vec::new();
    for file in files {
        let code = production_code(&std::fs::read_to_string(file).expect("readable"));
        for (n, line) in code.lines().enumerate() {
            for ident in identifiers {
                let whole_word = line.match_indices(ident.as_str()).any(|(at, _)| {
                    let before = line[..at].chars().last();
                    let after = line[at + ident.len()..].chars().next();
                    let is_ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                    !is_ident(before) && !is_ident(after)
                });
                if whole_word {
                    hits.push(format!("{}:{} names `{ident}`", file.display(), n + 1));
                }
            }
        }
    }
    hits
}

#[test]
fn the_cli_library_names_no_extension_crate() {
    let workspace = Workspace::from_metadata(&metadata());
    let identifiers = extension_identifiers(&workspace);
    let library = module_tree(&workspace.target(CLI_PACKAGE, "lib").src_path);
    let binary = module_tree(&workspace.target(CLI_PACKAGE, "bin").src_path);
    assert!(library.len() >= 4, "the library's module tree is read: {library:?}");
    // The scan covers every source file: one the two trees do not reach is a module
    // shape the resolver missed — or a test-only file, which would be listed here.
    let src = workspace.target(CLI_PACKAGE, "lib").src_path.parent().expect("a source directory").to_path_buf();
    let unreached: Vec<PathBuf> =
        rust_files(&src).into_iter().filter(|f| !library.contains(f) && !binary.contains(f)).collect();
    assert!(unreached.is_empty(), "no module tree reaches {unreached:?}: the scan would never read them");

    let hits = names_in(&library, &identifiers);
    assert!(
        hits.is_empty(),
        "the cli library names an extension crate — only the binary's composition root \
         (cli/src/composition.rs) may (CR-213, ADR-74 §3):\n  {}",
        hits.join("\n  ")
    );
    // The scan sees a name where one is: the binary's composition names the wiki.
    let composition = names_in(&binary, &identifiers);
    assert!(
        composition.iter().any(|h| h.contains("composition.rs") && h.contains("`logos_wiki`")),
        "the binary's composition names the wiki extension: {composition:?}"
    );
    assert!(!library.iter().any(|f| f.ends_with("composition.rs")), "the composition is the binary's, not the library's");
}

#[test]
fn the_cli_packages_extension_edges_are_optional() {
    let workspace = Workspace::from_metadata(&metadata());
    let required: Vec<String> = workspace
        .package(CLI_PACKAGE)
        .edges
        .iter()
        .filter(|e| e.kind != "dev" && !e.optional && EXTENSION_CRATES.contains(&e.to.as_str()))
        .map(|e| format!("{CLI_PACKAGE} → {}", e.describe()))
        .collect();
    assert!(
        required.is_empty(),
        "the cli package requires an extension crate, so a build without extensions still \
         links it (FR-DS-05):\n  {}",
        required.join("\n  ")
    );
}

/// The two source matchers, probed with their near misses.
#[test]
fn the_source_matchers_skip_comments_tests_and_longer_names() {
    let names = BTreeSet::from(["logos_wiki".to_string()]);
    let dir = tempfile::TempDir::new().expect("a temp dir");
    let file = dir.path().join("lib.rs");
    std::fs::write(
        &file,
        "// logos_wiki in a comment\n\
         use my_logos_wiki::x;\n\
         use logos_wiki_ext::y;\n\
         #[cfg(test)]\nmod tests {\n    use logos_wiki::z;\n    fn f() { if true { } }\n}\n\
         #[cfg(test)]\n#[path = \"nope.rs\"]\nmod nope;\n\
         fn g() { logos_wiki::open(); }\n",
    )
    .expect("write");
    assert_eq!(names_in(std::slice::from_ref(&file), &names), [format!("{}:12 names `logos_wiki`", file.display())]);
    assert_eq!(module_tree(&file), [file], "a #[cfg(test)] module is not followed");

    // Every declaration shape is followed, and a `//` inside a string hides nothing.
    let dir = tempfile::TempDir::new().expect("a temp dir");
    let root = dir.path().join("lib.rs");
    std::fs::write(
        &root,
        "#[cfg(feature = \"x\")] mod a;\npub(super) mod b;\npub(in crate) mod c;\n\
         #[path = \"elsewhere.rs\"] pub mod d;\n#[cfg(test)] mod t { const S: &str = \"}\"; }\nfn f() {}\n",
    )
    .expect("write");
    for (file, body) in [("a.rs", ""), ("b.rs", ""), ("c.rs", ""), ("elsewhere.rs", "")] {
        std::fs::write(dir.path().join(file), body).expect("write");
    }
    let followed: BTreeSet<String> = module_tree(&root)
        .iter()
        .map(|f| f.file_name().expect("a name").to_string_lossy().into_owned())
        .collect();
    assert_eq!(followed, BTreeSet::from(["lib.rs", "a.rs", "b.rs", "c.rs", "elsewhere.rs"].map(String::from)));
    std::fs::write(dir.path().join("a.rs"), "const U: &str = \"https://x\"; use logos_wiki::y;\n").expect("write");
    assert_eq!(
        names_in(&[dir.path().join("a.rs")], &names),
        [format!("{}:1 names `logos_wiki`", dir.path().join("a.rs").display())]
    );
}
