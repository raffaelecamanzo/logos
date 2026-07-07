//! SPA-bundle fitness functions (CR-049, FR-UI-22, ADR-43, NFR-PC-04).
//!
//! The Vite + React SPA is built by a build-time-only toolchain and embedded
//! into the binary ([`web::spa`]). These tests assert — over **whatever is
//! embedded** (the committed placeholder in a Node-free build, or the real hashed
//! bundle in a CI/local build that ran `npm run build`) — the carve-out
//! invariants the served CSP otherwise enforces at runtime:
//!
//!   1. **No inline `<script>`/`<style>`.** Every `<script>` in any embedded HTML
//!      carries a `src` (external module), and no `<style>` tag appears — so the
//!      self-only CSP (`default-src 'self'`, no `unsafe-inline`) stays
//!      byte-identical (FR-UI-22, ADR-43). This is the SPA analog of ADR-27's
//!      no-inline asset scan.
//!   2. **Names no external origin.** No embedded text file names a remote
//!      *fetch* host. Like the vendored Mermaid bundle (web/src/assets.rs), a
//!      React build legitimately names W3C XML-namespace URIs
//!      (`http://www.w3.org/2000/svg`, for `createElementNS`) and a doc-citation
//!      URL in its minified error message — neither is ever fetched. So the
//!      honest check is "no remote fetch host", allowlisting namespace +
//!      citation hosts and forbidding any CDN/loader origin — not a blanket
//!      "contains no http" (impossible for code that emits SVG).
//!   3. **`index.html` references only same-origin `/assets/*`.** No `http(s)://`
//!      and no protocol-relative `//host` asset reference.
//!
//! It also records the embedded bundle size beside the ui-build budget
//! (NFR-PC-04) — logged for visibility and capped generously so a runaway is
//! caught without coupling to the exact React/viz bundle weight (later stories
//! that add ECharts/uPlot/Mermaid re-bless the cap deliberately).

use web::spa::Spa;

/// A generous upper bound on the total embedded SPA bundle size (NFR-PC-04). The
/// foundation bundle (React + ReactDOM) is well under 1 MB; this cap exists only
/// to catch an accidental multi-megabyte regression, and is re-blessed when the
/// per-tab migrations add the heavier viz libraries (ECharts/uPlot/Mermaid).
///
/// Re-baselined for assistant-ui (S-200, CR-051, ADR-45, NFR-PC-04): the chat
/// surface rebuilt on `@assistant-ui/react` (+ radix-ui primitives, react-markdown,
/// remark-gfm) adds ~0.4 MB to the hashed JS chunk. The full embedded bundle —
/// app JS + CSS + the vendored Mermaid runtime (S-196) — measures ~4.4 MB, so the
/// 8 MB cap still holds with comfortable headroom; it is deliberately re-blessed
/// here rather than tightened, keeping its role as a runaway guard (the exact
/// measured size is logged by `embedded_bundle_size_is_recorded_and_within_budget`).
const BUNDLE_SIZE_BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// File extensions whose bytes are text we can scan for origins/inline markup.
/// Binary assets (fonts, images) are excluded — scanning them as text would be
/// meaningless and could false-positive on incidental byte sequences.
fn is_text(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("");
    matches!(ext, "html" | "js" | "mjs" | "css" | "json" | "map" | "svg" | "txt")
}

/// Every remote host named by `text` across `http(s)://` URLs — the host is the
/// run between `://` and the next delimiter. Mirrors the `remote_hosts` audit in
/// web/src/assets.rs used for the vendored Mermaid bundle: it audits for **fetch**
/// origins without a blanket "contains no http" check (impossible for code that
/// emits SVG/XML, whose elements must name their W3C namespace URIs).
fn remote_hosts(text: &str) -> std::collections::BTreeSet<String> {
    let mut hosts = std::collections::BTreeSet::new();
    for scheme in ["http://", "https://"] {
        let mut from = 0;
        while let Some(rel) = text[from..].find(scheme) {
            let start = from + rel + scheme.len();
            let host: String = text[start..]
                .chars()
                .take_while(|c| {
                    !matches!(
                        c,
                        '/' | '"' | '\'' | ' ' | ')' | '(' | '\\' | '<' | '>' | '\n' | '\r'
                            | '\t' | '`' | ',' | ';'
                    )
                })
                .collect();
            hosts.insert(host);
            from = start;
        }
    }
    hosts
}

/// Strip HTML comments (`<!-- ... -->`) from `html`. Markup inside a comment is
/// never parsed or executed by the browser, so the inline-`<script>`/`<style>`
/// scan must ignore it — otherwise a comment that merely *describes* the
/// invariant (e.g. "no inline script tags") would false-positive.
fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + "-->".len()..],
            // An unterminated comment swallows the remainder (as a browser would).
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Read every embedded file as `(path, bytes)`.
fn embedded_files() -> Vec<(String, Vec<u8>)> {
    Spa::iter()
        .map(|path| {
            let file = Spa::get(&path).expect("an iterated embedded file resolves");
            (path.to_string(), file.data.into_owned())
        })
        .collect()
}

