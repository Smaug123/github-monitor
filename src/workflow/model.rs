use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::de::{self, DeserializeOwned, Visitor};
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
    pub workflow_run: Option<WorkflowRun>,
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
                "workflow_run" => triggers.workflow_run = Some(WorkflowRun::default()),
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
            workflow_run: parse_optional_default(
                mapping_value(&mapping, "workflow_run"),
                "workflow_run",
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
    #[serde(
        default,
        rename = "paths-ignore",
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub paths_ignore: Vec<String>,
    /// Activity types the event is restricted to; empty means GitHub's
    /// defaults (for `pull_request`: `opened`, `synchronize`, `reopened`).
    #[serde(
        default,
        deserialize_with = "deserialize_string_or_vec",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowDispatch {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WorkflowRun {}

/// A job. Whether it is a reusable-workflow call is decided by the presence of
/// a `uses:` key (see [`RawJob`]), never by which shape happens to parse: an
/// untagged fallback once turned a call with a float input into an empty
/// standard job, hiding its `uses:` from every rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(try_from = "RawJob")]
pub struct Job {
    /// The display name, which is also the name of the job's status check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// The `strategy` block, kept opaque: rules only ask whether it has a
    /// matrix (which changes the job's status-check names).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<serde_json::Value>,
    #[serde(flatten)]
    pub kind: JobKind,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JobKind {
    Reusable(ReusableJob),
    Standard(StandardJob),
}

/// Every job key the model reads, before the reusable/standard split.
#[derive(Deserialize)]
struct RawJob {
    #[serde(default, deserialize_with = "deserialize_optional_scalar_text")]
    name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    needs: Vec<String>,
    #[serde(
        default,
        rename = "if",
        deserialize_with = "deserialize_optional_scalar_text"
    )]
    condition: Option<String>,
    #[serde(default)]
    strategy: Option<serde_json::Value>,
    #[serde(default)]
    uses: Option<ActionReference>,
    #[serde(default)]
    with: BTreeMap<String, WithValue>,
    #[serde(default, rename = "runs-on")]
    runs_on: Option<RunsOn>,
    #[serde(default)]
    steps: Vec<Step>,
}

impl TryFrom<RawJob> for Job {
    type Error = String;

    fn try_from(raw: RawJob) -> Result<Self, Self::Error> {
        let kind = match raw.uses {
            Some(uses) => {
                if raw.runs_on.is_some() || !raw.steps.is_empty() {
                    return Err(
                        "a job that calls a reusable workflow (`uses:`) cannot also have \
                         `runs-on:` or `steps:`"
                            .to_owned(),
                    );
                }
                JobKind::Reusable(ReusableJob {
                    uses,
                    with: raw.with,
                })
            }
            None => {
                if !raw.with.is_empty() {
                    return Err(
                        "`with:` is only valid on a job that calls a reusable workflow".to_owned(),
                    );
                }
                JobKind::Standard(StandardJob {
                    runs_on: raw.runs_on,
                    steps: raw.steps,
                })
            }
        };
        Ok(Self {
            name: raw.name,
            needs: raw.needs,
            condition: raw.condition,
            strategy: raw.strategy,
            kind,
        })
    }
}

impl Default for JobKind {
    fn default() -> Self {
        Self::Standard(StandardJob::default())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StandardJob {
    #[serde(default, rename = "runs-on", skip_serializing_if = "Option::is_none")]
    pub runs_on: Option<RunsOn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReusableJob {
    pub uses: ActionReference,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub with: BTreeMap<String, WithValue>,
}

impl Job {
    pub fn uses(&self) -> Option<&ActionReference> {
        match &self.kind {
            JobKind::Reusable(reusable) => Some(&reusable.uses),
            JobKind::Standard(_) => None,
        }
    }

    pub fn steps(&self) -> &[Step] {
        match &self.kind {
            JobKind::Standard(standard) => &standard.steps,
            JobKind::Reusable(_) => &[],
        }
    }

    /// The name of the single status check this job reports, given its key in
    /// `jobs:`, or `None` when that name is not a fixed string. A matrix
    /// appends its values to the name (one check per combination), an
    /// expression in `name:` is evaluated per run, and a reusable-workflow
    /// call reports `caller / callee` checks for the callee's jobs.
    pub fn status_check_name<'a>(&'a self, key: &'a str) -> Option<&'a str> {
        if self.has_matrix() || matches!(self.kind, JobKind::Reusable(_)) {
            return None;
        }
        match &self.name {
            None => Some(key),
            Some(name) if name.contains("${{") => None,
            Some(name) => Some(name),
        }
    }

    /// Whether the job's `strategy` defines a matrix. A `strategy` of a shape
    /// the model does not recognise counts as having one, since the caller
    /// then cannot know the job's check names.
    fn has_matrix(&self) -> bool {
        match &self.strategy {
            None => false,
            Some(serde_json::Value::Object(strategy)) => strategy.contains_key("matrix"),
            Some(_) => true,
        }
    }

    #[cfg(test)]
    pub fn runs_on(&self) -> Option<&RunsOn> {
        match &self.kind {
            JobKind::Standard(standard) => standard.runs_on.as_ref(),
            JobKind::Reusable(_) => None,
        }
    }
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

/// A step. Whether it runs an action or a script is decided by which of
/// `uses:`/`run:` is present (see [`RawStep`]), never by which shape happens to
/// parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawStep")]
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

/// Every step key the model reads, before the action/script split.
#[derive(Deserialize)]
struct RawStep {
    #[serde(default, deserialize_with = "deserialize_optional_scalar_text")]
    name: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(
        default,
        rename = "if",
        deserialize_with = "deserialize_optional_scalar_text"
    )]
    condition: Option<String>,
    #[serde(default)]
    uses: Option<ActionReference>,
    #[serde(default)]
    with: BTreeMap<String, WithValue>,
    #[serde(default)]
    run: Option<String>,
}

impl TryFrom<RawStep> for Step {
    type Error = String;

    fn try_from(raw: RawStep) -> Result<Self, Self::Error> {
        let kind = match (raw.uses, raw.run) {
            (Some(uses), None) => StepKind::Action(ActionStep {
                uses,
                with: raw.with,
            }),
            (None, Some(run)) if raw.with.is_empty() => StepKind::Run(RunStep { run }),
            (None, Some(_)) => {
                return Err("`with:` is only valid on a step that `uses:` an action".to_owned());
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err("a step must have exactly one of `uses:` or `run:`".to_owned());
            }
        };
        Ok(Self {
            name: raw.name,
            id: raw.id,
            condition: raw.condition,
            kind,
        })
    }
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

    #[cfg(test)]
    pub fn with(&self) -> Option<&BTreeMap<String, WithValue>> {
        match &self.kind {
            StepKind::Action(action) => Some(&action.with),
            StepKind::Run(_) => None,
        }
    }

    /// The `with:` input `name` of an action step; `None` for a `run:` step or
    /// an input it does not set.
    pub fn input(&self, name: &str) -> Option<&WithValue> {
        match &self.kind {
            StepKind::Action(action) => action.with.get(name),
            StepKind::Run(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
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
    #[cfg(test)]
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

/// A `with:` input. GitHub hands every input to the action as a string, but
/// the YAML may spell it as any scalar (`python-version: 3.12`, `ref:` with no
/// value), and all of those must parse.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WithValue {
    String(String),
    Bool(bool),
    Integer(i64),
    Float(f64),
    Null,
}

impl WithValue {
    /// The scalar's text, as GitHub reads it where it expects a string; `None`
    /// for `Null`.
    fn into_text(self) -> Option<String> {
        match self {
            Self::String(value) => Some(value),
            Self::Bool(value) => Some(value.to_string()),
            Self::Integer(value) => Some(value.to_string()),
            Self::Float(value) => Some(value.to_string()),
            Self::Null => None,
        }
    }
}

impl<'de> Deserialize<'de> for WithValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ScalarVisitor;

        impl<'de> Visitor<'de> for ScalarVisitor {
            type Value = WithValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a scalar (string, boolean, number, or null)")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(WithValue::String(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(WithValue::String(value))
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(WithValue::Bool(value))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(WithValue::Integer(value))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                i64::try_from(value).map(WithValue::Integer).map_err(|_| {
                    E::invalid_value(de::Unexpected::Unsigned(value), &"an integer within i64")
                })
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Ok(WithValue::Float(value))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(WithValue::Null)
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(WithValue::Null)
            }

            fn visit_some<D: Deserializer<'de>>(
                self,
                deserializer: D,
            ) -> Result<Self::Value, D::Error> {
                WithValue::deserialize(deserializer)
            }
        }

        deserializer.deserialize_any(ScalarVisitor)
    }
}

/// An optional scalar, as text. GitHub reads `if:` as an expression and
/// `name:` as text, so `if: false` and `name: 2` mean `"false"` and `"2"`.
fn deserialize_optional_scalar_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<WithValue>::deserialize(deserializer)?.and_then(WithValue::into_text))
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
    use proptest::test_runner::TestCaseError;
    use std::collections::BTreeSet;
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
            proptest::collection::vec(path_fragment(), 0..3),
            proptest::collection::vec(identifier(), 0..3),
        )
            .prop_map(
                |(branches, branches_ignore, tags, tags_ignore, paths, paths_ignore, types)| {
                    TriggerFilter {
                        branches,
                        branches_ignore,
                        tags,
                        tags_ignore,
                        paths,
                        paths_ignore,
                        types,
                    }
                },
            )
    }

    fn triggers_strategy() -> impl Strategy<Value = Triggers> {
        (
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            proptest::option::of(trigger_filter_strategy()),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(push, pull_request, pull_request_target, workflow_run, workflow_dispatch)| {
                    Triggers {
                        push,
                        pull_request,
                        pull_request_target,
                        workflow_run: workflow_run.then_some(WorkflowRun::default()),
                        workflow_dispatch: workflow_dispatch.then_some(WorkflowDispatch::default()),
                    }
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
            (-1.0e6_f64..1.0e6).prop_map(WithValue::Float),
            Just(WithValue::Null),
        ]
    }

    fn job_strategy_block() -> impl Strategy<Value = serde_json::Value> {
        prop_oneof![
            proptest::collection::vec(identifier(), 1..3)
                .prop_map(|os| serde_json::json!({ "matrix": { "os": os } })),
            any::<bool>().prop_map(|fail_fast| serde_json::json!({ "fail-fast": fail_fast })),
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
        let standard = (
            proptest::option::of(runs_on_strategy()),
            proptest::collection::vec(step_strategy(), 0..4),
        )
            .prop_map(|(runs_on, steps)| JobKind::Standard(StandardJob { runs_on, steps }));
        let reusable = (
            action_reference_strategy(),
            proptest::collection::btree_map(identifier(), with_value_strategy(), 0..3),
        )
            .prop_map(|(uses, with)| JobKind::Reusable(ReusableJob { uses, with }));

        (
            proptest::option::of(path_fragment()),
            proptest::collection::vec(identifier(), 0..3),
            proptest::option::of(path_fragment()),
            proptest::option::of(job_strategy_block()),
            prop_oneof![standard, reusable],
        )
            .prop_map(|(name, needs, condition, strategy, kind)| Job {
                name,
                needs,
                condition,
                strategy,
                kind,
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
                paths_ignore: Vec::new(),
                types: Vec::new(),
            })
        );
        assert_eq!(
            workflow.triggers.workflow_dispatch,
            Some(WorkflowDispatch::default())
        );

        let build = workflow.jobs.get("build").unwrap();
        assert_eq!(
            build.runs_on(),
            Some(&RunsOn::Label("ubuntu-latest".to_owned()))
        );
        assert_eq!(build.steps().first().unwrap().run(), Some("cargo test"));

        let self_hosted_build = workflow.jobs.get("self-hosted-build").unwrap();
        assert_eq!(
            self_hosted_build.runs_on(),
            Some(&RunsOn::Labels(vec![
                "self-hosted".to_owned(),
                "linux".to_owned(),
            ]))
        );

        let deploy = workflow.jobs.get("deploy").unwrap();
        assert_eq!(
            deploy.runs_on(),
            Some(&RunsOn::Group(RunsOnGroup {
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
    fn parses_workflow_run_string_trigger() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on: workflow_run
jobs: {}
"#,
        )
        .unwrap();

        assert_eq!(workflow.triggers.workflow_run, Some(WorkflowRun::default()));
    }

    #[test]
    fn parses_workflow_run_in_list() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  - push
  - workflow_run
jobs: {}
"#,
        )
        .unwrap();

        assert_eq!(workflow.triggers.push, Some(TriggerFilter::default()));
        assert_eq!(workflow.triggers.workflow_run, Some(WorkflowRun::default()));
    }

    #[test]
    fn parses_workflow_run_map_trigger() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on:
  workflow_run:
    workflows: [CI]
    types: [completed]
jobs: {}
"#,
        )
        .unwrap();

        assert_eq!(workflow.triggers.workflow_run, Some(WorkflowRun::default()));
    }

    #[test]
    fn parses_reusable_job() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on: push
