//! Test-only generator of CI workflows built around an aggregator ("gate") job,
//! each paired with its ground truth. The truth is computed from the
//! generator's own choices, never by parsing the YAML it renders, so it is an
//! independent oracle for the code under test. Shared by the WF004 property
//! (does the rule accept exactly the gates that enforce?) and the RS012 fix
//! property (does `--fix` only require a check every pull request reports?).

use std::collections::BTreeSet;

use proptest::prelude::*;

pub(crate) const AGGREGATOR: &str = "all-required-checks-complete";
const GATE_ACTION: &str =
    "G-Research/common-actions/check-required-lite@2b7dc49cb14f3344fbe6019c14a31165e258c059";

/// Branch patterns for `pull_request` filters. Of these, only `main` and `**`
/// match the default branch.
const BRANCH_PATTERNS: [&str; 4] = ["main", "dev", "**", "release/*"];
const OTHER_JOBS: [&str; 3] = ["build", "test", "lint"];
const PULL_REQUEST_TYPES: [&str; 5] = ["opened", "synchronize", "reopened", "labeled", "closed"];

#[derive(Debug, Clone)]
pub(crate) enum PullRequestTrigger {
    /// The workflow only runs on `push`.
    Absent,
    /// `on: pull_request`.
    StringForm,
    /// `on: [push, pull_request]`.
    ListForm,
    /// `on: { push: ..., pull_request: <filter> }`.
    Filtered(PullRequestFilter),
    /// `pull_request_target` instead: it runs in the base branch's context, so
    /// it reports nothing on the pull request's head commit.
    TargetOnly,
}

#[derive(Debug, Clone)]
pub(crate) struct PullRequestFilter {
    branches: BranchFilter,
    paths: bool,
    paths_ignore: bool,
    types: Option<Vec<&'static str>>,
}

