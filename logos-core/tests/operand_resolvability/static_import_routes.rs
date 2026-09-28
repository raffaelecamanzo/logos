//! **S-470's estate report** — each Spring mapping path the reference estate
//! writes as a `+` concatenation, reported as promoted (with its folded path) or
//! counted in `routes_not_composed`, per member with denominators ([CR-151]
//! §2.1, [FR-FW-05]).
//!
//! # What is the product's and what is the harness's
//!
//! Which sites exist is the **harness's** census: a text reading of each
//! main-tree `.java` file, independent of the product's query, so the product is
//! measured against something it did not produce ([`concatenated_mapping_sites`],
//! pinned by the always-run fixtures below). What happened to each site is the
//! **product's**: the member is indexed by the shipped [`Engine`], and a site is
//! *promoted* when a `route` node of its file covers the annotation's line.
//!
//! A refusal carries no location — [`FrameworkStats`] publishes a count, not a
//! list — so an unpromoted site is attributed to the member's
//! `routes_not_composed` count: the run fails when a member has more unpromoted
//! sites than refusals, which is the one way a site can have been **dropped**.
//! No count is asserted beyond that: [CR-151] §2.1's 16 is a census, not a
//! floor, and §2.2 is a hypothesis.
//!
//! # Never over the live estate
//!
//! The harness writes nothing under `LOGOS_REF_WORKSPACE`. Each member holding
//! a site is copied — its `.java` files only — into a temporary directory and
//! indexed there, so the member's own `.logos` store is never opened, whichever
//! workspace the variable names.
//!
//! [CR-151]: ../../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md
//! [FR-FW-05]: ../../../docs/specs/requirements/FR-FW-05.md
//! [`FrameworkStats`]: logos_core::models::pipeline::FrameworkStats

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use logos_core::federation::discover;
use logos_core::model::NodeKind;
use logos_core::Engine;
use tempfile::TempDir;

use super::corpus_root;
use super::cross_member_type_refs::{consumer_tree, Tree};

/// One mapping annotation whose path is written as a concatenation.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    /// 1-based line of the annotation's `@`.
    line: u32,
    /// The annotation's argument text, whitespace-collapsed, for the report.
    arguments: String,
    /// The shape of each concatenated path the annotation writes — its string
    /// literals in order, every other operand a gap ([`path_shapes`]).
    shapes: Vec<Vec<String>>,
}

