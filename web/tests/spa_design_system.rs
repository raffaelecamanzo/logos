//! SPA design-system conformance tests (S-193, CR-050, FR-UI-23, ADR-44).
//!
//! These assert the machine-checkable half of the design-system contract over the
//! AUTHORED source (the token + base stylesheets, the theme bootstrap, the
//! component CSS Modules) — the source of truth a Vite build extracts to external
//! hashed CSS. The CSP-cleanliness of the *built* bundle is guarded separately by
//! `tests/spa_bundle.rs` (over the embedded bytes); the legacy server-rendered
//! design system is guarded by `tests/design_system.rs` (over `assets/logos.css`).
//! This file is the SPA-design-system analog: tokens, dark-first theming, the
//! signal-only-red invariant, WCAG 2.1 AA contrast in BOTH themes, the
//! :focus-visible ring, reduced-motion, and the no-flash theme bootstrap.
//!
//! The contrast checks model the WCAG 2.1 relative-luminance formula over the
//! resolved semantic-token pairs in each theme — there is no headless browser in
//! CI, so the token values are resolved and compared directly, the same pattern
//! the legacy `design_system.rs` busy-overlay cascade guard uses.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `<root>/web/ui` — the SPA project root (CARGO_MANIFEST_DIR is `<root>/web`).
fn ui_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn read(rel: &str) -> String {
    let path = ui_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Remove `/* … */` CSS comments so they can't pollute selector/value scans.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The `{ … }` block body immediately following the first occurrence of `needle`,
/// brace-matched. `needle` must select the rule (e.g. `:root`, or a full attribute
/// selector). Comments must already be stripped.
fn block_after(css: &str, needle: &str) -> String {
    let i = css.find(needle).unwrap_or_else(|| panic!("selector `{needle}` not found"));
    let rest = &css[i..];
    let open = rest.find('{').expect("rule has an opening brace");
    let bytes = rest.as_bytes();
    let mut depth = 0i32;
    let start = open + 1;
    let mut j = open;
    while j < bytes.len() {
        match bytes[j] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return rest[start..j].to_string();
                }
            }
            _ => {}
        }
        j += 1;
    }
    panic!("unterminated block for `{needle}`");
}

/// Parse `--name: value;` custom-property declarations from a block body into a map.
fn declarations(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        let name = name.trim();
        if !name.starts_with("--") {
            continue;
        }
        map.insert(name.to_string(), value.trim().to_string());
    }
    map
}

/// Resolve a token value to a concrete value, following `var(--x)` references
/// through `map` (with `base` as the fallback scope for primitives). Returns the
/// resolved string (a hex/rgb/keyword literal).
fn resolve(value: &str, map: &BTreeMap<String, String>, base: &BTreeMap<String, String>) -> String {
    let mut v = value.trim().to_string();
    for _ in 0..16 {
        let Some(inner) = v.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) else {
            return v;
        };
        // var(--x) or var(--x, fallback) — take the first name.
        let name = inner.split(',').next().unwrap_or("").trim();
        v = map
            .get(name)
            .or_else(|| base.get(name))
            .cloned()
            .unwrap_or_else(|| panic!("unresolved var `{name}`"));
    }
    panic!("var resolution did not terminate for `{value}`");
}

/// Parse a `#rrggbb` (or `#rgb`) literal into linear-ready 0–255 channels.
fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim().trim_start_matches('#');
    let full = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => h.to_string(),
        _ => panic!("not a hex colour: `{hex}`"),
    };
    let n = u32::from_str_radix(&full, 16).unwrap_or_else(|_| panic!("bad hex `{hex}`"));
    (((n >> 16) & 0xff) as u8, ((n >> 8) & 0xff) as u8, (n & 0xff) as u8)
}

/// WCAG relative luminance of an sRGB colour.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    fn lin(c: u8) -> f64 {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two colours (1.0–21.0).
fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Build the effective token map for a theme: the base `:root` map, with the
/// theme's overrides applied (dark = base as-is; light = base + light block).
fn theme_map(base: &BTreeMap<String, String>, overrides: Option<&BTreeMap<String, String>>) -> BTreeMap<String, String> {
    let mut m = base.clone();
    if let Some(o) = overrides {
        for (k, v) in o {
            m.insert(k.clone(), v.clone());
        }
    }
    m
}

/// Resolve a semantic token to its RGB in a given theme map.
fn token_rgb(name: &str, map: &BTreeMap<String, String>, base: &BTreeMap<String, String>) -> (u8, u8, u8) {
    let raw = map.get(name).or_else(|| base.get(name)).unwrap_or_else(|| panic!("token `{name}` missing"));
    hex_rgb(&resolve(raw, map, base))
}

// ── 1. The authoritative primitive palette is present and exact ──────────────

#[test]
fn primitive_palette_carries_the_authoritative_sourcesense_values() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    for (name, hex) in [
        ("--so-red", "#da291c"),
        ("--so-orange", "#e35205"),
        ("--so-merlin", "#3d3935"),
        ("--so-merlin-50", "#f4f4f2"),
        ("--so-muted", "#716b5d"),
        ("--so-green", "#16a34a"),
    ] {
        assert_eq!(
            base.get(name).map(String::as_str),
            Some(hex),
            "authoritative SOURCESENSE primitive {name} must be {hex} (frontend-design §1.2)",
        );
    }
}

// ── 2. Dark-first theming via data-theme + prefers-color-scheme ──────────────

#[test]
fn dark_is_the_canonical_default_on_root() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base_body = block_after(&css, ":root");
    let base = declarations(&base_body);
    // The :root default is dark: color-scheme dark, and the page surface resolves
    // to the dark neutral, not the brand off-white.
    assert!(base_body.contains("color-scheme: dark"), ":root declares color-scheme: dark");
    let surface0 = token_rgb("--surface-0", &base, &base);
    assert_eq!(surface0, hex_rgb("#0f1216"), "the default page surface is the dark neutral");
}

#[test]
fn light_is_a_first_class_opt_in_via_data_theme() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));
    let light_map = theme_map(&base, Some(&light));
    // The explicit light theme remaps the page surface back to the brand off-white
    // and primary text to merlin — proving a theme is a token remap.
    assert_eq!(token_rgb("--surface-0", &light_map, &base), hex_rgb("#f4f4f2"));
    assert_eq!(token_rgb("--text-1", &light_map, &base), hex_rgb("#3d3935"));
    // An explicit dark theme block also exists (a dark choice survives a light OS).
    let _dark = block_after(&css, ":root[data-theme=\"dark\"]");
}

#[test]
fn first_visit_honors_prefers_color_scheme_without_an_explicit_choice() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    // A light-OS first-visit user (no data-theme set yet) gets light via the media
    // query, gated on :not([data-theme]) so a persisted choice always wins.
    assert!(css.contains("@media (prefers-color-scheme: light)"), "first-visit media query present");
    let mq = &css[css.find("@media (prefers-color-scheme: light)").unwrap()..];
    assert!(
        mq.contains(":root:not([data-theme])"),
        "the first-visit flip is gated on :root:not([data-theme]) so an explicit choice wins",
    );
}

// ── 3. --so-red is signal-only in both themes (never a large background fill) ─

#[test]
fn so_red_is_signal_only_never_a_surface_fill() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));
    let red = hex_rgb("#da291c");
    for (theme, map) in [
        ("dark", theme_map(&base, None)),
        ("light", theme_map(&base, Some(&light))),
    ] {
        // No page/card/raised surface is ever the signal red.
        for surface in ["--surface-0", "--surface-1", "--surface-2"] {
            assert_ne!(
                token_rgb(surface, &map, &base),
                red,
                "{surface} must never be the signal red in the {theme} theme (red is signal-only)",
            );
        }
        // The accent token IS the signal red in both themes (verdicts/active/focus).
        assert_eq!(
            token_rgb("--color-accent", &map, &base),
            red,
            "--color-accent stays the signal red in the {theme} theme",
        );
        assert_eq!(
            token_rgb("--focus-ring", &map, &base),
            red,
            "the focus ring is the signal red in the {theme} theme",
        );
    }
}

// ── 4. WCAG 2.1 AA contrast in BOTH themes ───────────────────────────────────

#[test]
fn text_and_signal_contrast_meets_wcag_aa_in_both_themes() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    let light = declarations(&block_after(&css, ":root[data-theme=\"light\"]"));

    for (theme, map) in [
        ("dark", theme_map(&base, None)),
        ("light", theme_map(&base, Some(&light))),
    ] {
        let s0 = token_rgb("--surface-0", &map, &base);
        let s1 = token_rgb("--surface-1", &map, &base);
        let t1 = token_rgb("--text-1", &map, &base);
        let t2 = token_rgb("--text-2", &map, &base);
        let accent = token_rgb("--color-accent", &map, &base);
        let focus = token_rgb("--focus-ring", &map, &base);

        // Body text (normal): ≥ 4.5:1 on both the page and card surfaces.
        for (label, text) in [("text-1", t1), ("text-2", t2)] {
            for (sl, surf) in [("surface-0", s0), ("surface-1", s1)] {
                let c = contrast(text, surf);
                assert!(
                    c >= 4.5,
                    "{theme}: {label} on {sl} is {c:.2}:1, below the 4.5:1 AA body minimum",
                );
            }
        }
        // Signal red + focus ring are UI/graphic affordances: ≥ 3:1 on the page.
        assert!(
            contrast(accent, s0) >= 3.0,
            "{theme}: the signal red on the page surface is below 3:1 (UI minimum)",
        );
        assert!(
            contrast(focus, s0) >= 3.0,
            "{theme}: the focus ring on the page surface is below 3:1 (UI minimum)",
        );
    }
}

