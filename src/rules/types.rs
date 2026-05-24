use serde::{Deserialize, Serialize};

use crate::facts::{RepoFacts, RepoSettings};
use crate::github::types::{ForkPrWorkflowsPolicy, MergeMethod};
use crate::types::RuleId;

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
    ForkPrWorkflowsPolicy,
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
            Self::ForkPrWorkflowsPolicy => "fork_pr_workflows_policy",
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
            Self::AllowAutoMerge => SettingValue::Bool(settings.allow_auto_merge),
            Self::DeleteBranchOnMerge => SettingValue::Bool(settings.delete_branch_on_merge),
            Self::AllowUpdateBranch => SettingValue::Bool(settings.allow_update_branch),
            Self::AllowSquashMerge => SettingValue::Bool(settings.allow_squash_merge),
            Self::AllowMergeCommit => SettingValue::Bool(settings.allow_merge_commit),
            Self::AllowRebaseMerge => SettingValue::Bool(settings.allow_rebase_merge),
            Self::ForkPrWorkflowsPolicy => {
                SettingValue::ForkPrWorkflowsPolicy(settings.fork_pr_workflows_policy.clone())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Bool(bool),
    ForkPrWorkflowsPolicy(Option<ForkPrWorkflowsPolicy>),
}

impl SettingValue {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::ForkPrWorkflowsPolicy(Some(policy)) => String::from(policy.clone()),
            Self::ForkPrWorkflowsPolicy(None) => "unknown".to_owned(),
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::ForkPrWorkflowsPolicy(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleKind {
    RulesetExists,
    RulesetRequiresStatusCheck {
        check_name: String,
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
