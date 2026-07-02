use std::collections::{BTreeMap, BTreeSet, HashMap};

use proptest::prelude::*;

use super::glob::{branch_matches_filters, branch_pattern_matches};
use super::workflows::is_commit_sha;
use super::*;
use crate::config::{RepoConfig, Visibility};
use crate::facts::{RepoFacts, RepoSettings, WorkflowFile};
use crate::github::types::{
    BranchProtection, BypassActor, BypassActorType, BypassMode, DefaultWorkflowPermissions,
    ForkPrApprovalPolicy, LegacyEnabledFlag, LegacyRequiredPullRequestReviews,
    LegacyRequiredStatusChecks, LegacyRestrictions, MergeMethod, PullRequestParameters,
    RefNameCondition, RequiredStatusCheck, RequiredStatusChecksParameters, Ruleset,
    RulesetConditions, RulesetEnforcement, RulesetRule, RulesetRuleType, RulesetTarget,
};
use crate::types::{BranchName, Gathered, RepoRef, RuleId};
use crate::workflow::model::{
    ActionRef, ActionReference, ActionStep, Job, JobKind, ReusableJob, RunStep, StandardJob, Step,
    StepKind, TriggerFilter, Triggers, Workflow, WorkflowDispatch, WorkflowRun,
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

fn fork_pr_approval_policy_strategy() -> impl Strategy<Value = Gathered<ForkPrApprovalPolicy>> {
    prop_oneof![
        Just(Gathered::Absent),
        Just(Gathered::Unknown),
        Just(Gathered::Present(
            ForkPrApprovalPolicy::AllExternalContributors
        )),
        Just(Gathered::Present(
            ForkPrApprovalPolicy::FirstTimeContributorsNewToGithub
        )),
        Just(Gathered::Present(
            ForkPrApprovalPolicy::FirstTimeContributors
        )),
    ]
}

fn default_workflow_permissions_strategy() -> impl Strategy<Value = DefaultWorkflowPermissions> {
    prop_oneof![
        Just(DefaultWorkflowPermissions::Read),
        Just(DefaultWorkflowPermissions::Write),
    ]
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
        fork_pr_approval_policy_strategy(),
        default_workflow_permissions_strategy(),
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
                fork_pr_approval_policy,
                default_workflow_permissions,
            )| RepoSettings {
                private,
                archived,
                disabled,
                allow_auto_merge: Some(allow_auto_merge),
                delete_branch_on_merge: Some(delete_branch_on_merge),
                allow_update_branch: Some(allow_update_branch),
                allow_squash_merge: Some(allow_squash_merge),
                allow_merge_commit: Some(allow_merge_commit),
                allow_rebase_merge: Some(allow_rebase_merge),
                fork_pr_approval_policy,
                default_workflow_permissions,
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

fn required_status_check_strategy() -> impl Strategy<Value = RequiredStatusCheck> {
    (identifier(), proptest::option::of(any::<u64>())).prop_map(|(context, integration_id)| {
        RequiredStatusCheck {
            context,
            integration_id,
        }
    })
}

fn pull_request_parameters_strategy() -> impl Strategy<Value = PullRequestParameters> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        0u32..5,
        any::<bool>(),
        proptest::collection::vec(merge_method_strategy(), 0..4),
    )
        .prop_map(
            |(
                dismiss_stale_reviews_on_push,
                require_code_owner_review,
                require_last_push_approval,
                required_approving_review_count,
                required_review_thread_resolution,
                allowed_merge_methods,
            )| PullRequestParameters {
                dismiss_stale_reviews_on_push,
                require_code_owner_review,
                require_last_push_approval,
                required_approving_review_count,
                required_review_thread_resolution,
                allowed_merge_methods,
                extra: serde_json::Map::new(),
            },
        )
}

fn required_status_checks_parameters_strategy()
-> impl Strategy<Value = RequiredStatusChecksParameters> {
    (
        proptest::collection::vec(required_status_check_strategy(), 0..3),
        any::<bool>(),
        proptest::option::of(any::<bool>()),
    )
        .prop_map(
            |(
                required_status_checks,
                strict_required_status_checks_policy,
                do_not_enforce_on_create,
            )| {
                RequiredStatusChecksParameters {
                    required_status_checks,
                    strict_required_status_checks_policy,
                    do_not_enforce_on_create,
                    extra: serde_json::Map::new(),
                }
            },
        )
}

fn merge_method_strategy() -> impl Strategy<Value = MergeMethod> {
    prop_oneof![
        Just(MergeMethod::Merge),
        Just(MergeMethod::Squash),
        Just(MergeMethod::Rebase),
    ]
}

fn ruleset_rule_strategy() -> impl Strategy<Value = RulesetRule> {
    prop_oneof![
        pull_request_parameters_strategy().prop_map(RulesetRule::PullRequest),
        required_status_checks_parameters_strategy().prop_map(RulesetRule::RequiredStatusChecks),
        prop_oneof![
            Just(RulesetRuleType::Creation),
            Just(RulesetRuleType::Update),
            Just(RulesetRuleType::Deletion),
            Just(RulesetRuleType::RequiredLinearHistory),
            Just(RulesetRuleType::RequiredSignatures),
            Just(RulesetRuleType::NonFastForward),
        ]
        .prop_map(RulesetRule::parameterless),
    ]
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
        .prop_map(
            |(branches, branches_ignore, tags, tags_ignore, paths)| TriggerFilter {
                branches,
                branches_ignore,
                tags,
                tags_ignore,
                paths,
            },
        )
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
        any::<bool>(),
        proptest::collection::btree_map(
            identifier(),
            proptest::collection::vec(step_strategy(), 0..4),
            0..4,
        ),
    )
        .prop_map(
            |(
                name,
                push,
                pull_request,
                pull_request_target,
                workflow_run,
                workflow_dispatch,
                jobs,
            )| Workflow {
                name,
                triggers: Triggers {
                    push,
                    pull_request,
                    pull_request_target,
                    workflow_run: workflow_run.then_some(WorkflowRun::default()),
                    workflow_dispatch: workflow_dispatch.then_some(WorkflowDispatch::default()),
                },
                jobs: jobs
                    .into_iter()
                    .map(|(name, steps)| {
                        (
                            name,
                            Job {
                                needs: Vec::new(),
                                condition: None,
                                kind: JobKind::Standard(StandardJob {
                                    runs_on: None,
                                    steps,
                                }),
                            },
                        )
                    })
                    .collect(),
            },
        )
}

fn workflow_file_strategy() -> impl Strategy<Value = WorkflowFile> {
    (
        path_fragment(),
        workflow_strategy(),
        proptest::option::of(any::<String>()),
    )
        .prop_map(|(path, workflow, raw_yaml)| WorkflowFile {
            path,
            workflow,
            raw_yaml,
        })
}