#[test]
fn badge_ink_meets_wcag_aa_on_the_signal_hues() {
    let css = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&css, ":root"));
    // Badge hues are theme-independent signals, so the legible ink is too.
    let red = token_rgb("--so-red", &base, &base);
    let warm = token_rgb("--so-orange", &base, &base);
    let green = token_rgb("--so-green", &base, &base);
    let ink_red = token_rgb("--ink-on-red", &base, &base);
    let ink_warm = token_rgb("--ink-on-warm", &base, &base);
    for (label, ink, bg) in [
        ("white-on-red", ink_red, red),
        ("ink-on-orange", ink_warm, warm),
        ("ink-on-green", ink_warm, green),
    ] {
        let c = contrast(ink, bg);
        assert!(c >= 4.5, "badge {label} contrast is {c:.2}:1, below the 4.5:1 AA minimum");
    }
}

// ── 5. :focus-visible ring + reduced motion (theme-independent a11y) ──────────

#[test]
fn base_layer_has_focus_visible_ring_and_reduced_motion() {
    let base_css = read("src/styles/base.css");
    // A 2px signal-red ring on keyboard focus only (frontend-design §1.2/§7).
    assert!(base_css.contains(":focus-visible"), "a :focus-visible rule exists");
    assert!(
        base_css.contains("outline: 2px solid var(--focus-ring)"),
        "the global focus ring is 2px solid var(--focus-ring)",
    );
    // Reduced motion collapses non-essential animation/transition.
    assert!(
        base_css.contains("@media (prefers-reduced-motion: reduce)"),
        "a prefers-reduced-motion rule disables non-essential motion",
    );
}

// ── 6. Components are token-driven (a theme is a remap, no component change) ──

#[test]
fn component_modules_use_tokens_not_raw_hex_colours() {
    // Every component stylesheet must reference semantic tokens (var(--…)), never a
    // raw hex literal — so switching the theme remaps token VALUES with no
    // component change (FR-UI-23, ADR-44). Scrims use rgba(0,0,0,…) intentionally
    // (a fixed black overlay, not a themed colour); those carry no `#`.
    let dir = ui_dir().join("src");
    let mut module_files = Vec::new();
    collect_module_css(&dir, &mut module_files);
    assert!(module_files.len() >= 10, "the component library has CSS Modules: {}", module_files.len());
    let hex = regex_hex();
    for path in module_files {
        let css = strip_comments(&std::fs::read_to_string(&path).unwrap());
        for line in css.lines() {
            assert!(
                !hex(line),
                "{} contains a raw hex colour (`{}`) — components must use design tokens only",
                path.display(),
                line.trim(),
            );
        }
    }
}

// ── 7. The no-flash theme bootstrap is CSP-clean and consistent ──────────────

#[test]
fn theme_bootstrap_is_an_external_classic_head_script() {
    let index = read("index.html");
    // The served shell references the bootstrap as an EXTERNAL classic script
    // (carries src → not an inline script the self-only CSP forbids), in <head>.
    assert!(
        index.contains("<script src=\"/theme-init.js\"></script>"),
        "index.html loads /theme-init.js as an external classic head script",
    );
    let head_close = index.find("</head>").expect("index.html has a </head>");
    let script_at = index.find("theme-init.js").expect("theme-init referenced");
    assert!(script_at < head_close, "the theme bootstrap is inside <head> (runs before paint)");

    let js = read("public/theme-init.js");
    // It applies a persisted choice and is self-contained: no external origin, no eval.
    assert!(js.contains("data-theme"), "the bootstrap sets the data-theme attribute");
    assert!(js.contains("logos-theme"), "it reads the persisted choice key (mirrors theme.ts)");
    assert!(!js.contains("http://") && !js.contains("https://"), "names no external origin");
    assert!(!js.contains("eval("), "uses no eval (CSP)");
}

// ── 8. The chat view: flat turns inside the view's one card (S-300, S-308) ───
//
// [FR-UI-31] flattened the turn to the base assistant-ui grammar; [FR-UI-33]
// (S-308) put the transcript inside the application's own two-pane card grammar.
// The invariants below are declared CSS, and the SPA's Vitest run disables CSS
// processing (`css: false` — a CSS-module import is then a proxy that names ANY
// key, so class PLACEMENT reaches the jsdom DOM but a rule's DEFINITION never
// does), which makes the authored stylesheet the only place the contract can be
// checked without a headless browser — the same reasoning that puts the
// token/contrast checks above in this file.

#[test]
fn chat_assistant_turn_carries_no_card_chrome() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let assistant = rule_body(&css, ".assistant");
    for banned in ["box-shadow", "border-top", "background", "--card-accent"] {
        assert!(
            !assistant.contains(banned),
            "the assistant turn must not re-introduce `{banned}`: it is a flat, \
             left-aligned block in the assistant-ui column grammar, not a card \
             (FR-UI-31)",
        );
    }
}

/// The chat Mermaid viewer's zoom ladder is CSS, not an inline transform (S-302,
/// [FR-UI-32], NFR-SE-06): the served `default-src 'self'` policy has no `style-src`
/// escape hatch, so a `style="transform: scale(…)"` attribute would be blocked
/// exactly as Mermaid's own injected `<style>` is.
///
/// This lives here rather than in Vitest because the SPA tests run with `css: false`,
/// where the CSS-module proxy fabricates ANY key it is asked for — so a component
/// asking for a `.zoomNN` class that does not exist is unfalsifiable there. Reading
/// the stylesheet is the only way to prove each rung actually carries a scale.
#[test]
fn chat_mermaid_zoom_ladder_is_declared_as_css_classes() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Every rung the component can select must exist and scale.
    for step in [50, 67, 80, 100, 125, 150, 200, 250, 300] {
        let body = rule_body(&css, &format!(".zoom{step}"));
        assert!(
            body.contains("transform:") && body.contains("scale("),
            ".zoom{step} must carry a `transform: scale(...)` — the zoom ladder is \
             CSS-only because an inline style attribute is CSP-blocked (NFR-SE-06)",
        );
    }
    // Distinct rungs must scale DIFFERENTLY, or zoom ships dead while every test
    // that only reads the percent label stays green.
    let scales: std::collections::BTreeSet<String> = [50, 67, 80, 100, 125, 150, 200, 250, 300]
        .iter()
        .map(|s| rule_body(&css, &format!(".zoom{s}")).replace(char::is_whitespace, ""))
        .collect();
    assert_eq!(scales.len(), 9, "each zoom rung must declare its own distinct scale");
}

/// The chat Mermaid fallback must carry the label-centring rule, not just colours
/// (S-302, CR-034/S-196). Under the self-only CSP Mermaid's injected `<style>` is
/// stripped, so `getBBox()` collapses during measurement and the centring translate
/// is never applied — labels stay left-anchored and overflow their node boxes. The
/// wiki reader learned this the hard way; the chat viewer runs the same bundle under
/// the same policy, so it needs the same rule.
#[test]
fn chat_mermaid_fallback_centers_node_labels_like_the_wiki() {
    let chat = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Exact-match the RULE, the way the zoom-ladder guard above does: two
    // independent `contains` checks over the whole file would also pass with the
    // centring on some unrelated selector and the node-label selectors carrying
    // something else — which is the very drift this guard exists to catch.
    let body = rule_body(
        &chat,
        ".mermaidScale :global(.mermaid .node text), .mermaidScale :global(.mermaid .node tspan)",
    );
    assert!(
        body.contains("text-anchor: middle"),
        "the chat Mermaid fallback must set `text-anchor: middle` on the node-label \
         selectors themselves (mirrors WikiView.module.css) — without it, CSP-stripped \
         measurement leaves labels overflowing their boxes",
    );
}

/// The properties that cap an element's inline size, i.e. that set a reading
/// measure. Both the physical and the logical spelling, because a guard that
/// names one spelling guards exactly one spelling.
const MEASURE_PROPERTIES: [&str; 4] = ["max-width", "max-inline-size", "width", "inline-size"];

/// Whether a rule body CENTRES A MEASURE: it caps the element's inline size AND
/// centres the capped box horizontally. That pair — not either half alone — is what
/// S-300's `--chat-measure` column was (`max-width: var(--chat-measure)` +
/// `margin-inline: auto`), and what left the 172px void beside the rail. A cap
/// without centring (`.userBubble`'s `max-width: 80%`) is a bubble, not a measure;
/// centring without a cap is a no-op; `100%` / `none` / `auto` cap nothing.
///
/// Every centring spelling is read: `margin-inline: auto`, the logical longhands,
/// the physical pair, the `margin` shorthand in each of its 1–4 value forms, and
/// the self-alignment keywords (`justify-self`/`align-self`/`place-self: center`)
/// that centre a capped box in a grid or flex parent without touching a margin.
fn centres_a_measure(body: &str) -> bool {
    let decls = declarations_of(body);
    let get = |p: &str| decls.iter().rev().find(|(n, _)| n == p).map(|(_, v)| v.as_str());
    let capped = MEASURE_PROPERTIES
        .iter()
        .filter_map(|p| get(p))
        .any(|v| !matches!(v, "100%" | "none" | "auto"));
    let auto = |v: Option<&str>| v == Some("auto");
    let shorthand_centres = get("margin").is_some_and(|m| {
        let parts: Vec<&str> = m.split_whitespace().collect();
        match parts.as_slice() {
            [all] => *all == "auto",
            [_, inline] | [_, inline, _] => *inline == "auto",
            [_, right, _, left] => *right == "auto" && *left == "auto",
            _ => false,
        }
    });
    let centred = auto(get("margin-inline"))
        || (auto(get("margin-inline-start")) && auto(get("margin-inline-end")))
        || (auto(get("margin-left")) && auto(get("margin-right")))
        || shorthand_centres
        || ["justify-self", "align-self", "place-self"].iter().any(|p| get(p) == Some("center"));
    capped && centred
}

/// Every `(selector, body)` rule in a stylesheet, INCLUDING the rules nested in
/// `@media` / `@supports` blocks — a measure re-introduced inside a width rung is
/// just as much a measure. `@keyframes` frames are not style rules and are skipped.
fn all_style_rules(css: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (selector, body) in top_level_rules(css) {
        if selector.starts_with("@keyframes") {
            continue;
        }
        if selector.starts_with('@') {
            out.extend(all_style_rules(&body));
        } else {
            out.push((selector, body));
        }
    }
    out
}

