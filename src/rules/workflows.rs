use std::collections::BTreeSet;

use crate::facts::{RepoFacts, WorkflowFile};
use crate::workflow::expressions::{expression_blocks, references_secret, secret_tokens};
use crate::workflow::model::{ActionReference, Job, Step, Workflow};

use serde::{Deserialize, Serialize};

use super::glob::{branch_matches_filters, branch_pattern_matches};
use super::RuleResult;

const REQUIRED_CHECKS_JOB_NAME: &str = "all-required-checks-complete";
const REQUIRED_CHECKS_ACTION: &str = "G-Research/common-actions/check-required-lite";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkflowCheck {
    WorkflowExistsForDefaultBranch,
    WorkflowHasJob {
        job_name: String,
    },
    WorkflowActionsPinnedToSha,
    NoPullRequestTargetWithCheckout,
    NoWorkflowRunTrigger,
    NoPullRequestSecretReferences,
    WorkflowUsesAction {
        action: String,
    },
    WorkflowHasRequiredChecksComplete,
}

pub(super) fn evaluate(rule: &WorkflowCheck, facts: &RepoFacts) -> RuleResult {
    match rule {
        WorkflowCheck::WorkflowExistsForDefaultBranch => {
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
        WorkflowCheck::WorkflowHasJob { job_name } => {
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
        WorkflowCheck::WorkflowActionsPinnedToSha => {
            let offenders = facts
                .workflows
                .iter()
                .flat_map(|workflow_file| {
                    workflow_file
                        .workflow
                        .jobs
                        .values()
                        .flat_map(|job| {
                            job.uses()
                                .into_iter()
                                .chain(job.steps().iter().filter_map(|step| step.uses()))
                        })
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
        WorkflowCheck::NoPullRequestTargetWithCheckout => {
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
        WorkflowCheck::NoWorkflowRunTrigger => {
            let offenders = facts
                .workflows
                .iter()
                .filter(|workflow_file| workflow_file.workflow.triggers.workflow_run.is_some())
                .map(|workflow_file| workflow_file.path.clone())
                .collect::<Vec<_>>();

            if offenders.is_empty() {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!(
                        "workflow_run grants write permissions and secrets to runs from \
                         potentially fork-authored upstream events; do not use this trigger: {}",
                        offenders.join(", ")
                    ),
                }
            }
        }
        WorkflowCheck::WorkflowUsesAction { action } => {
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
        WorkflowCheck::WorkflowHasRequiredChecksComplete => evaluate_required_checks_complete(facts),
        WorkflowCheck::NoPullRequestSecretReferences => evaluate_pr_secrets(facts),
    }
}

fn evaluate_pr_secrets(facts: &RepoFacts) -> RuleResult {
    let mut offenders: Vec<String> = Vec::new();
    let mut unevaluated: Vec<String> = Vec::new();

    for workflow_file in &facts.workflows {
        if !workflow_has_pr_trigger(&workflow_file.workflow) {
            continue;
        }
        match collect_pr_secret_references(workflow_file) {
            PrSecretScan::Refs(refs) if refs.is_empty() => {}
            PrSecretScan::Refs(refs) => {
                let listed = refs.into_iter().collect::<Vec<_>>().join(", ");
                offenders.push(format!("{}: {}", workflow_file.path, listed));
            }
            PrSecretScan::Unevaluable => {
                unevaluated.push(workflow_file.path.clone());
            }
        }
    }

    match (offenders.is_empty(), unevaluated.is_empty()) {
        (true, true) => RuleResult::Pass,
        (true, false) => RuleResult::Skip {
            reason: format!(
                "raw YAML unavailable for PR-triggered workflows: {}",
                unevaluated.join(", ")
            ),
        },
        (false, _) => RuleResult::Fail {
            reason: format!(
                "PR-triggered workflows must not reference `secrets.*`: {}",
                summarize_examples(&offenders)
            ),
        },
    }
}

enum PrSecretScan {
    Refs(BTreeSet<String>),
    Unevaluable,
}

fn collect_pr_secret_references(workflow_file: &WorkflowFile) -> PrSecretScan {
    let Some(raw) = workflow_file.raw_yaml.as_deref() else {
        return PrSecretScan::Unevaluable;
    };
    let Ok(parsed) = serde_yml::from_str::<serde_yml::Value>(raw) else {
        return PrSecretScan::Unevaluable;
    };
    let mut refs = BTreeSet::new();
    walk_secret_references(&parsed, &mut refs);
    PrSecretScan::Refs(refs)
}

fn walk_secret_references(value: &serde_yml::Value, refs: &mut BTreeSet<String>) {
    match value {
        serde_yml::Value::String(s) => {
            for expr in expression_blocks(s) {
                if references_secret(expr) {
                    refs.extend(secret_tokens(expr));
                }
            }
        }
        serde_yml::Value::Sequence(items) => {
            for item in items {
                walk_secret_references(item, refs);
            }
        }
        serde_yml::Value::Mapping(map) => {
            for (_, child) in map {
                walk_secret_references(child, refs);
            }
        }
        _ => {}
    }
}

fn workflow_has_pr_trigger(workflow: &Workflow) -> bool {
    workflow.triggers.pull_request.is_some() || workflow.triggers.pull_request_target.is_some()
}

fn evaluate_required_checks_complete(facts: &RepoFacts) -> RuleResult {
    let candidates = facts
        .workflows
        .iter()
        .filter_map(|workflow_file| {
            workflow_file
                .workflow
                .jobs
                .get(REQUIRED_CHECKS_JOB_NAME)
                .map(|job| (workflow_file.path.as_str(), job))
        })
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return RuleResult::Fail {
            reason: format!("no workflow defines the job `{REQUIRED_CHECKS_JOB_NAME}`"),
        };
    }

    if candidates
        .iter()
        .any(|(_, job)| required_checks_job_is_valid(job))
    {
        return RuleResult::Pass;
    }

    let details = candidates
        .iter()
        .map(|(path, job)| format!("{path}: {}", describe_job_issues(job)))
        .collect::<Vec<_>>()
        .join("; ");

    RuleResult::Fail {
        reason: format!("job `{REQUIRED_CHECKS_JOB_NAME}` is misconfigured ({details})",),
    }
}

fn required_checks_job_is_valid(job: &Job) -> bool {
    condition_is_always(job.condition.as_deref())
        && job
            .steps()
            .iter()
            .any(|step| step_uses_action(step, REQUIRED_CHECKS_ACTION))
}

fn describe_job_issues(job: &Job) -> String {
    let mut issues = Vec::new();

    if !condition_is_always(job.condition.as_deref()) {
        let actual = job.condition.as_deref().unwrap_or("<missing>");
        issues.push(format!(
            "if-condition is `{actual}` but must be `${{{{ always() }}}}`"
        ));
    }

    if !job
        .steps()
        .iter()
        .any(|step| step_uses_action(step, REQUIRED_CHECKS_ACTION))
    {
        issues.push(format!("no step uses `{REQUIRED_CHECKS_ACTION}`"));
    }

    issues.join("; ")
}

fn condition_is_always(condition: Option<&str>) -> bool {
    let Some(condition) = condition else {
        return false;
    };

    let normalized: String = condition.chars().filter(|ch| !ch.is_whitespace()).collect();
    normalized == "${{always()}}"
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

fn has_branch_push_filters(push: &crate::workflow::model::TriggerFilter) -> bool {
    !push.branches.is_empty() || !push.branches_ignore.is_empty()
}

fn has_tag_push_filters(push: &crate::workflow::model::TriggerFilter) -> bool {
    !push.tags.is_empty() || !push.tags_ignore.is_empty()
}

fn workflow_uses_action(workflow: &Workflow, action: &str) -> bool {
    workflow
        .jobs
        .values()
        .flat_map(|job| job.steps().iter())
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

pub(super) fn is_commit_sha(version: &str) -> bool {
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