fn repo_facts_strategy() -> impl Strategy<Value = RepoFacts> {
    (
        repo_ref_strategy(),
        repo_settings_strategy(),
        proptest::collection::vec(ruleset_strategy(), 0..4),
        prop_oneof![
            Just(Gathered::Absent),
            Just(Gathered::Unknown),
            Just(Gathered::Present(BranchProtection::default())),
        ],
        identifier(),
        proptest::collection::vec(workflow_file_strategy(), 0..4),
        proptest::collection::btree_set(path_fragment(), 0..8),
    )
        .prop_map(
            |(
                repo,
                settings,
                rulesets,
                legacy_branch_protection,
                default_branch,
                workflows,
                files_present,
            )| RepoFacts {
                repo,
                settings,
                rulesets,
                legacy_branch_protection,
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

fn required_check_source_strategy() -> impl Strategy<Value = RequiredCheckSource> {
    prop_oneof![
        Just(RequiredCheckSource::Any),
        Just(RequiredCheckSource::GitHubActions),
    ]
}

fn rule_kind_strategy() -> impl Strategy<Value = RuleKind> {
    prop_oneof![
        Just(RuleKind::Ruleset(RulesetCheck::RulesetExists)),
        (identifier(), required_check_source_strategy()).prop_map(|(check_name, source)| {
            RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck { check_name, source })
        }),
        Just(RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins)),
        Just(RuleKind::Ruleset(
            RulesetCheck::RulesetRequiresLinearHistory
        )),
        Just(RuleKind::Ruleset(RulesetCheck::RulesetPreventsForcePush)),
        Just(RuleKind::Ruleset(RulesetCheck::RulesetRestrictsDeletions)),
        Just(RuleKind::Ruleset(
            RulesetCheck::RulesetRequiresSignedCommits
        )),
        Just(RuleKind::Ruleset(RulesetCheck::RulesetRequiresPullRequest)),
        proptest::collection::vec(merge_method_strategy(), 0..4).prop_map(|allowed| {
            RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods { allowed })
        }),
        Just(RuleKind::Ruleset(
            RulesetCheck::RulesetRequiresStrictStatusChecks
        )),
        Just(RuleKind::Ruleset(
            RulesetCheck::UsesRulesetsNotLegacyProtection
        )),
        Just(RuleKind::Workflow(
            WorkflowCheck::WorkflowExistsForDefaultBranch
        )),
        identifier()
            .prop_map(|job_name| RuleKind::Workflow(WorkflowCheck::WorkflowHasJob { job_name })),
        Just(RuleKind::Workflow(
            WorkflowCheck::WorkflowActionsPinnedToSha
        )),
        Just(RuleKind::Workflow(
            WorkflowCheck::NoPullRequestTargetWithCheckout
        )),
        Just(RuleKind::Workflow(WorkflowCheck::NoWorkflowRunTrigger)),
        (identifier(), identifier()).prop_map(|(owner, repo)| RuleKind::Workflow(
            WorkflowCheck::WorkflowUsesAction {
                action: format!("{owner}/{repo}"),
            }
        )),
        Just(RuleKind::Workflow(
            WorkflowCheck::WorkflowHasRequiredChecksComplete
        )),
        path_fragment().prop_map(|path| RuleKind::File(FileCheck::FileExists { path })),
        Just(RuleKind::File(FileCheck::NixFlakeExists)),
        Just(RuleKind::File(FileCheck::NixFlakeHasCheck)),
        (repo_setting_strategy(), setting_value_strategy()).prop_map(|(setting, expected)| {
            RuleKind::Setting(SettingCheck::RepoSettingMatch { setting, expected })
        }),
        identifier().prop_map(|name| RuleKind::Setting(SettingCheck::DefaultBranchNameIs { name })),
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
        allow_auto_merge: Some(false),
        delete_branch_on_merge: Some(false),
        allow_update_branch: Some(false),
        allow_squash_merge: Some(false),
        allow_merge_commit: Some(false),
        allow_rebase_merge: Some(false),
        fork_pr_approval_policy: Gathered::Absent,
        default_workflow_permissions: DefaultWorkflowPermissions::Read,
    }
}

fn base_facts() -> RepoFacts {
    RepoFacts {
        repo: RepoRef::new("example", "repo"),
        settings: empty_repo_settings(),
        rulesets: Vec::new(),
        legacy_branch_protection: Gathered::Absent,
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
    workflow_with_single_job_kind(
        job_name,
        JobKind::Standard(StandardJob {
            runs_on: None,
            steps,
        }),
    )
}

fn workflow_with_reusable_job(job_name: &str, uses: ActionReference) -> WorkflowFile {
    workflow_with_single_job_kind(
        job_name,
        JobKind::Reusable(ReusableJob {
            uses,
            with: BTreeMap::new(),
        }),
    )
}

fn workflow_with_single_job_kind(job_name: &str, kind: JobKind) -> WorkflowFile {
    WorkflowFile {
        path: ".github/workflows/ci.yml".to_owned(),
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::from([(
                job_name.to_owned(),
                Job {
                    needs: Vec::new(),
                    condition: None,
                    kind,
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

fn repo_config_with_visibility(visibility: Visibility) -> RepoConfig {
    RepoConfig {
        owner: "example-org".to_owned(),
        name: "example-repo".to_owned(),
        visibility,
        disabled_rules: None,
    }
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
                    let (min_count, max_count, next_pattern_index) = match pattern
                        .get(pattern_index + 1)
                        .copied()
                    {
                        Some('?') => (0usize, 1usize, pattern_index + 2),
                        Some('+') => (1usize, usize::MAX, pattern_index + 2),
                        _ => {
                            return branch.get(branch_index) == Some(&ch)
                                && go(pattern, pattern_index + 1, branch, branch_index + 1, memo);
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
            legacy_branch_protection: Gathered::Absent,
            default_branch: BranchName::new(default_branch),
            workflows,
            files_present,
        };

        let result = evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts);
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
        prop_assert_eq!(evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts), RuleResult::Pass);
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

        let result = evaluate(&RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha), &facts);
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
            evaluate(&RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha), &facts),
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

        let result = evaluate(&RuleKind::File(FileCheck::FileExists { path }), &facts);
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
fn rules_for_repo_ids_are_unique() {
    let config = repo_config_with_visibility(Visibility::Private);
    let ids = rules_for_repo(&config)
        .into_iter()
        .map(|rule| rule.id.to_string())
        .collect::<Vec<_>>();
    let unique = ids.iter().cloned().collect::<BTreeSet<_>>();

    assert_eq!(unique.len(), ids.len());
}

#[test]
fn rules_for_repo_excludes_disabled_rules() {
    let mut config = repo_config_with_visibility(Visibility::Private);
    config.disabled_rules = Some(vec!["RS001".to_owned(), "WF002".to_owned()]);
    let ids: BTreeSet<String> = rules_for_repo(&config)
        .into_iter()
        .map(|rule| rule.id.to_string())
        .collect();

    assert!(!ids.contains("RS001"), "RS001 should be disabled");
    assert!(!ids.contains("WF002"), "WF002 should be disabled");
    assert!(ids.contains("RS004"), "other rules stay enabled");
    assert!(ids.contains("ST009"), "the per-repo ST009 stays enabled");
}

#[test]
fn rules_for_repo_keeps_everything_when_none_disabled() {
    let config = repo_config_with_visibility(Visibility::Private);
    let ids: BTreeSet<String> = rules_for_repo(&config)
        .into_iter()
        .map(|rule| rule.id.to_string())
        .collect();
    assert!(ids.contains("RS001"));
}

#[test]
fn unknown_disabled_rule_ids_detects_unknown_and_accepts_known() {
    let mut config = repo_config_with_visibility(Visibility::Private);
    // RS001 and the per-repo ST009 are valid; RS999/typo are not.
    config.disabled_rules = Some(vec![
        "RS001".to_owned(),
        "ST009".to_owned(),
        "RS999".to_owned(),
        "typo".to_owned(),
    ]);
    assert_eq!(
        unknown_disabled_rule_ids(&config),
        vec!["RS999".to_owned(), "typo".to_owned()]
    );
}

#[test]
fn unknown_disabled_rule_ids_empty_when_all_known_or_none() {
    let mut config = repo_config_with_visibility(Visibility::Private);
    assert!(unknown_disabled_rule_ids(&config).is_empty());
    config.disabled_rules = Some(vec!["RS001".to_owned(), "ST009".to_owned()]);
    assert!(unknown_disabled_rule_ids(&config).is_empty());
}

#[test]
fn workflow_has_job_passes_when_job_exists() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_with_single_job("build-and-test", Vec::new())];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowHasJob {
                job_name: "build-and-test".to_owned(),
            }),
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
            &RuleKind::Workflow(WorkflowCheck::WorkflowUsesAction {
                action: "actions/checkout".to_owned(),
            }),
            &facts,
        ),
        RuleResult::Pass
    );
}

