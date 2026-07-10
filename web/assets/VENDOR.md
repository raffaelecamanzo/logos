# Vendored web-UI assets — provenance & license manifest

> Governs the assets embedded into the `logos` binary under the `ui` feature
> (CR-012, [FR-UI-02](../../docs/specs/requirements/FR-UI-02.md),
> [NFR-CR-01](../../docs/specs/requirements/NFR-CR-01.md),
> [ADR-27](../../docs/specs/architecture/decisions/ADR-27.md)).

## Carve-out invariant

Nothing here is fetched at **build** or **run** time. Each file is committed to
the repository and embedded with `include_bytes!` in [`web/src/assets.rs`](../src/assets.rs);
`cargo build` is the entire build (no Node toolchain, no `package.json`, no
bundler). The self-only CSP (`default-src 'self'`) makes the no-egress posture
browser-enforced. The files were vendored once, at development time, by
[`scripts/vendor-web-assets.sh`](../../scripts/vendor-web-assets.sh) — re-running
that script is the **only** sanctioned way to update them: bump a pinned version,
re-run, review the diff, and update the checksum table below.

`cargo-deny` audits Rust crates but cannot see these files, so this manifest
**is** their license audit (NFR-CR-01): every entry is a permissive license on
the project allowlist (MIT / BSD / 0BSD / Apache-2.0 / OFL-1.1).

## Manifest

