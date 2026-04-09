use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::rules::{RepoSetting, Rule, RuleKind, RuleOutput, RuleResult, SettingValue};
use crate::types::RuleId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFix {
    pub rule_id: RuleId,
    pub rule_name: String,
    pub description: String,
    pub status: FixStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FixStatus {
    Planned,
    Rejected { reason: String },
}

pub fn plan_repo_fixes(rules: &[Rule], outputs: &[RuleOutput]) -> Vec<RepoFix> {
    let outputs_by_id = index_rule_outputs(outputs);

    rules
        .iter()
        .filter_map(|rule| {
            let output = outputs_by_id
                .get(&rule.id)
                .copied()
                .unwrap_or_else(|| panic!("missing evaluation output for rule {}", rule.id));
            plan_rule_fix(rule, output)
        })
        .collect()
}

fn index_rule_outputs(outputs: &[RuleOutput]) -> HashMap<RuleId, &RuleOutput> {
    let mut outputs_by_id = HashMap::with_capacity(outputs.len());

    for output in outputs {
        let replaced = outputs_by_id.insert(output.id.clone(), output);
        assert!(
            replaced.is_none(),
            "duplicate evaluation output for rule {}",
            output.id
        );
    }

    outputs_by_id
}

fn plan_rule_fix(rule: &Rule, output: &RuleOutput) -> Option<RepoFix> {
    let RuleResult::Fail { .. } = &output.result else {
        return None;
    };

    let (description, status) = match &rule.kind {
        RuleKind::RepoSettingMatch { setting, expected }
            if repo_setting_is_safe_to_auto_fix(setting) =>
        {
            (
                format!(
                    "set repository setting `{}` to {}",
                    repo_setting_name(setting),
                    setting_value_as_bool(expected)
                ),
                FixStatus::Planned,
            )
        }
        RuleKind::RepoSettingMatch { setting, .. } => (
            "automatic fix unavailable".to_owned(),
            FixStatus::Rejected {
                reason: format!(
                    "automatic fixes for repository setting `{}` are not enabled",
                    repo_setting_name(setting)
                ),
            },
        ),
        _ => (
            "automatic fix unavailable".to_owned(),
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            },
        ),
    };

    Some(RepoFix {
        rule_id: output.id.clone(),
        rule_name: output.name.clone(),
        description,
        status,
    })
}

fn repo_setting_is_safe_to_auto_fix(setting: &RepoSetting) -> bool {
    matches!(
        setting,
        RepoSetting::AllowAutoMerge
            | RepoSetting::DeleteBranchOnMerge
            | RepoSetting::AllowUpdateBranch
            | RepoSetting::AllowSquashMerge
            | RepoSetting::AllowMergeCommit
            | RepoSetting::AllowRebaseMerge
    )
}

fn repo_setting_name(setting: &RepoSetting) -> &'static str {
    match setting {
        RepoSetting::Private => "private",
        RepoSetting::Archived => "archived",
        RepoSetting::Disabled => "disabled",
        RepoSetting::AllowAutoMerge => "allow_auto_merge",
        RepoSetting::DeleteBranchOnMerge => "delete_branch_on_merge",
        RepoSetting::AllowUpdateBranch => "allow_update_branch",
        RepoSetting::AllowSquashMerge => "allow_squash_merge",
        RepoSetting::AllowMergeCommit => "allow_merge_commit",
        RepoSetting::AllowRebaseMerge => "allow_rebase_merge",
    }
}

fn setting_value_as_bool(value: &SettingValue) -> bool {
    match value {
        SettingValue::Bool(value) => *value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::RepoFacts;
    use crate::rules::{Rule, default_rules, evaluate_rules};
    use std::collections::BTreeMap;

    fn bad_fixture() -> RepoFacts {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/example-org/bad-repo.json"
        )))
        .unwrap()
    }

    #[test]
    fn bad_fixture_plans_effects_and_rejections_for_failed_rules() {
        let facts = bad_fixture();
        let rules = default_rules();
        let outputs = evaluate_rules(&rules, &facts);
        let fixes = plan_repo_fixes(&rules, &outputs);
        let by_rule_id = fixes
            .iter()
            .map(|fix| (fix.rule_id.to_string(), fix))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(fixes.len(), 15);
        assert_eq!(by_rule_id["ST001"].status, FixStatus::Planned);
        assert_eq!(
            by_rule_id["ST001"].description,
            "set repository setting `allow_auto_merge` to true"
        );
        assert_eq!(by_rule_id["ST004"].status, FixStatus::Planned);
        assert_eq!(by_rule_id["ST006"].status, FixStatus::Planned);
        assert_eq!(
            by_rule_id["RS001"].status,
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
        assert_eq!(
            by_rule_id["WF003"].status,
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
    }

    #[test]
    fn risky_repo_setting_rules_are_rejected_instead_of_silently_dropped() {
        let facts = bad_fixture();
        let rules = vec![Rule::new(
            "ST999",
            "Repository is private",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::Private,
                expected: SettingValue::Bool(true),
            },
        )];
        let outputs = evaluate_rules(&rules, &facts);
        let fixes = plan_repo_fixes(&rules, &outputs);

        assert_eq!(
            fixes,
            vec![RepoFix {
                rule_id: RuleId::new("ST999"),
                rule_name: "Repository is private".to_owned(),
                description: "automatic fix unavailable".to_owned(),
                status: FixStatus::Rejected {
                    reason: "automatic fixes for repository setting `private` are not enabled"
                        .to_owned(),
                },
            }]
        );
    }

    #[test]
    fn fix_planning_matches_outputs_by_rule_id_not_position() {
        let facts = bad_fixture();
        let rules = default_rules();
        let outputs = evaluate_rules(&rules, &facts);
        let expected = plan_repo_fixes(&rules, &outputs);

        let mut reversed_outputs = outputs.clone();
        reversed_outputs.reverse();

        assert_eq!(plan_repo_fixes(&rules, &reversed_outputs), expected);
    }
}