/// A TSX source with its comments removed (`/* … */`, `{/* … */}` and `// …`), so a
/// structural scan never matches an element that is only NAMED in prose. A `//`
/// inside a string literal (a URL) is truncated too; harmless for an element scan.
fn strip_tsx_comments(src: &str) -> String {
    strip_comments(src)
        .lines()
        .map(|l| l.find("//").map_or(l, |i| &l[..i]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Byte offsets of every opening `<Tag` element in a TSX source — `<Card` followed by
/// whitespace, `>` or `/`, so `<CardList` and a closing `</Card>` never count.
fn opening_tags(src: &str, tag: &str) -> Vec<usize> {
    let needle = format!("<{tag}");
    src.match_indices(&needle)
        .filter(|(i, _)| {
            src[i + needle.len()..].chars().next().is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        })
        .map(|(i, _)| i)
        .collect()
}

/// The near misses the measure detector must tell apart. Every spelling of a
/// centred measure is ADMITTED and every one-step-away shape is not: a detector
/// proven only by deleting the real rule can never show what it wrongly admits,
/// or what spelling it misses.
#[test]
fn centred_measure_detector_admits_every_spelling_and_rejects_its_near_misses() {
    for centred in [
        "width: 100%; max-width: var(--chat-measure); margin-inline: auto",
        "max-width: 46rem; margin: 0 auto",
        "max-width: 46rem; margin: 0 auto var(--space-4)",
        "max-width: 46rem; margin: 0 auto 0 auto",
        "max-width: 46rem; margin: auto",
        "max-width: 46rem; margin-left: auto; margin-right: auto",
        "max-inline-size: 60ch; margin-inline-start: auto; margin-inline-end: auto",
        "width: 46rem; justify-self: center",
        "max-width: 46rem; align-self: center",
        "inline-size: 46rem; place-self: center",
    ] {
        assert!(centres_a_measure(centred), "a centred measure must be detected: `{centred}`");
    }
    for not_centred in [
        // The user bubble: capped, but hugging the right edge — not centred.
        "max-width: 80%; padding: var(--space-2) var(--space-3)",
        // Full-width content centred on nothing.
        "width: 100%; margin-inline: auto",
        "max-width: none; margin: 0 auto",
        // The log's negative inline margin: not `auto`, and no cap.
        "margin-inline: calc(-1 * var(--space-4)); padding: var(--space-5) var(--space-4)",
        // Only ONE side auto pushes the box to an edge; it does not centre it.
        "max-width: 46rem; margin-left: auto",
        "max-width: 46rem; margin: 0 auto 0 0",
        // A custom property NAMED like a margin is not a margin.
        "max-width: 46rem; --margin-inline: auto",
    ] {
        assert!(!centres_a_measure(not_centred), "not a centred measure: `{not_centred}`");
    }
}

/// The retired-grammar guard's replacement (S-308, [FR-UI-33], CR-092): the
/// conversation renders inside the view's ONE shared `Card`, filling its track, and
/// no rule centres a body-content measure. It supersedes S-300's
/// `chat_roles_share_one_centered_readable_measure`, which locked the centred
/// `--chat-measure` column this story retires, and keeps every assertion of it that
/// is still true: neither role self-aligns, the user turn hugs the right edge, and
/// the bubble treatment lives on the inner `.userBubble`.
#[test]
fn chat_conversation_sits_in_one_card_with_no_centred_measure() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));

    // (1) No rule anywhere in the chat stylesheet — rungs included — centres a
    // measure, and the retired measure token is gone rather than left dormant.
    for (selector, body) in all_style_rules(&css) {
        assert!(
            !centres_a_measure(&body),
            "`{selector}` centres a capped reading measure. The chat view fills its card \
             (FR-UI-33): a centred measure is what left the 172px void beside the rail",
        );
    }
    assert!(!css.contains("--chat-measure"), "the retired `--chat-measure` token is gone");

    // (2) The conversation is inside the view's ONE card. Exactly one `<Card>` in the
    // view file, and it encloses the whole thread — the transcript viewport, the
    // messages and the composer. One, not "at least one": a second card is a card
    // per turn (or per notice), which is the CR-089 chrome the flat turn removed.
    let view = strip_tsx_comments(&read("src/views/chat/ChatView.tsx"));
    let cards = opening_tags(&view, "Card");
    assert_eq!(
        cards.len(),
        1,
        "ChatView renders exactly one shared `Card` — the view's conversation container, \
         never a turn's (FR-UI-33, FR-UI-31)",
    );
    let open = cards[0];
    let close = open + view[open..].find("</Card>").expect("the view's `<Card>` is closed");
    let inside = &view[open..close];
    for part in ["<ThreadPrimitive.Root", "<ThreadPrimitive.Viewport", "<ThreadPrimitive.Messages", "<Composer"] {
        assert!(
            inside.contains(part),
            "`{part}` renders INSIDE the view's card: the transcript and the composer live in \
             that one container (FR-UI-33)",
        );
    }
    assert!(
        !inside.contains("<ThreadList"),
        "the history rail is the other pane, not part of the conversation card",
    );

    // (3) The card fills its track: nothing between the grid cell and the card caps
    // or centres it, and the composer rides the card's full content box.
    for selector in [".main", ".threadRoot", ".composer"] {
        let body = rule_body(&css, selector);
        for prop in MEASURE_PROPERTIES {
            assert!(
                !declarations_of(&body).iter().any(|(n, v)| n == prop && v != "100%"),
                "`{selector}` declares `{prop}`: the conversation fills its card and the card its \
                 track (FR-UI-33)",
            );
        }
    }

    // (4) Still true from S-300: neither role escapes the line with its own
    // alignment; the user bubble is right-aligned INSIDE the transcript.
    let user = rule_body(&css, ".user");
    assert!(!user.contains("align-self"), "the user turn aligns within the transcript, not against it");
    assert!(!rule_body(&css, ".assistant").contains("align-self"));
    assert!(user.contains("justify-content: flex-end"), "the user bubble hugs the transcript's right edge");
    assert!(
        rule_body(&css, ".userBubble").contains("background: var(--surface-2)"),
        "the bubble treatment lives on the inner .userBubble element",
    );
}

/// The two-pane grid is the WIKI's (S-308, [FR-UI-33]): the chat layout reads its
/// rail track and column gap off `WikiView.module.css` rather than restating them,
/// so the two views cannot drift apart without this failing. The rail spans the full
/// column height with no viewport-height cap of its own, and its "+ New chat" is a
/// ≥36px action pinned above the list that scrolls.
#[test]
fn chat_two_pane_grid_is_the_wiki_rail_track_at_full_column_height() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let wiki = strip_comments(&read("src/views/wiki/WikiView.module.css"));
    let prop = |body: &str, p: &str| {
        declarations_of(body).into_iter().rev().find(|(n, _)| n == p).map(|(_, v)| v)
    };
    let (chat_layout, wiki_layout) = (rule_body(&css, ".layout"), rule_body(&wiki, ".layout"));
    for p in ["grid-template-columns", "gap"] {
        let (c, w) = (prop(&chat_layout, p), prop(&wiki_layout, p));
        assert!(w.is_some(), "the Wiki `.layout` declares `{p}`");
        assert_eq!(c, w, "the chat two-pane `{p}` is the Wiki menu's, verbatim (FR-UI-33)");
    }

    // Full column height: the row stretches its items (the grid default). The Wiki's
    // own `align-items: start` is exactly what must NOT be copied — it is what sized
    // the chat rail to its items (240×99px).
    let align = prop(&chat_layout, "align-items");
    assert!(
        matches!(align.as_deref(), None | Some("stretch") | Some("normal")),
        "the chat `.layout` stretches the rail to the row (found `align-items: {align:?}`)",
    );
    let rail = rule_body(&css, ".rail");
    assert_eq!(prop(&rail, "height").as_deref(), Some("100%"), "the rail fills its stretched pane");

    // The pane is size-contained on desktop, so a long list scrolls inside the rail
    // instead of stretching the row past the card. Below the breakpoint the rail
    // stacks and must size to its content again: a containment NOT released there
    // collapses the opened narrow-viewport rail to 0px, and nothing else would say so.
    if prop(&rule_body(&css, ".railPane"), "contain").is_some_and(|v| v.contains("size")) {
        let narrow = top_level_rules(&css)
            .into_iter()
            .find(|(at, _)| at == "@media (max-width: 1023px)")
            .map(|(_, body)| body)
            .expect("the chat layout has its ≤1023px rung");
        assert_eq!(
            prop(&rule_body(&narrow, ".railPane"), "contain").as_deref(),
            Some("none"),
            "the ≤1023px rung releases the rail pane's size containment, or the opened rail \
             renders 0px tall",
        );
    }

    // No viewport-height cap on any part of the rail, and the rail as a whole does not
    // scroll — only the list does, which is what keeps "+ New chat" pinned.
    for selector in [".railPane", ".rail", ".threadList", ".newChat"] {
        let body = rule_body(&css, selector);
        for (n, v) in declarations_of(&body) {
            assert!(
                !(n.contains("height") && (v.contains("vh") || v.contains("vb"))),
                "`{selector}` declares `{n}: {v}` — the rail's bound is its grid row, not the \
                 viewport (FR-UI-33)",
            );
        }
    }
    assert!(prop(&rail, "overflow-y").is_none() && prop(&rail, "overflow").is_none(), "the rail itself does not scroll");
    let list = rule_body(&css, ".threadList");
    assert_eq!(prop(&list, "overflow-y").as_deref(), Some("auto"), "the conversation list is the scroll region");
    assert_eq!(prop(&list, "min-height").as_deref(), Some("0"), "the list can shrink below its content, so it scrolls");

    let new_chat = rule_body(&css, ".newChat");
    assert_eq!(prop(&new_chat, "flex").as_deref(), Some("none"), "+ New chat never shrinks into the list's scroll");
    let floor = prop(&new_chat, "min-height").and_then(|v| length_px(&v));
    assert!(
        floor.is_some_and(|px| px >= 36.0),
        "+ New chat is at least 36px tall (found min-height {floor:?})",
    );
}

