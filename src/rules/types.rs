use serde::{Deserialize, Serialize};

use crate::facts::{RepoFacts, RepoSettings};
use crate::github::types::{
    DefaultWorkflowPermissions, ForkPrApprovalPolicy, GITHUB_ACTIONS_INTEGRATION_ID, MergeMethod,
};
use crate::types::{Gathered, RuleId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoSetting {
    Private,
    Archived,
    Disabled,
    AllowAutoMerge,
    DeleteBranchOnMerge,
    AllowUpdateBranch,
    AllowSquashMerge,
    AllowMergeCommit,
    AllowRebaseMerge,
    ForkPrApprovalPolicy,
    DefaultWorkflowPermissions,
}

impl RepoSetting {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Archived => "archived",
            Self::Disabled => "disabled",
            Self::AllowAutoMerge => "allow_auto_merge",
            Self::DeleteBranchOnMerge => "delete_branch_on_merge",
            Self::AllowUpdateBranch => "allow_update_branch",
            Self::AllowSquashMerge => "allow_squash_merge",
            Self::AllowMergeCommit => "allow_merge_commit",
            Self::AllowRebaseMerge => "allow_rebase_merge",
            Self::ForkPrApprovalPolicy => "fork_pr_approval_policy",
            Self::DefaultWorkflowPermissions => "default_workflow_permissions",
        }
    }

    pub(crate) fn is_safe_to_auto_fix(&self) -> bool {
        matches!(
            self,
            Self::AllowAutoMerge
                | Self::DeleteBranchOnMerge
                | Self::AllowUpdateBranch
                | Self::AllowSquashMerge
                | Self::AllowMergeCommit
                | Self::AllowRebaseMerge
        )
    }

    pub(super) fn read(&self, settings: &RepoSettings) -> SettingValue {
        match self {
            Self::Private => SettingValue::Bool(settings.private),
            Self::Archived => SettingValue::Bool(settings.archived),
            Self::Disabled => SettingValue::Bool(settings.disabled),
            Self::AllowAutoMerge => SettingValue::from_optional_bool(settings.allow_auto_merge),
            Self::DeleteBranchOnMerge => {
                SettingValue::from_optional_bool(settings.delete_branch_on_merge)
            }
            Self::AllowUpdateBranch => {
                SettingValue::from_optional_bool(settings.allow_update_branch)
            }
            Self::AllowSquashMerge => {
                SettingValue::from_optional_bool(settings.allow_squash_merge)
            }
            Self::AllowMergeCommit => SettingValue::from_optional_bool(settings.allow_merge_commit),
            Self::AllowRebaseMerge => {
                SettingValue::from_optional_bool(settings.allow_rebase_merge)
            }
            Self::ForkPrApprovalPolicy => match &settings.fork_pr_approval_policy {
                Gathered::Present(policy) => {
                    SettingValue::ForkPrApprovalPolicy(Some(policy.clone()))
                }
                Gathered::Absent => SettingValue::ForkPrApprovalPolicy(None),
                Gathered::Unknown => SettingValue::Unknown,
            },
            Self::DefaultWorkflowPermissions => SettingValue::DefaultWorkflowPermissions(
                settings.default_workflow_permissions.clone(),
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Bool(bool),
    /// A setting whose value could not be determined (the token lacks permission,
    /// so GitHub omitted the field or 404'd the endpoint). Distinct from any
    /// concrete value: a rule that reads it must `Error`, never pass or fail
    /// vacuously. Only ever produced by reading actual facts; rule expectations
    /// are always concrete.
    Unknown,
    ForkPrApprovalPolicy(Option<ForkPrApprovalPolicy>),
    DefaultWorkflowPermissions(DefaultWorkflowPermissions),
}

impl SettingValue {
    pub(super) fn from_optional_bool(value: Option<bool>) -> Self {
        match value {
            Some(value) => Self::Bool(value),
            None => Self::Unknown,
        }
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Unknown => "unknown (not reported by GitHub)".to_owned(),
            Self::ForkPrApprovalPolicy(Some(policy)) => String::from(policy.clone()),
            Self::ForkPrApprovalPolicy(None) => "unknown".to_owned(),
            Self::DefaultWorkflowPermissions(value) => String::from(value.clone()),
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Unknown
            | Self::ForkPrApprovalPolicy(_)
            | Self::DefaultWorkflowPermissions(_) => None,
        }
    }
}

/// Which app must report a required status check for it to count. GitHub records
/// the reporting app on each required check as an `integration_id`; `Any` imposes
/// no constraint (GitHub's default), while `GitHubActions` requires the check to
/// come from the first-party GitHub Actions app, so a third-party integration
/// can't satisfy the rule by reporting a same-named context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequiredCheckSource {
    Any,
    GitHubActions,
}

impl RequiredCheckSource {
    /// The `integration_id` GitHub records for a check from this source, or
    /// `None` when the source pins no particular app.
    pub(crate) fn integration_id(&self) -> Option<u64> {
        match self {
            Self::Any => None,
            Self::GitHubActions => Some(GITHUB_ACTIONS_INTEGRATION_ID),
        }
    }

    /// Whether a check whose reporting app is `integration_id` satisfies this
    /// source constraint.
    pub(crate) fn accepts(&self, integration_id: Option<u64>) -> bool {
        match self {
            Self::Any => true,
            Self::GitHubActions => integration_id == Some(GITHUB_ACTIONS_INTEGRATION_ID),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleKind {
    RulesetExists,
    RulesetRequiresStatusCheck {
        check_name: String,
        source: RequiredCheckSource,
    },
    RulesetEnforcesAdmins,
    RulesetRequiresLinearHistory,
    RulesetPreventsForcePush,
    RulesetRestrictsDeletions,
    RulesetRequiresSignedCommits,
    RulesetRequiresPullRequest,
    RulesetRestrictsMergeMethods {
        allowed: Vec<MergeMethod>,
    },
    RulesetRequiresStrictStatusChecks,
    UsesRulesetsNotLegacyProtection,
    WorkflowExistsForDefaultBranch,
    WorkflowHasJob {
        job_name: String,
    },
    WorkflowActionsPinnedToSha,
    NoPullRequestTargetWithCheckout,
    NoWorkflowRunTrigger,
    NoPullRequestSecretReferences,
    WorkflowUsesAction {
        action: String,
    },
    WorkflowHasRequiredChecksComplete,
    FileExists {
        path: String,
    },
    NixFlakeExists,
    NixFlakeHasCheck,
    RepoSettingMatch {
        setting: RepoSetting,
        expected: SettingValue,
    },
    DefaultBranchNameIs {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleResult {
    Pass,
    Fail { reason: String },
    Skip { reason: String },
    Error { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub name: String,
    pub kind: RuleKind,
}

impl Rule {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: RuleKind) -> Self {
        Self {
            id: RuleId::new(id),
            name: name.into(),
            kind,
        }
    }

    pub fn evaluate(&self, facts: &RepoFacts) -> RuleOutput {
        RuleOutput {
            id: self.id.clone(),
            name: self.name.clone(),
            result: super::evaluate(&self.kind, facts),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleOutput {
    pub id: RuleId,
    pub name: String,
    pub result: RuleResult,
}