#[test]
fn repo_setting_match_errors_when_boolean_setting_is_unknown() {
    let mut facts = base_facts();
    // GitHub omitted the flag (mis-scoped token): the fact is unknown, not `false`.
    facts.settings.allow_merge_commit = None;

    let result = evaluate(
        &RuleKind::Setting(SettingCheck::RepoSettingMatch {
            setting: RepoSetting::AllowMergeCommit,
            expected: SettingValue::Bool(false),
        }),
        &facts,
    );

    assert!(
        matches!(result, RuleResult::Error { .. }),
        "an unknown (None) boolean setting must Error, not pass vacuously against \
         `expected: false`; got {result:?}",
    );
}

#[test]
fn repo_setting_match_errors_when_fork_pr_approval_policy_unknown() {
    let mut facts = base_facts();
    // The fork-PR approval endpoint 404'd (token can't read it): the policy is
    // unknown, not "unset". ST007 must Error rather than report non-compliance.
    facts.settings.fork_pr_approval_policy = Gathered::Unknown;

    let result = evaluate(
        &RuleKind::Setting(SettingCheck::RepoSettingMatch {
            setting: RepoSetting::ForkPrApprovalPolicy,
            expected: SettingValue::ForkPrApprovalPolicy(Some(
                ForkPrApprovalPolicy::AllExternalContributors,
            )),
        }),
        &facts,
    );

    assert!(
        matches!(result, RuleResult::Error { .. }),
        "an unknown fork-PR approval policy must Error, not fail vacuously; got {result:?}",
    );
}

#[test]
fn repo_setting_match_reads_boolean_settings() {
    let mut facts = base_facts();
    facts.settings.allow_auto_merge = Some(true);

    assert_eq!(
        evaluate(
            &RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowAutoMerge,
                expected: SettingValue::Bool(true),
            }),
            &facts,
        ),
        RuleResult::Pass
    );
    assert!(matches!(
        evaluate(
            &RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::AllowAutoMerge,
                expected: SettingValue::Bool(false),
            }),
            &facts,
        ),
        RuleResult::Fail { .. }
    ));
}

fn evaluate_st009(visibility: Visibility, actual_private: bool) -> RuleResult {
    let mut facts = base_facts();
    facts.settings.private = actual_private;
    let config = repo_config_with_visibility(visibility);
    let rule = rules_for_repo(&config)
        .into_iter()
        .find(|rule| rule.id.to_string() == "ST009")
        .expect("ST009 must be in rules_for_repo");
    rule.evaluate(&facts).result
}

#[test]
fn st009_passes_when_public_repo_configured_public() {
    assert_eq!(evaluate_st009(Visibility::Public, false), RuleResult::Pass);
}

#[test]
fn st009_passes_when_private_repo_configured_private() {
    assert_eq!(evaluate_st009(Visibility::Private, true), RuleResult::Pass);
}

#[test]
fn st009_fails_when_public_repo_configured_private() {
    assert!(matches!(
        evaluate_st009(Visibility::Private, false),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn st009_fails_when_private_repo_configured_public() {
    assert!(matches!(
        evaluate_st009(Visibility::Public, true),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn st010_passes_when_default_workflow_permissions_is_read() {
    let mut facts = base_facts();
    facts.settings.default_workflow_permissions = DefaultWorkflowPermissions::Read;

    assert_eq!(
        evaluate(
            &RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::DefaultWorkflowPermissions,
                expected: SettingValue::DefaultWorkflowPermissions(
                    DefaultWorkflowPermissions::Read,
                ),
            }),
            &facts,
        ),
        RuleResult::Pass,
    );
}

#[test]
fn st010_fails_when_default_workflow_permissions_is_write() {
    let mut facts = base_facts();
    facts.settings.default_workflow_permissions = DefaultWorkflowPermissions::Write;

    assert!(matches!(
        evaluate(
            &RuleKind::Setting(SettingCheck::RepoSettingMatch {
                setting: RepoSetting::DefaultWorkflowPermissions,
                expected: SettingValue::DefaultWorkflowPermissions(
                    DefaultWorkflowPermissions::Read,
                ),
            }),
            &facts,
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn default_branch_name_is_passes_when_branch_matches() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");

    assert_eq!(
        evaluate(
            &RuleKind::Setting(SettingCheck::DefaultBranchNameIs {
                name: "main".to_owned(),
            }),
            &facts,
        ),
        RuleResult::Pass
    );
}

#[test]
fn default_branch_name_is_fails_when_branch_differs() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("master");

    let result = evaluate(
        &RuleKind::Setting(SettingCheck::DefaultBranchNameIs {
            name: "main".to_owned(),
        }),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("`master`") && reason.contains("`main`"),
        "unexpected reason: {reason}"
    );
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
        evaluate(&RuleKind::File(FileCheck::NixFlakeHasCheck), &facts),
        RuleResult::Pass
    );
}

#[test]
fn uses_rulesets_not_legacy_protection_passes_when_default_branch_has_no_legacy_protection() {
    let facts = base_facts();

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::UsesRulesetsNotLegacyProtection),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn uses_rulesets_not_legacy_protection_fails_when_legacy_protection_present() {
    let mut facts = base_facts();
    facts.legacy_branch_protection = Gathered::Present(BranchProtection::default());

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::UsesRulesetsNotLegacyProtection),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn uses_rulesets_not_legacy_protection_errors_when_protection_unknown() {
    let mut facts = base_facts();
    // We could not determine whether legacy protection exists (e.g. the endpoint
    // was unreadable): neither Pass nor Fail is justified.
    facts.legacy_branch_protection = Gathered::Unknown;

    let result = evaluate(
        &RuleKind::Ruleset(RulesetCheck::UsesRulesetsNotLegacyProtection),
        &facts,
    );
    assert!(
        matches!(result, RuleResult::Error { .. }),
        "unknown legacy protection must Error, not pass/fail vacuously; got {result:?}",
    );
}

#[test]
fn workflow_exists_for_default_branch_respects_single_star_slash_boundaries() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("release/train/main");
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/release.yml".to_owned(),
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowExistsForDefaultBranch),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn workflow_exists_for_default_branch_supports_double_star_and_negation_order() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("release/beta/3-alpha");
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/release.yml".to_owned(),
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowExistsForDefaultBranch),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn workflow_exists_for_default_branch_respects_branches_ignore() {
    let mut facts = base_facts();
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/ci.yml".to_owned(),
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowExistsForDefaultBranch),
            &facts
        ),
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
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowExistsForDefaultBranch),
            &facts
        ),
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
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf002_passes_for_pinned_reusable_workflow() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_with_reusable_job(
        "call-shared",
        ActionReference::Other(
            "owner/repo/.github/workflows/x.yml@0123456789abcdef0123456789abcdef01234567"
                .to_owned(),
        ),
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn wf002_fails_for_unpinned_reusable_workflow() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_with_reusable_job(
        "call-shared",
        ActionReference::Other("owner/repo/.github/workflows/x.yml@main".to_owned()),
    )];

    let result = evaluate(
        &RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("owner/repo/.github/workflows/x.yml@main"),
        "reason `{reason}` should mention the offending reusable workflow ref",
    );
    assert!(
        reason.contains(".github/workflows/ci.yml"),
        "reason `{reason}` should mention the workflow file path",
    );
}

