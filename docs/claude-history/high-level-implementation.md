Implement this plan with each stage on its own branch, stacked as necessary on previous branches, so that a reviewer can review each branch in isolation.

## Stage 1: Newtypes, config parsing, and module skeleton

**Dependencies**: None

**Implements**: overall-design.md §Architecture/Core types (newtypes, `Config`, `RepoConfig`), §Implementation stages/Stage 1

Add `serde`, `toml`, and `serde_json` dependencies. Create the module structure (`config.rs`, `github/mod.rs`, `github/types.rs`, `facts.rs`, `rules/mod.rs`, `workflow/mod.rs`, `report.rs`) with placeholder contents. Implement:

- Newtypes: `Owner`, `RepoName`, `BranchName`, `RepoRef`, `RuleId` — with `Display`, `Debug`, `Serialize`, `Deserialize` as appropriate.
- `config.rs`: `Config` and `RepoConfig` types, parsed from TOML via serde. Include the `disabled_rules` field as `Option<Vec<String>>` (parsed but unused).
- `rules/mod.rs`: `RuleKind` enum, `RuleResult` enum, `RuleOutput` struct — type definitions only, no `evaluate()` logic.
- `facts.rs`: `RepoFacts` struct with placeholder inner types (e.g. `RepoSettings` as an empty struct, `Ruleset` as an empty struct, `workflows: Vec<(String, ())>`). Derive `Serialize`/`Deserialize` so the snapshot path isn't blocked.

**Correctness oracle**:
- Property: TOML config roundtrips through serialize/deserialize (for all generated `Config` values, `toml::from_str(toml::to_string(c))` equals `c`)
- Property: `RepoRef` display format is `"{owner}/{name}"` for all generated owner/name pairs
- Property: `RuleId` string representation preserves the inner value
- The project compiles and all existing tests pass (`cargo test`)

---

## Stage 2: GitHub Actions workflow model and YAML parser

**Dependencies**: Stage 1

**Implements**: overall-design.md §Architecture/Workflow model, §Implementation stages/Stage 2

Add `serde_yml` dependency. Implement `workflow/model.rs` with the rules-driven subset of GitHub Actions workflow syntax:

- `Workflow`: name, `on` (triggers), jobs map
- Trigger types: `push`, `pull_request`, `pull_request_target`, `workflow_dispatch` — handling GitHub's polymorphism (`on` as string, list, or map)
- Push/PR trigger filters: `branches`, `paths`
- `Job`: `runs-on` (string or object), `steps`, `needs`, `if` condition
- `Step`: `uses` (action ref), `run`, `name`, `id`, `if`, `with`
- `ActionRef`: parsed from the `uses` string into owner/repo + version string, so rules can check whether the version is a SHA

All structs use `#[serde(default)]` / skip unknown fields (no `deny_unknown_fields`). Unknown fields are silently ignored.

**Correctness oracle**:
- Property: workflow YAML roundtrips through deserialize/serialize/deserialize (second deserialize equals first, for all generated `Workflow` values)
- Property: for all generated `ActionRef` values, `parse(display(ref))` equals `ref`
- Test: parse this repo's own `.github/workflows/*.yml` files without error
- Test: parse hand-crafted YAML exercising polymorphic cases — `on: push` (string), `on: [push, pull_request]` (list), `on: { push: { branches: [main] } }` (map); `runs-on: ubuntu-latest` (string) vs `runs-on: { group: foo }` (object)
- Test: YAML with unknown fields deserializes without error (unknown fields are dropped)

---

## Stage 3: GitHub API client

**Dependencies**: Stage 1

**Implements**: overall-design.md §Architecture/GitHub API client, §Implementation stages/Stage 3

Add `ureq` dependency. Implement `github/client.rs`:

- `GitHubClient` struct holding a token (`String`) and rate-limit state
- Auth: `GITHUB_TOKEN` env var read at the boundary (in `main.rs`), passed as a value to the client constructor
- Rate limiting: parse `X-RateLimit-Remaining` and `X-RateLimit-Reset` response headers; sleep when near the limit
- Pagination: follow `Link: <url>; rel="next"` headers, collecting all pages
- Retry: bounded backoff on 5xx / network errors (max 3 retries)

Implement `github/types.rs` with response types for:

- `GET /repos/{owner}/{repo}` — repo metadata, default branch, settings
- `GET /repos/{owner}/{repo}/rulesets` — list of rulesets (paginated)
- `GET /repos/{owner}/{repo}/rulesets/{id}` — single ruleset detail
- `GET /repos/{owner}/{repo}/contents/{path}` — file contents (base64-encoded)
- `GET /repos/{owner}/{repo}/git/trees/{sha}?recursive=1` — file listing

**Correctness oracle**:
- Unit tests for `Link` header parsing: property test that for all generated URLs, a `Link` header containing `<url>; rel="next"` parses correctly; edge cases (no next link, multiple rels) handled
- Unit tests for rate-limit header parsing: property test over generated remaining/reset values
- Unit tests for retry logic: verify bounded retries (mock-free — test the retry *decision function*, not actual HTTP calls)
- Integration test (gated behind `#[cfg(feature = "integration")]` or `#[ignore]`): authenticate against a real public repo and successfully fetch repo metadata. Requires `GITHUB_TOKEN` env var.

