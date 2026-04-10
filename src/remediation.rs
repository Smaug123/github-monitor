use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::facts::RepoFacts;
use crate::github::client::{GitHubClient, NonRootRepoPath};
use crate::github::types::{
    ContentEncoding, CreateGitReference, CreatePullRequest, PullRequest, RepositoryFileContent,
    RepositoryUpdate, UpdateRepositoryFile,
};
use crate::rules::{RepoSetting, Rule, RuleKind, RuleOutput, RuleResult, evaluate_rules};
use crate::types::{BranchName, RepoRef, RuleId};
use crate::workflow::model::{ActionRef, ActionReference};

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
    OpenWorkflowPinPullRequest {
        plan: WorkflowPinPullRequestPlan,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkflowPinPullRequestPlan {
    repo: RepoRef,
    default_branch: BranchName,
    workflows: Vec<WorkflowFilePins>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowFilePins {
    path: String,
    pins: Vec<WorkflowActionPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowActionPin {
    action: RepositoryActionUse,
    occurrences: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RepositoryActionUse {
    repo: RepoRef,
    subpath: Option<String>,
    version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedWorkflowUpdate {
    path: String,
    sha: String,
    content: String,
    changes: Vec<WorkflowPinChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkflowPinChange {
    from: String,
    to: String,
}

#[derive(Debug, Default)]
struct RepoFixExecution {
    repo_settings: Option<Result<(), String>>,
    pull_requests: Vec<PullRequestExecution>,
}

#[derive(Debug)]
struct PullRequestExecution {
    rule_id: RuleId,
    result: Result<PullRequest, String>,
}

#[derive(Debug, Clone)]
struct QueuedPullRequest {
    rule_id: RuleId,
    plan: WorkflowPinPullRequestPlan,
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
            Self::OpenWorkflowPinPullRequest { plan } => {
                let file_count = plan.workflows.len();
                let pin_count = plan
                    .workflows
                    .iter()
                    .flat_map(|workflow| &workflow.pins)
                    .map(|pin| pin.occurrences)
                    .sum::<usize>();

                format!(
                    "open a pull request that pins {pin_count} workflow action {} across {file_count} workflow {} to commit SHAs",
                    pluralize(pin_count, "reference", "references"),
                    pluralize(file_count, "file", "files"),
                )
            }
        }
    }

    fn repo(&self) -> &RepoRef {
        match self {
            Self::SetRepositorySetting { repo, .. } => repo,
            Self::OpenWorkflowPinPullRequest { plan } => &plan.repo,
        }
    }
}

impl RepositoryActionUse {
    fn from_action_ref(action_ref: &ActionRef) -> Self {
        Self {
            repo: RepoRef {
                owner: action_ref.owner.clone(),
                name: action_ref.repo.clone(),
            },
            subpath: None,
            version: action_ref.version.clone(),
        }
    }

    fn resolution_key(&self) -> String {
        format!("{}@{}", self.repo, self.version)
    }

    fn rendered_with_version(&self, version: &str) -> String {
        match &self.subpath {
            Some(subpath) => format!("{}/{subpath}@{version}", self.repo),
            None => format!("{}@{version}", self.repo),
        }
    }
}

impl std::fmt::Display for RepositoryActionUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered_with_version(&self.version))
    }
}

pub fn plan_repo_fixes(rules: &[Rule], facts: &RepoFacts) -> Vec<PlannedFix> {
    let outputs = evaluate_rules(rules, facts);

    std::iter::zip(rules, &outputs)
        .filter_map(|(rule, output)| plan_rule_fix(facts, rule, output))
        .collect()
}

pub fn execute_repo_fixes(client: &mut GitHubClient, fixes: &[PlannedFix]) -> Vec<RepoFix> {
    let execution = execute_planned_effects(client, fixes);

    fixes
        .iter()
        .map(|fix| match &fix.plan {
            FixPlan::Rejected { reason } => fix.with_status(FixStatus::Rejected {
                reason: reason.clone(),
            }),
            FixPlan::Effect(FixEffect::SetRepositorySetting { .. }) => {
                match execution.repo_settings.as_ref() {
                    Some(Ok(())) => fix.with_status(FixStatus::Applied),
                    Some(Err(reason)) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason: "internal error: missing repository settings execution result"
                            .to_owned(),
                    }),
                }
            }
            FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { .. }) => {
                match execution
                    .pull_requests
                    .iter()
                    .find(|execution| execution.rule_id == fix.rule_id)
                {
                    Some(PullRequestExecution { result: Ok(_), .. }) => {
                        fix.with_status(FixStatus::Applied)
                    }
                    Some(PullRequestExecution {
                        result: Err(reason),
                        ..
                    }) => fix.with_status(FixStatus::Failed {
                        reason: reason.clone(),
                    }),
                    None => fix.with_status(FixStatus::Failed {
                        reason: "internal error: missing pull request execution result".to_owned(),
                    }),
                }
            }
        })
        .collect()
}

