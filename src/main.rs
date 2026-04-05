mod config;
mod facts;
mod github;
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
use crate::types::RepoRef;

const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";

fn github_token_from_env() -> Option<GitHubToken> {
    GitHubToken::from_env(GITHUB_TOKEN_ENV)
}

fn main() -> ExitCode {
    match try_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            error.exit_code()
        }
    }
}

fn try_main() -> Result<(), MainError> {
    let args = parse_cli_args(std::env::args().skip(1)).map_err(MainError::Cli)?;
    let _facts = run(args).map_err(MainError::App)?;
    Ok(())
}

fn run(args: CliArgs) -> Result<Vec<RepoFacts>, AppError> {
    let config = Config::from_path(&args.config_path)?;
    let repos = config.repo_refs();

    match args.snapshot_mode {
        SnapshotMode::Load(snapshot_dir) => repos
            .iter()
            .map(|repo| load_snapshot(&snapshot_dir, repo).map_err(AppError::from))
            .collect(),
        SnapshotMode::Save(snapshot_dir) => {
            let facts = gather_facts_from_github(&repos)?;
            for repo_facts in &facts {
                save_snapshot(&snapshot_dir, repo_facts)?;
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
    let mut facts = Vec::with_capacity(repos.len());

    for repo in repos {
        let repo_facts =
            gather_repo_facts(&mut client, repo.clone()).map_err(|source| AppError::Facts {
                repo: repo.clone(),
                source: Box::new(source),
            })?;
        facts.push(repo_facts);
    }

    Ok(facts)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    config_path: PathBuf,
    snapshot_mode: SnapshotMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SnapshotMode {
    None,
    Save(PathBuf),
    Load(PathBuf),
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

    Ok(CliArgs {
        config_path,
        snapshot_mode,
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
            Self::UnknownArgument(arg) => write!(f, "unknown argument {arg}"),
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
            }
        );
    }

    #[test]
    fn snapshot_load_reads_committed_fixtures() {
        let facts = run(CliArgs {
            config_path: fixture_path("tests/fixtures/repos.toml"),
            snapshot_mode: SnapshotMode::Load(fixture_path("tests/fixtures")),
        })
        .unwrap();

        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].repo, RepoRef::new("example-org", "good-repo"));
        assert_eq!(facts[1].repo, RepoRef::new("example-org", "bad-repo"));
    }
}
