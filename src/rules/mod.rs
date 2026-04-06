use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::facts::{RepoFacts, RepoSettings};
use crate::github::types::{
    BypassActorType, RefNameCondition, Ruleset, RulesetConditions, RulesetEnforcement,
    RulesetRuleType, RulesetTarget,
};
use crate::types::RuleId;
use crate::workflow::model::{ActionReference, Step, Workflow};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RepoSetting {
    Private,
    Archived,
    Disabled,
    AllowAutoMerge,
    DeleteBranchOnMerge,
    AllowUpdateBranch,
    AllowSquashMerge,
    AllowMergeCommit,
    AllowRebaseMerge,
}

impl RepoSetting {
    fn name(&self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Archived => "archived",
            Self::Disabled => "disabled",
            Self::AllowAutoMerge => "allow_auto_merge",
            Self::DeleteBranchOnMerge => "delete_branch_on_merge",
            Self::AllowUpdateBranch => "allow_update_branch",
            Self::AllowSquashMerge => "allow_squash_merge",
            Self::AllowMergeCommit => "allow_merge_commit",
            Self::AllowRebaseMerge => "allow_rebase_merge",
        }
    }

    fn read(&self, settings: &RepoSettings) -> SettingValue {
        let value = match self {
            Self::Private => settings.private,
            Self::Archived => settings.archived,
            Self::Disabled => settings.disabled,
            Self::AllowAutoMerge => settings.allow_auto_merge,
            Self::DeleteBranchOnMerge => settings.delete_branch_on_merge,
            Self::AllowUpdateBranch => settings.allow_update_branch,
            Self::AllowSquashMerge => settings.allow_squash_merge,
            Self::AllowMergeCommit => settings.allow_merge_commit,
            Self::AllowRebaseMerge => settings.allow_rebase_merge,
        };

        SettingValue::Bool(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SettingValue {
    Bool(bool),
}

impl SettingValue {
    fn describe(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
        }
    }
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
pub struct Rule {
    pub id: RuleId,
    pub name: String,
    pub kind: RuleKind,
}

impl Rule {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: RuleKind) -> Self {
        Self {
            id: RuleId::new(id),
            name: name.into(),
            kind,
        }
    }

    pub fn evaluate(&self, facts: &RepoFacts) -> RuleOutput {
        RuleOutput {
            id: self.id.clone(),
            name: self.name.clone(),
            result: evaluate(&self.kind, facts),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleOutput {
    pub id: RuleId,
    pub name: String,
    pub result: RuleResult,
}

pub fn default_rules() -> Vec<Rule> {
    vec![
        Rule::new("RS001", "Rulesets exist", RuleKind::RulesetExists),
        Rule::new(
            "RS002",
            "CI status check is required",
            RuleKind::RulesetRequiresStatusCheck {
                check_name: "ci".to_owned(),
            },
        ),
        Rule::new(
            "RS003",
            "Two approving reviews are required",
            RuleKind::RulesetRequiresReviewers { min_count: 2 },
        ),
        Rule::new(
            "RS004",
            "Admins cannot bypass rulesets",
            RuleKind::RulesetEnforcesAdmins,
        ),
        Rule::new(
            "RS005",
            "Rulesets require linear history",
            RuleKind::RulesetRequiresLinearHistory,
        ),
        Rule::new(
            "RS006",
            "Rulesets prevent force pushes",
            RuleKind::RulesetPreventsForcePush,
        ),
        Rule::new(
            "RS007",
            "Repository uses rulesets instead of legacy protection",
            RuleKind::UsesRulesetsNotLegacyProtection,
        ),
        Rule::new(
            "WF001",
            "A workflow runs on pushes to the default branch",
            RuleKind::WorkflowExistsForDefaultBranch,
        ),
        Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        ),
        Rule::new(
            "WF003",
            "No pull_request_target workflow checks out code",
            RuleKind::NoPullRequestTargetWithCheckout,
        ),
        Rule::new(
            "FL001",
            "CODEOWNERS exists",
            RuleKind::FileExists {
                path: "CODEOWNERS".to_owned(),
            },
        ),
        Rule::new("NX001", "flake.nix exists", RuleKind::NixFlakeExists),
        Rule::new(
            "NX002",
            "The flake has observable check coverage",
            RuleKind::NixFlakeHasCheck,
        ),
        Rule::new(
            "ST001",
            "Auto-merge is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowAutoMerge,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST002",
            "Delete branch on merge is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::DeleteBranchOnMerge,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST003",
            "Update branch is enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowUpdateBranch,
                expected: SettingValue::Bool(true),
            },
        ),
        Rule::new(
            "ST004",
            "Merge commits are disabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowMergeCommit,
                expected: SettingValue::Bool(false),
            },
        ),
        Rule::new(
            "ST005",
            "Rebase merges are enabled",
            RuleKind::RepoSettingMatch {
                setting: RepoSetting::AllowRebaseMerge,
                expected: SettingValue::Bool(true),
            },
        ),
    ]
}

pub fn evaluate_rules(rules: &[Rule], facts: &RepoFacts) -> Vec<RuleOutput> {
    rules.iter().map(|rule| rule.evaluate(facts)).collect()
}

pub fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
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
        RuleKind::RulesetRequiresReviewers { min_count } => {
            if !has_active_branch_ruleset_for_default_branch(facts) {
                return RuleResult::Fail {
                    reason: "no active branch ruleset was found".to_owned(),
                };
            }

            if active_branch_rulesets_for_default_branch(facts).any(|ruleset| {
                ruleset.rules.iter().any(|rule| {
                    rule.kind == RulesetRuleType::PullRequest
                        && rule.parameters.as_ref().is_some_and(|parameters| {
                            parameters
                                .required_approving_review_count
                                .unwrap_or_default()
                                >= *min_count
                        })
                })
            }) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "no active branch ruleset requires at least {min_count} approving reviews"
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
                .find_map(admin_bypass_actor_name)
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
        RuleKind::UsesRulesetsNotLegacyProtection => {
            RuleResult::Skip {
                reason: "RepoFacts does not record legacy branch protection state, so this rule cannot yet distinguish rulesets from legacy protection".to_owned(),
            }
        }
        RuleKind::WorkflowExistsForDefaultBranch => {
            let default_branch = facts.default_branch.to_string();

            if facts.workflows.iter().any(|workflow_file| {
                workflow_runs_on_push_to_branch(&workflow_file.workflow, &default_branch)
            }) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "no workflow runs on pushes to the default branch `{default_branch}`"
                    ),
                }
            }
        }
        RuleKind::WorkflowHasJob { job_name } => {
            if facts
                .workflows
                .iter()
                .any(|workflow_file| workflow_file.workflow.jobs.contains_key(job_name))
            {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("no workflow defines the job `{job_name}`"),
                }
            }
        }
        RuleKind::WorkflowActionsPinnedToSha => {
            let offenders = facts
                .workflows
                .iter()
                .flat_map(|workflow_file| {
                    workflow_file
                        .workflow
                        .jobs
                        .values()
                        .flat_map(|job| job.steps.iter())
                        .filter_map(|step| step.uses())
                        .filter(|uses| !action_reference_is_pinned_to_sha(uses))
                        .map(|uses| {
                            format!(
                                "{} uses {}",
                                workflow_file.path,
                                action_reference_text(uses)
                            )
                        })
                })
                .collect::<Vec<_>>();

            if offenders.is_empty() {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "workflow actions must be pinned to 40-character commit SHAs: {}",
                        summarize_examples(&offenders)
                    ),
                }
            }
        }
        RuleKind::NoPullRequestTargetWithCheckout => {
            let offenders = facts
                .workflows
                .iter()
                .filter(|workflow_file| {
                    workflow_file
                        .workflow
                        .triggers
                        .pull_request_target
                        .is_some()
                })
                .filter(|workflow_file| {
                    workflow_uses_action(&workflow_file.workflow, "actions/checkout")
                })
                .map(|workflow_file| workflow_file.path.clone())
                .collect::<Vec<_>>();

            if offenders.is_empty() {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "pull_request_target workflows must not use actions/checkout: {}",
                        offenders.join(", ")
                    ),
                }
            }
        }
        RuleKind::WorkflowUsesAction { action } => {
            if facts
                .workflows
                .iter()
                .any(|workflow_file| workflow_uses_action(&workflow_file.workflow, action))
            {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("no workflow uses the action `{action}`"),
                }
            }
        }
        RuleKind::FileExists { path } => {
            if facts.files_present.contains(path) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("required file `{path}` is missing"),
                }
            }
        }
        RuleKind::NixFlakeExists => {
            if facts.files_present.contains("flake.nix") {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: "required file `flake.nix` is missing".to_owned(),
                }
            }
        }
        RuleKind::NixFlakeHasCheck => {
            if !facts.files_present.contains("flake.nix") {
                RuleResult::Fail {
                    reason: "cannot observe flake checks because `flake.nix` is missing".to_owned(),
                }
            } else if workflows_run_nix_flake_check(facts) {
                RuleResult::Pass
            } else {
                RuleResult::Skip {
                    reason: "RepoFacts does not yet capture flake outputs; only explicit `nix flake check` workflow steps can prove this rule".to_owned(),
                }
            }
        }
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
/// pattern and must not match any exclude pattern.
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
    let included = ref_name.include.is_empty()
        || ref_name
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