#[test]
fn wf002_fails_when_reusable_pin_is_unpinned_alongside_pinned_steps() {
    let pinned_step = action_step(ActionReference::Repository(ActionRef::new(
        "actions",
        "checkout",
        "0123456789abcdef0123456789abcdef01234567",
    )));

    let mut facts = base_facts();
    facts.workflows = vec![
        workflow_with_single_job_kind(
            "mixed",
            JobKind::Standard(StandardJob {
                runs_on: None,
                steps: vec![pinned_step],
            }),
        ),
        workflow_with_reusable_job(
            "call-shared",
            ActionReference::Other("owner/repo/.github/workflows/x.yml@v1".to_owned()),
        ),
    ];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowActionsPinnedToSha),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn no_workflow_run_trigger_passes_when_absent() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_with_single_job(
        "build",
        vec![run_step("cargo test")],
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoWorkflowRunTrigger),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn no_workflow_run_trigger_fails_when_present() {
    let mut facts = base_facts();
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/post-ci.yml".to_owned(),
        raw_yaml: None,
        workflow: Workflow {
            name: Some("Post-CI".to_owned()),
            triggers: Triggers {
                push: None,
                pull_request: None,
                pull_request_target: None,
                workflow_run: Some(WorkflowRun::default()),
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    let RuleResult::Fail { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoWorkflowRunTrigger),
        &facts,
    ) else {
        panic!("expected Fail");
    };
    assert!(
        reason.contains(".github/workflows/post-ci.yml"),
        "reason should name the offending workflow: {reason}",
    );
}

#[test]
fn no_workflow_run_trigger_fails_irrespective_of_jobs_or_checkout() {
    let mut facts = base_facts();
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/empty-but-workflow-run.yml".to_owned(),
        raw_yaml: None,
        workflow: Workflow {
            name: None,
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
                workflow_run: Some(WorkflowRun::default()),
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoWorkflowRunTrigger),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

fn workflow_from_raw(path: &str, raw: &str) -> WorkflowFile {
    WorkflowFile {
        path: path.to_owned(),
        workflow: serde_yml::from_str(raw).expect("test YAML parses"),
        raw_yaml: Some(raw.to_owned()),
    }
}

#[test]
fn wf006_passes_when_no_pr_triggers() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/push.yml",
        "name: Push\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo ${{ secrets.NPM_TOKEN }}\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_passes_for_pr_workflow_with_no_secrets() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: cargo test\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_fails_for_pull_request_step_env_secret() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo $NPM_TOKEN\n        \
         env:\n          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n",
    )];

    let RuleResult::Fail { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        &facts,
    ) else {
        panic!("expected Fail");
    };
    assert!(reason.contains(".github/workflows/pr.yml"), "{reason}");
    assert!(reason.contains("secrets.NPM_TOKEN"), "{reason}");
}

#[test]
fn wf006_fails_for_pull_request_target_step_env_secret() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/prt.yml",
        "name: PRT\non: pull_request_target\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo $NPM_TOKEN\n        \
         env:\n          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n",
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf006_fails_for_secret_in_step_with_argument() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  publish:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - uses: some/action@v1\n        \
         with:\n          token: ${{ secrets.MY_TOKEN }}\n",
    )];

    let RuleResult::Fail { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        &facts,
    ) else {
        panic!("expected Fail");
    };
    assert!(reason.contains("secrets.MY_TOKEN"), "{reason}");
}

#[test]
fn wf006_fails_for_secret_in_run_script() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: \"echo ${{ secrets.FOO }}\"\n",
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf006_fails_for_workflow_level_env_secret() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\nenv:\n  GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}\n\
         jobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - run: gh pr list\n",
    )];

    let RuleResult::Fail { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        &facts,
    ) else {
        panic!("expected Fail");
    };
    assert!(reason.contains("secrets.GITHUB_TOKEN"), "{reason}");
}

#[test]
fn wf006_fails_for_secret_in_step_if_condition() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - if: ${{ secrets.SOMETHING != '' }}\n        run: echo hi\n",
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf006_ignores_outputs_secrets_member_access() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo ${{ steps.x.outputs.secrets }}\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_fails_for_tojson_secrets_dump() {
    // `${{ toJSON(secrets) }}` exfiltrates every secret without any `secrets.`
    // member access, so the dot-only scan used to miss it entirely.
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo '${{ toJSON(secrets) }}'\n",
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf006_fails_for_dynamic_secret_index() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - run: echo ${{ secrets[github.event.pull_request.title] }}\n",
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn wf006_fails_for_reusable_job_secrets_inherit() {
    // A reusable-workflow call with `secrets: inherit` forwards every secret to
    // the callee without any `secrets.` expression appearing in the YAML.
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  call:\n    \
         uses: ./.github/workflows/reusable.yml\n    secrets: inherit\n",
    )];

    let RuleResult::Fail { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        &facts,
    ) else {
        panic!("expected Fail");
    };
    assert!(reason.contains("inherit"), "{reason}");
}

