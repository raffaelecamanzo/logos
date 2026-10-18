# Graph-context fixture — a finding's enclosing symbol, reachability and blast radius

Input for `logos-core/tests/security_context.rs` (S-669, FR-SC-13). The test copies
`axum/`, `spring/` and `scripts/` into a temporary git repository, indexes it, and
ingests the two reports here.

| Path | Shape | What it proves |
|------|-------|----------------|
| `axum/src/main.rs` | An Axum router whose handler `show_user` calls `db::find_user` in a second file | A finding in a handler is `reachable` (1 hop), `resolved`; the cross-file call gives Rust a cross-file `calls` figure; a finding on the `.route(...)` line binds to `app`, never to the route node whose span that line is |
| `axum/src/db.rs` | `find_user` (called by the handler) and `purge` (called by nothing) | `reachable` at 2 hops; `unreachable` where calls resolve and no route reaches |
| `axum/Dockerfile` | An IaC file the index holds only as config (`DockerfileStage`) nodes | A finding there is bound to the file but `unbound` (`no_symbol`): config nodes never enclose a finding |
| `axum/Cargo.lock` | A lockfile, excluded from the index by default | A dependency finding is `unbound` (`not_indexed`) with its manifest line, 7 |
| `spring/.../UserController.java` | A `@RestController` whose `@GetMapping` handler calls `UserService.find` through a typed field | A finding in a Spring handler is `reachable`, `resolved` |
| `spring/.../UserService.java` | The service the handler calls | Gives Java its cross-file `calls` figure |
| `scripts/report.py` | One Python file: its language resolves no call across files | A finding there is `unresolved` and `unknown`, never `unreachable` |
| `semgrep.sarif` | A Semgrep OSS report: two findings in each of `axum/src/main.rs` and `axum/src/db.rs`, one in each other source file above and in the `Dockerfile`, and one result with no location (`no_location`) | — |
| `osv-scanner.sarif` | An osv-scanner report with one finding in `axum/Cargo.lock` | — |

Both reports are written by hand in the shape those scanners write (see
`../sarif/README.md`); the line numbers they name are the lines the table describes,
so editing a source file here means updating its report.
