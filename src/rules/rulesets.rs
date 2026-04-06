use crate::facts::RepoFacts;
use crate::github::types::{
    BypassActor, BypassActorType, RefNameCondition, Ruleset, RulesetConditions, RulesetEnforcement,
    RulesetRuleType, RulesetTarget,
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
        RuleKind::UsesRulesetsNotLegacyProtection => RuleResult::Skip {
            reason: "RepoFacts does not record legacy branch protection state, so this rule cannot yet distinguish rulesets from legacy protection".to_owned(),
        },
        _ => unreachable!("non-ruleset rule passed to rulesets::evaluate"),
    }
}

fn active_branch_rulesets_for_default_branch<'a>(
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
