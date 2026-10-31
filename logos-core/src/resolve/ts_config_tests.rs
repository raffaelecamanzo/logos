//! Unit tests of [`super`]: which candidate paths a bare specifier is rewritten
//! into, read off real files in a temporary tree (S-531).

use super::*;

fn write(root: &Path, rel: &str, text: &str) {
    let at = root.join(rel);
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::fs::write(at, text).unwrap();
}

fn ts() -> HashSet<String> {
    ["ts", "tsx", "js", "jsx"].iter().map(|e| e.to_string()).collect()
}

fn candidates(resolution: &TsResolution, from: &str, spec: &str) -> Vec<String> {
    resolution.candidates(from, spec).expect("a file of an opted-in language")
}

#[test]
fn strip_jsonc_drops_comments_and_trailing_commas_but_never_string_text() {
    let text = r#"{
      // a line comment
      "compilerOptions": { /* block */ "baseUrl": ".", "paths": { "@/*": ["./src/*",], }, },
      "url": "http://x/*not a comment*/,}",
    }"#;
    let value: Value = serde_json::from_str(&strip_jsonc(text)).expect("valid JSON once stripped");
    assert_eq!(value["compilerOptions"]["paths"]["@/*"][0], "./src/*");
    assert_eq!(value["url"], "http://x/*not a comment*/,}");
    // A comment between the trailing comma and its bracket hides nothing.
    let hidden: Value = serde_json::from_str(&strip_jsonc("[1, 2, // two\n]")).unwrap();
    assert_eq!(hidden, serde_json::json!([1, 2]));
}

#[test]
fn a_paths_alias_rewrites_against_the_nearest_config_then_base_url() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
    );
    let r = discover(tmp.path(), ts(), ["src/app.tsx"]);
    assert_eq!(candidates(&r, "src/app.tsx", "@/lib/api"), ["./src/lib/api", "@/lib/api"]);
    // No pattern matches: only `baseUrl` rewrites it.
    assert_eq!(candidates(&r, "src/app.tsx", "react"), ["react"]);
}

#[test]
fn each_package_of_a_monorepo_resolves_through_its_own_config() {
    let tmp = tempfile::tempdir().unwrap();
    let config = r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#;
    write(tmp.path(), "apps/a/tsconfig.json", config);
    write(tmp.path(), "apps/b/jsconfig.json", config);
    let r = discover(tmp.path(), ts(), ["apps/a/src/x.ts", "apps/b/src/y.js"]);
    assert_eq!(candidates(&r, "apps/a/src/x.ts", "@/lib")[0], "apps/a/./src/lib");
    assert_eq!(candidates(&r, "apps/b/src/y.js", "@/lib")[0], "apps/b/./src/lib");
}

