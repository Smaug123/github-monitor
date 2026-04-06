use serde::{Deserialize, Serialize};

use crate::facts::RepoFacts;
use crate::github::client::GitHubClient;
use crate::github::types::RepositoryUpdate;
use crate::rules::{RepoSetting, Rule, RuleKind, RuleOutput, RuleResult, evaluate_rules};
use crate::types::{RepoRef, RuleId};

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
    Applied,
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFix {
    rule_id: RuleId,
    rule_name: String,
    plan: FixPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixPlan {
    Effect(FixEffect),
    Rejected { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixEffect {
    SetRepositorySetting {
        repo: RepoRef,
        setting: RepoSetting,
        value: bool,
    },
}

impl PlannedFix {
    pub fn planned_report(&self) -> RepoFix {
        match &self.plan {
            FixPlan::Effect(_) => self.with_status(FixStatus::Planned),
            FixPlan::Rejected { reason } => self.with_status(FixStatus::Rejected {
                reason: reason.clone(),
            }),
        }
    }

    fn with_status(&self, status: FixStatus) -> RepoFix {
        RepoFix {
            rule_id: self.rule_id.clone(),
            rule_name: self.rule_name.clone(),
            description: self.description(),
            status,
        }
    }

    fn description(&self) -> String {
        match &self.plan {
            FixPlan::Effect(effect) => effect.describe(),
            FixPlan::Rejected { .. } => "automatic fix unavailable".to_owned(),
        }
    }
}

impl FixEffect {
    fn describe(&self) -> String {
        match self {
            Self::SetRepositorySetting { setting, value, .. } => {
                format!("set repository setting `{}` to {value}", setting.name())
            }
        }
    }

    fn repo(&self) -> &RepoRef {
        match self {
            Self::SetRepositorySetting { repo, .. } => repo,
        }
    }
}

pub fn plan_repo_fixes(rules: &[Rule], facts: &RepoFacts) -> Vec<PlannedFix> {
    let outputs = evaluate_rules(rules, facts);

    std::iter::zip(rules, &outputs)
        .filter_map(|(rule, output)| plan_rule_fix(&facts.repo, rule, output))
        .collect()
}

pub fn execute_repo_fixes(client: &mut GitHubClient, fixes: &[PlannedFix]) -> Vec<RepoFix> {
    let effect_result = execute_planned_effects(client, fixes);

    fixes
        .iter()
        .map(|fix| match &fix.plan {
            FixPlan::Rejected { reason } => fix.with_status(FixStatus::Rejected {
                reason: reason.clone(),
            }),
            FixPlan::Effect(_) => match effect_result.as_ref() {
                Some(Ok(())) => fix.with_status(FixStatus::Applied),
                Some(Err(reason)) => fix.with_status(FixStatus::Failed {
                    reason: reason.clone(),
                }),
                None => fix.with_status(FixStatus::Failed {
                    reason: "internal error: missing execution result for planned effect"
                        .to_owned(),
                }),
            },
        })
        .collect()
}

fn execute_planned_effects(
    client: &mut GitHubClient,
    fixes: &[PlannedFix],
) -> Option<Result<(), String>> {
    let mut repo = None::<RepoRef>;
    let mut update = RepositoryUpdate::default();
    let mut internal_error = None::<String>;
    let mut saw_effect = false;

    for fix in fixes {
        let FixPlan::Effect(effect) = &fix.plan else {
            continue;
        };

        saw_effect = true;

        if let Some(existing_repo) = &repo {
            if existing_repo != effect.repo() && internal_error.is_none() {
                internal_error = Some(format!(
                    "internal error: planned fixes span multiple repositories (`{existing_repo}` and `{}`)",
                    effect.repo()
                ));
            }
        } else {
            repo = Some(effect.repo().clone());
        }

        if internal_error.is_none()
            && let Some(reason) = apply_fix_effect_to_repository_update(&mut update, effect)
        {
            internal_error = Some(reason);
        }
    }

    if !saw_effect {
        return None;
    }

    if let Some(reason) = internal_error {
        return Some(Err(reason));
    }

    if update.is_empty() {
        return Some(Err(
            "internal error: automatic fix produced an empty repository update".to_owned(),
        ));
    }

    let repo = repo.expect("saw_effect guarantees a repository was recorded");
    Some(
        client
            .update_repository(&repo, &update)
            .map(|_| ())
            .map_err(|error| error.to_string()),
    )
}

fn plan_rule_fix(repo: &RepoRef, rule: &Rule, output: &RuleOutput) -> Option<PlannedFix> {
    let RuleResult::Fail { .. } = &output.result else {
        return None;
    };

    Some(PlannedFix {
        rule_id: output.id.clone(),
        rule_name: output.name.clone(),
        plan: match &rule.kind {
            RuleKind::RepoSettingMatch { setting, expected } if setting.is_safe_to_auto_fix() => {
                FixPlan::Effect(FixEffect::SetRepositorySetting {
                    repo: repo.clone(),
                    setting: setting.clone(),
                    value: expected.as_bool(),
                })
            }
            RuleKind::RepoSettingMatch { setting, .. } => FixPlan::Rejected {
                reason: format!(
                    "automatic fixes for repository setting `{}` are not enabled",
                    setting.name()
                ),
            },
            _ => FixPlan::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            },
        },
    })
}

fn apply_fix_effect_to_repository_update(
    update: &mut RepositoryUpdate,
    effect: &FixEffect,
) -> Option<String> {
    match effect {
        FixEffect::SetRepositorySetting { setting, value, .. } => {
            apply_repo_setting_update(update, setting, *value);
            None
        }
    }
}

fn apply_repo_setting_update(update: &mut RepositoryUpdate, setting: &RepoSetting, value: bool) {
    match setting {
        RepoSetting::Private => update.private = Some(value),
        RepoSetting::Archived => update.archived = Some(value),
        RepoSetting::Disabled => update.disabled = Some(value),
        RepoSetting::AllowAutoMerge => update.allow_auto_merge = Some(value),
        RepoSetting::DeleteBranchOnMerge => update.delete_branch_on_merge = Some(value),
        RepoSetting::AllowUpdateBranch => update.allow_update_branch = Some(value),
        RepoSetting::AllowSquashMerge => update.allow_squash_merge = Some(value),
        RepoSetting::AllowMergeCommit => update.allow_merge_commit = Some(value),
        RepoSetting::AllowRebaseMerge => update.allow_rebase_merge = Some(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{RepoSetting, Rule, SettingValue, default_rules};
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
        let fixes = plan_repo_fixes(&default_rules(), &facts);
        let by_rule_id = fixes
            .iter()
            .map(|fix| (fix.rule_id.to_string(), fix))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(fixes.len(), 15);
        assert_eq!(
            by_rule_id["ST001"].plan,
            FixPlan::Effect(FixEffect::SetRepositorySetting {
                repo: facts.repo.clone(),
                setting: RepoSetting::AllowAutoMerge,
                value: true,
            })
        );
        assert_eq!(
            by_rule_id["ST004"].plan,
            FixPlan::Effect(FixEffect::SetRepositorySetting {
                repo: facts.repo.clone(),
                setting: RepoSetting::AllowMergeCommit,
                value: false,
            })
        );
        assert_eq!(
            by_rule_id["RS001"].plan,
            FixPlan::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
        assert_eq!(
            by_rule_id["WF003"].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            }
        );
        assert_eq!(
            by_rule_id["ST005"].planned_report().status,
            FixStatus::Planned
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
        let fixes = plan_repo_fixes(&rules, &facts);

        assert_eq!(
            fixes,
            vec![PlannedFix {
                rule_id: RuleId::new("ST999"),
                rule_name: "Repository is private".to_owned(),
                plan: FixPlan::Rejected {
                    reason: "automatic fixes for repository setting `private` are not enabled"
                        .to_owned(),
                },
            }]
        );
    }
}
