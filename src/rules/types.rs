use serde::{Deserialize, Serialize};

use crate::facts::{RepoFacts, RepoSettings};
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
        let value = match self {
            Self::Private => settings.private,
            Self::Archived => settings.archived,
            Self::Disabled => settings.disabled,
            Self::AllowAutoMerge => settings.allow_auto_merge,
            Self::DeleteBranchOnMerge => settings.delete_branch_on_merge,
            Self::AllowUpdateBranch => settings.allow_update_branch,
            Self::AllowSquashMerge => settings.allow_squash_merge,
            Self::AllowMergeCommit => settings.allow_merge_commit,
            Self::AllowRebaseMerge => settings.allow_rebase_merge,
        };

        SettingValue::Bool(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Bool(bool),
}

impl SettingValue {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
        }
    }

    pub(crate) fn as_bool(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
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
    FileExists {
        path: String,
    },
    NixFlakeExists,
    NixFlakeHasCheck,
    RepoSettingMatch {
        setting: RepoSetting,
        expected: SettingValue,
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
