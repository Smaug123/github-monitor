mod catalog;
mod files;
mod glob;
mod rulesets;
mod settings;
#[cfg(test)]
mod tests;
mod types;
mod workflows;

#[cfg(test)]
pub use self::catalog::default_rules;
pub use self::catalog::rules_for_repo;
pub use self::files::FileCheck;
pub use self::rulesets::RulesetCheck;
pub(crate) use self::rulesets::{
    active_branch_rulesets_for_default_branch, legacy_protection_superseded_by_rulesets,
};
pub use self::settings::SettingCheck;
pub use self::types::{
    RepoSetting, RequiredCheckSource, Rule, RuleKind, RuleOutput, RuleResult, SettingValue,
};
pub use self::workflows::WorkflowCheck;

use crate::facts::RepoFacts;

pub fn evaluate_rules(rules: &[Rule], facts: &RepoFacts) -> Vec<RuleOutput> {
    rules.iter().map(|rule| rule.evaluate(facts)).collect()
}

pub fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
    match kind {
        RuleKind::Ruleset(rule) => rulesets::evaluate(rule, facts),
        RuleKind::Workflow(rule) => workflows::evaluate(rule, facts),
        RuleKind::File(rule) => files::evaluate(rule, facts),
        RuleKind::Setting(rule) => settings::evaluate(rule, facts),
    }
}
