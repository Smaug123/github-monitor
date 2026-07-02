use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::github::client::{GitHubClient, GitHubClientError, NonRootRepoPath, RepoPathError};
use crate::github::types::{
    BranchProtection, ContentEncoding, DefaultWorkflowPermissions, ForkPrApprovalPolicy,
    GitTreeEntryType, Repository, Ruleset,
};
use crate::types::{BranchName, Gathered, RepoRef};
use crate::workflow::model::Workflow;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSettings {
    pub private: bool,
    pub archived: bool,
    pub disabled: bool,
    /// Merge-policy flags are `None` when GitHub did not report them (the token
    /// lacks permission to read them). Rules treat an unknown flag as `Error`,
    /// never a vacuous pass. A snapshot storing an explicit `true`/`false` loads
    /// as `Some(_)`, so existing snapshots remain valid.
    pub allow_auto_merge: Option<bool>,
    pub delete_branch_on_merge: Option<bool>,
    pub allow_update_branch: Option<bool>,
    pub allow_squash_merge: Option<bool>,
    pub allow_merge_commit: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
    /// `Absent` means the fork-PR approval policy was gathered and found unset — a
    /// known state (ST007 fails on it), distinct from a snapshot that never
    /// recorded the field (which fails to load). See [`Gathered`].
    pub fork_pr_approval_policy: Gathered<ForkPrApprovalPolicy>,
    pub default_workflow_permissions: DefaultWorkflowPermissions,
}

