use super::{RepoSetting, Rule, RuleKind, SettingValue};

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule::new("RS001", "Rulesets exist", RuleKind::RulesetExists),
        Rule::new(
            "RS002",
            "CI status check is required",
            RuleKind::RulesetRequiresStatusCheck {
                check_name: "ci".to_owned(),
            },
        ),
        Rule::new(
            "RS004",
            "Organization admins or repository roles cannot bypass rulesets",
            RuleKind::RulesetEnforcesAdmins,
        ),
        Rule::new(
            "RS005",
            "Rulesets require linear history",
            RuleKind::RulesetRequiresLinearHistory,
        ),
        Rule::new(
            "RS006",
            "Rulesets prevent force pushes",
            RuleKind::RulesetPreventsForcePush,
        ),
        Rule::new(
            "RS007",
            "Repository uses rulesets instead of legacy protection",
            RuleKind::UsesRulesetsNotLegacyProtection,
        ),
        Rule::new(
            "WF001",
            "A workflow runs on pushes to the default branch",
            RuleKind::WorkflowExistsForDefaultBranch,
        ),
        Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        ),
        Rule::new(
            "WF003",
            "No pull_request_target workflow checks out code",
            RuleKind::NoPullRequestTargetWithCheckout,
        ),
        Rule::new("NX001", "flake.nix exists", RuleKind::NixFlakeExists),
        Rule::new(
            "NX002",
            "The flake has observable check coverage",
            RuleKind::NixFlakeHasCheck,
        ),
        Rule::new(
            "ST001",
            "Auto-merge is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowAutoMerge,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST002",
            "Delete branch on merge is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::DeleteBranchOnMerge,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST003",
            "Update branch is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowUpdateBranch,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST004",
            "Merge commits are disabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowMergeCommit,
                expected: SettingValue::Bool(false),
            },
        ),
        Rule::new(
            "ST005",
            "Rebase merges are disabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowRebaseMerge,
                expected: SettingValue::Bool(false),
            },
        ),
        Rule::new(
            "ST006",
            "Squash merges are enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowSquashMerge,
                expected: SettingValue::Bool(true),
            },
        ),
    ]
}
