//! The wiki page reader's Markdown→HTML renderer (S-076, CR-012, FR-UI-06),
//! backed by the [`comrak`] CommonMark/GFM crate (S-136, CR-035).
//!
//! The wiki body is agent-authored Markdown stored byte-verbatim
//! ([FR-WK-02](../../docs/specs/requirements/FR-WK-02.md)); the reader renders it
//! ([frontend-design.md](../../docs/specs/frontend-design.md) §4.11). The original
//! adapter hand-rolled a small Markdown subset to stay dependency-light
//! ([ADR-27](../../docs/specs/architecture/decisions/ADR-27.md)); S-136 reverses
//! that decision so consolidated wiki pages render **GFM pipe tables** as real
//! `<table>`s instead of raw pipe text. comrak is a mature CommonMark+GFM engine
//! and is **`ui`-gated and non-networking**: it is a dependency of the `web` crate,
//! which only links into the binary under cli's non-default `ui` feature, so it
//! never enters the default-feature tree the no-network fitness function guards
//! (`logos-core/tests/no_network_deps.rs`, NFR-SE-01) — comrak pulls only
//! in-process parsing crates, no socket/HTTP surface.
//!
//! # Safety invariant (preserved)
//! The renderer **never passes raw HTML or a dangerous URL through**. comrak runs
//! with `render.r#unsafe = false` (the default), which is its XSS boundary:
//!
//! * A literal `<script>` (or any raw HTML run) is replaced by an inert
//!   `<!-- raw HTML omitted -->` comment — it can never become a live element.
//! * A `javascript:`/`data:`/`vbscript:` link or image URL has its target dropped
//!   (the anchor renders with an empty `href`), so the scheme can never fire.
//!
//! This is the same "never trust a string we did not author" discipline as
//! [`crate::components::escape`]; the neutralization is now comrak's rather than a
//! hand-rolled escape, but the invariant — no injection vector under the self-only
//! CSP — is identical. The two project-specific contracts comrak does not provide
//! out of the box are layered on after rendering:
//!
//! * **` ```mermaid ` fence interception** (FR-WK-15): a fenced `mermaid` block is
//!   rewritten from comrak's `<pre><code>` into a `<div class="mermaid">` the
//!   vendored renderer turns into a diagram. Its source stays comrak-escaped, so a
//!   `<script>` inside a diagram is inert text and, with scripting off, the escaped
//!   source shows verbatim rather than a blank.
//! * **`toc-<slug>` heading ids** (FR-WK-11): every heading is shifted one level
//!   down (so a body `#` becomes `<h2>`, never colliding with the shell's `h1`
//!   app-title or the card's `h1` wiki-title — which since S-139 leads the
//!   `.card.wiki-body` box) and given a page-unique `toc-<slug>` anchor id
//!   using the same slug/uniquify rules the wiki TOC rail uses, so `collect_toc`
//!   reuses the id verbatim (S-109 → S-111) rather than re-anchoring.

use std::borrow::Cow;
use std::collections::HashMap;

/// HTML-escape one text run for element content or a double-quoted attribute.
/// Identical policy to [`crate::components::escape`]; duplicated so the renderer
/// is self-contained and its safety does not depend on a sibling module. Used by
/// [`rewrite_mermaid_blocks`] and for the generated anchor ids.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render `body` (Markdown) to a safe HTML fragment. The output is a sequence of
/// block elements wrapped by the caller; it carries no `<html>`/`<body>` chrome.
///
/// comrak does the parsing and the safety neutralization (raw HTML / dangerous
/// URLs, see the module docs); two project contracts are layered on afterwards —
/// the ` ```mermaid ` fence rewrite and the `toc-<slug>` heading anchors.
pub fn render(body: &str) -> String {
    // GFM tables are the point of S-136. `render.r#unsafe` stays at its default
    // `false` — that is comrak's no-raw-HTML / no-dangerous-URL safety boundary,
    // and the safety invariant depends on it staying off. Struct-update syntax
    // (not `let mut … ; opts.field = …`) keeps clippy's `field_reassign_with_default`
    // quiet under the workspace's `-D warnings` gate (NFR-MA-04).
    let options = comrak::Options {
        extension: comrak::options::Extension { table: true, ..Default::default() },
        ..Default::default()
    };
    let html = comrak::markdown_to_html(body, &options);
    let html = rewrite_mermaid_blocks(&html);
    shift_and_anchor_headings(&html)
}