#[test]
fn a_tsconfig_is_read_over_a_jsconfig_beside_it_and_the_nearest_owns_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "tsconfig.json", r#"{ "compilerOptions": { "paths": { "~/*": ["./root/*"] } } }"#);
    write(tmp.path(), "app/tsconfig.json", r#"{ "compilerOptions": { "paths": { "~/*": ["./ts/*"] } } }"#);
    write(tmp.path(), "app/jsconfig.json", r#"{ "compilerOptions": { "paths": { "~/*": ["./js/*"] } } }"#);
    let r = discover(tmp.path(), ts(), ["app/x.ts", "other/y.ts"]);
    assert_eq!(candidates(&r, "app/x.ts", "~/m"), ["app/./ts/m"]);
    assert_eq!(candidates(&r, "other/y.ts", "~/m"), ["./root/m"]);
}

#[test]
fn extends_is_followed_inside_the_repository_and_an_extending_key_replaces_its_base() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tsconfig.base.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@shared/*": ["packages/shared/*"] } } }"#,
    );
    // Inherits both keys; `paths` are relative to the inherited `baseUrl`.
    write(tmp.path(), "apps/a/tsconfig.json", r#"{ "extends": "../../tsconfig.base" }"#);
    // Replaces `paths` wholesale, keeps the base's `baseUrl`.
    write(
        tmp.path(),
        "apps/b/tsconfig.json",
        r#"{ "extends": ["@tsconfig/node18/tsconfig.json", "../../tsconfig.base.json"],
             "compilerOptions": { "paths": { "@b/*": ["apps/b/src/*"] } } }"#,
    );
    let r = discover(tmp.path(), ts(), ["apps/a/x.ts", "apps/b/y.ts"]);
    assert_eq!(candidates(&r, "apps/a/x.ts", "@shared/util"), ["packages/shared/util", "@shared/util"]);
    assert_eq!(candidates(&r, "apps/b/y.ts", "@shared/util"), ["@shared/util"]);
    assert_eq!(candidates(&r, "apps/b/y.ts", "@b/z"), ["apps/b/src/z", "@b/z"]);
}

#[test]
fn paths_without_a_base_url_are_relative_to_the_config_that_declared_them() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "tsconfig.json", r#"{ "compilerOptions": { "paths": { "~/*": ["./src/*"] } } }"#);
    write(tmp.path(), "apps/a/tsconfig.json", r#"{ "extends": "../../tsconfig.json" }"#);
    let r = discover(tmp.path(), ts(), ["apps/a/x.ts"]);
    assert_eq!(candidates(&r, "apps/a/x.ts", "~/m"), ["./src/m"]);
}

#[test]
fn a_cyclic_extends_chain_terminates() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tsconfig.json",
        r#"{ "extends": "./b.tsconfig.json", "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
    );
    write(tmp.path(), "b.tsconfig.json", r#"{ "extends": "./tsconfig.json" }"#);
    let r = discover(tmp.path(), ts(), ["x.ts"]);
    assert_eq!(candidates(&r, "x.ts", "@/m"), ["./src/m"]);
}

#[test]
fn an_exact_pattern_beats_a_wildcard_and_the_longest_prefix_wins() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tsconfig.json",
        r#"{ "compilerOptions": { "paths": {
            "@/*": ["./src/*"],
            "@/ui/*": ["./ui/*", "./fallback/*"],
            "@/config": ["./config/index"],
            "a*b*c": ["./never"]
        } } }"#,
    );
    let r = discover(tmp.path(), ts(), ["x.ts"]);
    assert_eq!(candidates(&r, "x.ts", "@/ui/button"), ["./ui/button", "./fallback/button"]);
    assert_eq!(candidates(&r, "x.ts", "@/config"), ["./config/index"]);
    assert_eq!(candidates(&r, "x.ts", "@/lib"), ["./src/lib"]);
    assert!(candidates(&r, "x.ts", "axbxc").is_empty(), "a two-`*` pattern is ignored");
}

#[test]
fn a_workspace_package_resolves_to_its_entry_in_the_declared_order() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui",
             "exports": { ".": { "require": "./cjs/index.js", "import": "./src/index.ts", "default": "./dist/index.js" },
                          "./button": "./src/button.tsx",
                          "./icons/*": { "types": "./src/icons/*.ts" } },
             "types": "./dist/index.d.ts", "module": "./esm/index.js", "main": "./dist/index.js" }"#,
    );
    write(tmp.path(), "packages/lib/package.json", r#"{ "name": "lib", "main": "src/main.ts" }"#);
    let files = ["apps/web/x.ts", "packages/ui/src/index.ts", "packages/lib/src/main.ts"];
    let r = discover(tmp.path(), ts(), files);
    assert_eq!(
        candidates(&r, "apps/web/x.ts", "@acme/ui"),
        [
            "packages/ui/./src/index.ts",
            "packages/ui/./dist/index.js",
            "packages/ui/./dist/index.d.ts",
            "packages/ui/./esm/index.js",
            "packages/ui/./dist/index.js",
            "packages/ui",
        ]
    );
    assert_eq!(candidates(&r, "apps/web/x.ts", "@acme/ui/button"), ["packages/ui/./src/button.tsx"]);
    assert_eq!(candidates(&r, "apps/web/x.ts", "@acme/ui/icons/home"), ["packages/ui/./src/icons/home.ts"]);
    // `exports` lists its subpaths: an unlisted one names nothing.
    assert!(candidates(&r, "apps/web/x.ts", "@acme/ui/internal").is_empty());
    // No `exports`: a subpath is a path under the package root.
    assert_eq!(candidates(&r, "apps/web/x.ts", "lib"), ["packages/lib/src/main.ts", "packages/lib"]);
    assert_eq!(candidates(&r, "apps/web/x.ts", "lib/util"), ["packages/lib/util"]);
    // A name that only starts with a package's is not that package.
    assert!(candidates(&r, "apps/web/x.ts", "library").is_empty());
}

#[test]
fn an_exports_subpath_takes_the_exact_key_then_the_longest_pattern_and_null_excludes() {
    // `./*` is declared first: declaration order must not decide.
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": {
              "./*": "./src/*.ts",
              "./internal/*": null,
              "./features/*": "./src/features/*/index.ts",
              "./button": "./src/button/index.ts",
              "./deep/*": "./src/*/*.ts" } }"#,
    );
    let r = discover(tmp.path(), ts(), ["apps/web/x.ts", "packages/ui/src/a.ts"]);
    let from = "apps/web/x.ts";
    assert_eq!(candidates(&r, from, "@acme/ui/button"), ["packages/ui/./src/button/index.ts"]);
    assert_eq!(candidates(&r, from, "@acme/ui/features/auth"), ["packages/ui/./src/features/auth/index.ts"]);
    assert!(candidates(&r, from, "@acme/ui/internal/secret").is_empty(), "a null target excludes");
    assert_eq!(candidates(&r, from, "@acme/ui/deep/x"), ["packages/ui/./src/x/x.ts"]);
    assert_eq!(candidates(&r, from, "@acme/ui/other"), ["packages/ui/./src/other.ts"]);
}

