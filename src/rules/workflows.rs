use std::collections::BTreeSet;

use crate::facts::{RepoFacts, WorkflowFile};
use crate::workflow::expressions::{expression_blocks, references_secret, secret_tokens};
use crate::workflow::model::{ActionReference, Job, Step, WithValue, Workflow};

use serde::{Deserialize, Serialize};

use super::RuleResult;
use super::glob::{branch_matches_filters, branch_pattern_matches};

const REQUIRED_CHECKS_JOB_NAME: &str = "all-required-checks-complete";
const REQUIRED_CHECKS_ACTION: &str = "G-Research/common-actions/check-required-lite";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WorkflowCheck {
    WorkflowExistsForDefaultBranch,
    WorkflowHasJob { job_name: String },
    WorkflowActionsPinnedToSha,
    NoPullRequestTargetWithCheckout,
    NoWorkflowRunTrigger,
    NoPullRequestSecretReferences,
    WorkflowUsesAction { action: String },
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
                        .filter(|uses| !action_reference_is_pinned(uses))
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
                        "workflow actions must be pinned to 40-character commit SHAs \
                         (`docker://` images to sha256 digests): {}",
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
        WorkflowCheck::WorkflowHasRequiredChecksComplete => {
            evaluate_required_checks_complete(facts)
        }
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
                "PR-triggered workflows must not access repository secrets \
                 (`secrets.*`, `toJSON(secrets)`, `secrets: inherit`, …): {}",
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
    collect_secrets_inherit(&parsed, &mut refs);
    PrSecretScan::Refs(refs)
}

/// Flags `jobs.<job>.secrets: inherit` on a reusable-workflow call, which
/// forwards every secret to the callee with no `secrets.` expression in the
/// YAML. This needs the `jobs.<job>` path — a step `with:` input or an `env:`
/// entry that happens to be named `secrets`/`uses` must not count — so it
/// navigates the parsed document rather than matching mappings anywhere.
fn collect_secrets_inherit(parsed: &serde_yml::Value, refs: &mut BTreeSet<String>) {
    let Some(jobs) = parsed.get("jobs").and_then(serde_yml::Value::as_mapping) else {
        return;
    };
    for (_, job) in jobs {
        let Some(job) = job.as_mapping() else {
            continue;
        };
        let is_reusable_call = job.get("uses").is_some();
        let inherits_secrets =
            job.get("secrets").and_then(serde_yml::Value::as_str) == Some("inherit");
        if is_reusable_call && inherits_secrets {
            refs.insert("secrets: inherit".to_owned());
        }
    }
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

/// GitHub's default activity types for `pull_request`. A workflow restricted to
/// fewer may skip a head commit (e.g. without `synchronize`, a push to the pull
/// request runs nothing), leaving that commit without the check.
const DEFAULT_PULL_REQUEST_TYPES: [&str; 3] = ["opened", "synchronize", "reopened"];

/// Whether `workflow` runs on the head commit of every pull request into
/// `branch`. That needs a `pull_request` trigger: `pull_request_target` runs on
/// the base branch and reports nothing on the head commit. Its branch filter
/// must admit `branch`, it must keep every default activity type, and it must
/// have no path filter, because GitHub leaves a required check from a
/// path-skipped workflow pending forever.
fn runs_on_every_pull_request_into(workflow: &Workflow, branch: &str) -> bool {
    workflow
        .triggers
        .pull_request
        .as_ref()
        .is_some_and(|filter| {
            filter.paths.is_empty()
                && filter.paths_ignore.is_empty()
                && branch_matches_filters(&filter.branches, branch)
                && !filter
                    .branches_ignore
                    .iter()
                    .any(|pattern| branch_pattern_matches(pattern, branch))
                && (filter.types.is_empty()
                    || DEFAULT_PULL_REQUEST_TYPES
                        .iter()
                        .all(|required| filter.types.iter().any(|kind| kind == required)))
        })
}

/// Whether some workflow reports a status check named `check_name` on the head
/// commit of every pull request into the default branch. Requiring a check for
/// which this is false blocks every merge: GitHub waits for it forever.
pub(crate) fn every_default_branch_pull_request_reports_check(
    facts: &RepoFacts,
    check_name: &str,
) -> bool {
    let default_branch = facts.default_branch.to_string();
    facts.workflows.iter().any(|workflow_file| {
        runs_on_every_pull_request_into(&workflow_file.workflow, &default_branch)
            && workflow_file
                .workflow
                .jobs
                .iter()
                .any(|(key, job)| job.status_check_name(key) == Some(check_name))
    })
}

fn evaluate_required_checks_complete(facts: &RepoFacts) -> RuleResult {
    let default_branch = facts.default_branch.to_string();
    let default_branch = default_branch.as_str();
    // A job is a candidate if either its key or its display name is the
    // aggregator's, so a misnamed aggregator is diagnosed rather than ignored.
    let candidates = facts
        .workflows
        .iter()
        .flat_map(|workflow_file| {
            workflow_file
                .workflow
                .jobs
                .iter()
                .filter(|(key, job)| {
                    key.as_str() == REQUIRED_CHECKS_JOB_NAME
                        || job.name.as_deref() == Some(REQUIRED_CHECKS_JOB_NAME)
                })
                .map(move |(key, job)| {
                    (
                        workflow_file.path.as_str(),
                        aggregator_job_problems(&workflow_file.workflow, key, job, default_branch),
                    )
                })
        })
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return RuleResult::Fail {
            reason: format!("no workflow defines the job `{REQUIRED_CHECKS_JOB_NAME}`"),
        };
    }

    if candidates.iter().any(|(_, problems)| problems.is_empty()) {
        return RuleResult::Pass;
    }

    let details = candidates
        .iter()
        .map(|(path, problems)| format!("{path}: {}", problems.join("; ")))
        .collect::<Vec<_>>()
        .join("; ");

    RuleResult::Fail {
        reason: format!("job `{REQUIRED_CHECKS_JOB_NAME}` is misconfigured ({details})",),
    }
}

