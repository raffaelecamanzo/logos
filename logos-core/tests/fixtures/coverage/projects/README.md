# Coverage tool fixture projects (S-735)

One small project per registered coverage tool, each with its ecosystem
marker, a little source and the report the tool writes for it
(`fixture-report*.xml` / `fixture-report.out`). The coverage runner's tests
copy a project out, put a stub of the tool on a test-only `PATH`, run the
tool's fixed argv against it and ingest what the stub wrote. No real tool
runs in the default test suite.

The ignored test `real_tools_against_their_fixture_projects` runs the real
tools found on `PATH` (or in the project) against the same projects, and
reports each tool it could not find as not run. Running Maven, Gradle or a
JS tool fetches dependencies, so that run is the owner's, by hand:

    LOGOS_COVERAGE_REAL_TOOLS=1 cargo test -p logos-core --lib \
      coverage::tests::real_tools_against_their_fixture_projects -- --ignored --nocapture

## Dependency-addition overlays (S-749)

`deps/` holds overlays for the coverage dev-dependency tests: each is copied
over a tool's project (`vitest` or `pytest-cov`) to give it a manifest
without the coverage dependency and the lockfile that names its package
manager — `pnpm-vitest` (pnpm), `uv`, `poetry` — or no lockfile at all
(`no-lockfile`). The package manager is a stub on the test-only `PATH`
that records its argv and changes the manifest and lockfile as the real
one would; no real package manager runs.
