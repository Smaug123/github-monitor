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

/// Every rule that *could* apply to a repo before `disabled_rules` filtering:
/// the default set plus the per-repo `ST009`. This is the authoritative set of
/// valid IDs a `disabled_rules` entry may name.
fn candidate_rules_for_repo(repo: &RepoConfig) -> Vec<Rule> {
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

/// The rules to evaluate (and, in `--fix` mode, remediate) for `repo`, with any
/// `disabled_rules` entries removed. Unknown IDs are ignored here — they are
/// rejected up front by [`unknown_disabled_rule_ids`], so by the time this runs
/// every listed ID is known.
pub fn rules_for_repo(repo: &RepoConfig) -> Vec<Rule> {
    let mut rules = candidate_rules_for_repo(repo);
    if let Some(disabled) = &repo.disabled_rules {
        rules.retain(|rule| !disabled.iter().any(|id| id == rule.id.as_str()));
    }
    rules
}

/// Rule IDs listed in `disabled_rules` that no rule in this repo's candidate set
/// defines — a typo or a stale ID. Returned in the order they appear in the
/// config so the caller can report them verbatim. Validated up front so `--fix`
/// never plans work for a rule the user believes is disabled and a typo is never
/// silently ignored.
pub fn unknown_disabled_rule_ids(repo: &RepoConfig) -> Vec<String> {
    let Some(disabled) = &repo.disabled_rules else {
        return Vec::new();
    };
    let candidates = candidate_rules_for_repo(repo);
    let known: std::collections::BTreeSet<&str> =
        candidates.iter().map(|rule| rule.id.as_str()).collect();
    disabled
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .cloned()
        .collect()
}
