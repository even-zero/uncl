# Releases

Code-bearing pushes to `dev` build a draft; code-bearing pushes to `master`
build and publish their own source to crates.io and GitHub. Master never
publishes an arbitrary waiting draft. Commit titles (including `[skip release]`)
do not suppress code changes. Docs-only pushes are compared against the latest
public release, so a previously skipped fix is still detected.

Versions use a project-specific base-99 release counter in MAJOR.MINOR.PATCH
format: `0.1.97 -> 0.1.98 -> 0.2.0`, and `0.98.98 -> 1.0.0`.
Each qualifying push allocates one increment beyond the largest manifest or
published tag version. This is an automatic counter, not conventional SemVer's
breaking-change/feature classification.

To force a fresh release, dispatch the `release` workflow at the desired ref
with `publish=true`. This always tests, increments, builds all four platforms,
creates a draft targeting that exact source commit, publishes the crate, then
makes that same GitHub release public. Never manually publish a stale draft.
A failed publish leaves the draft private; rerun the failed jobs to retry.

The workflow requires contents write permission (including permission to push
version commits to its triggering branch) and the existing
`CARGO_REGISTRY_TOKEN` secret. Token-generated version commits do not trigger
another workflow. Concurrent source pushes reject the version commit rather
than replacing newer source. Rust tests run in CI before allocating a version.

Release counter tests: `python3 -m unittest discover -s .github/scripts -p 'test_*.py'`.
