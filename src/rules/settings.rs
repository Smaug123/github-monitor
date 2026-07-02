use serde::{Deserialize, Serialize};

use crate::facts::RepoFacts;

use super::{RepoSetting, RuleResult, SettingValue};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingCheck {
    RepoSettingMatch {
        setting: RepoSetting,
        expected: SettingValue,
    },
    DefaultBranchNameIs {
        name: String,
    },
}

pub(super) fn evaluate(rule: &SettingCheck, facts: &RepoFacts) -> RuleResult {
    match rule {
        SettingCheck::RepoSettingMatch { setting, expected } => {
            let actual = setting.read(&facts.settings);
            if let SettingValue::Unknown = actual {
                return RuleResult::Error {
                    reason: format!(
                        "repository setting `{}` was not reported by GitHub; the token may lack \
                         the permission needed to read it",
                        setting.name(),
                    ),
                };
            }
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
        SettingCheck::DefaultBranchNameIs { name } => {
            let actual = facts.default_branch.to_string();
            if &actual == name {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("default branch is `{actual}`, expected `{name}`"),
                }
            }
        }
    }
}
