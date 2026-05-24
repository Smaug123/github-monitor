mod catalog;
mod files;
mod glob;
mod rulesets;
mod settings;
#[cfg(test)]
mod tests;
mod types;
mod workflows;

pub use self::catalog::default_rules;
pub(crate) use self::rulesets::active_branch_rulesets_for_default_branch;
pub use self::types::{RepoSetting, Rule, RuleKind, RuleOutput, RuleResult, SettingValue};

use crate::facts::RepoFacts;

pub fn evaluate_rules(rules: &[Rule], facts: &RepoFacts) -> Vec<RuleOutput> {
    rules.iter().map(|rule| rule.evaluate(facts)).collect()
}

pub fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
    match kind {
        RuleKind::RulesetExists
        | RuleKind::RulesetRequiresStatusCheck { .. }
        | RuleKind::RulesetEnforcesAdmins
        | RuleKind::RulesetRequiresLinearHistory
        | RuleKind::RulesetPreventsForcePush
        | RuleKind::RulesetRestrictsDeletions
        | RuleKind::RulesetRequiresSignedCommits
        | RuleKind::RulesetRequiresPullRequest
        | RuleKind::RulesetRestrictsMergeMethods { .. }
        | RuleKind::RulesetRequiresStrictStatusChecks
        | RuleKind::UsesRulesetsNotLegacyProtection => rulesets::evaluate(kind, facts),
        RuleKind::WorkflowExistsForDefaultBranch
        | RuleKind::WorkflowHasJob { .. }
        | RuleKind::WorkflowActionsPinnedToSha
        | RuleKind::NoPullRequestTargetWithCheckout
        | RuleKind::WorkflowUsesAction { .. }
        | RuleKind::WorkflowHasRequiredChecksComplete => workflows::evaluate(kind, facts),
        RuleKind::FileExists { .. } | RuleKind::NixFlakeExists | RuleKind::NixFlakeHasCheck => {
            files::evaluate(kind, facts)
        }
        RuleKind::RepoSettingMatch { .. } | RuleKind::DefaultBranchNameIs { .. } => {
            settings::evaluate(kind, facts)
        }
    }
}
