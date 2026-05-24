use crate::facts::RepoFacts;
use std::collections::BTreeSet;

use crate::github::types::{
    BypassActor, BypassActorType, MergeMethod, RefNameCondition, Ruleset, RulesetConditions,
    RulesetEnforcement, RulesetRuleType, RulesetTarget,
};

use super::glob::branch_pattern_matches;
use super::{RuleKind, RuleResult};

pub(super) fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
    match kind {
        RuleKind::RulesetExists => {
            if has_active_branch_ruleset_for_default_branch(facts) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: "no active branch ruleset applies to the default branch".to_owned(),
                }
            }
        }
        RuleKind::RulesetRequiresStatusCheck { check_name } => {
            if !has_active_branch_ruleset_for_default_branch(facts) {
                return RuleResult::Fail {
                    reason: "no active branch ruleset was found".to_owned(),
                };
            }

            if active_branch_rulesets_for_default_branch(facts).any(|ruleset| {
                ruleset.rules.iter().any(|rule| {
                    rule.kind == RulesetRuleType::RequiredStatusChecks
                        && rule.parameters.as_ref().is_some_and(|parameters| {
                            parameters
                                .required_status_checks
                                .iter()
                                .any(|check| check.context == *check_name)
                        })
                })
            }) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "no active branch ruleset requires status check `{check_name}`"
                    ),
                }
            }
        }
        RuleKind::RulesetEnforcesAdmins => {
            if !has_active_branch_ruleset_for_default_branch(facts) {
                return RuleResult::Fail {
                    reason: "no active branch ruleset was found".to_owned(),
                };
            }

            if let Some(actor_type) = active_branch_rulesets_for_default_branch(facts)
                .flat_map(|ruleset| ruleset.bypass_actors.iter())
                .find_map(forbidden_bypass_actor_name)
            {
                RuleResult::Fail {
                    reason: format!("an active branch ruleset allows `{actor_type}` to bypass it"),
                }
            } else {
                RuleResult::Pass
            }
        }
        RuleKind::RulesetRequiresLinearHistory => ruleset_rule_presence_result(
            facts,
            RulesetRuleType::RequiredLinearHistory,
            "required_linear_history",
        ),
        RuleKind::RulesetPreventsForcePush => {
            ruleset_rule_presence_result(facts, RulesetRuleType::NonFastForward, "non_fast_forward")
        }
        RuleKind::RulesetRestrictsDeletions => {
            ruleset_rule_presence_result(facts, RulesetRuleType::Deletion, "deletion")
        }
        RuleKind::RulesetRequiresSignedCommits => ruleset_rule_presence_result(
            facts,
            RulesetRuleType::RequiredSignatures,
            "required_signatures",
        ),
        RuleKind::RulesetRequiresPullRequest => {
            ruleset_rule_presence_result(facts, RulesetRuleType::PullRequest, "pull_request")
        }
        RuleKind::RulesetRestrictsMergeMethods { allowed } => {
            evaluate_allowed_merge_methods(facts, allowed)
        }
        RuleKind::RulesetRequiresStrictStatusChecks => evaluate_strict_status_checks(facts),
        RuleKind::UsesRulesetsNotLegacyProtection => {
            if facts.legacy_branch_protection.is_some() {
                RuleResult::Fail {
                    reason: "legacy branch protection is configured on the default branch"
                        .to_owned(),
                }
            } else {
                RuleResult::Pass
            }
        }
        _ => unreachable!("non-ruleset rule passed to rulesets::evaluate"),
    }
}

pub(crate) fn active_branch_rulesets_for_default_branch<'a>(
    facts: &'a RepoFacts,
) -> impl Iterator<Item = &'a Ruleset> + 'a {
    let default_branch = facts.default_branch.to_string();
    facts.rulesets.iter().filter(move |ruleset| {
        ruleset.target == RulesetTarget::Branch
            && ruleset.enforcement == RulesetEnforcement::Active
            && ruleset_conditions_include_branch(&ruleset.conditions, &default_branch)
    })
}

fn has_active_branch_ruleset_for_default_branch(facts: &RepoFacts) -> bool {
    active_branch_rulesets_for_default_branch(facts)
        .next()
        .is_some()
}

/// Returns `true` if the ruleset's conditions include the given branch.
///
/// When `conditions` is `None` (e.g. from an older snapshot that predates
/// condition modelling), we conservatively assume the ruleset applies.
/// When conditions are present, the branch must match at least one include
/// pattern and must not match any exclude pattern. An empty include list
/// therefore matches nothing.
fn ruleset_conditions_include_branch(
    conditions: &Option<RulesetConditions>,
    default_branch: &str,
) -> bool {
    let Some(conditions) = conditions else {
        return true;
    };
    let Some(ref_name) = &conditions.ref_name else {
        return true;
    };
    ref_name_includes_branch(ref_name, default_branch)
}