#[test]
fn wf006_passes_for_quoted_secrets_literal() {
    // The literal string 'secrets' in an expression is data, not secret access.
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - if: ${{ contains(github.event.pull_request.title, 'secrets') }}\n        \
         run: echo hi\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_passes_for_non_reusable_secrets_inherit_mapping() {
    // A mapping literally named `secrets` with value `inherit` that is *not* a
    // reusable-workflow call (no sibling `uses:`) has no GitHub Actions meaning.
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\nenv:\n  secrets: inherit\njobs:\n  test:\n    \
         runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_passes_for_step_with_block_named_uses_and_secrets() {
    // `secrets: inherit` only has meaning on a `jobs.<job>` reusable call. A step
    // input mapping that happens to have `uses` and `secrets` keys must not be
    // flagged, which requires path context, not a context-free mapping match.
    let mut facts = base_facts();
    facts.workflows = vec![workflow_from_raw(
        ".github/workflows/pr.yml",
        "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
         steps:\n      - uses: some/action@v1\n        \
         with:\n          uses: inner\n          secrets: inherit\n",
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Pass,
    );
}

#[test]
fn wf006_skips_when_raw_yaml_unavailable_on_pr_workflow() {
    let mut facts = base_facts();
    facts.workflows = vec![WorkflowFile {
        path: ".github/workflows/pr.yml".to_owned(),
        raw_yaml: None,
        workflow: Workflow {
            name: Some("PR".to_owned()),
            triggers: Triggers {
                push: None,
                pull_request: Some(TriggerFilter::default()),
                pull_request_target: None,
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::new(),
        },
    }];

    let RuleResult::Skip { reason } = evaluate(
        &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
        &facts,
    ) else {
        panic!("expected Skip");
    };
    assert!(reason.contains(".github/workflows/pr.yml"), "{reason}");
}

#[test]
fn wf006_fail_dominates_skip() {
    let mut facts = base_facts();
    facts.workflows = vec![
        workflow_from_raw(
            ".github/workflows/leaky.yml",
            "name: PR\non: pull_request\njobs:\n  test:\n    runs-on: ubuntu-latest\n    \
             steps:\n      - run: echo ${{ secrets.FOO }}\n",
        ),
        WorkflowFile {
            path: ".github/workflows/unreadable.yml".to_owned(),
            raw_yaml: None,
            workflow: Workflow {
                name: None,
                triggers: Triggers {
                    push: None,
                    pull_request: Some(TriggerFilter::default()),
                    pull_request_target: None,
                    workflow_run: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::new(),
            },
        },
    ];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::NoPullRequestSecretReferences),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn good_snapshot_matches_expected_default_rule_results() {
    let facts = good_fixture();
    let config = repo_config_with_visibility(Visibility::Public);
    let outputs = evaluate_rules(&rules_for_repo(&config), &facts);
    let actual = outputs
        .into_iter()
        .map(|output| (output.id.to_string(), result_tag(&output.result)))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("FL001".to_owned(), "pass"),
        ("FL002".to_owned(), "pass"),
        ("FL003".to_owned(), "pass"),
        ("NX001".to_owned(), "pass"),
        ("NX002".to_owned(), "skip"),
        ("RS001".to_owned(), "pass"),
        ("RS004".to_owned(), "pass"),
        ("RS005".to_owned(), "pass"),
        ("RS006".to_owned(), "pass"),
        ("RS007".to_owned(), "pass"),
        ("RS008".to_owned(), "pass"),
        ("RS009".to_owned(), "pass"),
        ("RS010".to_owned(), "pass"),
        ("RS011".to_owned(), "pass"),
        ("RS012".to_owned(), "pass"),
        ("RS013".to_owned(), "pass"),
        ("ST001".to_owned(), "pass"),
        ("ST002".to_owned(), "pass"),
        ("ST003".to_owned(), "pass"),
        ("ST004".to_owned(), "pass"),
        ("ST005".to_owned(), "pass"),
        ("ST006".to_owned(), "pass"),
        ("ST007".to_owned(), "pass"),
        ("ST008".to_owned(), "pass"),
        ("ST009".to_owned(), "pass"),
        ("ST010".to_owned(), "pass"),
        ("WF001".to_owned(), "pass"),
        ("WF002".to_owned(), "pass"),
        ("WF003".to_owned(), "pass"),
        ("WF004".to_owned(), "pass"),
        ("WF005".to_owned(), "pass"),
        ("WF006".to_owned(), "pass"),
    ]);

    assert_eq!(actual, expected);
}

#[test]
fn bad_snapshot_matches_expected_default_rule_results() {
    let facts = bad_fixture();
    let config = repo_config_with_visibility(Visibility::Private);
    let outputs = evaluate_rules(&rules_for_repo(&config), &facts);
    let actual = outputs
        .into_iter()
        .map(|output| (output.id.to_string(), result_tag(&output.result)))
        .collect::<BTreeMap<_, _>>();
    let expected = BTreeMap::from([
        ("FL001".to_owned(), "fail"),
        ("FL002".to_owned(), "fail"),
        ("FL003".to_owned(), "fail"),
        ("NX001".to_owned(), "fail"),
        ("NX002".to_owned(), "fail"),
        ("RS001".to_owned(), "fail"),
        ("RS004".to_owned(), "fail"),
        ("RS005".to_owned(), "fail"),
        ("RS006".to_owned(), "fail"),
        ("RS007".to_owned(), "fail"),
        ("RS008".to_owned(), "fail"),
        ("RS009".to_owned(), "fail"),
        ("RS010".to_owned(), "fail"),
        ("RS011".to_owned(), "fail"),
        ("RS012".to_owned(), "fail"),
        ("RS013".to_owned(), "fail"),
        ("ST001".to_owned(), "fail"),
        ("ST002".to_owned(), "fail"),
        ("ST003".to_owned(), "fail"),
        ("ST004".to_owned(), "fail"),
        ("ST005".to_owned(), "pass"),
        ("ST006".to_owned(), "fail"),
        ("ST007".to_owned(), "fail"),
        ("ST008".to_owned(), "pass"),
        ("ST009".to_owned(), "fail"),
        ("ST010".to_owned(), "fail"),
        ("WF001".to_owned(), "fail"),
        ("WF002".to_owned(), "fail"),
        ("WF003".to_owned(), "fail"),
        ("WF004".to_owned(), "fail"),
        ("WF005".to_owned(), "fail"),
        ("WF006".to_owned(), "fail"),
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
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_enforces_admins_fails_when_repository_role_can_bypass() {
    let mut facts = base_facts();
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.bypass_actors.push(BypassActor {
        actor_id: Some(5),
        actor_type: BypassActorType::RepositoryRole,
        bypass_mode: BypassMode::Always,
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_enforces_admins_errors_on_unknown_bypass_actor() {
    // A bypass-actor class GitHub introduces that we do not model is an unknown
    // fact: RS004 must not pass it silently. It reports Error, not Pass.
    let mut facts = base_facts();
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.bypass_actors.push(BypassActor {
        actor_id: Some(9),
        actor_type: BypassActorType::Unknown("EnterpriseOwner".to_owned()),
        bypass_mode: BypassMode::Always,
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
            &facts
        ),
        RuleResult::Error { .. }
    ));
}

#[test]
fn ruleset_enforces_admins_fails_when_admin_and_unknown_both_present() {
    // A definite forbidden bypass takes precedence over an unrecognised one.
    let mut facts = base_facts();
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.bypass_actors.push(BypassActor {
        actor_id: Some(9),
        actor_type: BypassActorType::Unknown("EnterpriseOwner".to_owned()),
        bypass_mode: BypassMode::Always,
    });
    ruleset.bypass_actors.push(BypassActor {
        actor_id: Some(5),
        actor_type: BypassActorType::OrganizationAdmin,
        bypass_mode: BypassMode::Always,
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_enforces_admins_passes_with_only_permitted_bypass_actors() {
    // Team / Integration / DeployKey bypasses are deliberately permitted.
    let mut facts = base_facts();
    let mut ruleset = active_branch_ruleset(Vec::new());
    for actor_type in [
        BypassActorType::Team,
        BypassActorType::Integration,
        BypassActorType::DeployKey,
    ] {
        ruleset.bypass_actors.push(BypassActor {
            actor_id: Some(1),
            actor_type,
            bypass_mode: BypassMode::Always,
        });
    }
    facts.rulesets = vec![ruleset];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetEnforcesAdmins),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_requires_status_check_passes_when_check_exists() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![
        RulesetRule::RequiredStatusChecks(RequiredStatusChecksParameters {
            required_status_checks: vec![RequiredStatusCheck {
                context: "ci".to_owned(),
                integration_id: None,
            }],
            strict_required_status_checks_policy: true,
            do_not_enforce_on_create: None,
            extra: serde_json::Map::new(),
        }),
    ])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck {
                check_name: "ci".to_owned(),
                source: RequiredCheckSource::Any,
            }),
            &facts,
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_requires_status_check_respects_github_actions_source() {
    let check_with = |integration_id| {
        let mut facts = base_facts();
        facts.rulesets = vec![active_branch_ruleset(vec![
            RulesetRule::RequiredStatusChecks(RequiredStatusChecksParameters {
                required_status_checks: vec![RequiredStatusCheck {
                    context: "all-required-checks-complete".to_owned(),
                    integration_id,
                }],
                ..Default::default()
            }),
        ])];
        facts
    };
    let github_actions = |integration_id| {
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck {
                check_name: "all-required-checks-complete".to_owned(),
                source: RequiredCheckSource::GitHubActions,
            }),
            &check_with(integration_id),
        )
    };

    // Present but with no pinned app (the GitHub default, "any source") does not
    // satisfy a GitHub-Actions-sourced requirement.
    assert!(matches!(github_actions(None), RuleResult::Fail { .. }));
    // Present but reported by a different app likewise fails.
    assert!(matches!(github_actions(Some(7)), RuleResult::Fail { .. }));
    // Pinned to the GitHub Actions app passes.
    assert_eq!(github_actions(Some(15368)), RuleResult::Pass);

    // The `Any` source accepts the check regardless of which app reports it.
    let any = |integration_id| {
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck {
                check_name: "all-required-checks-complete".to_owned(),
                source: RequiredCheckSource::Any,
            }),
            &check_with(integration_id),
        )
    };
    assert_eq!(any(None), RuleResult::Pass);
    assert_eq!(any(Some(7)), RuleResult::Pass);
}

#[test]
fn ruleset_scoped_to_other_branch_does_not_satisfy_default_branch_rules() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(vec![RulesetRule::RequiredStatusChecks(
        RequiredStatusChecksParameters {
            required_status_checks: vec![RequiredStatusCheck {
                context: "ci".to_owned(),
                integration_id: None,
            }],
            ..Default::default()
        },
    )]);
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: vec!["release/*".to_owned()],
            exclude: Vec::new(),
        }),
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Fail { .. }
    ));
    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStatusCheck {
                check_name: "ci".to_owned(),
                source: RequiredCheckSource::Any,
            }),
            &facts,
        ),
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

    assert_eq!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Pass
    );
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

    assert_eq!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Pass
    );
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
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_with_empty_include_does_not_apply() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: Vec::new(),
            exclude: Vec::new(),
        }),
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_with_qualified_default_branch_pattern_applies() {
    // GitHub returns `refs/heads/`-qualified include patterns; a hand-created
    // ruleset targeting `refs/heads/main` must be recognised as covering the
    // default branch `main`.
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: vec!["refs/heads/main".to_owned()],
            exclude: Vec::new(),
        }),
    });
    facts.rulesets = vec![ruleset];

    assert_eq!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_with_qualified_glob_pattern_applies() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: vec!["refs/heads/ma*".to_owned()],
            exclude: Vec::new(),
        }),
    });
    facts.rulesets = vec![ruleset];

    assert_eq!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_excluding_qualified_default_branch_does_not_apply() {
    // The dangerous false-pass: `include: ~ALL` with `exclude: refs/heads/main`
    // must be judged as NOT covering the default branch. Before qualification the
    // exclude pattern was matched against the bare name and silently missed.
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: vec!["~ALL".to_owned()],
            exclude: vec!["refs/heads/main".to_owned()],
        }),
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_scoped_to_qualified_other_branch_does_not_apply() {
    let mut facts = base_facts();
    facts.default_branch = BranchName::new("main");
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = Some(RulesetConditions {
        ref_name: Some(RefNameCondition {
            include: vec!["refs/heads/release/*".to_owned()],
            exclude: Vec::new(),
        }),
    });
    facts.rulesets = vec![ruleset];

    assert!(matches!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Fail { .. }
    ));
}

