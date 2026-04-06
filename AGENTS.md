Standard Cargo pipeline, with toolchain defined in a Nix devshell, should already be on PATH because of direnv.

We keep Clippy passing, and we don't suppress Clippy lints without having a very good reason for it.

Some integration tests are gated behind `#[ignore]` and require `GITHUB_TOKEN` + network access.
Don't run them yourself; you can ask the user to run them explicitly with `cargo test -- --ignored`.

## Purpose

The app checks that many GitHub repos all conform to specific required standards, like "main branch is protected".

## Architecture

CLI app, following a **gather-evaluate-report** pipeline:

1. **Config** (`src/config.rs`): reads a TOML file listing repos to audit (owner/name, optional disabled_rules).
2. **Facts** (`src/facts.rs`): gathers a `RepoFacts` struct per repo — settings, rulesets, workflows, file tree — either live from the GitHub API or from JSON snapshot files (`--snapshot-load`/`--snapshot-save`).
3. **Rules** (`src/rules/mod.rs`): a closed set of `RuleKind` variants (e.g. `RulesetExists`, `WorkflowActionsPinnedToSha`, `FileExists`). Each is evaluated against `RepoFacts` producing `Pass`/`Fail`/`Skip`/`Error`. The default rule set is defined in `default_rules()`. Rule IDs use prefixes: RS (rulesets), WF (workflows), FL (files), NX (Nix), ST (repo settings).
4. **Report** (`src/report.rs`): renders rule outputs as text or JSON. Exit code 1 if any rule fails or errors.

We don't use Octokit but instead have our own GH client. (Every time I use Octokit, I've eventually ended up having to write my own anyway.)

## Testing

Tests use a hand-rolled snapshot system: `tests/fixtures/` contains TOML configs and JSON `RepoFacts` snapshots so the full pipeline can run without network access.
The `--snapshot-save` flag captures live API responses for new fixtures.

Property-based tests (proptest) cover serialization roundtrips for configs, facts, workflow models, and GitHub types.

## Consumers

Back-compat is unimportant: this is a personal project with one consumer.