fn execute_planned_effects(client: &mut GitHubClient, fixes: &[PlannedFix]) -> RepoFixExecution {
    let mut repo = None::<RepoRef>;
    let mut update = RepositoryUpdate::default();
    let mut saw_repo_settings = false;
    let mut queued_pull_requests = Vec::new();
    let mut internal_error = None::<String>;

    for fix in fixes {
        let FixPlan::Effect(effect) = &fix.plan else {
            continue;
        };

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

        match effect {
            FixEffect::SetRepositorySetting { .. } => {
                saw_repo_settings = true;
                if internal_error.is_none()
                    && let Some(reason) = apply_fix_effect_to_repository_update(&mut update, effect)
                {
                    internal_error = Some(reason);
                }
            }
            FixEffect::OpenWorkflowPinPullRequest { plan } => {
                queued_pull_requests.push(QueuedPullRequest {
                    rule_id: fix.rule_id.clone(),
                    plan: plan.clone(),
                });
            }
        }
    }

    if let Some(reason) = internal_error {
        return RepoFixExecution {
            repo_settings: saw_repo_settings.then(|| Err(reason.clone())),
            pull_requests: queued_pull_requests
                .into_iter()
                .map(|queued| PullRequestExecution {
                    rule_id: queued.rule_id,
                    result: Err(reason.clone()),
                })
                .collect(),
        };
    }

    if saw_repo_settings && update.is_empty() {
        let reason = "internal error: automatic fix produced an empty repository update".to_owned();
        return RepoFixExecution {
            repo_settings: Some(Err(reason.clone())),
            pull_requests: queued_pull_requests
                .into_iter()
                .map(|queued| PullRequestExecution {
                    rule_id: queued.rule_id,
                    result: Err(reason.clone()),
                })
                .collect(),
        };
    }

    let repo_settings = if saw_repo_settings {
        let repo = repo
            .as_ref()
            .expect("repository recorded whenever a repository setting effect is present");
        Some(
            client
                .update_repository(repo, &update)
                .map(|_| ())
                .map_err(|error| error.to_string()),
        )
    } else {
        None
    };

    let pull_requests = queued_pull_requests
        .into_iter()
        .map(|queued| PullRequestExecution {
            rule_id: queued.rule_id,
            result: create_workflow_pin_pull_request(client, &queued.plan),
        })
        .collect();

    RepoFixExecution {
        repo_settings,
        pull_requests,
    }
}

