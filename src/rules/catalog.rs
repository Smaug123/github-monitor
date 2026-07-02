use crate::config::{RepoConfig, Visibility};
use crate::github::types::{DefaultWorkflowPermissions, ForkPrApprovalPolicy, MergeMethod};

use super::{
    FileCheck, RepoSetting, RequiredCheckSource, Rule, RuleKind, RulesetCheck, SettingCheck,
    SettingValue, WorkflowCheck,
};

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule::new(
            "RS001",
            "Rulesets exist",
            RuleKind::Ruleset(RulesetCheck::RulesetExists),
        ),
        Rule::new(
            "RS004",
            "Organization admins or repository roles cannot bypass rulesets",
            RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
        ),
        Rule::new(
            "RS005",
            "Rulesets require linear history",
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresLinearHistory),
        ),
        Rule::new(
            "RS006",
            "Rulesets prevent force pushes",
            RuleKind::Ruleset(RulesetCheck::RulesetPreventsForcePush),
        ),
        Rule::new(
            "RS007",
            "Repository uses rulesets instead of legacy protection",
            RuleKind::Ruleset(RulesetCheck::UsesRulesetsNotLegacyProtection),
        ),
        Rule::new(
            "RS008",
            "Rulesets restrict deletions",
            RuleKind::Ruleset(RulesetCheck::RulesetRestrictsDeletions),
        ),
        Rule::new(
            "RS009",
            "Rulesets require signed commits",
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresSignedCommits),
        ),
        Rule::new(
            "RS010",
            "Rulesets require a pull request before merging",
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresPullRequest),
        ),
        Rule::new(
            "RS011",
            "Pull-request rule allows only squash merges",
            RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            }),
        ),
        Rule::new(
            "RS012",
            "all-required-checks-complete status check is required from GitHub Actions",
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck {
                check_name: "all-required-checks-complete".to_owned(),
                source: RequiredCheckSource::GitHubActions,
            }),
        ),
        Rule::new(
            "RS013",
            "Branches must be up-to-date before merging",
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresStrictStatusChecks),
        ),
        Rule::new(
            "WF001",
            "A workflow runs on pushes to the default branch",
            RuleKind::Workflow(WorkflowCheck::WorkflowExistsForDefaultBranch),
        ),
        Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha),
        ),
        Rule::new(
            "WF003",
            "No pull_request_target workflow checks out code",
            RuleKind::Workflow(WorkflowCheck::NoPullRequestTargetWithCheckout),
        ),
        Rule::new(
            "WF004",
            "Workflow has an all-required-checks-complete aggregator job",
            RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
        ),
        Rule::new(
            "WF005",
            "No workflow uses the workflow_run trigger",
            RuleKind::Workflow(WorkflowCheck::NoWorkflowRunTrigger),
        ),
        Rule::new(
            "WF006",
            "No `secrets` references in PR-triggered workflows",
            RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        ),
        Rule::new(
            "NX001",
            "flake.nix exists",
            RuleKind::File(FileCheck::NixFlakeExists),
        ),
        Rule::new(
            "NX002",
            "The flake has observable check coverage",
            RuleKind::File(FileCheck::NixFlakeHasCheck),
        ),
        Rule::new(
            "ST001",
            "Auto-merge is enabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowAutoMerge,
                expected: SettingValue::Bool(true),
            }),
        ),
        Rule::new(
            "ST002",
            "Delete branch on merge is enabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::DeleteBranchOnMerge,
                expected: SettingValue::Bool(true),
            }),
        ),
        Rule::new(
            "ST003",
            "Update branch is enabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowUpdateBranch,
                expected: SettingValue::Bool(true),
            }),
        ),
        Rule::new(
            "ST004",
            "Merge commits are disabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowMergeCommit,
                expected: SettingValue::Bool(false),
            }),
        ),
        Rule::new(
            "ST005",
            "Rebase merges are disabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowRebaseMerge,
                expected: SettingValue::Bool(false),
            }),
        ),
        Rule::new(
            "ST006",
            "Squash merges are enabled",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowSquashMerge,
                expected: SettingValue::Bool(true),
            }),
        ),
        Rule::new(
            "ST007",
            "Fork PR workflows require approval for all external contributors",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::ForkPrApprovalPolicy,
                expected: SettingValue::ForkPrApprovalPolicy(Some(
                    ForkPrApprovalPolicy::AllExternalContributors,
                )),
            }),
        ),
        Rule::new(
            "ST008",
            "Default branch is named `main`",
            RuleKind::Setting(SettingCheck::DefaultBranchNameIs {
                name: "main".to_owned(),
            }),
        ),
        Rule::new(
            "ST010",
            "GITHUB_TOKEN defaults to read-only workflow permissions",
            RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::DefaultWorkflowPermissions,
                expected: SettingValue::DefaultWorkflowPermissions(
                    DefaultWorkflowPermissions::Read,
                ),
            }),
        ),
        Rule::new(
            "FL001",
            "`.envrc` exists",
            RuleKind::File(FileCheck::FileExists {
                path: ".envrc".to_owned(),
            }),
        ),
        Rule::new(
            "FL002",
            "`AGENTS.md` exists",
            RuleKind::File(FileCheck::FileExists {
                path: "AGENTS.md".to_owned(),
            }),
        ),
        Rule::new(
            "FL003",
            "`CLAUDE.md` exists",
            RuleKind::File(FileCheck::FileExists {
                path: "CLAUDE.md".to_owned(),
            }),
        ),
    ]
}

pub fn rules_for_repo(repo: &RepoConfig) -> Vec<Rule> {
    let mut rules = default_rules();
    let expect_private = matches!(repo.visibility, Visibility::Private);
    rules.push(Rule::new(
        "ST009",
        "Repository visibility matches configured expectation",
        RuleKind::Setting(SettingCheck::RepoSettingMatch {
            setting: RepoSetting::Private,
            expected: SettingValue::Bool(expect_private),
        }),
    ));
    rules
}
