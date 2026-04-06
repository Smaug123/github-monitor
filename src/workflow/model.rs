use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::types::{Owner, RepoName};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Workflow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, rename = "on")]
    pub triggers: Triggers,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub jobs: BTreeMap<String, Job>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Triggers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<TriggerFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<TriggerFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request_target: Option<TriggerFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_dispatch: Option<WorkflowDispatch>,
}

impl<'de> Deserialize<'de> for Triggers {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = serde_yml::Value::deserialize(deserializer)?;
        Self::from_yaml_value(raw).map_err(serde::de::Error::custom)
    }
}

impl Triggers {
    fn from_yaml_value(raw: serde_yml::Value) -> Result<Self, String> {
        match raw {
            serde_yml::Value::String(name) => Ok(Self::from_event_names([name])),
            serde_yml::Value::Sequence(items) => {
                let mut names = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        serde_yml::Value::String(name) => names.push(name),
                        _ => return Err("workflow `on` list items must be strings".to_owned()),
                    }
                }

                Ok(Self::from_event_names(names))
            }
            serde_yml::Value::Mapping(mapping) => Self::from_mapping(mapping),
            other => Err(format!(
                "workflow `on` must be a string, list, or map, got {other:?}"
            )),
        }
    }

    fn from_event_names<I>(names: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut triggers = Self::default();

        for name in names {
            match name.as_str() {
                "push" => triggers.push = Some(TriggerFilter::default()),
                "pull_request" => triggers.pull_request = Some(TriggerFilter::default()),
                "pull_request_target" => {
                    triggers.pull_request_target = Some(TriggerFilter::default())
                }
                "workflow_dispatch" => {
                    triggers.workflow_dispatch = Some(WorkflowDispatch::default())
                }
                _ => {}
            }
        }

        triggers
    }

    fn from_mapping(mapping: serde_yml::Mapping) -> Result<Self, String> {
        Ok(Self {
            push: parse_optional_default(mapping_value(&mapping, "push"), "push")?,
            pull_request: parse_optional_default(
                mapping_value(&mapping, "pull_request"),
                "pull_request",
            )?,
            pull_request_target: parse_optional_default(
                mapping_value(&mapping, "pull_request_target"),
                "pull_request_target",
            )?,
            workflow_dispatch: parse_optional_default(
                mapping_value(&mapping, "workflow_dispatch"),
                "workflow_dispatch",
            )?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TriggerFilter {
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub branches: Vec<String>,
    #[serde(
        default,
        rename = "branches-ignore",
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub branches_ignore: Vec<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<String>,
    #[serde(
        default,
        rename = "tags-ignore",
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags_ignore: Vec<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowDispatch {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Job {
    #[serde(default, rename = "runs-on", skip_serializing_if = "Option::is_none")]
    pub runs_on: Option<RunsOn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub needs: Vec<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RunsOn {
    Label(String),
    Labels(Vec<String>),
    Group(RunsOnGroup),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RunsOnGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Step {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(flatten)]
    pub kind: StepKind,
}

impl Step {
    pub fn uses(&self) -> Option<&ActionReference> {
        match &self.kind {
            StepKind::Action(action) => Some(&action.uses),
            StepKind::Run(_) => None,
        }
    }

    pub fn run(&self) -> Option<&str> {
        match &self.kind {
            StepKind::Action(_) => None,
            StepKind::Run(run) => Some(&run.run),
        }
    }

    pub fn with(&self) -> Option<&BTreeMap<String, WithValue>> {
        match &self.kind {
            StepKind::Action(action) => Some(&action.with),
            StepKind::Run(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StepKind {
    Action(ActionStep),
    Run(RunStep),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionStep {
    pub uses: ActionReference,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, WithValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStep {
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ActionReference {
    Repository(ActionRef),
    Other(String),
}

impl ActionReference {
    pub fn as_action_ref(&self) -> Option<&ActionRef> {
        match self {
            Self::Repository(action_ref) => Some(action_ref),
            Self::Other(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ActionRef {
    pub owner: Owner,
    pub repo: RepoName,
    pub version: String,
}

impl ActionRef {
    pub fn new(
        owner: impl Into<String>,
        repo: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            owner: Owner::new(owner),
            repo: RepoName::new(repo),
            version: version.into(),
        }
    }
}

impl fmt::Display for ActionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}@{}", self.owner, self.repo, self.version)
    }
}

impl FromStr for ActionRef {
    type Err = ParseActionRefError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (path, version) = s
            .split_once('@')
            .ok_or_else(|| ParseActionRefError::new(s))?;

        let mut path_parts = path.split('/');
        let owner = path_parts.next().unwrap_or_default();
        let repo = path_parts.next().unwrap_or_default();

        if owner.is_empty() || repo.is_empty() || version.is_empty() || path_parts.next().is_some()
        {
            return Err(ParseActionRefError::new(s));
        }

        Ok(Self::new(owner, repo, version))
    }
}

impl TryFrom<String> for ActionRef {
    type Error = ParseActionRefError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ActionRef> for String {
    fn from(value: ActionRef) -> Self {
        value.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseActionRefError {
    input: String,
}

impl ParseActionRefError {
    fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
        }
    }
}

impl fmt::Display for ParseActionRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "action reference must have the form owner/repo@version: {}",
            self.input
        )
    }
}

impl std::error::Error for ParseActionRefError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WithValue {
    String(String),
    Bool(bool),
    Integer(i64),
}

fn parse_optional_default<T>(
    raw: Option<&serde_yml::Value>,
    field_name: &str,
) -> Result<Option<T>, String>
where
    T: DeserializeOwned + Default,
{
    match raw {
        None => Ok(None),
        Some(serde_yml::Value::Null) => Ok(Some(T::default())),
        Some(value) => serde_yml::from_value(value.clone())
            .map(Some)
            .map_err(|error| format!("invalid workflow trigger `{field_name}`: {error}")),
    }
}

fn mapping_value<'a>(
    mapping: &'a serde_yml::Mapping,
    field_name: &str,
) -> Option<&'a serde_yml::Value> {
    mapping.iter().find_map(|(key, value)| match key {
        serde_yml::Value::String(candidate) if candidate == field_name => Some(value),
        _ => None,
    })
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        One(String),
        Many(Vec<String>),
    }

    let raw = Option::<StringOrVec>::deserialize(deserializer)?;
    Ok(match raw {
        None => Vec::new(),
        Some(StringOrVec::One(item)) => vec![item],
        Some(StringOrVec::Many(items)) => items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::fs;
    use std::path::Path;

    fn identifier() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_-]{0,12}"
    }

    fn path_fragment() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_./-]{0,20}"
    }

    fn version() -> impl Strategy<Value = String> {
        "[A-Za-z0-9._/@-]{1,20}"
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

    fn triggers_strategy() -> impl Strategy<Value = Triggers> {
        (
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            any::<bool>(),
        )
            .prop_map(
                |(push, pull_request, pull_request_target, workflow_dispatch)| Triggers {
                    push,
                    pull_request,
                    pull_request_target,
                    workflow_dispatch: workflow_dispatch.then_some(WorkflowDispatch::default()),
                },
            )
    }

    fn runs_on_strategy() -> impl Strategy<Value = RunsOn> {
        prop_oneof![
            identifier().prop_map(RunsOn::Label),
            proptest::collection::vec(identifier(), 1..4).prop_map(RunsOn::Labels),
            (
                proptest::option::of(identifier()),
                proptest::collection::vec(identifier(), 0..3),
            )
                .prop_map(|(group, labels)| RunsOn::Group(RunsOnGroup { group, labels })),
        ]
    }

    fn action_ref_strategy() -> impl Strategy<Value = ActionRef> {
        (identifier(), identifier(), version())
            .prop_map(|(owner, repo, version)| ActionRef::new(owner, repo, version))
    }

    fn action_reference_strategy() -> impl Strategy<Value = ActionReference> {
        prop_oneof![
            action_ref_strategy().prop_map(ActionReference::Repository),
            "[./A-Za-z0-9_:-]{1,30}".prop_map(ActionReference::Other),
        ]
    }

    fn with_value_strategy() -> impl Strategy<Value = WithValue> {
        prop_oneof![
            path_fragment().prop_map(WithValue::String),
            any::<bool>().prop_map(WithValue::Bool),
            any::<i32>().prop_map(|value| WithValue::Integer(i64::from(value))),
        ]
    }

    fn step_strategy() -> impl Strategy<Value = Step> {
        let action_step = (
            proptest::option::of(path_fragment()),
            proptest::option::of(identifier()),
            proptest::option::of(path_fragment()),
            action_reference_strategy(),
            proptest::collection::btree_map(identifier(), with_value_strategy(), 0..3),
        )
            .prop_map(|(name, id, condition, uses, with)| Step {
                name,
                id,
                condition,
                kind: StepKind::Action(ActionStep { uses, with }),
            });

        let run_step = (
            proptest::option::of(path_fragment()),
            proptest::option::of(identifier()),
            proptest::option::of(path_fragment()),
            ".{1,40}",
        )
            .prop_map(|(name, id, condition, run)| Step {
                name,
                id,
                condition,
                kind: StepKind::Run(RunStep { run }),
            });

        prop_oneof![action_step, run_step]
    }

    fn job_strategy() -> impl Strategy<Value = Job> {
        (
            proptest::option::of(runs_on_strategy()),
            proptest::collection::vec(step_strategy(), 0..4),
            proptest::collection::vec(identifier(), 0..3),
            proptest::option::of(path_fragment()),
        )
            .prop_map(|(runs_on, steps, needs, condition)| Job {
                runs_on,
                steps,
                needs,
                condition,
            })
    }

    fn workflow_strategy() -> impl Strategy<Value = Workflow> {
        (
            proptest::option::of(path_fragment()),
            triggers_strategy(),
            proptest::collection::btree_map(identifier(), job_strategy(), 0..4),
        )
            .prop_map(|(name, triggers, jobs)| Workflow {
                name,
                triggers,
                jobs,
            })
    }

    proptest! {
        #[test]
        fn workflow_yaml_roundtrip(workflow in workflow_strategy()) {
            let first_yaml = serde_yml::to_string(&workflow).unwrap();
            let first: Workflow = serde_yml::from_str(&first_yaml).unwrap();
            prop_assert_eq!(&first, &workflow);

            let second_yaml = serde_yml::to_string(&first).unwrap();
            let second: Workflow = serde_yml::from_str(&second_yaml).unwrap();

            prop_assert_eq!(&second, &first);
        }

        #[test]
        fn action_ref_parse_display_roundtrip(action_ref in action_ref_strategy()) {
            let reparsed: ActionRef = action_ref.to_string().parse().unwrap();
            prop_assert_eq!(reparsed, action_ref);
        }
    }

    #[test]
    fn parses_action_ref_with_at_in_version() {
        let action_ref: ActionRef = "owner/repo@feature@123".parse().unwrap();

        assert_eq!(action_ref, ActionRef::new("owner", "repo", "feature@123"));
        assert_eq!(action_ref.to_string(), "owner/repo@feature@123");
    }

    #[test]
    fn parses_string_triggers() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
name: String trigger
on: push
jobs: {}
"#,
        )
        .unwrap();

        assert_eq!(workflow.name.as_deref(), Some("String trigger"));
        assert_eq!(workflow.triggers.push, Some(TriggerFilter::default()));
        assert_eq!(workflow.triggers.pull_request, None);
    }

    #[test]
    fn parses_list_triggers() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  - push
  - pull_request
jobs: {}
"#,
        )
        .unwrap();

        assert_eq!(workflow.triggers.push, Some(TriggerFilter::default()));
        assert_eq!(
            workflow.triggers.pull_request,
            Some(TriggerFilter::default())
        );
    }

    #[test]
    fn parses_map_triggers_and_polymorphic_runs_on() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  push:
    branches: [main]
    paths: app
  workflow_dispatch: {}
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
  self-hosted-build:
    runs-on:
      - self-hosted
      - linux
    steps:
      - run: cargo fmt --check
  deploy:
    runs-on:
      group: release-runners
      labels: linux
    needs: build
    if: github.ref == 'refs/heads/main'
    steps:
      - uses: actions/checkout@v6
"#,
        )
        .unwrap();

        assert_eq!(
            workflow.triggers.push,
            Some(TriggerFilter {
                branches: vec!["main".to_owned()],
                branches_ignore: Vec::new(),
                tags: Vec::new(),
                tags_ignore: Vec::new(),
                paths: vec!["app".to_owned()],
            })
        );
        assert_eq!(
            workflow.triggers.workflow_dispatch,
            Some(WorkflowDispatch::default())
        );

        let build = workflow.jobs.get("build").unwrap();
        assert_eq!(
            build.runs_on,
            Some(RunsOn::Label("ubuntu-latest".to_owned()))
        );
        assert_eq!(build.steps.first().unwrap().run(), Some("cargo test"));

        let self_hosted_build = workflow.jobs.get("self-hosted-build").unwrap();
        assert_eq!(
            self_hosted_build.runs_on,
            Some(RunsOn::Labels(vec![
                "self-hosted".to_owned(),
                "linux".to_owned(),
            ]))
        );

        let deploy = workflow.jobs.get("deploy").unwrap();
        assert_eq!(
            deploy.runs_on,
            Some(RunsOn::Group(RunsOnGroup {
                group: Some("release-runners".to_owned()),
                labels: vec!["linux".to_owned()],
            }))
        );
        assert_eq!(deploy.needs, vec!["build".to_owned()]);
        assert_eq!(
            deploy.condition.as_deref(),
            Some("github.ref == 'refs/heads/main'")
        );
    }

    #[test]
    fn parses_branches_ignore_filters() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  push:
    branches-ignore:
      - main
      - release/**
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#,
        )
        .unwrap();

        assert_eq!(
            workflow.triggers.push,
            Some(TriggerFilter {
                branches: Vec::new(),
                branches_ignore: vec!["main".to_owned(), "release/**".to_owned()],
                tags: Vec::new(),
                tags_ignore: Vec::new(),
                paths: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_tags_only_push_filters() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  push:
    tags:
      - v*
    tags-ignore:
      - v0.*
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test
"#,
        )
        .unwrap();

        assert_eq!(
            workflow.triggers.push,
            Some(TriggerFilter {
                branches: Vec::new(),
                branches_ignore: Vec::new(),
                tags: vec!["v*".to_owned()],
                tags_ignore: vec!["v0.*".to_owned()],
                paths: Vec::new(),
            })
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
name: Unknown fields
unknown-top-level: true
on:
  push:
    branches: [main]
    unknown-trigger-field: true
  schedule:
    - cron: "0 0 * * *"
jobs:
  build:
    runs-on:
      group: linux
      unknown-runs-on-field: true
    unknown-job-field: ignored
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 1
        unknown-step-field: ignored
"#,
        )
        .unwrap();

        assert_eq!(
            workflow.triggers.push,
            Some(TriggerFilter {
                branches: vec!["main".to_owned()],
                branches_ignore: Vec::new(),
                tags: Vec::new(),
                tags_ignore: Vec::new(),
                paths: Vec::new(),
            })
        );
        let build = workflow.jobs.get("build").unwrap();
        assert_eq!(
            build.runs_on,
            Some(RunsOn::Group(RunsOnGroup {
                group: Some("linux".to_owned()),
                labels: Vec::new(),
            }))
        );

        let step = build.steps.first().unwrap();
        let uses = step.uses().unwrap().as_action_ref().unwrap();
        assert_eq!(uses, &ActionRef::new("actions", "checkout", "v4"));
        assert_eq!(
            step.with().unwrap().get("fetch-depth"),
            Some(&WithValue::Integer(1))
        );
    }

    #[test]
    fn parses_repo_workflows_without_error() {
        let workflows_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows");
        if !workflows_dir.is_dir() {
            return;
        }

        let entries = fs::read_dir(&workflows_dir).unwrap();

        for entry in entries {
            let path = entry.unwrap().path();
            let is_workflow_file = matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yml" | "yaml")
            );

            if !is_workflow_file {
                continue;
            }

            let yaml = fs::read_to_string(&path).unwrap();
            serde_yml::from_str::<Workflow>(&yaml)
                .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        }
    }
}
