use crate::facts::RepoFacts;

use super::{RuleKind, RuleResult};

pub(super) fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
    match kind {
        RuleKind::RepoSettingMatch { setting, expected } => {
            let actual = setting.read(&facts.settings);
            if &actual == expected {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "repository setting `{}` was {}, expected {}",
                        setting.name(),
                        actual.describe(),
                        expected.describe()
                    ),
                }
            }
        }
        RuleKind::DefaultBranchNameIs { name } => {
            let actual = facts.default_branch.to_string();
            if &actual == name {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("default branch is `{actual}`, expected `{name}`"),
                }
            }
        }
        _ => unreachable!("non-setting rule passed to settings::evaluate"),
    }
}