fn create_workflow_pin_pull_request(
    client: &mut GitHubClient,
    plan: &WorkflowPinPullRequestPlan,
) -> Result<PullRequest, String> {
    let prepared_updates = prepare_workflow_updates(client, plan)?;
    let branch_name = workflow_pin_branch_name();
    let base_sha = client
        .resolve_commit_sha(&plan.repo, &plan.default_branch.to_string())
        .map_err(|error| {
            format!(
                "failed to resolve base branch `{}` for `{}`: {error}",
                plan.default_branch, plan.repo
            )
        })?;

    client
        .create_git_reference(
            &plan.repo,
            &CreateGitReference {
                reference: format!("refs/heads/{branch_name}"),
                sha: base_sha,
            },
        )
        .map_err(|error| {
            format!(
                "failed to create branch `{branch_name}` in `{}`: {error}",
                plan.repo
            )
        })?;

    for update in &prepared_updates {
        let path = NonRootRepoPath::new(&update.path).map_err(|error| {
            format!(
                "generated workflow path `{}` is not a valid repository path: {error}",
                update.path
            )
        })?;

        client
            .update_file_contents(
                &plan.repo,
                &path,
                &UpdateRepositoryFile {
                    message: format!("Pin GitHub Actions to commit SHAs in {}", update.path),
                    content: base64::engine::general_purpose::STANDARD
                        .encode(update.content.as_bytes()),
                    sha: update.sha.clone(),
                    branch: branch_name.clone(),
                },
            )
            .map_err(|error| {
                format!(
                    "failed to update workflow `{}` in `{}`: {error}",
                    update.path, plan.repo
                )
            })?;
    }

    client
        .create_pull_request(
            &plan.repo,
            &CreatePullRequest {
                title: workflow_pin_pull_request_title(),
                head: branch_name,
                base: plan.default_branch.to_string(),
                body: workflow_pin_pull_request_body(&prepared_updates),
            },
        )
        .map_err(|error| {
            format!(
                "failed to open pull request for workflow action pinning in `{}`: {error}",
                plan.repo
            )
        })
}

fn prepare_workflow_updates(
    client: &mut GitHubClient,
    plan: &WorkflowPinPullRequestPlan,
) -> Result<Vec<PreparedWorkflowUpdate>, String> {
    let mut resolved_shas = std::collections::HashMap::<String, String>::new();
    let mut prepared = Vec::with_capacity(plan.workflows.len());

    for workflow in &plan.workflows {
        let path = NonRootRepoPath::new(&workflow.path).map_err(|error| {
            format!(
                "workflow path `{}` is not a valid repository path: {error}",
                workflow.path
            )
        })?;
        let file = client
            .get_file_contents(&plan.repo, &path)
            .map_err(|error| {
                format!(
                    "failed to fetch workflow `{}` from `{}`: {error}",
                    workflow.path, plan.repo
                )
            })?;
        let original = decode_repository_text_file(&file)?;
        let mut content = original.clone();
        let mut changes = Vec::with_capacity(workflow.pins.len());

        for pin in &workflow.pins {
            let resolved_sha =
                if let Some(existing) = resolved_shas.get(&pin.action.resolution_key()) {
                    existing.clone()
                } else {
                    let resolved = client
                        .resolve_commit_sha(&pin.action.repo, &pin.action.version)
                        .map_err(|error| {
                            format!(
                                "failed to resolve `{}` to a commit SHA: {error}",
                                pin.action
                            )
                        })?;
                    resolved_shas.insert(pin.action.resolution_key(), resolved.clone());
                    resolved
                };

            let from = pin.action.to_string();
            let to = pin.action.rendered_with_version(&resolved_sha);
            let (updated_content, replacements) = replace_uses_line_value(&content, &from, &to)?;

            if replacements != pin.occurrences {
                return Err(format!(
                    "expected to update {} occurrence(s) of `{from}` in `{}`, updated {replacements}",
                    pin.occurrences, workflow.path
                ));
            }

            content = updated_content;
            changes.push(WorkflowPinChange { from, to });
        }

        if content == original {
            return Err(format!(
                "workflow `{}` did not change during pinning",
                workflow.path
            ));
        }

        prepared.push(PreparedWorkflowUpdate {
            path: file.path,
            sha: file.sha,
            content,
            changes,
        });
    }

    Ok(prepared)
}