#[test]
fn an_external_or_twice_declared_package_has_no_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "a/package.json", r#"{ "name": "dup", "main": "a.ts" }"#);
    write(tmp.path(), "b/package.json", r#"{ "name": "dup", "main": "b.ts" }"#);
    write(tmp.path(), "package.json", r#"{ "private": true }"#);
    let r = discover(tmp.path(), ts(), ["a/a.ts", "b/b.ts", "x.ts"]);
    assert!(candidates(&r, "x.ts", "react").is_empty());
    assert!(candidates(&r, "x.ts", "dup").is_empty(), "two packages named `dup` decide neither");
}

#[test]
fn an_extends_naming_a_workspace_package_reads_its_config() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "packages/config/package.json", r#"{ "name": "@repo/config" }"#);
    write(
        tmp.path(),
        "packages/config/base.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "$/*": ["./shared/*"] } } }"#,
    );
    write(tmp.path(), "apps/a/tsconfig.json", r#"{ "extends": "@repo/config/base" }"#);
    // A package of configs only: its own indexed `package.json` is what finds it.
    let r = discover(tmp.path(), ts(), ["apps/a/x.ts", "packages/config/package.json"]);
    assert_eq!(candidates(&r, "apps/a/x.ts", "$/m")[0], "packages/config/./shared/m");
    // Without an indexed file there, it is not found.
    let r = discover(tmp.path(), ts(), ["apps/a/x.ts"]);
    assert!(candidates(&r, "apps/a/x.ts", "$/m").is_empty());
}

#[test]
fn a_byte_order_mark_does_not_void_a_config_or_a_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "tsconfig.json", "\u{feff}{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"] } } }");
    write(tmp.path(), "packages/ui/package.json", "\u{feff}{ \"name\": \"@acme/ui\", \"main\": \"src/index.ts\" }");
    let r = discover(tmp.path(), ts(), ["src/x.ts", "packages/ui/src/index.ts"]);
    assert_eq!(candidates(&r, "src/x.ts", "@/lib/api"), ["./src/lib/api"]);
    assert_eq!(candidates(&r, "src/x.ts", "@acme/ui"), ["packages/ui/src/index.ts", "packages/ui"]);
}

#[test]
fn a_malformed_nearest_config_owns_its_files_and_adds_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "tsconfig.json", r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#);
    write(tmp.path(), "app/tsconfig.json", "{ not json");
    let r = discover(tmp.path(), ts(), ["app/x.ts"]);
    assert!(candidates(&r, "app/x.ts", "@/m").is_empty(), "the root's alias never reaches app/");
}

#[test]
fn only_opted_in_files_are_governed_and_a_tree_without_them_reads_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "tsconfig.json", r#"{ "compilerOptions": { "baseUrl": "." } }"#);
    let r = discover(tmp.path(), ts(), ["main.go", "x.ts"]);
    assert_eq!(r.candidates("main.go", "shared"), None);
    assert!(discover(tmp.path(), ts(), ["main.go"]).is_empty());
    assert!(discover(tmp.path(), HashSet::new(), ["x.ts"]).is_empty());
}

#[test]
fn is_descriptor_names_the_files_a_bare_specifier_binds_through() {
    for rel in ["package.json", "apps/a/tsconfig.json", "tsconfig.base.json", "web/jsconfig.json", "configs/base.json", "X.JSON"] {
        assert!(is_descriptor(rel), "{rel}");
    }
    for rel in ["tsconfig.ts", "src/x.jsonc", "json", "a.json/x.ts"] {
        assert!(!is_descriptor(rel), "{rel}");
    }
}

#[test]
fn strip_jsonc_keeps_escaped_quotes_inside_strings() {
    let text = r#"{ "a": "q\"//not a comment,}", "b": "\\", }"#;
    let value: Value = serde_json::from_str(&strip_jsonc(text)).expect("valid JSON once stripped");
    assert_eq!(value["a"], "q\"//not a comment,}");
    assert_eq!(value["b"], "\\");
}

#[test]
fn an_extending_base_url_replaces_its_bases_and_a_later_base_overrides_an_earlier_one() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "base.json", r#"{ "compilerOptions": { "baseUrl": "." } }"#);
    write(tmp.path(), "one.json", r#"{ "compilerOptions": { "paths": { "@/*": ["./one/*"] } } }"#);
    write(tmp.path(), "two.json", r#"{ "compilerOptions": { "paths": { "@/*": ["./two/*"] } } }"#);
    write(
        tmp.path(),
        "app/tsconfig.json",
        r#"{ "extends": ["../base.json", "../one.json", "../two.json"], "compilerOptions": { "baseUrl": "./src" } }"#,
    );
    let r = discover(tmp.path(), ts(), ["app/src/x.ts"]);
    // `two.json`'s `paths` win over `one.json`'s; the app's own `baseUrl`
    // replaces `base.json`'s, and the `paths` resolve against it.
    assert_eq!(candidates(&r, "app/src/x.ts", "@/m"), ["app/src/./two/m", "app/src/@/m"]);
}

#[test]
fn the_longest_prefix_wins_in_any_declaration_order_and_the_first_wins_a_tie() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tsconfig.json",
        r##"{ "compilerOptions": { "paths": {
            "@/ui/*": ["./ui/*"],
            "@/*": ["./src/*"],
            "#/*a": ["./first/*"],
            "#/*b": ["./never/*"],
            "#/x*": ["./x/*"],
            "#/*": ["./tied-second/*"]
        } } }"##,
    );
    let r = discover(tmp.path(), ts(), ["x.ts"]);
    assert_eq!(candidates(&r, "x.ts", "@/ui/button"), ["./ui/button"]);
    assert_eq!(candidates(&r, "x.ts", "@/lib"), ["./src/lib"]);
    assert_eq!(candidates(&r, "x.ts", "#/xa"), ["./x/a"], "`#/x` is longer than `#/`");
    // `#/*a` and `#/*` share the prefix `#/`; the first declared decides.
    assert_eq!(candidates(&r, "x.ts", "#/qa"), ["./first/q"]);
}

#[test]
fn exports_conditions_strings_and_same_length_names_resolve_their_own_package() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "packages/ui/package.json",
        r#"{ "name": "@acme/ui", "exports": { ".": { "types": "./types.ts", "import": "./esm.ts" } } }"#,
    );
    write(tmp.path(), "packages/db/package.json", r#"{ "name": "@acme/db", "exports": "./src/db.ts" }"#);
    write(
        tmp.path(),
        "packages/io/package.json",
        r#"{ "name": "@acme/io", "exports": { "default": "./d.ts", "import": "./i.ts" } }"#,
    );
    let files = ["x.ts", "packages/ui/a.ts", "packages/db/a.ts", "packages/io/a.ts"];
    let r = discover(tmp.path(), ts(), files);
    assert_eq!(candidates(&r, "x.ts", "@acme/ui"), ["packages/ui/./esm.ts", "packages/ui/./types.ts", "packages/ui"]);
    assert_eq!(candidates(&r, "x.ts", "@acme/db"), ["packages/db/./src/db.ts", "packages/db"]);
    assert_eq!(candidates(&r, "x.ts", "@acme/io"), ["packages/io/./i.ts", "packages/io/./d.ts", "packages/io"]);
    // A string or top-level condition `exports` declares no subpath map.
    assert_eq!(candidates(&r, "x.ts", "@acme/db/extra"), ["packages/db/extra"]);
}

#[test]
fn an_extends_naming_only_a_workspace_package_reads_its_tsconfig() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "packages/config/package.json", r#"{ "name": "@repo/config" }"#);
    write(
        tmp.path(),
        "packages/config/tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["./shared/*"] } } }"#,
    );
    write(tmp.path(), "apps/a/tsconfig.json", r#"{ "extends": "@repo/config" }"#);
    let r = discover(tmp.path(), ts(), ["apps/a/x.ts", "packages/config/package.json"]);
    assert_eq!(candidates(&r, "apps/a/x.ts", "~/m")[0], "packages/config/./shared/m");
}
