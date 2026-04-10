mod config;
mod facts;
mod github;
mod remediation;
mod report;
mod rules;
mod types;
mod workflow;

use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::{Config, ConfigError};
use crate::facts::{
    FactsError, RepoFacts, SnapshotError, gather_repo_facts, load_snapshot, save_snapshot,
};
use crate::github::client::{GitHubClient, GitHubToken};
use crate::remediation::{PlannedFix, RepoFix, execute_repo_fixes, plan_repo_fixes};
use crate::report::{OutputFormat, OutputFormatError, RepoReport, ReportError};
use crate::rules::{default_rules, evaluate_rules};
use crate::types::RepoRef;

const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";

fn github_token_from_env() -> Option<GitHubToken> {
    GitHubToken::from_env(GITHUB_TOKEN_ENV)
}

fn main() -> ExitCode {
    match try_main() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    }
}

fn try_main() -> Result<ExitCode, MainError> {
    let args = parse_cli_args(std::env::args().skip(1)).map_err(MainError::Cli)?;
    let output = run(args).map_err(MainError::App)?;
    print!("{}", output.rendered);
    Ok(output.exit_code())
}

fn run(args: CliArgs) -> Result<RunOutput, AppError> {
    let config = Config::from_path(&args.config_path)?;
    let reports = match args.execution_mode {
        ExecutionMode::Plan => {
            let facts = load_facts(&config, &args.snapshot_mode)?;
            evaluate_repo_reports(facts, build_planned_repo_fixes)
        }
        ExecutionMode::Execute => execute_fix_run(&config, &args.snapshot_mode)?,
    };
    let rendered = report::render(args.format, &reports)?;

    Ok(RunOutput { reports, rendered })
}

fn load_facts(config: &Config, snapshot_mode: &SnapshotMode) -> Result<Vec<RepoFacts>, AppError> {
    let repos = config.repo_refs();

    match snapshot_mode {
        SnapshotMode::Load(snapshot_dir) => repos
            .iter()
            .map(|repo| load_snapshot(snapshot_dir, repo).map_err(AppError::from))
            .collect(),
        SnapshotMode::Save(snapshot_dir) => {
            let facts = gather_facts_from_github(&repos)?;
            for repo_facts in &facts {
                save_snapshot(snapshot_dir, repo_facts)?;
            }
            Ok(facts)
        }
        SnapshotMode::None => gather_facts_from_github(&repos),
    }
}

fn gather_facts_from_github(repos: &[RepoRef]) -> Result<Vec<RepoFacts>, AppError> {
    let token = github_token_from_env().ok_or(AppError::MissingGitHubToken {
        env_var: GITHUB_TOKEN_ENV,
    })?;
    let mut client = GitHubClient::new(token);
    gather_facts_from_github_with_client(&mut client, repos)
}

fn gather_facts_from_github_with_client(
    client: &mut GitHubClient,
    repos: &[RepoRef],
) -> Result<Vec<RepoFacts>, AppError> {
    let mut facts = Vec::with_capacity(repos.len());

    for repo in repos {
        let repo_facts =
            gather_repo_facts(client, repo.clone()).map_err(|source| AppError::Facts {
                repo: repo.clone(),
                source: Box::new(source),
            })?;
        facts.push(repo_facts);
    }

    Ok(facts)
}

fn execute_fix_run(
    config: &Config,
    snapshot_mode: &SnapshotMode,
) -> Result<Vec<RepoReport>, AppError> {
    let repos = config.repo_refs();
    let token = github_token_from_env().ok_or(AppError::MissingGitHubToken {
        env_var: GITHUB_TOKEN_ENV,
    })?;
    let mut client = GitHubClient::new(token);
    let initial_facts = gather_facts_from_github_with_client(&mut client, &repos)?;
    let planned_fixes = plan_repo_fix_batches(&initial_facts);

    if planned_fixes.iter().all(Vec::is_empty) {
        save_facts_if_requested(snapshot_mode, &initial_facts)?;
        return Ok(evaluate_repo_reports(initial_facts, |facts| {
            vec![Vec::new(); facts.len()]
        }));
    }

    let executed_fixes = planned_fixes
        .iter()
        .map(|fixes| execute_repo_fixes(&mut client, fixes))
        .collect::<Vec<_>>();
    let final_facts = gather_facts_from_github_with_client(&mut client, &repos)?;
    save_facts_if_requested(snapshot_mode, &final_facts)?;

    Ok(evaluate_repo_reports(final_facts, move |_| {
        executed_fixes.clone()
    }))
}