/// If `body` opens (after any leading blank lines) with an ATX Markdown heading
/// whose text equals `title`, return `body` with that one heading line removed;
/// otherwise return `body` unchanged.
///
/// The wiki reader renders the authoritative page-metadata title as the content
/// card's single `h1` (S-139, [FR-UI-06]); a body that begins by repeating its own
/// title as a heading would then show the title twice. Suppression happens here, on
/// the Markdown **source before [`render`]**, rather than on the rendered HTML — so
/// the removed heading never reaches comrak and therefore never claims a shifted
/// `toc-<slug>` anchor id or an "On this page" TOC-rail entry. Only the *leading*
/// heading is considered; a same-named heading deeper in the body is a real section
/// and is kept. The match is a trimmed, plain-text comparison: a leading heading
/// carrying inline markup (e.g. `# Use \`x\``) is not treated as a duplicate of a
/// plain title, which is the safe direction (it renders rather than vanishing).
pub fn suppress_leading_title_heading<'a>(body: &'a str, title: &str) -> Cow<'a, str> {
    let title = title.trim();
    let mut offset = 0;
    // `split_inclusive` keeps each line's trailing `\n`, so byte offsets line up
    // with `body` and the remainder after the heading is carried verbatim.
    for line in body.split_inclusive('\n') {
        if line.trim().is_empty() {
            offset += line.len();
            continue;
        }
        // The first non-blank line. If it is a title-matching ATX heading, drop it
        // (keeping any leading blank lines, which comrak ignores, and the verbatim
        // remainder); otherwise the body opens with something else — leave it whole.
        if atx_heading_text(line).is_some_and(|text| text == title) {
            let after = offset + line.len();
            return Cow::Owned([&body[..offset], &body[after..]].concat());
        }
        break;
    }
    Cow::Borrowed(body)
}