/// Everything that stops job `key` from being an aggregator that enforces: its
/// check must be reported on every pull request under the name the ruleset
/// requires, it must run even when other jobs fail, it must need every other
/// job in its workflow, and it must hand the gate action the full `needs`
/// context, from which the action decides the check's result. Empty means the
/// job enforces.
fn aggregator_job_problems(
    workflow: &Workflow,
    key: &str,
    job: &Job,
    default_branch: &str,
) -> Vec<String> {
    let mut problems = Vec::new();

    match job.status_check_name(key) {
        Some(REQUIRED_CHECKS_JOB_NAME) => {}
        Some(name) => problems.push(format!(
            "job `{key}` reports its status check as `{name}`, not `{REQUIRED_CHECKS_JOB_NAME}`"
        )),
        None => problems.push(format!(
            "job `{key}` has no fixed status-check name (it has a matrix, an expression in \
             `name:`, or calls a reusable workflow)"
        )),
    }

    if !runs_on_every_pull_request_into(workflow, default_branch) {
        problems.push(format!(
            "the workflow does not run on every pull request into `{default_branch}` (it needs a \
             `pull_request` trigger with no path filter, a branch filter admitting \
             `{default_branch}`, and every default activity type)"
        ));
    }

    if !condition_is_always(job.condition.as_deref()) {
        let actual = job.condition.as_deref().unwrap_or("<missing>");
        problems.push(format!(
            "if-condition is `{actual}` but must be `${{{{ always() }}}}`"
        ));
    }

    match job
        .steps()
        .iter()
        .find(|step| step_uses_action(step, REQUIRED_CHECKS_ACTION))
    {
        None => problems.push(format!("no step uses `{REQUIRED_CHECKS_ACTION}`")),
        Some(step) if !passes_full_needs_context(step) => problems.push(format!(
            "the `{REQUIRED_CHECKS_ACTION}` step must set `needs-context: ${{{{ toJSON(needs) }}}}`"
        )),
        Some(_) => {}
    }

    let needed = job
        .needs
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let unneeded = workflow
        .jobs
        .keys()
        .map(String::as_str)
        .filter(|other| *other != key && !needed.contains(other))
        .map(|other| format!("`{other}`"))
        .collect::<Vec<_>>();
    if !unneeded.is_empty() {
        problems.push(format!(
            "`needs` omits {}, so their failures do not fail the check",
            unneeded.join(", ")
        ));
    }

    problems
}

/// Whether the step's `needs-context` input is `${{ toJSON(needs) }}`, up to
/// whitespace and case (expression function and context names are
/// case-insensitive).
fn passes_full_needs_context(step: &Step) -> bool {
    let Some(WithValue::String(value)) = step.input("needs-context") else {
        return false;
    };
    let normalized: String = value
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    normalized == "${{tojson(needs)}}"
}

fn condition_is_always(condition: Option<&str>) -> bool {
    let Some(condition) = condition else {
        return false;
    };

    let normalized: String = condition.chars().filter(|ch| !ch.is_whitespace()).collect();
    // GitHub evaluates `if:` as an expression, so the `${{ }}` wrapper is
    // optional: `if: always()` and `if: ${{ always() }}` are equivalent.
    normalized == "always()" || normalized == "${{always()}}"
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

/// Whether `uses` names an immutable revision: a full commit SHA for an action
/// or reusable workflow in another repository, or a sha256 content digest for a
/// `docker://` image (a tag can be re-pointed at any time). A local `./` action
/// is fixed by this repository's own commit.
pub(crate) fn action_reference_is_pinned(uses: &ActionReference) -> bool {
    match uses {
        ActionReference::Repository(action_ref) => is_commit_sha(&action_ref.version),
        ActionReference::Other(raw) => {
            if raw.starts_with("./") {
                true
            } else if let Some(image) = raw.strip_prefix("docker://") {
                docker_image_is_pinned_by_digest(image)
            } else if let Some((_, version)) = raw.split_once('@') {
                is_commit_sha(version)
            } else {
                false
            }
        }
    }
}

/// `image[:tag]@sha256:<64 lowercase hex>`; OCI digests are lowercase.
fn docker_image_is_pinned_by_digest(image: &str) -> bool {
    image.split_once('@').is_some_and(|(name, digest)| {
        !name.is_empty()
            && digest.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            })
    })
}

fn action_reference_text(uses: &ActionReference) -> String {
    match uses {
        ActionReference::Repository(action_ref) => action_ref.to_string(),
        ActionReference::Other(raw) => raw.clone(),
    }
}

pub(crate) fn is_commit_sha(version: &str) -> bool {
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