/// Every `@…Mapping(…)` annotation in `source` whose argument list, read with
/// string literal contents removed, holds a `+` — the census rule of [CR-151]
/// §2.1 ("whose method path is a `+` concatenation"), applied to method and
/// type annotations alike.
///
/// Deliberately text-level and independent of the product's query: the
/// harness measures the product against it. Comments are skipped so a
/// commented-out mapping is not a site; char literals and text blocks do not
/// occur in the estate's annotations and are read as ordinary text.
///
/// [CR-151]: ../../../docs/requests/CR-151-provider-routes-composed-from-string-constants.md
fn concatenated_mapping_sites(source: &str) -> Vec<Site> {
    let code = without_comments(source);
    let bytes = code.as_bytes();
    let mut sites = Vec::new();
    let mut i = 0;
    while let Some(offset) = code[i..].find('@') {
        let at = i + offset;
        let name_end = at
            + 1
            + code[at + 1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(code.len() - at - 1);
        i = name_end;
        if !code[at + 1..name_end].ends_with("Mapping") {
            continue;
        }
        let open = name_end + code[name_end..].len() - code[name_end..].trim_start().len();
        if bytes.get(open) != Some(&b'(') {
            continue;
        }
        let Some(close) = matching_paren(bytes, open) else {
            continue;
        };
        let arguments = &code[open + 1..close];
        if strip_strings(arguments).contains('+') {
            sites.push(Site {
                line: code[..at].matches('\n').count() as u32 + 1,
                arguments: arguments.split_whitespace().collect::<Vec<_>>().join(" "),
                shapes: path_shapes(arguments),
            });
        }
        i = close;
    }
    sites
}

/// The shape of each concatenated path in an annotation's `arguments`: the
/// `value =` / `path =` argument, or the positional one, each list element on
/// its own; for each element holding a top-level `+`, its string literals'
/// contents in order, with an empty string standing for every other operand.
/// `"/u/{" + ID + "}/x"` → `["/u/{", "", "}/x"]`.
///
/// The harness's own reading of what path a site writes — it resolves no
/// name, so it cannot agree with the product's fold by construction; it only
/// says which route a site could have become.
fn path_shapes(arguments: &str) -> Vec<Vec<String>> {
    let path_argument = split_top_level(arguments, ',')
        .into_iter()
        .find_map(|argument| {
            let argument = argument.trim();
            match argument.split_once('=') {
                Some((key, value)) if !key.contains('"') => {
                    matches!(key.trim(), "value" | "path").then(|| value.trim().to_string())
                }
                _ => Some(argument.to_string()),
            }
        })
        .unwrap_or_default();
    let elements = match path_argument.strip_prefix('{').and_then(|rest| rest.strip_suffix('}')) {
        Some(list) => split_top_level(list, ','),
        None => vec![path_argument],
    };
    elements
        .iter()
        .map(|element| split_top_level(element, '+'))
        .filter(|operands| operands.len() > 1)
        .map(|operands| {
            operands
                .iter()
                .map(|operand| {
                    let operand = operand.trim().trim_start_matches('(').trim_end_matches(')').trim();
                    operand
                        .strip_prefix('"')
                        .and_then(|rest| rest.strip_suffix('"'))
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .collect()
}

/// `text` split at every `separator` outside string literals, parentheses and
/// braces.
fn split_top_level(text: &str, separator: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else {
            match c {
                '"' => in_string = true,
                '(' | '{' => depth += 1,
                ')' | '}' => depth -= 1,
                c if c == separator && depth == 0 => {
                    parts.push(String::new());
                    continue;
                }
                _ => {}
            }
        }
        parts.last_mut().expect("never empty").push(c);
    }
    parts
}

/// `true` when a route's `path` ends with `shape`: its literals in order, each
/// gap standing for one or more characters. A suffix, because the product
/// joins a type-level prefix in front of the method path.
fn shape_matches(shape: &[String], path: &str) -> bool {
    let mut pattern = String::from("^.*");
    for piece in shape {
        pattern.push_str(&if piece.is_empty() { ".+?".to_string() } else { regex::escape(piece) });
    }
    pattern.push('$');
    regex::Regex::new(&pattern).is_ok_and(|re| re.is_match(path))
}

/// `source` with `//` and `/* */` comments blanked to spaces — newlines kept,
/// so line numbers survive — and string literals left intact.
fn without_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(escaped) = chars.next() {
                    out.push(escaped);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for skipped in chars.by_ref() {
                    out.push(if skipped == '\n' { '\n' } else { ' ' });
                    if last == '*' && skipped == '/' {
                        break;
                    }
                    last = skipped;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The byte index of the `)` closing the `(` at `open`, skipping string
/// literals.
fn matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            match b {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match b {
                b'"' => in_string = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// `text` with every string literal's contents removed (quotes kept).
fn strip_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
                out.push(c);
            }
        } else {
            if c == '"' {
                in_string = true;
            }
            out.push(c);
        }
    }
    out
}

/// Directories [`java_files`] never descends into: build output and tool state.
const SKIPPED_DIRS: [&str; 5] = ["target", "build", ".git", ".logos", "node_modules"];

/// A main-tree Java file — the main tree by [S-471]'s segment rule
/// ([`consumer_tree`]), shared rather than restated.
///
/// [S-471]: ../../../docs/planning/journal.md#s-471-measure-cross-member-type-references-over-the-reference-estate
fn is_main_tree_java(rel: &str) -> bool {
    rel.ends_with(".java") && consumer_tree(rel) == Tree::Main
}

/// Every `.java` file under `dir`, as paths relative to it, sorted.
fn java_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if !SKIPPED_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if name.ends_with(".java") {
                if let Ok(rel) = path.strip_prefix(dir) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out
}

/// What became of one site.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// A route node of its own: the node's lines cover the site's annotation.
    Promoted(Vec<String>),
    /// Its path is a route node another site of the file also promoted — one
    /// `(method, path)` is one node, carrying the first site's lines — and the
    /// member refused nothing the unpromoted sites do not account for, so this
    /// site was promoted.
    Merged(Vec<String>),
    /// As [`Verdict::Merged`], but the member refused more than its unpromoted
    /// sites account for, so this site may be one of the refusals: a refusal
    /// carries no location, and the harness says so rather than guess.
    MergedOrCounted(Vec<String>),
    /// No route of its file has its shape: refused and counted, or — when the
    /// member's refusal count cannot cover it — dropped.
    NotPromoted,
}

/// What indexing one member did with its sites.
struct MemberReport {
    /// `(file, site, verdict)`, in file and line order.
    sites: Vec<(String, Site, Verdict)>,
    routes_not_composed: u64,
    routes: u64,
}

impl MemberReport {
    fn count(&self, pick: impl Fn(&Verdict) -> bool) -> usize {
        self.sites.iter().filter(|(_, _, v)| pick(v)).count()
    }
}

/// Index a copy of `member_root`'s Java files and judge each of `sites`.
fn index_member(member_root: &Path, sites: &BTreeMap<String, Vec<Site>>) -> MemberReport {
    let copy = TempDir::new().expect("temp dir");
    for rel in java_files(member_root) {
        let target = copy.path().join(&rel);
        fs::create_dir_all(target.parent().expect("a file has a parent")).expect("mkdir");
        fs::copy(member_root.join(&rel), &target).expect("copy");
    }
    let engine = Engine::start(copy.path()).expect("engine starts over the copy");
    let stats = engine.index().framework;
    let routes: Vec<(String, String, i64, i64)> = engine
        .runtime()
        .expect("runtime")
        .submit_read(|store| {
            Ok(store
                .all_nodes()?
                .into_iter()
                .filter(|n| n.kind == NodeKind::Route)
                .map(|n| {
                    (
                        n.file_path.unwrap_or_default(),
                        n.name,
                        n.start_line.unwrap_or(0),
                        n.end_line.unwrap_or(0),
                    )
                })
                .collect())
        })
        .expect("read runs");
    let mut judged = Vec::new();
    for (file, file_sites) in sites {
        for site in file_sites {
            // Matched by shape, not by line: two sites the product promotes as
            // one `(method, path)` are one route node, whose lines are the
            // first site's (`dedup_routes`).
            let line = i64::from(site.line);
            let mut names: Vec<String> = Vec::new();
            let mut own_node = true;
            let every_shape = !site.shapes.is_empty()
                && site.shapes.iter().all(|shape| {
                    let matching: Vec<&(String, String, i64, i64)> = routes
                        .iter()
                        .filter(|(f, name, _, _)| {
                            f == file
                                && name
                                    .split_once(' ')
                                    .is_some_and(|(_, path)| shape_matches(shape, path))
                        })
                        .collect();
                    own_node &= matching.iter().any(|(_, _, start, end)| *start <= line && line <= *end);
                    names.extend(matching.iter().map(|(_, name, _, _)| name.clone()));
                    !matching.is_empty()
                });
            names.sort();
            names.dedup();
            let verdict = match (every_shape, own_node) {
                (false, _) => Verdict::NotPromoted,
                (true, true) => Verdict::Promoted(names),
                (true, false) => Verdict::Merged(names),
            };
            judged.push((file.clone(), site.clone(), verdict));
        }
    }
    // Refusals the unpromoted sites do not account for may be merged sites.
    let unpromoted = judged.iter().filter(|(_, _, v)| *v == Verdict::NotPromoted).count() as u64;
    if stats.routes_not_composed > unpromoted {
        for (_, _, verdict) in &mut judged {
            if let Verdict::Merged(names) = verdict {
                *verdict = Verdict::MergedOrCounted(std::mem::take(names));
            }
        }
    }
    MemberReport {
        sites: judged,
        routes_not_composed: stats.routes_not_composed,
        routes: stats.routes,
    }
}

/// The estate run: every concatenated site, per member, promoted or counted.
///
/// ```text
/// LOGOS_REF_WORKSPACE=~/source/.estate-copies/pec-services-S-470 \
///   cargo test -p logos-core --test operand_resolvability \
///   static_import_routes -- --nocapture
/// ```
#[test]
fn report_each_concatenated_mapping_site_on_the_reference_workspace() {
    let Some(root) = corpus_root() else {
        eprintln!("LOGOS_REF_WORKSPACE unset — S-470's estate report skipped");
        return;
    };
    let federation = discover(&root)
        .expect("the workspace manifest parses")
        .expect("LOGOS_REF_WORKSPACE names a workspace");

    let mut total_sites = 0usize;
    let mut total_promoted = 0usize;
    let mut total_uncertain = 0usize;
    let mut dropped: Vec<String> = Vec::new();
    println!("S-470 concatenated mapping sites, main tree, per member:");
    for member in &federation.members {
        let mut sites: BTreeMap<String, Vec<Site>> = BTreeMap::new();
        for rel in java_files(&member.root) {
            if !is_main_tree_java(&rel) {
                continue;
            }
            let Ok(source) = fs::read_to_string(member.root.join(&rel)) else { continue };
            let found = concatenated_mapping_sites(&source);
            if !found.is_empty() {
                sites.insert(rel, found);
            }
        }
        if sites.is_empty() {
            continue;
        }
        let report = index_member(&member.root, &sites);
        let promoted = report.count(|v| matches!(v, Verdict::Promoted(_) | Verdict::Merged(_)));
        let uncertain = report.count(|v| matches!(v, Verdict::MergedOrCounted(_)));
        let unpromoted = report.count(|v| *v == Verdict::NotPromoted);
        println!(
            "\n{}: {} site(s) — {promoted} promoted, {uncertain} promoted-or-counted, {unpromoted} not promoted; \
             member routes {}, routes_not_composed {}",
            member.name,
            report.sites.len(),
            report.routes,
            report.routes_not_composed
        );
        for (file, site, verdict) in &report.sites {
            let verdict = match verdict {
                Verdict::Promoted(names) => format!("PROMOTED {}", names.join(", ")),
                Verdict::Merged(names) => format!(
                    "PROMOTED {} (one node with a same-method, same-path site of this file)",
                    names.join(", ")
                ),
                Verdict::MergedOrCounted(names) => format!(
                    "PROMOTED as {} OR COUNTED (the member has unattributed refusals)",
                    names.join(", ")
                ),
                Verdict::NotPromoted => "COUNTED (not promoted)".to_string(),
            };
            println!("  {file}:{} — {verdict}\n      ({})", site.line, site.arguments);
        }
        if unpromoted as u64 > report.routes_not_composed {
            dropped.push(format!(
                "{}: {unpromoted} unpromoted site(s) but routes_not_composed {}",
                member.name, report.routes_not_composed
            ));
        }
        total_uncertain += uncertain;
        total_sites += report.sites.len();
        total_promoted += promoted;
    }
    println!(
        "\nTOTAL: {total_sites} concatenated site(s) — {total_promoted} promoted, {total_uncertain} \
         promoted-or-counted, {} counted",
        total_sites - total_promoted - total_uncertain
    );
    assert!(total_sites > 0, "the census found no site: a green run that measured nothing");
    assert!(dropped.is_empty(), "sites neither promoted nor counted: {dropped:#?}");
}

// ── The census classifier, always run ───────────────────────────────────────

#[test]
fn a_concatenated_mapping_path_is_a_site_and_a_literal_one_is_not() {
    let source = r#"
@RequestMapping("/v1")
public interface MailboxApiV1 {
    @RequestMapping(
            method = RequestMethod.GET,
            value = "/users/{userId}/mailboxes/{" + EMAIL_ADDRESS_PARAMETER_NAME + "}/availability",
            produces = { "application/json" }
    )
    String availability();

    @GetMapping(value = "/literal/+/plus")
    String literal();

    @PostMapping(BASE + "/x")
    String positional();
}
"#;
    let sites = concatenated_mapping_sites(source);
    let lines: Vec<u32> = sites.iter().map(|s| s.line).collect();
    // The `+` inside a string literal is not a concatenation.
    assert_eq!(lines, [4, 14]);
    assert!(sites[0].arguments.contains("EMAIL_ADDRESS_PARAMETER_NAME"));
}

#[test]
fn a_commented_out_or_non_mapping_annotation_is_not_a_site() {
    let source = "// @GetMapping(A + \"/x\")\n\
                  /* @GetMapping(B + \"/y\") */\n\
                  @InitBinder(value = A + B)\n\
                  @Value(\"${a}\" + \"b\")\n\
                  @GetMapping(\"/c\" /* + D */)\n";
    assert_eq!(concatenated_mapping_sites(source), Vec::<Site>::new());
}

#[test]
fn a_sites_path_shape_keeps_its_literals_and_matches_only_its_route() {
    let sites = concatenated_mapping_sites(
        r#"@RequestMapping(method = RequestMethod.GET, value = "/users/{" + ID + "}/x", produces = {"a", "b"})"#,
    );
    let shape = &sites[0].shapes;
    assert_eq!(shape, &[vec!["/users/{".to_string(), String::new(), "}/x".to_string()]]);
    assert!(shape_matches(&shape[0], "/v1/users/{userId}/x"), "prefixed");
    assert!(shape_matches(&shape[0], "/users/{userId}/x"), "unprefixed");
    // Near misses: another tail, an empty gap, another head.
    assert!(!shape_matches(&shape[0], "/v1/users/{userId}/y"));
    assert!(!shape_matches(&shape[0], "/v1/users/{}/x"));
    assert!(!shape_matches(&shape[0], "/v1/members/{userId}/x"));

    let positional = concatenated_mapping_sites(r#"@GetMapping({"/a", BASE + "/b"})"#);
    assert_eq!(positional[0].shapes, [vec![String::new(), "/b".to_string()]]);
}

#[test]
fn only_a_main_tree_java_file_is_read() {
    assert!(is_main_tree_java("api/src/main/java/a/B.java"));
    assert!(!is_main_tree_java("api/src/test/java/a/B.java"));
    assert!(!is_main_tree_java("api/xsrc/main/java/a/B.java"), "a segment, not a substring");
    assert!(!is_main_tree_java("api/src/main/java/a/B.kt"));
}

/// The estate's dominant shape, run through the shipped engine: three
/// `@RequestMapping` methods on one folded path become **one** `ANY` route
/// node, whose lines are the first site's. Each site is still promoted — the
/// two later ones as that same node — and where the member also refused a
/// same-shaped site, the harness does not claim to know which one.
#[test]
fn sites_merged_into_one_route_node_are_each_promoted() {
    let member = TempDir::new().expect("temp dir");
    let write = |rel: &str, text: &str| {
        let path = member.path().join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, text).expect("write");
    };
    write(
        "src/main/java/a/b/Advice.java",
        "package a.b;\n\npublic class Advice {\n    public static final String EMAIL = \"emailAddress\";\n}\n",
    );
    let mapping = |method: &str, handler: &str, name: &str| {
        format!(
            "    @RequestMapping(\n            method = RequestMethod.{method},\n            \
             value = \"/users/{{\" + {name} + \"}}/availability\")\n    String {handler}();\n\n"
        )
    };
    let api = format!(
        "package a.b;\n\n\
         import static a.b.Advice.EMAIL;\n\
         import org.springframework.web.bind.annotation.RequestMapping;\n\
         import org.springframework.web.bind.annotation.RequestMethod;\n\n\
         @RequestMapping(\"/v1\")\n\
         public interface Api {{\n{}{}{}}}\n",
        mapping("GET", "get", "EMAIL"),
        mapping("POST", "post", "EMAIL"),
        mapping("DELETE", "delete", "EMAIL"),
    );

    let judge = |api: &str| {
        write("src/main/java/a/b/Api.java", api);
        let sites: BTreeMap<String, Vec<Site>> =
            [("src/main/java/a/b/Api.java".to_string(), concatenated_mapping_sites(api))].into();
        let report = index_member(member.path(), &sites);
        let verdicts: Vec<(u32, Verdict)> =
            report.sites.iter().map(|(_, site, v)| (site.line, v.clone())).collect();
        (verdicts, report.routes, report.routes_not_composed)
    };
    let route = || vec!["ANY /v1/users/{emailAddress}/availability".to_string()];

    // Three methods, one node: each site promoted, the later two as merged.
    let (verdicts, routes, refused) = judge(&api);
    assert_eq!(
        verdicts,
        [
            (9, Verdict::Promoted(route())),
            (14, Verdict::Merged(route())),
            (19, Verdict::Merged(route())),
        ]
    );
    assert_eq!((routes, refused), (1, 0));

    // A fourth site of the same shape whose constant is not declared is
    // refused — and the refusal carries no location, so every merged site is
    // reported as possibly that refusal rather than as promoted.
    let with_undeclared = api.replace("}\n", &format!("{}}}\n", mapping("GET", "other", "UNDECLARED")));
    let (verdicts, routes, refused) = judge(&with_undeclared);
    assert_eq!(
        verdicts,
        [
            (9, Verdict::Promoted(route())),
            (14, Verdict::MergedOrCounted(route())),
            (19, Verdict::MergedOrCounted(route())),
            (24, Verdict::MergedOrCounted(route())),
        ]
    );
    assert_eq!((routes, refused), (1, 1));
}