/// GitHub rejects `branches` and `branches-ignore` on the same event, so a
/// filter has at most one of them.
#[derive(Debug, Clone)]
pub(crate) enum BranchFilter {
    Absent,
    Branches(Vec<&'static str>),
    BranchesIgnore(Vec<&'static str>),
}

#[derive(Debug, Clone)]
pub(crate) enum DisplayName {
    Absent,
    /// `name: all-required-checks-complete`.
    Aggregator,
    /// A literal name other than the aggregator's.
    Other,
    /// A name containing an expression, so the reported check name is dynamic.
    Expression,
}

#[derive(Debug, Clone)]
pub(crate) enum Condition {
    Absent,
    AlwaysWrapped,
    AlwaysBare,
    Success,
    /// `if: true`: GitHub implicitly prefixes `success() &&`, so this is not
    /// `always()`.
    True,
}

#[derive(Debug, Clone)]
pub(crate) enum GateStep {
    /// The gate job has no step using the gate action.
    Absent,
    /// The gate action with no `needs-context` input.
    WithoutNeedsContext,
    /// The gate action with this `needs-context`; the flag records whether
    /// that value hands the action every needed job's result.
    NeedsContext(&'static str, bool),
}

/// `needs-context` values, and whether each passes the full `needs` context.
/// Expression function and context names are case-insensitive.
const NEEDS_CONTEXTS: [(&str, bool); 5] = [
    ("${{ toJSON(needs) }}", true),
    ("${{toJSON(needs)}}", true),
    ("${{ tojson(needs) }}", true),
    ("{}", false),
    ("${{ toJSON(needs.build) }}", false),
];

#[derive(Debug, Clone)]
pub(crate) struct CiShape {
    pull_request: PullRequestTrigger,
    other_jobs: Vec<&'static str>,
    gate_key: &'static str,
    gate_name: DisplayName,
    gate_matrix: bool,
    gate_needs: Vec<&'static str>,
    gate_condition: Condition,
    gate_step: GateStep,
}

impl CiShape {
    fn runs_on_every_pull_request_into_default_branch(&self) -> bool {
        match &self.pull_request {
            PullRequestTrigger::Absent | PullRequestTrigger::TargetOnly => false,
            PullRequestTrigger::StringForm | PullRequestTrigger::ListForm => true,
            PullRequestTrigger::Filtered(filter) => {
                let matches_default = |patterns: &[&str]| {
                    patterns
                        .iter()
                        .any(|pattern| matches!(*pattern, "main" | "**"))
                };
                let branch_ok = match &filter.branches {
                    BranchFilter::Absent => true,
                    BranchFilter::Branches(patterns) => matches_default(patterns),
                    BranchFilter::BranchesIgnore(patterns) => !matches_default(patterns),
                };
                // Every head commit gets a run only if the workflow triggers on
                // all of GitHub's default activity types.
                let types_ok = filter.types.as_ref().is_none_or(|types| {
                    ["opened", "synchronize", "reopened"]
                        .iter()
                        .all(|required| types.contains(required))
                });
                branch_ok && types_ok && !filter.paths && !filter.paths_ignore
            }
        }
    }

    /// The status-check name GitHub reports for the gate job, or `None` when
    /// it is not a fixed string (a matrix appends its values to the name).
    fn gate_reported_name(&self) -> Option<&str> {
        if self.gate_matrix {
            return None;
        }
        match self.gate_name {
            DisplayName::Absent => Some(self.gate_key),
            DisplayName::Aggregator => Some(AGGREGATOR),
            DisplayName::Other => Some("Required checks"),
            DisplayName::Expression => None,
        }
    }

    /// Ground truth: every pull request into the default branch gets an
    /// `all-required-checks-complete` status check on its head commit.
    pub(crate) fn reports_aggregator_on_every_pull_request(&self) -> bool {
        self.runs_on_every_pull_request_into_default_branch()
            && self.gate_reported_name() == Some(AGGREGATOR)
    }

    /// Ground truth: the aggregator check is reported on every pull request
    /// and fails whenever any other job in its workflow failed.
    pub(crate) fn has_enforcing_aggregator(&self) -> bool {
        let needs = self.gate_needs.iter().collect::<BTreeSet<_>>();
        let others = self.other_jobs.iter().collect::<BTreeSet<_>>();
        self.reports_aggregator_on_every_pull_request()
            && matches!(
                self.gate_condition,
                Condition::AlwaysWrapped | Condition::AlwaysBare
            )
            && matches!(self.gate_step, GateStep::NeedsContext(_, true))
            && needs == others
    }

    pub(crate) fn yaml(&self) -> String {
        let mut yaml = String::from("name: CI\n");
        yaml.push_str(&self.trigger_yaml());
        yaml.push_str("jobs:\n");
        for job in &self.other_jobs {
            yaml.push_str(&format!(
                "  {job}:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo {job}\n"
            ));
        }
        yaml.push_str(&format!("  {}:\n", self.gate_key));
        match self.gate_name {
            DisplayName::Absent => {}
            DisplayName::Aggregator => yaml.push_str(&format!("    name: {AGGREGATOR}\n")),
            DisplayName::Other => yaml.push_str("    name: Required checks\n"),
            DisplayName::Expression => yaml.push_str("    name: ${{ github.workflow }} gate\n"),
        }
        if self.gate_matrix {
            yaml.push_str(
                "    strategy:\n      matrix:\n        os: [ubuntu-latest, macos-latest]\n",
            );
        }
        if !self.gate_needs.is_empty() {
            yaml.push_str(&format!("    needs: [{}]\n", self.gate_needs.join(", ")));
        }
        match self.gate_condition {
            Condition::Absent => {}
            Condition::AlwaysWrapped => yaml.push_str("    if: ${{ always() }}\n"),
            Condition::AlwaysBare => yaml.push_str("    if: always()\n"),
            Condition::Success => yaml.push_str("    if: ${{ success() }}\n"),
            Condition::True => yaml.push_str("    if: true\n"),
        }
        yaml.push_str("    runs-on: ubuntu-latest\n    steps:\n");
        match self.gate_step {
            GateStep::Absent => yaml.push_str("      - run: echo done\n"),
            GateStep::WithoutNeedsContext => {
                yaml.push_str(&format!("      - uses: {GATE_ACTION}\n"));
            }
            GateStep::NeedsContext(value, _) => yaml.push_str(&format!(
                "      - uses: {GATE_ACTION}\n        with:\n          needs-context: '{value}'\n"
            )),
        }
        yaml
    }

    fn trigger_yaml(&self) -> String {
        let push = "  push:\n    branches: [main]\n";
        match &self.pull_request {
            PullRequestTrigger::Absent => format!("on:\n{push}"),
            PullRequestTrigger::StringForm => "on: pull_request\n".to_owned(),
            PullRequestTrigger::ListForm => "on: [push, pull_request]\n".to_owned(),
            PullRequestTrigger::TargetOnly => {
                "on:\n  pull_request_target:\n    branches: [main]\n".to_owned()
            }
            PullRequestTrigger::Filtered(filter) => {
                let quoted = |items: &[&str]| {
                    items
                        .iter()
                        .map(|item| format!("'{item}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let mut keys = String::new();
                match &filter.branches {
                    BranchFilter::Absent => {}
                    BranchFilter::Branches(patterns) => {
                        keys.push_str(&format!("    branches: [{}]\n", quoted(patterns)));
                    }
                    BranchFilter::BranchesIgnore(patterns) => {
                        keys.push_str(&format!("    branches-ignore: [{}]\n", quoted(patterns)));
                    }
                }
                if filter.paths {
                    keys.push_str("    paths: ['src/**']\n");
                }
                if filter.paths_ignore {
                    keys.push_str("    paths-ignore: ['docs/**']\n");
                }
                if let Some(types) = &filter.types {
                    keys.push_str(&format!("    types: [{}]\n", types.join(", ")));
                }
                format!("on:\n{push}  pull_request:\n{keys}")
            }
        }
    }
}

fn branch_patterns() -> impl Strategy<Value = Vec<&'static str>> {
    proptest::sample::subsequence(BRANCH_PATTERNS.to_vec(), 1..=BRANCH_PATTERNS.len())
}

fn pull_request_filter() -> impl Strategy<Value = PullRequestFilter> {
    // Weighted towards filters that admit every pull request, so that both
    // outcomes of the properties are well represented.
    let branches = prop_oneof![
        2 => Just(BranchFilter::Absent),
        1 => branch_patterns().prop_map(BranchFilter::Branches),
        1 => branch_patterns().prop_map(BranchFilter::BranchesIgnore),
    ];
    let types = proptest::option::weighted(
        0.3,
        proptest::sample::subsequence(PULL_REQUEST_TYPES.to_vec(), 1..=PULL_REQUEST_TYPES.len()),
    );
    (
        branches,
        prop::bool::weighted(0.15),
        prop::bool::weighted(0.15),
        types,
    )
        .prop_map(|(branches, paths, paths_ignore, types)| PullRequestFilter {
            branches,
            paths,
            paths_ignore,
            types,
        })
}

fn pull_request_trigger() -> impl Strategy<Value = PullRequestTrigger> {
    prop_oneof![
        1 => Just(PullRequestTrigger::Absent),
        2 => Just(PullRequestTrigger::StringForm),
        2 => Just(PullRequestTrigger::ListForm),
        1 => Just(PullRequestTrigger::TargetOnly),
        // Weighted up: the filtered form is where most of the cases live.
        4 => pull_request_filter().prop_map(PullRequestTrigger::Filtered),
    ]
}

fn gate_step() -> impl Strategy<Value = GateStep> {
    prop_oneof![
        1 => Just(GateStep::Absent),
        1 => Just(GateStep::WithoutNeedsContext),
        6 => proptest::sample::select(NEEDS_CONTEXTS.to_vec())
            .prop_map(|(value, valid)| GateStep::NeedsContext(value, valid)),
    ]
}

pub(crate) fn ci_shape() -> impl Strategy<Value = CiShape> {
    proptest::sample::subsequence(OTHER_JOBS.to_vec(), 0..=OTHER_JOBS.len())
        .prop_flat_map(|other_jobs| {
            let needs = prop_oneof![
                // Weighted towards the correct `needs`, which is otherwise rare.
                2 => Just(other_jobs.clone()),
                1 => proptest::sample::subsequence(other_jobs.clone(), 0..=other_jobs.len()),
            ];
            (
                Just(other_jobs),
                needs,
                pull_request_trigger(),
                prop_oneof![3 => Just(AGGREGATOR), 1 => Just("gate")],
                prop_oneof![
                    5 => Just(DisplayName::Absent),
                    1 => Just(DisplayName::Aggregator),
                    1 => Just(DisplayName::Other),
                    1 => Just(DisplayName::Expression),
                ],
                prop::bool::weighted(0.1),
                prop_oneof![
                    1 => Just(Condition::Absent),
                    4 => Just(Condition::AlwaysWrapped),
                    2 => Just(Condition::AlwaysBare),
                    1 => Just(Condition::Success),
                    1 => Just(Condition::True),
                ],
                gate_step(),
            )
        })
        .prop_map(
            |(
                other_jobs,
                gate_needs,
                pull_request,
                gate_key,
                gate_name,
                gate_matrix,
                gate_condition,
                gate_step,
            )| CiShape {
                pull_request,
                other_jobs,
                gate_key,
                gate_name,
                gate_matrix,
                gate_needs,
                gate_condition,
                gate_step,
            },
        )
}