fn refname_branch_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,8}(/[a-z0-9-]{1,8}){0,2}"
}

fn refname_glob_pattern_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            "[a-z0-9-]{1,6}",
            Just("*".to_owned()),
            Just("**".to_owned())
        ],
        1..4,
    )
    .prop_map(|parts| parts.join("/"))
}

proptest! {
    /// A `refs/heads/`-qualified include pattern covers the default branch iff
    /// the equivalent bare glob matches the bare branch name — i.e. the ruleset
    /// matcher delegates ref-name globbing to the (independently property-tested)
    /// branch glob, only after qualifying the ref as `refs/heads/{branch}`.
    #[test]
    fn qualified_include_pattern_matches_iff_bare_glob_matches(
        pattern in refname_glob_pattern_strategy(),
        branch in refname_branch_strategy(),
    ) {
        let mut facts = base_facts();
        facts.default_branch = BranchName::new(branch.clone());
        let mut ruleset = active_branch_ruleset(Vec::new());
        ruleset.conditions = Some(RulesetConditions {
            ref_name: Some(RefNameCondition {
                include: vec![format!("refs/heads/{pattern}")],
                exclude: Vec::new(),
            }),
        });
        facts.rulesets = vec![ruleset];

        let covered = matches!(
            evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
            RuleResult::Pass
        );
        prop_assert_eq!(covered, branch_pattern_matches(&pattern, &branch));
    }
}

fn required_checks_workflow(condition: Option<&str>, steps: Vec<Step>) -> WorkflowFile {
    WorkflowFile {
        path: ".github/workflows/ci.yml".to_owned(),
        raw_yaml: None,
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
                workflow_run: None,
                workflow_dispatch: None,
            },
            jobs: BTreeMap::from([(
                "all-required-checks-complete".to_owned(),
                Job {
                    needs: Vec::new(),
                    condition: condition.map(str::to_owned),
                    kind: JobKind::Standard(StandardJob {
                        runs_on: None,
                        steps,
                    }),
                },
            )]),
        },
    }
}

fn check_required_lite_step() -> Step {
    action_step(ActionReference::Other(
        "G-Research/common-actions/check-required-lite@2b7dc49cb14f3344fbe6019c14a31165e258c059"
            .to_owned(),
    ))
}

#[test]
fn workflow_has_required_checks_complete_passes_for_canonical_shape() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        Some("${{ always() }}"),
        vec![check_required_lite_step()],
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn workflow_has_required_checks_complete_tolerates_extra_whitespace() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        Some("${{  always()  }}"),
        vec![check_required_lite_step()],
    )];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn workflow_has_required_checks_complete_fails_when_job_absent() {
    let mut facts = base_facts();
    facts.workflows = vec![workflow_with_single_job(
        "build",
        vec![check_required_lite_step()],
    )];

    let result = evaluate(
        &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("no workflow defines the job"),
        "unexpected reason: {reason}"
    );
}

#[test]
fn workflow_has_required_checks_complete_fails_when_action_missing() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        Some("${{ always() }}"),
        vec![run_step("echo done")],
    )];

    let result = evaluate(
        &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("no step uses `G-Research/common-actions/check-required-lite`"),
        "unexpected reason: {reason}"
    );
}

#[test]
fn workflow_has_required_checks_complete_fails_when_if_condition_missing() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        None,
        vec![check_required_lite_step()],
    )];

    let result = evaluate(
        &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("if-condition is `<missing>`"),
        "unexpected reason: {reason}"
    );
}

