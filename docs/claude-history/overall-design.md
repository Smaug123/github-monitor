# github-infra: Implementation Plan

## Context

We have a skeleton Rust app that needs to become a GitHub repo compliance checker. It reads a TOML config listing repos, fetches facts about each repo via the GitHub API, and evaluates a set of rules (Rust code) against those facts. Key decisions already made:

- TOML config, explicit repo listing (no org enumeration)
- Roll our own GitHub API client (no octocrab) — `ureq` for sync HTTP
- Rules-driven subset of GitHub Actions workflow syntax (serde ignores unknown fields, no deny_unknown_fields)
- GitHub Rulesets API only (legacy branch protection is itself a failure)
- Rules are a `RuleKind` enum + interpreter function, not trait objects
- Rules have ID codes; config schema permits future rule disabling but doesn't implement it yet

---

## Architecture

```
src/
  main.rs              CLI entry, wiring (imperative shell)
  config.rs            TOML config parsing
  github/
    mod.rs
    client.rs          HTTP client: auth, rate limiting, pagination
    types.rs           GitHub API response types (repo settings, rulesets)
  workflow/
    mod.rs
    model.rs           Full structural model of GH Actions workflow syntax
  facts.rs             RepoFacts: unified data model rules operate on
  rules/
    mod.rs             RuleKind enum, RuleId, RuleResult, evaluate()
  report.rs            Terminal + JSON output
```

### Core types

```rust
// Newtypes (no primitive obsession)
struct Owner(String);
struct RepoName(String);
struct BranchName(String);

struct RepoRef { owner: Owner, name: RepoName }

// Config
struct Config {
    repos: Vec<RepoConfig>,
    // Future: rules: Option<RulesConfig>
}
struct RepoConfig {
    owner: String,
    name: String,
    // Future: disabled_rules: Option<Vec<String>>
}

// Rule results
enum RuleResult {
    Pass,
    Fail { reason: String },
    Skip { reason: String },
    Error { reason: String },
}

struct RuleId(&'static str);  // e.g. "RS001", "WF001", "NX001"

struct RuleOutput {
    id: RuleId,
    name: &'static str,
    result: RuleResult,
}

// The key data description: everything we know about a repo
struct RepoFacts {
    repo: RepoRef,
    settings: RepoSettings,
    rulesets: Vec<Ruleset>,
    default_branch: BranchName,
    workflows: Vec<(String, Workflow)>,  // (filename, parsed workflow)
    files_present: HashSet<String>,       // paths that exist in the repo
}

// Rules as a closed enum
enum RuleKind {
    RulesetExists,
    RulesetRequiresStatusCheck { check_name: String },
    RulesetRequiresReviewers { min_count: u32 },
    RulesetEnforcesAdmins,
    RulesetRequiresLinearHistory,
    RulesetPreventsForce Push,
    WorkflowExistsForDefaultBranch,
    WorkflowHasJob { job_name: String },
    WorkflowActionsPinnedToSha,
    NoPullRequestTargetWithCheckout,
    WorkflowUsesAction { action: String },
    FileExists { path: String },
    NixFlakeExists,
    NixFlakeHasCheck,
    RepoSettingMatch { setting: RepoSetting, expected: SettingValue },
    UsesRulesetsNotLegacyProtection,
}

fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult { ... }
```

### GitHub API client

Hand-rolled over `ureq`:
- Auth via `GITHUB_TOKEN` env var (parsed at boundary, threaded as value)
- Rate limiting: parse `X-RateLimit-Remaining` / `X-RateLimit-Reset` headers, sleep when near limit
- Pagination: follow `Link` headers for paginated endpoints
- Retry with backoff on 5xx / network errors (bounded: max 3 retries)

Endpoints needed:
- `GET /repos/{owner}/{repo}` — repo settings, default branch
- `GET /repos/{owner}/{repo}/rulesets` — list rulesets
- `GET /repos/{owner}/{repo}/rulesets/{id}` — ruleset details
- `GET /repos/{owner}/{repo}/contents/{path}` — file contents (workflow files, flake.nix)
- `GET /repos/{owner}/{repo}/git/trees/{sha}?recursive=1` — file listing

### Workflow model

Rules-driven subset of the GitHub Actions workflow syntax. We model the fields our rules need to query, and use `#[serde(flatten)]` or simply ignore unknown fields so we don't break on unmodelled fields. Structures grow as rules need new fields.

Initial fields needed (driven by planned rules):