/// The transcript stays a BOUNDED scroll box inside the card (FR-UI-33, CR-092
/// §3.2). It is assistant-ui's thread viewport, which follows the stream by driving
/// its own `scrollTop`; an unbounded box would silently stop auto-follow on the app's
/// only streaming surface and walk the composer off-screen mid-answer. The floor
/// keeps a short transcript from collapsing to its content.
#[test]
fn chat_transcript_is_a_bounded_scroll_box_inside_the_card() {
    let log = rule_body(&strip_comments(&read("src/views/chat/Chat.module.css")), ".log");
    let decls = declarations_of(&log);
    let get = |p: &str| decls.iter().rev().find(|(n, _)| n == p).map(|(_, v)| v.as_str());
    assert_eq!(get("overflow-y"), Some("auto"), "the transcript scrolls internally");
    assert!(
        get("max-height").is_some_and(|v| v != "none") || get("height").is_some_and(|v| v != "auto"),
        "the transcript declares a height bound — without one it grows with the conversation \
         and assistant-ui has no scroll box to follow the stream in",
    );
    assert!(get("min-height").is_some_and(|v| v != "0"), "the transcript has a floor");
}

#[test]
fn chat_transcript_has_generous_spacing_and_viewport_padding() {
    // With the card chrome gone, spacing is what separates one turn from the next.
    let log = rule_body(&strip_comments(&read("src/views/chat/Chat.module.css")), ".log");
    assert!(log.contains("gap: var(--space-6)"), "generous inter-turn spacing");
    assert!(log.contains("padding: var(--space-5) var(--space-4)"), "increased viewport padding");
}

#[test]
fn chat_transcript_text_on_the_card_fill_clears_wcag_aa_in_both_themes() {
    // Every rule in the chat view that declares its own ink clears the 4.5:1 AA body
    // minimum against WHATEVER IT ACTUALLY SITS ON. S-308 moved the transcript into
    // the view's `Card`, so that base is the CARD FILL — read off `Card.module.css`
    // here, not restated, so the guard follows the card if its fill ever moves. It
    // was `--surface-0` while S-300's transcript sat bare on the page.
    //
    // Two lists, and the split is the point. The turns have no fill of their own, so
    // most transcript text renders on the card fill (list a) — which is why the
    // halt/error notices and the answer links carry the signal hue on a
    // border/underline (a 3:1 UI affordance) rather than in the text colour. The
    // Activity status glyphs are the exception, because HF-1 gave them a chip fill
    // of their own to sit on (list b); measuring THOSE against the card fill would
    // measure a background they never touch. The chip geometry that makes list (b)
    // legitimate is asserted separately, by the test below this one.
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let card = strip_comments(&read("src/components/Card.module.css"));
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));

    let themes = || {
        [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))]
    };

    // The base, derived rather than assumed: the card's own fill, and nothing
    // between it and the turns repaints it. A `background` on the thread root or
    // the log would put the transcript on a surface this guard never measured.
    let card_fill = var_token(&rule_body(&card, ".card"), "background")
        .expect("the shared `.card` declares a `background: var(--…)` fill");
    for selector in [".threadRoot", ".log"] {
        assert!(
            !declarations_of(&rule_body(&css, selector)).iter().any(|(n, _)| n.starts_with("background")),
            "`{selector}` paints no fill of its own: the transcript sits on the card fill \
             ({card_fill}), which is what the pairs below are measured against",
        );
    }
    let rail_fill = var_token(&rule_body(&css, ".rail"), "background")
        .expect("the rail declares a `background: var(--…)` fill");

    // (a) Rules that ink STRAIGHT ONTO a fill they do not own — the transcript's on
    // the card fill, the rail's notes on the rail fill.
    for (selector, surface) in [
        (".halt, .error", card_fill.as_str()),
        (".markdown a", card_fill.as_str()),
        (".railEmpty, .railError", rail_fill.as_str()),
        (".railError", rail_fill.as_str()),
    ] {
        let token = color_token(&rule_body(&css, selector))
            .unwrap_or_else(|| panic!("`{selector}` declares a `color: var(--…)`"));
        for (theme, map) in themes() {
            let c = contrast(token_rgb(&token, &map, &base), token_rgb(surface, &map, &base));
            assert!(
                c >= 4.5,
                "{theme}: `{selector}` ink ({token}) is {c:.2}:1 on {surface}, below the 4.5:1 \
                 AA body minimum — a signal hue must be carried by a border/underline edge, \
                 not by the text colour",
            );
        }
    }

    // (b) The Activity status glyphs (S-301) are the sprint's near-miss and the
    // other resolution of the same rule. Added INSIDE the unfilled column a story
    // after this invariant was established, they first took the signal hues as
    // `color:` — `--color-pass` was 2.99:1 on the light page surface, under even the
    // 3:1 non-text floor (3.30:1 on today's card fill: still under AA). HF-1 put the
    // hue where the house keeps it: a `Badge`-style chip FILL. So they are measured
    // as fill/ink pairs — carrying them in list (a) would now be worse than wrong, it
    // would be vacuous, because `--ink-on-warm` never touches the card fill and
    // would pass on a contrast it does not have.
    for (selector, fill) in [(".activityRunning", "--so-orange"), (".activityDone", "--so-green")] {
        let body = rule_body(&css, selector);
        let ink = color_token(&body)
            .unwrap_or_else(|| panic!("`{selector}` declares a `color: var(--…)`"));
        let declared = var_token(&body, "background").unwrap_or_else(|| {
            panic!(
                "`{selector}` declares a `background: var(--…)`: the signal hue is this glyph's \
                 FILL, not its ink — as ink on the card fill it does not clear AA",
            )
        });
        assert_eq!(
            declared, fill,
            "`{selector}` carries the state's own signal hue as its fill",
        );
        for (theme, map) in themes() {
            let c = contrast(token_rgb(&ink, &map, &base), token_rgb(fill, &map, &base));
            assert!(
                c >= 4.5,
                "{theme}: `{selector}` ink ({ink}) is {c:.2}:1 on its own fill ({fill}), below the \
                 4.5:1 AA minimum the `Badge` chips hold to — a fill only carries a signal if the \
                 glyph on it stays legible",
            );
        }
    }
}

/// The other half of the fill/ink-pair affordance the contrast guard above measures:
/// the glyphs must actually be CHIPS. Without the `Badge` geometry a fill reads as
/// tinted text on a stray coloured background — and the measurement above would go
/// on passing, because a fill/ink ratio says nothing about whether the fill looks
/// deliberate. Separate test rather than a third block up there: this is a structural
/// assertion, not a contrast one, and a failure should say so by name.
#[test]
fn chat_activity_status_glyphs_carry_badge_chip_geometry() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    let chip = rule_body(&css, ".activityRunning, .activityDone");
    for decl in [
        "display: inline-flex",
        "align-items: center",
        "border-radius: var(--radius-sm)",
        "padding: 0.2rem 0.45rem",
        "border: 1px solid transparent",
        "line-height: 1",
    ] {
        assert!(
            chip.contains(decl),
            "the Activity glyphs carry `Badge`'s chip geometry (`{decl}`) so the signal hue reads \
             as an affordance, not as tinted text",
        );
    }
}

/// The house's signal hues — the tokens that mean something (danger, in-progress,
/// done) rather than the neutral inks (`--text-1`/`--text-2`, asserted against every
/// surface in `text_and_signal_contrast_meets_wcag_aa_in_both_themes`) and the
/// on-fill inks (`--ink-on-*`, asserted against their hues in `badge_ink_meets_…`).
/// Carrying one of these as `color:` is the decision this module polices.
const SIGNAL_HUES: &[&str] = &[
    "--color-accent",
    "--color-accent-warm",
    "--color-pass",
    "--so-red",
    "--so-orange",
    "--so-green",
    "--so-lime",
    "--so-merlin",
];

/// Every rule in `Chat.module.css` that inks a SIGNAL HUE, and why that is allowed.
/// This list is the coverage contract: the test below fails on any signal-hue
/// `color:` rule missing from it, so a new one cannot be added without a decision.
///
/// A rule reaching for a signal hue has three legitimate resolutions, and only the
/// third leaves the hue in `color:` — which is why this list is short:
/// - **Move it to a fill** — carry the hue as `background:` under an `--ink-on-*`
///   ink with `Badge` chip geometry. That is what HF-1 did to `.activityRunning` /
///   `.activityDone`, and it takes them OUT of this list: they now ink
///   `--ink-on-warm`, and the fill/ink pair is measured by
///   `chat_transcript_text_on_the_card_fill_clears_wcag_aa_in_both_themes`.
/// - **Drop it** — keep `--text-1` and carry the signal on a border/underline, as
///   `.halt, .error`, `.markdown a` and (since S-308) `.railError` do. Also not in
///   this list, for the same reason: `.railError` inked `--color-accent` at 3.55:1 on
///   the dark rail fill, which is text, and text needs 4.5:1.
/// - **Keep it, as a UI affordance** — a glyph that is a graphic signal rather than
///   body text, where the hue need only clear the ≥3:1 non-text floor. The test
///   below measures that floor for each entry against the surface named beside it,
///   in both themes. These are the entries below; each one is a deliberate
///   exception, not a default.
///
/// The surface is the one the glyph is painted on: the rule's OWN `background` when
/// it declares one (the test checks the two agree), else the fill it sits on — the
/// view card's `--surface-1` for the transcript (S-308).
const SIGNAL_INK_COVERAGE: &[(&str, &str, &str)] = &[
    (".threadDelete:hover, .threadDelete:focus-visible", "--surface-2", "UiAffordance: rail ✕ icon button"),
    (".streaming::after", "--surface-1", "UiAffordance: streaming caret glyph on the card fill"),
];