---

## Stage 4: Facts gathering and JSON snapshot support

**Dependencies**: Stage 2, Stage 3

**Implements**: overall-design.md §Architecture/Snapshot mode, §Implementation stages/Stage 4

Wire together the GitHub client and workflow parser to build `RepoFacts`:

- `facts.rs`: given a `GitHubClient` + `RepoRef`, fetch repo settings, rulesets, workflow files (list `.github/workflows/` via tree endpoint, fetch and parse each), and check file existence (e.g. `flake.nix`, `CODEOWNERS`)
- Replace placeholder types from Stage 1 with real `RepoSettings`, `Ruleset`, and `Workflow` types
- JSON snapshot serialization/deserialization of `RepoFacts` (already derived in Stage 1, but now with real content)
- CLI flags: `--snapshot-save <dir>` dumps `RepoFacts` as JSON per repo; `--snapshot-load <dir>` reads saved facts (no GitHub token needed)
- Commit initial test fixtures in `tests/fixtures/`: at least one known-good and one known-bad snapshot (can be hand-crafted or captured from a real repo)

**Correctness oracle**:
- Property: `RepoFacts` JSON roundtrips (`serde_json::from_str(serde_json::to_string(facts))` equals `facts`, for all generated `RepoFacts`)
- Test: snapshot-save then snapshot-load produces identical `RepoFacts`
- Integration test (gated): fetch facts for a known public repo, verify key fields are populated (e.g. default branch is non-empty, at least one workflow file parsed)
- Test: `--snapshot-load` with the committed test fixtures deserializes without error

---

## Stage 5: Rule evaluation

**Dependencies**: Stage 4

**Implements**: overall-design.md §Architecture/Core types (`RuleKind` variants, `evaluate()`), §Implementation stages/Stage 5, §ID code scheme

Implement `evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult` for every `RuleKind` variant. Define the default rule set (all rules, hardcoded list). Group rules by ID prefix:

- `RS0xx`: `RulesetExists`, `RulesetRequiresStatusCheck`, `RulesetRequiresReviewers`, `RulesetEnforcesAdmins`, `RulesetRequiresLinearHistory`, `RulesetPreventsForcePush`, `UsesRulesetsNotLegacyProtection`
- `WF0xx`: `WorkflowExistsForDefaultBranch`, `WorkflowHasJob`, `WorkflowActionsPinnedToSha`, `NoPullRequestTargetWithCheckout`, `WorkflowUsesAction`
- `FL0xx`: `FileExists`
- `NX0xx`: `NixFlakeExists`, `NixFlakeHasCheck`
- `ST0xx`: `RepoSettingMatch`

**Correctness oracle**:
- Property: for all `RepoFacts` where `rulesets` is empty, `RulesetExists` evaluates to `Fail`
- Property: for all `RepoFacts` with at least one ruleset, `RulesetExists` evaluates to `Pass`
- Property: for all `RepoFacts` containing a workflow with an action ref that is not a 40-char hex string, `WorkflowActionsPinnedToSha` evaluates to `Fail`
- Property: for all `RepoFacts` where every action ref is a 40-char hex string, `WorkflowActionsPinnedToSha` evaluates to `Pass`
- Property: for all `RepoFacts` where `files_present` does not contain a given path, `FileExists { path }` evaluates to `Fail`
- Property: for all generated `RepoFacts` and all `RuleKind` variants, `evaluate` returns one of the four `RuleResult` variants (does not panic)
- Test: evaluate all rules against the committed known-good snapshot; verify expected passes
- Test: evaluate all rules against the committed known-bad snapshot; verify expected failures

---

## Stage 6: Reporting and CLI wiring

**Dependencies**: Stage 5

**Implements**: overall-design.md §Implementation stages/Stage 6, §Verification

Implement `report.rs` and wire everything together in `main.rs`:

- `report.rs`: format rule results as a terminal table (human-readable) and as JSON (machine-readable), selectable via CLI flag (e.g. `--format json|text`)
- `main.rs`: read config from `--config <path>`, create `GitHubClient` (or load snapshots), gather facts per repo, evaluate the default rule set, report results, set exit code (0 if all pass, 1 if any fail)
- End-to-end: a config file pointing at snapshot fixtures produces deterministic output

**Correctness oracle**:
- Test: given a config + `--snapshot-load` pointing at committed fixtures, the process exits with the expected exit code
- Test: JSON output deserializes into the expected `Vec<RepoReport>` structure (each `RepoReport` contains a `repo` field and a `rules: Vec<RuleOutput>` field)
- Test: human-readable output contains each rule ID and its pass/fail status
- Smoke test: `cargo run -- --config example.toml --snapshot-load tests/fixtures/` runs without error and produces non-empty output
