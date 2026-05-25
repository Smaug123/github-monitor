use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::facts::RepoFacts;
use crate::github::client::{GitHubClient, GitHubClientError, NonRootRepoPath};
use crate::github::types::{
    ContentEncoding, CreateGitReference, CreatePullRequest, ForkPrApprovalPolicy, MergeMethod,
    PullRequest, RefNameCondition, RepositoryFileContent, RepositoryUpdate, RulesetConditions,
    RulesetEnforcement, RulesetRule, RulesetRuleParameters, RulesetRuleType, RulesetTarget,
    UpdateRepositoryFile, UpdateRulesetRequest,
};
use crate::rules::{
    RepoSetting, Rule, RuleKind, RuleOutput, RuleResult, SettingValue,
    active_branch_rulesets_for_default_branch, evaluate_rules,
    legacy_protection_superseded_by_rulesets,
};
use crate::types::{BranchName, RepoRef, RuleId};
use crate::workflow::model::{ActionRef, ActionReference};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFix {
    pub rule_id: RuleId,
    pub rule_name: String,
    pub description: String,
    pub status: FixStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixStatus {
    Planned,
    Rejected { reason: String },
    Applied,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFix {
    rule_id: RuleId,
    rule_name: String,
    plan: FixPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixPlan {
    Effect(FixEffect),
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixEffect {
    SetRepositorySetting {
        repo: RepoRef,
        setting: RepoSetting,
        value: bool,
    },
    OpenWorkflowPinPullRequest {
        plan: WorkflowPinPullRequestPlan,
    },
    OpenAddEnvrcPullRequest {
        plan: AddEnvrcPullRequestPlan,
    },
    AddRulesetRules {
        repo: RepoRef,
        target: PlannedRulesetTarget,
        rules: Vec<RulesetRule>,
    },
    SetRulesetPullRequestMergeMethods {
        repo: RepoRef,
        target: PlannedRulesetTarget,
        allowed: Vec<MergeMethod>,
    },
    SetRulesetStrictRequiredStatusChecks {
        repo: RepoRef,
        target: PlannedRulesetTarget,
    },
    EnsureRulesetRequiredStatusCheck {
        repo: RepoRef,
        target: PlannedRulesetTarget,
        context: String,
    },
    SetForkPrApprovalPolicy {
        repo: RepoRef,
        policy: ForkPrApprovalPolicy,
    },
    DeleteLegacyBranchProtection {
        repo: RepoRef,
        branch: BranchName,
    },
    CreateDefaultBranchRuleset {
        repo: RepoRef,
        target: PlannedRulesetTarget,
    },
}

/// Identifies a ruleset that a planned fix wants to modify or create. `Existing`
/// names a ruleset that the live facts already contain (by GitHub-assigned id);
/// `PendingDefaultBranch` names the default-branch ruleset that RS001's fix
/// would create in the same batch.
///
/// Why this isn't just `Option<u64>`: planning and the executor need a single
/// merge key per ruleset so several rules' fixes compose into one GitHub call.
/// For pending creation the id doesn't exist yet, so we key by "the one
/// default-branch ruleset we're about to create" and let the executor decide
/// at apply-time whether that resolves to a POST or, if RS001 isn't in the
/// batch, a per-effect rejection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedRulesetTarget {
    Existing {
        id: u64,
        name: String,
    },
    PendingDefaultBranch {
        default_branch: BranchName,
        name: String,
    },
}

impl PlannedRulesetTarget {
    fn name(&self) -> &str {
        match self {
            Self::Existing { name, .. } | Self::PendingDefaultBranch { name, .. } => name,
        }
    }

    fn merges_with(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Existing { id: a, .. }, Self::Existing { id: b, .. }) => a == b,
            (Self::PendingDefaultBranch { .. }, Self::PendingDefaultBranch { .. }) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowPinPullRequestPlan {
    repo: RepoRef,
    default_branch: BranchName,
    workflows: Vec<WorkflowFilePins>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddEnvrcPullRequestPlan {
    repo: RepoRef,
    default_branch: BranchName,
}

const ENVRC_PATH: &str = ".envrc";
const ENVRC_CONTENTS: &str = "use flake\n";

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowFilePins {
    path: String,
    pins: Vec<WorkflowActionPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowActionPin {
    action: RepositoryActionUse,
    occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepositoryActionUse {
    repo: RepoRef,
    subpath: Option<String>,
    version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedWorkflowUpdate {
    path: String,
    sha: String,
    content: String,
    changes: Vec<WorkflowPinChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowPinChange {
    from: String,
    to: String,
    tag_comment: Option<String>,
}

#[derive(Debug, Default)]
struct RepoFixExecution {
    repo_settings: Option<Result<(), String>>,
    pull_requests: Vec<PullRequestExecution>,
    ruleset_updates: Vec<RulesetUpdateExecution>,
    fork_pr_approval_policy: Option<Result<(), String>>,
    legacy_branch_protection_deletion: Option<Result<(), String>>,
}

#[derive(Debug)]
struct PullRequestExecution {
    rule_id: RuleId,
    result: Result<PullRequest, String>,
}

#[derive(Debug)]
struct RulesetUpdateExecution {
    rule_ids: Vec<RuleId>,
    result: Result<(), String>,
}

#[derive(Debug, Clone)]
struct QueuedPullRequest {
    rule_id: RuleId,
    plan: WorkflowPinPullRequestPlan,
}

#[derive(Debug, Clone)]
struct QueuedEnvrcPullRequest {
    rule_id: RuleId,
    plan: AddEnvrcPullRequestPlan,
}

#[derive(Debug, Clone)]
struct QueuedRulesetUpdate {
    target: PlannedRulesetTarget,
    rule_ids: Vec<RuleId>,
    rules_to_add: Vec<RulesetRule>,
    set_pull_request_allowed_merge_methods: Option<Vec<MergeMethod>>,
    set_strict_required_status_checks: Option<bool>,
    add_required_status_check_contexts: Vec<String>,
    /// `true` if a `CreateDefaultBranchRuleset` effect contributed to this
    /// queue entry. The apply step uses this to decide between POST (new
    /// ruleset) and GET+PUT (mutate existing). Only meaningful when `target`
    /// is `PendingDefaultBranch`.
    create: bool,
}

impl PlannedFix {
    pub fn planned_report(&self) -> RepoFix {
        match &self.plan {
            FixPlan::Effect(_) => self.with_status(FixStatus::Planned),
            FixPlan::Rejected { reason } => self.with_status(FixStatus::Rejected {
                reason: reason.clone(),
            }),
        }
    }

    fn with_status(&self, status: FixStatus) -> RepoFix {
        RepoFix {
            rule_id: self.rule_id.clone(),
            rule_name: self.rule_name.clone(),
            description: self.description(),
            status,
        }
    }

    fn description(&self) -> String {
        match &self.plan {
            FixPlan::Effect(effect) => effect.describe(),
            FixPlan::Rejected { .. } => "automatic fix unavailable".to_owned(),
        }
    }
}

impl FixEffect {
    fn describe(&self) -> String {
        match self {
            Self::SetRepositorySetting { setting, value, .. } => {
                format!("set repository setting `{}` to {value}", setting.name())
            }
            Self::OpenWorkflowPinPullRequest { plan } => {
                let file_count = plan.workflows.len();
                let pin_count = plan
                    .workflows
                    .iter()
                    .flat_map(|workflow| &workflow.pins)
                    .map(|pin| pin.occurrences)
                    .sum::<usize>();

                format!(
                    "open a pull request that pins {pin_count} workflow action {} across {file_count} workflow {} to commit SHAs",
                    pluralize(pin_count, "reference", "references"),
                    pluralize(file_count, "file", "files"),
                )
            }
            Self::OpenAddEnvrcPullRequest { .. } => {
                format!("open a pull request that adds `{ENVRC_PATH}` with `use flake`")
            }
            Self::AddRulesetRules { target, rules, .. } => {
                let rule_names = rules
                    .iter()
                    .map(|rule| format!("`{}`", ruleset_rule_type_name(&rule.kind)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "add {} {} to ruleset `{}`: {rule_names}",
                    rules.len(),
                    pluralize(rules.len(), "rule", "rules"),
                    target.name(),
                )
            }
            Self::SetRulesetPullRequestMergeMethods {
                target, allowed, ..
            } => {
                let methods = allowed
                    .iter()
                    .map(|method| format!("`{}`", String::from(method.clone())))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "set `pull_request` allowed merge methods on ruleset `{}` to: {methods}",
                    target.name(),
                )
            }
            Self::SetRulesetStrictRequiredStatusChecks { target, .. } => {
                format!(
                    "enable `strict_required_status_checks_policy` on ruleset `{}`",
                    target.name(),
                )
            }
            Self::EnsureRulesetRequiredStatusCheck {
                target, context, ..
            } => {
                format!(
                    "require status check `{context}` on ruleset `{}`",
                    target.name(),
                )
            }
            Self::SetForkPrApprovalPolicy { policy, .. } => {
                format!(
                    "set fork PR contributor approval policy to `{}`",
                    String::from(policy.clone())
                )
            }
            Self::DeleteLegacyBranchProtection { branch, .. } => {
                format!("delete legacy branch protection on `{branch}`")
            }
            Self::CreateDefaultBranchRuleset { target, .. } => match target {
                PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch,
                    name,
                } => format!("create active branch ruleset `{name}` covering `{default_branch}`",),
                PlannedRulesetTarget::Existing { name, .. } => format!(
                    "create active branch ruleset `{name}` (internal error: target marked as existing)",
                ),
            },
        }
    }

    fn repo(&self) -> &RepoRef {
        match self {
            Self::SetRepositorySetting { repo, .. } => repo,
            Self::OpenWorkflowPinPullRequest { plan } => &plan.repo,
            Self::OpenAddEnvrcPullRequest { plan } => &plan.repo,
            Self::AddRulesetRules { repo, .. } => repo,
            Self::SetRulesetPullRequestMergeMethods { repo, .. } => repo,
            Self::SetRulesetStrictRequiredStatusChecks { repo, .. } => repo,
            Self::EnsureRulesetRequiredStatusCheck { repo, .. } => repo,
            Self::SetForkPrApprovalPolicy { repo, .. } => repo,
            Self::DeleteLegacyBranchProtection { repo, .. } => repo,
            Self::CreateDefaultBranchRuleset { repo, .. } => repo,
        }
    }

    fn ruleset_target(&self) -> Option<&PlannedRulesetTarget> {
        match self {
            Self::AddRulesetRules { target, .. }
            | Self::SetRulesetPullRequestMergeMethods { target, .. }
            | Self::SetRulesetStrictRequiredStatusChecks { target, .. }
            | Self::EnsureRulesetRequiredStatusCheck { target, .. }
            | Self::CreateDefaultBranchRuleset { target, .. } => Some(target),
            Self::SetRepositorySetting { .. }
            | Self::OpenWorkflowPinPullRequest { .. }
            | Self::OpenAddEnvrcPullRequest { .. }
            | Self::SetForkPrApprovalPolicy { .. }
            | Self::DeleteLegacyBranchProtection { .. } => None,
        }
    }
}

fn ruleset_rule_type_name(kind: &RulesetRuleType) -> String {
    String::from(kind.clone())
}

impl RepositoryActionUse {
    fn from_action_ref(action_ref: &ActionRef) -> Self {
        Self {
            repo: RepoRef {
                owner: action_ref.owner.clone(),
                name: action_ref.repo.clone(),
            },
            subpath: None,
            version: action_ref.version.clone(),
        }
    }

    fn resolution_key(&self) -> String {
        format!("{}@{}", self.repo, self.version)
    }

    fn rendered_with_version(&self, version: &str) -> String {
        match &self.subpath {
            Some(subpath) => format!("{}/{subpath}@{version}", self.repo),
            None => format!("{}@{version}", self.repo),
        }
    }

    /// Returns the original version string to use as a YAML comment when pinning
    /// changes the version (e.g. from a tag to a SHA), or `None` if unchanged.
    fn tag_comment(&self, pinned_version: &str) -> Option<&str> {
        if pinned_version != self.version {
            Some(&self.version)
        } else {
            None
        }
    }
}

impl std::fmt::Display for RepositoryActionUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered_with_version(&self.version))
    }
}

pub fn plan_repo_fixes(rules: &[Rule], facts: &RepoFacts) -> Vec<PlannedFix> {
    let outputs = evaluate_rules(rules, facts);

    let mut planned: Vec<PlannedFix> = std::iter::zip(rules, &outputs)
        .filter_map(|(rule, output)| plan_rule_fix(facts, rule, output))
        .collect();
    reconcile_pending_default_branch_target(&mut planned);
    planned
}

/// Population planners (RS012/RS013 and friends) emit fixes targeting the
/// pending default-branch ruleset whenever no live ruleset covers it, leaving
/// the actual creation to RS001's `CreateDefaultBranchRuleset` effect. If RS001
/// is not in the batch — either disabled or already passing — those fixes have
/// nothing to attach to and must be rejected; otherwise they would silently
/// pile up state in a ruleset that never gets created.
fn reconcile_pending_default_branch_target(planned: &mut [PlannedFix]) {
    let creation_planned = planned.iter().any(|fix| {
        matches!(
            &fix.plan,
            FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset { .. })
        )
    });
    if creation_planned {
        return;
    }
    for fix in planned.iter_mut() {
        let FixPlan::Effect(effect) = &fix.plan else {
            continue;
        };
        let Some(target) = effect.ruleset_target() else {
            continue;
        };
        if !matches!(target, PlannedRulesetTarget::PendingDefaultBranch { .. }) {
            continue;
        }
        fix.plan = FixPlan::Rejected {
            reason: "no active branch ruleset applies to the default branch, and RS001 \
                     (create ruleset) is not in the plan — enable RS001 or create the \
                     ruleset manually"
                .to_owned(),
        };
    }
}

pub fn execute_repo_fixes(client: &mut GitHubClient, fixes: &[PlannedFix]) -> Vec<RepoFix> {
    let execution = execute_planned_effects(client, fixes);

    fixes
        .iter()
        .map(|fix| match &fix.plan {
            FixPlan::Rejected { reason } => fix.with_status(FixStatus::Rejected {
                reason: reason.clone(),
            }),
            FixPlan::Effect(FixEffect::SetRepositorySetting { .. }) => {
                match execution.repo_settings.as_ref() {
                    Some(Ok(())) => fix.with_status(FixStatus::Applied),
                    Some(Err(reason)) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason: "internal error: missing repository settings execution result"
                            .to_owned(),
                    }),
                }
            }
            FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { .. })
            | FixPlan::Effect(FixEffect::OpenAddEnvrcPullRequest { .. }) => {
                match execution
                    .pull_requests
                    .iter()
                    .find(|execution| execution.rule_id == fix.rule_id)
                {
                    Some(PullRequestExecution { result: Ok(_), .. }) => {
                        fix.with_status(FixStatus::Applied)
                    }
                    Some(PullRequestExecution {
                        result: Err(reason),
                        ..
                    }) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason: "internal error: missing pull request execution result".to_owned(),
                    }),
                }
            }
            FixPlan::Effect(FixEffect::AddRulesetRules { .. })
            | FixPlan::Effect(FixEffect::SetRulesetPullRequestMergeMethods { .. })
            | FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks { .. })
            | FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck { .. })
            | FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset { .. }) => {
                match execution
                    .ruleset_updates
                    .iter()
                    .find(|update| update.rule_ids.contains(&fix.rule_id))
                {
                    Some(RulesetUpdateExecution { result: Ok(()), .. }) => {
                        fix.with_status(FixStatus::Applied)
                    }
                    Some(RulesetUpdateExecution {
                        result: Err(reason),
                        ..
                    }) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason: "internal error: missing ruleset update execution result"
                            .to_owned(),
                    }),
                }
            }
            FixPlan::Effect(FixEffect::SetForkPrApprovalPolicy { .. }) => {
                match execution.fork_pr_approval_policy.as_ref() {
                    Some(Ok(())) => fix.with_status(FixStatus::Applied),
                    Some(Err(reason)) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason:
                            "internal error: missing fork PR contributor approval policy execution result"
                                .to_owned(),
                    }),
                }
            }
            FixPlan::Effect(FixEffect::DeleteLegacyBranchProtection { .. }) => {
                match execution.legacy_branch_protection_deletion.as_ref() {
                    Some(Ok(())) => fix.with_status(FixStatus::Applied),
                    Some(Err(reason)) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason:
                            "internal error: missing legacy branch protection deletion execution result"
                                .to_owned(),
                    }),
                }
            }
        })
        .collect()
}

