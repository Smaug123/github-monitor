# github-infra

`github-infra` audits one or more GitHub repositories against a fixed set of repository standards.

Today it checks repository settings, branch rulesets, workflow safety, and a small set of required files. It can run either:

- live against the GitHub API, or
- offline from previously saved JSON snapshots

## Quick Start

This repo normally gets its Rust toolchain from the Nix dev shell.

```sh
direnv allow
# or: nix develop
```

Create a config file:

```toml
[[repos]]
owner = "example-org"
name = "service-a"

[[repos]]
owner = "example-org"
name = "service-b"
```

Run against live GitHub data:

```sh
export GITHUB_TOKEN=ghp_your_token_here
cargo run -- --config repos.toml
```

Run against the committed offline fixtures instead:

```sh
cargo run -- --config tests/fixtures/repos.toml --snapshot-load tests/fixtures
```

## What You Need

For live runs: `GITHUB_TOKEN` with enough access to read repository metadata, rulesets, git trees, and repository file contents for the repos being audited

## CLI Reference

There is no built-in `--help` flag yet. These are the supported arguments:

| Flag | Required | Meaning |
| --- | --- | --- |
| `--config <path>` | yes | TOML file listing repos to audit |
| `--snapshot-load <dir>` | no | Load repo facts from JSON snapshots under `<dir>` instead of calling GitHub |
| `--snapshot-save <dir>` | no | Fetch live repo facts from GitHub and save snapshots under `<dir>` |
| `--format <text|json>` | no | Output format. Defaults to `text` |

Notes:

- `--snapshot-load` and `--snapshot-save` are mutually exclusive.
- If you do not use `--snapshot-load`, `GITHUB_TOKEN` is required.
- `--snapshot-save` still performs a live GitHub audit before writing snapshots.

## Config File Format

The config file is TOML with one `[[repos]]` entry per repository:

```toml
[[repos]]
owner = "example-org"
name = "good-repo"
```

The parser also accepts an optional `disabled_rules` field:

```toml
[[repos]]
owner = "example-org"
name = "example-repo"
disabled_rules = ["NX002"]
```

`disabled_rules` is reserved for future use. It is parsed today, but the current evaluator does not apply it yet.

## Output

### Text Output

The default output is a human-readable report per repository, followed by an overall summary.

Example:

```text
Repository: example-org/good-repo
STATUS  RULE   NAME
PASS    RS001  Rulesets exist
PASS    RS002  CI status check is required
SKIP    NX002  The flake has observable check coverage
        reason: RepoFacts does not yet capture flake outputs; only explicit `nix flake check` workflow steps can prove this rule
Summary: 16 pass, 0 fail, 2 skip, 0 error

Overall: 16 pass, 0 fail, 2 skip, 0 error
```

Status meanings:

- `PASS`: the repository satisfied the rule
- `FAIL`: the repository violated the rule
- `SKIP`: the rule could not be decided from the available facts
- `ERROR`: evaluation failed unexpectedly

Any non-`PASS` result includes a `reason`.

### JSON Output

Use `--format json` for machine-readable output:

```sh
cargo run -- --config repos.toml --format json
```

The top-level JSON value is an array of per-repository reports:

```json
[
  {
    "repo": {
      "owner": "example-org",
      "name": "good-repo"
    },
    "rules": [
      {
        "id": "RS001",
        "name": "Rulesets exist",
        "result": "Pass"
      },
      {
        "id": "NX002",
        "name": "The flake has observable check coverage",
        "result": {
          "Skip": {
            "reason": "RepoFacts does not yet capture flake outputs; only explicit `nix flake check` workflow steps can prove this rule"
          }
        }
      }
    ]
  }
]
```

## Exit Codes

- `0`: all evaluated rules passed or were skipped
- `1`: at least one rule failed, at least one rule errored, or the application could not complete a run
- `2`: invalid CLI usage, such as a missing required flag or an unknown argument

## Live Runs And Snapshots

### Live Audit

A normal run fetches facts from GitHub, evaluates rules, and prints a report:

```sh
export GITHUB_TOKEN=ghp_your_token_here
cargo run -- --config repos.toml
```

### Save Snapshots

This mode fetches live GitHub data, writes one JSON snapshot per repository, and evaluates the same run:

```sh
export GITHUB_TOKEN=ghp_your_token_here
cargo run -- --config repos.toml --snapshot-save snapshots
```

Snapshots are written as:

```text
snapshots/<owner>/<repo>.json
```

For example:

```text
snapshots/example-org/service-a.json
```

### Load Snapshots

This mode does not call GitHub. It loads previously saved JSON files and evaluates them locally:

```sh
cargo run -- --config repos.toml --snapshot-load snapshots
```

This is useful for:

- repeatable tests
- offline debugging
- reviewing a known set of repository facts in CI

## Current Rules

The default rule set is currently fixed in code.

| ID | What it checks |
| --- | --- |
| `RS001` | At least one active branch ruleset applies to the default branch |
| `RS002` | An active branch ruleset requires the `ci` status check |
| `RS003` | An active branch ruleset requires at least two approving reviews |
| `RS004` | Organization admins and repository roles cannot bypass active branch rulesets |
| `RS005` | Active branch rulesets require linear history |
| `RS006` | Active branch rulesets prevent force pushes |
| `RS007` | The repo uses rulesets instead of legacy branch protection |
| `WF001` | At least one workflow runs on pushes to the default branch |
| `WF002` | GitHub Actions references are pinned to 40-character commit SHAs |
| `WF003` | `pull_request_target` workflows do not use `actions/checkout` |
| `FL001` | `CODEOWNERS` exists at the repository root |
| `NX001` | `flake.nix` exists |
| `NX002` | The flake has observable check coverage |
| `ST001` | `allow_auto_merge = true` |
| `ST002` | `delete_branch_on_merge = true` |
| `ST003` | `allow_update_branch = true` |
| `ST004` | `allow_merge_commit = false` |
| `ST005` | `allow_rebase_merge = true` |

## Current Limitations

- The rule set is hard-coded. There is no CLI flag for selecting a custom subset of rules.
- `disabled_rules` is parsed from config but not enforced yet.
- `RS007` may return `SKIP` because the current fact model does not record legacy branch protection state.
- `NX002` may return `SKIP` because the current fact model does not capture full flake outputs.

## Example Offline Run

The repo includes committed fixtures, so you can try the tool without GitHub access:

```sh
cargo run -- --config tests/fixtures/repos.toml --snapshot-load tests/fixtures
```

That example intentionally includes one passing repository and one failing repository, so it should exit with status `1`.