/// The embedded bundle names no external **fetch** origin (FR-UI-22, ADR-43).
/// Allowlist: W3C XML-namespace URIs (a React build names them for `createElementNS`),
/// the React doc-citation host (a string in the minified error message), and the
/// vendored Mermaid bundle's license-citation hosts — none is ever a runtime fetch
/// target. Any CDN/loader origin — or any other unlisted host — fails.
#[test]
fn embedded_bundle_names_no_external_fetch_origin() {
    // Allowed hosts fall into three categories — all citation-only, never fetched:
    //   1. W3C XML-namespace/SVG URIs (React build: `createElementNS` calls)
    //   2. React error-decoder doc citation (in minified error messages)
    //   3. Mermaid bundle license-comment citations (BSD/MIT attribution strings
    //      embedded in the vendored mermaid.min.js; the self-only CSP prevents
    //      any runtime fetch regardless of what strings appear in the bundle)
    // A real runtime fetch to an unlisted host would surface it here; the
    // CSP provides the runtime enforcement guarantee on top of this static check.
    const ALLOWED_HOSTS: &[&str] = &[
        // W3C namespace URIs (React build: createElementNS for SVG/XHTML/MathML)
        "www.w3.org",
        // React error-decoder doc citations (never fetched; in minified error messages)
        "reactjs.org",
        "react.dev",
        // Mermaid bundle license-comment citations (BSD/MIT attribution in mermaid.min.js;
        // the self-only CSP prevents any runtime fetch to these regardless — S-196)
        "github.com",
        "opensource.org",
        "jquery.org",
        "tldrlegal.com",
        "engelschall.com",
        "www.eclipse.org",
        "en.wikipedia.org",
    ];
    let mut scanned_any = false;
    for (path, bytes) in embedded_files() {
        if !is_text(&path) {
            continue;
        }
        scanned_any = true;
        let text = String::from_utf8_lossy(&bytes);
        for host in remote_hosts(&text) {
            assert!(
                host.is_empty() || ALLOWED_HOSTS.contains(&host.as_str()),
                "the embedded SPA bundle file `{path}` names an unexpected remote host \
                 `{host}` — only W3C XML-namespace, React doc-citation, and Mermaid \
                 license-citation URLs are permitted (no egress, FR-UI-22)",
            );
        }
        // Belt-and-suspenders: no popular CDN/loader origin slipped in.
        for cdn in ["unpkg.com", "jsdelivr", "cdnjs", "googleapis", "gstatic", "cdn."] {
            assert!(
                !text.contains(cdn),
                "the embedded SPA bundle file `{path}` must name no CDN/loader origin \
                 (found `{cdn}`) — the bundle is self-contained (FR-UI-22)",
            );
        }
    }
    assert!(scanned_any, "at least the placeholder index.html is embedded and scanned");
}

/// No embedded HTML carries an inline `<script>` (one without a `src`) or any
/// `<style>` tag — so the served self-only CSP (no `unsafe-inline`) stays
/// byte-identical (FR-UI-22, ADR-43, ADR-44).
#[test]
fn embedded_html_has_no_inline_script_or_style() {
    let mut html_count = 0;
    for (path, bytes) in embedded_files() {
        if !path.ends_with(".html") {
            continue;
        }
        html_count += 1;
        // Scan the live markup only — commented-out markup is never executed, so
        // a comment describing the invariant must not false-positive.
        let html = strip_html_comments(&String::from_utf8_lossy(&bytes));

        // No `<style>` tag at all — Vite extracts CSS to external `<link>` files.
        assert!(
            !html.contains("<style"),
            "{path} carries an inline <style> — the self-only CSP forbids it (FR-UI-22)",
        );

        // Every `<script ...>` opening tag must carry a `src` attribute; a tag
        // without one is an inline script the CSP forbids. We inspect each
        // `<script` occurrence's attributes up to the closing `>`.
        let mut from = 0;
        while let Some(rel) = html[from..].find("<script") {
            let tag_start = from + rel;
            let after = &html[tag_start + "<script".len()..];
            // The attributes run to the first `>`.
            let tag_attrs = after.split('>').next().unwrap_or("");
            assert!(
                tag_attrs.contains("src="),
                "{path} carries an inline <script> (no src) — the self-only CSP forbids \
                 it; the SPA must load external module scripts only (FR-UI-22)",
            );
            from = tag_start + "<script".len();
        }
    }
    assert!(html_count >= 1, "at least index.html is embedded");
}

/// `index.html` references its assets only as same-origin `/assets/*` — no
/// `http(s)://` origin and no protocol-relative `//host` reference (FR-UI-22).
#[test]
fn index_html_references_only_same_origin_assets() {
    let bytes = web::spa::index_html().expect("index.html embedded");
    let html = String::from_utf8_lossy(&bytes).to_string();
    assert!(
        !html.contains("http://") && !html.contains("https://"),
        "index.html names an external origin — assets must be same-origin /assets/* (FR-UI-22)",
    );
    // A protocol-relative `src="//host/..."` is also cross-origin. Vite emits
    // `src="/assets/..."` (single leading slash); guard against the `//` form.
    assert!(
        !html.contains("src=\"//") && !html.contains("href=\"//"),
        "index.html uses a protocol-relative (//host) asset reference — must be same-origin",
    );
}

/// The embedded bundle stays within the ui-build size budget (NFR-PC-04); the
/// measured size is logged so a release can record it beside the default budget.
#[test]
fn embedded_bundle_size_is_recorded_and_within_budget() {
    let total: usize = embedded_files().iter().map(|(_, b)| b.len()).sum();
    // Surfaced with `cargo test -- --nocapture`; recorded beside the ui-build
    // budget (NFR-PC-04). The placeholder build measures ~1 KB; a real React
    // build measures a few hundred KB.
    println!("embedded SPA bundle size: {total} bytes ({} KiB)", total / 1024);
    assert!(
        total <= BUNDLE_SIZE_BUDGET_BYTES,
        "the embedded SPA bundle is {total} bytes, over the {BUNDLE_SIZE_BUDGET_BYTES}-byte \
         ui-build budget (NFR-PC-04) — re-bless the cap deliberately if the growth is intended",
    );
}
