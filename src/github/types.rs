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
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub allow_auto_merge: bool,
    #[serde(default)]
    pub delete_branch_on_merge: bool,
    #[serde(default)]
    pub allow_update_branch: bool,
    #[serde(default)]
    pub allow_squash_merge: bool,
    #[serde(default)]
    pub allow_merge_commit: bool,
    #[serde(default)]
    pub allow_rebase_merge: bool,
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
    pub sha: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesetConditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_name: Option<RefNameCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefNameCondition {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BypassActor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<u64>,
    pub actor_type: BypassActorType,
    pub bypass_mode: BypassMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesetRule {
    #[serde(rename = "type")]
    pub kind: RulesetRuleType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<RulesetRuleParameters>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RulesetRuleParameters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_status_checks: Vec<RequiredStatusCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_required_status_checks_policy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_approving_review_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_code_owner_review: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_last_push_approval: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_review_thread_resolution: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_stale_reviews_on_push: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub do_not_enforce_on_create: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredStatusCheck {
    pub context: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_id: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;

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
        "require_code_owner_review": true
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
        assert_eq!(ruleset.rules[0].kind, RulesetRuleType::RequiredStatusChecks);
        let conditions = ruleset.conditions.unwrap();
        let ref_name = conditions.ref_name.unwrap();
        assert_eq!(ref_name.include, vec!["~DEFAULT_BRANCH"]);
        assert!(ref_name.exclude.is_empty());
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
            sha: "abc123".to_owned(),
            branch: "topic".to_owned(),
        };

        assert_eq!(
            serde_json::to_string(&update).unwrap(),
            r#"{"message":"Pin actions","content":"Y29udGVudA==","sha":"abc123","branch":"topic"}"#
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
}