fn execute_planned_effects(client: &mut GitHubClient, fixes: &[PlannedFix]) -> RepoFixExecution {
    let mut repo = None::<RepoRef>;
    let mut update = RepositoryUpdate::default();
    let mut saw_repo_settings = false;
    let mut queued_pull_requests = Vec::new();
    let mut queued_envrc_pull_requests = Vec::<QueuedEnvrcPullRequest>::new();
    let mut queued_ruleset_updates = Vec::<QueuedRulesetUpdate>::new();
    let mut fork_pr_approval_policy_to_apply = None::<ForkPrApprovalPolicy>;
    let mut legacy_branch_protection_to_delete = None::<BranchName>;
    let mut internal_error = None::<String>;

    for fix in fixes {
        let FixPlan::Effect(effect) = &fix.plan else {
            continue;
        };

        if let Some(existing_repo) = &repo {
            if existing_repo != effect.repo() && internal_error.is_none() {
                internal_error = Some(format!(
                    "internal error: planned fixes span multiple repositories (`{existing_repo}` and `{}`)",
                    effect.repo()
                ));
            }
        } else {
            repo = Some(effect.repo().clone());
        }

        match effect {
            FixEffect::SetRepositorySetting { .. } => {
                saw_repo_settings = true;
                if internal_error.is_none()
                    && let Some(reason) = apply_fix_effect_to_repository_update(&mut update, effect)
                {
                    internal_error = Some(reason);
                }
            }
            FixEffect::OpenWorkflowPinPullRequest { plan } => {
                queued_pull_requests.push(QueuedPullRequest {
                    rule_id: fix.rule_id.clone(),
                    plan: plan.clone(),
                });
            }
            FixEffect::OpenAddEnvrcPullRequest { plan } => {
                queued_envrc_pull_requests.push(QueuedEnvrcPullRequest {
                    rule_id: fix.rule_id.clone(),
                    plan: plan.clone(),
                });
            }
            FixEffect::AddRulesetRules { target, rules, .. } => {
                enqueue_ruleset_update(
                    &mut queued_ruleset_updates,
                    fix.rule_id.clone(),
                    target.clone(),
                    rules.clone(),
                );
            }
            FixEffect::SetRulesetPullRequestMergeMethods {
                target, allowed, ..
            } => {
                enqueue_set_pull_request_merge_methods(
                    &mut queued_ruleset_updates,
                    fix.rule_id.clone(),
                    target.clone(),
                    allowed.clone(),
                );
            }
            FixEffect::SetRulesetStrictRequiredStatusChecks { target, .. } => {
                enqueue_set_strict_required_status_checks(
                    &mut queued_ruleset_updates,
                    fix.rule_id.clone(),
                    target.clone(),
                );
            }
            FixEffect::EnsureRulesetRequiredStatusCheck {
                target, context, ..
            } => {
                enqueue_ensure_required_status_check(
                    &mut queued_ruleset_updates,
                    fix.rule_id.clone(),
                    target.clone(),
                    context.clone(),
                );
            }
            FixEffect::SetForkPrApprovalPolicy { policy, .. } => {
                fork_pr_approval_policy_to_apply = Some(policy.clone());
            }
            FixEffect::DeleteLegacyBranchProtection { branch, .. } => {
                legacy_branch_protection_to_delete = Some(branch.clone());
            }
            FixEffect::CreateDefaultBranchRuleset { target, .. } => {
                enqueue_ruleset_creation(
                    &mut queued_ruleset_updates,
                    fix.rule_id.clone(),
                    target.clone(),
                );
            }
        }
    }

    if let Some(reason) = internal_error {
        return RepoFixExecution {
            repo_settings: saw_repo_settings.then(|| Err(reason.clone())),
            pull_requests: queued_pull_requests
                .into_iter()
                .map(|queued| PullRequestExecution {
                    rule_id: queued.rule_id,
                    result: Err(reason.clone()),
                })
                .chain(
                    queued_envrc_pull_requests
                        .into_iter()
                        .map(|queued| PullRequestExecution {
                            rule_id: queued.rule_id,
                            result: Err(reason.clone()),
                        }),
                )
                .collect(),
            ruleset_updates: queued_ruleset_updates
                .into_iter()
                .map(|queued| RulesetUpdateExecution {
                    rule_ids: queued.rule_ids,
                    result: Err(reason.clone()),
                })
                .collect(),
            fork_pr_approval_policy: fork_pr_approval_policy_to_apply
                .as_ref()
                .map(|_| Err(reason.clone())),
            legacy_branch_protection_deletion: legacy_branch_protection_to_delete
                .as_ref()
                .map(|_| Err(reason.clone())),
        };
    }

    if saw_repo_settings && update.is_empty() {
        let reason = "internal error: automatic fix produced an empty repository update".to_owned();
        return RepoFixExecution {
            repo_settings: Some(Err(reason.clone())),
            pull_requests: queued_pull_requests
                .into_iter()
                .map(|queued| PullRequestExecution {
                    rule_id: queued.rule_id,
                    result: Err(reason.clone()),
                })
                .chain(
                    queued_envrc_pull_requests
                        .into_iter()
                        .map(|queued| PullRequestExecution {
                            rule_id: queued.rule_id,
                            result: Err(reason.clone()),
                        }),
                )
                .collect(),
            ruleset_updates: queued_ruleset_updates
                .into_iter()
                .map(|queued| RulesetUpdateExecution {
                    rule_ids: queued.rule_ids,
                    result: Err(reason.clone()),
                })
                .collect(),
            fork_pr_approval_policy: fork_pr_approval_policy_to_apply
                .as_ref()
                .map(|_| Err(reason.clone())),
            legacy_branch_protection_deletion: legacy_branch_protection_to_delete
                .as_ref()
                .map(|_| Err(reason.clone())),
        };
    }

    let repo_settings = if saw_repo_settings {
        let repo = repo
            .as_ref()
            .expect("repository recorded whenever a repository setting effect is present");
        Some(
            client
                .update_repository(repo, &update)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        )
    } else {
        None
    };

    let mut pull_requests = queued_pull_requests
        .into_iter()
        .map(|queued| PullRequestExecution {
            rule_id: queued.rule_id,
            result: create_workflow_pin_pull_request(client, &queued.plan),
        })
        .collect::<Vec<_>>();
    pull_requests.extend(queued_envrc_pull_requests.into_iter().map(|queued| {
        PullRequestExecution {
            rule_id: queued.rule_id,
            result: create_add_envrc_pull_request(client, &queued.plan),
        }
    }));

    let ruleset_updates = if queued_ruleset_updates.is_empty() {
        Vec::new()
    } else {
        let repo = repo
            .as_ref()
            .expect("repository recorded whenever a ruleset update effect is present");
        queued_ruleset_updates
            .into_iter()
            .map(|queued| RulesetUpdateExecution {
                result: apply_ruleset_update(client, repo, &queued),
                rule_ids: queued.rule_ids,
            })
            .collect()
    };

    let fork_pr_approval_policy = fork_pr_approval_policy_to_apply.map(|policy| {
        let repo = repo
            .as_ref()
            .expect("repository recorded whenever a fork-PR policy effect is present");
        client
            .set_fork_pr_approval_permission(repo, &policy)
            .map_err(|error| error.to_string())
    });

    let legacy_branch_protection_deletion = legacy_branch_protection_to_delete.map(|branch| {
        let repo = repo
            .as_ref()
            .expect("repository recorded whenever a legacy branch protection effect is present");
        client
            .delete_branch_protection(repo, &branch)
            .map_err(|error| error.to_string())
    });

    RepoFixExecution {
        repo_settings,
        pull_requests,
        ruleset_updates,
        fork_pr_approval_policy,
        legacy_branch_protection_deletion,
    }
}

fn queued_ruleset_entry_mut<'a>(
    queue: &'a mut [QueuedRulesetUpdate],
    target: &PlannedRulesetTarget,
) -> Option<&'a mut QueuedRulesetUpdate> {
    queue
        .iter_mut()
        .find(|entry| entry.target.merges_with(target))
}

fn empty_queued_ruleset_update(
    target: PlannedRulesetTarget,
    rule_id: RuleId,
) -> QueuedRulesetUpdate {
    QueuedRulesetUpdate {
        target,
        rule_ids: vec![rule_id],
        rules_to_add: Vec::new(),
        set_pull_request_allowed_merge_methods: None,
        set_strict_required_status_checks: None,
        add_required_status_check_contexts: Vec::new(),
        create: false,
    }
}

fn enqueue_ruleset_update(
    queue: &mut Vec<QueuedRulesetUpdate>,
    rule_id: RuleId,
    target: PlannedRulesetTarget,
    rules: Vec<RulesetRule>,
) {
    if let Some(existing) = queued_ruleset_entry_mut(queue, &target) {
        existing.rule_ids.push(rule_id);
        for rule in rules {
            if !existing
                .rules_to_add
                .iter()
                .any(|existing_rule| existing_rule.kind == rule.kind)
            {
                existing.rules_to_add.push(rule);
            }
        }
    } else {
        let mut entry = empty_queued_ruleset_update(target, rule_id);
        entry.rules_to_add = rules;
        queue.push(entry);
    }
}

fn enqueue_set_pull_request_merge_methods(
    queue: &mut Vec<QueuedRulesetUpdate>,
    rule_id: RuleId,
    target: PlannedRulesetTarget,
    allowed: Vec<MergeMethod>,
) {
    if let Some(existing) = queued_ruleset_entry_mut(queue, &target) {
        existing.rule_ids.push(rule_id);
        debug_assert!(
            existing.set_pull_request_allowed_merge_methods.is_none(),
            "duplicate pull-request merge-method update for ruleset `{}`",
            target.name(),
        );
        existing.set_pull_request_allowed_merge_methods = Some(allowed);
    } else {
        let mut entry = empty_queued_ruleset_update(target, rule_id);
        entry.set_pull_request_allowed_merge_methods = Some(allowed);
        queue.push(entry);
    }
}

fn enqueue_set_strict_required_status_checks(
    queue: &mut Vec<QueuedRulesetUpdate>,
    rule_id: RuleId,
    target: PlannedRulesetTarget,
) {
    if let Some(existing) = queued_ruleset_entry_mut(queue, &target) {
        existing.rule_ids.push(rule_id);
        debug_assert!(
            existing.set_strict_required_status_checks.is_none(),
            "duplicate strict-required-status-checks update for ruleset `{}`",
            target.name(),
        );
        existing.set_strict_required_status_checks = Some(true);
    } else {
        let mut entry = empty_queued_ruleset_update(target, rule_id);
        entry.set_strict_required_status_checks = Some(true);
        queue.push(entry);
    }
}

fn enqueue_ensure_required_status_check(
    queue: &mut Vec<QueuedRulesetUpdate>,
    rule_id: RuleId,
    target: PlannedRulesetTarget,
    context: String,
) {
    if let Some(existing) = queued_ruleset_entry_mut(queue, &target) {
        existing.rule_ids.push(rule_id);
        if !existing
            .add_required_status_check_contexts
            .iter()
            .any(|existing_context| existing_context == &context)
        {
            existing.add_required_status_check_contexts.push(context);
        }
    } else {
        let mut entry = empty_queued_ruleset_update(target, rule_id);
        entry.add_required_status_check_contexts.push(context);
        queue.push(entry);
    }
}

fn enqueue_ruleset_creation(
    queue: &mut Vec<QueuedRulesetUpdate>,
    rule_id: RuleId,
    target: PlannedRulesetTarget,
) {
    if let Some(existing) = queued_ruleset_entry_mut(queue, &target) {
        existing.rule_ids.push(rule_id);
        debug_assert!(
            !existing.create,
            "duplicate ruleset creation for target `{}`",
            target.name(),
        );
        existing.create = true;
    } else {
        let mut entry = empty_queued_ruleset_update(target, rule_id);
        entry.create = true;
        queue.push(entry);
    }
}

fn apply_ruleset_update(
    client: &mut GitHubClient,
    repo: &RepoRef,
    queued: &QueuedRulesetUpdate,
) -> Result<(), String> {
    match &queued.target {
        PlannedRulesetTarget::Existing { id, name } => {
            apply_existing_ruleset_update(client, repo, *id, name, queued)
        }
        PlannedRulesetTarget::PendingDefaultBranch {
            default_branch,
            name,
        } => {
            if !queued.create {
                return Err(format!(
                    "automatic fix targeted ruleset `{name}` covering `{default_branch}`, \
                     but RS001 (create ruleset) is not in the plan — enable RS001 or create \
                     the ruleset manually",
                ));
            }
            create_default_branch_ruleset(client, repo, default_branch, name, queued)
        }
    }
}

fn apply_existing_ruleset_update(
    client: &mut GitHubClient,
    repo: &RepoRef,
    id: u64,
    name: &str,
    queued: &QueuedRulesetUpdate,
) -> Result<(), String> {
    let mut ruleset = client.get_ruleset(repo, id).map_err(|error| {
        format!("failed to fetch ruleset `{name}` (id {id}) from `{repo}`: {error}")
    })?;

    apply_queued_modifications(&mut ruleset.rules, queued);

    let body = UpdateRulesetRequest::from_ruleset(&ruleset);
    client
        .update_ruleset(repo, id, &body)
        .map(|_| ())
        .map_err(|error| {
            format!("failed to update ruleset `{name}` (id {id}) on `{repo}`: {error}")
        })
}

fn create_default_branch_ruleset(
    client: &mut GitHubClient,
    repo: &RepoRef,
    default_branch: &BranchName,
    name: &str,
    queued: &QueuedRulesetUpdate,
) -> Result<(), String> {
    let mut rules: Vec<RulesetRule> = Vec::new();
    apply_queued_modifications(&mut rules, queued);

    let body = UpdateRulesetRequest {
        name: name.to_owned(),
        target: RulesetTarget::Branch,
        enforcement: RulesetEnforcement::Active,
        conditions: Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec!["~DEFAULT_BRANCH".to_owned()],
                exclude: Vec::new(),
            }),
        }),
        bypass_actors: Vec::new(),
        rules,
    };
    client
        .create_ruleset(repo, &body)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "failed to create ruleset `{name}` on `{repo}` covering `{default_branch}`: {error}"
            )
        })
}

