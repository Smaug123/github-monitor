use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::github::client::{GitHubClient, GitHubClientError, NonRootRepoPath, RepoPathError};
use crate::github::types::{ContentEncoding, GitTreeEntryType, Repository, Ruleset};
use crate::types::{BranchName, RepoRef};
use crate::workflow::model::Workflow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSettings {
    pub private: bool,
    pub archived: bool,
    pub disabled: bool,
    pub allow_auto_merge: bool,
    pub delete_branch_on_merge: bool,
    pub allow_update_branch: bool,
    pub allow_squash_merge: bool,
    pub allow_merge_commit: bool,
    pub allow_rebase_merge: bool,
}

impl From<&Repository> for RepoSettings {
    fn from(repository: &Repository) -> Self {
        Self {
            private: repository.private,
            archived: repository.archived,
            disabled: repository.disabled,
            allow_auto_merge: repository.allow_auto_merge,
            delete_branch_on_merge: repository.delete_branch_on_merge,
            allow_update_branch: repository.allow_update_branch,
            allow_squash_merge: repository.allow_squash_merge,
            allow_merge_commit: repository.allow_merge_commit,
            allow_rebase_merge: repository.allow_rebase_merge,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowFile {
    pub path: String,
    pub workflow: Workflow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoFacts {
    pub repo: RepoRef,
    pub settings: RepoSettings,
    pub rulesets: Vec<Ruleset>,
    pub default_branch: BranchName,
    pub workflows: Vec<WorkflowFile>,
    pub files_present: BTreeSet<String>,
}

pub fn gather_repo_facts(
    client: &mut GitHubClient,
    repo: RepoRef,
) -> Result<RepoFacts, FactsError> {
    let repository = client.get_repo(&repo)?;
    let default_branch = repository.default_branch.clone();
    let settings = RepoSettings::from(&repository);
    let rulesets = fetch_rulesets(client, &repo)?;
    let tree = client.get_git_tree(&repo, &default_branch.to_string())?;

    if tree.truncated {
        return Err(FactsError::TruncatedGitTree {
            repo,
            reference: default_branch.to_string(),
        });
    }

    let files_present = tree
        .tree
        .iter()
        .filter(|entry| entry.kind != GitTreeEntryType::Tree)
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let workflows = fetch_workflows(client, &repo, &tree.tree)?;

    Ok(RepoFacts {
        repo,
        settings,
        rulesets,
        default_branch,
        workflows,
        files_present,
    })
}

pub fn save_snapshot(snapshot_dir: &Path, facts: &RepoFacts) -> Result<PathBuf, SnapshotError> {
    let path = snapshot_path(snapshot_dir, &facts.repo);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SnapshotError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let json = serde_json::to_vec_pretty(facts).map_err(|source| SnapshotError::Serialize {
        repo: facts.repo.clone(),
        source,
    })?;
    fs::write(&path, json).map_err(|source| SnapshotError::Io {
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

pub fn load_snapshot(snapshot_dir: &Path, repo: &RepoRef) -> Result<RepoFacts, SnapshotError> {
    let path = snapshot_path(snapshot_dir, repo);
    let raw = fs::read_to_string(&path).map_err(|source| SnapshotError::Io {
        path: path.clone(),
        source,
    })?;
    let facts: RepoFacts =
        serde_json::from_str(&raw).map_err(|source| SnapshotError::Deserialize {
            path: path.clone(),
            source,
        })?;

    if &facts.repo != repo {
        return Err(SnapshotError::RepoMismatch {
            path,
            expected: repo.clone(),
            actual: facts.repo,
        });
    }

    Ok(facts)
}

pub fn snapshot_path(snapshot_dir: &Path, repo: &RepoRef) -> PathBuf {
    snapshot_dir
        .join(repo.owner.to_string())
        .join(format!("{}.json", repo.name))
}

fn fetch_rulesets(client: &mut GitHubClient, repo: &RepoRef) -> Result<Vec<Ruleset>, FactsError> {
    let listed_rulesets = client.list_rulesets(repo)?;
    let mut rulesets = Vec::with_capacity(listed_rulesets.len());

    for ruleset in listed_rulesets {
        rulesets.push(client.get_ruleset(repo, ruleset.id)?);
    }

    rulesets.sort_by_key(|ruleset| ruleset.id);
    Ok(rulesets)
}

fn fetch_workflows(
    client: &mut GitHubClient,
    repo: &RepoRef,
    tree_entries: &[crate::github::types::GitTreeEntry],
) -> Result<Vec<WorkflowFile>, FactsError> {
    let mut workflow_paths = tree_entries
        .iter()
        .filter(|entry| entry.kind == GitTreeEntryType::Blob && is_workflow_path(&entry.path))
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    workflow_paths.sort();

    let mut workflows = Vec::with_capacity(workflow_paths.len());

    for path in workflow_paths {
        let repo_path = NonRootRepoPath::new(&path).map_err(|source| FactsError::InvalidPath {
            path: path.clone(),
            source,
        })?;
        let file = client.get_file_contents(repo, &repo_path)?;
        let yaml = decode_repository_text_file(&file.path, &file.encoding, &file.content)?;
        let workflow = serde_yml::from_str(&yaml).map_err(|source| FactsError::WorkflowParse {
            path: file.path.clone(),
            source,
        })?;

        workflows.push(WorkflowFile {
            path: file.path,
            workflow,
        });
    }

    Ok(workflows)
}

fn is_workflow_path(path: &str) -> bool {
    path.starts_with(".github/workflows/")
        && matches!(
            Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("yml" | "yaml")
        )
}

fn decode_repository_text_file(
    path: &str,
    encoding: &ContentEncoding,
    content: &str,
) -> Result<String, FactsError> {
    match encoding {
        ContentEncoding::Base64 => {
            let compact = content
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|source| FactsError::Base64Decode {
                    path: path.to_owned(),
                    source,
                })?;

            String::from_utf8(bytes).map_err(|source| FactsError::Utf8Decode {
                path: path.to_owned(),
                source,
            })
        }
        ContentEncoding::Utf8 => Ok(content.to_owned()),
        ContentEncoding::Unknown(encoding) => Err(FactsError::UnsupportedEncoding {
            path: path.to_owned(),
            encoding: encoding.clone(),
        }),
    }
}

#[derive(Debug)]
pub enum FactsError {
    GitHub(GitHubClientError),
    InvalidPath {
        path: String,
        source: RepoPathError,
    },
    WorkflowParse {
        path: String,
        source: serde_yml::Error,
    },
    Base64Decode {
        path: String,
        source: base64::DecodeError,
    },
    Utf8Decode {
        path: String,
        source: std::string::FromUtf8Error,
    },
    UnsupportedEncoding {
        path: String,
        encoding: String,
    },
    TruncatedGitTree {
        repo: RepoRef,
        reference: String,
    },
}

impl From<GitHubClientError> for FactsError {
    fn from(source: GitHubClientError) -> Self {
        Self::GitHub(source)
    }
}

impl std::fmt::Display for FactsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitHub(source) => source.fmt(f),
            Self::InvalidPath { path, source } => {
                write!(f, "invalid repository path {path}: {source}")
            }
            Self::WorkflowParse { path, source } => {
                write!(f, "failed to parse workflow {path}: {source}")
            }
            Self::Base64Decode { path, source } => {
                write!(f, "failed to decode base64 file {path}: {source}")
            }
            Self::Utf8Decode { path, source } => {
                write!(f, "failed to decode utf-8 file {path}: {source}")
            }
            Self::UnsupportedEncoding { path, encoding } => {
                write!(f, "unsupported encoding {encoding} for file {path}")
            }
            Self::TruncatedGitTree { repo, reference } => {
                write!(
                    f,
                    "git tree for {repo} at reference {reference} was truncated; refusing incomplete facts"
                )
            }
        }
    }
}

impl std::error::Error for FactsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::GitHub(source) => Some(source),
            Self::InvalidPath { source, .. } => Some(source),
            Self::WorkflowParse { source, .. } => Some(source),
            Self::Base64Decode { source, .. } => Some(source),
            Self::Utf8Decode { source, .. } => Some(source),
            Self::UnsupportedEncoding { .. } | Self::TruncatedGitTree { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum SnapshotError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Serialize {
        repo: RepoRef,
        source: serde_json::Error,
    },
    Deserialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    RepoMismatch {
        path: PathBuf,
        expected: RepoRef,
        actual: RepoRef,
    },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "snapshot I/O failed at {}: {source}", path.display())
            }
            Self::Serialize { repo, source } => {
                write!(f, "failed to serialize snapshot for {repo}: {source}")
            }
            Self::Deserialize { path, source } => {
                write!(
                    f,
                    "failed to deserialize snapshot {}: {source}",
                    path.display()
                )
            }
            Self::RepoMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "snapshot {} contained repo {actual}, expected {expected}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for SnapshotError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize { source, .. } => Some(source),
            Self::Deserialize { source, .. } => Some(source),
            Self::RepoMismatch { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::types::{
        BypassActor, BypassActorType, BypassMode, RequiredStatusCheck, Ruleset, RulesetEnforcement,
        RulesetRule, RulesetRuleParameters, RulesetRuleType, RulesetTarget,
    };
    use crate::workflow::model::{
        ActionRef, ActionReference, ActionStep, Job, RunStep, Step, StepKind, TriggerFilter,
        Triggers, WithValue, Workflow,
    };
    use proptest::prelude::*;
    use std::collections::BTreeMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn identifier() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{0,12}"
    }

    fn text() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9 _-]{0,20}"
    }

    fn repo_settings_strategy() -> impl Strategy<Value = RepoSettings> {
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(
                    private,
                    archived,
                    disabled,
                    allow_auto_merge,
                    delete_branch_on_merge,
                    allow_update_branch,
                    allow_squash_merge,
                    allow_merge_commit,
                    allow_rebase_merge,
                )| RepoSettings {
                    private,
                    archived,
                    disabled,
                    allow_auto_merge,
                    delete_branch_on_merge,
                    allow_update_branch,
                    allow_squash_merge,
                    allow_merge_commit,
                    allow_rebase_merge,
                },
            )
    }

    fn ruleset_target_strategy() -> impl Strategy<Value = RulesetTarget> {
        prop_oneof![
            Just(RulesetTarget::Branch),
            Just(RulesetTarget::Tag),
            Just(RulesetTarget::Push),
        ]
    }

    fn ruleset_enforcement_strategy() -> impl Strategy<Value = RulesetEnforcement> {
        prop_oneof![
            Just(RulesetEnforcement::Active),
            Just(RulesetEnforcement::Evaluate),
            Just(RulesetEnforcement::Disabled),
        ]
    }

    fn bypass_actor_type_strategy() -> impl Strategy<Value = BypassActorType> {
        prop_oneof![
            Just(BypassActorType::OrganizationAdmin),
            Just(BypassActorType::RepositoryRole),
            Just(BypassActorType::Team),
            Just(BypassActorType::Integration),
            Just(BypassActorType::DeployKey),
        ]
    }

    fn bypass_mode_strategy() -> impl Strategy<Value = BypassMode> {
        prop_oneof![Just(BypassMode::Always), Just(BypassMode::PullRequest),]
    }

    fn bypass_actor_strategy() -> impl Strategy<Value = BypassActor> {
        (
            proptest::option::of(any::<u64>()),
            bypass_actor_type_strategy(),
            bypass_mode_strategy(),
        )
            .prop_map(|(actor_id, actor_type, bypass_mode)| BypassActor {
                actor_id,
                actor_type,
                bypass_mode,
            })
    }

    fn ruleset_rule_type_strategy() -> impl Strategy<Value = RulesetRuleType> {
        prop_oneof![
            Just(RulesetRuleType::Creation),
            Just(RulesetRuleType::Update),
            Just(RulesetRuleType::Deletion),
            Just(RulesetRuleType::RequiredLinearHistory),
            Just(RulesetRuleType::RequiredSignatures),
            Just(RulesetRuleType::PullRequest),
            Just(RulesetRuleType::RequiredStatusChecks),
            Just(RulesetRuleType::NonFastForward),
        ]
    }

    fn required_status_check_strategy() -> impl Strategy<Value = RequiredStatusCheck> {
        (text(), proptest::option::of(any::<u64>())).prop_map(|(context, integration_id)| {
            RequiredStatusCheck {
                context,
                integration_id,
            }
        })
    }

    fn ruleset_rule_parameters_strategy() -> impl Strategy<Value = RulesetRuleParameters> {
        (
            proptest::collection::vec(required_status_check_strategy(), 0..3),
            proptest::option::of(any::<bool>()),
            proptest::option::of(0_u32..5),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
        )
            .prop_map(
                |(
                    required_status_checks,
                    strict_required_status_checks_policy,
                    required_approving_review_count,
                    require_code_owner_review,
                    require_last_push_approval,
                    required_review_thread_resolution,
                    dismiss_stale_reviews_on_push,
                    do_not_enforce_on_create,
                )| RulesetRuleParameters {
                    required_status_checks,
                    strict_required_status_checks_policy,
                    required_approving_review_count,
                    require_code_owner_review,
                    require_last_push_approval,
                    required_review_thread_resolution,
                    dismiss_stale_reviews_on_push,
                    do_not_enforce_on_create,
                },
            )
    }

    fn ruleset_rule_strategy() -> impl Strategy<Value = RulesetRule> {
        (
            ruleset_rule_type_strategy(),
            proptest::option::of(ruleset_rule_parameters_strategy()),
        )
            .prop_map(|(kind, parameters)| RulesetRule { kind, parameters })
    }

    fn ruleset_strategy() -> impl Strategy<Value = Ruleset> {
        (
            any::<u64>(),
            text(),
            ruleset_target_strategy(),
            ruleset_enforcement_strategy(),
            proptest::collection::vec(bypass_actor_strategy(), 0..2),
            proptest::collection::vec(ruleset_rule_strategy(), 0..4),
        )
            .prop_map(
                |(id, name, target, enforcement, bypass_actors, rules)| Ruleset {
                    id,
                    name,
                    target,
                    enforcement,
                    bypass_actors,
                    rules,
                },
            )
    }

    fn trigger_filter_strategy() -> impl Strategy<Value = TriggerFilter> {
        (
            proptest::collection::vec(identifier(), 0..3),
            proptest::collection::vec(identifier(), 0..3),
            proptest::collection::vec(identifier(), 0..3),
        )
            .prop_map(|(branches, branches_ignore, paths)| TriggerFilter {
                branches,
                branches_ignore,
                paths,
            })
    }

    fn with_value_strategy() -> impl Strategy<Value = WithValue> {
        prop_oneof![
            text().prop_map(WithValue::String),
            any::<bool>().prop_map(WithValue::Bool),
            any::<i32>().prop_map(|value| WithValue::Integer(i64::from(value))),
        ]
    }

    fn step_strategy() -> impl Strategy<Value = Step> {
        let action_step = (
            proptest::option::of(text()),
            proptest::option::of(identifier()),
            proptest::option::of(text()),
            identifier(),
            identifier(),
            text(),
            proptest::collection::btree_map(identifier(), with_value_strategy(), 0..3),
        )
            .prop_map(|(name, id, condition, owner, repo, version, with)| Step {
                name,
                id,
                condition,
                kind: StepKind::Action(ActionStep {
                    uses: ActionReference::Repository(ActionRef::new(owner, repo, version)),
                    with,
                }),
            });

        let run_step = (
            proptest::option::of(text()),
            proptest::option::of(identifier()),
            proptest::option::of(text()),
            ".{1,30}",
        )
            .prop_map(|(name, id, condition, run)| Step {
                name,
                id,
                condition,
                kind: StepKind::Run(RunStep { run }),
            });

        prop_oneof![action_step, run_step]
    }

    fn workflow_strategy() -> impl Strategy<Value = Workflow> {
        (
            proptest::option::of(text()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            any::<bool>(),
            proptest::collection::btree_map(
                identifier(),
                (
                    proptest::collection::vec(step_strategy(), 0..4),
                    proptest::collection::vec(identifier(), 0..3),
                    proptest::option::of(text()),
                )
                    .prop_map(|(steps, needs, condition)| Job {
                        runs_on: None,
                        steps,
                        needs,
                        condition,
                    }),
                0..3,
            ),
        )
            .prop_map(
                |(name, push, pull_request, pull_request_target, workflow_dispatch, jobs)| {
                    Workflow {
                        name,
                        triggers: Triggers {
                            push,
                            pull_request,
                            pull_request_target,
                            workflow_dispatch: workflow_dispatch.then_some(Default::default()),
                        },
                        jobs,
                    }
                },
            )
    }

    fn workflow_file_strategy() -> impl Strategy<Value = WorkflowFile> {
        (identifier(), workflow_strategy()).prop_map(|(name, workflow)| WorkflowFile {
            path: format!(".github/workflows/{name}.yml"),
            workflow,
        })
    }

    fn repo_facts_strategy() -> impl Strategy<Value = RepoFacts> {
        (
            identifier(),
            identifier(),
            repo_settings_strategy(),
            proptest::collection::vec(ruleset_strategy(), 0..3),
            identifier(),
            proptest::collection::vec(workflow_file_strategy(), 0..3),
            proptest::collection::btree_set("[./A-Za-z0-9_-]{1,40}", 0..10),
        )
            .prop_map(
                |(owner, name, settings, rulesets, branch, workflows, files_present)| RepoFacts {
                    repo: RepoRef::new(owner, name),
                    settings,
                    rulesets,
                    default_branch: BranchName::new(branch),
                    workflows,
                    files_present,
                },
            )
    }

    fn sample_repo_facts() -> RepoFacts {
        let mut jobs = BTreeMap::new();
        jobs.insert(
            "build".to_owned(),
            Job {
                runs_on: None,
                steps: vec![
                    Step {
                        name: Some("Checkout".to_owned()),
                        id: None,
                        condition: None,
                        kind: StepKind::Action(ActionStep {
                            uses: ActionReference::Repository(ActionRef::new(
                                "actions", "checkout", "f00ba4",
                            )),
                            with: BTreeMap::new(),
                        }),
                    },
                    Step {
                        name: Some("Test".to_owned()),
                        id: None,
                        condition: None,
                        kind: StepKind::Run(RunStep {
                            run: "cargo test".to_owned(),
                        }),
                    },
                ],
                needs: Vec::new(),
                condition: None,
            },
        );

        RepoFacts {
            repo: RepoRef::new("example-org", "snapshot-roundtrip"),
            settings: RepoSettings {
                private: false,
                archived: false,
                disabled: false,
                allow_auto_merge: true,
                delete_branch_on_merge: true,
                allow_update_branch: true,
                allow_squash_merge: true,
                allow_merge_commit: false,
                allow_rebase_merge: true,
            },
            rulesets: vec![Ruleset {
                id: 1,
                name: "main protection".to_owned(),
                target: RulesetTarget::Branch,
                enforcement: RulesetEnforcement::Active,
                bypass_actors: Vec::new(),
                rules: vec![RulesetRule {
                    kind: RulesetRuleType::RequiredStatusChecks,
                    parameters: Some(RulesetRuleParameters {
                        required_status_checks: vec![RequiredStatusCheck {
                            context: "ci".to_owned(),
                            integration_id: None,
                        }],
                        strict_required_status_checks_policy: Some(true),
                        required_approving_review_count: None,
                        require_code_owner_review: None,
                        require_last_push_approval: None,
                        required_review_thread_resolution: None,
                        dismiss_stale_reviews_on_push: None,
                        do_not_enforce_on_create: None,
                    }),
                }],
            }],
            default_branch: BranchName::new("main"),
            workflows: vec![WorkflowFile {
                path: ".github/workflows/ci.yml".to_owned(),
                workflow: Workflow {
                    name: Some("CI".to_owned()),
                    triggers: Triggers {
                        push: Some(TriggerFilter {
                            branches: vec!["main".to_owned()],
                            branches_ignore: Vec::new(),
                            paths: Vec::new(),
                        }),
                        pull_request: Some(TriggerFilter {
                            branches: vec!["main".to_owned()],
                            branches_ignore: Vec::new(),
                            paths: Vec::new(),
                        }),
                        pull_request_target: None,
                        workflow_dispatch: None,
                    },
                    jobs,
                },
            }],
            files_present: BTreeSet::from([
                ".github/workflows/ci.yml".to_owned(),
                "flake.nix".to_owned(),
                "flake.lock".to_owned(),
                "CODEOWNERS".to_owned(),
            ]),
        }
    }

    fn unique_temp_dir() -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "github-infra-facts-test-{}-{timestamp}",
            std::process::id()
        ))
    }

    proptest! {
        #[test]
        fn repo_facts_json_roundtrip(facts in repo_facts_strategy()) {
            let json = serde_json::to_string(&facts).unwrap();
            let deserialized: RepoFacts = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(deserialized, facts);
        }
    }

    #[test]
    fn snapshot_save_then_load_preserves_facts() {
        let snapshot_dir = unique_temp_dir();
        let facts = sample_repo_facts();

        let saved_path = save_snapshot(&snapshot_dir, &facts).unwrap();
        let loaded = load_snapshot(&snapshot_dir, &facts.repo).unwrap();

        assert_eq!(loaded, facts);
        assert_eq!(saved_path, snapshot_path(&snapshot_dir, &facts.repo));

        fs::remove_dir_all(snapshot_dir).unwrap();
    }

    #[test]
    #[ignore = "requires GITHUB_TOKEN and network access"]
    fn gathers_public_repo_facts() {
        let token = crate::github::client::GitHubToken::from_env("GITHUB_TOKEN")
            .expect("GITHUB_TOKEN must be set");
        let mut client = GitHubClient::new(token);
        let facts = gather_repo_facts(&mut client, RepoRef::new("rust-lang", "cargo")).unwrap();

        assert!(!facts.default_branch.to_string().is_empty());
        assert!(!facts.workflows.is_empty());
    }
}