#[test]
fn workflow_has_required_checks_complete_fails_for_wrong_if_condition() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        Some("${{ success() }}"),
        vec![check_required_lite_step()],
    )];

    let result = evaluate(
        &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
        &facts,
    );
    let RuleResult::Fail { reason } = result else {
        panic!("expected Fail, got {result:?}");
    };
    assert!(
        reason.contains("if-condition is `${{ success() }}`"),
        "unexpected reason: {reason}"
    );
}

#[test]
fn workflow_has_required_checks_complete_rejects_bare_always_without_wrapper() {
    let mut facts = base_facts();
    facts.workflows = vec![required_checks_workflow(
        Some("always()"),
        vec![check_required_lite_step()],
    )];

    assert!(matches!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn workflow_has_required_checks_complete_passes_when_one_of_many_jobs_matches() {
    let mut facts = base_facts();
    facts.workflows = vec![
        required_checks_workflow(Some("${{ success() }}"), vec![check_required_lite_step()]),
        WorkflowFile {
            path: ".github/workflows/release.yml".to_owned(),
            raw_yaml: None,
            workflow: Workflow {
                name: Some("Release".to_owned()),
                triggers: Triggers {
                    push: None,
                    pull_request: None,
                    pull_request_target: None,
                    workflow_run: None,
                    workflow_dispatch: None,
                },
                jobs: BTreeMap::from([(
                    "all-required-checks-complete".to_owned(),
                    Job {
                        needs: Vec::new(),
                        condition: Some("${{ always() }}".to_owned()),
                        kind: JobKind::Standard(StandardJob {
                            runs_on: None,
                            steps: vec![check_required_lite_step()],
                        }),
                    },
                )]),
            },
        },
    ];

    assert_eq!(
        evaluate(
            &RuleKind::Workflow(WorkflowCheck::WorkflowHasRequiredChecksComplete),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_without_conditions_is_treated_as_applying() {
    let mut facts = base_facts();
    let mut ruleset = active_branch_ruleset(Vec::new());
    ruleset.conditions = None;
    facts.rulesets = vec![ruleset];

    assert_eq!(
        evaluate(&RuleKind::Ruleset(RulesetCheck::RulesetExists), &facts),
        RuleResult::Pass
    );
}

fn pull_request_rule_with_methods(methods: Vec<MergeMethod>) -> RulesetRule {
    RulesetRule::PullRequest(PullRequestParameters {
        allowed_merge_methods: methods,
        ..PullRequestParameters::default()
    })
}

fn required_status_checks_rule(strict: bool) -> RulesetRule {
    RulesetRule::RequiredStatusChecks(RequiredStatusChecksParameters {
        required_status_checks: vec![RequiredStatusCheck {
            context: "ci".to_owned(),
            integration_id: None,
        }],
        strict_required_status_checks_policy: strict,
        ..RequiredStatusChecksParameters::default()
    })
}

#[test]
fn ruleset_restricts_deletions_passes_when_rule_present() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![RulesetRule::parameterless(
        RulesetRuleType::Deletion,
    )])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsDeletions),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_restricts_deletions_fails_when_rule_absent() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(Vec::new())];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsDeletions),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_restricts_deletions_fails_when_no_active_ruleset() {
    let facts = base_facts();

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsDeletions),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_requires_signed_commits_passes_when_rule_present() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![RulesetRule::parameterless(
        RulesetRuleType::RequiredSignatures,
    )])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresSignedCommits),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_requires_signed_commits_fails_when_rule_absent() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(Vec::new())];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresSignedCommits),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_requires_pull_request_passes_when_rule_present() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![pull_request_rule_with_methods(
        Vec::new(),
    )])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresPullRequest),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_requires_pull_request_fails_when_rule_absent() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(Vec::new())];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresPullRequest),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_restricts_merge_methods_passes_when_set_matches_exactly() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![pull_request_rule_with_methods(
        vec![MergeMethod::Squash],
    )])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            }),
            &facts,
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_restricts_merge_methods_is_set_equality_not_subset() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![pull_request_rule_with_methods(
        vec![MergeMethod::Squash, MergeMethod::Merge],
    )])];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            }),
            &facts,
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_restricts_merge_methods_fails_when_empty_allow_all_default() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![pull_request_rule_with_methods(
        Vec::new(),
    )])];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            }),
            &facts,
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_restricts_merge_methods_fails_without_pull_request_rule() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(Vec::new())];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRestrictsMergeMethods {
                allowed: vec![MergeMethod::Squash],
            }),
            &facts,
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_requires_strict_status_checks_passes_with_strict_true() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![required_status_checks_rule(
        true,
    )])];

    assert_eq!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStrictStatusChecks),
            &facts
        ),
        RuleResult::Pass
    );
}

#[test]
fn ruleset_requires_strict_status_checks_fails_with_strict_false() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![required_status_checks_rule(
        false,
    )])];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStrictStatusChecks),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_requires_strict_status_checks_fails_with_strict_none() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![required_status_checks_rule(
        false,
    )])];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStrictStatusChecks),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

#[test]
fn ruleset_requires_strict_status_checks_fails_without_required_status_checks_rule() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(Vec::new())];

    assert!(matches!(
        evaluate(
            &RuleKind::Ruleset(RulesetCheck::RulesetRequiresStrictStatusChecks),
            &facts
        ),
        RuleResult::Fail { .. }
    ));
}

fn pull_request_rule(parameters: PullRequestParameters) -> RulesetRule {
    RulesetRule::PullRequest(parameters)
}

fn rule_without_parameters(kind: RulesetRuleType) -> RulesetRule {
    RulesetRule::parameterless(kind)
}

fn fully_covering_facts() -> RepoFacts {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![
        required_status_checks_rule(true),
        pull_request_rule(PullRequestParameters {
            required_approving_review_count: 2,
            require_code_owner_review: true,
            require_last_push_approval: true,
            required_review_thread_resolution: true,
            dismiss_stale_reviews_on_push: true,
            ..PullRequestParameters::default()
        }),
        rule_without_parameters(RulesetRuleType::RequiredLinearHistory),
        rule_without_parameters(RulesetRuleType::NonFastForward),
        rule_without_parameters(RulesetRuleType::Deletion),
        rule_without_parameters(RulesetRuleType::RequiredSignatures),
        rule_without_parameters(RulesetRuleType::Creation),
    ])];
    facts
}

fn fully_covering_legacy() -> BranchProtection {
    BranchProtection {
        required_status_checks: Some(LegacyRequiredStatusChecks {
            strict: true,
            contexts: vec!["ci".to_owned()],
            checks: Vec::new(),
        }),
        required_pull_request_reviews: Some(LegacyRequiredPullRequestReviews {
            required_approving_review_count: Some(2),
            require_code_owner_reviews: true,
            dismiss_stale_reviews: true,
            require_last_push_approval: true,
            required_review_thread_resolution: true,
            bypass_pull_request_allowances: None,
        }),
        required_linear_history: Some(LegacyEnabledFlag { enabled: true }),
        allow_force_pushes: Some(LegacyEnabledFlag { enabled: false }),
        allow_deletions: Some(LegacyEnabledFlag { enabled: false }),
        required_signatures: Some(LegacyEnabledFlag { enabled: true }),
        required_conversation_resolution: Some(LegacyEnabledFlag { enabled: true }),
        enforce_admins: Some(LegacyEnabledFlag { enabled: true }),
        block_creations: Some(LegacyEnabledFlag { enabled: true }),
        lock_branch: Some(LegacyEnabledFlag { enabled: false }),
        restrictions: None,
    }
}