/// Applies the queued modifications to a ruleset's rule list in place. Used by
/// both the update (GET-modify-PUT) and create (build-then-POST) paths so the
/// per-effect merging logic — adding distinct rule kinds, setting parameters
/// on existing or newly-added `PullRequest` / `RequiredStatusChecks` rules —
/// is identical regardless of whether the ruleset already exists.
fn apply_queued_modifications(rules: &mut Vec<RulesetRule>, queued: &QueuedRulesetUpdate) {
    for rule in &queued.rules_to_add {
        if !rules.iter().any(|existing| existing.kind == rule.kind) {
            let to_push = if rule.kind == RulesetRuleType::PullRequest && rule.parameters.is_none()
            {
                new_pull_request_rule_with_required_defaults()
            } else {
                rule.clone()
            };
            rules.push(to_push);
        }
    }

    if let Some(allowed) = &queued.set_pull_request_allowed_merge_methods {
        let pr_rule = match rules
            .iter_mut()
            .find(|rule| rule.kind == RulesetRuleType::PullRequest)
        {
            Some(rule) => rule,
            None => {
                rules.push(new_pull_request_rule_with_required_defaults());
                rules.last_mut().expect("just pushed")
            }
        };
        let params = pr_rule
            .parameters
            .get_or_insert_with(RulesetRuleParameters::default);
        params.allowed_merge_methods = allowed.clone();
    }

    let want_strict = queued.set_strict_required_status_checks == Some(true);
    let want_contexts = !queued.add_required_status_check_contexts.is_empty();
    if want_strict || want_contexts {
        let status_rule = match rules
            .iter_mut()
            .find(|rule| rule.kind == RulesetRuleType::RequiredStatusChecks)
        {
            Some(rule) => rule,
            None => {
                rules.push(RulesetRule {
                    kind: RulesetRuleType::RequiredStatusChecks,
                    parameters: None,
                });
                rules.last_mut().expect("just pushed")
            }
        };
        let params = status_rule
            .parameters
            .get_or_insert_with(RulesetRuleParameters::default);
        if want_strict {
            params.strict_required_status_checks_policy = Some(true);
        }
        for context in &queued.add_required_status_check_contexts {
            if !params
                .required_status_checks
                .iter()
                .any(|check| check.context == *context)
            {
                params
                    .required_status_checks
                    .push(crate::github::types::RequiredStatusCheck {
                        context: context.clone(),
                        integration_id: None,
                    });
            }
        }
    }
}

/// Builds a fresh `pull_request` rule with the parameters GitHub's
/// create-ruleset endpoint requires. Sending the rule without these fields
/// causes a 422 ("data matches no possible input"). All defaults are
/// permissive — explicit rules (e.g. RS011) override them.
fn new_pull_request_rule_with_required_defaults() -> RulesetRule {
    RulesetRule {
        kind: RulesetRuleType::PullRequest,
        parameters: Some(RulesetRuleParameters {
            required_approving_review_count: Some(0),
            dismiss_stale_reviews_on_push: Some(false),
            require_code_owner_review: Some(false),
            require_last_push_approval: Some(false),
            required_review_thread_resolution: Some(false),
            ..RulesetRuleParameters::default()
        }),
    }
}

fn create_workflow_pin_pull_request(
    client: &mut GitHubClient,
    plan: &WorkflowPinPullRequestPlan,
) -> Result<PullRequest, String> {
    let branch_name = workflow_pin_branch_name();
    let base_sha = client
        .resolve_commit_sha(&plan.repo, &plan.repo, &plan.default_branch.to_string())
        .map_err(|error| {
            format!(
                "failed to resolve base branch `{}` for `{}`: {error}",
                plan.default_branch, plan.repo
            )
        })?;
    let prepared_updates = prepare_workflow_updates(client, plan, &base_sha)?;

    client
        .create_git_reference(
            &plan.repo,
            &CreateGitReference {
                reference: format!("refs/heads/{branch_name}"),
                sha: base_sha,
            },
        )
        .map_err(|error| {
            format!(
                "failed to create branch `{branch_name}` in `{}`: {error}",
                plan.repo
            )
        })?;

    for update in &prepared_updates {
        let path = NonRootRepoPath::new(&update.path).map_err(|error| {
            format!(
                "generated workflow path `{}` is not a valid repository path: {error}",
                update.path
            )
        })?;

        if let Err(error) = client.update_file_contents(
            &plan.repo,
            &path,
            &UpdateRepositoryFile {
                message: format!("Pin GitHub Actions to commit SHAs in {}", update.path),
                content: base64::engine::general_purpose::STANDARD
                    .encode(update.content.as_bytes()),
                sha: Some(update.sha.clone()),
                branch: branch_name.clone(),
            },
        ) {
            let failure = format!(
                "failed to update workflow `{}` in `{}`: {error}",
                update.path, plan.repo
            );
            return Err(cleanup_failed_workflow_pin_branch(
                client,
                plan,
                &branch_name,
                failure,
            ));
        }
    }

    match client.create_pull_request(
        &plan.repo,
        &CreatePullRequest {
            title: workflow_pin_pull_request_title(),
            head: branch_name.clone(),
            base: plan.default_branch.to_string(),
            body: workflow_pin_pull_request_body(&prepared_updates),
        },
    ) {
        Ok(pull_request) => Ok(pull_request),
        Err(error) => {
            let failure = format!(
                "failed to open pull request for workflow action pinning in `{}`: {error}",
                plan.repo
            );

            match error {
                GitHubClientError::UnexpectedStatus { .. } => Err(
                    cleanup_failed_workflow_pin_branch(client, plan, &branch_name, failure),
                ),
                GitHubClientError::Request { .. } | GitHubClientError::Auth { .. } => Err(failure),
                GitHubClientError::UnexpectedContentsShape { .. } => {
                    unreachable!("pull request creation does not use repository contents endpoints")
                }
            }
        }
    }
}

fn create_add_envrc_pull_request(
    client: &mut GitHubClient,
    plan: &AddEnvrcPullRequestPlan,
) -> Result<PullRequest, String> {
    let branch_name = add_envrc_branch_name();
    let base_sha = client
        .resolve_commit_sha(&plan.repo, &plan.repo, &plan.default_branch.to_string())
        .map_err(|error| {
            format!(
                "failed to resolve base branch `{}` for `{}`: {error}",
                plan.default_branch, plan.repo
            )
        })?;

    client
        .create_git_reference(
            &plan.repo,
            &CreateGitReference {
                reference: format!("refs/heads/{branch_name}"),
                sha: base_sha,
            },
        )
        .map_err(|error| {
            format!(
                "failed to create branch `{branch_name}` in `{}`: {error}",
                plan.repo
            )
        })?;

    let path = NonRootRepoPath::new(ENVRC_PATH)
        .map_err(|error| format!("`{ENVRC_PATH}` is not a valid repository path: {error}"))?;

    if let Err(error) = client.update_file_contents(
        &plan.repo,
        &path,
        &UpdateRepositoryFile {
            message: format!("Add `{ENVRC_PATH}`"),
            content: base64::engine::general_purpose::STANDARD.encode(ENVRC_CONTENTS.as_bytes()),
            sha: None,
            branch: branch_name.clone(),
        },
    ) {
        let failure = format!(
            "failed to create `{ENVRC_PATH}` in `{}`: {error}",
            plan.repo
        );
        return Err(cleanup_failed_add_envrc_branch(
            client,
            plan,
            &branch_name,
            failure,
        ));
    }

    match client.create_pull_request(
        &plan.repo,
        &CreatePullRequest {
            title: add_envrc_pull_request_title(),
            head: branch_name.clone(),
            base: plan.default_branch.to_string(),
            body: add_envrc_pull_request_body(),
        },
    ) {
        Ok(pull_request) => Ok(pull_request),
        Err(error) => {
            let failure = format!(
                "failed to open pull request that adds `{ENVRC_PATH}` in `{}`: {error}",
                plan.repo
            );

            match error {
                GitHubClientError::UnexpectedStatus { .. } => Err(cleanup_failed_add_envrc_branch(
                    client,
                    plan,
                    &branch_name,
                    failure,
                )),
                GitHubClientError::Request { .. } | GitHubClientError::Auth { .. } => Err(failure),
                GitHubClientError::UnexpectedContentsShape { .. } => {
                    unreachable!("pull request creation does not use repository contents endpoints")
                }
            }
        }
    }
}

fn add_envrc_pull_request_title() -> String {
    format!("Add `{ENVRC_PATH}`")
}

fn add_envrc_pull_request_body() -> String {
    format!(
        "Generated by github-infra.\n\nAdds `{ENVRC_PATH}` containing `use flake` so the Nix devshell is loaded automatically by direnv.",
    )
}

fn add_envrc_branch_name() -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("github-infra/add-envrc-{suffix}")
}

fn cleanup_failed_add_envrc_branch(
    client: &mut GitHubClient,
    plan: &AddEnvrcPullRequestPlan,
    branch_name: &str,
    failure: String,
) -> String {
    match client.delete_git_reference(&plan.repo, &format!("heads/{branch_name}")) {
        Ok(()) => failure,
        Err(cleanup_error) => format!(
            "{failure}; additionally failed to delete temporary branch `{branch_name}` in `{}`: {cleanup_error}",
            plan.repo
        ),
    }
}

fn prepare_workflow_updates(
    client: &mut GitHubClient,
    plan: &WorkflowPinPullRequestPlan,
    reference: &str,
) -> Result<Vec<PreparedWorkflowUpdate>, String> {
    let mut resolved_shas = std::collections::HashMap::<String, String>::new();
    let mut prepared = Vec::with_capacity(plan.workflows.len());

    for workflow in &plan.workflows {
        let path = NonRootRepoPath::new(&workflow.path).map_err(|error| {
            format!(
                "workflow path `{}` is not a valid repository path: {error}",
                workflow.path
            )
        })?;
        let file = client
            .get_file_contents_at_ref(&plan.repo, &path, reference)
            .map_err(|error| {
                format!(
                    "failed to fetch workflow `{}` from `{}` at `{reference}`: {error}",
                    workflow.path, plan.repo
                )
            })?;
        let original = decode_repository_text_file(&file)?;
        let mut content = original.clone();
        let mut changes = Vec::with_capacity(workflow.pins.len());

        for pin in &workflow.pins {
            let resolved_sha =
                if let Some(existing) = resolved_shas.get(&pin.action.resolution_key()) {
                    existing.clone()
                } else {
                    let resolved = client
                        .resolve_commit_sha(&plan.repo, &pin.action.repo, &pin.action.version)
                        .map_err(|error| {
                            format!(
                                "failed to resolve `{}` to a commit SHA: {error}",
                                pin.action
                            )
                        })?;
                    resolved_shas.insert(pin.action.resolution_key(), resolved.clone());
                    resolved
                };

            let from = pin.action.to_string();
            let to = pin.action.rendered_with_version(&resolved_sha);
            let tag_comment = pin.action.tag_comment(&resolved_sha);
            let (updated_content, replacements, effective_comment) =
                replace_uses_line_value(&content, &from, &to, tag_comment)?;

            if replacements != pin.occurrences {
                return Err(format!(
                    "expected to update {} occurrence(s) of `{from}` in `{}`, updated {replacements}",
                    pin.occurrences, workflow.path
                ));
            }

            content = updated_content;
            changes.push(WorkflowPinChange {
                from,
                to,
                tag_comment: effective_comment,
            });
        }

        if content == original {
            return Err(format!(
                "workflow `{}` did not change during pinning",
                workflow.path
            ));
        }

        prepared.push(PreparedWorkflowUpdate {
            path: file.path,
            sha: file.sha,
            content,
            changes,
        });
    }

    Ok(prepared)
}

fn workflow_pin_pull_request_title() -> String {
    "Pin GitHub Actions to commit SHAs".to_owned()
}

fn workflow_pin_pull_request_body(updates: &[PreparedWorkflowUpdate]) -> String {
    let mut lines = vec![
        "Generated by github-infra.".to_owned(),
        String::new(),
        "Pins GitHub Actions references to immutable commit SHAs:".to_owned(),
    ];

    for update in updates {
        for change in &update.changes {
            let display_to = match &change.tag_comment {
                Some(tag) => format!("{} # {tag}", change.to),
                None => change.to.clone(),
            };
            lines.push(format!(
                "- `{}`: `{}` -> `{}`",
                update.path, change.from, display_to
            ));
        }
    }

    lines.join("\n")
}

fn workflow_pin_branch_name() -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("github-infra/pin-workflow-actions-{suffix}")
}

fn plan_rule_fix(facts: &RepoFacts, rule: &Rule, output: &RuleOutput) -> Option<PlannedFix> {
    let RuleResult::Fail { .. } = &output.result else {
        return None;
    };

    Some(PlannedFix {
        rule_id: output.id.clone(),
        rule_name: output.name.clone(),
        plan: match &rule.kind {
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::ForkPrApprovalPolicy,
                expected: SettingValue::ForkPrApprovalPolicy(Some(policy)),
            } => FixPlan::Effect(FixEffect::SetForkPrApprovalPolicy {
                repo: facts.repo.clone(),
                policy: policy.clone(),
            }),
            RuleKind::RepoSettingMatch { setting, expected } if setting.is_safe_to_auto_fix() => {
                FixPlan::Effect(FixEffect::SetRepositorySetting {
                    repo: facts.repo.clone(),
                    setting: setting.clone(),
                    value: expected
                        .as_bool()
                        .expect("auto-fix is gated to bool-valued repository settings"),
                })
            }
            RuleKind::RepoSettingMatch { setting, .. } => FixPlan::Rejected {
                reason: format!(
                    "automatic fixes for repository setting `{}` are not enabled",
                    setting.name()
                ),
            },
            RuleKind::WorkflowActionsPinnedToSha => plan_workflow_pin_pull_request(facts),
            RuleKind::RulesetRequiresLinearHistory => {
                plan_add_ruleset_rule(facts, RulesetRuleType::RequiredLinearHistory)
            }
            RuleKind::RulesetRestrictsDeletions => {
                plan_add_ruleset_rule(facts, RulesetRuleType::Deletion)
            }
            RuleKind::RulesetRequiresSignedCommits => {
                plan_add_ruleset_rule(facts, RulesetRuleType::RequiredSignatures)
            }
            RuleKind::RulesetRequiresPullRequest => {
                plan_add_ruleset_rule(facts, RulesetRuleType::PullRequest)
            }
            RuleKind::RulesetRestrictsMergeMethods { allowed } => {
                plan_set_pull_request_merge_methods(facts, allowed)
            }
            RuleKind::RulesetRequiresStrictStatusChecks => {
                plan_set_strict_required_status_checks(facts)
            }
            RuleKind::RulesetRequiresStatusCheck { check_name } => {
                plan_ensure_required_status_check(facts, check_name)
            }
            RuleKind::FileExists { path } if path == ENVRC_PATH => {
                FixPlan::Effect(FixEffect::OpenAddEnvrcPullRequest {
                    plan: AddEnvrcPullRequestPlan {
                        repo: facts.repo.clone(),
                        default_branch: facts.default_branch.clone(),
                    },
                })
            }
            RuleKind::UsesRulesetsNotLegacyProtection => {
                plan_delete_legacy_branch_protection(facts)
            }
            RuleKind::RulesetExists => plan_create_default_branch_ruleset(facts),
            _ => FixPlan::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            },
        },
    })
}