| File | Role | Upstream | Version | License | License file |
|------|------|----------|---------|---------|--------------|
| `vendor/htmx.min.js` | Fragment swaps (search, sort, filters) | [htmx.org](https://htmx.org) | 2.0.4 | 0BSD | `vendor/licenses/htmx-LICENSE.txt` |
| `vendor/uplot.min.js` | Charts (health trend, churn, coverage bars) | [leeoniya/uPlot](https://github.com/leeoniya/uPlot) | 1.6.31 | MIT | `vendor/licenses/uplot-LICENSE.txt` |
| `vendor/uplot.min.css` | uPlot stylesheet | [leeoniya/uPlot](https://github.com/leeoniya/uPlot) | 1.6.31 | MIT | `vendor/licenses/uplot-LICENSE.txt` |
| `vendor/echarts-graph.min.js` | Graph & Decisions interactive canvas (ECharts graph series, ADR-29) | [apache/echarts](https://github.com/apache/echarts) | 5.6.0 | Apache-2.0 | `vendor/licenses/echarts-LICENSE.txt` |
| `vendor/mermaid.min.js` | Wiki diagram renderer — native dependency diagram + agent-prose ` ```mermaid ` fences (FR-WK-15, S-111/CR-028) | [mermaid-js/mermaid](https://github.com/mermaid-js/mermaid) | 10.9.3 | MIT | `vendor/licenses/mermaid-LICENSE.txt` |
| `fonts/inter-400.woff2` | Sans face 400 (brand fallback, acknowledged) | [@fontsource/inter](https://fontsource.org/fonts/inter) (rsms/inter) | 5.1.0 | OFL-1.1 | `vendor/licenses/inter-OFL.txt` |
| `fonts/inter-600.woff2` | Sans face 600 | @fontsource/inter | 5.1.0 | OFL-1.1 | `vendor/licenses/inter-OFL.txt` |
| `fonts/inter-700.woff2` | Sans face 700 | @fontsource/inter | 5.1.0 | OFL-1.1 | `vendor/licenses/inter-OFL.txt` |
| `fonts/jetbrains-mono-400.woff2` | Code/identifier face 400 | [@fontsource/jetbrains-mono](https://fontsource.org/fonts/jetbrains-mono) | 5.1.0 | OFL-1.1 | `vendor/licenses/jetbrains-mono-OFL.txt` |
| `fonts/jetbrains-mono-600.woff2` | Code/identifier face 600 | @fontsource/jetbrains-mono | 5.1.0 | OFL-1.1 | `vendor/licenses/jetbrains-mono-OFL.txt` |
| `logos.css` | All styling; the §1.2 tokens are its `:root` | authored | — | MIT (project) | [`LICENSE`](../../LICENSE) |
| `nav-progress.js` | In-flight navigation/fragment loading affordance (FR-UI-07); progressive enhancement, references no external origin | authored | — | MIT (project) | [`LICENSE`](../../LICENSE) |
| `graph.js` | Interactive whole-graph canvas bootstrap (instantiates the ECharts graph series over `/api/graph`, FR-UI-08/ADR-29) | authored | — | MIT (project) | [`LICENSE`](../../LICENSE) |
| `mermaid-init.js` | Wiki Mermaid bootstrap — initializes the vendored bundle and renders every `.mermaid` block same-origin (FR-WK-15/S-111); references no external origin | authored | — | MIT (project) | [`LICENSE`](../../LICENSE) |
| `../../assets/logo/logos-lockup.png` | Header brand lockup (icon + wordmark, 252×64) | user-provided Logos brand asset (repo-root `assets/logo/`) | — | MIT (project) | [`LICENSE`](../../LICENSE) |

The font WOFF2 files are the **latin subset** from Fontsource (smallest set that
covers the dashboard's ASCII identifiers/paths) — chosen to keep the ui artifact
small against the [NFR-PC-04](../../docs/specs/requirements/NFR-PC-04.md) budget.

## Checksums (SHA-256)

| File | Bytes | SHA-256 |
|------|------:|---------|
| `fonts/inter-400.woff2` | 23692 | `dd05e326cf8eac3b55acecf29c842ed73e6e6dd06491cf47f7e8800680ab3e33` |
| `fonts/inter-600.woff2` | 24304 | `62553d159189834af73c9a6264704be5b2bee9a08da66a14768d8e5c6ffd2cdb` |
| `fonts/inter-700.woff2` | 24352 | `aac638f7503cebb084ec494cf00f75f7d8260d50c2f4e7820bccabba09626a3a` |
| `fonts/jetbrains-mono-400.woff2` | 21088 | `7c53386f55c866c1b4c9309c4bcf74eda10896aab3a1780b0af5cc4976e27a27` |
| `fonts/jetbrains-mono-600.woff2` | 21936 | `1cd6778760d101a5c522f5d1de6fe17efa9e66950bcd5fce274ae3b4f494f923` |
| `vendor/echarts-graph.min.js` | 590089 | `1aa6ae7a664158866724b0fafe0b3b886c5e1ba79672e648269705bceafa1ab7` |
| `vendor/htmx.min.js` | 50917 | `e209dda5c8235479f3166defc7750e1dbcd5a5c1808b7792fc2e6733768fb447` |
| `vendor/mermaid.min.js` | 3336760 | `5a8ec91820bd55afef049068489369910e5d6ce70c8103952f27e29d3e76e8bc` |
| `vendor/uplot.min.css` | 1857 | `df630c6a8d6f8eeaff264b50f73ce5b114f646ffd9a0bb74f049b0a00135fa04` |
| `vendor/uplot.min.js` | 50312 | `2d27e8ad3d228164525ce213f9dc716f39b4e3aee0cc773fb3491c96cf4921a2` |
| `../../assets/logo/logos-lockup.png` | 4378 | `fc0bebf20bdb43ad01157ae9a24513b1d7d3c350611554ef7ea65ff5c45e0e25` |

Total vendored third-party payload: **~4.1 MB** (≈101 KB other JS+CSS, ≈115 KB
fonts, the slim ECharts graph build ≈590 KB — cytoscape ≈365 KB was removed in
HF-1 — plus the Mermaid single-bundle ≈3.2 MB added in S-111/CR-028). The Mermaid
bundle is a UMD single-file build (its in-tree d3/dagre/etc. are already bundled);
it loads only on a Wiki page that carries a diagram. Recorded against the
[NFR-PC-04](../../docs/specs/requirements/NFR-PC-04.md) `ui`-artifact budget in the
implementation notes.
