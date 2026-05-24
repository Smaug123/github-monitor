use crate::facts::RepoFacts;
use std::collections::BTreeSet;

use crate::github::types::{
    BranchProtection, BypassActor, BypassActorType, MergeMethod, RefNameCondition, Ruleset,
    RulesetConditions, RulesetEnforcement, RulesetRuleType, RulesetTarget,
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

/// Reports whether the active branch rulesets on the default branch enforce at
/// least as much as the legacy branch protection on that branch. Returns
/// `Ok(())` if every legacy constraint is covered; otherwise the `Vec` holds
/// one human-readable reason per uncovered constraint, in deterministic order.
///
/// Semantics: multiple matching rulesets compose by AND (each rule enforced
/// independently), so context sets union across rulesets and boolean flags
/// are satisfied if any covering ruleset sets them. A constraint that has no
/// clean ruleset equivalent (e.g. `restrictions`, `lock_branch`) yields a
/// rejection reason — we never delete legacy protection in those cases.
pub(crate) fn legacy_protection_superseded_by_rulesets(
    legacy: &BranchProtection,
    facts: &RepoFacts,
) -> Result<(), Vec<String>> {
    let mut reasons = Vec::new();

    if legacy.is_empty() {
        reasons.push(
            "legacy branch protection has no fields our model recognises — refusing to delete, \
             since GitHub may be returning fields that this tool does not yet parse"
                .to_owned(),
        );
        return Err(reasons);
    }

    let rulesets: Vec<&Ruleset> = active_branch_rulesets_for_default_branch(facts).collect();

    check_required_status_checks(&legacy.required_status_checks, &rulesets, &mut reasons);
    check_pull_request_reviews(&legacy.required_pull_request_reviews, &rulesets, &mut reasons);
    check_rule_presence(
        legacy.required_linear_history.as_ref(),
        &rulesets,
        RulesetRuleType::RequiredLinearHistory,
        "required_linear_history",
        &mut reasons,
    );
    if legacy_blocks_force_pushes(&legacy.allow_force_pushes) {
        check_force_push_or_deletion(
            &rulesets,
            RulesetRuleType::NonFastForward,
            "non_fast_forward",
            "blocks force pushes",
            &mut reasons,
        );
    }
    if legacy_blocks_deletions(&legacy.allow_deletions) {
        check_force_push_or_deletion(
            &rulesets,
            RulesetRuleType::Deletion,
            "deletion",
            "blocks branch deletion",
            &mut reasons,
        );
    }
    check_rule_presence(
        legacy.required_signatures.as_ref(),
        &rulesets,
        RulesetRuleType::RequiredSignatures,
        "required_signatures",
        &mut reasons,
    );
    check_rule_presence(
        legacy.block_creations.as_ref(),
        &rulesets,
        RulesetRuleType::Creation,
        "creation",
        &mut reasons,
    );
    check_conversation_resolution(
        legacy.required_conversation_resolution.as_ref(),
        &rulesets,
        &mut reasons,
    );
    check_enforce_admins(legacy.enforce_admins.as_ref(), &rulesets, &mut reasons);
    if let Some(restrictions) = &legacy.restrictions
        && !restrictions.is_empty()
    {
        reasons.push(
            "legacy `restrictions` (push allowlists for users/teams/apps) has no equivalent in \
             ruleset semantics; refusing to delete"
                .to_owned(),
        );
    }
    if let Some(flag) = &legacy.lock_branch
        && flag.enabled
    {
        reasons.push(
            "legacy `lock_branch` cannot be safely mapped to a ruleset rule; refusing to delete"
                .to_owned(),
        );
    }

    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons)
    }
}

// Legacy `allow_force_pushes` defaults to the restrictive state (force pushes
// blocked) when the field is absent or `enabled: false`. Only `enabled: true`
// is permissive.
fn legacy_blocks_force_pushes(flag: &Option<crate::github::types::LegacyEnabledFlag>) -> bool {
    match flag {
        None => true,
        Some(flag) => !flag.enabled,
    }
}