fn admin_bypass_actor_name(actor: &crate::github::types::BypassActor) -> Option<&'static str> {
    match actor.actor_type {
        BypassActorType::OrganizationAdmin => Some("OrganizationAdmin"),
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

fn workflow_runs_on_push_to_branch(workflow: &Workflow, branch: &str) -> bool {
    workflow.triggers.push.as_ref().is_some_and(|push| {
        if !has_branch_push_filters(push) && has_tag_push_filters(push) {
            return false;
        }

        branch_matches_filters(&push.branches, branch)
            && !push
                .branches_ignore
                .iter()
                .any(|pattern| branch_pattern_matches(pattern, branch))
    })
}

fn branch_matches_filters(filters: &[String], branch: &str) -> bool {
    if filters.is_empty() {
        return true;
    }

    let mut matched = false;
    let mut saw_positive_pattern = false;

    for filter in filters {
        let (negated, pattern) = if let Some(pattern) = filter.strip_prefix('!') {
            (true, pattern)
        } else {
            saw_positive_pattern = true;
            (false, filter.as_str())
        };

        if branch_pattern_matches(pattern, branch) {
            matched = !negated;
        }
    }

    saw_positive_pattern && matched
}

fn branch_pattern_matches(pattern: &str, branch: &str) -> bool {
    branch_pattern_regex(pattern).is_some_and(|regex| regex.is_match(branch))
}

fn has_branch_push_filters(push: &crate::workflow::model::TriggerFilter) -> bool {
    !push.branches.is_empty() || !push.branches_ignore.is_empty()
}

fn has_tag_push_filters(push: &crate::workflow::model::TriggerFilter) -> bool {
    !push.tags.is_empty() || !push.tags_ignore.is_empty()
}

fn branch_pattern_regex(pattern: &str) -> Option<Regex> {
    let body = github_pattern_to_regex(pattern)?;
    Regex::new(&format!("^{body}$")).ok()
}

fn github_pattern_to_regex(pattern: &str) -> Option<String> {
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut regex = String::new();
    let mut index = 0usize;
    let mut previous_token_is_quantifiable = false;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let escaped = chars.get(index + 1).copied().unwrap_or('\\');
                push_escaped_char(&mut regex, escaped);
                previous_token_is_quantifiable = true;
                index += if index + 1 < chars.len() { 2 } else { 1 };
            }
            '*' => {
                if chars.get(index + 1) == Some(&'*') {
                    regex.push_str(".*");
                    index += 2;
                } else {
                    regex.push_str("[^/]*");
                    index += 1;
                }
                previous_token_is_quantifiable = false;
            }
            '?' | '+' => {
                if !previous_token_is_quantifiable {
                    return None;
                }

                regex.push(chars[index]);
                previous_token_is_quantifiable = false;
                index += 1;
            }
            '[' => {
                let (class_regex, next_index) = parse_character_class(&chars, index)?;
                regex.push_str(&class_regex);
                previous_token_is_quantifiable = true;
                index = next_index;
            }
            ch => {
                push_escaped_char(&mut regex, ch);
                previous_token_is_quantifiable = true;
                index += 1;
            }
        }
    }

    Some(regex)
}

fn parse_character_class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut regex = String::from("[");
    let mut index = start + 1;
    let mut saw_content = false;

    if chars.get(index) == Some(&'!') {
        regex.push('^');
        index += 1;
    }

    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let escaped = chars.get(index + 1).copied().unwrap_or('\\');
                push_regex_class_literal(&mut regex, escaped);
                saw_content = true;
                index += if index + 1 < chars.len() { 2 } else { 1 };
            }
            ']' if saw_content => {
                regex.push(']');
                return Some((regex, index + 1));
            }
            '[' | '^' => {
                push_regex_class_literal(&mut regex, chars[index]);
                saw_content = true;
                index += 1;
            }
            '-' => {
                regex.push('-');
                saw_content = true;
                index += 1;
            }
            ch => {
                regex.push(ch);
                saw_content = true;
                index += 1;
            }
        }
    }

    None
}

fn push_escaped_char(regex: &mut String, ch: char) {
    regex.push_str(&regex::escape(&ch.to_string()));
}

fn push_regex_class_literal(regex: &mut String, ch: char) {
    match ch {
        '\\' | '[' | ']' | '^' | '-' => {
            regex.push('\\');
            regex.push(ch);
        }
        _ => regex.push(ch),
    }
}

