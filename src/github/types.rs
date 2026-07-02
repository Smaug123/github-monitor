use serde::{Deserialize, Serialize};

use crate::types::{BranchName, RepoName};

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal,)* }) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum $name {
            $($variant,)*
            Unknown(String),
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($value => Self::$variant,)*
                    _ => Self::Unknown(value),
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                match value {
                    $( $name::$variant => $value.to_owned(), )*
                    $name::Unknown(value) => value,
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                let value = match self {
                    $(Self::$variant => $value,)*
                    Self::Unknown(value) => value.as_str(),
                };

                serializer.serialize_str(value)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Ok(Self::from(value))
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repository {
    pub name: RepoName,
    pub default_branch: BranchName,
    // `private`/`archived`/`disabled` are returned by `GET /repos` for every
    // caller, so they are required: a missing one is a malformed response, not a
    // silent `false`.
    pub private: bool,
    pub archived: bool,
    pub disabled: bool,
    // GitHub omits the merge-policy booleans from `GET /repos` when the caller
    // lacks permission to read them (e.g. a mis-scoped token). `None` therefore
    // means "not reported by the API" — an unknown, distinct from a definite
    // `false`. Serde maps a missing `Option` field to `None`, which is exactly
    // the unknown we want to preserve for the rules to turn into `Error`.
    pub allow_auto_merge: Option<bool>,
    pub delete_branch_on_merge: Option<bool>,
    pub allow_update_branch: Option<bool>,
    pub allow_squash_merge: Option<bool>,
    pub allow_merge_commit: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepositoryUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_auto_merge: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_branch_on_merge: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_update_branch: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_squash_merge: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_merge_commit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_rebase_merge: Option<bool>,
}

impl RepositoryUpdate {
    pub fn is_empty(&self) -> bool {
        self.private.is_none()
            && self.archived.is_none()
            && self.disabled.is_none()
            && self.allow_auto_merge.is_none()
            && self.delete_branch_on_merge.is_none()
            && self.allow_update_branch.is_none()
            && self.allow_squash_merge.is_none()
            && self.allow_merge_commit.is_none()
            && self.allow_rebase_merge.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateGitReference {
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdateRepositoryFile {
    pub message: String,
    pub content: String,
    /// SHA of the blob to replace. Omit when creating a new file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    pub branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatePullRequest {
    pub title: String,
    pub head: String,
    pub base: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitReference {
    #[serde(rename = "ref")]
    pub reference: String,
    pub object: GitReferenceObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitReferenceObject {
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitRef {
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub html_url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset {
    pub id: u64,
    pub name: String,
    pub target: RulesetTarget,
    pub enforcement: RulesetEnforcement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<RulesetConditions>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_actors: Vec<BypassActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RulesetRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateRulesetRequest {
    pub name: String,
    pub target: RulesetTarget,
    pub enforcement: RulesetEnforcement,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<RulesetConditions>,
    pub bypass_actors: Vec<BypassActor>,
    pub rules: Vec<RulesetRule>,
}

impl UpdateRulesetRequest {
    pub fn from_ruleset(ruleset: &Ruleset) -> Self {
        Self {
            name: ruleset.name.clone(),
            target: ruleset.target.clone(),
            enforcement: ruleset.enforcement.clone(),
            conditions: ruleset.conditions.clone(),
            bypass_actors: ruleset.bypass_actors.clone(),
            rules: ruleset.rules.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesetConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<RefNameCondition>,
}

// GitHub's PUT /repos/{owner}/{repo}/rulesets/{ruleset_id} endpoint requires
// both `include` and `exclude` to be present whenever `ref_name` is sent —
// omitting either yields a 422 "Missing required parameter" response — so we
// always serialize them, even when empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefNameCondition {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BypassActor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<u64>,
    pub actor_type: BypassActorType,
    pub bypass_mode: BypassMode,
}

/// A single rule within a ruleset. On the wire each rule is a
/// `{"type": <kind>, "parameters": {...}}` object whose parameter shape depends
/// on the kind. The two kinds this crate reads and rewrites in detail get typed
/// variants, so an incomplete `pull_request` rule (missing required review
/// fields) or `required_status_checks` rule (missing its check list or strict
/// flag) — the two historical 422s — is unrepresentable. Every other kind is
/// carried in `Other` with its `parameters` object verbatim, so a ruleset read
/// from the live API round-trips losslessly through a GET-modify-PUT.
///
/// `serde_json::Value` (in `Other` and the flattened `extra` maps) isn't `Eq`,
/// so this type and its containers are `PartialEq`-only.
#[derive(Debug, Clone, PartialEq)]
pub enum RulesetRule {
    PullRequest(PullRequestParameters),
    RequiredStatusChecks(RequiredStatusChecksParameters),
    /// Any other rule kind, with its `parameters` object preserved verbatim
    /// (`None` when the rule carries no `parameters` key). Covers both the
    /// parameterless kinds this crate only checks for presence and any kind it
    /// does not model — so a fix's GET-modify-PUT never drops a rule's
    /// configuration (finding 2).
    Other {
        kind: RulesetRuleType,
        parameters: Option<serde_json::Value>,
    },
}

impl RulesetRule {
    /// The rule's `type` discriminant, for presence checks and reporting.
    pub fn kind(&self) -> RulesetRuleType {
        match self {
            RulesetRule::PullRequest(_) => RulesetRuleType::PullRequest,
            RulesetRule::RequiredStatusChecks(_) => RulesetRuleType::RequiredStatusChecks,
            RulesetRule::Other { kind, .. } => kind.clone(),
        }
    }

    /// A parameterless rule of the given kind (e.g. `deletion`,
    /// `required_linear_history`) — used when the planner adds a rule it only
    /// needs to assert the presence of.
    pub fn parameterless(kind: RulesetRuleType) -> Self {
        RulesetRule::Other {
            kind,
            parameters: None,
        }
    }
}

impl Serialize for RulesetRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        let mut map = serializer.serialize_map(None)?;
        match self {
            RulesetRule::PullRequest(parameters) => {
                map.serialize_entry("type", "pull_request")?;
                map.serialize_entry("parameters", parameters)?;
            }
            RulesetRule::RequiredStatusChecks(parameters) => {
                map.serialize_entry("type", "required_status_checks")?;
                map.serialize_entry("parameters", parameters)?;
            }
            RulesetRule::Other { kind, parameters } => {
                map.serialize_entry("type", kind)?;
                if let Some(parameters) = parameters {
                    map.serialize_entry("parameters", parameters)?;
                }
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for RulesetRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            parameters: Option<serde_json::Value>,
        }

        let Raw { kind, parameters } = Raw::deserialize(deserializer)?;
        let rule = match kind.as_str() {
            "pull_request" => RulesetRule::PullRequest(rule_parameters(&kind, parameters)?),
            "required_status_checks" => {
                RulesetRule::RequiredStatusChecks(rule_parameters(&kind, parameters)?)
            }
            _ => RulesetRule::Other {
                kind: RulesetRuleType::from(kind),
                parameters,
            },
        };
        Ok(rule)
    }
}

/// Parses a known rule kind's `parameters` object into its typed form. A known
/// kind with parameters absent is a malformed rule (GitHub always sends them).
fn rule_parameters<T, E>(kind: &str, parameters: Option<serde_json::Value>) -> Result<T, E>
where
    T: serde::de::DeserializeOwned,
    E: serde::de::Error,
{
    let parameters = parameters
        .ok_or_else(|| E::custom(format!("`{kind}` ruleset rule is missing `parameters`")))?;
    serde_json::from_value(parameters).map_err(E::custom)
}

/// Parameters of a `pull_request` ruleset rule. GitHub's create/update-ruleset
/// endpoint requires all five review fields whenever the rule is present (a 422
/// "data matches no possible input" otherwise), so they are non-optional and
/// always serialized; see `new_pull_request_rule_with_required_defaults`. They
/// are `#[serde(default)]` only so a response omitting one parses to GitHub's own
/// default rather than erroring. `extra` carries any parameter the model doesn't
/// name (e.g. `required_reviewers`) through GET-modify-PUT verbatim, so a fix
/// never silently drops configuration GitHub still enforces (finding 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PullRequestParameters {
    #[serde(default)]
    pub dismiss_stale_reviews_on_push: bool,
    #[serde(default)]
    pub require_code_owner_review: bool,
    #[serde(default)]
    pub require_last_push_approval: bool,
    #[serde(default)]
    pub required_approving_review_count: u32,
    #[serde(default)]
    pub required_review_thread_resolution: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_merge_methods: Vec<MergeMethod>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Parameters of a `required_status_checks` ruleset rule. GitHub requires both
/// the check list (which may be empty) and the strict-policy flag, so both are
/// always serialized — dropping an empty check list yields a 422. `extra` carries
/// unmodeled parameters through verbatim (see [`PullRequestParameters`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RequiredStatusChecksParameters {
    #[serde(default)]
    pub required_status_checks: Vec<RequiredStatusCheck>,
    #[serde(default)]
    pub strict_required_status_checks_policy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_not_enforce_on_create: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// App ID of the first-party GitHub Actions GitHub App on github.com. A
/// required status check pins the app allowed to report it via `integration_id`;
/// this is the value GitHub records when the check must come from GitHub Actions.
/// (On GitHub Enterprise Server the ID differs; this crate audits github.com.)
pub(crate) const GITHUB_ACTIONS_INTEGRATION_ID: u64 = 15368;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredStatusCheck {
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<u64>,
}

// Parsed shape of the legacy (pre-rulesets) branch-protection endpoint. We only
// model fields the autofix needs to reason about when deciding whether
// rulesets supersede the legacy protection; unknown fields are silently
// dropped on deserialization, which means the autofix planner must refuse to
// delete an empty parse (could be GitHub fields we don't know about yet).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_status_checks: Option<LegacyRequiredStatusChecks>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_pull_request_reviews: Option<LegacyRequiredPullRequestReviews>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_linear_history: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_force_pushes: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_deletions: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_signatures: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_conversation_resolution: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforce_admins: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_creations: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_branch: Option<LegacyEnabledFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restrictions: Option<LegacyRestrictions>,
}

impl BranchProtection {
    /// True when every parsed field is absent — either because the protection
    /// is genuinely empty, or because GitHub returned fields our model doesn't
    /// yet recognise. The autofix treats this as ambiguous.
    pub fn is_empty(&self) -> bool {
        let Self {
            required_status_checks,
            required_pull_request_reviews,
            required_linear_history,
            allow_force_pushes,
            allow_deletions,
            required_signatures,
            required_conversation_resolution,
            enforce_admins,
            block_creations,
            lock_branch,
            restrictions,
        } = self;
        required_status_checks.is_none()
            && required_pull_request_reviews.is_none()
            && required_linear_history.is_none()
            && allow_force_pushes.is_none()
            && allow_deletions.is_none()
            && required_signatures.is_none()
            && required_conversation_resolution.is_none()
            && enforce_admins.is_none()
            && block_creations.is_none()
            && lock_branch.is_none()
            && restrictions.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyEnabledFlag {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyRequiredStatusChecks {
    #[serde(default)]
    pub strict: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contexts: Vec<String>,
    // Newer field replacing `contexts`; carries an optional app id alongside.
    // We normalise to context strings for the supersedes check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<LegacyStatusCheck>,
}

impl LegacyRequiredStatusChecks {
    /// Union of `contexts` and the context names from `checks`.
    pub fn all_contexts(&self) -> std::collections::BTreeSet<String> {
        let mut set: std::collections::BTreeSet<String> = self.contexts.iter().cloned().collect();
        for check in &self.checks {
            set.insert(check.context.clone());
        }
        set
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyStatusCheck {
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyRequiredPullRequestReviews {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_approving_review_count: Option<u32>,
    #[serde(default)]
    pub require_code_owner_reviews: bool,
    #[serde(default)]
    pub dismiss_stale_reviews: bool,
    #[serde(default)]
    pub require_last_push_approval: bool,
    #[serde(default)]
    pub required_review_thread_resolution: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_pull_request_allowances: Option<LegacyBypassPullRequestAllowances>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyBypassPullRequestAllowances {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<serde_json::Value>,
}

impl LegacyBypassPullRequestAllowances {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.teams.is_empty() && self.apps.is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyRestrictions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub teams: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<serde_json::Value>,
}

impl LegacyRestrictions {
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.teams.is_empty() && self.apps.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryFileContent {
    pub name: String,
    pub path: String,
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: RepositoryContentType,
    pub encoding: ContentEncoding,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDirectoryEntry {
    pub name: String,
    pub path: String,
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: RepositoryContentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RepositoryContents {
    File(RepositoryFileContent),
    Directory(Vec<RepositoryDirectoryEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitTree {
    pub sha: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tree: Vec<GitTreeEntry>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitTreeEntry {
    pub path: String,
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: GitTreeEntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

string_enum!(RulesetTarget {
    Branch => "branch",
    Tag => "tag",
    Push => "push",
});

string_enum!(RulesetEnforcement {
    Active => "active",
    Evaluate => "evaluate",
    Disabled => "disabled",
});

string_enum!(BypassActorType {
    OrganizationAdmin => "OrganizationAdmin",
    RepositoryRole => "RepositoryRole",
    Team => "Team",
    Integration => "Integration",
    DeployKey => "DeployKey",
});

string_enum!(BypassMode {
    Always => "always",
    PullRequest => "pull_request",
});

string_enum!(MergeMethod {
    Merge => "merge",
    Squash => "squash",
    Rebase => "rebase",
});

string_enum!(RulesetRuleType {
    Creation => "creation",
    Update => "update",
    Deletion => "deletion",
    RequiredLinearHistory => "required_linear_history",
    RequiredSignatures => "required_signatures",
    PullRequest => "pull_request",
    RequiredStatusChecks => "required_status_checks",
    NonFastForward => "non_fast_forward",
});

string_enum!(RepositoryContentType {
    File => "file",
    Dir => "dir",
    Symlink => "symlink",
    Submodule => "submodule",
});

string_enum!(ContentEncoding {
    Base64 => "base64",
    Utf8 => "utf-8",
});

string_enum!(GitTreeEntryType {
    Blob => "blob",
    Tree => "tree",
    Commit => "commit",
});

string_enum!(ForkPrApprovalPolicy {
    AllExternalContributors => "all_external_contributors",
    FirstTimeContributorsNewToGithub => "first_time_contributors_new_to_github",
    FirstTimeContributors => "first_time_contributors",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkPrApprovalPermission {
    pub approval_policy: ForkPrApprovalPolicy,
}

string_enum!(DefaultWorkflowPermissions {
    Read => "read",
    Write => "write",
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowPermissions {
    pub default_workflow_permissions: DefaultWorkflowPermissions,
    pub can_approve_pull_request_reviews: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding 4: a `Repository` boolean that is *absent* from the API response must
    /// not silently become `false`. Under a mis-scoped token GitHub omits the
    /// merge-policy and visibility booleans entirely; `#[serde(default)]` currently
    /// turns each absence into a definite `false`, so e.g. ST004 ("merge commits
    /// disabled") passes vacuously. Absence must be *distinguishable* from an
    /// explicit `false`, whether the fix makes the field required (absence => parse
    /// error) or `Option<bool>` (absence => `None`).
    ///
    /// RED today: both deserialize to the same `false`.
    #[test]
    fn absent_repository_boolean_is_distinguishable_from_explicit_false() {
        let boolean_fields = [
            "private",
            "archived",
            "disabled",
            "allow_auto_merge",
            "delete_branch_on_merge",
            "allow_update_branch",
            "allow_squash_merge",
            "allow_merge_commit",
            "allow_rebase_merge",
        ];

        for field in boolean_fields {
            let mut with_false = serde_json::json!({
                "name": "repo",
                "default_branch": "main",
                "private": true,
                "archived": true,
                "disabled": true,
                "allow_auto_merge": true,
                "delete_branch_on_merge": true,
                "allow_update_branch": true,
                "allow_squash_merge": true,
                "allow_merge_commit": true,
                "allow_rebase_merge": true,
            });
            // Field under test present-and-`false`; every other field stays `true`.
            with_false[field] = serde_json::Value::Bool(false);

            let mut without = with_false.clone();
            without.as_object_mut().expect("object").remove(field);

            let explicit_false = serde_json::from_value::<Repository>(with_false);
            let absent = serde_json::from_value::<Repository>(without);

            // If either side fails to parse, absence is already distinguishable from
            // an explicit `false`, so the invariant holds. It's the both-`Ok`,
            // both-equal case that is the coercion bug.
            if let (Ok(explicit_false), Ok(absent)) = (explicit_false, absent) {
                assert_ne!(
                    explicit_false, absent,
                    "`Repository` field `{field}` deserialized identically whether \
                     present-and-false or absent; a missing (privilege-gated) field is \
                     being coerced to `false`",
                );
            }
        }
    }

    #[test]
    fn deserializes_ruleset_payload() {
        let ruleset: Ruleset = serde_json::from_str(
            r#"
{
  "id": 42,
  "name": "main protection",
  "target": "branch",
  "enforcement": "active",
  "conditions": {
    "ref_name": {
      "include": ["~DEFAULT_BRANCH"],
      "exclude": []
    }
  },
  "bypass_actors": [
    {
      "actor_id": 5,
      "actor_type": "RepositoryRole",
      "bypass_mode": "always"
    }
  ],
  "rules": [
    {
      "type": "required_status_checks",
      "parameters": {
        "required_status_checks": [
          { "context": "ci", "integration_id": 1 }
        ],
        "strict_required_status_checks_policy": true
      }
    },
    {
      "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 2,
        "require_code_owner_review": true,
        "require_last_push_approval": false,
        "dismiss_stale_reviews_on_push": false,
        "required_review_thread_resolution": false,
        "allowed_merge_methods": ["squash"]
      }
    }
  ]
}
"#,
        )
        .unwrap();

        assert_eq!(ruleset.target, RulesetTarget::Branch);
        assert_eq!(ruleset.enforcement, RulesetEnforcement::Active);
        assert_eq!(ruleset.rules.len(), 2);
        assert_eq!(
            ruleset.rules[0].kind(),
            RulesetRuleType::RequiredStatusChecks
        );
        assert_eq!(ruleset.rules[1].kind(), RulesetRuleType::PullRequest);
        let RulesetRule::PullRequest(pull_request_parameters) = &ruleset.rules[1] else {
            panic!("expected a pull_request rule, got {:?}", ruleset.rules[1]);
        };
        assert_eq!(
            pull_request_parameters.allowed_merge_methods,
            vec![MergeMethod::Squash]
        );
        let conditions = ruleset.conditions.unwrap();
        let ref_name = conditions.ref_name.unwrap();
        assert_eq!(ref_name.include, vec!["~DEFAULT_BRANCH"]);
        assert!(ref_name.exclude.is_empty());
    }

    #[test]
    fn deserializes_branch_protection_payload_with_populated_fields() {
        let protection: BranchProtection = serde_json::from_str(
            r#"
{
  "url": "https://api.github.com/repos/example/example/branches/main/protection",
  "required_status_checks": {
    "url": "https://api.github.com/repos/example/example/branches/main/protection/required_status_checks",
    "strict": true,
    "contexts": ["ci"],
    "checks": [{"context": "ci", "app_id": 17}]
  },
  "required_pull_request_reviews": {
    "url": "https://api.github.com/repos/example/example/branches/main/protection/required_pull_request_reviews",
    "required_approving_review_count": 2,
    "require_code_owner_reviews": true,
    "dismiss_stale_reviews": true,
    "require_last_push_approval": false,
    "required_review_thread_resolution": true
  },
  "enforce_admins": {
    "url": "https://api.github.com/repos/example/example/branches/main/protection/enforce_admins",
    "enabled": true
  },
  "required_linear_history": {"enabled": true},
  "allow_force_pushes": {"enabled": false},
  "allow_deletions": {"enabled": false},
  "required_signatures": {"enabled": true},
  "required_conversation_resolution": {"enabled": true},
  "block_creations": {"enabled": false},
  "lock_branch": {"enabled": false}
}
"#,
        )
        .unwrap();

        let status_checks = protection.required_status_checks.as_ref().unwrap();
        assert!(status_checks.strict);
        assert_eq!(status_checks.contexts, vec!["ci".to_owned()]);
        assert_eq!(status_checks.checks.len(), 1);
        assert_eq!(status_checks.checks[0].context, "ci");
        assert_eq!(status_checks.checks[0].app_id, Some(17));

        let reviews = protection.required_pull_request_reviews.as_ref().unwrap();
        assert_eq!(reviews.required_approving_review_count, Some(2));
        assert!(reviews.require_code_owner_reviews);
        assert!(reviews.dismiss_stale_reviews);
        assert!(!reviews.require_last_push_approval);
        assert!(reviews.required_review_thread_resolution);

        assert_eq!(
            protection.enforce_admins,
            Some(LegacyEnabledFlag { enabled: true })
        );
        assert_eq!(
            protection.required_linear_history,
            Some(LegacyEnabledFlag { enabled: true })
        );
        assert_eq!(
            protection.allow_force_pushes,
            Some(LegacyEnabledFlag { enabled: false })
        );
        assert_eq!(
            protection.required_signatures,
            Some(LegacyEnabledFlag { enabled: true })
        );
        assert_eq!(
            protection.required_conversation_resolution,
            Some(LegacyEnabledFlag { enabled: true })
        );
        assert!(!protection.is_empty());
    }

    #[test]
    fn deserializes_branch_protection_payload_ignoring_unknown_fields() {
        let protection: BranchProtection =
            serde_json::from_str(r#"{"unrecognised_field": {"enabled": true}}"#).unwrap();
        assert!(protection.is_empty());
        assert_eq!(serde_json::to_string(&protection).unwrap(), "{}");
    }

    #[test]
    fn deserializes_ruleset_without_conditions() {
        let ruleset: Ruleset = serde_json::from_str(
            r#"
{
  "id": 1,
  "name": "legacy",
  "target": "branch",
  "enforcement": "active"
}
"#,
        )
        .unwrap();

        assert!(ruleset.conditions.is_none());
    }

    #[test]
    fn update_ruleset_request_always_serializes_ref_name_include_and_exclude() {
        let ruleset = Ruleset {
            id: 42,
            name: "main".to_owned(),
            target: RulesetTarget::Branch,
            enforcement: RulesetEnforcement::Active,
            conditions: Some(RulesetConditions {
                ref_name: Some(RefNameCondition {
                    include: vec!["~DEFAULT_BRANCH".to_owned()],
                    exclude: Vec::new(),
                }),
            }),
            bypass_actors: Vec::new(),
            rules: Vec::new(),
        };

        let body = UpdateRulesetRequest::from_ruleset(&ruleset);
        let json: serde_json::Value = serde_json::to_value(&body).unwrap();
        let ref_name = &json["conditions"]["ref_name"];

        assert_eq!(ref_name["include"], serde_json::json!(["~DEFAULT_BRANCH"]));
        assert_eq!(
            ref_name["exclude"],
            serde_json::json!([]),
            "GitHub's update-ruleset endpoint rejects requests with `exclude` omitted",
        );
    }

    #[test]
    fn serializes_repository_update_without_unset_fields() {
        let update = RepositoryUpdate {
            allow_auto_merge: Some(true),
            allow_merge_commit: Some(false),
            ..RepositoryUpdate::default()
        };

        assert_eq!(
            serde_json::to_string(&update).unwrap(),
            r#"{"allow_auto_merge":true,"allow_merge_commit":false}"#
        );
    }

    #[test]
    fn serializes_create_git_reference_payload() {
        let create = CreateGitReference {
            reference: "refs/heads/topic".to_owned(),
            sha: "abc123".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&create).unwrap(),
            r#"{"ref":"refs/heads/topic","sha":"abc123"}"#
        );
    }

    #[test]
    fn serializes_update_repository_file_payload() {
        let update = UpdateRepositoryFile {
            message: "Pin actions".to_owned(),
            content: "Y29udGVudA==".to_owned(),
            sha: Some("abc123".to_owned()),
            branch: "topic".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&update).unwrap(),
            r#"{"message":"Pin actions","content":"Y29udGVudA==","sha":"abc123","branch":"topic"}"#
        );
    }

    #[test]
    fn serializes_create_repository_file_payload_without_sha() {
        let create = UpdateRepositoryFile {
            message: "Add .envrc".to_owned(),
            content: "dXNlIGZsYWtlCg==".to_owned(),
            sha: None,
            branch: "topic".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&create).unwrap(),
            r#"{"message":"Add .envrc","content":"dXNlIGZsYWtlCg==","branch":"topic"}"#
        );
    }

    #[test]
    fn serializes_create_pull_request_payload() {
        let create = CreatePullRequest {
            title: "Pin actions".to_owned(),
            head: "topic".to_owned(),
            base: "main".to_owned(),
            body: "Generated by github-infra.".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&create).unwrap(),
            r#"{"title":"Pin actions","head":"topic","base":"main","body":"Generated by github-infra."}"#
        );
    }

    #[test]
    fn deserializes_git_tree_payload() {
        let tree: GitTree = serde_json::from_str(
            r#"
{
  "sha": "abc123",
  "truncated": false,
  "tree": [
    {
      "path": ".github/workflows/ci.yml",
      "mode": "100644",
      "type": "blob",
      "sha": "def456",
      "size": 123
    }
  ]
}
"#,
        )
        .unwrap();

        assert_eq!(tree.tree.len(), 1);
        assert_eq!(tree.tree[0].kind, GitTreeEntryType::Blob);
    }

    #[test]
    fn deserializes_file_contents_payload() {
        let file: RepositoryFileContent = serde_json::from_str(
            r#"
{
  "name": "ci.yml",
  "path": ".github/workflows/ci.yml",
  "sha": "abc123",
  "type": "file",
  "encoding": "base64",
  "content": "Y2FyZ28gdGVzdAo=",
  "size": 11
}
"#,
        )
        .unwrap();

        assert_eq!(file.kind, RepositoryContentType::File);
        assert_eq!(file.encoding, ContentEncoding::Base64);
    }

    #[test]
    fn deserializes_directory_contents_payload() {
        let contents: RepositoryContents = serde_json::from_str(
            r#"
[
  {
    "name": "workflows",
    "path": ".github/workflows",
    "sha": "def456",
    "type": "dir"
  }
]
"#,
        )
        .unwrap();

        match contents {
            RepositoryContents::Directory(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].kind, RepositoryContentType::Dir);
            }
            RepositoryContents::File(_) => panic!("expected directory contents"),
        }
    }

    /// Finding 2: `--fix` fetches a ruleset, edits it, and PUTs the whole thing back
    /// (`PUT /repos/{o}/{r}/rulesets/{id}` is a full replacement). Any rule parameter
    /// the model doesn't understand must survive that GET-modify-PUT, or the write
    /// silently resets configuration GitHub still enforces — and for a rule type whose
    /// parameters are *required* (e.g. `commit_message_pattern`), re-sending `{}` 422s
    /// the entire update, permanently blocking remediation on that ruleset.
    ///
    /// The write body is `UpdateRulesetRequest::from_ruleset`; this asserts it carries
    /// through both an unmodeled field on a *known* rule type and every parameter of an
    /// *unknown* rule type.
    #[test]
    fn write_body_preserves_unmodeled_and_unknown_rule_parameters() {
        // A GET /repos/{o}/{r}/rulesets/{id} response as GitHub returns it.
        let response = serde_json::json!({
            "id": 42,
            "name": "main protection",
            "target": "branch",
            "enforcement": "active",
            "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
            "bypass_actors": [],
            "rules": [
                {
                    "type": "pull_request",
                    "parameters": {
                        "required_approving_review_count": 1,
                        // Unmodeled today; resetting it to GitHub's default would silently
                        // change a review policy the org configured.
                        "automatic_copilot_code_review_enabled": true
                    }
                },
                {
                    // A rule type the model doesn't know. Its parameters are *required*:
                    // re-sending `{}` 422s the whole ruleset update.
                    "type": "commit_message_pattern",
                    "parameters": {
                        "operator": "starts_with",
                        "pattern": "PROJ-"
                    }
                }
            ]
        });

        let ruleset: Ruleset =
            serde_json::from_value(response).expect("ruleset response must deserialize");
        let body = serde_json::to_value(UpdateRulesetRequest::from_ruleset(&ruleset))
            .expect("write body must serialize");

        let rules = body["rules"]
            .as_array()
            .expect("write body has a rules array");
        let params_of = |rule_type: &str| -> serde_json::Value {
            rules
                .iter()
                .find(|rule| rule["type"] == rule_type)
                .unwrap_or_else(|| panic!("write body dropped the `{rule_type}` rule"))
                .get("parameters")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        };

        let pull_request = params_of("pull_request");
        assert_eq!(
            pull_request["required_approving_review_count"],
            serde_json::json!(1),
            "modeled parameter must round-trip",
        );
        assert_eq!(
            pull_request["automatic_copilot_code_review_enabled"],
            serde_json::json!(true),
            "unmodeled parameter on a known rule type must survive GET-modify-PUT",
        );

        assert_eq!(
            params_of("commit_message_pattern"),
            serde_json::json!({ "operator": "starts_with", "pattern": "PROJ-" }),
            "an unknown rule type's required parameters must survive verbatim, or the \
             PUT 422s / silently resets them",
        );
    }
}