const DEFAULT_BRANCH_RULESET_NAME: &str = "github-infra: default branch protection";

fn plan_create_default_branch_ruleset(facts: &RepoFacts) -> FixPlan {
    if active_branch_rulesets_for_default_branch(facts)
        .next()
        .is_some()
    {
        return FixPlan::Rejected {
            reason: "internal error: ruleset creation planned despite an active branch ruleset \
                     already applying to the default branch"
                .to_owned(),
        };
    }

    FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset {
        repo: facts.repo.clone(),
        target: pending_default_branch_target(facts),
    })
}

fn pending_default_branch_target(facts: &RepoFacts) -> PlannedRulesetTarget {
    PlannedRulesetTarget::PendingDefaultBranch {
        default_branch: facts.default_branch.clone(),
        name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
    }
}

fn plan_add_ruleset_rule(facts: &RepoFacts, missing: RulesetRuleType) -> FixPlan {
    let missing_name = ruleset_rule_type_name(&missing);
    let candidates = active_branch_rulesets_for_default_branch(facts)
        .filter(|ruleset| !ruleset.rules.iter().any(|rule| rule.kind == missing))
        .collect::<Vec<_>>();

    let rules = vec![RulesetRule {
        kind: missing,
        parameters: None,
    }];

    match candidates.as_slice() {
        [] => FixPlan::Effect(FixEffect::AddRulesetRules {
            repo: facts.repo.clone(),
            target: pending_default_branch_target(facts),
            rules,
        }),
        [ruleset] => FixPlan::Effect(FixEffect::AddRulesetRules {
            repo: facts.repo.clone(),
            target: PlannedRulesetTarget::Existing {
                id: ruleset.id,
                name: ruleset.name.clone(),
            },
            rules,
        }),
        many => {
            let names = many
                .iter()
                .map(|ruleset| format!("`{}`", ruleset.name))
                .collect::<Vec<_>>()
                .join(", ");
            FixPlan::Rejected {
                reason: format!(
                    "multiple active branch rulesets apply to the default branch ({names}); add `{missing_name}` to one manually",
                ),
            }
        }
    }
}

fn plan_set_pull_request_merge_methods(facts: &RepoFacts, allowed: &[MergeMethod]) -> FixPlan {
    let desired = merge_method_string_set(allowed);
    let candidates = active_branch_rulesets_for_default_branch(facts)
        .filter(|ruleset| {
            ruleset
                .rules
                .iter()
                .find(|rule| rule.kind == RulesetRuleType::PullRequest)
                .map(|pr_rule| {
                    let actual = pr_rule
                        .parameters
                        .as_ref()
                        .map(|parameters| {
                            merge_method_string_set(&parameters.allowed_merge_methods)
                        })
                        .unwrap_or_default();
                    actual != desired
                })
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    match candidates.as_slice() {
        [] => FixPlan::Effect(FixEffect::SetRulesetPullRequestMergeMethods {
            repo: facts.repo.clone(),
            target: pending_default_branch_target(facts),
            allowed: allowed.to_vec(),
        }),
        [ruleset] => FixPlan::Effect(FixEffect::SetRulesetPullRequestMergeMethods {
            repo: facts.repo.clone(),
            target: PlannedRulesetTarget::Existing {
                id: ruleset.id,
                name: ruleset.name.clone(),
            },
            allowed: allowed.to_vec(),
        }),
        many => {
            let names = many
                .iter()
                .map(|ruleset| format!("`{}`", ruleset.name))
                .collect::<Vec<_>>()
                .join(", ");
            FixPlan::Rejected {
                reason: format!(
                    "multiple active branch rulesets apply to the default branch ({names}); set `pull_request.allowed_merge_methods` on one manually",
                ),
            }
        }
    }
}

fn merge_method_string_set(methods: &[MergeMethod]) -> std::collections::BTreeSet<String> {
    methods.iter().map(|m| String::from(m.clone())).collect()
}

fn plan_ensure_required_status_check(facts: &RepoFacts, context: &str) -> FixPlan {
    let candidates = active_branch_rulesets_for_default_branch(facts)
        .filter(|ruleset| {
            !ruleset.rules.iter().any(|rule| {
                rule.kind == RulesetRuleType::RequiredStatusChecks
                    && rule.parameters.as_ref().is_some_and(|parameters| {
                        parameters
                            .required_status_checks
                            .iter()
                            .any(|check| check.context == context)
                    })
            })
        })
        .collect::<Vec<_>>();

    match candidates.as_slice() {
        [] => FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck {
            repo: facts.repo.clone(),
            target: pending_default_branch_target(facts),
            context: context.to_owned(),
        }),
        [ruleset] => FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck {
            repo: facts.repo.clone(),
            target: PlannedRulesetTarget::Existing {
                id: ruleset.id,
                name: ruleset.name.clone(),
            },
            context: context.to_owned(),
        }),
        many => {
            let names = many
                .iter()
                .map(|ruleset| format!("`{}`", ruleset.name))
                .collect::<Vec<_>>()
                .join(", ");
            FixPlan::Rejected {
                reason: format!(
                    "multiple active branch rulesets apply to the default branch ({names}); add status check `{context}` to one manually",
                ),
            }
        }
    }
}

fn plan_set_strict_required_status_checks(facts: &RepoFacts) -> FixPlan {
    let candidates = active_branch_rulesets_for_default_branch(facts)
        .filter(|ruleset| {
            ruleset
                .rules
                .iter()
                .find(|rule| rule.kind == RulesetRuleType::RequiredStatusChecks)
                .map(|status_rule| {
                    status_rule
                        .parameters
                        .as_ref()
                        .and_then(|parameters| parameters.strict_required_status_checks_policy)
                        != Some(true)
                })
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();

    match candidates.as_slice() {
        [] => FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
            repo: facts.repo.clone(),
            target: pending_default_branch_target(facts),
        }),
        [ruleset] => FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
            repo: facts.repo.clone(),
            target: PlannedRulesetTarget::Existing {
                id: ruleset.id,
                name: ruleset.name.clone(),
            },
        }),
        many => {
            let names = many
                .iter()
                .map(|ruleset| format!("`{}`", ruleset.name))
                .collect::<Vec<_>>()
                .join(", ");
            FixPlan::Rejected {
                reason: format!(
                    "multiple active branch rulesets apply to the default branch ({names}); enable `strict_required_status_checks_policy` on one manually",
                ),
            }
        }
    }
}

fn plan_delete_legacy_branch_protection(facts: &RepoFacts) -> FixPlan {
    let Some(legacy) = facts.legacy_branch_protection.as_ref() else {
        return FixPlan::Rejected {
            reason: "internal error: no legacy branch protection present despite RS007 failure"
                .to_owned(),
        };
    };

    match legacy_protection_superseded_by_rulesets(legacy, facts) {
        Ok(()) => FixPlan::Effect(FixEffect::DeleteLegacyBranchProtection {
            repo: facts.repo.clone(),
            branch: facts.default_branch.clone(),
        }),
        Err(reasons) => FixPlan::Rejected {
            reason: format!(
                "legacy branch protection is not fully superseded by rulesets: {}",
                reasons.join("; ")
            ),
        },
    }
}

fn plan_workflow_pin_pull_request(facts: &RepoFacts) -> FixPlan {
    let mut workflows = Vec::new();
    let mut unsupported = Vec::new();
    let mut inline_flow_workflows = Vec::new();

    for workflow_file in &facts.workflows {
        let mut pins = Vec::new();

        for job in workflow_file.workflow.jobs.values() {
            for uses in job
                .uses()
                .into_iter()
                .chain(job.steps().iter().filter_map(|step| step.uses()))
            {
                if workflow_action_reference_is_pinned(uses) {
                    continue;
                }

                match repository_action_use_from_reference(uses) {
                    Some(action) => record_workflow_action_pin(&mut pins, action),
                    None => unsupported.push(format!(
                        "{} uses {}",
                        workflow_file.path,
                        action_reference_text(uses)
                    )),
                }
            }
        }

        if !pins.is_empty() {
            if let Some(raw_yaml) = workflow_file.raw_yaml.as_deref()
                && workflow_pins_have_inline_flow_refs(&pins, raw_yaml)
            {
                inline_flow_workflows.push(workflow_file.path.clone());
            }
            pins.sort_by_key(|left| left.action.to_string());
            workflows.push(WorkflowFilePins {
                path: workflow_file.path.clone(),
                pins,
            });
        }
    }

    workflows.sort_by(|left, right| left.path.cmp(&right.path));
    inline_flow_workflows.sort();

    if !inline_flow_workflows.is_empty() {
        return FixPlan::Rejected {
            reason: format!(
                "automatic fixes for workflow actions only support block-style YAML; rewrite {} to block style and re-run",
                summarize_examples(&inline_flow_workflows)
            ),
        };
    }

    if !unsupported.is_empty() {
        return FixPlan::Rejected {
            reason: format!(
                "automatic fixes for workflow actions only support literal repository action references: {}",
                summarize_examples(&unsupported)
            ),
        };
    }

    if workflows.is_empty() {
        return FixPlan::Rejected {
            reason: "automatic fix could not find any unpinned workflow actions to update"
                .to_owned(),
        };
    }

    FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest {
        plan: WorkflowPinPullRequestPlan {
            repo: facts.repo.clone(),
            default_branch: facts.default_branch.clone(),
            workflows,
        },
    })
}

fn record_workflow_action_pin(pins: &mut Vec<WorkflowActionPin>, action: RepositoryActionUse) {
    if let Some(existing) = pins.iter_mut().find(|pin| pin.action == action) {
        existing.occurrences += 1;
    } else {
        pins.push(WorkflowActionPin {
            action,
            occurrences: 1,
        });
    }
}

fn workflow_pins_have_inline_flow_refs(pins: &[WorkflowActionPin], raw_yaml: &str) -> bool {
    // The rewriter calls `replace_uses_line_value` with `pin.action.to_string()`
    // as the `from` string and expects exactly `pin.occurrences` matches. If a
    // workflow encodes a step in an inline flow mapping or a quoted-key form,
    // the regex finds fewer (typically zero) matches than the AST contains.
    // Detecting that here lets us reject the fix plan with a clear message
    // before opening an empty PR.
    pins.iter().any(|pin| {
        let pattern = crate::workflow::source::block_uses_line_regex(&pin.action.to_string());
        pattern.captures_iter(raw_yaml).count() < pin.occurrences
    })
}

fn repository_action_use_from_reference(uses: &ActionReference) -> Option<RepositoryActionUse> {
    let action = match uses {
        ActionReference::Repository(action_ref) if !is_commit_sha(&action_ref.version) => {
            Some(RepositoryActionUse::from_action_ref(action_ref))
        }
        ActionReference::Repository(_) => None,
        ActionReference::Other(raw) => parse_literal_repository_action_use(raw),
    }?;

    repository_action_use_is_literal(&action).then_some(action)
}

fn parse_literal_repository_action_use(raw: &str) -> Option<RepositoryActionUse> {
    if raw.starts_with("./") || raw.starts_with("docker://") || raw.matches('@').count() != 1 {
        return None;
    }

    let (path, version) = raw.rsplit_once('@')?;
    if version.is_empty() || version.contains("${{") || version.chars().any(char::is_whitespace) {
        return None;
    }

    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }

    let subpath = if segments.len() > 2 {
        Some(segments[2..].join("/"))
    } else {
        None
    };

    Some(RepositoryActionUse {
        repo: RepoRef::new(segments[0], segments[1]),
        subpath,
        version: version.to_owned(),
    })
}

fn repository_action_use_is_literal(action: &RepositoryActionUse) -> bool {
    literal_action_component(&action.repo.owner.to_string())
        && literal_action_component(&action.repo.name.to_string())
        && action
            .subpath
            .as_ref()
            .is_none_or(|subpath| literal_action_component(subpath))
        && literal_action_component(&action.version)
}

fn literal_action_component(value: &str) -> bool {
    !value.is_empty() && !value.contains("${{") && !value.chars().any(char::is_whitespace)
}

fn workflow_action_reference_is_pinned(uses: &ActionReference) -> bool {
    match uses {
        ActionReference::Repository(action_ref) => is_commit_sha(&action_ref.version),
        ActionReference::Other(raw) => {
            if raw.starts_with("./") || raw.starts_with("docker://") {
                true
            } else if let Some((_, version)) = raw.split_once('@') {
                is_commit_sha(version)
            } else {
                false
            }
        }
    }
}

fn action_reference_text(uses: &ActionReference) -> String {
    match uses {
        ActionReference::Repository(action_ref) => action_ref.to_string(),
        ActionReference::Other(raw) => raw.clone(),
    }
}

fn is_commit_sha(version: &str) -> bool {
    version.len() == 40 && version.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn replace_uses_line_value(
    text: &str,
    from: &str,
    to: &str,
    tag_comment: Option<&str>,
) -> Result<(String, usize, Option<String>), String> {
    let pattern = crate::workflow::source::block_uses_line_regex(from);

    let replacements = pattern.captures_iter(text).count();
    let effective_comment = std::cell::Cell::new(None);
    let updated = pattern
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let prefix = &captures[1];
            let close_quote = &captures[2];
            let existing_comment = &captures[3];
            let cr = &captures[4];

            let comment = match (tag_comment, existing_comment.contains('#')) {
                (Some(tag), false) => format!(" # {tag}"),
                _ => existing_comment.to_owned(),
            };
            let tag_text = comment
                .find('#')
                .map(|i| comment[i + 1..].trim().to_owned());
            effective_comment.set(tag_text);
            format!("{prefix}{to}{close_quote}{comment}{cr}")
        })
        .into_owned();

    Ok((updated, replacements, effective_comment.into_inner()))
}

fn decode_repository_text_file(file: &RepositoryFileContent) -> Result<String, String> {
    match &file.encoding {
        ContentEncoding::Base64 => {
            let compact = file
                .content
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|error| {
                    format!(
                        "failed to base64-decode `{}` from GitHub: {error}",
                        file.path
                    )
                })?;

            String::from_utf8(bytes).map_err(|error| {
                format!(
                    "workflow `{}` was not valid UTF-8 after decoding: {error}",
                    file.path
                )
            })
        }
        ContentEncoding::Utf8 => Ok(file.content.clone()),
        ContentEncoding::Unknown(encoding) => Err(format!(
            "workflow `{}` used unsupported encoding `{encoding}`",
            file.path
        )),
    }
}

fn cleanup_failed_workflow_pin_branch(
    client: &mut GitHubClient,
    plan: &WorkflowPinPullRequestPlan,
    branch_name: &str,
    failure: String,
) -> String {
    match client.delete_git_reference(&plan.repo, &format!("heads/{branch_name}")) {
        Ok(()) => failure,
        Err(cleanup_error) => format!(
            "{failure}; additionally failed to delete temporary branch `{branch_name}` in `{}`: {cleanup_error}",
            plan.repo
        ),
    }
}