impl RepoSettings {
    pub fn new(
        repository: &Repository,
        fork_pr_approval_policy: Gathered<ForkPrApprovalPolicy>,
        default_workflow_permissions: DefaultWorkflowPermissions,
    ) -> Self {
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
            fork_pr_approval_policy,
            default_workflow_permissions,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowFile {
    pub path: String,
    pub workflow: Workflow,
    /// The raw source of the workflow file. The workflow-pin rewriter operates
    /// on this text and needs it to verify whether each `uses:` it would change
    /// is actually findable via its block-style regex; storing it lets the
    /// planner reject inline-flow YAML before the rewrite runs. `None` means
    /// the source is unavailable (e.g. legacy snapshot) and the planner falls
    /// back to trusting the AST.
    #[serde(default)]
    pub raw_yaml: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoFacts {
    pub repo: RepoRef,
    pub settings: RepoSettings,
    pub rulesets: Vec<Ruleset>,
    /// `Absent` means branch protection was queried and found absent (the endpoint
    /// 404s for an unprotected branch) — a known state RS007 passes on. A snapshot
    /// that omits the field fails to load rather than masquerading as `Absent`.
    pub legacy_branch_protection: Gathered<BranchProtection>,
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
    let fork_pr_approval_policy = client.get_fork_pr_approval_permission(&repo)?;
    let default_workflow_permissions = client
        .get_workflow_permissions(&repo)?
        .default_workflow_permissions;
    let settings = RepoSettings::new(
        &repository,
        fork_pr_approval_policy,
        default_workflow_permissions,
    );
    let rulesets = fetch_rulesets(client, &repo)?;
    let legacy_branch_protection =
        Gathered::from_option(client.get_branch_protection(&repo, &default_branch)?);
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
        legacy_branch_protection,
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
            raw_yaml: Some(yaml),
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
        BranchProtection, BypassActor, BypassActorType, BypassMode, MergeMethod, RefNameCondition,
        RequiredStatusCheck, Ruleset, RulesetConditions, RulesetEnforcement, RulesetRule,
        RulesetRuleParameters, RulesetRuleType, RulesetTarget,
    };
    use crate::workflow::model::{
        ActionRef, ActionReference, ActionStep, Job, JobKind, RunStep, StandardJob, Step, StepKind,
        TriggerFilter, Triggers, WithValue, Workflow,
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

    fn fork_pr_approval_policy_strategy() -> impl Strategy<Value = Gathered<ForkPrApprovalPolicy>> {
        prop_oneof![
            Just(Gathered::Absent),
            Just(Gathered::Unknown),
            Just(Gathered::Present(ForkPrApprovalPolicy::AllExternalContributors)),
            Just(Gathered::Present(
                ForkPrApprovalPolicy::FirstTimeContributorsNewToGithub
            )),
            Just(Gathered::Present(ForkPrApprovalPolicy::FirstTimeContributors)),
            "[a-z][a-z0-9_]{0,16}"
                .prop_map(|value| Gathered::Present(ForkPrApprovalPolicy::Unknown(value))),
        ]
    }

    fn default_workflow_permissions_strategy() -> impl Strategy<Value = DefaultWorkflowPermissions>
    {
        prop_oneof![
            Just(DefaultWorkflowPermissions::Read),
            Just(DefaultWorkflowPermissions::Write),
            "[a-z][a-z0-9_]{0,16}".prop_map(DefaultWorkflowPermissions::Unknown),
        ]
    }

    fn repo_settings_strategy() -> impl Strategy<Value = RepoSettings> {
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            fork_pr_approval_policy_strategy(),
            default_workflow_permissions_strategy(),
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
                    fork_pr_approval_policy,
                    default_workflow_permissions,
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
                    fork_pr_approval_policy,
                    default_workflow_permissions,
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

    fn merge_method_strategy() -> impl Strategy<Value = MergeMethod> {
        prop_oneof![
            Just(MergeMethod::Merge),
            Just(MergeMethod::Squash),
            Just(MergeMethod::Rebase),
        ]
    }

    /// Unmodeled parameter keys GitHub may attach to a rule. Keys are synthetic and
    /// disjoint from the modeled fields; values are limited to strings/bools so JSON
    /// round-trip equality is exact.
    fn ruleset_extra_parameters_strategy(
    ) -> impl Strategy<Value = serde_json::Map<String, serde_json::Value>> {
        proptest::collection::btree_map(
            "x_extra_[a-z]{1,8}",
            prop_oneof![
                text().prop_map(serde_json::Value::String),
                any::<bool>().prop_map(serde_json::Value::Bool),
            ],
            0..3,
        )
        .prop_map(|map| map.into_iter().collect())
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
            proptest::collection::vec(merge_method_strategy(), 0..4),
            ruleset_extra_parameters_strategy(),
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
                    allowed_merge_methods,
                    extra,
                )| RulesetRuleParameters {
                    required_status_checks,
                    strict_required_status_checks_policy,
                    required_approving_review_count,
                    require_code_owner_review,
                    require_last_push_approval,
                    required_review_thread_resolution,
                    dismiss_stale_reviews_on_push,
                    do_not_enforce_on_create,
                    allowed_merge_methods,
                    extra,
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

    fn ref_name_condition_strategy() -> impl Strategy<Value = RefNameCondition> {
        (
            proptest::collection::vec(
                prop_oneof![
                    Just("~DEFAULT_BRANCH".to_owned()),
                    Just("~ALL".to_owned()),
                    identifier(),
                ],
                0..3,
            ),
            proptest::collection::vec(identifier(), 0..2),
        )
            .prop_map(|(include, exclude)| RefNameCondition { include, exclude })
    }

    fn ruleset_conditions_strategy() -> impl Strategy<Value = Option<RulesetConditions>> {
        proptest::option::of(
            proptest::option::of(ref_name_condition_strategy())
                .prop_map(|ref_name| RulesetConditions { ref_name }),
        )
    }

    fn ruleset_strategy() -> impl Strategy<Value = Ruleset> {
        (
            any::<u64>(),
            text(),
            ruleset_target_strategy(),
            ruleset_enforcement_strategy(),
            ruleset_conditions_strategy(),
            proptest::collection::vec(bypass_actor_strategy(), 0..2),
            proptest::collection::vec(ruleset_rule_strategy(), 0..4),
        )
            .prop_map(
                |(id, name, target, enforcement, conditions, bypass_actors, rules)| Ruleset {
                    id,
                    name,
                    target,
                    enforcement,
                    conditions,
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
            proptest::collection::vec(identifier(), 0..3),
            proptest::collection::vec(identifier(), 0..3),
        )
            .prop_map(|(branches, branches_ignore, tags, tags_ignore, paths)| {
                TriggerFilter {
                    branches,
                    branches_ignore,
                    tags,
                    tags_ignore,
                    paths,
                }
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
            any::<bool>(),
            proptest::collection::btree_map(
                identifier(),
                (
                    proptest::collection::vec(step_strategy(), 0..4),
                    proptest::collection::vec(identifier(), 0..3),
                    proptest::option::of(text()),
                )
                    .prop_map(|(steps, needs, condition)| Job {
                        needs,
                        condition,
                        kind: JobKind::Standard(StandardJob {
                            runs_on: None,
                            steps,
                        }),
                    }),
                0..3,
            ),
        )
            .prop_map(
                |(
                    name,
                    push,
                    pull_request,
                    pull_request_target,
                    workflow_run,
                    workflow_dispatch,
                    jobs,
                )| {
                    Workflow {
                        name,
                        triggers: Triggers {
                            push,
                            pull_request,
                            pull_request_target,
                            workflow_run: workflow_run.then_some(Default::default()),
                            workflow_dispatch: workflow_dispatch.then_some(Default::default()),
                        },
                        jobs,
                    }
                },
            )
    }

    fn workflow_file_strategy() -> impl Strategy<Value = WorkflowFile> {
        (
            identifier(),
            workflow_strategy(),
            proptest::option::of(any::<String>()),
        )
            .prop_map(|(name, workflow, raw_yaml)| WorkflowFile {
                path: format!(".github/workflows/{name}.yml"),
                workflow,
                raw_yaml,
            })
    }

    fn repo_facts_strategy() -> impl Strategy<Value = RepoFacts> {
        (
            identifier(),
            identifier(),
            repo_settings_strategy(),
            proptest::collection::vec(ruleset_strategy(), 0..3),
            prop_oneof![
                Just(Gathered::Absent),
                Just(Gathered::Unknown),
                Just(Gathered::Present(BranchProtection::default())),
            ],
            identifier(),
            proptest::collection::vec(workflow_file_strategy(), 0..3),
            proptest::collection::btree_set("[./A-Za-z0-9_-]{1,40}", 0..10),
        )
            .prop_map(
                |(
                    owner,
                    name,
                    settings,
                    rulesets,
                    legacy_branch_protection,
                    branch,
                    workflows,
                    files_present,
                )| RepoFacts {
                    repo: RepoRef::new(owner, name),
                    settings,
                    rulesets,
                    legacy_branch_protection,
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
                needs: Vec::new(),
                condition: None,
                kind: JobKind::Standard(StandardJob {
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
                }),
            },
        );

        RepoFacts {
            repo: RepoRef::new("example-org", "snapshot-roundtrip"),
            settings: RepoSettings {
                private: false,
                archived: false,
                disabled: false,
                allow_auto_merge: Some(true),
                delete_branch_on_merge: Some(true),
                allow_update_branch: Some(true),
                allow_squash_merge: Some(true),
                allow_merge_commit: Some(false),
                allow_rebase_merge: Some(false),
                fork_pr_approval_policy: Gathered::Present(
                    ForkPrApprovalPolicy::AllExternalContributors,
                ),
                default_workflow_permissions: DefaultWorkflowPermissions::Read,
            },
            legacy_branch_protection: Gathered::Absent,
            rulesets: vec![Ruleset {
                id: 1,
                name: "main protection".to_owned(),
                target: RulesetTarget::Branch,
                enforcement: RulesetEnforcement::Active,
                conditions: Some(RulesetConditions {
                    ref_name: Some(RefNameCondition {
                        include: vec!["~DEFAULT_BRANCH".to_owned()],
                        exclude: Vec::new(),
                    }),
                }),
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
                        allowed_merge_methods: Vec::new(),
                        extra: serde_json::Map::new(),
                    }),
                }],
            }],
            default_branch: BranchName::new("main"),
            workflows: vec![WorkflowFile {
                path: ".github/workflows/ci.yml".to_owned(),
                raw_yaml: None,
                workflow: Workflow {
                    name: Some("CI".to_owned()),
                    triggers: Triggers {
                        push: Some(TriggerFilter {
                            branches: vec!["main".to_owned()],
                            branches_ignore: Vec::new(),
                            tags: Vec::new(),
                            tags_ignore: Vec::new(),
                            paths: Vec::new(),
                        }),
                        pull_request: Some(TriggerFilter {
                            branches: vec!["main".to_owned()],
                            branches_ignore: Vec::new(),
                            tags: Vec::new(),
                            tags_ignore: Vec::new(),
                            paths: Vec::new(),
                        }),
                        pull_request_target: None,
                        workflow_run: None,
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

    proptest! {
        /// Finding 2, generalized: the ruleset write body (`UpdateRulesetRequest`) is a
        /// full replacement, so every parameter GitHub sends — including rule types and
        /// fields the model doesn't understand — must be echoed back verbatim, or a fix
        /// silently resets it (or 422s, for required parameters).
        ///
        /// We take a generated ruleset, serialize it the way GitHub would return it,
        /// then decorate every rule with an unmodeled parameter and append an unknown
        /// rule type carrying required parameters. The write body must preserve every
        /// key present in that response. Because the response is itself a serialization,
        /// re-serializing the modeled keys is idempotent, so the only way containment can
        /// fail is by dropping the unmodeled data — exactly finding 2.
        #[test]
        fn ruleset_write_body_preserves_unmodeled_rule_parameters(
            ruleset in ruleset_strategy(),
        ) {
            use crate::github::types::UpdateRulesetRequest;

            let probe_key = "x_unmodeled_probe";
            let probe_value = serde_json::json!({ "kept": [true, "verbatim"] });

            let mut response = serde_json::to_value(&ruleset).expect("ruleset serializes");
            // An empty `rules` is omitted by `skip_serializing_if`, so materialize the
            // array before decorating it.
            let rules = response
                .as_object_mut()
                .expect("ruleset is an object")
                .entry("rules")
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .expect("rules is an array");
            for rule in rules.iter_mut() {
                let rule = rule.as_object_mut().expect("rule is an object");
                rule.entry("parameters")
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                    .as_object_mut()
                    .expect("parameters is an object")
                    .insert(probe_key.to_owned(), probe_value.clone());
            }
            rules.push(serde_json::json!({
                "type": "commit_message_pattern",
                "parameters": { "operator": "starts_with", "pattern": "X" },
            }));

            let parsed: Ruleset =
                serde_json::from_value(response.clone()).expect("decorated response deserializes");
            let body = serde_json::to_value(UpdateRulesetRequest::from_ruleset(&parsed))
                .expect("write body serializes");

            let input_rules = response["rules"].as_array().expect("response has rules");
            let output_rules = body["rules"].as_array().expect("write body has rules");
            prop_assert_eq!(output_rules.len(), input_rules.len());

            for (input, output) in input_rules.iter().zip(output_rules) {
                for (key, value) in input["parameters"].as_object().expect("params object") {
                    prop_assert_eq!(
                        output["parameters"].get(key),
                        Some(value),
                        "rule `{}` dropped or altered parameter `{}` on GET-modify-PUT",
                        input["type"].to_string(),
                        key
                    );
                }
            }
        }
    }

    /// A security-relevant fact whose *absence* from a snapshot the invariant below
    /// probes. `present`/`contrasting` are two concrete JSON values for the field;
    /// the fact that some rule's verdict differs between them is what proves the
    /// rule *observes* the field, so no hand-maintained rule->field map is needed.
    struct HoleableFact {
        name: &'static str,
        /// Object-key path to the field inside a serialized `RepoFacts`.
        path: &'static [&'static str],
        present: fn() -> serde_json::Value,
        contrasting: fn() -> serde_json::Value,
        /// The JSON for "gathered but could not be determined". Setting the field
        /// to this must make every observing rule `Error` — the represented-unknown
        /// counterpart to *removing* the key (which must fail to load).
        unknown: fn() -> serde_json::Value,
    }

    /// The `RepoFacts`-layer facts whose *presence in a snapshot* is mandatory.
    /// Each is now a [`Gathered`] value, so removing its key makes the snapshot fail
    /// to load (Mechanism B) — the invariant below relies on that `else continue`
    /// branch rather than on any rule returning `Error`.
    fn holeable_facts() -> Vec<HoleableFact> {
        vec![
            HoleableFact {
                name: "legacy_branch_protection",
                path: &["legacy_branch_protection"],
                present: || {
                    serde_json::to_value(Gathered::Present(BranchProtection::default())).unwrap()
                },
                contrasting: || {
                    serde_json::to_value(Gathered::<BranchProtection>::Absent).unwrap()
                },
                unknown: || serde_json::to_value(Gathered::<BranchProtection>::Unknown).unwrap(),
            },
            HoleableFact {
                name: "settings.fork_pr_approval_policy",
                path: &["settings", "fork_pr_approval_policy"],
                present: || {
                    serde_json::to_value(Gathered::Present(
                        ForkPrApprovalPolicy::AllExternalContributors,
                    ))
                    .unwrap()
                },
                contrasting: || {
                    serde_json::to_value(Gathered::Present(
                        ForkPrApprovalPolicy::FirstTimeContributors,
                    ))
                    .unwrap()
                },
                unknown: || {
                    serde_json::to_value(Gathered::<ForkPrApprovalPolicy>::Unknown).unwrap()
                },
            },
        ]
    }

    fn parent_object<'a>(
        root: &'a mut serde_json::Value,
        path: &[&str],
    ) -> &'a mut serde_json::Map<String, serde_json::Value> {
        let mut node = root;
        for key in &path[..path.len() - 1] {
            node = node
                .get_mut(*key)
                .expect("intermediate path element must exist");
        }
        node.as_object_mut().expect("parent of field must be an object")
    }

    fn set_field(
        mut root: serde_json::Value,
        path: &[&str],
        value: serde_json::Value,
    ) -> serde_json::Value {
        let last = *path.last().expect("path must be non-empty");
        parent_object(&mut root, path).insert(last.to_owned(), value);
        root
    }

    fn remove_field(mut root: serde_json::Value, path: &[&str]) -> serde_json::Value {
        let last = *path.last().expect("path must be non-empty");
        parent_object(&mut root, path).remove(last);
        root
    }

    proptest! {
        /// P2: an *absent* (unknown) security-relevant fact must never produce a
        /// definite verdict. For a fact that some rule observes, either the snapshot
        /// fails to load (absence is unrepresentable) or every rule reading the
        /// now-unknown fact returns `Error` — never `Pass`/`Fail`/`Skip`.
        ///
        /// The snapshot-layer facts in `holeable_facts` are [`Gathered`], so removing
        /// a key makes the snapshot fail to load; privilege-gated API booleans
        /// instead surface as `Error` (see finding 4). Either way, no rule reads a
        /// hole and passes.
        #[test]
        fn absent_security_fact_never_yields_a_definite_verdict(
            facts in repo_facts_strategy(),
        ) {
            use crate::rules::{default_rules, evaluate_rules, RuleResult};

            let rules = default_rules();
            let base = serde_json::to_value(&facts).expect("facts serialize");

            for field in holeable_facts() {
                let present_json = set_field(base.clone(), field.path, (field.present)());
                let contrasting_json = set_field(base.clone(), field.path, (field.contrasting)());

                let present_facts: RepoFacts = serde_json::from_value(present_json.clone())
                    .expect("present value must deserialize");
                let contrasting_facts: RepoFacts = serde_json::from_value(contrasting_json)
                    .expect("contrasting value must deserialize");

                let present_out = evaluate_rules(&rules, &present_facts);
                let contrasting_out = evaluate_rules(&rules, &contrasting_facts);

                // Rules that observe this field: their verdict moves when it does.
                let observing: Vec<usize> = (0..rules.len())
                    .filter(|&i| present_out[i].result != contrasting_out[i].result)
                    .collect();
                if observing.is_empty() {
                    // Not observable in this sample; nothing to prove for this field.
                    continue;
                }

                // Represented unknown: an explicit "unknown" value must make every
                // observing rule Error — never a definite verdict.
                let unknown_json = set_field(base.clone(), field.path, (field.unknown)());
                let unknown_facts: RepoFacts = serde_json::from_value(unknown_json)
                    .expect("unknown value must deserialize");
                let unknown_out = evaluate_rules(&rules, &unknown_facts);
                for &i in &observing {
                    prop_assert!(
                        matches!(unknown_out[i].result, RuleResult::Error { .. }),
                        "field `{}` was Unknown, but rule `{}` returned {:?}; a rule that \
                         reads an unknown fact must return Error",
                        field.name,
                        unknown_out[i].id,
                        unknown_out[i].result,
                    );
                }

                // Omitted key: absence must be unrepresentable — the snapshot fails
                // to load rather than defaulting to a definite verdict.
                let holed_json = remove_field(present_json, field.path);
                let Ok(holed_facts) = serde_json::from_value::<RepoFacts>(holed_json) else {
                    // Absence is unrepresentable: the snapshot fails to load. OK.
                    continue;
                };

                let holed_out = evaluate_rules(&rules, &holed_facts);
                for &i in &observing {
                    prop_assert!(
                        matches!(holed_out[i].result, RuleResult::Error { .. }),
                        "field `{}` was absent from the snapshot, but rule `{}` returned \
                         {:?}; a rule that reads an unknown fact must return Error, never a \
                         definite verdict",
                        field.name,
                        holed_out[i].id,
                        holed_out[i].result,
                    );
                }
            }
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

    /// The Mechanism-B guarantee, stated directly: a `Gathered` fact whose key is
    /// omitted from a snapshot must fail to load, rather than silently defaulting
    /// to `Absent` the way a bare `Option` would. Guards against anyone reverting
    /// these fields to `Option` or re-adding `#[serde(default)]`.
    #[test]
    fn snapshot_omitting_a_gathered_fact_fails_to_load() {
        let base = serde_json::to_value(sample_repo_facts()).unwrap();

        let mut without_legacy = base.clone();
        without_legacy
            .as_object_mut()
            .unwrap()
            .remove("legacy_branch_protection");
        assert!(
            serde_json::from_value::<RepoFacts>(without_legacy).is_err(),
            "a snapshot omitting `legacy_branch_protection` must fail to load, not default to Absent",
        );

        let mut without_fork = base;
        without_fork
            .pointer_mut("/settings")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap()
            .remove("fork_pr_approval_policy");
        assert!(
            serde_json::from_value::<RepoFacts>(without_fork).is_err(),
            "a snapshot omitting `settings.fork_pr_approval_policy` must fail to load",
        );
    }

    #[test]
    #[ignore = "requires GITHUB_TOKEN + network access and GITHUB_PUBLIC_REPO"]
    fn gathers_public_repo_facts() {
        // Not hardcoded to a third party's repo: point GITHUB_PUBLIC_REPO at any
        // public repo you choose (e.g. one of your own) that has at least one
        // Actions workflow, since this asserts `workflows` is non-empty.
        let token = crate::github::client::GitHubToken::from_env("GITHUB_TOKEN")
            .expect("GITHUB_TOKEN must be set");
        let spec = std::env::var("GITHUB_PUBLIC_REPO").expect(
            "set GITHUB_PUBLIC_REPO=owner/name to a public repo you can read that has \
             at least one Actions workflow",
        );
        let (owner, name) = spec
            .split_once('/')
            .expect("GITHUB_PUBLIC_REPO must be owner/name");
        let mut client = GitHubClient::new(token);
        let facts = gather_repo_facts(&mut client, RepoRef::new(owner, name)).unwrap();

        assert!(!facts.default_branch.to_string().is_empty());
        assert!(!facts.workflows.is_empty());
    }
}