/// The coverage half of the S-300 invariant, and the guard that would have caught
/// S-301 on the story that introduced it.
///
/// S-300 encoded "a signal hue cannot be ink on the unfilled column" as measurements
/// over a HARDCODED selector list. S-301 then added `.activityRunning`/`.activityDone`
/// to the same stylesheet a story later, inking `--color-pass` at 2.99:1 — and the
/// guard said nothing, because a closed list cannot notice what it does not name.
/// The fix is to invert it: enumerate the stylesheet and require every signal-hue ink
/// to be CLASSIFIED, so the omission fails loudly instead of passing silently.
#[test]
fn every_signal_hue_ink_in_the_chat_stylesheet_is_classified() {
    let css = strip_comments(&read("src/views/chat/Chat.module.css"));
    // Rungs included: a signal ink re-introduced inside a `@media` block is just as
    // much a signal ink, and a top-level-only walk would never see it.
    for (selector, body) in all_style_rules(&css) {
        let Some(token) = color_token(&body) else { continue };
        if !SIGNAL_HUES.contains(&token.as_str()) {
            continue;
        }
        assert!(
            SIGNAL_INK_COVERAGE.iter().any(|(sel, _, _)| *sel == selector),
            "`{selector}` inks the signal hue `{token}` but is not classified in \
             `SIGNAL_INK_COVERAGE`. On the unfilled transcript column a signal hue does \
             not clear 4.5:1 as text — move it to a `background:` fill under an \
             `--ink-on-*` ink with `Badge` chip geometry, or drop it and carry the signal \
             on a border/underline. If this rule is chrome OUTSIDE that column it may keep \
             the hue as a ≥3:1 UI affordance — then add it below with its reason. Pick one \
             and record it: leaving a rule unclassified is how S-301 shipped at 2.99:1.",
        );
    }
    // …and the contract cannot rot in the other direction either: a classified
    // selector that no longer inks a signal hue is stale and must be dropped, or the
    // list slowly becomes a record of what the stylesheet used to look like.
    let tokens = strip_comments(&read("src/styles/tokens.css"));
    let base = declarations(&block_after(&tokens, ":root"));
    let light = declarations(&block_after(&tokens, ":root[data-theme=\"light\"]"));
    for (selector, surface, note) in SIGNAL_INK_COVERAGE {
        let body = rule_body(&css, selector);
        let token = color_token(&body).unwrap_or_else(|| {
            panic!("`{selector}` is classified ({note}) but declares no `color: var(--…)`")
        });
        assert!(
            SIGNAL_HUES.contains(&token.as_str()),
            "`{selector}` is classified as a signal-hue ink ({note}) but now inks \
             `{token}`, which is not a signal hue — drop it from `SIGNAL_INK_COVERAGE`",
        );
        // A classified affordance still has to BE one: ≥3:1 against what it is painted
        // on, in both themes. The surface is the rule's own fill when it declares one.
        if let Some(own) = var_token(&body, "background") {
            assert_eq!(
                own, *surface,
                "`{selector}` paints its own `{own}` fill; classify it against that surface",
            );
        }
        for (theme, map) in [("dark", theme_map(&base, None)), ("light", theme_map(&base, Some(&light)))] {
            let c = contrast(token_rgb(&token, &map, &base), token_rgb(surface, &map, &base));
            assert!(
                c >= 3.0,
                "{theme}: `{selector}` ({note}) inks {token} at {c:.2}:1 on {surface}, below the \
                 3:1 non-text floor a UI affordance must clear",
            );
        }
    }
}

// ── The shell header's progressive disclosure (S-317, FR-UI-34, CR-089/CR-092) ──

/// The two rungs of the disclosure ladder, in CSS px: the complements of the tiers
/// frontend-design §7 documents (desktop ≥1024px, tablet ≥768px), and the shell's
/// existing collapse point (`AppShell.module.css`, `Chat.module.css`).
///
/// Pinned as constants because a rung is the one value in this story that can drift
/// in silence. Every rendered measurement in the S-317 record was taken by hand,
/// once, against these two numbers; move them to 419px and 400px and both elements
/// are back on the row at the exact viewport CR-089 measured, with nothing to say so.
const READOUT_RUNG_PX: f64 = 1023.0;
const SUBTITLE_RUNG_PX: f64 = 767.0;

/// Every declaration that takes an element off the page. `display: none` is the one
/// the header uses; the other two are here because an element hidden by any of them
/// is just as lost to the reader, and a survivor guard that names one spelling
/// guards exactly one spelling.
const HIDING_DECLARATIONS: [(&str, &str); 3] =
    [("display", "none"), ("visibility", "hidden"), ("opacity", "0")];

/// Every `(selector, rung px)` the header stylesheet hides inside a width rung.
///
/// A rung whose width cannot be read as a length **panics** rather than being
/// skipped. The first draft of this walk skipped it, and an `em`-expressed rung — or
/// a reformatted `max-width :`, which is valid CSS — then carried whatever it hid
/// past every assertion below without a word.
/// Every stylesheet whose classes render into the header row.
///
/// The survivor guard below is a claim about what the reader can still SEE in
/// that row, and the row is not one file: S-317 gave the member selector its own
/// width concessions in its own module, in the same commit that wrote the guard.
/// A survivor's stylesheet has to be inside the walk, not exempted by a sentence
/// next to it.
///
/// S-425 moved the member SELECTOR off this row into the sidebar's Service-section
/// header, and `MemberSelector.module.css` left the walk with it. What stayed on the
/// row is the workspace-probe fault badge, whose rules moved to
/// `WorkspaceFault.module.css` in the same change — so the row still has two
/// stylesheets and the guard still covers every survivor on it. Dropping the
/// selector's module WITHOUT adding the badge's would have left this walk reading the
/// header's own file alone, which is the exact shape the doc comment above records as
/// having been wrong once.
const HEADER_ROW_STYLESHEETS: [&str; 2] = [
    "src/shell/Header.module.css",
    "src/shell/WorkspaceFault.module.css",
];

fn header_disclosure_ladder(stylesheet: &str) -> Vec<(String, f64)> {
    let css = strip_comments(&read(stylesheet));
    let mut out = Vec::new();
    for (at_rule, body) in top_level_rules(&css) {
        if !at_rule.starts_with("@media") || !at_rule.to_ascii_lowercase().contains("max-width") {
            continue;
        }
        let px = max_width_px(&at_rule).unwrap_or_else(|| {
            panic!(
                "`{at_rule}` names a max-width this suite cannot read as a length. \
                 Skipping it would hide whatever the rung hides from every assertion \
                 in this section — express the rung in px or rem, or teach \
                 `length_px` the unit",
            )
        });
        for (selector, decls) in top_level_rules(&body) {
            if hides_element(&decls) {
                out.push((selector, px));
            }
        }
    }
    out
}

/// The WIDEST rung at which `selector` is dropped — as the viewport narrows it is
/// the rung reached first, so it is the one that fixes the disclosure order.
fn header_drops_at(selector: &str) -> Option<f64> {
    HEADER_ROW_STYLESHEETS
        .iter()
        .flat_map(|sheet| header_disclosure_ladder(sheet))
        .filter(|(sel, _)| sel == selector)
        .map(|(_, px)| px)
        .reduce(f64::max)
}

/// The rungs themselves. Everything else asserted about the ladder is relative —
/// the order test only says one rung is wider than the other — so the whole ladder
/// could slide off the documented tiers while every other test here stayed green.
#[test]
fn header_disclosure_rungs_are_the_documented_tiers() {
    assert_eq!(
        header_drops_at(".status"),
        Some(READOUT_RUNG_PX),
        "the graph-state readout gives way at the tablet tier the AppShell sidebar \
         and the Chat rail already collapse at (frontend-design §7)",
    );
    assert_eq!(
        header_drops_at(".brandSub"),
        Some(SUBTITLE_RUNG_PX),
        "the brand subtitle gives way below tablet (frontend-design §7)",
    );
}

/// Nothing in the header is hidden UNCONDITIONALLY. Every other assertion in this
/// section looks inside a rung, so a `display: none` at the top level — the readout
/// gone at 1600px as surely as at 420px — was invisible to all of them, and the SPA
/// suite cannot see it either (`css: false`, so jsdom evaluates no stylesheet).
#[test]
fn header_hides_nothing_unconditionally() {
    let css = strip_comments(&read("src/shell/Header.module.css"));
    for (selector, body) in top_level_rules(&css) {
        if selector.starts_with('@') {
            continue;
        }
        assert!(
            !hides_element(&body),
            "`{selector}` is hidden at EVERY width rather than inside a rung — the \
             readout is required PRESENT at desktop widths (FR-UI-34), and the \
             acceptance grid records it rendered at 1024px and 1600px",
        );
    }
}

/// The priority the design contract states (frontend-design §3, re-baselined by
/// CR-097): the readout gives way FIRST, the brand subtitle SECOND, so the brand
/// lockup, the workspace-probe fault badge and the theme toggle survive to the
/// narrowest supported viewport. (The member selector was on this row until S-425
/// moved it into the sidebar's Service-section header.) `.status` is the readout's
/// whole slot — every one of its four states renders into it (`Header.test.tsx`
/// binds that end).
#[test]
fn header_drops_the_readout_before_the_brand_subtitle() {
    let readout = header_drops_at(".status")
        .expect("the graph-state readout (`.status`) must be dropped at some rung");
    let subtitle = header_drops_at(".brandSub")
        .expect("the brand subtitle (`.brandSub`) must be dropped at some rung");
    assert!(
        readout > subtitle,
        "the readout gives way FIRST, so its rung ({readout}px) must be WIDER than \
         the brand subtitle's ({subtitle}px) — equal rungs drop both at once and \
         state no order at all (FR-UI-34)",
    );
}

/// A dropped readout is ABSENT, not truncated. An ellipsis would present a clipped
/// `rev 3,9…` as a figure, which is the reporting failure NFR-CC-04 forbids.
///
/// The clip ban is scoped to the readout's own rules, deliberately. Banning it over
/// the whole stylesheet also blocks the **project root path** that frontend-design
/// §3 records as a known-unbuilt header element — a path is the canonical ellipsis
/// case, and the story that builds it would fail a test whose message talks about
/// the readout.
#[test]
fn header_drops_the_readout_absent_never_truncated() {
    let css = strip_comments(&read("src/shell/Header.module.css"));
    assert!(
        header_drops_at(".status").is_some(),
        "the readout must be removed, not merely narrowed",
    );
    for selector in [".status", ".readout"] {
        let body = rule_body(&css, selector);
        for needle in ["text-overflow", "ellipsis"] {
            assert!(
                !body.contains(needle),
                "`{selector}` declares `{needle}` — a clipped readout presents a \
                 truncation as a measurement (NFR-CC-04, NFR-RA-05)",
            );
        }
    }
}

