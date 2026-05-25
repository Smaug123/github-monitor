use crate::github::types::{ForkPrApprovalPolicy, MergeMethod};

use super::{RepoSetting, Rule, RuleKind, SettingValue};

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule::new("RS001", "Rulesets exist", RuleKind::RulesetExists),
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
            "RS008",
            "Rulesets restrict deletions",
            RuleKind::RulesetRestrictsDeletions,
        ),
        Rule::new(
            "RS009",
            "Rulesets require signed commits",
            RuleKind::RulesetRequiresSignedCommits,
        ),
        Rule::new(
            "RS010",
            "Rulesets require a pull request before merging",
            RuleKind::RulesetRequiresPullRequest,
        ),
        Rule::new(
            "RS011",
            "Pull-request rule allows only squash merges",
            RuleKind::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            },
        ),
        Rule::new(
            "RS012",
            "all-required-checks-complete status check is required",
            RuleKind::RulesetRequiresStatusCheck {
                check_name: "all-required-checks-complete".to_owned(),
            },
        ),
        Rule::new(
            "RS013",
            "Branches must be up-to-date before merging",
            RuleKind::RulesetRequiresStrictStatusChecks,
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
        Rule::new(
            "WF004",
            "Workflow has an all-required-checks-complete aggregator job",
            RuleKind::WorkflowHasRequiredChecksComplete,
        ),
        Rule::new(
            "WF005",
            "No workflow uses the workflow_run trigger",
            RuleKind::NoWorkflowRunTrigger,
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
        Rule::new(
            "ST007",
            "Fork PR workflows require approval for all external contributors",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::ForkPrApprovalPolicy,
                expected: SettingValue::ForkPrApprovalPolicy(Some(
                    ForkPrApprovalPolicy::AllExternalContributors,
                )),
            },
        ),
        Rule::new(
            "ST008",
            "Default branch is named `main`",
            RuleKind::DefaultBranchNameIs {
                name: "main".to_owned(),
            },
        ),
        Rule::new(
            "FL001",
            "`.envrc` exists",
            RuleKind::FileExists {
                path: ".envrc".to_owned(),
            },
        ),
    ]
}