// Same default-restrictive semantics as `allow_force_pushes`.
fn legacy_blocks_deletions(flag: &Option<crate::github::types::LegacyEnabledFlag>) -> bool {
    match flag {
        None => true,
        Some(flag) => !flag.enabled,
    }
}

fn check_required_status_checks(
    legacy: &Option<crate::github::types::LegacyRequiredStatusChecks>,
    rulesets: &[&Ruleset],
    reasons: &mut Vec<String>,
) {
    let Some(checks) = legacy else {
        return;
    };
    let legacy_contexts = checks.all_contexts();

    if !legacy_contexts.is_empty() {
        let mut covered: BTreeSet<String> = BTreeSet::new();
        for ruleset in rulesets {
            for rule in &ruleset.rules {
                if rule.kind != RulesetRuleType::RequiredStatusChecks {
                    continue;
                }
                if let Some(parameters) = &rule.parameters {
                    for check in &parameters.required_status_checks {
                        covered.insert(check.context.clone());
                    }
                }
            }
        }
        let missing: Vec<String> = legacy_contexts.difference(&covered).cloned().collect();
        if !missing.is_empty() {
            reasons.push(format!(
                "legacy required status checks not enforced by any active branch ruleset: [{}]",
                missing
                    .iter()
                    .map(|context| format!("`{context}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ));
        }
    }

    if checks.strict {
        let strict_satisfied = rulesets.iter().any(|ruleset| {
            ruleset.rules.iter().any(|rule| {
                if rule.kind != RulesetRuleType::RequiredStatusChecks {
                    return false;
                }
                let Some(parameters) = &rule.parameters else {
                    return false;
                };
                if parameters.strict_required_status_checks_policy != Some(true) {
                    return false;
                }
                let ruleset_contexts: BTreeSet<String> = parameters
                    .required_status_checks
                    .iter()
                    .map(|check| check.context.clone())
                    .collect();
                legacy_contexts.is_subset(&ruleset_contexts)
            })
        });
        if !strict_satisfied {
            reasons.push(
                "legacy `required_status_checks.strict` is not enforced by any active branch \
                 ruleset whose `required_status_checks` covers the legacy context set"
                    .to_owned(),
            );
        }
    }
}

fn check_pull_request_reviews(
    legacy: &Option<crate::github::types::LegacyRequiredPullRequestReviews>,
    rulesets: &[&Ruleset],
    reasons: &mut Vec<String>,
) {
    let Some(reviews) = legacy else {
        return;
    };

    if let Some(bypass) = &reviews.bypass_pull_request_allowances
        && !bypass.is_empty()
    {
        reasons.push(
            "legacy `required_pull_request_reviews.bypass_pull_request_allowances` is non-empty \
             and cannot be mapped to ruleset semantics; refusing to delete"
                .to_owned(),
        );
    }

    let pr_rules: Vec<&crate::github::types::RulesetRuleParameters> = rulesets
        .iter()
        .flat_map(|ruleset| ruleset.rules.iter())
        .filter(|rule| rule.kind == RulesetRuleType::PullRequest)
        .filter_map(|rule| rule.parameters.as_ref())
        .collect();

    if pr_rules.is_empty() {
        reasons.push(
            "legacy `required_pull_request_reviews` is configured but no active branch ruleset \
             contains a `pull_request` rule"
                .to_owned(),
        );
        return;
    }

    if let Some(required_count) = reviews.required_approving_review_count
        && required_count > 0
    {
        let max_count = pr_rules
            .iter()
            .filter_map(|parameters| parameters.required_approving_review_count)
            .max()
            .unwrap_or(0);
        if max_count < required_count {
            reasons.push(format!(
                "legacy requires {required_count} approving reviews but no active branch ruleset \
                 `pull_request` rule requires that many (max is {max_count})",
            ));
        }
    }

    check_pr_boolean(
        reviews.require_code_owner_reviews,
        &pr_rules,
        |parameters| parameters.require_code_owner_review,
        "require_code_owner_reviews",
        "require_code_owner_review",
        reasons,
    );
    check_pr_boolean(
        reviews.dismiss_stale_reviews,
        &pr_rules,
        |parameters| parameters.dismiss_stale_reviews_on_push,
        "dismiss_stale_reviews",
        "dismiss_stale_reviews_on_push",
        reasons,
    );
    check_pr_boolean(
        reviews.require_last_push_approval,
        &pr_rules,
        |parameters| parameters.require_last_push_approval,
        "require_last_push_approval",
        "require_last_push_approval",
        reasons,
    );
    check_pr_boolean(
        reviews.required_review_thread_resolution,
        &pr_rules,
        |parameters| parameters.required_review_thread_resolution,
        "required_review_thread_resolution",
        "required_review_thread_resolution",
        reasons,
    );
}

fn check_pr_boolean(
    legacy_value: bool,
    pr_rules: &[&crate::github::types::RulesetRuleParameters],
    extract: impl Fn(&crate::github::types::RulesetRuleParameters) -> Option<bool>,
    legacy_name: &str,
    ruleset_name: &str,
    reasons: &mut Vec<String>,
) {
    if !legacy_value {
        return;
    }
    let satisfied = pr_rules
        .iter()
        .any(|parameters| extract(parameters) == Some(true));
    if !satisfied {
        reasons.push(format!(
            "legacy `{legacy_name}` is enabled but no active branch ruleset `pull_request` rule \
             sets `{ruleset_name}` to true",
        ));
    }
}

fn check_rule_presence(
    legacy: Option<&crate::github::types::LegacyEnabledFlag>,
    rulesets: &[&Ruleset],
    required_kind: RulesetRuleType,
    legacy_name: &str,
    reasons: &mut Vec<String>,
) {
    let Some(flag) = legacy else {
        return;
    };
    if !flag.enabled {
        return;
    }
    let present = rulesets
        .iter()
        .any(|ruleset| ruleset.rules.iter().any(|rule| rule.kind == required_kind));
    if !present {
        reasons.push(format!(
            "legacy `{legacy_name}` is enabled but no active branch ruleset contains a \
             `{legacy_name}` rule",
        ));
    }
}

fn check_force_push_or_deletion(
    rulesets: &[&Ruleset],
    required_kind: RulesetRuleType,
    ruleset_rule_name: &str,
    legacy_description: &str,
    reasons: &mut Vec<String>,
) {
    let present = rulesets
        .iter()
        .any(|ruleset| ruleset.rules.iter().any(|rule| rule.kind == required_kind));
    if !present {
        reasons.push(format!(
            "legacy protection {legacy_description} but no active branch ruleset contains a \
             `{ruleset_rule_name}` rule",
        ));
    }
}

fn check_conversation_resolution(
    legacy: Option<&crate::github::types::LegacyEnabledFlag>,
    rulesets: &[&Ruleset],
    reasons: &mut Vec<String>,
) {
    let Some(flag) = legacy else {
        return;
    };
    if !flag.enabled {
        return;
    }
    let satisfied = rulesets.iter().any(|ruleset| {
        ruleset.rules.iter().any(|rule| {
            rule.kind == RulesetRuleType::PullRequest
                && rule
                    .parameters
                    .as_ref()
                    .and_then(|parameters| parameters.required_review_thread_resolution)
                    == Some(true)
        })
    });
    if !satisfied {
        reasons.push(
            "legacy `required_conversation_resolution` is enabled but no active branch ruleset \
             `pull_request` rule sets `required_review_thread_resolution` to true"
                .to_owned(),
        );
    }
}

fn check_enforce_admins(
    legacy: Option<&crate::github::types::LegacyEnabledFlag>,
    rulesets: &[&Ruleset],
    reasons: &mut Vec<String>,
) {
    let Some(flag) = legacy else {
        return;
    };
    if !flag.enabled {
        return;
    }
    if let Some(actor_type) = rulesets
        .iter()
        .flat_map(|ruleset| ruleset.bypass_actors.iter())
        .find_map(forbidden_bypass_actor_name)
    {
        reasons.push(format!(
            "legacy `enforce_admins` is enabled but an active branch ruleset allows \
             `{actor_type}` to bypass it",
        ));
    }
}