fn workflow_uses_action(workflow: &Workflow, action: &str) -> bool {
    workflow
        .jobs
        .values()
        .flat_map(|job| job.steps.iter())
        .any(|step| step_uses_action(step, action))
}

fn step_uses_action(step: &Step, action: &str) -> bool {
    let Some(uses) = step.uses() else {
        return false;
    };

    match uses {
        ActionReference::Repository(action_ref) => {
            let action_name = format!("{}/{}", action_ref.owner, action_ref.repo);
            action == action_name || action == action_ref.to_string()
        }
        ActionReference::Other(raw) => action_reference_matches(raw, action),
    }
}

fn action_reference_matches(raw: &str, action: &str) -> bool {
    if action.contains('@') {
        raw == action
    } else {
        raw == action
            || raw
                .strip_prefix(action)
                .is_some_and(|suffix| suffix.starts_with('@') || suffix.starts_with('/'))
    }
}

fn action_reference_is_pinned_to_sha(uses: &ActionReference) -> bool {
    match uses {
        ActionReference::Repository(action_ref) => is_commit_sha(&action_ref.version),
        ActionReference::Other(raw) => {
            if raw.starts_with("./") || raw.starts_with("docker://") {
                true
            } else if let Some((_, version)) = raw.split_once('@') {
                is_commit_sha(version)
            } else {
                false
            }
        }
    }
}

fn action_reference_text(uses: &ActionReference) -> String {
    match uses {
        ActionReference::Repository(action_ref) => action_ref.to_string(),
        ActionReference::Other(raw) => raw.clone(),
    }
}

