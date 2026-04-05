use serde::{Deserialize, Serialize};

use crate::types::RuleId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RepoSetting {
    Placeholder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Placeholder,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RuleKind {
    RulesetExists,
    RulesetRequiresStatusCheck {
        check_name: String,
    },
    RulesetRequiresReviewers {
        min_count: u32,
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
pub struct RuleOutput {
    pub id: RuleId,
    pub name: String,
    pub result: RuleResult,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn reason() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9 .,;:!?-]{0,100}"
    }

    fn rule_result_strategy() -> impl Strategy<Value = RuleResult> {
        prop_oneof![
            Just(RuleResult::Pass),
            reason().prop_map(|reason| RuleResult::Fail { reason }),
            reason().prop_map(|reason| RuleResult::Skip { reason }),
            reason().prop_map(|reason| RuleResult::Error { reason }),
        ]
    }

    fn rule_output_strategy() -> impl Strategy<Value = RuleOutput> {
        (
            "[A-Z]{2}[0-9]{3}",
            "[a-zA-Z][a-zA-Z0-9 _-]{0,50}",
            rule_result_strategy(),
        )
            .prop_map(|(id, name, result)| RuleOutput {
                id: RuleId::new(id),
                name,
                result,
            })
    }

    proptest! {
        #[test]
        fn rule_result_json_roundtrip(result in rule_result_strategy()) {
            let json = serde_json::to_string(&result).unwrap();
            let deserialized: RuleResult = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(deserialized, result);
        }

        #[test]
        fn rule_output_json_roundtrip(output in rule_output_strategy()) {
            let json = serde_json::to_string(&output).unwrap();
            let deserialized: RuleOutput = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(deserialized, output);
        }
    }
}