#[test]
fn supersedes_rejects_empty_legacy_protection() {
    let facts = base_facts();
    let legacy = BranchProtection::default();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0].contains("no fields our model recognises"),
        "reason: {}",
        reasons[0]
    );
}

#[test]
fn supersedes_accepts_fully_covered_legacy_protection() {
    let facts = fully_covering_facts();
    let legacy = fully_covering_legacy();
    legacy_protection_superseded_by_rulesets(&legacy, &facts).expect("should be superseded");
}

#[test]
fn supersedes_rejects_missing_status_check_context() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0].rules[0] =
        RulesetRule::RequiredStatusChecks(RequiredStatusChecksParameters {
            required_status_checks: vec![RequiredStatusCheck {
                context: "other".to_owned(),
                integration_id: None,
            }],
            strict_required_status_checks_policy: true,
            ..RequiredStatusChecksParameters::default()
        });
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("`ci`") && reason.contains("not enforced")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_legacy_strict_without_strict_ruleset() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0].rules[0] = required_status_checks_rule(false);
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons.iter().any(|reason| reason.contains("strict")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_pr_review_count_below_legacy() {
    let mut facts = fully_covering_facts();
    if let RulesetRule::PullRequest(parameters) = &mut facts.rulesets[0].rules[1] {
        parameters.required_approving_review_count = 1;
    }
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("2 approving reviews")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_when_no_ruleset_blocks_force_pushes() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0]
        .rules
        .retain(|rule| rule.kind() != RulesetRuleType::NonFastForward);
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("non_fast_forward")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_treats_missing_allow_force_pushes_as_restrictive() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0]
        .rules
        .retain(|rule| rule.kind() != RulesetRuleType::NonFastForward);
    let mut legacy = fully_covering_legacy();
    legacy.allow_force_pushes = None;

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons.iter().any(|reason| reason.contains("force pushes")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_ignores_allow_force_pushes_when_legacy_is_permissive() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0]
        .rules
        .retain(|rule| rule.kind() != RulesetRuleType::NonFastForward);
    let mut legacy = fully_covering_legacy();
    legacy.allow_force_pushes = Some(LegacyEnabledFlag { enabled: true });

    legacy_protection_superseded_by_rulesets(&legacy, &facts).expect("permissive legacy is fine");
}

#[test]
fn supersedes_rejects_restrictions_with_any_entries() {
    let facts = fully_covering_facts();
    let mut legacy = fully_covering_legacy();
    legacy.restrictions = Some(LegacyRestrictions {
        users: vec![serde_json::json!({"login": "alice"})],
        teams: Vec::new(),
        apps: Vec::new(),
    });

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons.iter().any(|reason| reason.contains("restrictions")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_lock_branch() {
    let facts = fully_covering_facts();
    let mut legacy = fully_covering_legacy();
    legacy.lock_branch = Some(LegacyEnabledFlag { enabled: true });

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons.iter().any(|reason| reason.contains("lock_branch")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_enforce_admins_when_ruleset_allows_bypass() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0].bypass_actors.push(BypassActor {
        actor_id: Some(1),
        actor_type: BypassActorType::OrganizationAdmin,
        bypass_mode: BypassMode::Always,
    });
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("enforce_admins")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_enforce_admins_when_ruleset_has_unknown_bypass_actor() {
    // An unrecognised bypass-actor type means we cannot verify the ruleset
    // enforces admins, so we must refuse to delete the legacy protection.
    let mut facts = fully_covering_facts();
    facts.rulesets[0].bypass_actors.push(BypassActor {
        actor_id: Some(1),
        actor_type: BypassActorType::Unknown("EnterpriseOwner".to_owned()),
        bypass_mode: BypassMode::Always,
    });
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("enforce_admins")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_when_pr_reviews_present_but_no_pr_rule() {
    let mut facts = fully_covering_facts();
    facts.rulesets[0]
        .rules
        .retain(|rule| rule.kind() != RulesetRuleType::PullRequest);
    let legacy = fully_covering_legacy();

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("no active branch ruleset contains a `pull_request`")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_rejects_when_bypass_pull_request_allowances_non_empty() {
    let facts = fully_covering_facts();
    let mut legacy = fully_covering_legacy();
    legacy
        .required_pull_request_reviews
        .as_mut()
        .unwrap()
        .bypass_pull_request_allowances =
        Some(crate::github::types::LegacyBypassPullRequestAllowances {
            users: vec![serde_json::json!({"login": "alice"})],
            teams: Vec::new(),
            apps: Vec::new(),
        });

    let reasons = legacy_protection_superseded_by_rulesets(&legacy, &facts).unwrap_err();
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("bypass_pull_request_allowances")),
        "reasons: {reasons:?}",
    );
}

#[test]
fn supersedes_status_check_context_set_normalises_legacy_checks_field() {
    let mut facts = base_facts();
    facts.rulesets = vec![active_branch_ruleset(vec![
        RulesetRule::RequiredStatusChecks(RequiredStatusChecksParameters {
            required_status_checks: vec![
                RequiredStatusCheck {
                    context: "ci".to_owned(),
                    integration_id: None,
                },
                RequiredStatusCheck {
                    context: "lint".to_owned(),
                    integration_id: None,
                },
            ],
            ..RequiredStatusChecksParameters::default()
        }),
    ])];
    let legacy = BranchProtection {
        required_status_checks: Some(LegacyRequiredStatusChecks {
            strict: false,
            contexts: Vec::new(),
            checks: vec![
                crate::github::types::LegacyStatusCheck {
                    context: "ci".to_owned(),
                    app_id: Some(17),
                },
                crate::github::types::LegacyStatusCheck {
                    context: "lint".to_owned(),
                    app_id: None,
                },
            ],
        }),
        allow_force_pushes: Some(LegacyEnabledFlag { enabled: true }),
        allow_deletions: Some(LegacyEnabledFlag { enabled: true }),
        ..BranchProtection::default()
    };

    legacy_protection_superseded_by_rulesets(&legacy, &facts).expect("contexts covered");
}

#[test]
fn supersedes_status_check_unions_contexts_across_rulesets() {
    let mut facts = base_facts();
    facts.rulesets = vec![
        Ruleset {
            id: 1,
            ..active_branch_ruleset(vec![RulesetRule::RequiredStatusChecks(
                RequiredStatusChecksParameters {
                    required_status_checks: vec![RequiredStatusCheck {
                        context: "ci".to_owned(),
                        integration_id: None,
                    }],
                    ..RequiredStatusChecksParameters::default()
                },
            )])
        },
        Ruleset {
            id: 2,
            ..active_branch_ruleset(vec![RulesetRule::RequiredStatusChecks(
                RequiredStatusChecksParameters {
                    required_status_checks: vec![RequiredStatusCheck {
                        context: "lint".to_owned(),
                        integration_id: None,
                    }],
                    ..RequiredStatusChecksParameters::default()
                },
            )])
        },
    ];
    let legacy = BranchProtection {
        required_status_checks: Some(LegacyRequiredStatusChecks {
            strict: false,
            contexts: vec!["ci".to_owned(), "lint".to_owned()],
            checks: Vec::new(),
        }),
        allow_force_pushes: Some(LegacyEnabledFlag { enabled: true }),
        allow_deletions: Some(LegacyEnabledFlag { enabled: true }),
        ..BranchProtection::default()
    };

    legacy_protection_superseded_by_rulesets(&legacy, &facts).expect("union covers");
}