/// …and nothing ELSE gives way. The brand lockup, the workspace-probe fault badge and
/// the theme toggle survive to the narrowest supported viewport (FR-UI-29,
/// NFR-RA-05): a workspace whose roster could not be read must say so at every width.
///
/// The ladder counts every mechanism in `HIDING_DECLARATIONS`, not `display` alone —
/// a `visibility: hidden` on the brand lockup loses the home link just as completely
/// and once slipped past this assertion.
///
/// It walks every stylesheet in `HEADER_ROW_STYLESHEETS`, not the header's own.
/// This guard previously read `Header.module.css` alone while its own message
/// named the member selector as the survivor that mattered — and that selector's
/// rules lived in a module of their own, which the same story had created width
/// concessions in. (S-425 moved the selector off this row; the survivor whose rules
/// sit outside the header's file is now the fault badge, in
/// `WorkspaceFault.module.css`. The lesson is unchanged, and so is the walk.)
/// Appending
///
/// ```css
/// @media (max-width: 420px) { .select { display: none } }
/// ```
///
/// to that module removed the selector at the narrowest supported viewport and
/// left this suite green at 25/25. A survivor guarded by a sentence is not
/// guarded.
#[test]
fn header_hides_nothing_but_the_readout_and_the_brand_subtitle() {
    let mut hidden: Vec<String> = HEADER_ROW_STYLESHEETS
        .iter()
        .flat_map(|sheet| header_disclosure_ladder(sheet))
        .map(|(sel, _)| sel)
        .collect();
    hidden.sort();
    hidden.dedup();
    assert_eq!(
        hidden,
        vec![".brandSub".to_string(), ".status".to_string()],
        "only the readout and the brand subtitle give way, across every stylesheet \
         that renders into the header row; anything else here is a survivor the \
         narrow viewport has lost (FR-UI-29, FR-UI-34)",
    );
}

/// The theme toggle is a `Button`, so its module is the third way off the row —
/// and it is a SHARED component module, where a width rung would be an app-wide
/// change rather than a header disclosure. Asserted separately, and precisely:
/// the toggle cannot be dropped from there because nothing there is width-rung'd
/// at all. If that ever changes, this fails with its own message rather than
/// surfacing as a confusing failure in the header ladder above.
#[test]
fn the_theme_toggles_module_declares_no_width_rung() {
    let css = strip_comments(&read("src/components/Button.module.css"));
    let rungs: Vec<String> = top_level_rules(&css)
        .into_iter()
        .map(|(at_rule, _)| at_rule)
        .filter(|at_rule| {
            at_rule.starts_with("@media") && at_rule.to_ascii_lowercase().contains("width")
        })
        .collect();
    assert!(
        rungs.is_empty(),
        "the theme toggle survives to the narrowest supported viewport (FR-UI-29); \
         `Button.module.css` now declares a width rung, so check whether the toggle \
         can be dropped from it: {rungs:?}",
    );
}

/// The sidebar's scope labels — "Workspace" and "Service" — are what tell two views
/// registered under the same name apart (ADR-66), so they must be readable at every
/// supported width, 420px included ([FR-UI-35], [NFR-CC-04]). The markup half (that
/// they are text, in a heading, in the rendered DOM) is asserted in
/// `web/ui/src/shell/Sidebar.test.tsx`; this is the stylesheet half, which that
/// suite cannot see because it runs with `css: false`.
///
/// Asserted as "this module declares no width rung at all", the same shape as the
/// theme toggle's guard above and for the same reason: it is a fact about the file
/// rather than about one spelling of hiding, so a rung that hid the label with
/// `visibility`, `opacity`, `content-visibility` or a negative `text-indent` fails
/// here too. A rung added for some unrelated sidebar tweak fails this with its own
/// message, which is the moment to check whether the labels survive it.
#[test]
fn sidebar_scope_label_survives_every_breakpoint() {
    let css = strip_comments(&read("src/shell/Sidebar.module.css"));

    // The label rule exists and is the text treatment, not a decoration: if it is
    // gone, an empty rung list below would otherwise report success over nothing.
    let label = rule_body(&css, ".sectionLabel");
    assert!(
        !label.trim().is_empty(),
        "`.sectionLabel` is the scope label's only styling hook; this suite's rung \
         assertion is worthless if the rule has been renamed or removed",
    );
    for (prop, value) in HIDING_DECLARATIONS {
        assert!(
            !declarations_of(&label).iter().any(|(n, v)| n == prop && v == value),
            "`.sectionLabel` declares `{prop}: {value}` — the scope label is the only \
             thing distinguishing two same-named views (ADR-66), so it is never taken \
             off the page",
        );
    }

    let rungs: Vec<String> = top_level_rules(&css)
        .into_iter()
        .map(|(at_rule, _)| at_rule)
        .filter(|at_rule| {
            at_rule.starts_with("@media") && at_rule.to_ascii_lowercase().contains("width")
        })
        .collect();
    assert!(
        rungs.is_empty(),
        "the sidebar's scope labels survive to the narrowest supported viewport \
         (FR-UI-35, NFR-CC-04); `Sidebar.module.css` now declares a width rung, so \
         check whether the labels can be dropped from it: {rungs:?}",
    );
}

/// The member selector sits in the sidebar's Service-section header (S-425); it
/// shared the app header's row with the graph-state readout until then. The header
/// sits in a 232px column at desktop width (≥1024px; below that the sidebar takes
/// the full viewport), and a `<select>` sizes itself to its LONGEST option — on the
/// reference workspace a 42-character member name, measured at 467px, which
/// overflowed a 420px viewport by 242px even after the header had dropped both of
/// its own elements, and which overflows the narrower column by more. So the header
/// must be able to give width back, the label must yield by ellipsis rather than
/// overflow or vanish, and the control must stop yielding while it is still a
/// control. All three are asserted, because in the shared row each one alone was a
/// defect: no `min-width: 0` on the label was an overflow, no floor on the control
/// was one measured at 24px, and no ellipsis on the label was a clipped name.
///
/// Since CR-145 the header is a COLUMN — the label on its own row, the control at
/// full width beneath it (frontend-design §3) — because the shared row it replaced
/// gave width to a long member name and truncated the label to `Se…`. The label no
/// longer competes with the control for width, so its yield rungs are a BACKSTOP
/// rather than the row's width ordering: the ellipsis rungs engage only for a label
/// longer than the column, and the two `min-width: 0` rungs are inert in the column
/// and guard a return to a row. They are still declared, and this test asserts them
/// unchanged — so its body's own row-era wording ("yields BEFORE the control") is
/// kept byte-identical by the same rule. The stacking itself is
/// verified against rendered state at review, not here: this suite reads
/// declarations, and no declaration check can tell a column that fits from one
/// that does not.
///
/// It reads TWO stylesheets, because the header is two components: the header and
/// its label belong to the sidebar, the control to its own module. An earlier
/// version of this test read the selector's module alone, back when that module
/// owned a `.selector` wrapper and a `.label`; both were deleted in S-425 when the
/// section heading became the control's label, and a test that had kept reading them
/// would have failed on absence rather than on the property.
#[test]
fn the_service_section_header_gives_width_back_but_stays_a_control() {
    let sidebar = strip_comments(&read("src/shell/Sidebar.module.css"));
    let selector = strip_comments(&read("src/shell/MemberSelector.module.css"));
    let decl = |css: &str, sel: &str, prop: &str| -> Option<String> {
        declarations_of(&rule_body(css, sel))
            .into_iter()
            .find(|(name, _)| name == prop)
            .map(|(_, value)| value)
    };

    assert_eq!(
        decl(&sidebar, ".sectionHeader", "min-width").as_deref(),
        Some("0"),
        "`.sectionHeader` must lift its automatic minimum size, or the longest \
         member name pins the row at that width and the column overflows (FR-UI-34)",
    );

    // The label yields BEFORE the control — and by ellipsis, never by removal: it is
    // the `<select>`'s accessible name as well as the section's (ADR-66), so taking
    // it off the page trades a layout defect for an accessibility one. That it is
    // never width-rung'd away is asserted in `sidebar_scope_label_survives_every_breakpoint`.
    for (prop, value) in [("min-width", "0"), ("overflow", "hidden"), ("text-overflow", "ellipsis")]
    {
        assert_eq!(
            decl(&sidebar, ".sectionLabel", prop).as_deref(),
            Some(value),
            "`.sectionLabel` must yield BEFORE the control does; without `{prop}: \
             {value}` the label cannot shrink and puts the select straight onto its \
             floor",
        );
    }

    let floor = decl(&selector, ".select", "min-width")
        .unwrap_or_else(|| panic!("`.select` declares no `min-width` floor"));
    let floor_px = length_px(&floor).unwrap_or_else(|| {
        panic!(
            "`.select` min-width is `{floor}`, which is not a length — `auto` and \
             `min-content` are the intrinsic sizings `min-width: 0` on the row exists \
             to defeat, so they are not floors",
        )
    });
    assert!(
        floor_px > 0.0,
        "`.select` must keep a NON-ZERO `min-width` floor — a control squeezed to a \
         sliver is present but not reachable (FR-UI-29); found `{floor}`",
    );

    // And the cap must be the container, not a fixed length wider than it: 18rem
    // (288px) exceeded the 232px column outright, which is what the former header
    // row could afford and this one cannot.
    assert_eq!(
        decl(&selector, ".select", "max-width").as_deref(),
        Some("100%"),
        "`.select`'s cap must be its container — a fixed cap wider than the 232px \
         sidebar column overflows it at every viewport width",
    );

    // The deleted rules stay deleted: a `.selector` wrapper or a `.label` reappearing
    // here means the control has grown a second name beside the section heading, and
    // the assertions above would then be guarding the wrong element.
    let declared: Vec<String> =
        top_level_rules(&selector).into_iter().map(|(sel, _)| sel).collect();
    for gone in [".selector", ".label"] {
        assert!(
            !declared.iter().any(|sel| sel == gone),
            "`{gone}` is back in `MemberSelector.module.css`; S-425 made the section \
             heading the control's label, so a second label element is a duplicate \
             name on the row (FR-UI-35, NFR-CC-04). Declared: {declared:?}",
        );
    }
}