fn save_facts_if_requested(
    snapshot_mode: &SnapshotMode,
    facts: &[RepoFacts],
) -> Result<(), AppError> {
    if let SnapshotMode::Save(snapshot_dir) = snapshot_mode {
        for repo_facts in facts {
            save_snapshot(snapshot_dir, repo_facts)?;
        }
    }

    Ok(())
}

fn evaluate_repo_reports<F>(facts: Vec<RepoFacts>, repo_fixes: F) -> Vec<RepoReport>
where
    F: FnOnce(&[RepoFacts]) -> Vec<Vec<RepoFix>>,
{
    let rules = default_rules();
    let repo_fixes = repo_fixes(&facts);
    debug_assert_eq!(facts.len(), repo_fixes.len());

    std::iter::zip(facts, repo_fixes)
        .map(|(repo_facts, fixes)| {
            let outputs = evaluate_rules(&rules, &repo_facts);
            RepoReport::new(repo_facts.repo, outputs, fixes)
        })
        .collect()
}

fn build_planned_repo_fixes(facts: &[RepoFacts]) -> Vec<Vec<RepoFix>> {
    plan_repo_fix_batches(facts)
        .into_iter()
        .map(|fixes| fixes.into_iter().map(|fix| fix.planned_report()).collect())
        .collect()
}

fn plan_repo_fix_batches(facts: &[RepoFacts]) -> Vec<Vec<PlannedFix>> {
    let rules = default_rules();

    facts
        .iter()
        .map(|repo_facts| plan_repo_fixes(&rules, repo_facts))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    config_path: PathBuf,
    snapshot_mode: SnapshotMode,
    format: OutputFormat,
    execution_mode: ExecutionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotMode {
    None,
    Save(PathBuf),
    Load(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExecutionMode {
    Plan,
    Execute,
}

fn parse_cli_args<I, S>(args: I) -> Result<CliArgs, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into);
    let mut config_path = None;
    let mut snapshot_save = None;
    let mut snapshot_load = None;
    let mut format = OutputFormat::Text;
    let mut execution_mode = ExecutionMode::Plan;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => {
                config_path = Some(PathBuf::from(next_arg_value(&mut args, "--config")?));
            }
            "--snapshot-save" => {
                snapshot_save = Some(PathBuf::from(next_arg_value(&mut args, "--snapshot-save")?));
            }
            "--snapshot-load" => {
                snapshot_load = Some(PathBuf::from(next_arg_value(&mut args, "--snapshot-load")?));
            }
            "--format" => {
                let raw = next_arg_value(&mut args, "--format")?;
                format = OutputFormat::parse(&raw).map_err(CliError::InvalidFormat)?;
            }
            "--fix" => execution_mode = ExecutionMode::Execute,
            other => return Err(CliError::UnknownArgument(other.to_owned())),
        }
    }

    let config_path = config_path.ok_or(CliError::MissingRequiredArgument("--config"))?;

    let snapshot_mode = match (snapshot_save, snapshot_load) {
        (Some(_), Some(_)) => return Err(CliError::ConflictingSnapshotModes),
        (Some(path), None) => SnapshotMode::Save(path),
        (None, Some(path)) => SnapshotMode::Load(path),
        (None, None) => SnapshotMode::None,
    };

    if execution_mode == ExecutionMode::Execute && matches!(snapshot_mode, SnapshotMode::Load(_)) {
        return Err(CliError::FixRequiresLiveGitHub);
    }

    Ok(CliArgs {
        config_path,
        snapshot_mode,
        format,
        execution_mode,
    })
}

fn next_arg_value<I>(args: &mut I, flag: &'static str) -> Result<String, CliError>
where
    I: Iterator<Item = String>,
{
    args.next().ok_or(CliError::MissingValue(flag))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliError {
    MissingRequiredArgument(&'static str),
    MissingValue(&'static str),
    ConflictingSnapshotModes,
    FixRequiresLiveGitHub,
    InvalidFormat(OutputFormatError),
    UnknownArgument(String),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingRequiredArgument(flag) => write!(f, "missing required argument {flag}"),
            Self::MissingValue(flag) => write!(f, "missing value for argument {flag}"),
            Self::ConflictingSnapshotModes => {
                f.write_str("only one of --snapshot-save or --snapshot-load may be provided")
            }
            Self::FixRequiresLiveGitHub => {
                f.write_str("--fix may not be used with --snapshot-load because fixes require live GitHub access")
            }
            Self::InvalidFormat(source) => source.fmt(f),
            Self::UnknownArgument(arg) => write!(f, "unknown argument {arg}"),
        }
    }
}

impl std::error::Error for CliError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidFormat(source) => Some(source),
            Self::MissingRequiredArgument(_)
            | Self::MissingValue(_)
            | Self::ConflictingSnapshotModes
            | Self::FixRequiresLiveGitHub
            | Self::UnknownArgument(_) => None,
        }
    }
}