/// The plain text of an ATX Markdown heading line (`#`..`######`, a required space,
/// the text, and an optional trailing `#` closing sequence), or `None` if `line` is
/// not an ATX heading. Mirrors the CommonMark rules comrak applies: up to three
/// leading spaces of indentation (four-plus is a code block, not a heading), one to
/// six `#`, the opening run followed by a space/tab or end-of-line, and a trailing
/// run of `#` stripped only when it is preceded by whitespace (so `# foo #` is
/// "foo" but `# foo#` keeps the literal `#`).
fn atx_heading_text(line: &str) -> Option<&str> {
    let line = line.trim_end_matches(['\n', '\r']);
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let after_indent = &line[indent..];
    let hashes = after_indent.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &after_indent[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    // The returned text is always a subslice of `line` (`rest.trim()` and the
    // closing-`#` strip both yield subslices), so no owned allocation is needed —
    // the caller only compares it to the title for equality.
    let text = rest.trim();
    let stripped = text.trim_end_matches('#');
    let text = if stripped.len() == text.len() || stripped.is_empty() || stripped.ends_with([' ', '\t'])
    {
        stripped.trim_end()
    } else {
        text
    };
    Some(text)
}

/// comrak's exact opening markup for a ` ```mermaid ` fence under the default
/// render options (`github_pre_lang = false`, no syntax-highlight plugin): the
/// info string's first word becomes the code element's `language-…` class.
const MERMAID_OPEN: &str = "<pre><code class=\"language-mermaid\">";
/// comrak's matching code-block close. The fence body is HTML-escaped by comrak,
/// so a literal `</code></pre>` can never occur inside it — the first occurrence
/// after the open is always the true close.
const CODE_CLOSE: &str = "</code></pre>";

/// Rewrite each ` ```mermaid ` code block comrak rendered as `<pre><code
/// class="language-mermaid">…</code></pre>` into the `<div class="mermaid">…</div>`
/// block the vendored renderer turns into a visual diagram (FR-WK-15). The body is
/// comrak-escaped and carried through unchanged, so the safety invariant holds (a
/// `<script>` in a diagram is inert text) and the escaped source stays visible when
/// scripting is off. Non-`mermaid` fences keep their literal `<pre><code>` markup.
fn rewrite_mermaid_blocks(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut i = 0;
    while let Some(rel) = html[i..].find(MERMAID_OPEN) {
        let open = i + rel;
        out.push_str(&html[i..open]);
        let body_start = open + MERMAID_OPEN.len();
        match html[body_start..].find(CODE_CLOSE) {
            Some(crel) => {
                let body_end = body_start + crel;
                out.push_str("<div class=\"mermaid\">");
                out.push_str(&html[body_start..body_end]);
                out.push_str("</div>");
                i = body_end + CODE_CLOSE.len();
            }
            // No close (cannot happen for well-formed comrak output) — emit the
            // open literally and resume just past it rather than loop forever.
            None => {
                out.push_str(MERMAID_OPEN);
                i = body_start;
            }
        }
    }
    out.push_str(&html[i..]);
    out
}

/// Shift every heading one level down (clamped at `h6`) and give it a page-unique
/// `toc-<slug>` anchor id.
///
/// The shift maps a body `#`→`<h2>` … so a heading never collides with the shell's
/// `h1` (app title) or the card `h3`, and lands every `h1`/`h2` source heading in
/// the `<h2>`/`<h3>` range the wiki TOC rail and `collect_toc` scan. The id uses the
/// same `slugify`/`uniquify` rules as the wiki TOC, so `collect_toc` reuses the id
/// verbatim instead of injecting a second one (FR-WK-11). comrak emits bare
/// `<hN>…</hN>` (no attributes — `header_ids` is off), so a simple tag scan is
/// exact; heading inner markup is already comrak-escaped and is carried through.
fn shift_and_anchor_headings(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    // Page-unique id ledger, mirroring the wiki TOC's `uniquify` (dedup probes
    // `-2`, `-3`, … against the set so a natural `foo-2` slug cannot collide with
    // the disambiguated form of a repeated `foo`).
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < html.len() {
        let Some(rel) = html[i..].find('<') else {
            out.push_str(&html[i..]);
            break;
        };
        let lt = i + rel;
        out.push_str(&html[i..lt]);
        // A genuine heading open is `<hN>` with N in 1..=6 (comrak adds no attrs).
        if let Some(level) = heading_open_level(&html[lt..]) {
            let body_start = lt + 4; // past "<hN>"
            let close = format!("</h{level}>");
            if let Some(crel) = html[body_start..].find(&close) {
                let inner = &html[body_start..body_start + crel];
                let shifted = (level + 1).min(6);
                let id = uniquify(&mut seen, format!("toc-{}", slugify(&strip_tags(inner))));
                out.push_str(&format!("<h{shifted} id=\"{}\">{}</h{shifted}>", esc(&id), inner));
                i = body_start + crel + close.len();
                continue;
            }
        }
        // Not a heading (or no close found) — emit the '<' and resume after it.
        out.push('<');
        i = lt + 1;
    }
    out
}

/// If `s` begins with a bare heading-open tag `<hN>` (N in 1..=6), return `N`.
/// Matches comrak's attribute-free heading markup exactly — the byte after `<hN`
/// must be `>`, so a tag like `<h2 …>` (which comrak never emits for headings) or a
/// false prefix is not mistaken for one.
fn heading_open_level(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    if b.len() >= 4 && b[0] == b'<' && b[1] == b'h' && b[2].is_ascii_digit() && b[3] == b'>' {
        let level = b[2] - b'0';
        return (1..=6).contains(&level).then_some(level);
    }
    None
}

/// Strip HTML tags from a heading's inner markup, leaving the text. The runs were
/// already escaped at render, so entities are preserved; the slug source is the
/// visible text, not the markup. Mirrors the wiki TOC's `strip_tags`.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// A URL/id-safe slug from `s` — lowercase alphanumerics, every other run
/// collapsed to a single `-`, no leading/trailing `-`. Never empty (`"section"`
/// fallback). Mirrors the wiki TOC's slug rule so the ids line up.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "section".to_string()
    } else {
        out
    }
}