- **Workflow**: name, on (triggers), jobs
- **Triggers**: push, pull_request, pull_request_target, workflow_dispatch (enough to check "runs on push to main", "no dangerous pull_request_target")
- **Push/PR trigger filters**: branches, paths
- **Job**: name (map key), runs-on, steps, needs, `if`
- **Step**: uses (action ref), run (shell command), name, id, `if`, with
- **Action reference**: parsed into owner/repo, version string — rules check whether version is a SHA

Deserialized from YAML via `serde_yml`. Must handle GitHub's polymorphism (e.g. `on` can be a string, list, or map; `runs-on` can be a string or object). Unknown fields are silently ignored.

### Snapshot mode

`RepoFacts` is serializable to/from JSON. This enables:
- Deterministic offline evaluation and testing
- Diffing compliance state over time
- CI without a GitHub token (check committed snapshots)

---

## Implementation stages

### Stage 1: Config + core types + module skeleton
- Add dependencies: `serde`, `toml`, `serde_json`, `serde_yml`
- `config.rs`: parse TOML into typed `Config`
- Newtypes: `Owner`, `RepoName`, `BranchName`, `RuleId`
- `rules/mod.rs`: `RuleKind`, `RuleResult`, `RuleOutput` types (no evaluation logic yet)
- `facts.rs`: `RepoFacts` struct (with placeholder inner types)
- Property tests: roundtrip config serialization, newtype construction

### Stage 2: GitHub Actions workflow model + parser
- `workflow/model.rs`: rules-driven subset of structural types
- Serde deserialization with `serde_yml`, handling GitHub's polymorphic fields
- Unknown fields silently ignored (no `deny_unknown_fields`)
- Property tests: parse real-world workflow files (including this repo's own `ci.yml`), roundtrip serialization, test polymorphic cases (on-as-string vs on-as-map, etc.)
- This stage has no GitHub API dependency — pure parsing

### Stage 3: GitHub API client
- `github/client.rs`: `GitHubClient` struct with token, rate limit tracking
- `github/types.rs`: response types for repo settings, rulesets
- Implement endpoints: repo info, rulesets, file contents, tree listing
- Rate limiting, pagination, retry logic
- Integration test (gated behind a feature flag or env var) against a real repo

### Stage 4: Facts gathering + snapshots (first-class)
- `facts.rs`: orchestration — given `GitHubClient` + `RepoRef`, build `RepoFacts`
- Fetch repo settings + default branch
- Fetch rulesets
- List `.github/workflows/` and parse each file
- Check file existence (flake.nix, flake.lock, CODEOWNERS, etc.)
- JSON snapshot serialization/deserialization of `RepoFacts`
- `--snapshot save <dir>` to dump facts per repo
- `--snapshot load <dir>` to evaluate rules offline from saved facts
- Snapshots are the primary testing mechanism: commit known-good and known-bad snapshots, run rules against them in CI without a GitHub token

### Stage 5: Rules
- `rules/mod.rs`: implement `evaluate()` for each `RuleKind` variant
- Define the default rule set (all rules, hardcoded)
- Property tests: e.g. "for all RepoFacts where rulesets is empty, RulesetExists fails", "for all workflows containing an action ref not pinned to SHA, WorkflowActionsPinnedToSha fails"
- Test against snapshots of known-good and known-bad repos

### Stage 6: Reporting + CLI wiring
- `report.rs`: terminal table output, JSON output
- `main.rs`: read config, create client, gather facts per repo, evaluate rules, report, set exit code
- End-to-end test: config file + snapshot -> expected output

---

## Dependencies

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
serde_json = "1"
serde_yml = "0.0.12"
ureq = { version = "3", features = ["json"] }

[dev-dependencies]
proptest = "1"
```

## Verification

- `cargo test` — unit + property tests at each stage
- `cargo run -- --config example.toml` — end-to-end against real repos (requires GITHUB_TOKEN)
- `cargo run -- --config example.toml --snapshot save ./snapshots/` — save facts for offline use
- `cargo run -- --config example.toml --snapshot load ./snapshots/` — evaluate rules from saved snapshots (no token needed)
- CI: `nix build` + `nix develop --command cargo test` (existing workflow)
- Committed test snapshots (known-good, known-bad repos) in `tests/fixtures/` for deterministic rule testing

## ID code scheme for rules

Prefix by category:
- `RS0xx` — Ruleset rules
- `WF0xx` — Workflow/CI rules  
- `FL0xx` — File existence rules
- `NX0xx` — Nix-specific rules
- `ST0xx` — Repo settings rules