#[derive(Debug)]
enum AppError {
    Config(Box<ConfigError>),
    MissingGitHubToken {
        env_var: &'static str,
    },
    Facts {
        repo: RepoRef,
        source: Box<FactsError>,
    },
    Snapshot(Box<SnapshotError>),
    Report(Box<ReportError>),
}

impl From<ConfigError> for AppError {
    fn from(source: ConfigError) -> Self {
        Self::Config(Box::new(source))
    }
}

impl From<SnapshotError> for AppError {
    fn from(source: SnapshotError) -> Self {
        Self::Snapshot(Box::new(source))
    }
}

impl From<ReportError> for AppError {
    fn from(source: ReportError) -> Self {
        Self::Report(Box::new(source))
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(source) => source.fmt(f),
            Self::MissingGitHubToken { env_var } => {
                write!(
                    f,
                    "missing {env_var}; it is required unless --snapshot-load is used"
                )
            }
            Self::Facts { repo, source } => {
                write!(f, "failed to gather facts for {repo}: {source}")
            }
            Self::Snapshot(source) => source.fmt(f),
            Self::Report(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for AppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(source) => Some(source),
            Self::MissingGitHubToken { .. } => None,
            Self::Facts { source, .. } => Some(source),
            Self::Snapshot(source) => Some(source),
            Self::Report(source) => Some(source),
        }
    }
}

#[derive(Debug)]
enum MainError {
    Cli(CliError),
    App(AppError),
}

impl MainError {
    fn exit_code(&self) -> ExitCode {
        match self {
            Self::Cli(_) => ExitCode::from(2),
            Self::App(_) => ExitCode::from(1),
        }
    }
}

impl std::fmt::Display for MainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cli(source) => source.fmt(f),
            Self::App(source) => source.fmt(f),
        }
    }
}

struct RunOutput {
    reports: Vec<RepoReport>,
    rendered: String,
}