/// Allocate a render-unique anchor id from `base`, recording every id handed out
/// in `seen`. The first use of a slug wins it verbatim; a repeat probes `-2`,
/// `-3`, … against `seen` (not a per-base counter) so a natural slug like `foo-2`
/// can never collide with the disambiguated form of a repeated `foo`. Mirrors the
/// wiki TOC's `uniquify` so headings stay collision-free across the page.
fn uniquify(seen: &mut HashMap<String, usize>, base: String) -> String {
    if !seen.contains_key(&base) {
        seen.insert(base.clone(), 1);
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !seen.contains_key(&candidate) {
            seen.insert(candidate.clone(), 1);
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gfm_pipe_table_renders_a_real_table_not_raw_pipes() {
        // The reason S-136 exists: a GFM pipe table must become an HTML <table>,
        // not survive as literal pipe text (AC1).
        let html = render("| A | B |\n| --- | --- |\n| 1 | 2 |");
        assert!(html.contains("<table>"), "a GFM table renders a <table>: {html}");
        assert!(html.contains("<th>A</th>"), "header cells render: {html}");
        assert!(html.contains("<td>1</td>") && html.contains("<td>2</td>"), "body cells render: {html}");
        assert!(!html.contains("| A | B |"), "raw pipe text must not survive: {html}");
    }

    #[test]
    fn headings_shift_below_the_shell_chrome_and_carry_toc_ids() {
        // A body `#` maps to <h2> (never the shell h1) with a slugged `toc-<slug>`
        // id the TOC rail reuses (S-111). comrak appends a trailing newline, so we
        // assert on substrings.
        assert!(render("# Title").contains("<h2 id=\"toc-title\">Title</h2>"));
        assert!(render("## Sub").contains("<h3 id=\"toc-sub\">Sub</h3>"));
        // A mid-ladder level shifts one down (#### → h5), confirming the shift is
        // uniform, not just clamped at the ends.
        assert!(render("#### Four").contains("<h5 id=\"toc-four\">Four</h5>"));
        // Level clamps at h6: both `#####` (5) and `######` (6) land on h6.
        assert!(render("##### Five").contains("<h6 id=\"toc-five\">Five</h6>"));
        assert!(render("###### Six").contains("<h6 id=\"toc-six\">Six</h6>"));
        // 7 hashes is not a heading in CommonMark — it renders as paragraph text.
        let seven = render("####### too deep");
        assert!(!seven.contains("<h"), "a 7-hash line is not a heading: {seven}");
    }

    #[test]
    fn headings_get_unique_slugged_ids() {
        // Two same-text headings get collision-free ids (the wiki TOC reuses them
        // verbatim, so uniqueness must hold here at render time): the first wins the
        // bare slug, the repeat is disambiguated with `-2`.
        let html = render("## Setup\n\n## Setup");
        assert!(html.contains("<h3 id=\"toc-setup\">Setup</h3>"), "first wins the bare slug: {html}");
        assert!(html.contains("<h3 id=\"toc-setup-2\">Setup</h3>"), "the repeat is disambiguated: {html}");
        // Inline markup in the heading text slugs from its text, not the markers.
        assert!(
            render("## Use `Engine::start`").contains("<h3 id=\"toc-use-engine-start\">Use <code>Engine::start</code></h3>"),
        );
    }

    #[test]
    fn the_safety_invariant_no_raw_html_passthrough() {
        // A literal <script> in the body must never reach the DOM as an element —
        // comrak drops raw HTML to an inert `<!-- raw HTML omitted -->` comment.
        let html = render("hello <script>alert(1)</script> world");
        assert!(!html.contains("<script"), "raw HTML must never pass through: {html}");
        assert!(html.contains("<!-- raw HTML omitted -->"), "raw HTML degrades to an inert comment: {html}");
        assert!(html.contains("hello") && html.contains("world"), "surrounding prose survives");
    }

    #[test]
    fn unsafe_link_schemes_are_neutralized() {
        // A javascript: link degrades to inert text — the link text stays but the
        // dangerous scheme never reaches an href (comrak drops the target).
        let html = render("[click](javascript:alert(1))");
        assert!(!html.contains("javascript:"), "a javascript: scheme must never reach the output: {html}");
        assert!(html.contains(">click</a>"), "the link text is preserved as inert text: {html}");
        // The other dangerous schemes the module guarantees are neutralized too —
        // a `data:` (non-image) and a `vbscript:` target must never reach an href.
        assert!(!render("[x](data:text/html,alert(1))").contains("data:"), "a data: scheme must not survive");
        assert!(!render("[x](vbscript:alert(1))").contains("vbscript:"), "a vbscript: scheme must not survive");
        // A safe scheme still becomes a real link.
        assert!(
            render("[docs](https://example.com)").contains("<a href=\"https://example.com\">docs</a>"),
            "safe schemes still link",
        );
        // A relative ref (a sibling wiki page) still links.
        assert!(render("[rel](FR-WK-04.md)").contains("<a href=\"FR-WK-04.md\">rel</a>"));
    }

    #[test]
    fn a_same_origin_asset_image_src_survives_rendering() {
        // The S-270 transform rewrites a doc-relative `<img src>` to the same-origin
        // asset route (`/api/v1/wiki/asset/...`, [FR-WK-27]); a root-relative URL is a
        // safe scheme, so comrak keeps it — the diagram reaches the browser, which
        // fetches it from the sandboxed route (no data:/external host, CSP unchanged).
        let html = render("![diagram](/api/v1/wiki/asset/docs/specs/architecture/images/x.png)");
        assert!(
            html.contains("<img src=\"/api/v1/wiki/asset/docs/specs/architecture/images/x.png\""),
            "the same-origin asset image src is preserved through rendering: {html}",
        );
        assert!(html.contains("alt=\"diagram\""), "the alt text is preserved: {html}");
    }

    #[test]
    fn mermaid_fence_becomes_a_rendered_block_not_literal_code() {
        // A ```mermaid fence passes through as a `.mermaid` block (the renderer
        // turns it visual), NOT the literal `<pre><code>` other fences get (FR-WK-15).
        let html = render("```mermaid\ngraph LR\n  a-->b\n```");
        assert!(html.contains("<div class=\"mermaid\">"), "mermaid fence → .mermaid block: {html}");
        assert!(!html.contains("language-mermaid"), "the code-fence markup is fully rewritten: {html}");
        assert!(!html.contains("<pre>"), "a mermaid fence is not literal <pre> code: {html}");
        // The source is escaped (safety invariant) but recoverable as text — the
        // arrow is entity-escaped, so a `<script>` in a diagram could never run.
        assert!(html.contains("graph LR"), "the diagram source is carried: {html}");
        assert!(html.contains("a--&gt;b"), "the source is HTML-escaped: {html}");
    }

    #[test]
    fn empty_mermaid_fence_renders_a_well_formed_empty_block() {
        // The degenerate case (an empty ```mermaid fence) still produces a
        // well-formed `.mermaid` element — never malformed markup or a panic.
        assert!(render("```mermaid\n```").contains("<div class=\"mermaid\"></div>"));
    }

    #[test]
    fn multiple_mermaid_fences_each_become_a_diagram_block() {
        // `rewrite_mermaid_blocks` loops over every fence — two fences with prose
        // between them must each become an independent `.mermaid` block, with the
        // prose preserved and no code-fence markup surviving.
        let html = render("```mermaid\ngraph LR\n  a-->b\n```\n\nbetween\n\n```mermaid\ngraph TD\n  x-->y\n```");
        assert_eq!(html.matches("<div class=\"mermaid\">").count(), 2, "both fences become .mermaid blocks: {html}");
        assert!(!html.contains("language-mermaid"), "no code-fence markup survives: {html}");
        assert!(html.contains("<p>between</p>"), "prose between the fences is preserved: {html}");
    }

    #[test]
    fn mermaid_fence_body_with_a_close_tag_lookalike_stays_bounded() {
        // The fence body is comrak-escaped, so a literal `</code></pre>` in the
        // source becomes entities — the rewrite never mistakes it for the real
        // structural close, and the block stays correctly bounded.
        let html = render("```mermaid\ngraph LR\n  a[\"</code></pre>\"]\n```");
        assert_eq!(html.matches("<div class=\"mermaid\">").count(), 1, "exactly one diagram block: {html}");
        assert!(!html.contains("language-mermaid"), "the code-fence markup is fully rewritten: {html}");
        assert!(html.contains("&lt;/code&gt;&lt;/pre&gt;"), "the lookalike is carried as escaped text: {html}");
    }

    #[test]
    fn mermaid_fence_escapes_html_in_the_source() {
        // The .mermaid body is escaped like any run: a literal tag is inert text.
        let html = render("```mermaid\ngraph LR\n  a[\"<script>\"]\n```");
        assert!(html.contains("&lt;script&gt;"), "diagram source is escaped, not raw HTML: {html}");
        assert!(!html.contains("<script>"), "no raw HTML passes through a mermaid block: {html}");
    }

    #[test]
    fn a_non_mermaid_fence_with_a_language_stays_literal_code() {
        // An info string other than `mermaid` keeps the literal-code path.
        let html = render("```rust\nlet x = a < b && c;\n```");
        assert!(html.contains("<pre><code class=\"language-rust\">"), "rust fence stays literal: {html}");
        assert!(!html.contains("class=\"mermaid\""), "only a mermaid fence becomes a diagram: {html}");
        // Code content is escaped — no inline interpretation inside a code block.
        assert!(html.contains("let x = a &lt; b &amp;&amp; c;"), "code content is escaped: {html}");
        // The CSS code-block rule (`.wiki-prose pre`, S-151/CR-039) keys to the
        // BARE comrak `<pre>` — the language class rides the inner `<code>`, never
        // the `<pre>`. The old renderer's `pre.wiki-code` class is gone; were it to
        // reappear here the stylesheet selector would silently orphan again (the
        // 0.7.2 regression). Pin the renderer end of that contract.
        assert!(html.contains("<pre>"), "the block is a bare <pre> the stylesheet targets: {html}");
        assert!(!html.contains("wiki-code"), "the dead `pre.wiki-code` class must not return: {html}");
    }

    #[test]
    fn inline_code_strong_emphasis_and_paragraphs() {
        assert!(render("`Engine::start`").contains("<code>Engine::start</code>"));
        assert!(render("**bold**").contains("<strong>bold</strong>"));
        assert!(render("*em*").contains("<em>em</em>"));
        // A blank line separates paragraphs.
        let html = render("a\n\nb");
        assert!(html.contains("<p>a</p>") && html.contains("<p>b</p>"), "blank line splits paragraphs: {html}");
    }

    #[test]
    fn lists_blockquote_and_hr() {
        assert!(render("- a\n- b").contains("<ul>\n<li>a</li>\n<li>b</li>\n</ul>"));
        assert!(render("1. a\n2. b").contains("<ol>"));
        assert!(render("> quoted").contains("<blockquote>"));
        assert!(render("---").contains("<hr />"));
    }

    #[test]
    fn code_span_content_is_escaped_not_interpreted() {
        assert!(render("`<b>x</b>`").contains("<code>&lt;b&gt;x&lt;/b&gt;</code>"));
    }

    #[test]
    fn empty_body_renders_nothing() {
        assert_eq!(render(""), "");
        assert_eq!(render("\n\n"), "");
    }

    #[test]
    fn suppresses_a_leading_heading_equal_to_the_title() {
        // The reason S-139 exists: a body opening with a heading that repeats the
        // page title would render the title twice (once as the card h1). The leading
        // heading line is removed; the rest of the body is carried verbatim.
        let body = "# Alpha objectives\n\nThe quux subsystem.\n";
        let out = suppress_leading_title_heading(body, "Alpha objectives");
        assert_eq!(out, "\nThe quux subsystem.\n", "the duplicate leading heading is dropped");
        // Rendering the result yields the prose with no leftover heading.
        let html = render(&out);
        assert!(!html.contains("Alpha objectives"), "the duplicate title text is gone: {html}");
        assert!(html.contains("The quux subsystem."), "the prose survives: {html}");
    }

    #[test]
    fn suppression_matches_any_atx_level_and_trims() {
        // Any ATX level (1..=6) counts as the leading heading, and surrounding
        // whitespace on the heading text and the title is ignored.
        assert_eq!(suppress_leading_title_heading("### Title\n\nx\n", "Title"), "\nx\n");
        assert_eq!(suppress_leading_title_heading("#   Spaced   \n\nx\n", "Spaced"), "\nx\n");
        // A leading blank line before the heading is tolerated (and kept — comrak
        // ignores it), the matching heading is still removed.
        assert_eq!(suppress_leading_title_heading("\n# Title\nbody\n", "Title"), "\nbody\n");
        // A trailing `#` closing sequence is stripped before the comparison.
        assert_eq!(suppress_leading_title_heading("## Title ##\n\nx\n", "Title"), "\nx\n");
        // A body that is *only* the title heading suppresses to the empty string,
        // which `render` turns into no HTML at all (the card then shows just the h1).
        assert_eq!(suppress_leading_title_heading("# Title\n", "Title"), "");
        assert_eq!(render(""), "");
    }

    #[test]
    fn suppression_keeps_a_non_matching_or_non_leading_heading() {
        // A leading heading whose text differs from the title is a real section and
        // is preserved (the fixture's "## Alpha" under title "Alpha objectives").
        let body = "## Alpha\n\nThe quux subsystem.\n";
        assert_eq!(suppress_leading_title_heading(body, "Alpha objectives"), body);
        // A title-matching heading that is NOT the leading element stays — only the
        // first content line is considered.
        let body = "Intro paragraph.\n\n# Title\n";
        assert_eq!(suppress_leading_title_heading(body, "Title"), body);
        // A body that opens with prose (no heading) is returned untouched, and a
        // page with no leading duplicate still renders its body in full.
        let body = "Just prose, no heading.\n";
        assert_eq!(suppress_leading_title_heading(body, "Title"), body);
    }

    #[test]
    fn suppression_does_not_mistake_non_headings() {
        // `#foo` (no space after the hashes) is not an ATX heading — not suppressed.
        assert_eq!(suppress_leading_title_heading("#Title\n\nx\n", "Title"), "#Title\n\nx\n");
        // Seven `#` is not a heading in CommonMark.
        assert_eq!(
            suppress_leading_title_heading("####### Title\n", "Title"),
            "####### Title\n",
        );
        // Four-space indentation is a code block, not a heading — left intact.
        assert_eq!(
            suppress_leading_title_heading("    # Title\n", "Title"),
            "    # Title\n",
        );
        // `# foo#` keeps the literal trailing `#` (no space before it), so it does
        // not match the bare title "foo".
        assert_eq!(suppress_leading_title_heading("# foo#\n", "foo"), "# foo#\n");
    }
}
