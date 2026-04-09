use crate::facts::RepoFacts;
use crate::workflow::model::{ActionReference, Step, Workflow};

use super::glob::{branch_matches_filters, branch_pattern_matches};
use super::{RuleKind, RuleResult};

pub(super) fn evaluate(kind: &RuleKind, facts: &RepoFacts) -> RuleResult {
    match kind {
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
        _ => unreachable!("non-workflow rule passed to workflows::evaluate"),
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