jobs:
  call-shared:
    uses: foo/bar/.github/workflows/x.yml@v1
"#,
        )
        .unwrap();

        let job = workflow.jobs.get("call-shared").unwrap();
        let uses = job.uses().unwrap();
        assert_eq!(
            uses,
            &ActionReference::Other("foo/bar/.github/workflows/x.yml@v1".to_owned())
        );
        assert!(job.steps().is_empty());
        assert!(job.runs_on().is_none());
        assert!(matches!(job.kind, JobKind::Reusable(_)));
    }

    #[test]
    fn parses_reusable_job_with_with() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on: push
jobs:
  call-shared:
    uses: foo/bar/.github/workflows/x.yml@v1
    with:
      flag: true
      label: release
"#,
        )
        .unwrap();

        let job = workflow.jobs.get("call-shared").unwrap();
        let JobKind::Reusable(reusable) = &job.kind else {
            panic!("expected reusable job, got {:?}", job.kind);
        };
        assert_eq!(reusable.with.get("flag"), Some(&WithValue::Bool(true)));
        assert_eq!(
            reusable.with.get("label"),
            Some(&WithValue::String("release".to_owned()))
        );
    }

    #[test]
    fn parses_standard_job_still_works() {
        let workflow: Workflow = serde_yml::from_str(
            r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#,
        )
        .unwrap();

        let job = workflow.jobs.get("build").unwrap();
        assert!(matches!(job.kind, JobKind::Standard(_)));
        assert!(job.uses().is_none());
        assert_eq!(
            job.runs_on(),
            Some(&RunsOn::Label("ubuntu-latest".to_owned()))
        );
        assert_eq!(job.steps().first().unwrap().run(), Some("echo hi"));
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
                paths_ignore: Vec::new(),
                types: Vec::new(),
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
                paths_ignore: Vec::new(),
                types: Vec::new(),
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
                paths_ignore: Vec::new(),
                types: Vec::new(),
            })
        );
        let build = workflow.jobs.get("build").unwrap();
        assert_eq!(
            build.runs_on(),
            Some(&RunsOn::Group(RunsOnGroup {
                group: Some("linux".to_owned()),
                labels: Vec::new(),
            }))
        );

        let step = build.steps().first().unwrap();
        let uses = step.uses().unwrap().as_action_ref().unwrap();
        assert_eq!(uses, &ActionRef::new("actions", "checkout", "v4"));
        assert_eq!(
            step.with().unwrap().get("fetch-depth"),
            Some(&WithValue::Integer(1))
        );
    }

    /// Any YAML scalar: GitHub coerces each of these to a string when it is an
    /// action input, and accepts a bool or number where it expects an
    /// expression (`if:`) or a display name (`name:`).
    fn yaml_scalar() -> impl Strategy<Value = serde_yml::Value> {
        prop_oneof![
            "[a-z0-9 ._-]{0,12}".prop_map(serde_yml::Value::from),
            any::<bool>().prop_map(serde_yml::Value::from),
            any::<i64>().prop_map(serde_yml::Value::from),
            (-1.0e9_f64..1.0e9).prop_map(serde_yml::Value::from),
            Just(serde_yml::Value::Null),
        ]
    }

    fn yaml_mapping<K: Into<serde_yml::Value>>(
        entries: impl IntoIterator<Item = (K, serde_yml::Value)>,
    ) -> serde_yml::Value {
        let mut mapping = serde_yml::Mapping::new();
        for (key, value) in entries {
            mapping.insert(key.into(), value);
        }
        serde_yml::Value::Mapping(mapping)
    }

    fn with_inputs() -> impl Strategy<Value = Option<serde_yml::Value>> {
        proptest::collection::btree_map("[a-z][a-z0-9-]{0,8}", yaml_scalar(), 0..4)
            .prop_map(|inputs| (!inputs.is_empty()).then(|| yaml_mapping(inputs)))
    }

    fn if_condition() -> impl Strategy<Value = Option<serde_yml::Value>> {
        proptest::option::of(prop_oneof![
            any::<bool>().prop_map(serde_yml::Value::from),
            Just(serde_yml::Value::from("${{ always() }}")),
            Just(serde_yml::Value::from("github.event_name == 'push'")),
        ])
    }

    fn display_name() -> impl Strategy<Value = Option<serde_yml::Value>> {
        proptest::option::of(prop_oneof![
            "[A-Za-z][A-Za-z0-9 ]{0,12}".prop_map(serde_yml::Value::from),
            any::<i64>().prop_map(serde_yml::Value::from),
            (-1.0e3_f64..1.0e3).prop_map(serde_yml::Value::from),
        ])
    }

    fn step_uses() -> impl Strategy<Value = String> {
        prop_oneof![
            (
                "[a-z]{1,8}",
                "[a-z]{1,8}",
                prop_oneof!["v[0-9]", "[0-9a-f]{40}", Just("main".to_owned())],
            )
                .prop_map(|(owner, repo, version)| format!("{owner}/{repo}@{version}")),
            ("[a-z]{1,8}", "[a-z]{1,8}").prop_map(|(owner, repo)| format!("{owner}/{repo}/sub@v1")),
            Just("docker://alpine:3".to_owned()),
            Just("./.github/actions/local".to_owned()),
        ]
    }

    fn reusable_uses() -> impl Strategy<Value = String> {
        (
            "[a-z]{1,8}",
            "[a-z]{1,8}",
            prop_oneof![Just("main"), Just("v1")],
        )
            .prop_map(|(owner, repo, version)| {
                format!("{owner}/{repo}/.github/workflows/build.yml@{version}")
            })
    }

    /// Appends the optional keys, skipping absent ones (a present `null` is a
    /// different document from an absent key).
    fn with_optional_keys(
        mut entries: Vec<(&'static str, serde_yml::Value)>,
        optional: Vec<(&'static str, Option<serde_yml::Value>)>,
    ) -> serde_yml::Value {
        entries.extend(
            optional
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        );
        yaml_mapping(entries)
    }

    fn step_document() -> impl Strategy<Value = serde_yml::Value> {
        let action = (step_uses(), with_inputs(), if_condition(), display_name()).prop_map(
            |(uses, with, condition, name)| {
                with_optional_keys(
                    vec![("uses", serde_yml::Value::from(uses))],
                    vec![("with", with), ("if", condition), ("name", name)],
                )
            },
        );
        let run = (".{1,20}", if_condition(), display_name()).prop_map(|(run, condition, name)| {
            with_optional_keys(
                vec![("run", serde_yml::Value::from(run))],
                vec![("if", condition), ("name", name)],
            )
        });
        prop_oneof![action, run]
    }

    fn job_document() -> impl Strategy<Value = serde_yml::Value> {
        let standard = (
            proptest::collection::vec(step_document(), 1..4),
            if_condition(),
            display_name(),
        )
            .prop_map(|(steps, condition, name)| {
                with_optional_keys(
                    vec![
                        ("runs-on", serde_yml::Value::from("ubuntu-latest")),
                        ("steps", serde_yml::Value::Sequence(steps)),
                    ],
                    vec![("if", condition), ("name", name)],
                )
            });
        let reusable = (
            reusable_uses(),
            with_inputs(),
            if_condition(),
            display_name(),
        )
            .prop_map(|(uses, with, condition, name)| {
                with_optional_keys(
                    vec![("uses", serde_yml::Value::from(uses))],
                    vec![("with", with), ("if", condition), ("name", name)],
                )
            });
        prop_oneof![standard, reusable]
    }

    fn workflow_document() -> impl Strategy<Value = serde_yml::Value> {
        proptest::collection::btree_map("[a-z][a-z0-9_-]{0,8}", job_document(), 1..4).prop_map(
            |jobs| {
                yaml_mapping([
                    ("on", serde_yml::Value::from("push")),
                    ("jobs", yaml_mapping(jobs)),
                ])
            },
        )
    }

    /// Every `uses:` reference in a workflow document, found by walking the raw
    /// YAML rather than the typed model: `job: ref` for a reusable-workflow
    /// call and `job[i]: ref` for step `i`'s action.
    fn uses_in_raw_yaml(document: &serde_yml::Value) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let jobs = document["jobs"].as_mapping().expect("jobs is a mapping");
        for (key, job) in jobs {
            let key = key.as_str().expect("job keys are strings");
            if let Some(uses) = job.get("uses").and_then(serde_yml::Value::as_str) {
                found.insert(format!("{key}: {uses}"));
            }
            let steps = job.get("steps").and_then(serde_yml::Value::as_sequence);
            for (index, step) in steps.into_iter().flatten().enumerate() {
                if let Some(uses) = step.get("uses").and_then(serde_yml::Value::as_str) {
                    found.insert(format!("{key}[{index}]: {uses}"));
                }
            }
        }
        found
    }

    fn uses_in_model(workflow: &Workflow) -> BTreeSet<String> {
        fn text(uses: &ActionReference) -> String {
            match uses {
                ActionReference::Repository(action_ref) => action_ref.to_string(),
                ActionReference::Other(raw) => raw.clone(),
            }
        }

        let mut found = BTreeSet::new();
        for (key, job) in &workflow.jobs {
            if let Some(uses) = job.uses() {
                found.insert(format!("{key}: {}", text(uses)));
            }
            for (index, step) in job.steps().iter().enumerate() {
                if let Some(uses) = step.uses() {
                    found.insert(format!("{key}[{index}]: {}", text(uses)));
                }
            }
        }
        found
    }

    proptest! {
        /// Oracle for the typed model: it must parse every workflow GitHub
        /// accepts (whatever scalar types appear in `with:`, `if:` and `name:`)
        /// and expose exactly the `uses:` references a raw YAML walk finds. A
        /// reference the model drops is invisible to WF002, so it passes
        /// vacuously.
        #[test]
        fn model_exposes_exactly_the_uses_references_in_the_yaml(
            document in workflow_document(),
        ) {
            let yaml = serde_yml::to_string(&document).unwrap();
            let workflow: Workflow = serde_yml::from_str(&yaml)
                .map_err(|error| TestCaseError::fail(format!("{error}\n{yaml}")))?;
            let reparsed: serde_yml::Value = serde_yml::from_str(&yaml).unwrap();

            prop_assert_eq!(uses_in_model(&workflow), uses_in_raw_yaml(&reparsed), "{}", yaml);
        }
    }

    #[test]
    fn reusable_job_with_non_string_inputs_keeps_its_uses() {
        // Regression: a float or null input made the reusable variant fail to
        // parse, and the untagged fallback silently produced an empty
        // standard job, hiding the unpinned `uses:` from WF002.
        for input in ["1.5", "", "~", "null"] {
            let yaml = format!(
                "on: push\njobs:\n  call:\n    uses: evil/repo/.github/workflows/x.yml@main\n    \
                 with:\n      v: {input}\n"
            );
            let workflow: Workflow = serde_yml::from_str(&yaml).unwrap();
            assert_eq!(
                workflow.jobs["call"].uses(),
                Some(&ActionReference::Other(
                    "evil/repo/.github/workflows/x.yml@main".to_owned()
                )),
                "input `{input}`"
            );
        }
    }

    #[test]
    fn parses_steps_with_non_string_scalars() {
        // Each of these is a valid workflow that used to abort the whole run.
        let yaml = "on: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      \
                    - uses: actions/setup-python@v5\n        with:\n          python-version: 3.12\n          \
                    cache:\n      - run: echo hi\n        if: false\n        name: 2\n";
        let workflow: Workflow = serde_yml::from_str(yaml).unwrap();
        let steps = workflow.jobs["build"].steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[1].condition.as_deref(), Some("false"));
        assert_eq!(steps[1].name.as_deref(), Some("2"));
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