fn is_commit_sha(version: &str) -> bool {
    version.len() == 40 && version.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn summarize_examples(values: &[String]) -> String {
    const MAX_EXAMPLES: usize = 3;

    if values.len() <= MAX_EXAMPLES {
        values.join(", ")
    } else {
        let extra = values.len() - MAX_EXAMPLES;
        format!("{}, and {extra} more", values[..MAX_EXAMPLES].join(", "))
    }
}

fn workflows_run_nix_flake_check(facts: &RepoFacts) -> bool {
    facts.workflows.iter().any(|workflow_file| {
        workflow_file
            .workflow
            .jobs
            .values()
            .flat_map(|job| job.steps.iter())
            .filter_map(|step| step.run())
            .any(|run| run.contains("nix flake check"))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    use proptest::prelude::*;

    use super::*;
    use crate::facts::{RepoFacts, RepoSettings, WorkflowFile};
    use crate::github::types::{
        BypassActor, BypassMode, RefNameCondition, RequiredStatusCheck, Ruleset, RulesetConditions,
        RulesetEnforcement, RulesetRule, RulesetRuleParameters, RulesetRuleType, RulesetTarget,
    };
    use crate::types::{BranchName, RepoRef};
    use crate::workflow::model::{
        ActionRef, ActionStep, Job, RunStep, Step, StepKind, TriggerFilter, Triggers, Workflow,
        WorkflowDispatch,
    };

    fn reason() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9 .,;:!?-]{0,100}"
    }

    fn identifier() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{0,12}"
    }

    fn path_fragment() -> impl Strategy<Value = String> {
        "[A-Za-z0-9_./-]{1,30}"
    }

    fn version() -> impl Strategy<Value = String> {
        "[A-Za-z0-9._/-]{1,20}"
    }

    fn sha() -> impl Strategy<Value = String> {
        "[0-9a-f]{40}"
    }

    fn repo_ref_strategy() -> impl Strategy<Value = RepoRef> {
        (identifier(), identifier()).prop_map(|(owner, name)| RepoRef::new(owner, name))
    }

    fn repo_settings_strategy() -> impl Strategy<Value = RepoSettings> {
        (
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(
                    private,
                    archived,
                    disabled,
                    allow_auto_merge,
                    delete_branch_on_merge,
                    allow_update_branch,
                    allow_squash_merge,
                    allow_merge_commit,
                    allow_rebase_merge,
                )| RepoSettings {
                    private,
                    archived,
                    disabled,
                    allow_auto_merge,
                    delete_branch_on_merge,
                    allow_update_branch,
                    allow_squash_merge,
                    allow_merge_commit,
                    allow_rebase_merge,
                },
            )
    }

    fn bypass_actor_type_strategy() -> impl Strategy<Value = BypassActorType> {
        prop_oneof![
            Just(BypassActorType::OrganizationAdmin),
            Just(BypassActorType::RepositoryRole),
            Just(BypassActorType::Team),
            Just(BypassActorType::Integration),
            Just(BypassActorType::DeployKey),
        ]
    }

    fn bypass_mode_strategy() -> impl Strategy<Value = BypassMode> {
        prop_oneof![Just(BypassMode::Always), Just(BypassMode::PullRequest)]
    }

    fn bypass_actor_strategy() -> impl Strategy<Value = BypassActor> {
        (
            proptest::option::of(any::<u64>()),
            bypass_actor_type_strategy(),
            bypass_mode_strategy(),
        )
            .prop_map(|(actor_id, actor_type, bypass_mode)| BypassActor {
                actor_id,
                actor_type,
                bypass_mode,
            })
    }

    fn ruleset_rule_type_strategy() -> impl Strategy<Value = RulesetRuleType> {
        prop_oneof![
            Just(RulesetRuleType::Creation),
            Just(RulesetRuleType::Update),
            Just(RulesetRuleType::Deletion),
            Just(RulesetRuleType::RequiredLinearHistory),
            Just(RulesetRuleType::RequiredSignatures),
            Just(RulesetRuleType::PullRequest),
            Just(RulesetRuleType::RequiredStatusChecks),
            Just(RulesetRuleType::NonFastForward),
        ]
    }

    fn required_status_check_strategy() -> impl Strategy<Value = RequiredStatusCheck> {
        (identifier(), proptest::option::of(any::<u64>())).prop_map(|(context, integration_id)| {
            RequiredStatusCheck {
                context,
                integration_id,
            }
        })
    }

    fn ruleset_rule_parameters_strategy() -> impl Strategy<Value = RulesetRuleParameters> {
        (
            proptest::collection::vec(required_status_check_strategy(), 0..3),
            proptest::option::of(any::<bool>()),
            proptest::option::of(0u32..5),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
            proptest::option::of(any::<bool>()),
        )
            .prop_map(
                |(
                    required_status_checks,
                    strict_required_status_checks_policy,
                    required_approving_review_count,
                    require_code_owner_review,
                    require_last_push_approval,
                    required_review_thread_resolution,
                    dismiss_stale_reviews_on_push,
                    do_not_enforce_on_create,
                )| RulesetRuleParameters {
                    required_status_checks,
                    strict_required_status_checks_policy,
                    required_approving_review_count,
                    require_code_owner_review,
                    require_last_push_approval,
                    required_review_thread_resolution,
                    dismiss_stale_reviews_on_push,
                    do_not_enforce_on_create,
                },
            )
    }

    fn ruleset_rule_strategy() -> impl Strategy<Value = RulesetRule> {
        (
            ruleset_rule_type_strategy(),
            proptest::option::of(ruleset_rule_parameters_strategy()),
        )
            .prop_map(|(kind, parameters)| RulesetRule { kind, parameters })
    }

    fn ruleset_target_strategy() -> impl Strategy<Value = RulesetTarget> {
        prop_oneof![
            Just(RulesetTarget::Branch),
            Just(RulesetTarget::Tag),
            Just(RulesetTarget::Push),
        ]
    }

    fn ruleset_enforcement_strategy() -> impl Strategy<Value = RulesetEnforcement> {
        prop_oneof![
            Just(RulesetEnforcement::Active),
            Just(RulesetEnforcement::Evaluate),
            Just(RulesetEnforcement::Disabled),
        ]
    }

    fn ref_name_condition_strategy() -> impl Strategy<Value = RefNameCondition> {
        (
            proptest::collection::vec(
                prop_oneof![
                    Just("~DEFAULT_BRANCH".to_owned()),
                    Just("~ALL".to_owned()),
                    path_fragment(),
                ],
                0..3,
            ),
            proptest::collection::vec(path_fragment(), 0..2),
        )
            .prop_map(|(include, exclude)| RefNameCondition { include, exclude })
    }

    fn ruleset_conditions_strategy() -> impl Strategy<Value = Option<RulesetConditions>> {
        proptest::option::of(
            proptest::option::of(ref_name_condition_strategy())
                .prop_map(|ref_name| RulesetConditions { ref_name }),
        )
    }

    fn ruleset_strategy() -> impl Strategy<Value = Ruleset> {
        (
            any::<u64>(),
            path_fragment(),
            ruleset_target_strategy(),
            ruleset_enforcement_strategy(),
            ruleset_conditions_strategy(),
            proptest::collection::vec(bypass_actor_strategy(), 0..3),
            proptest::collection::vec(ruleset_rule_strategy(), 0..4),
        )
            .prop_map(
                |(id, name, target, enforcement, conditions, bypass_actors, rules)| Ruleset {
                    id,
                    name,
                    target,
                    enforcement,
                    conditions,
                    bypass_actors,
                    rules,
                },
            )
    }

    fn trigger_filter_strategy() -> impl Strategy<Value = TriggerFilter> {
        (
            proptest::collection::vec(path_fragment(), 0..3),
            proptest::collection::vec(path_fragment(), 0..3),
            proptest::collection::vec(path_fragment(), 0..3),
            proptest::collection::vec(path_fragment(), 0..3),
            proptest::collection::vec(path_fragment(), 0..3),
        )
            .prop_map(|(branches, branches_ignore, tags, tags_ignore, paths)| {
                TriggerFilter {
                    branches,
                    branches_ignore,
                    tags,
                    tags_ignore,
                    paths,
                }
            })
    }

    fn action_reference_strategy() -> impl Strategy<Value = ActionReference> {
        prop_oneof![
            (identifier(), identifier(), version()).prop_map(|(owner, repo, version)| {
                ActionReference::Repository(ActionRef::new(owner, repo, version))
            }),
            "[./A-Za-z0-9:_@/-]{1,40}".prop_map(ActionReference::Other),
        ]
    }

    fn step_strategy() -> impl Strategy<Value = Step> {
        let action_step = action_reference_strategy().prop_map(|uses| Step {
            name: None,
            id: None,
            condition: None,
            kind: StepKind::Action(ActionStep {
                uses,
                with: BTreeMap::new(),
            }),
        });
        let run_step = ".{1,40}".prop_map(|run| Step {
            name: None,
            id: None,
            condition: None,
            kind: StepKind::Run(RunStep { run }),
        });

        prop_oneof![action_step, run_step]
    }

    fn workflow_strategy() -> impl Strategy<Value = Workflow> {
        (
            proptest::option::of(path_fragment()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            any::<bool>(),
            proptest::collection::btree_map(
                identifier(),
                proptest::collection::vec(step_strategy(), 0..4),
                0..4,
            ),
        )
            .prop_map(
                |(name, push, pull_request, pull_request_target, workflow_dispatch, jobs)| {
                    Workflow {
                        name,
                        triggers: Triggers {
                            push,
                            pull_request,
                            pull_request_target,
                            workflow_dispatch: workflow_dispatch
                                .then_some(WorkflowDispatch::default()),
                        },
                        jobs: jobs
                            .into_iter()
                            .map(|(name, steps)| {
                                (
                                    name,
                                    Job {
                                        runs_on: None,
                                        steps,
                                        needs: Vec::new(),
                                        condition: None,
                                    },
                                )
                            })
                            .collect(),
                    }
                },
            )
    }

    fn workflow_file_strategy() -> impl Strategy<Value = WorkflowFile> {
        (path_fragment(), workflow_strategy())
            .prop_map(|(path, workflow)| WorkflowFile { path, workflow })
    }

    fn repo_facts_strategy() -> impl Strategy<Value = RepoFacts> {
        (
            repo_ref_strategy(),
            repo_settings_strategy(),
            proptest::collection::vec(ruleset_strategy(), 0..4),
            identifier(),
            proptest::collection::vec(workflow_file_strategy(), 0..4),
            proptest::collection::btree_set(path_fragment(), 0..8),
        )
            .prop_map(
                |(repo, settings, rulesets, default_branch, workflows, files_present)| RepoFacts {
                    repo,
                    settings,
                    rulesets,
                    default_branch: BranchName::new(default_branch),
                    workflows,
                    files_present,
                },
            )
    }

    fn repo_setting_strategy() -> impl Strategy<Value = RepoSetting> {
        prop_oneof![
            Just(RepoSetting::Private),
            Just(RepoSetting::Archived),
            Just(RepoSetting::Disabled),
            Just(RepoSetting::AllowAutoMerge),
            Just(RepoSetting::DeleteBranchOnMerge),
            Just(RepoSetting::AllowUpdateBranch),
            Just(RepoSetting::AllowSquashMerge),
            Just(RepoSetting::AllowMergeCommit),
            Just(RepoSetting::AllowRebaseMerge),
        ]
    }

    fn setting_value_strategy() -> impl Strategy<Value = SettingValue> {
        any::<bool>().prop_map(SettingValue::Bool)
    }

    fn rule_kind_strategy() -> impl Strategy<Value = RuleKind> {
        prop_oneof![
            Just(RuleKind::RulesetExists),
            identifier().prop_map(|check_name| RuleKind::RulesetRequiresStatusCheck { check_name }),
            (0u32..5).prop_map(|min_count| RuleKind::RulesetRequiresReviewers { min_count }),
            Just(RuleKind::RulesetEnforcesAdmins),
            Just(RuleKind::RulesetRequiresLinearHistory),
            Just(RuleKind::RulesetPreventsForcePush),
            Just(RuleKind::UsesRulesetsNotLegacyProtection),
            Just(RuleKind::WorkflowExistsForDefaultBranch),
            identifier().prop_map(|job_name| RuleKind::WorkflowHasJob { job_name }),
            Just(RuleKind::WorkflowActionsPinnedToSha),
            Just(RuleKind::NoPullRequestTargetWithCheckout),
            (identifier(), identifier()).prop_map(|(owner, repo)| RuleKind::WorkflowUsesAction {
                action: format!("{owner}/{repo}"),
            }),
            path_fragment().prop_map(|path| RuleKind::FileExists { path }),
            Just(RuleKind::NixFlakeExists),
            Just(RuleKind::NixFlakeHasCheck),
            (repo_setting_strategy(), setting_value_strategy()).prop_map(|(setting, expected)| {
                RuleKind::RepoSettingMatch { setting, expected }
            }),
        ]
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

    fn glob_literal_char_strategy() -> impl Strategy<Value = char> {
        let ascii_letters = ('a'..='z').collect::<Vec<_>>();
        let digits = ('0'..='9').collect::<Vec<_>>();

        prop_oneof![
            proptest::sample::select(ascii_letters),
            proptest::sample::select(digits),
            Just('/'),
            Just('-'),
            Just('_'),
            Just('.'),
        ]
    }

    fn glob_pattern_subset_strategy() -> impl Strategy<Value = String> {
        let literal = proptest::collection::vec(glob_literal_char_strategy(), 1..=3)
            .prop_map(|chars| chars.into_iter().collect::<String>());
        let quantified_literal = (
            glob_literal_char_strategy()
                .prop_filter("wildcards are not quantifiable literals", |ch| *ch != '*'),
            prop_oneof![Just('?'), Just('+')],
        )
            .prop_map(|(ch, quantifier)| format!("{ch}{quantifier}"));
        let escaped = prop_oneof![
            Just("\\*".to_owned()),
            Just("\\?".to_owned()),
            Just("\\+".to_owned()),
            Just("\\[".to_owned()),
            Just("\\]".to_owned()),
            Just("\\!".to_owned()),
            Just("\\\\".to_owned()),
        ];

        proptest::collection::vec(
            prop_oneof![
                literal,
                quantified_literal,
                Just("*".to_owned()),
                Just("**".to_owned()),
                escaped,
            ],
            0..8,
        )
        .prop_map(|parts| parts.concat())
    }

    fn branch_name_strategy() -> impl Strategy<Value = String> {
        proptest::collection::vec(glob_literal_char_strategy(), 0..12)
            .prop_map(|chars| chars.into_iter().collect::<String>())
    }

    fn empty_repo_settings() -> RepoSettings {
        RepoSettings {
            private: false,
            archived: false,
            disabled: false,
            allow_auto_merge: false,
            delete_branch_on_merge: false,
            allow_update_branch: false,
            allow_squash_merge: false,
            allow_merge_commit: false,
            allow_rebase_merge: false,
        }
    }

    fn base_facts() -> RepoFacts {
        RepoFacts {
            repo: RepoRef::new("example", "repo"),
            settings: empty_repo_settings(),
            rulesets: Vec::new(),
            default_branch: BranchName::new("main"),
            workflows: Vec::new(),
            files_present: BTreeSet::new(),
        }
    }

    fn active_branch_ruleset(rules: Vec<RulesetRule>) -> Ruleset {
        Ruleset {
            id: 1,
            name: "main protection".to_owned(),
            target: RulesetTarget::Branch,
            enforcement: RulesetEnforcement::Active,
            conditions: Some(RulesetConditions {
                ref_name: Some(RefNameCondition {
                    include: vec!["~DEFAULT_BRANCH".to_owned()],
                    exclude: Vec::new(),
                }),
            }),
            bypass_actors: Vec::new(),
            rules,
        }
    }

    fn workflow_with_single_job(job_name: &str, steps: Vec<Step>) -> WorkflowFile {
        WorkflowFile {
            path: ".github/workflows/ci.yml".to_owned(),
            workflow: Workflow {
                name: Some("CI".to_owned()),
                triggers: Triggers {
                    push: Some(TriggerFilter {
                        branches: vec!["main".to_owned()],
                        branches_ignore: Vec::new(),
                        tags: Vec::new(),
                        tags_ignore: Vec::new(),
                        paths: Vec::new(),
                    }),
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::from([(
                    job_name.to_owned(),
                    Job {
                        runs_on: None,
                        steps,
                        needs: Vec::new(),
                        condition: None,
                    },
                )]),
            },
        }
    }

    fn action_step(uses: ActionReference) -> Step {
        Step {
            name: None,
            id: None,
            condition: None,
            kind: StepKind::Action(ActionStep {
                uses,
                with: BTreeMap::new(),
            }),
        }
    }

    fn run_step(run: &str) -> Step {
        Step {
            name: None,
            id: None,
            condition: None,
            kind: StepKind::Run(RunStep {
                run: run.to_owned(),
            }),
        }
    }

    fn good_fixture() -> RepoFacts {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/example-org/good-repo.json"
        )))
        .unwrap()
    }

    fn bad_fixture() -> RepoFacts {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/example-org/bad-repo.json"
        )))
        .unwrap()
    }

    fn result_tag(result: &RuleResult) -> &'static str {
        match result {
            RuleResult::Pass => "pass",
            RuleResult::Fail { .. } => "fail",
            RuleResult::Skip { .. } => "skip",
            RuleResult::Error { .. } => "error",
        }
    }

    fn reference_branch_pattern_matches(pattern: &str, branch: &str) -> bool {
        fn go(
            pattern: &[char],
            pattern_index: usize,
            branch: &[char],
            branch_index: usize,
            memo: &mut HashMap<(usize, usize), bool>,
        ) -> bool {
            if let Some(result) = memo.get(&(pattern_index, branch_index)) {
                return *result;
            }

            let result = if pattern_index == pattern.len() {
                branch_index == branch.len()
            } else {
                match pattern[pattern_index] {
                    '\\' => {
                        let escaped = pattern.get(pattern_index + 1).copied().unwrap_or('\\');
                        let next_pattern_index = if pattern_index + 1 < pattern.len() {
                            pattern_index + 2
                        } else {
                            pattern_index + 1
                        };

                        branch.get(branch_index) == Some(&escaped)
                            && go(pattern, next_pattern_index, branch, branch_index + 1, memo)
                    }
                    '*' if pattern.get(pattern_index + 1) == Some(&'*') => {
                        (branch_index..=branch.len()).any(|next_branch_index| {
                            go(pattern, pattern_index + 2, branch, next_branch_index, memo)
                        })
                    }
                    '*' => {
                        let zero_width_match =
                            go(pattern, pattern_index + 1, branch, branch_index, memo);
                        zero_width_match
                            || (branch_index..branch.len())
                                .take_while(|index| branch[*index] != '/')
                                .map(|index| index + 1)
                                .any(|next_branch_index| {
                                    go(pattern, pattern_index + 1, branch, next_branch_index, memo)
                                })
                    }
                    ch => {
                        let (min_count, max_count, next_pattern_index) =
                            match pattern.get(pattern_index + 1).copied() {
                                Some('?') => (0usize, 1usize, pattern_index + 2),
                                Some('+') => (1usize, usize::MAX, pattern_index + 2),
                                _ => {
                                    return branch.get(branch_index) == Some(&ch)
                                        && go(
                                            pattern,
                                            pattern_index + 1,
                                            branch,
                                            branch_index + 1,
                                            memo,
                                        );
                                }
                            };

                        let mut matched_count = 0usize;
                        let mut next_branch_index = branch_index;

                        while next_branch_index < branch.len() && branch[next_branch_index] == ch {
                            matched_count += 1;
                            next_branch_index += 1;
                        }

                        if matched_count < min_count {
                            false
                        } else {
                            let upper_bound = matched_count.min(max_count);
                            (min_count..=upper_bound).any(|count| {
                                go(
                                    pattern,
                                    next_pattern_index,
                                    branch,
                                    branch_index + count,
                                    memo,
                                )
                            })
                        }
                    }
                }
            };

            memo.insert((pattern_index, branch_index), result);
            result
        }

        let pattern = pattern.chars().collect::<Vec<_>>();
        let branch = branch.chars().collect::<Vec<_>>();
        let mut memo = HashMap::new();

        go(&pattern, 0, &branch, 0, &mut memo)
    }

    fn reference_branch_matches_filters(filters: &[String], branch: &str) -> bool {
        if filters.is_empty() {
            return true;
        }

        let mut matched = false;
        let mut saw_positive_pattern = false;

        for filter in filters {
            let (negated, pattern) = if let Some(pattern) = filter.strip_prefix('!') {
                (true, pattern)
            } else {
                saw_positive_pattern = true;
                (false, filter.as_str())
            };

            if reference_branch_pattern_matches(pattern, branch) {
                matched = !negated;
            }
        }

        saw_positive_pattern && matched
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

        #[test]
        fn ruleset_exists_fails_when_rulesets_are_empty(
            repo in repo_ref_strategy(),
            settings in repo_settings_strategy(),
            default_branch in identifier(),
            workflows in proptest::collection::vec(workflow_file_strategy(), 0..4),
            files_present in proptest::collection::btree_set(path_fragment(), 0..8),
        ) {
            let facts = RepoFacts {
                repo,
                settings,
                rulesets: Vec::new(),
                default_branch: BranchName::new(default_branch),
                workflows,
                files_present,
            };

            let result = evaluate(&RuleKind::RulesetExists, &facts);
            let is_fail = matches!(result, RuleResult::Fail { .. });
            prop_assert!(is_fail);
        }

        #[test]
        fn ruleset_exists_passes_when_active_branch_ruleset_includes_default_branch(
            mut facts in repo_facts_strategy(),
            mut ruleset in ruleset_strategy(),
        ) {
            // Force the ruleset to be an active branch ruleset that applies to the default branch.
            ruleset.target = RulesetTarget::Branch;
            ruleset.enforcement = RulesetEnforcement::Active;
            ruleset.conditions = Some(RulesetConditions {
                ref_name: Some(RefNameCondition {
                    include: vec!["~DEFAULT_BRANCH".to_owned()],
                    exclude: Vec::new(),
                }),
            });
            facts.rulesets = vec![ruleset];
            prop_assert_eq!(evaluate(&RuleKind::RulesetExists, &facts), RuleResult::Pass);
        }

        #[test]
        fn workflow_actions_pinned_to_sha_fails_for_unpinned_repository_actions(
            mut facts in repo_facts_strategy(),
            owner in identifier(),
            repo in identifier(),
            version in version().prop_filter("version must not already be a full commit sha", |version| !is_commit_sha(version)),
        ) {
            facts.workflows = vec![workflow_with_single_job(
                "build",
                vec![action_step(ActionReference::Repository(ActionRef::new(owner, repo, version)))],
            )];

            let result = evaluate(&RuleKind::WorkflowActionsPinnedToSha, &facts);
            let is_fail = matches!(result, RuleResult::Fail { .. });
            prop_assert!(is_fail);
        }

        #[test]
        fn workflow_actions_pinned_to_sha_passes_for_full_commit_shas(
            mut facts in repo_facts_strategy(),
            versions in proptest::collection::vec(sha(), 1..4),
        ) {
            facts.workflows = versions
                .into_iter()
                .enumerate()
                .map(|(index, version)| {
                    workflow_with_single_job(
                        &format!("build-{index}"),
                        vec![action_step(ActionReference::Repository(ActionRef::new(
                            "actions",
                            "checkout",
                            version,
                        )))],
                    )
                })
                .collect();

            prop_assert_eq!(
                evaluate(&RuleKind::WorkflowActionsPinnedToSha, &facts),
                RuleResult::Pass
            );
        }

        #[test]
        fn file_exists_fails_when_path_is_missing(
            path in path_fragment(),
            present_paths in proptest::collection::btree_set(path_fragment(), 0..8),
        ) {
            prop_assume!(!present_paths.contains(&path));

            let mut facts = base_facts();
            facts.files_present = present_paths;

            let result = evaluate(&RuleKind::FileExists { path }, &facts);
            let is_fail = matches!(result, RuleResult::Fail { .. });
            prop_assert!(is_fail);
        }

        #[test]
        fn evaluate_never_panics(
            facts in repo_facts_strategy(),
            kind in rule_kind_strategy(),
        ) {
            let result = evaluate(&kind, &facts);
            let is_valid_variant = matches!(
                result,
                RuleResult::Pass
                    | RuleResult::Fail { .. }
                    | RuleResult::Skip { .. }
                    | RuleResult::Error { .. }
            );
            prop_assert!(is_valid_variant);
        }

        #[test]
        fn branch_pattern_matches_agrees_with_reference_for_core_glob_subset(
            pattern in glob_pattern_subset_strategy(),
            branch in branch_name_strategy(),
        ) {
            prop_assert_eq!(
                branch_pattern_matches(&pattern, &branch),
                reference_branch_pattern_matches(&pattern, &branch)
            );
        }

        #[test]
        fn branch_matches_filters_agrees_with_reference_for_core_glob_subset(
            raw_filters in proptest::collection::vec(
                (any::<bool>(), glob_pattern_subset_strategy()),
                0..6,
            ),
            branch in branch_name_strategy(),
        ) {
            let filters = raw_filters
                .into_iter()
                .enumerate()
                .map(|(index, (negated, pattern))| {
                    if negated && index > 0 {
                        format!("!{pattern}")
                    } else {
                        pattern
                    }
                })
                .collect::<Vec<_>>();

            prop_assert_eq!(
                branch_matches_filters(&filters, &branch),
                reference_branch_matches_filters(&filters, &branch)
            );
        }
    }

    #[test]
    fn default_rule_ids_are_unique() {
        let ids = default_rules()
            .into_iter()
            .map(|rule| rule.id.to_string())
            .collect::<Vec<_>>();
        let unique = ids.iter().cloned().collect::<BTreeSet<_>>();

        assert_eq!(unique.len(), ids.len());
    }

    #[test]
    fn workflow_has_job_passes_when_job_exists() {
        let mut facts = base_facts();
        facts.workflows = vec![workflow_with_single_job("build-and-test", Vec::new())];

        assert_eq!(
            evaluate(
                &RuleKind::WorkflowHasJob {
                    job_name: "build-and-test".to_owned(),
                },
                &facts,
            ),
            RuleResult::Pass
        );
    }

    #[test]
    fn workflow_uses_action_matches_repository_actions() {
        let mut facts = base_facts();
        facts.workflows = vec![workflow_with_single_job(
            "build",
            vec![action_step(ActionReference::Repository(ActionRef::new(
                "actions", "checkout", "v4",
            )))],
        )];

        assert_eq!(
            evaluate(
                &RuleKind::WorkflowUsesAction {
                    action: "actions/checkout".to_owned(),
                },
                &facts,
            ),
            RuleResult::Pass
        );
    }

    #[test]
    fn repo_setting_match_reads_boolean_settings() {
        let mut facts = base_facts();
        facts.settings.allow_auto_merge = true;

        assert_eq!(
            evaluate(
                &RuleKind::RepoSettingMatch {
                    setting: RepoSetting::AllowAutoMerge,
                    expected: SettingValue::Bool(true),
                },
                &facts,
            ),
            RuleResult::Pass
        );
        assert!(matches!(
            evaluate(
                &RuleKind::RepoSettingMatch {
                    setting: RepoSetting::AllowAutoMerge,
                    expected: SettingValue::Bool(false),
                },
                &facts,
            ),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn nix_flake_has_check_passes_when_workflow_runs_nix_flake_check() {
        let mut facts = base_facts();
        facts.files_present.insert("flake.nix".to_owned());
        facts.workflows = vec![workflow_with_single_job(
            "build",
            vec![run_step("nix flake check")],
        )];

        assert_eq!(
            evaluate(&RuleKind::NixFlakeHasCheck, &facts),
            RuleResult::Pass
        );
    }

    #[test]
    fn uses_rulesets_not_legacy_protection_skips_until_facts_include_legacy_state() {
        let facts = base_facts();

        assert!(matches!(
            evaluate(&RuleKind::UsesRulesetsNotLegacyProtection, &facts),
            RuleResult::Skip { .. }
        ));
    }

    #[test]
    fn workflow_exists_for_default_branch_respects_single_star_slash_boundaries() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("release/train/main");
        facts.workflows = vec![WorkflowFile {
            path: ".github/workflows/release.yml".to_owned(),
            workflow: Workflow {
                name: Some("Release".to_owned()),
                triggers: Triggers {
                    push: Some(TriggerFilter {
                        branches: vec!["release/*".to_owned()],
                        branches_ignore: Vec::new(),
                        tags: Vec::new(),
                        tags_ignore: Vec::new(),
                        paths: Vec::new(),
                    }),
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::new(),
            },
        }];

        assert!(matches!(
            evaluate(&RuleKind::WorkflowExistsForDefaultBranch, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn workflow_exists_for_default_branch_supports_double_star_and_negation_order() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("release/beta/3-alpha");
        facts.workflows = vec![WorkflowFile {
            path: ".github/workflows/release.yml".to_owned(),
            workflow: Workflow {
                name: Some("Release".to_owned()),
                triggers: Triggers {
                    push: Some(TriggerFilter {
                        branches: vec!["release/**".to_owned(), "!release/**-alpha".to_owned()],
                        branches_ignore: Vec::new(),
                        tags: Vec::new(),
                        tags_ignore: Vec::new(),
                        paths: Vec::new(),
                    }),
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::new(),
            },
        }];

        assert!(matches!(
            evaluate(&RuleKind::WorkflowExistsForDefaultBranch, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn workflow_exists_for_default_branch_respects_branches_ignore() {
        let mut facts = base_facts();
        facts.workflows = vec![WorkflowFile {
            path: ".github/workflows/ci.yml".to_owned(),
            workflow: Workflow {
                name: Some("CI".to_owned()),
                triggers: Triggers {
                    push: Some(TriggerFilter {
                        branches: Vec::new(),
                        branches_ignore: vec!["main".to_owned()],
                        tags: Vec::new(),
                        tags_ignore: Vec::new(),
                        paths: Vec::new(),
                    }),
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::new(),
            },
        }];

        assert!(matches!(
            evaluate(&RuleKind::WorkflowExistsForDefaultBranch, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn branch_pattern_matches_treats_question_mark_as_postfix_quantifier() {
        assert!(branch_pattern_matches("releasex?", "release"));
        assert!(branch_pattern_matches("releasex?", "releasex"));
        assert!(!branch_pattern_matches("releasex?", "releasexx"));
    }

    #[test]
    fn branch_pattern_matches_supports_plus_followed_by_literal_paren() {
        assert!(branch_pattern_matches("ab+(", "ab("));
        assert!(branch_pattern_matches("ab+(", "abbb("));
        assert!(!branch_pattern_matches("ab+(", "a("));
    }

    #[test]
    fn branch_pattern_matches_supports_escaped_closing_bracket_in_character_class() {
        assert!(branch_pattern_matches(r"[\]]", "]"));
        assert!(!branch_pattern_matches(r"[\]]", "\\"));
    }

    #[test]
    fn branch_pattern_matches_treats_backslash_escapes_in_character_class_as_literals() {
        assert!(branch_pattern_matches(r"[\d]", "d"));
        assert!(!branch_pattern_matches(r"[\d]", "5"));
    }

    #[test]
    fn workflow_exists_for_default_branch_ignores_tags_only_push_workflows() {
        let mut facts = base_facts();
        facts.workflows = vec![WorkflowFile {
            path: ".github/workflows/release.yml".to_owned(),
            workflow: Workflow {
                name: Some("Release".to_owned()),
                triggers: Triggers {
                    push: Some(TriggerFilter {
                        branches: Vec::new(),
                        branches_ignore: Vec::new(),
                        tags: vec!["v*".to_owned()],
                        tags_ignore: Vec::new(),
                        paths: Vec::new(),
                    }),
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::new(),
            },
        }];

        assert!(matches!(
            evaluate(&RuleKind::WorkflowExistsForDefaultBranch, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn workflow_actions_pinned_to_sha_fails_for_subdir_action_with_at_in_ref() {
        let mut facts = base_facts();
        facts.workflows = vec![workflow_with_single_job(
            "build",
            vec![action_step(ActionReference::Other(
                "owner/repo/path@feature@0123456789abcdef0123456789abcdef01234567".to_owned(),
            ))],
        )];

        assert!(matches!(
            evaluate(&RuleKind::WorkflowActionsPinnedToSha, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn good_snapshot_matches_expected_default_rule_results() {
        let facts = good_fixture();
        let outputs = evaluate_rules(&default_rules(), &facts);
        let actual = outputs
            .into_iter()
            .map(|output| (output.id.to_string(), result_tag(&output.result)))
            .collect::<BTreeMap<_, _>>();
        let expected = BTreeMap::from([
            ("FL001".to_owned(), "pass"),
            ("NX001".to_owned(), "pass"),
            ("NX002".to_owned(), "skip"),
            ("RS001".to_owned(), "pass"),
            ("RS002".to_owned(), "pass"),
            ("RS003".to_owned(), "pass"),
            ("RS004".to_owned(), "pass"),
            ("RS005".to_owned(), "pass"),
            ("RS006".to_owned(), "pass"),
            ("RS007".to_owned(), "skip"),
            ("ST001".to_owned(), "pass"),
            ("ST002".to_owned(), "pass"),
            ("ST003".to_owned(), "pass"),
            ("ST004".to_owned(), "pass"),
            ("ST005".to_owned(), "pass"),
            ("WF001".to_owned(), "pass"),
            ("WF002".to_owned(), "pass"),
            ("WF003".to_owned(), "pass"),
        ]);

        assert_eq!(actual, expected);
    }

    #[test]
    fn bad_snapshot_matches_expected_default_rule_results() {
        let facts = bad_fixture();
        let outputs = evaluate_rules(&default_rules(), &facts);
        let actual = outputs
            .into_iter()
            .map(|output| (output.id.to_string(), result_tag(&output.result)))
            .collect::<BTreeMap<_, _>>();
        let expected = BTreeMap::from([
            ("FL001".to_owned(), "fail"),
            ("NX001".to_owned(), "fail"),
            ("NX002".to_owned(), "fail"),
            ("RS001".to_owned(), "fail"),
            ("RS002".to_owned(), "fail"),
            ("RS003".to_owned(), "fail"),
            ("RS004".to_owned(), "fail"),
            ("RS005".to_owned(), "fail"),
            ("RS006".to_owned(), "fail"),
            ("RS007".to_owned(), "skip"),
            ("ST001".to_owned(), "fail"),
            ("ST002".to_owned(), "fail"),
            ("ST003".to_owned(), "fail"),
            ("ST004".to_owned(), "fail"),
            ("ST005".to_owned(), "fail"),
            ("WF001".to_owned(), "fail"),
            ("WF002".to_owned(), "fail"),
            ("WF003".to_owned(), "fail"),
        ]);

        assert_eq!(actual, expected);
    }

    #[test]
    fn ruleset_enforces_admins_fails_when_admins_can_bypass() {
        let mut facts = base_facts();
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.bypass_actors.push(BypassActor {
            actor_id: Some(5),
            actor_type: BypassActorType::OrganizationAdmin,
            bypass_mode: BypassMode::Always,
        });
        facts.rulesets = vec![ruleset];

        assert!(matches!(
            evaluate(&RuleKind::RulesetEnforcesAdmins, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn ruleset_enforces_admins_passes_when_only_repository_role_bypasses() {
        let mut facts = base_facts();
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.bypass_actors.push(BypassActor {
            actor_id: Some(5),
            actor_type: BypassActorType::RepositoryRole,
            bypass_mode: BypassMode::Always,
        });
        facts.rulesets = vec![ruleset];

        assert_eq!(
            evaluate(&RuleKind::RulesetEnforcesAdmins, &facts),
            RuleResult::Pass
        );
    }

    #[test]
    fn ruleset_requires_status_check_passes_when_check_exists() {
        let mut facts = base_facts();
        facts.rulesets = vec![active_branch_ruleset(vec![RulesetRule {
            kind: RulesetRuleType::RequiredStatusChecks,
            parameters: Some(RulesetRuleParameters {
                required_status_checks: vec![RequiredStatusCheck {
                    context: "ci".to_owned(),
                    integration_id: None,
                }],
                strict_required_status_checks_policy: Some(true),
                required_approving_review_count: None,
                require_code_owner_review: None,
                require_last_push_approval: None,
                required_review_thread_resolution: None,
                dismiss_stale_reviews_on_push: None,
                do_not_enforce_on_create: None,
            }),
        }])];

        assert_eq!(
            evaluate(
                &RuleKind::RulesetRequiresStatusCheck {
                    check_name: "ci".to_owned(),
                },
                &facts,
            ),
            RuleResult::Pass
        );
    }

    #[test]
    fn ruleset_scoped_to_other_branch_does_not_satisfy_default_branch_rules() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("main");
        let mut ruleset = active_branch_ruleset(vec![RulesetRule {
            kind: RulesetRuleType::PullRequest,
            parameters: Some(RulesetRuleParameters {
                required_approving_review_count: Some(1),
                ..Default::default()
            }),
        }]);
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec!["release/*".to_owned()],
                exclude: Vec::new(),
            }),
        });
        facts.rulesets = vec![ruleset];

        assert!(matches!(
            evaluate(&RuleKind::RulesetExists, &facts),
            RuleResult::Fail { .. }
        ));
        assert!(matches!(
            evaluate(&RuleKind::RulesetRequiresReviewers { min_count: 1 }, &facts,),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn ruleset_with_default_branch_token_applies_to_default_branch() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("main");
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec!["~DEFAULT_BRANCH".to_owned()],
                exclude: Vec::new(),
            }),
        });
        facts.rulesets = vec![ruleset];

        assert_eq!(evaluate(&RuleKind::RulesetExists, &facts), RuleResult::Pass);
    }

    #[test]
    fn ruleset_with_all_token_applies_to_any_branch() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("develop");
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec!["~ALL".to_owned()],
                exclude: Vec::new(),
            }),
        });
        facts.rulesets = vec![ruleset];

        assert_eq!(evaluate(&RuleKind::RulesetExists, &facts), RuleResult::Pass);
    }

    #[test]
    fn ruleset_excluded_default_branch_does_not_apply() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("main");
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec!["~ALL".to_owned()],
                exclude: vec!["main".to_owned()],
            }),
        });
        facts.rulesets = vec![ruleset];

        assert!(matches!(
            evaluate(&RuleKind::RulesetExists, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn ruleset_with_empty_include_still_honors_exclude_patterns() {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new("main");
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: Vec::new(),
                exclude: vec!["main".to_owned()],
            }),
        });
        facts.rulesets = vec![ruleset];

        assert!(matches!(
            evaluate(&RuleKind::RulesetExists, &facts),
            RuleResult::Fail { .. }
        ));
    }

    #[test]
    fn ruleset_without_conditions_is_treated_as_applying() {
        let mut facts = base_facts();
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = None;
        facts.rulesets = vec![ruleset];

        assert_eq!(evaluate(&RuleKind::RulesetExists, &facts), RuleResult::Pass);
    }
}