fn ref_name_includes_branch(ref_name: &RefNameCondition, default_branch: &str) -> bool {
    let included = ref_name
        .include
        .iter()
        .any(|pattern| ref_name_pattern_matches(pattern, default_branch));

    if !included {
        return false;
    }

    !ref_name
        .exclude
        .iter()
        .any(|pattern| ref_name_pattern_matches(pattern, default_branch))
}

fn ref_name_pattern_matches(pattern: &str, branch: &str) -> bool {
    match pattern {
        "~DEFAULT_BRANCH" => true,
        "~ALL" => true,
        _ => branch_pattern_matches(pattern, branch),
    }
}

// GitHub exposes bypassable repository roles under `RepositoryRole`, but our
// facts currently do not resolve the role ID into a narrower built-in or custom
// role name, so any repository-role bypass is treated as forbidden.
fn forbidden_bypass_actor_name(actor: &BypassActor) -> Option<&'static str> {
    match actor.actor_type {
        BypassActorType::OrganizationAdmin => Some("OrganizationAdmin"),
        BypassActorType::RepositoryRole => Some("RepositoryRole"),
        _ => None,
    }
}

fn evaluate_allowed_merge_methods(facts: &RepoFacts, required: &[MergeMethod]) -> RuleResult {
    if !has_active_branch_ruleset_for_default_branch(facts) {
        return RuleResult::Fail {
            reason: "no active branch ruleset was found".to_owned(),
        };
    }

    let required_set = merge_method_set(required);
    let required_text = describe_merge_method_set(&required_set);

    let mut saw_pull_request_rule = false;
    for ruleset in active_branch_rulesets_for_default_branch(facts) {
        for rule in &ruleset.rules {
            if rule.kind != RulesetRuleType::PullRequest {
                continue;
            }
            saw_pull_request_rule = true;
            let actual = rule
                .parameters
                .as_ref()
                .map(|parameters| merge_method_set(&parameters.allowed_merge_methods))
                .unwrap_or_default();
            if actual == required_set {
                return RuleResult::Pass;
            }
        }
    }

    let reason = if saw_pull_request_rule {
        format!(
            "no active branch ruleset's `pull_request` rule restricts `allowed_merge_methods` to {required_text}",
        )
    } else {
        format!(
            "no active branch ruleset contains a `pull_request` rule restricting `allowed_merge_methods` to {required_text}",
        )
    };
    RuleResult::Fail { reason }
}

fn merge_method_set(methods: &[MergeMethod]) -> BTreeSet<String> {
    methods
        .iter()
        .map(|method| String::from(method.clone()))
        .collect()
}

fn evaluate_strict_status_checks(facts: &RepoFacts) -> RuleResult {
    if !has_active_branch_ruleset_for_default_branch(facts) {
        return RuleResult::Fail {
            reason: "no active branch ruleset was found".to_owned(),
        };
    }

    let strict = active_branch_rulesets_for_default_branch(facts)
        .flat_map(|ruleset| ruleset.rules.iter())
        .filter(|rule| rule.kind == RulesetRuleType::RequiredStatusChecks)
        .any(|rule| {
            rule.parameters
                .as_ref()
                .and_then(|parameters| parameters.strict_required_status_checks_policy)
                .unwrap_or(false)
        });

    if strict {
        RuleResult::Pass
    } else {
        RuleResult::Fail {
            reason: "no active branch ruleset requires branches to be up-to-date before merging"
                .to_owned(),
        }
    }
}

fn describe_merge_method_set(methods: &BTreeSet<String>) -> String {
    let names = methods
        .iter()
        .map(|method| format!("`{method}`"))
        .collect::<Vec<_>>();
    format!("[{}]", names.join(", "))
}

fn ruleset_rule_presence_result(
    facts: &RepoFacts,
    required_kind: RulesetRuleType,
    required_name: &str,
) -> RuleResult {
    if !has_active_branch_ruleset_for_default_branch(facts) {
        return RuleResult::Fail {
            reason: "no active branch ruleset was found".to_owned(),
        };
    }

    if active_branch_rulesets_for_default_branch(facts)
        .any(|ruleset| ruleset.rules.iter().any(|rule| rule.kind == required_kind))
    {
        RuleResult::Pass
    } else {
        RuleResult::Fail {
            reason: format!("no active branch ruleset contains `{required_name}`"),
        }
    }
}