/// The Workspace section reads as ONE list (S-454, [FR-UI-35], [FR-UI-37]). Its two
/// CR-042 groups stay two `<ul>`s — the group says what a tab answers, and the
/// app-scoped Statistics tab shares its member-scoped twin's group on purpose — but
/// the hairline between them left that Statistics entry stranded between the
/// hairline and the Workspace/Service boundary, reading as belonging to neither
/// scope. So the inter-group hairline and gap are suppressed inside that section
/// only, by a section-scoped rule.
///
/// Four facts, because each alone is a half-applied fix that looks like a working
/// one: the scoped rule zeroes the border and both inter-group paddings; the
/// unscoped `.group` border and `.group + .group` gap are still declared (the
/// Service section's hairlines and gaps are unchanged); the section boundary still
/// declares its own; and the scoped rule, declared once, comes AFTER every rule it
/// ties with. `.appSection .group` is two classes, exactly
/// as `.group:last-child` and `.group + .group` are, so source order decides — placed
/// above `.group + .group`, the second Workspace group would keep its top gap.
///
/// The markup half — that the Workspace region still renders two lists and the
/// Service region three, and that the Workspace section and only it carries
/// `appSection` — is asserted in `web/ui/src/shell/Sidebar.test.tsx`; this is the
/// stylesheet half, which that suite cannot see because it runs with `css: false`.
#[test]
fn the_workspace_section_renders_its_groups_as_one_list() {
    const SCOPED: &str = ".appSection .group";
    let css = strip_comments(&read("src/shell/Sidebar.module.css"));
    // The LAST declaration of a property, because that is the one the cascade
    // applies: `padding-top: 0; padding-top: var(--space-3)` renders the gap.
    let decl = |sel: &str, prop: &str| -> Option<String> {
        declarations_of(&rule_body(&css, sel))
            .into_iter()
            .rfind(|(name, _)| name == prop)
            .map(|(_, value)| value)
    };
    // `none` zeroes a border but is INVALID for padding — the browser drops the
    // declaration and the unscoped padding applies — so it counts only for borders.
    let zero = |prop: &str, v: &str| {
        length_px(v) == Some(0.0) || (prop.starts_with("border") && v == "none")
    };

    // The scoped rule zeroes the hairline and the gap on both sides of it. Read as
    // longhands, so a shorthand in the same body is refused by name: appended after
    // the longhands it would win, and this read would never see it.
    let scoped = declarations_of(&rule_body(&css, SCOPED));
    for shorthand in ["padding", "border", "border-width", "border-style"] {
        assert!(
            !scoped.iter().any(|(name, _)| name == shorthand),
            "`{SCOPED}` declares the shorthand `{shorthand}`; this guard reads longhands, \
             so spell the zeroed edges as `border-bottom` / `padding-top` / `padding-bottom`",
        );
    }
    for prop in ["border-bottom", "padding-top", "padding-bottom"] {
        let value = decl(SCOPED, prop)
            .unwrap_or_else(|| panic!("`{SCOPED}` declares no `{prop}` (as a longhand)"));
        assert!(
            zero(prop, &value),
            "`{SCOPED}` must zero `{prop}` — the Workspace section renders as one list \
             (FR-UI-35); found `{value}`",
        );
    }

    // The unscoped rules still declare the hairline and the top gap the scoped one
    // suppresses: the Service section keeps both.
    for (sel, prop) in [(".group", "border-bottom"), (".group + .group", "padding-top")] {
        let value = decl(sel, prop).unwrap_or_else(|| panic!("`{sel}` declares no `{prop}`"));
        assert!(
            !zero(prop, &value),
            "`{sel}` must still declare a non-zero `{prop}` — the suppression is scoped \
             to the Workspace section, and the Service section's groups keep it; found \
             `{value}`",
        );
    }

    // The Workspace/Service boundary is its own rule and keeps its own hairline.
    let boundary = decl(".section + .section", "border-top")
        .unwrap_or_else(|| panic!("`.section + .section` declares no `border-top`"));
    assert!(
        !zero("border-top", &boundary),
        "the Workspace/Service boundary must keep its hairline; found `{boundary}`",
    );

    // Source order: the scoped rule wins the specificity tie only by coming later.
    let order: Vec<String> = top_level_rules(&css).into_iter().map(|(sel, _)| sel).collect();
    // Declared once: a second `{SCOPED}` later in the file would override this one,
    // and `rule_body` reads only the first.
    assert_eq!(
        order.iter().filter(|s| *s == SCOPED).count(),
        1,
        "`{SCOPED}` must be declared exactly once",
    );
    let at = |sel: &str| {
        order
            .iter()
            .position(|s| s == sel)
            .unwrap_or_else(|| panic!("rule `{sel}` not found in the stylesheet"))
    };
    for tied in [".group:last-child", ".group + .group"] {
        assert!(
            at(SCOPED) > at(tied),
            "`{SCOPED}` ties with `{tied}` on specificity (two classes each), so it must \
             come AFTER it in source order to win; it comes before",
        );
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// A CSS length in px (`rem` resolved at the 16px root), or `None` when the value is
/// not a length this suite understands — `auto`, `min-content`, a percentage. Unit
/// keywords are ASCII case-insensitive in CSS, so the comparison is too.
fn length_px(value: &str) -> Option<f64> {
    let v = value.trim().to_ascii_lowercase();
    let digits: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let n: f64 = digits.parse().ok()?;
    match v[digits.len()..].trim() {
        "px" => Some(n),
        "rem" => Some(n * 16.0),
        // A bare `0` is a valid CSS length and needs no unit.
        "" if n == 0.0 => Some(0.0),
        _ => None,
    }
}

/// The `max-width` a media query names, in CSS px, or `None` when it names none.
/// Lowercased and whitespace-normalised first, so `max-width : 1023PX` — valid CSS,
/// and what a reformat can produce — is read rather than silently missed.
fn max_width_px(query: &str) -> Option<f64> {
    let q = query.to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let at = q.find("max-width")? + "max-width".len();
    let rest = q[at..].trim_start().strip_prefix(':')?;
    length_px(rest.split(')').next()?)
}

/// The `property: value` declarations of a rule body, property names lowercased and
/// values whitespace-collapsed. Split per declaration rather than scanned as text,
/// so a custom property (`--display: none`) is never mistaken for the property it is
/// named after, and a declaration broken across lines still reads.
fn declarations_of(body: &str) -> Vec<(String, String)> {
    body.split(';')
        .filter_map(|decl| decl.split_once(':'))
        .map(|(name, value)| {
            (
                name.split_whitespace().collect::<String>().to_ascii_lowercase(),
                value.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase(),
            )
        })
        .collect()
}

/// Whether a rule body takes its element off the page, by any mechanism in
/// `HIDING_DECLARATIONS`.
fn hides_element(body: &str) -> bool {
    let declared = declarations_of(body);
    HIDING_DECLARATIONS
        .iter()
        .any(|(prop, value)| declared.iter().any(|(n, v)| n == prop && v == value))
}

/// Every top-level `selector { body }` pair in a stylesheet, selectors normalised
/// to single-spaced form (`".empty,\n.user"` → `".empty, .user"`). Comments must
/// already be stripped. At-rule bodies (`@media`, `@keyframes`) are returned under
/// the at-rule's own "selector" and are not descended into BY THIS CALL; a caller
/// that wants the rules inside a rung re-invokes it on the at-rule body, which is
/// what the header-disclosure section above does.
fn top_level_rules(css: &str) -> Vec<(String, String)> {
    let bytes = css.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut sel_start) = (0usize, 0usize);
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let selector = css[sel_start..i].split_whitespace().collect::<Vec<_>>().join(" ");
        let body_start = i + 1;
        let mut depth = 0i32;
        let mut j = i;
        while j < bytes.len() {
            match bytes[j] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        assert!(
            j < bytes.len(),
            "unterminated block for selector `{selector}` — a malformed stylesheet must fail \
             loudly here, not be absorbed into one giant trailing rule (cf. `block_after`)",
        );
        out.push((selector, css[body_start..j].to_string()));
        i = j + 1;
        sel_start = i;
    }
    out
}

/// The declaration body of the rule whose FULL selector list is exactly `selector`.
/// Exact-matching (not substring) is what keeps `.user` off `.userBubble` and the
/// standalone `.assistant` rule off the grouped `.empty, .user, .assistant` one.
fn rule_body(css: &str, selector: &str) -> String {
    top_level_rules(css)
        .into_iter()
        .find(|(sel, _)| sel == selector)
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("rule `{selector}` not found in the stylesheet"))
}

/// The `--token` named by the first `color: var(--token)` declaration in a rule body.
fn color_token(body: &str) -> Option<String> {
    var_token(body, "color")
}

/// The `--token` named by the first `<property>: var(--token)` declaration in a rule
/// body. A declaration that is not a `var(…)` (a keyword, or a literal) is skipped
/// rather than ending the scan, so a rule that declares a fallback before its token
/// — `color: inherit; color: var(--text-1)` — still resolves.
///
/// The value must be a single unadorned `var(…)`: a SHORTHAND that merely contains
/// one (`background: var(--x) no-repeat`) is skipped, not parsed. That never came up
/// while this only read `color`, which is never a shorthand, but `background` is —
/// so a caller that starts matching compound backgrounds must widen this first. It
/// fails loudly rather than silently (both call sites `panic!` on `None`), so the
/// failure mode is a confusing message, never a wrong token measured as if right.
fn var_token(body: &str, property: &str) -> Option<String> {
    for decl in body.split(';') {
        let Some((name, value)) = decl.split_once(':') else { continue };
        if name.trim() != property {
            continue;
        }
        let Some(inner) = value.trim().strip_prefix("var(").and_then(|v| v.strip_suffix(')'))
        else {
            continue;
        };
        return inner.split(',').next().map(|n| n.trim().to_string());
    }
    None
}