fn apply_fix_effect_to_repository_update(
    update: &mut RepositoryUpdate,
    effect: &FixEffect,
) -> Option<String> {
    match effect {
        FixEffect::SetRepositorySetting { setting, value, .. } => {
            apply_repo_setting_update(update, setting, *value);
            None
        }
        FixEffect::OpenWorkflowPinPullRequest { .. }
        | FixEffect::OpenAddEnvrcPullRequest { .. }
        | FixEffect::AddRulesetRules { .. }
        | FixEffect::SetRulesetPullRequestMergeMethods { .. }
        | FixEffect::SetRulesetStrictRequiredStatusChecks { .. }
        | FixEffect::EnsureRulesetRequiredStatusCheck { .. }
        | FixEffect::SetForkPrApprovalPolicy { .. }
        | FixEffect::DeleteLegacyBranchProtection { .. }
        | FixEffect::CreateDefaultBranchRuleset { .. } => None,
    }
}

fn apply_repo_setting_update(update: &mut RepositoryUpdate, setting: &RepoSetting, value: bool) {
    match setting {
        RepoSetting::Private => update.private = Some(value),
        RepoSetting::Archived => update.archived = Some(value),
        RepoSetting::Disabled => update.disabled = Some(value),
        RepoSetting::AllowAutoMerge => update.allow_auto_merge = Some(value),
        RepoSetting::DeleteBranchOnMerge => update.delete_branch_on_merge = Some(value),
        RepoSetting::AllowUpdateBranch => update.allow_update_branch = Some(value),
        RepoSetting::AllowSquashMerge => update.allow_squash_merge = Some(value),
        RepoSetting::AllowMergeCommit => update.allow_merge_commit = Some(value),
        RepoSetting::AllowRebaseMerge => update.allow_rebase_merge = Some(value),
        RepoSetting::ForkPrApprovalPolicy => {
            unreachable!("fork PR contributor approval policy is not configurable via PATCH /repos")
        }
    }
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn summarize_examples(values: &[String]) -> String {
    const MAX_EXAMPLES: usize = 3;

    if values.len() <= MAX_EXAMPLES {
        values.join(", ")
    } else {
        let extra = values.len() - MAX_EXAMPLES;
        format!("{}, and {extra} more", values[..MAX_EXAMPLES].join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{RepoSettings, WorkflowFile};
    use crate::rules::{RepoSetting, Rule, SettingValue, default_rules};
    use crate::workflow::model::{
        ActionStep, Job, JobKind, ReusableJob, RunStep, StandardJob, Step, StepKind, Triggers,
        Workflow, WorkflowDispatch,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn bad_fixture() -> RepoFacts {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/example-org/bad-repo.json"
        )))
        .unwrap()
    }

    fn base_facts() -> RepoFacts {
        RepoFacts {
            repo: RepoRef::new("example-org", "repo"),
            settings: RepoSettings {
                private: false,
                archived: false,
                disabled: false,
                allow_auto_merge: false,
                delete_branch_on_merge: false,
                allow_update_branch: false,
                allow_squash_merge: false,
                allow_merge_commit: true,
                allow_rebase_merge: false,
                fork_pr_approval_policy: None,
            },
            rulesets: Vec::new(),
            legacy_branch_protection: None,
            default_branch: BranchName::new("main"),
            workflows: Vec::new(),
            files_present: BTreeSet::new(),
        }
    }

    fn workflow_with_action(path: &str, uses: ActionReference) -> WorkflowFile {
        let block_line = format!("      - uses: {}\n", action_reference_text(&uses));
        workflow_with_action_and_yaml(path, uses, block_line)
    }

    fn single_pin_bad_repo_facts() -> RepoFacts {
        let mut facts = base_facts();
        facts.repo = RepoRef::new("example-org", "bad-repo");
        facts.workflows.push(workflow_with_action(
            ".github/workflows/unsafe.yml",
            ActionReference::Repository(ActionRef::new("actions", "checkout", "v4")),
        ));
        facts
    }

    fn workflow_with_reusable_job(path: &str, uses: ActionReference) -> WorkflowFile {
        let raw_yaml = format!(
            "name: Reusable\non:\n  workflow_dispatch: {{}}\njobs:\n  call:\n    uses: {}\n",
            action_reference_text(&uses),
        );
        WorkflowFile {
            path: path.to_owned(),
            raw_yaml: Some(raw_yaml),
            workflow: Workflow {
                name: Some("Reusable".to_owned()),
                triggers: Triggers {
                    push: None,
                    pull_request: None,
                    pull_request_target: None,
                    workflow_run: None,
                    workflow_dispatch: Some(WorkflowDispatch::default()),
                },
                jobs: BTreeMap::from([(
                    "call".to_owned(),
                    Job {
                        needs: Vec::new(),
                        condition: None,
                        kind: JobKind::Reusable(ReusableJob {
                            uses,
                            with: BTreeMap::new(),
                        }),
                    },
                )]),
            },
        }
    }

    fn workflow_with_action_and_yaml(
        path: &str,
        uses: ActionReference,
        raw_yaml: String,
    ) -> WorkflowFile {
        WorkflowFile {
            path: path.to_owned(),
            raw_yaml: Some(raw_yaml),
            workflow: Workflow {
                name: Some("CI".to_owned()),
                triggers: Triggers {
                    push: None,
                    pull_request: None,
                    pull_request_target: None,
                    workflow_run: None,
                    workflow_dispatch: Some(WorkflowDispatch::default()),
                },
                jobs: BTreeMap::from([(
                    "build".to_owned(),
                    Job {
                        needs: Vec::new(),
                        condition: None,
                        kind: JobKind::Standard(StandardJob {
                            runs_on: None,
                            steps: vec![
                                Step {
                                    name: None,
                                    id: None,
                                    condition: None,
                                    kind: StepKind::Action(ActionStep {
                                        uses,
                                        with: BTreeMap::new(),
                                    }),
                                },
                                Step {
                                    name: None,
                                    id: None,
                                    condition: None,
                                    kind: StepKind::Run(RunStep {
                                        run: "echo ok".to_owned(),
                                    }),
                                },
                            ],
                        }),
                    },
                )]),
            },
        }
    }

    #[test]
    fn bad_fixture_plans_effects_and_rejections_for_failed_rules() {
        let facts = bad_fixture();
        let fixes = plan_repo_fixes(&default_rules(), &facts);
        let by_rule_id = fixes
            .iter()
            .map(|fix| (fix.rule_id.to_string(), fix))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(fixes.len(), 25);
        assert_eq!(
            by_rule_id["ST001"].plan,
            FixPlan::Effect(FixEffect::SetRepositorySetting {
                repo: facts.repo.clone(),
                setting: RepoSetting::AllowAutoMerge,
                value: true,
            })
        );
        assert_eq!(
            by_rule_id["ST007"].plan,
            FixPlan::Effect(FixEffect::SetForkPrApprovalPolicy {
                repo: facts.repo.clone(),
                policy: ForkPrApprovalPolicy::AllExternalContributors,
            })
        );
        assert_eq!(
            by_rule_id["ST007"].planned_report().description,
            "set fork PR contributor approval policy to `all_external_contributors`"
        );
        assert!(matches!(
            by_rule_id["WF002"].plan,
            FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { .. })
        ));
        assert_eq!(
            by_rule_id["WF002"].planned_report().description,
            "open a pull request that pins 2 workflow action references across 2 workflow files to commit SHAs"
        );
        assert_eq!(
            by_rule_id["ST004"].plan,
            FixPlan::Effect(FixEffect::SetRepositorySetting {
                repo: facts.repo.clone(),
                setting: RepoSetting::AllowMergeCommit,
                value: false,
            })
        );
        assert_eq!(
            by_rule_id["RS001"].plan,
            FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch: BranchName::new("main"),
                    name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
                },
            })
        );
        assert_eq!(
            by_rule_id["RS001"].planned_report().description,
            format!("create active branch ruleset `{DEFAULT_BRANCH_RULESET_NAME}` covering `main`",)
        );
        assert_eq!(
            by_rule_id["WF003"].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
        assert_eq!(
            by_rule_id["WF005"].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
        assert_eq!(
            by_rule_id["ST006"].planned_report().status,
            FixStatus::Planned
        );
    }

    #[test]
    fn rs001_plans_creation_when_no_active_branch_ruleset_covers_default() {
        let facts = base_facts();
        let rules = vec![Rule::new(
            "RS001",
            "Rulesets exist",
            RuleKind::RulesetExists,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch: facts.default_branch.clone(),
                    name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
                },
            })
        );
    }

    #[test]
    fn rs001_does_not_plan_when_an_active_ruleset_already_covers_default() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(1, "main protection", Vec::new())];

        let rules = vec![Rule::new(
            "RS001",
            "Rulesets exist",
            RuleKind::RulesetExists,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);

        // RS001 passes, so no fix is planned at all.
        assert!(fixes.is_empty());
    }

    #[test]
    fn risky_repo_setting_rules_are_rejected_instead_of_silently_dropped() {
        let facts = bad_fixture();
        let rules = vec![Rule::new(
            "ST999",
            "Repository is private",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::Private,
                expected: SettingValue::Bool(true),
            },
        )];
        let fixes = plan_repo_fixes(&rules, &facts);

        assert_eq!(
            fixes,
            vec![PlannedFix {
                rule_id: RuleId::new("ST999"),
                rule_name: "Repository is private".to_owned(),
                plan: FixPlan::Rejected {
                    reason: "automatic fixes for repository setting `private` are not enabled"
                        .to_owned(),
                },
            }]
        );
    }

    #[test]
    fn workflow_pin_fix_plans_pin_for_job_level_reusable_workflow() {
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_reusable_job(
            ".github/workflows/release-caller.yml",
            ActionReference::Other(
                "example-org/shared/.github/workflows/release.yml@main".to_owned(),
            ),
        ));

        let FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { plan }) =
            plan_workflow_pin_pull_request(&facts)
        else {
            panic!("expected an OpenWorkflowPinPullRequest effect for the unpinned reusable job");
        };

        assert_eq!(plan.workflows.len(), 1);
        let workflow_pins = &plan.workflows[0];
        assert_eq!(workflow_pins.path, ".github/workflows/release-caller.yml");
        assert_eq!(workflow_pins.pins.len(), 1);

        let pin = &workflow_pins.pins[0];
        assert_eq!(pin.action.repo.owner.to_string(), "example-org");
        assert_eq!(pin.action.repo.name.to_string(), "shared");
        assert_eq!(
            pin.action.subpath.as_deref(),
            Some(".github/workflows/release.yml")
        );
        assert_eq!(pin.action.version, "main");
        assert_eq!(pin.occurrences, 1);
    }

    #[test]
    fn workflow_pin_fix_rejects_non_literal_repository_action_references() {
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_action(
            ".github/workflows/ci.yml",
            ActionReference::Other(
                "owner/repo/path@feature@0123456789abcdef0123456789abcdef01234567".to_owned(),
            ),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert_eq!(
            fixes[0].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for workflow actions only support literal repository action references: .github/workflows/ci.yml uses owner/repo/path@feature@0123456789abcdef0123456789abcdef01234567".to_owned(),
            }
        );
    }

    #[test]
    fn workflow_pin_fix_rejects_repository_action_refs_with_expressions() {
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_action(
            ".github/workflows/ci.yml",
            ActionReference::Repository(ActionRef::new("${{ matrix.owner }}", "checkout", "v4")),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert_eq!(
            fixes[0].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for workflow actions only support literal repository action references: .github/workflows/ci.yml uses ${{ matrix.owner }}/checkout@v4".to_owned(),
            }
        );
    }

    #[test]
    fn workflow_pin_fix_rejects_inline_flow_yaml_workflows() {
        let mut facts = base_facts();
        // AST sees `actions/checkout@v3` but the raw yaml encodes the step as an
        // inline flow mapping that `replace_uses_line_value` cannot match.
        facts.workflows.push(workflow_with_action_and_yaml(
            ".github/workflows/rust.yml",
            ActionReference::Repository(ActionRef::new("actions", "checkout", "v3")),
            "on: workflow_dispatch\njobs:\n  build:\n    steps:\n      - { uses: actions/checkout@v3 }\n".to_owned(),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert_eq!(
            fixes[0].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for workflow actions only support block-style YAML; rewrite .github/workflows/rust.yml to block style and re-run".to_owned(),
            }
        );
    }

    #[test]
    fn workflow_pin_fix_rejects_inline_flow_yaml_with_subpath_action() {
        // Subpath actions like `owner/repo/path@v1` deserialize as
        // `ActionReference::Other`, but the planner still tries to pin them.
        // The inline-flow check must include them.
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_action_and_yaml(
            ".github/workflows/rust.yml",
            ActionReference::Other("docker/build-push-action/sub@v5".to_owned()),
            "on: workflow_dispatch\njobs:\n  build:\n    steps:\n      - { uses: docker/build-push-action/sub@v5 }\n".to_owned(),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert_eq!(
            fixes[0].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for workflow actions only support block-style YAML; rewrite .github/workflows/rust.yml to block style and re-run".to_owned(),
            }
        );
    }

    #[test]
    fn workflow_pin_fix_plans_when_raw_yaml_is_unavailable() {
        // Legacy snapshots predate the raw_yaml field. The planner cannot tell
        // whether the source is inline-flow, so it trusts the AST and plans
        // the fix — the rewriter will error at apply time if the source turns
        // out to be inline-flow.
        let mut facts = base_facts();
        let mut workflow = workflow_with_action(
            ".github/workflows/rust.yml",
            ActionReference::Repository(ActionRef::new("actions", "checkout", "v3")),
        );
        workflow.raw_yaml = None;
        facts.workflows.push(workflow);

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert!(matches!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { .. })
        ));
    }

    #[test]
    fn workflow_pin_fix_with_inline_flow_uses_not_first() {
        // Inline flow step where `uses` is not the first key.
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_action_and_yaml(
            ".github/workflows/rust.yml",
            ActionReference::Repository(ActionRef::new("actions", "checkout", "v3")),
            "on: workflow_dispatch\njobs:\n  build:\n    steps:\n      - { name: Checkout, uses: actions/checkout@v3 }\n".to_owned(),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert!(matches!(
            fixes[0].planned_report().status,
            FixStatus::Rejected { .. }
        ));
    }

    fn checkout_pin() -> WorkflowActionPin {
        WorkflowActionPin {
            action: RepositoryActionUse {
                repo: RepoRef::new("actions", "checkout"),
                subpath: None,
                version: "v3".to_owned(),
            },
            occurrences: 1,
        }
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_false_for_block_yaml() {
        let yaml = "      - uses: actions/checkout@v3\n";
        assert!(!workflow_pins_have_inline_flow_refs(
            &[checkout_pin()],
            yaml
        ));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_true_for_inline_mapping() {
        let yaml = "      - { uses: actions/checkout@v3 }\n";
        assert!(workflow_pins_have_inline_flow_refs(&[checkout_pin()], yaml));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_true_for_quoted_key() {
        let yaml = "      - \"uses\": \"actions/checkout@v3\"\n";
        assert!(workflow_pins_have_inline_flow_refs(&[checkout_pin()], yaml));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_false_when_block_match_appears_in_comment_too() {
        // Trailing comment text containing `"uses":` must not confuse the
        // regex; the line still matches the block-style pattern.
        let yaml = "      - uses: actions/checkout@v3 # was { \"uses\": \"v2\" }\n";
        assert!(!workflow_pins_have_inline_flow_refs(
            &[checkout_pin()],
            yaml
        ));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_ignores_block_scalar_substring() {
        // A `run:` block scalar may contain literal `"uses":` text. There is no
        // block-style `actions/checkout@v3` line, so the check trips.
        let yaml = "      - run: |\n          echo '{\"uses\": \"actions/checkout@v3\"}'\n";
        assert!(workflow_pins_have_inline_flow_refs(&[checkout_pin()], yaml));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_under_counted_when_one_form_each() {
        // Expected 2 occurrences but only one block-style line exists.
        let yaml = "      - uses: actions/checkout@v3\n      - { uses: actions/checkout@v3 }\n";
        let pin = WorkflowActionPin {
            action: RepositoryActionUse {
                repo: RepoRef::new("actions", "checkout"),
                subpath: None,
                version: "v3".to_owned(),
            },
            occurrences: 2,
        };
        assert!(workflow_pins_have_inline_flow_refs(&[pin], yaml));
    }

    #[test]
    fn workflow_pins_have_inline_flow_refs_true_for_subpath_inline_flow() {
        let yaml = "      - { uses: docker/build-push-action/sub@v5 }\n";
        let pin = WorkflowActionPin {
            action: RepositoryActionUse {
                repo: RepoRef::new("docker", "build-push-action"),
                subpath: Some("sub".to_owned()),
                version: "v5".to_owned(),
            },
            occurrences: 1,
        };
        assert!(workflow_pins_have_inline_flow_refs(&[pin], yaml));
    }

    #[test]
    fn replace_uses_line_value_preserves_quotes_and_existing_comments() {
        let source = "      - uses: \"actions/checkout@v4\" # keep me\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            None,
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: \"actions/checkout@0123456789abcdef0123456789abcdef01234567\" # keep me\n"
        );
        assert_eq!(effective_comment.as_deref(), Some("keep me"));
    }

    #[test]
    fn replace_uses_line_value_preserves_crlf_line_endings() {
        let source = "      - uses: actions/checkout@v4\r\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            None,
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567\r\n"
        );
        assert_eq!(effective_comment, None);
    }

    #[test]
    fn replace_uses_line_value_places_tag_comment_outside_quotes() {
        let source = "      - uses: \"actions/checkout@v4\"\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            Some("v4"),
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: \"actions/checkout@0123456789abcdef0123456789abcdef01234567\" # v4\n"
        );
        assert_eq!(effective_comment.as_deref(), Some("v4"));
    }

    #[test]
    fn replace_uses_line_value_tag_comment_unquoted() {
        let source = "      - uses: actions/checkout@v4\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            Some("v4"),
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: actions/checkout@0123456789abcdef0123456789abcdef01234567 # v4\n"
        );
        assert_eq!(effective_comment.as_deref(), Some("v4"));
    }

    #[test]
    fn replace_uses_line_value_preserves_existing_comment_over_tag() {
        let source = "      - uses: \"actions/checkout@v4\" # pinned for security\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            Some("v4"),
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: \"actions/checkout@0123456789abcdef0123456789abcdef01234567\" # pinned for security\n"
        );
        assert_eq!(effective_comment.as_deref(), Some("pinned for security"));
    }

    #[test]
    fn replace_uses_line_value_tag_comment_with_crlf() {
        let source = "      - uses: \"actions/checkout@v4\"\r\n";
        let (updated, replacements, effective_comment) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
            Some("v4"),
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: \"actions/checkout@0123456789abcdef0123456789abcdef01234567\" # v4\r\n"
        );
        assert_eq!(effective_comment.as_deref(), Some("v4"));
    }

    #[test]
    fn execute_repo_fixes_opens_pull_request_for_workflow_pins() {
        let facts = single_pin_bad_repo_facts();
        let rules = vec![Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);
        let resolved_sha = "0123456789abcdef0123456789abcdef01234567";
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let workflow_yaml = concat!(
            "name: Unsafe CI\n",
            "on:\n",
            "  pull_request_target:\n",
            "jobs:\n",
            "  build:\n",
            "    runs-on: ubuntu-latest\n",
            "    steps:\n",
            "      - uses: actions/checkout@v4\n",
            "      - run: echo unsafe\n",
        );
        let workflow_content = base64::engine::general_purpose::STANDARD.encode(workflow_yaml);
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml?ref=fedcba9876543210fedcba9876543210fedcba98",
                |_| {},
                format!(
                    r#"{{"name":"unsafe.yml","path":".github/workflows/unsafe.yml","sha":"blobsha","type":"file","encoding":"base64","content":"{workflow_content}"}}"#
                ),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/actions/checkout/commits/v4",
                |_| {},
                format!(r#"{{"sha":"{resolved_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/git/refs",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], default_branch_sha);
                    assert!(
                        json["ref"]
                            .as_str()
                            .unwrap()
                            .starts_with("refs/heads/github-infra/pin-workflow-actions-")
                    );
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], "blobsha");
                    assert!(
                        json["branch"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/pin-workflow-actions-")
                    );
                    let content = json["content"].as_str().unwrap();
                    let decoded = String::from_utf8(
                        base64::engine::general_purpose::STANDARD
                            .decode(content)
                            .unwrap(),
                    )
                    .unwrap();
                    assert!(decoded.contains(&format!("actions/checkout@{resolved_sha} # v4")));
                },
                "{}".to_owned(),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/pulls",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["title"], "Pin GitHub Actions to commit SHAs");
                    assert_eq!(json["base"], "main");
                    assert!(
                        json["head"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/pin-workflow-actions-")
                    );
                    assert!(json["body"].as_str().unwrap().contains(&format!(
                        "actions/checkout@v4` -> `actions/checkout@{resolved_sha} # v4"
                    )));
                },
                r#"{"number":42,"html_url":"https://example.test/pr/42"}"#.to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_pins_quoted_uses_with_comment_outside_quotes() {
        let facts = single_pin_bad_repo_facts();
        let rules = vec![Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);
        let resolved_sha = "0123456789abcdef0123456789abcdef01234567";
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let workflow_yaml = concat!(
            "name: Unsafe CI\n",
            "on:\n",
            "  pull_request_target:\n",
            "jobs:\n",
            "  build:\n",
            "    runs-on: ubuntu-latest\n",
            "    steps:\n",
            "      - uses: \"actions/checkout@v4\"\n",
            "      - run: echo unsafe\n",
        );
        let workflow_content = base64::engine::general_purpose::STANDARD.encode(workflow_yaml);
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml?ref=fedcba9876543210fedcba9876543210fedcba98",
                |_| {},
                format!(
                    r#"{{"name":"unsafe.yml","path":".github/workflows/unsafe.yml","sha":"blobsha","type":"file","encoding":"base64","content":"{workflow_content}"}}"#
                ),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/actions/checkout/commits/v4",
                |_| {},
                format!(r#"{{"sha":"{resolved_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/git/refs",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], default_branch_sha);
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let content = json["content"].as_str().unwrap();
                    let decoded = String::from_utf8(
                        base64::engine::general_purpose::STANDARD
                            .decode(content)
                            .unwrap(),
                    )
                    .unwrap();
                    // The tag comment must be OUTSIDE the quotes
                    assert!(
                        decoded.contains(&format!("\"actions/checkout@{resolved_sha}\" # v4")),
                        "expected tag comment outside quotes, got:\n{decoded}"
                    );
                    // Must NOT have the comment inside quotes
                    assert!(
                        !decoded.contains(&format!("\"actions/checkout@{resolved_sha} # v4\"")),
                        "tag comment must not be inside quotes, got:\n{decoded}"
                    );
                },
                "{}".to_owned(),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/pulls",
                |_| {},
                r#"{"number":43,"html_url":"https://example.test/pr/43"}"#.to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_deletes_temporary_branch_after_pull_request_failure() {
        let facts = single_pin_bad_repo_facts();
        let rules = vec![Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);
        let resolved_sha = "0123456789abcdef0123456789abcdef01234567";
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let workflow_yaml = concat!(
            "name: Unsafe CI\n",
            "on:\n",
            "  pull_request_target:\n",
            "jobs:\n",
            "  build:\n",
            "    runs-on: ubuntu-latest\n",
            "    steps:\n",
            "      - uses: actions/checkout@v4\n",
            "      - run: echo unsafe\n",
        );
        let workflow_content = base64::engine::general_purpose::STANDARD.encode(workflow_yaml);
        let branch_name = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let delete_branch_name = branch_name.clone();
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml?ref=fedcba9876543210fedcba9876543210fedcba98",
                |_| {},
                format!(
                    r#"{{"name":"unsafe.yml","path":".github/workflows/unsafe.yml","sha":"blobsha","type":"file","encoding":"base64","content":"{workflow_content}"}}"#
                ),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/actions/checkout/commits/v4",
                |_| {},
                format!(r#"{{"sha":"{resolved_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/git/refs",
                {
                    let branch_name = branch_name.clone();
                    move |body| {
                        let json: serde_json::Value = serde_json::from_str(body).unwrap();
                        let reference = json["ref"].as_str().unwrap();
                        let branch = reference.strip_prefix("refs/heads/").unwrap().to_owned();
                        *branch_name.lock().unwrap() = Some(branch);
                    }
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml",
                |_| {},
                "{}".to_owned(),
            ),
            ExpectedRequest::with_status_and_path_assertion(
                "POST",
                |path| assert_eq!(path, "/repos/example-org/bad-repo/pulls"),
                |_| {},
                500,
                "{}".to_owned(),
            ),
            ExpectedRequest::with_status_and_path_assertion(
                "DELETE",
                move |path| {
                    let branch = delete_branch_name
                        .lock()
                        .unwrap()
                        .clone()
                        .expect("branch name should have been captured");
                    assert_eq!(
                        path,
                        format!("/repos/example-org/bad-repo/git/refs/heads/{branch}")
                    );
                },
                |_| {},
                204,
                String::new(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(reason.contains("failed to open pull request for workflow action pinning"));
                assert!(!reason.contains("failed to delete temporary branch"));
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    fn ruleset_for_default_branch(
        id: u64,
        name: &str,
        rules: Vec<RulesetRule>,
    ) -> crate::github::types::Ruleset {
        use crate::github::types::{
            RefNameCondition, RulesetConditions, RulesetEnforcement, RulesetTarget,
        };

        crate::github::types::Ruleset {
            id,
            name: name.to_owned(),
            target: RulesetTarget::Branch,
            enforcement: RulesetEnforcement::Active,
            conditions: Some(RulesetConditions {
                ref_name: Some(RefNameCondition {
                    include: vec!["~DEFAULT_BRANCH".to_owned()],
                    exclude: Vec::new(),
                }),
            }),
            bypass_actors: Vec::new(),
            rules,
        }
    }

    fn rs005_rule() -> Rule {
        Rule::new(
            "RS005",
            "Rulesets require linear history",
            RuleKind::RulesetRequiresLinearHistory,
        )
    }

    #[test]
    fn add_ruleset_rule_fix_targets_sole_active_branch_ruleset() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];

        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::AddRulesetRules {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
                rules: vec![RulesetRule {
                    kind: RulesetRuleType::RequiredLinearHistory,
                    parameters: None,
                }],
            })
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "add 1 rule to ruleset `main protection`: `required_linear_history`"
        );
    }

    #[test]
    fn add_ruleset_rule_fix_rejects_when_no_ruleset_exists() {
        let facts = base_facts();

        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(
                    reason.contains("no active branch ruleset"),
                    "unexpected rejection reason: {reason}"
                );
                assert!(reason.contains("RS001"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn add_ruleset_rule_fix_rejects_when_multiple_rulesets_match() {
        let mut facts = base_facts();
        facts.rulesets = vec![
            ruleset_for_default_branch(1, "main protection", Vec::new()),
            ruleset_for_default_branch(2, "extra protection", Vec::new()),
        ];

        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(reason.contains("`main protection`"), "reason: {reason}");
                assert!(reason.contains("`extra protection`"), "reason: {reason}");
                assert!(reason.contains("required_linear_history"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn add_ruleset_rule_fix_not_planned_when_rule_already_present() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            1,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredLinearHistory,
                parameters: None,
            }],
        )];

        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        assert!(
            fixes.is_empty(),
            "expected no fixes (rule passes), got {fixes:?}"
        );
    }

    #[test]
    fn execute_repo_fixes_adds_required_linear_history_to_ruleset() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["name"], "main protection");
                    assert_eq!(json["target"], "branch");
                    assert_eq!(json["enforcement"], "active");
                    assert!(json.get("id").is_none(), "PUT body should omit id");
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1);
                    assert_eq!(rules[0]["type"], "required_linear_history");
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"required_linear_history"}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_no_ops_when_ruleset_already_has_the_rule() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        // GET returns a ruleset that already includes the rule (e.g. an
        // out-of-band fix has landed). The executor still issues the PUT
        // (idempotent) and reports Applied.
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[{"type":"required_linear_history"}]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(
                        rules.len(),
                        1,
                        "must not duplicate the existing required_linear_history entry"
                    );
                    assert_eq!(rules[0]["type"], "required_linear_history");
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"required_linear_history"}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_reports_failure_when_ruleset_put_fails() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs005_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[]}"#
                    .to_owned(),
            ),
            ExpectedRequest::with_status_and_path_assertion(
                "PUT",
                |path| assert_eq!(path, "/repos/example-org/repo/rulesets/42"),
                |_| {},
                500,
                "{}".to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(
                    reason.contains("failed to update ruleset"),
                    "unexpected failure reason: {reason}"
                );
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    fn rs011_rule() -> Rule {
        Rule::new(
            "RS011",
            "Pull-request rule allows only squash merges",
            RuleKind::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            },
        )
    }

    fn rs010_rule() -> Rule {
        Rule::new(
            "RS010",
            "Rulesets require a pull request",
            RuleKind::RulesetRequiresPullRequest,
        )
    }

    fn pull_request_rule_with_methods(methods: Vec<MergeMethod>) -> RulesetRule {
        RulesetRule {
            kind: RulesetRuleType::PullRequest,
            parameters: Some(RulesetRuleParameters {
                allowed_merge_methods: methods,
                ..RulesetRuleParameters::default()
            }),
        }
    }

    #[test]
    fn set_pull_request_merge_methods_fix_targets_sole_active_branch_ruleset_when_pr_rule_absent() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];

        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::SetRulesetPullRequestMergeMethods {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
                allowed: vec![MergeMethod::Squash],
            })
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "set `pull_request` allowed merge methods on ruleset `main protection` to: `squash`"
        );
    }

    #[test]
    fn set_pull_request_merge_methods_fix_targets_sole_active_branch_ruleset_when_pr_rule_present_with_wrong_methods()
     {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![pull_request_rule_with_methods(vec![
                MergeMethod::Merge,
                MergeMethod::Squash,
            ])],
        )];

        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::SetRulesetPullRequestMergeMethods {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
                allowed: vec![MergeMethod::Squash],
            })
        );
    }

    #[test]
    fn set_pull_request_merge_methods_fix_rejects_when_no_ruleset_exists() {
        let facts = base_facts();

        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(
                    reason.contains("no active branch ruleset"),
                    "unexpected rejection reason: {reason}"
                );
                assert!(reason.contains("RS001"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn set_pull_request_merge_methods_fix_rejects_when_multiple_rulesets_match() {
        let mut facts = base_facts();
        facts.rulesets = vec![
            ruleset_for_default_branch(1, "main protection", Vec::new()),
            ruleset_for_default_branch(2, "extra protection", Vec::new()),
        ];

        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(reason.contains("`main protection`"), "reason: {reason}");
                assert!(reason.contains("`extra protection`"), "reason: {reason}");
                assert!(reason.contains("allowed_merge_methods"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn set_pull_request_merge_methods_fix_not_planned_when_methods_already_match() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            1,
            "main protection",
            vec![pull_request_rule_with_methods(vec![MergeMethod::Squash])],
        )];

        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        assert!(
            fixes.is_empty(),
            "expected no fixes (rule passes), got {fixes:?}"
        );
    }

    #[test]
    fn execute_repo_fixes_preserves_other_pull_request_parameters_when_setting_merge_methods() {
        // The existing PR rule has additional parameters that the auto-fix must
        // NOT clobber. This is the load-bearing property of the in-place
        // mutation strategy.
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::PullRequest,
                parameters: Some(RulesetRuleParameters {
                    required_approving_review_count: Some(2),
                    require_code_owner_review: Some(true),
                    allowed_merge_methods: vec![MergeMethod::Merge, MergeMethod::Squash],
                    ..RulesetRuleParameters::default()
                }),
            }],
        )];
        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[{"type":"pull_request","parameters":{"required_approving_review_count":2,"require_code_owner_review":true,"allowed_merge_methods":["merge","squash"]}}]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1);
                    assert_eq!(rules[0]["type"], "pull_request");
                    let parameters = &rules[0]["parameters"];
                    assert_eq!(parameters["allowed_merge_methods"], serde_json::json!(["squash"]));
                    assert_eq!(parameters["required_approving_review_count"], 2);
                    assert_eq!(parameters["require_code_owner_review"], true);
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"pull_request","parameters":{"required_approving_review_count":2,"require_code_owner_review":true,"allowed_merge_methods":["squash"]}}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_adds_pull_request_rule_with_squash_only_when_absent() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs011_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1);
                    assert_eq!(rules[0]["type"], "pull_request");
                    assert_eq!(
                        rules[0]["parameters"]["allowed_merge_methods"],
                        serde_json::json!(["squash"])
                    );
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"pull_request","parameters":{"allowed_merge_methods":["squash"]}}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_batches_rs010_and_rs011_into_one_ruleset_put() {
        // When both RS010 (add pull_request rule) and RS011 (set
        // allowed_merge_methods) fail on the same ruleset, the fixes must be
        // merged into a single PUT that introduces the pull_request rule with
        // the desired merge methods.
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs010_rule(), rs011_rule()], &facts);
        assert_eq!(fixes.len(), 2);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1, "expected a single pull_request rule");
                    assert_eq!(rules[0]["type"], "pull_request");
                    assert_eq!(
                        rules[0]["parameters"]["allowed_merge_methods"],
                        serde_json::json!(["squash"])
                    );
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"pull_request","parameters":{"allowed_merge_methods":["squash"]}}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 2);
        assert_eq!(executed[0].status, FixStatus::Applied);
        assert_eq!(executed[1].status, FixStatus::Applied);
    }

    fn rs013_rule() -> Rule {
        Rule::new(
            "RS013",
            "Branches must be up-to-date before merging",
            RuleKind::RulesetRequiresStrictStatusChecks,
        )
    }

    fn required_status_checks_rule(strict: Option<bool>) -> RulesetRule {
        RulesetRule {
            kind: RulesetRuleType::RequiredStatusChecks,
            parameters: Some(RulesetRuleParameters {
                strict_required_status_checks_policy: strict,
                ..RulesetRuleParameters::default()
            }),
        }
    }

    #[test]
    fn set_strict_required_status_checks_fix_targets_sole_active_branch_ruleset() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![required_status_checks_rule(Some(false))],
        )];

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
            })
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "enable `strict_required_status_checks_policy` on ruleset `main protection`"
        );
    }

    #[test]
    fn set_strict_required_status_checks_fix_targets_ruleset_when_strict_unset() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![required_status_checks_rule(None)],
        )];

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
            })
        );
    }

    #[test]
    fn set_strict_required_status_checks_fix_targets_ruleset_when_status_check_rule_absent() {
        // RS013 should plan an effect even when the ruleset has no
        // required_status_checks rule yet. The apply step creates the rule
        // (with strict=true and an empty checks list); RS012 fixes that
        // batch alongside RS013 will populate the checks list.
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
            })
        );
    }

    #[test]
    fn set_strict_required_status_checks_fix_rejects_when_no_ruleset_exists() {
        let facts = base_facts();

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(
                    reason.contains("no active branch ruleset"),
                    "unexpected rejection reason: {reason}"
                );
                assert!(reason.contains("RS001"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn set_strict_required_status_checks_fix_rejects_when_multiple_rulesets_match() {
        let mut facts = base_facts();
        facts.rulesets = vec![
            ruleset_for_default_branch(
                1,
                "main protection",
                vec![required_status_checks_rule(Some(false))],
            ),
            ruleset_for_default_branch(
                2,
                "extra protection",
                vec![required_status_checks_rule(None)],
            ),
        ];

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(reason.contains("`main protection`"), "reason: {reason}");
                assert!(reason.contains("`extra protection`"), "reason: {reason}");
                assert!(reason.contains("strict_required_status_checks_policy"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn set_strict_required_status_checks_fix_not_planned_when_already_strict() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![required_status_checks_rule(Some(true))],
        )];

        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        assert!(
            fixes.is_empty(),
            "expected no fixes (rule passes), got {fixes:?}"
        );
    }

    #[test]
    fn execute_repo_fixes_enables_strict_required_status_checks_preserving_other_parameters() {
        // The auto-fix must mutate the existing `required_status_checks` rule
        // in place, preserving other parameters (e.g. the list of required
        // checks).
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![required_status_checks_rule(Some(false))],
        )];
        let fixes = plan_repo_fixes(&[rs013_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[{"context":"ci","integration_id":1}]}}]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1);
                    assert_eq!(rules[0]["type"], "required_status_checks");
                    let parameters = &rules[0]["parameters"];
                    assert_eq!(parameters["strict_required_status_checks_policy"], true);
                    assert_eq!(
                        parameters["required_status_checks"],
                        serde_json::json!([{"context":"ci","integration_id":1}])
                    );
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"ci","integration_id":1}]}}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_batches_rs011_and_rs013_into_one_ruleset_put() {
        // When both RS011 (set allowed_merge_methods) and RS013 (enable strict
        // status checks) fail on the same ruleset, the fixes must be merged
        // into a single PUT.
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![required_status_checks_rule(Some(false))],
        )];
        let fixes = plan_repo_fixes(&[rs011_rule(), rs013_rule()], &facts);
        assert_eq!(fixes.len(), 2);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":false,"required_status_checks":[]}}]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 2);
                    let by_type: std::collections::HashMap<&str, &serde_json::Value> = rules
                        .iter()
                        .map(|rule| (rule["type"].as_str().unwrap(), rule))
                        .collect();
                    let status_rule = by_type
                        .get("required_status_checks")
                        .expect("required_status_checks rule should be present");
                    assert_eq!(
                        status_rule["parameters"]["strict_required_status_checks_policy"],
                        true
                    );
                    let pr_rule = by_type
                        .get("pull_request")
                        .expect("pull_request rule should be present");
                    assert_eq!(
                        pr_rule["parameters"]["allowed_merge_methods"],
                        serde_json::json!(["squash"])
                    );
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[]}},{"type":"pull_request","parameters":{"allowed_merge_methods":["squash"]}}]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 2);
        assert_eq!(executed[0].status, FixStatus::Applied);
        assert_eq!(executed[1].status, FixStatus::Applied);
    }

    fn rs012_rule() -> Rule {
        Rule::new(
            "RS012",
            "all-required-checks-complete status check is required",
            RuleKind::RulesetRequiresStatusCheck {
                check_name: "all-required-checks-complete".to_owned(),
            },
        )
    }

    #[test]
    fn ensure_required_status_check_fix_targets_sole_active_branch_ruleset_when_rule_absent() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];

        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
                context: "all-required-checks-complete".to_owned(),
            })
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "require status check `all-required-checks-complete` on ruleset `main protection`"
        );
    }

    #[test]
    fn ensure_required_status_check_fix_targets_ruleset_when_other_check_present() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredStatusChecks,
                parameters: Some(RulesetRuleParameters {
                    required_status_checks: vec![crate::github::types::RequiredStatusCheck {
                        context: "other".to_owned(),
                        integration_id: None,
                    }],
                    ..RulesetRuleParameters::default()
                }),
            }],
        )];

        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::Existing {
                    id: 42,
                    name: "main protection".to_owned(),
                },
                context: "all-required-checks-complete".to_owned(),
            })
        );
    }

    #[test]
    fn ensure_required_status_check_fix_not_planned_when_check_already_required() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredStatusChecks,
                parameters: Some(RulesetRuleParameters {
                    required_status_checks: vec![crate::github::types::RequiredStatusCheck {
                        context: "all-required-checks-complete".to_owned(),
                        integration_id: None,
                    }],
                    ..RulesetRuleParameters::default()
                }),
            }],
        )];

        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        assert!(
            fixes.is_empty(),
            "expected no fixes (rule passes), got {fixes:?}"
        );
    }

    #[test]
    fn ensure_required_status_check_fix_rejects_when_no_ruleset_exists() {
        let facts = base_facts();

        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(
                    reason.contains("no active branch ruleset"),
                    "unexpected rejection reason: {reason}"
                );
                assert!(reason.contains("RS001"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn ensure_required_status_check_fix_rejects_when_multiple_rulesets_match() {
        let mut facts = base_facts();
        facts.rulesets = vec![
            ruleset_for_default_branch(1, "main protection", Vec::new()),
            ruleset_for_default_branch(2, "extra protection", Vec::new()),
        ];

        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(reason.contains("`main protection`"), "reason: {reason}");
                assert!(reason.contains("`extra protection`"), "reason: {reason}");
                assert!(reason.contains("`all-required-checks-complete`"));
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn execute_repo_fixes_adds_required_status_check_to_existing_rule() {
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredStatusChecks,
                parameters: Some(RulesetRuleParameters {
                    required_status_checks: vec![crate::github::types::RequiredStatusCheck {
                        context: "other".to_owned(),
                        integration_id: Some(7),
                    }],
                    strict_required_status_checks_policy: Some(true),
                    ..RulesetRuleParameters::default()
                }),
            }],
        )];
        let fixes = plan_repo_fixes(&[rs012_rule()], &facts);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[{"type":"required_status_checks","parameters":{"strict_required_status_checks_policy":true,"required_status_checks":[{"context":"other","integration_id":7}]}}]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1);
                    let params = &rules[0]["parameters"];
                    let checks = params["required_status_checks"].as_array().unwrap();
                    let contexts: std::collections::BTreeSet<&str> = checks
                        .iter()
                        .map(|check| check["context"].as_str().unwrap())
                        .collect();
                    assert_eq!(
                        contexts,
                        ["all-required-checks-complete", "other"].into_iter().collect()
                    );
                    // The existing integration_id on "other" must be preserved.
                    let other_check = checks
                        .iter()
                        .find(|check| check["context"] == "other")
                        .unwrap();
                    assert_eq!(other_check["integration_id"], 7);
                    assert_eq!(params["strict_required_status_checks_policy"], true);
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_batches_rs012_rs013_into_one_ruleset_put() {
        // The ruleset has no required_status_checks rule at all. RS012 and
        // RS013 both fail. The auto-fix must create the rule in a single PUT
        // with the context and strict=true.
        let mut facts = base_facts();
        facts.rulesets = vec![ruleset_for_default_branch(
            42,
            "main protection",
            Vec::new(),
        )];
        let fixes = plan_repo_fixes(&[rs012_rule(), rs013_rule()], &facts);
        assert_eq!(fixes.len(), 2);

        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/rulesets/42",
                |_| {},
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}},"bypass_actors":[],"rules":[]}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/rulesets/42",
                |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    let rules = json["rules"].as_array().unwrap();
                    assert_eq!(rules.len(), 1, "expected exactly one rule (required_status_checks)");
                    assert_eq!(rules[0]["type"], "required_status_checks");
                    let params = &rules[0]["parameters"];
                    assert_eq!(params["strict_required_status_checks_policy"], true);
                    let checks = params["required_status_checks"].as_array().unwrap();
                    let contexts: std::collections::BTreeSet<&str> = checks
                        .iter()
                        .map(|check| check["context"].as_str().unwrap())
                        .collect();
                    assert_eq!(
                        contexts,
                        ["all-required-checks-complete"].into_iter().collect()
                    );
                },
                r#"{"id":42,"name":"main protection","target":"branch","enforcement":"active","rules":[]}"#
                    .to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 2);
        assert_eq!(executed[0].status, FixStatus::Applied);
        assert_eq!(executed[1].status, FixStatus::Applied);
    }

    fn st007_rule() -> Rule {
        Rule::new(
            "ST007",
            "Fork PR workflows require approval for all external contributors",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::ForkPrApprovalPolicy,
                expected: SettingValue::ForkPrApprovalPolicy(Some(
                    ForkPrApprovalPolicy::AllExternalContributors,
                )),
            },
        )
    }

    #[test]
    fn execute_repo_fixes_sets_fork_pr_approval_policy() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[st007_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::with_status_and_path_assertion(
            "PUT",
            |path| {
                assert_eq!(
                    path,
                    "/repos/example-org/repo/actions/permissions/fork-pr-contributor-approval"
                );
            },
            |body| {
                let json: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(json["approval_policy"], "all_external_contributors");
            },
            204,
            String::new(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_reports_failure_when_fork_pr_approval_put_fails() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[st007_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::with_status_and_path_assertion(
            "PUT",
            |path| {
                assert_eq!(
                    path,
                    "/repos/example-org/repo/actions/permissions/fork-pr-contributor-approval"
                );
            },
            |_| {},
            500,
            "{}".to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(
                    reason.contains("fork-pr-contributor-approval"),
                    "unexpected failure reason: {reason}"
                );
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    fn rs007_rule() -> Rule {
        Rule::new(
            "RS007",
            "Repository uses rulesets instead of legacy protection",
            RuleKind::UsesRulesetsNotLegacyProtection,
        )
    }

    fn linear_history_legacy() -> crate::github::types::BranchProtection {
        use crate::github::types::{BranchProtection, LegacyEnabledFlag};
        BranchProtection {
            required_linear_history: Some(LegacyEnabledFlag { enabled: true }),
            allow_force_pushes: Some(LegacyEnabledFlag { enabled: true }),
            allow_deletions: Some(LegacyEnabledFlag { enabled: true }),
            ..BranchProtection::default()
        }
    }

    #[test]
    fn delete_legacy_branch_protection_rejected_when_not_superseded() {
        let mut facts = base_facts();
        facts.legacy_branch_protection = Some(linear_history_legacy());
        facts.rulesets = vec![ruleset_for_default_branch(1, "main protection", Vec::new())];

        let fixes = plan_repo_fixes(&[rs007_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        match &fixes[0].plan {
            FixPlan::Rejected { reason } => {
                assert!(
                    reason.contains("not fully superseded"),
                    "unexpected rejection reason: {reason}"
                );
                assert!(
                    reason.contains("required_linear_history"),
                    "rejection should mention the missing rule: {reason}"
                );
            }
            other => panic!("expected rejection, got {other:?}"),
        }
    }

    #[test]
    fn delete_legacy_branch_protection_planned_when_superseded() {
        let mut facts = base_facts();
        facts.legacy_branch_protection = Some(linear_history_legacy());
        facts.rulesets = vec![ruleset_for_default_branch(
            1,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredLinearHistory,
                parameters: None,
            }],
        )];

        let fixes = plan_repo_fixes(&[rs007_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::DeleteLegacyBranchProtection {
                repo: facts.repo.clone(),
                branch: facts.default_branch.clone(),
            }),
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "delete legacy branch protection on `main`"
        );
    }

    #[test]
    fn execute_repo_fixes_deletes_legacy_branch_protection() {
        let mut facts = base_facts();
        facts.legacy_branch_protection = Some(linear_history_legacy());
        facts.rulesets = vec![ruleset_for_default_branch(
            1,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredLinearHistory,
                parameters: None,
            }],
        )];
        let fixes = plan_repo_fixes(&[rs007_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::with_status_and_path_assertion(
            "DELETE",
            |path| {
                assert_eq!(path, "/repos/example-org/repo/branches/main/protection");
            },
            |_| {},
            204,
            String::new(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_reports_failure_when_delete_branch_protection_fails() {
        let mut facts = base_facts();
        facts.legacy_branch_protection = Some(linear_history_legacy());
        facts.rulesets = vec![ruleset_for_default_branch(
            1,
            "main protection",
            vec![RulesetRule {
                kind: RulesetRuleType::RequiredLinearHistory,
                parameters: None,
            }],
        )];
        let fixes = plan_repo_fixes(&[rs007_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::with_status_and_path_assertion(
            "DELETE",
            |path| {
                assert_eq!(path, "/repos/example-org/repo/branches/main/protection");
            },
            |_| {},
            500,
            "{}".to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(
                    reason.contains("branches/main/protection"),
                    "unexpected failure reason: {reason}"
                );
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    fn rs001_rule() -> Rule {
        Rule::new("RS001", "Rulesets exist", RuleKind::RulesetExists)
    }

    #[test]
    fn execute_repo_fixes_creates_default_branch_ruleset() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs001_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::json(
            "POST",
            "/repos/example-org/repo/rulesets",
            |body| {
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(value["name"], DEFAULT_BRANCH_RULESET_NAME);
                assert_eq!(value["target"], "branch");
                assert_eq!(value["enforcement"], "active");
                assert_eq!(value["conditions"]["ref_name"]["include"][0], "~DEFAULT_BRANCH");
                assert_eq!(value["conditions"]["ref_name"]["exclude"].as_array().unwrap().len(), 0);
                assert_eq!(value["bypass_actors"].as_array().unwrap().len(), 0);
                assert_eq!(value["rules"].as_array().unwrap().len(), 0);
            },
            r#"{"id":99,"name":"github-infra: default branch protection","target":"branch","enforcement":"active"}"#.to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].rule_id.to_string(), "RS001");
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_reports_failure_when_create_ruleset_fails() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs001_rule()], &facts);

        let server = TestServer::spawn(vec![ExpectedRequest::with_status_and_path_assertion(
            "POST",
            |path| {
                assert_eq!(path, "/repos/example-org/repo/rulesets");
            },
            |_| {},
            500,
            "{}".to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(
                    reason.contains("failed to create ruleset"),
                    "unexpected failure reason: {reason}"
                );
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    #[test]
    fn rs001_plus_rs012_rs013_plan_pending_default_branch_targets() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs001_rule(), rs012_rule(), rs013_rule()], &facts);

        let by_rule_id: BTreeMap<_, _> = fixes
            .iter()
            .map(|fix| (fix.rule_id.to_string(), fix))
            .collect();

        assert_eq!(
            by_rule_id["RS001"].plan,
            FixPlan::Effect(FixEffect::CreateDefaultBranchRuleset {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch: facts.default_branch.clone(),
                    name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
                },
            })
        );
        assert_eq!(
            by_rule_id["RS012"].plan,
            FixPlan::Effect(FixEffect::EnsureRulesetRequiredStatusCheck {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch: facts.default_branch.clone(),
                    name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
                },
                context: "all-required-checks-complete".to_owned(),
            })
        );
        assert_eq!(
            by_rule_id["RS013"].plan,
            FixPlan::Effect(FixEffect::SetRulesetStrictRequiredStatusChecks {
                repo: facts.repo.clone(),
                target: PlannedRulesetTarget::PendingDefaultBranch {
                    default_branch: facts.default_branch.clone(),
                    name: DEFAULT_BRANCH_RULESET_NAME.to_owned(),
                },
            })
        );
    }

    #[test]
    fn execute_repo_fixes_batches_rs001_rs012_rs013_into_one_creation_post() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs001_rule(), rs012_rule(), rs013_rule()], &facts);
        assert_eq!(fixes.len(), 3);

        let server = TestServer::spawn(vec![ExpectedRequest::json(
            "POST",
            "/repos/example-org/repo/rulesets",
            |body| {
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(value["name"], DEFAULT_BRANCH_RULESET_NAME);
                assert_eq!(value["target"], "branch");
                assert_eq!(value["enforcement"], "active");
                assert_eq!(value["conditions"]["ref_name"]["include"][0], "~DEFAULT_BRANCH");

                let rules = value["rules"].as_array().unwrap();
                assert_eq!(rules.len(), 1, "expected single required_status_checks rule");
                assert_eq!(rules[0]["type"], "required_status_checks");
                let params = &rules[0]["parameters"];
                assert_eq!(params["strict_required_status_checks_policy"], true);
                let contexts: BTreeSet<&str> = params["required_status_checks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|check| check["context"].as_str().unwrap())
                    .collect();
                assert_eq!(contexts, ["all-required-checks-complete"].into_iter().collect());
            },
            r#"{"id":99,"name":"github-infra: default branch protection","target":"branch","enforcement":"active"}"#.to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 3);
        for fix in &executed {
            assert_eq!(
                fix.status,
                FixStatus::Applied,
                "rule {} expected Applied, got {:?}",
                fix.rule_id,
                fix.status
            );
        }
    }

    #[test]
    fn creation_post_supplies_required_defaults_for_new_pull_request_rule() {
        // Reproduces the live 422 we hit: GitHub's create-ruleset endpoint
        // rejects a pull_request rule that omits required_approving_review_count
        // and the dismiss/require_* booleans, even when only allowed_merge_methods
        // is being set. The autofix must supply permissive defaults.
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs001_rule(), rs010_rule(), rs011_rule()], &facts);
        assert_eq!(fixes.len(), 3);

        let server = TestServer::spawn(vec![ExpectedRequest::json(
            "POST",
            "/repos/example-org/repo/rulesets",
            |body| {
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                let rules = value["rules"].as_array().unwrap();
                let pr_rule = rules
                    .iter()
                    .find(|rule| rule["type"] == "pull_request")
                    .expect("expected pull_request rule in body");
                let params = &pr_rule["parameters"];
                assert_eq!(params["required_approving_review_count"], 0);
                assert_eq!(params["dismiss_stale_reviews_on_push"], false);
                assert_eq!(params["require_code_owner_review"], false);
                assert_eq!(params["require_last_push_approval"], false);
                assert_eq!(params["required_review_thread_resolution"], false);
                let methods: Vec<&str> = params["allowed_merge_methods"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|method| method.as_str().unwrap())
                    .collect();
                assert_eq!(methods, vec!["squash"]);
            },
            r#"{"id":99,"name":"github-infra: default branch protection","target":"branch","enforcement":"active"}"#.to_owned(),
        )]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 3);
        for fix in &executed {
            assert_eq!(
                fix.status,
                FixStatus::Applied,
                "rule {} expected Applied, got {:?}",
                fix.rule_id,
                fix.status
            );
        }
    }

    #[test]
    fn population_rules_rejected_when_rs001_absent_and_no_ruleset_exists() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[rs012_rule(), rs013_rule()], &facts);

        assert_eq!(fixes.len(), 2);
        for fix in &fixes {
            match &fix.plan {
                FixPlan::Rejected { reason } => {
                    assert!(
                        reason.contains("no active branch ruleset"),
                        "rule {} unexpected reason: {reason}",
                        fix.rule_id,
                    );
                    assert!(
                        reason.contains("RS001"),
                        "rule {} unexpected reason: {reason}",
                        fix.rule_id,
                    );
                }
                other => panic!("rule {} expected rejection, got {other:?}", fix.rule_id),
            }
        }
    }

    fn fl001_rule() -> Rule {
        Rule::new(
            "FL001",
            "`.envrc` exists",
            RuleKind::FileExists {
                path: ENVRC_PATH.to_owned(),
            },
        )
    }

    #[test]
    fn add_envrc_fix_is_planned_when_envrc_missing() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[fl001_rule()], &facts);

        assert_eq!(fixes.len(), 1);
        assert_eq!(
            fixes[0].plan,
            FixPlan::Effect(FixEffect::OpenAddEnvrcPullRequest {
                plan: AddEnvrcPullRequestPlan {
                    repo: facts.repo.clone(),
                    default_branch: facts.default_branch.clone(),
                },
            })
        );
        assert_eq!(
            fixes[0].planned_report().description,
            "open a pull request that adds `.envrc` with `use flake`"
        );
    }

    #[test]
    fn add_envrc_fix_not_planned_when_envrc_present() {
        let mut facts = base_facts();
        facts.files_present.insert(ENVRC_PATH.to_owned());

        let fixes = plan_repo_fixes(&[fl001_rule()], &facts);

        assert!(
            fixes.is_empty(),
            "expected no fixes (rule passes), got {fixes:?}"
        );
    }

    #[test]
    fn file_exists_rule_for_other_path_is_rejected() {
        let facts = base_facts();
        let rule = Rule::new(
            "FL999",
            "`CODEOWNERS` exists",
            RuleKind::FileExists {
                path: "CODEOWNERS".to_owned(),
            },
        );

        let fixes = plan_repo_fixes(&[rule], &facts);

        assert_eq!(fixes.len(), 1);
        assert!(matches!(fixes[0].plan, FixPlan::Rejected { .. }));
    }

    #[test]
    fn execute_repo_fixes_opens_pull_request_for_add_envrc() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[fl001_rule()], &facts);
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let expected_envrc_content =
            base64::engine::general_purpose::STANDARD.encode(b"use flake\n");
        let expected_envrc_content_for_assert = expected_envrc_content.clone();
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/repo/git/refs",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], default_branch_sha);
                    assert!(
                        json["ref"]
                            .as_str()
                            .unwrap()
                            .starts_with("refs/heads/github-infra/add-envrc-")
                    );
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/contents/.envrc",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["content"], expected_envrc_content_for_assert);
                    assert!(
                        json.get("sha").is_none(),
                        "PUT body for new file must omit sha; got {body}"
                    );
                    assert!(
                        json["branch"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/add-envrc-")
                    );
                    assert_eq!(json["message"], "Add `.envrc`");
                },
                "{}".to_owned(),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/repo/pulls",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["title"], "Add `.envrc`");
                    assert_eq!(json["base"], "main");
                    assert!(
                        json["head"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/add-envrc-")
                    );
                    assert!(json["body"].as_str().unwrap().contains("use flake"));
                },
                r#"{"number":99,"html_url":"https://example.test/pr/99"}"#.to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    #[test]
    fn execute_repo_fixes_deletes_temporary_branch_after_envrc_pull_request_failure() {
        let facts = base_facts();
        let fixes = plan_repo_fixes(&[fl001_rule()], &facts);
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let branch_name = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let delete_branch_name = branch_name.clone();
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/repo/git/refs",
                {
                    let branch_name = branch_name.clone();
                    move |body| {
                        let json: serde_json::Value = serde_json::from_str(body).unwrap();
                        let reference = json["ref"].as_str().unwrap();
                        let branch = reference.strip_prefix("refs/heads/").unwrap().to_owned();
                        *branch_name.lock().unwrap() = Some(branch);
                    }
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/repo/contents/.envrc",
                |_| {},
                "{}".to_owned(),
            ),
            ExpectedRequest::with_status_and_path_assertion(
                "POST",
                |path| assert_eq!(path, "/repos/example-org/repo/pulls"),
                |_| {},
                500,
                "{}".to_owned(),
            ),
            ExpectedRequest::with_status_and_path_assertion(
                "DELETE",
                move |path| {
                    let branch = delete_branch_name
                        .lock()
                        .unwrap()
                        .clone()
                        .expect("branch name should have been captured");
                    assert_eq!(
                        path,
                        format!("/repos/example-org/repo/git/refs/heads/{branch}")
                    );
                },
                |_| {},
                204,
                String::new(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        match &executed[0].status {
            FixStatus::Failed { reason } => {
                assert!(
                    reason.contains("failed to open pull request that adds `.envrc`"),
                    "unexpected failure reason: {reason}"
                );
                assert!(!reason.contains("failed to delete temporary branch"));
            }
            other => panic!("expected failed status, got {other:?}"),
        }
    }

    struct ExpectedRequest {
        method: &'static str,
        assert_path: Box<dyn Fn(&str) + Send>,
        assert_body: Box<dyn Fn(&str) + Send>,
        status_code: u16,
        response_body: String,
    }

    impl ExpectedRequest {
        fn json(
            method: &'static str,
            path: &'static str,
            assert_body: impl Fn(&str) + Send + 'static,
            response_body: String,
        ) -> Self {
            Self::with_status_and_path_assertion(
                method,
                {
                    let path = path.to_owned();
                    move |request_path| assert_eq!(request_path, path)
                },
                assert_body,
                200,
                response_body,
            )
        }

        fn with_status_and_path_assertion(
            method: &'static str,
            assert_path: impl Fn(&str) + Send + 'static,
            assert_body: impl Fn(&str) + Send + 'static,
            status_code: u16,
            response_body: String,
        ) -> Self {
            Self {
                method,
                assert_path: Box::new(assert_path),
                assert_body: Box::new(assert_body),
                status_code,
                response_body,
            }
        }
    }

    struct TestServer {
        base_url: String,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn spawn(expectations: Vec<ExpectedRequest>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = thread::spawn(move || {
                for expected in expectations {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    assert_eq!(request.method, expected.method);
                    (expected.assert_path)(&request.path);
                    (expected.assert_body)(&request.body);

                    let response = format!(
                        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        expected.status_code,
                        expected.response_body.len(),
                        expected.response_body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });

            Self {
                base_url: format!("http://{address}"),
                handle: Some(handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    struct RecordedRequest {
        method: String,
        path: String,
        body: String,
    }

    fn read_request(stream: &mut impl Read) -> RecordedRequest {
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while !buffer.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            buffer.push(byte[0]);
        }

        let header_text = String::from_utf8(buffer.clone()).unwrap();
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap().to_owned();
        let path = request_parts.next().unwrap().to_owned();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                (name.eq_ignore_ascii_case("content-length")).then(|| value.trim().parse().ok())
            })
            .flatten()
            .unwrap_or(0);

        let mut body = vec![0_u8; content_length];
        stream.read_exact(&mut body).unwrap();

        RecordedRequest {
            method,
            path,
            body: String::from_utf8(body).unwrap(),
        }
    }
}