impl RunOutput {
    fn exit_code(&self) -> ExitCode {
        if report::has_failures(&self.reports) || report::has_failed_fixes(&self.reports) {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_path(path: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)
    }

    #[test]
    fn parses_snapshot_load_cli_flags() {
        let args = parse_cli_args([
            "--config",
            "tests/fixtures/repos.toml",
            "--snapshot-load",
            "tests/fixtures",
        ])
        .unwrap();

        assert_eq!(
            args,
            CliArgs {
                config_path: PathBuf::from("tests/fixtures/repos.toml"),
                snapshot_mode: SnapshotMode::Load(PathBuf::from("tests/fixtures")),
                format: OutputFormat::Text,
                execution_mode: ExecutionMode::Plan,
            }
        );
    }

    #[test]
    fn parses_json_output_format() {
        let args = parse_cli_args([
            "--config",
            "tests/fixtures/repos.toml",
            "--snapshot-load",
            "tests/fixtures",
            "--format",
            "json",
        ])
        .unwrap();

        assert_eq!(
            args,
            CliArgs {
                config_path: PathBuf::from("tests/fixtures/repos.toml"),
                snapshot_mode: SnapshotMode::Load(PathBuf::from("tests/fixtures")),
                format: OutputFormat::Json,
                execution_mode: ExecutionMode::Plan,
            }
        );
    }

    #[test]
    fn parses_fix_cli_flag() {
        let args = parse_cli_args(["--config", "tests/fixtures/repos.toml", "--fix"]).unwrap();

        assert_eq!(
            args,
            CliArgs {
                config_path: PathBuf::from("tests/fixtures/repos.toml"),
                snapshot_mode: SnapshotMode::None,
                format: OutputFormat::Text,
                execution_mode: ExecutionMode::Execute,
            }
        );
    }

    #[test]
    fn rejects_fix_with_snapshot_load() {
        assert_eq!(
            parse_cli_args([
                "--config",
                "tests/fixtures/repos.toml",
                "--snapshot-load",
                "tests/fixtures",
                "--fix",
            ])
            .unwrap_err()
            .to_string(),
            "--fix may not be used with --snapshot-load because fixes require live GitHub access"
        );
    }

    #[test]
    fn snapshot_load_reads_committed_fixtures() {
        let config = Config::from_path(fixture_path("tests/fixtures/repos.toml")).unwrap();
        let facts =
            load_facts(&config, &SnapshotMode::Load(fixture_path("tests/fixtures"))).unwrap();

        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].repo, RepoRef::new("example-org", "good-repo"));
        assert_eq!(facts[1].repo, RepoRef::new("example-org", "bad-repo"));
    }

    #[test]
    fn snapshot_run_with_good_repo_only_exits_successfully() {
        let output = run(CliArgs {
            config_path: fixture_path("tests/fixtures/good-repo.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
            format: OutputFormat::Text,
            execution_mode: ExecutionMode::Plan,
        })
        .unwrap();

        assert_eq!(output.exit_code(), ExitCode::SUCCESS);
        assert!(
            output
                .rendered
                .contains("Repository: example-org/good-repo")
        );
        assert!(output.rendered.contains("PASS    RS001"));
        assert!(output.rendered.contains("SKIP    NX002"));
    }

    #[test]
    fn snapshot_run_with_mixed_repos_returns_failing_exit_code() {
        let output = run(CliArgs {
            config_path: fixture_path("tests/fixtures/repos.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
            format: OutputFormat::Text,
            execution_mode: ExecutionMode::Plan,
        })
        .unwrap();

        assert_eq!(output.exit_code(), ExitCode::from(1));
        assert!(output.rendered.contains("Repository: example-org/bad-repo"));
        assert!(output.rendered.contains("FAIL    WF003"));
    }

    #[test]
    fn snapshot_run_renders_json_reports() {
        let output = run(CliArgs {
            config_path: fixture_path("tests/fixtures/repos.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
            format: OutputFormat::Json,
            execution_mode: ExecutionMode::Plan,
        })
        .unwrap();

        let decoded: Vec<RepoReport> = serde_json::from_str(&output.rendered).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].repo, RepoRef::new("example-org", "good-repo"));
        assert_eq!(decoded[1].repo, RepoRef::new("example-org", "bad-repo"));
        assert!(
            decoded[0]
                .rules
                .iter()
                .any(|rule| rule.id.to_string() == "RS001"
                    && matches!(rule.result, crate::rules::RuleResult::Pass))
        );
    }

    #[test]
    fn json_report_top_level_is_vec_repo_report() {
        let output = run(CliArgs {
            config_path: fixture_path("tests/fixtures/good-repo.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
            format: OutputFormat::Json,
            execution_mode: ExecutionMode::Plan,
        })
        .unwrap();

        // Validate the top-level JSON structure directly: an array of objects
        // each containing "repo" and "rules" keys.
        let raw: serde_json::Value = serde_json::from_str(&output.rendered).unwrap();
        let array = raw.as_array().expect("top-level JSON should be an array");
        assert!(!array.is_empty());

        for entry in array {
            let obj = entry.as_object().expect("each entry should be an object");
            assert!(obj.contains_key("repo"), "entry missing 'repo' key");
            assert!(obj.contains_key("rules"), "entry missing 'rules' key");
            assert!(obj.contains_key("fixes"), "entry missing 'fixes' key");
            let rules = obj["rules"].as_array().expect("'rules' should be an array");
            for rule in rules {
                let rule_obj = rule.as_object().expect("each rule should be an object");
                assert!(rule_obj.contains_key("id"), "rule missing 'id' key");
                assert!(rule_obj.contains_key("name"), "rule missing 'name' key");
                assert!(rule_obj.contains_key("result"), "rule missing 'result' key");
            }
        }

        // Also confirm it round-trips through the typed schema.
        let _decoded: Vec<RepoReport> = serde_json::from_str(&output.rendered).unwrap();
    }

    #[test]
    fn snapshot_plan_run_lists_planned_fixes_for_fixable_failures() {
        let output = run(CliArgs {
            config_path: fixture_path("tests/fixtures/repos.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
            format: OutputFormat::Text,
            execution_mode: ExecutionMode::Plan,
        })
        .unwrap();

        assert!(output.rendered.contains("Fixes:"));
        assert!(output.rendered.contains("PLANNED  ST001"));
        assert!(output.rendered.contains("PLANNED  ST006"));
    }
}