fn workflow_pin_pull_request_title() -> String {
    "Pin GitHub Actions to commit SHAs".to_owned()
}

fn workflow_pin_pull_request_body(updates: &[PreparedWorkflowUpdate]) -> String {
    let mut lines = vec![
        "Generated by github-infra.".to_owned(),
        String::new(),
        "Pins GitHub Actions references to immutable commit SHAs:".to_owned(),
    ];

    for update in updates {
        for change in &update.changes {
            lines.push(format!(
                "- `{}`: `{}` -> `{}`",
                update.path, change.from, change.to
            ));
        }
    }

    lines.join("\n")
}

fn workflow_pin_branch_name() -> String {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("github-infra/pin-workflow-actions-{suffix}")
}

fn plan_rule_fix(facts: &RepoFacts, rule: &Rule, output: &RuleOutput) -> Option<PlannedFix> {
    let RuleResult::Fail { .. } = &output.result else {
        return None;
    };

    Some(PlannedFix {
        rule_id: output.id.clone(),
        rule_name: output.name.clone(),
        plan: match &rule.kind {
            RuleKind::RepoSettingMatch { setting, expected } if setting.is_safe_to_auto_fix() => {
                FixPlan::Effect(FixEffect::SetRepositorySetting {
                    repo: facts.repo.clone(),
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
            RuleKind::WorkflowActionsPinnedToSha => plan_workflow_pin_pull_request(facts),
            _ => FixPlan::Rejected {
                reason: "automatic fixes for this rule are not implemented yet".to_owned(),
            },
        },
    })
}

fn plan_workflow_pin_pull_request(facts: &RepoFacts) -> FixPlan {
    let mut workflows = Vec::new();
    let mut unsupported = Vec::new();

    for workflow_file in &facts.workflows {
        let mut pins = Vec::new();

        for job in workflow_file.workflow.jobs.values() {
            for step in &job.steps {
                let Some(uses) = step.uses() else {
                    continue;
                };

                if workflow_action_reference_is_pinned(uses) {
                    continue;
                }

                match repository_action_use_from_reference(uses) {
                    Some(action) => record_workflow_action_pin(&mut pins, action),
                    None => unsupported.push(format!(
                        "{} uses {}",
                        workflow_file.path,
                        action_reference_text(uses)
                    )),
                }
            }
        }

        if !pins.is_empty() {
            pins.sort_by(|left, right| left.action.to_string().cmp(&right.action.to_string()));
            workflows.push(WorkflowFilePins {
                path: workflow_file.path.clone(),
                pins,
            });
        }
    }

    workflows.sort_by(|left, right| left.path.cmp(&right.path));

    if !unsupported.is_empty() {
        return FixPlan::Rejected {
            reason: format!(
                "automatic fixes for workflow actions only support literal repository action references: {}",
                summarize_examples(&unsupported)
            ),
        };
    }

    if workflows.is_empty() {
        return FixPlan::Rejected {
            reason: "automatic fix could not find any unpinned workflow actions to update"
                .to_owned(),
        };
    }

    FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest {
        plan: WorkflowPinPullRequestPlan {
            repo: facts.repo.clone(),
            default_branch: facts.default_branch.clone(),
            workflows,
        },
    })
}

fn record_workflow_action_pin(pins: &mut Vec<WorkflowActionPin>, action: RepositoryActionUse) {
    if let Some(existing) = pins.iter_mut().find(|pin| pin.action == action) {
        existing.occurrences += 1;
    } else {
        pins.push(WorkflowActionPin {
            action,
            occurrences: 1,
        });
    }
}

fn repository_action_use_from_reference(uses: &ActionReference) -> Option<RepositoryActionUse> {
    match uses {
        ActionReference::Repository(action_ref) if !is_commit_sha(&action_ref.version) => {
            Some(RepositoryActionUse::from_action_ref(action_ref))
        }
        ActionReference::Repository(_) => None,
        ActionReference::Other(raw) => parse_literal_repository_action_use(raw),
    }
}

fn parse_literal_repository_action_use(raw: &str) -> Option<RepositoryActionUse> {
    if raw.starts_with("./") || raw.starts_with("docker://") || raw.matches('@').count() != 1 {
        return None;
    }

    let (path, version) = raw.rsplit_once('@')?;
    if version.is_empty() || version.contains("${{") || version.chars().any(char::is_whitespace) {
        return None;
    }

    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }

    let subpath = if segments.len() > 2 {
        Some(segments[2..].join("/"))
    } else {
        None
    };

    Some(RepositoryActionUse {
        repo: RepoRef::new(segments[0], segments[1]),
        subpath,
        version: version.to_owned(),
    })
}

fn workflow_action_reference_is_pinned(uses: &ActionReference) -> bool {
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

fn replace_uses_line_value(text: &str, from: &str, to: &str) -> Result<(String, usize), String> {
    let pattern = Regex::new(&format!(
        r#"(?m)^([ \t-]*uses:[ \t]*['"]?){}(['"]?[ \t]*(?:#.*)?)$"#,
        regex::escape(from)
    ))
    .map_err(|error| format!("invalid workflow replacement pattern for `{from}`: {error}"))?;

    let replacements = pattern.captures_iter(text).count();
    let updated = pattern
        .replace_all(text, |captures: &regex::Captures<'_>| {
            format!("{}{}{}", &captures[1], to, &captures[2])
        })
        .into_owned();

    Ok((updated, replacements))
}

fn decode_repository_text_file(file: &RepositoryFileContent) -> Result<String, String> {
    match &file.encoding {
        ContentEncoding::Base64 => {
            let compact = file
                .content
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(compact)
                .map_err(|error| {
                    format!(
                        "failed to base64-decode `{}` from GitHub: {error}",
                        file.path
                    )
                })?;

            String::from_utf8(bytes).map_err(|error| {
                format!(
                    "workflow `{}` was not valid UTF-8 after decoding: {error}",
                    file.path
                )
            })
        }
        ContentEncoding::Utf8 => Ok(file.content.clone()),
        ContentEncoding::Unknown(encoding) => Err(format!(
            "workflow `{}` used unsupported encoding `{encoding}`",
            file.path
        )),
    }
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
        FixEffect::OpenWorkflowPinPullRequest { .. } => None,
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

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{RepoSettings, WorkflowFile};
    use crate::rules::{RepoSetting, Rule, SettingValue, default_rules};
    use crate::workflow::model::{
        ActionStep, Job, RunStep, Step, StepKind, Triggers, Workflow, WorkflowDispatch,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn bad_fixture() -> RepoFacts {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/example-org/bad-repo.json"
        )))
        .unwrap()
    }

    fn base_facts() -> RepoFacts {
        RepoFacts {
            repo: RepoRef::new("example-org", "repo"),
            settings: RepoSettings {
                private: false,
                archived: false,
                disabled: false,
                allow_auto_merge: false,
                delete_branch_on_merge: false,
                allow_update_branch: false,
                allow_squash_merge: false,
                allow_merge_commit: true,
                allow_rebase_merge: false,
            },
            rulesets: Vec::new(),
            default_branch: BranchName::new("main"),
            workflows: Vec::new(),
            files_present: BTreeSet::new(),
        }
    }

    fn workflow_with_action(path: &str, uses: ActionReference) -> WorkflowFile {
        WorkflowFile {
            path: path.to_owned(),
            workflow: Workflow {
                name: Some("CI".to_owned()),
                triggers: Triggers {
                    push: None,
                    pull_request: None,
                    pull_request_target: None,
                    workflow_dispatch: Some(WorkflowDispatch::default()),
                },
                jobs: BTreeMap::from([(
                    "build".to_owned(),
                    Job {
                        runs_on: None,
                        steps: vec![
                            Step {
                                name: None,
                                id: None,
                                condition: None,
                                kind: StepKind::Action(ActionStep {
                                    uses,
                                    with: BTreeMap::new(),
                                }),
                            },
                            Step {
                                name: None,
                                id: None,
                                condition: None,
                                kind: StepKind::Run(RunStep {
                                    run: "echo ok".to_owned(),
                                }),
                            },
                        ],
                        needs: Vec::new(),
                        condition: None,
                    },
                )]),
            },
        }
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
        assert!(matches!(
            by_rule_id["WF002"].plan,
            FixPlan::Effect(FixEffect::OpenWorkflowPinPullRequest { .. })
        ));
        assert_eq!(
            by_rule_id["WF002"].planned_report().description,
            "open a pull request that pins 1 workflow action reference across 1 workflow file to commit SHAs"
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
            by_rule_id["ST006"].planned_report().status,
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

    #[test]
    fn workflow_pin_fix_rejects_non_literal_repository_action_references() {
        let mut facts = base_facts();
        facts.workflows.push(workflow_with_action(
            ".github/workflows/ci.yml",
            ActionReference::Other(
                "owner/repo/path@feature@0123456789abcdef0123456789abcdef01234567".to_owned(),
            ),
        ));

        let fixes = plan_repo_fixes(
            &[Rule::new(
                "WF002",
                "Workflow actions are pinned to commit SHAs",
                RuleKind::WorkflowActionsPinnedToSha,
            )],
            &facts,
        );

        assert_eq!(
            fixes[0].planned_report().status,
            FixStatus::Rejected {
                reason: "automatic fixes for workflow actions only support literal repository action references: .github/workflows/ci.yml uses owner/repo/path@feature@0123456789abcdef0123456789abcdef01234567".to_owned(),
            }
        );
    }

    #[test]
    fn replace_uses_line_value_preserves_quotes_and_comments() {
        let source = "      - uses: \"actions/checkout@v4\" # keep me\n";
        let (updated, replacements) = replace_uses_line_value(
            source,
            "actions/checkout@v4",
            "actions/checkout@0123456789abcdef0123456789abcdef01234567",
        )
        .unwrap();

        assert_eq!(replacements, 1);
        assert_eq!(
            updated,
            "      - uses: \"actions/checkout@0123456789abcdef0123456789abcdef01234567\" # keep me\n"
        );
    }

    #[test]
    fn execute_repo_fixes_opens_pull_request_for_workflow_pins() {
        let facts = bad_fixture();
        let rules = vec![Rule::new(
            "WF002",
            "Workflow actions are pinned to commit SHAs",
            RuleKind::WorkflowActionsPinnedToSha,
        )];
        let fixes = plan_repo_fixes(&rules, &facts);
        let resolved_sha = "0123456789abcdef0123456789abcdef01234567";
        let default_branch_sha = "fedcba9876543210fedcba9876543210fedcba98";
        let workflow_yaml = concat!(
            "name: Unsafe CI\n",
            "on:\n",
            "  pull_request_target:\n",
            "jobs:\n",
            "  build:\n",
            "    runs-on: ubuntu-latest\n",
            "    steps:\n",
            "      - uses: actions/checkout@v4\n",
            "      - run: echo unsafe\n",
        );
        let workflow_content = base64::engine::general_purpose::STANDARD.encode(workflow_yaml);
        let server = TestServer::spawn(vec![
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml",
                |_| {},
                format!(
                    r#"{{"name":"unsafe.yml","path":".github/workflows/unsafe.yml","sha":"blobsha","type":"file","encoding":"base64","content":"{workflow_content}"}}"#
                ),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/actions/checkout/commits/v4",
                |_| {},
                format!(r#"{{"sha":"{resolved_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "GET",
                "/repos/example-org/bad-repo/commits/main",
                |_| {},
                format!(r#"{{"sha":"{default_branch_sha}"}}"#),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/git/refs",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], default_branch_sha);
                    assert!(
                        json["ref"]
                            .as_str()
                            .unwrap()
                            .starts_with("refs/heads/github-infra/pin-workflow-actions-")
                    );
                },
                r#"{"ref":"refs/heads/topic","object":{"sha":"abc123","type":"commit"}}"#
                    .to_owned(),
            ),
            ExpectedRequest::json(
                "PUT",
                "/repos/example-org/bad-repo/contents/.github/workflows/unsafe.yml",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["sha"], "blobsha");
                    assert!(
                        json["branch"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/pin-workflow-actions-")
                    );
                    let content = json["content"].as_str().unwrap();
                    let decoded = String::from_utf8(
                        base64::engine::general_purpose::STANDARD
                            .decode(content)
                            .unwrap(),
                    )
                    .unwrap();
                    assert!(decoded.contains(&format!("actions/checkout@{resolved_sha}")));
                },
                "{}".to_owned(),
            ),
            ExpectedRequest::json(
                "POST",
                "/repos/example-org/bad-repo/pulls",
                move |body| {
                    let json: serde_json::Value = serde_json::from_str(body).unwrap();
                    assert_eq!(json["title"], "Pin GitHub Actions to commit SHAs");
                    assert_eq!(json["base"], "main");
                    assert!(
                        json["head"]
                            .as_str()
                            .unwrap()
                            .starts_with("github-infra/pin-workflow-actions-")
                    );
                    assert!(json["body"].as_str().unwrap().contains(&format!(
                        "actions/checkout@v4` -> `actions/checkout@{resolved_sha}"
                    )));
                },
                r#"{"number":42,"html_url":"https://example.test/pr/42"}"#.to_owned(),
            ),
        ]);
        let mut client = GitHubClient::with_base_url(
            crate::github::client::GitHubToken::new("token"),
            server.base_url(),
        );

        let executed = execute_repo_fixes(&mut client, &fixes);

        assert_eq!(executed.len(), 1);
        assert_eq!(executed[0].status, FixStatus::Applied);
    }

    struct ExpectedRequest {
        method: &'static str,
        path: &'static str,
        assert_body: Box<dyn Fn(&str) + Send>,
        response_body: String,
    }

    impl ExpectedRequest {
        fn json(
            method: &'static str,
            path: &'static str,
            assert_body: impl Fn(&str) + Send + 'static,
            response_body: String,
        ) -> Self {
            Self {
                method,
                path,
                assert_body: Box::new(assert_body),
                response_body,
            }
        }
    }

    struct TestServer {
        base_url: String,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl TestServer {
        fn spawn(expectations: Vec<ExpectedRequest>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = thread::spawn(move || {
                for expected in expectations {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    assert_eq!(request.method, expected.method);
                    assert_eq!(request.path, expected.path);
                    (expected.assert_body)(&request.body);

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        expected.response_body.len(),
                        expected.response_body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
            });

            Self {
                base_url: format!("http://{address}"),
                handle: Some(handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    struct RecordedRequest {
        method: String,
        path: String,
        body: String,
    }

    fn read_request(stream: &mut impl Read) -> RecordedRequest {
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while !buffer.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            buffer.push(byte[0]);
        }

        let header_text = String::from_utf8(buffer.clone()).unwrap();
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap().to_owned();
        let path = request_parts.next().unwrap().to_owned();
        let content_length = lines
            .filter_map(|line| line.split_once(':'))
            .find_map(|(name, value)| {
                (name.eq_ignore_ascii_case("content-length")).then(|| value.trim().parse().ok())
            })
            .flatten()
            .unwrap_or(0);

        let mut body = vec![0_u8; content_length];
        stream.read_exact(&mut body).unwrap();

        RecordedRequest {
            method,
            path,
            body: String::from_utf8(body).unwrap(),
        }
    }
}