fn collect_module_css(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_module_css(&path, out);
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".module.css")) {
            out.push(path);
        }
    }
}

/// A tiny `#rrggbb`/`#rgb` hex-colour detector (no regex crate dependency): true
/// when a line contains a `#` followed by exactly 3 or 6 hex digits at a boundary.
fn regex_hex() -> impl Fn(&str) -> bool {
    |line: &str| {
        let bytes = line.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if b != b'#' {
                continue;
            }
            let run = bytes[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            // A trailing non-hex boundary (or EOL) distinguishes #fff/#ffffff from
            // longer alnum tokens (e.g. an id fragment).
            let boundary = bytes
                .get(i + 1 + run)
                .map(|c| !c.is_ascii_alphanumeric())
                .unwrap_or(true);
            if (run == 3 || run == 6) && boundary {
                return true;
            }
        }
        false
    }
}

// ── 14. A view never names a CSS-Module class its stylesheet does not define ───
//
// Added by S-429, which made `StatisticsView.module.css` serve TWO views. The
// hazard is specific to this toolchain and invisible to the SPA suite: `css: false`
// in `vitest.config.ts` makes every CSS Module resolve to `{}`, so `styles.gone` is
// `undefined`, React renders no class at all, and all 800-plus specs stay green. A
// rule deleted from a shared stylesheet as "only its original view uses this" would
// therefore break the other consumer in total silence.
//
// It is checked HERE, not in Vitest, because for a `*.module.css` Vite's CSS-modules
// plugin wins over the `?raw` query — the glob hands back the empty proxy rather than
// the stylesheet text. This file reads stylesheets off disk, which is the same reason
// it already owns every other stylesheet contract in the project.
//
// Both scanners below were written by hand (no regex crate here) and both were WRONG
// in their first form — a review agent broke each one with a mutation the guard
// passed. The two defects are named at their fixes, because a hand-rolled scanner
// that admits a non-definition is worse than no guard: it reports safety it does not
// provide.

/// The stylesheet each `import <binding> from "….module.css"` binds, as
/// `(binding, relative path)`.
///
/// Matched on the SPECIFIER, not on the binding name. The first version of this
/// scanner looked for the literal `import styles from "` and so skipped, in silence,
/// any file that binds its stylesheet to another name — and skipped every import
/// after the first, because it used `split_once`.
fn module_css_imports(source: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("import ") else { continue };
        let Some((binding, after)) = rest.split_once(" from ") else { continue };
        let binding = binding.trim();
        if binding.is_empty() || !binding.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue; // a named/namespace import, never a CSS-Module default
        }
        let spec = after.trim().trim_end_matches(';').trim_matches('"').trim_matches('\'');
        if spec.ends_with(".module.css") {
            found.push((binding.to_string(), spec.to_string()));
        }
    }
    found
}

/// Every class key `binding` is used with in `source` — both `binding.key` and
/// `binding["key"]`.
///
/// The bracket form was missed entirely by the first version. It type-checks against
/// the CSS-Module declaration and is what any codemod or dynamic-key refactor emits,
/// so a view could name a nonexistent class through it and this guard would pass —
/// proven by mutation before this arm existed.
fn module_style_keys(source: &str, binding: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let bytes = source.as_bytes();
    let dotted = format!("{binding}.");
    let bracketed = format!("{binding}[");
    let mut i = 0;
    while i < bytes.len() {
        // These sources are UTF-8 with plenty of em dashes in their comments, so a
        // byte cursor must not be used to slice: `source[i..]` panics mid-character.
        // (The scan stays byte-driven because every token it looks for is ASCII.)
        if !source.is_char_boundary(i) {
            i += 1;
            continue;
        }
        // The byte before must not be able to continue an identifier, so a longer
        // name ending in the binding (`myStyles.foo`) is not a match.
        let boundary_ok = i == 0 || {
            let p = bytes[i - 1];
            !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$' || p == b'.')
        };
        if !boundary_ok {
            i += 1;
            continue;
        }
        if source[i..].starts_with(&dotted) {
            let mut j = i + dotted.len();
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > i + dotted.len() {
                keys.push(source[i + dotted.len()..j].to_string());
            }
            i = j.max(i + 1);
            continue;
        }
        if source[i..].starts_with(&bracketed) {
            let after = &source[i + bracketed.len()..];
            // A literal key only. A computed key cannot be checked statically, and
            // saying so is better than pretending the walk covered it.
            let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'' || *c == '`');
            if let Some(q) = quote {
                if let Some(end) = after[1..].find(q) {
                    let key = &after[1..1 + end];
                    if !key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        keys.push(key.to_string());
                    }
                }
            }
            i += bracketed.len();
            continue;
        }
        i += 1;
    }
    keys
}

/// The SELECTOR PRELUDES of `css` — the text that actually creates rules: everything
/// between a `}` (or the start of the file) and the next `{`, with at-rule preludes,
/// functional-pseudo argument lists (`:not(…)`, `:is(…)`, `:where(…)`, `:has(…)`)
/// and quoted strings removed.
///
/// Scanning the whole stylesheet was the second defect. `(`, `)` and `"` are all
/// identifier boundaries, so `.capNoteZ:not(.capNote) { … }` and
/// `content: ".capNote"` both counted `.capNote` as DEFINED — while no rule defines
/// it and every view using it renders no class. That is the exact failure this guard
/// exists to catch, so it read green on its own subject.
fn selector_preludes(css: &str) -> String {
    let mut out = String::new();
    let mut segment = String::new();
    let mut quote: Option<char> = None;
    let mut paren_depth = 0usize;
    for c in css.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue; // drop the whole string body
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            _ if paren_depth > 0 => {}
            '{' => {
                let trimmed = segment.trim();
                if !trimmed.starts_with('@') {
                    out.push_str(trimmed);
                    out.push('\n');
                }
                segment.clear();
            }
            '}' => segment.clear(),
            _ => segment.push(c),
        }
    }
    out
}

/// Does `preludes` define `.key` as a class selector?
fn defines_class(preludes: &str, key: &str) -> bool {
    let pat = format!(".{key}");
    let mut rest = preludes;
    while let Some(at) = rest.find(&pat) {
        let after = &rest[at + pat.len()..];
        let next_ok = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        let before_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        if next_ok && before_ok {
            return true;
        }
        rest = &rest[at + 1..];
    }
    false
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| (n.ends_with(".ts") || n.ends_with(".tsx")) && !n.contains(".test."))
        {
            out.push(path);
        }
    }
}

#[test]
fn every_module_style_key_a_view_uses_is_defined_in_the_stylesheet_it_imports() {
    // The ONE pre-existing violation this guard found when it was written, named
    // rather than excluded by narrowing the walk. `Tabs.tsx` sets `styles.tabs` and
    // `Tabs.module.css` defines `.tablist` / `.tab` / `.active` / `.panel` but no
    // `.tabs`, so that wrapper has silently carried no class. It is a real defect and
    // it is OUTSIDE S-429's scope — that component is in no diff here, and guessing
    // at the intended rule would be a change nobody asked for.
    const ALLOWED_PREEXISTING: [(&str, &str); 1] = [("components/Tabs.tsx", "tabs")];

    let src = ui_dir().join("src");
    let mut sources = Vec::new();
    collect_sources(&src, &mut sources);
    // The walk's own denominator: a partial walk would make every assertion below
    // vacuously true.
    assert!(
        sources.len() > 60,
        "the source walk found only {} modules — it is not walking the tree",
        sources.len(),
    );

    let mut pairs = 0usize;
    let mut checked = 0usize;
    let mut missing: Vec<String> = Vec::new();
    let mut hit_exemptions: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    for path in &sources {
        let source = std::fs::read_to_string(path).unwrap();
        let rel_src = path.strip_prefix(&src).unwrap().display().to_string();
        for (binding, spec) in module_css_imports(&source) {
            let stylesheet = path.parent().unwrap().join(&spec);
            let Ok(css_raw) = std::fs::read_to_string(&stylesheet) else {
                panic!("{rel_src} imports {spec}, which does not exist");
            };
            let preludes = selector_preludes(&strip_comments(&css_raw));
            pairs += 1;
            for key in module_style_keys(&source, &binding) {
                checked += 1;
                if defines_class(&preludes, &key) {
                    continue;
                }
                let pair = (rel_src.clone(), key.clone());
                if ALLOWED_PREEXISTING.contains(&(pair.0.as_str(), pair.1.as_str())) {
                    hit_exemptions.insert(pair);
                    continue;
                }
                missing.push(format!("{rel_src} uses {binding}.{key}, undefined in {spec}"));
            }
        }
    }

    assert!(pairs >= 10, "found only {pairs} view/stylesheet pairs — the import scan is broken");
    assert!(checked >= 50, "checked only {checked} class keys — the key scan is broken");
    assert!(
        missing.is_empty(),
        "a view names {} CSS-Module class(es) its stylesheet does not define. \
         Under `css: false` this renders as NO class and no Vitest spec can see it:\n  {}",
        missing.len(),
        missing.join("\n  "),
    );

    // A stale exemption is a lie about the codebase, so it fails too. Compared as a
    // SET of pairs, not as a count of occurrences: the first version counted
    // occurrences against the list's length, so a second use of an allow-listed key
    // made the counts disagree and the diagnostic underflowed a `usize` — the test
    // still failed, but with "attempt to subtract with overflow" instead of its
    // message. Found by mutation.
    let declared: std::collections::BTreeSet<(String, String)> = ALLOWED_PREEXISTING
        .iter()
        .map(|(f, k)| ((*f).to_string(), (*k).to_string()))
        .collect();
    let stale: Vec<_> = declared.difference(&hit_exemptions).collect();
    assert!(
        stale.is_empty(),
        "{} allow-listed pre-existing violation(s) no longer occur — delete the stale \
         entr{} from ALLOWED_PREEXISTING: {stale:?}",
        stale.len(),
        if stale.len() == 1 { "y" } else { "ies" },
    );
}
