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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ruleset {
    pub id: u64,
    pub name: String,
    pub target: RulesetTarget,
    pub enforcement: RulesetEnforcement,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bypass_actors: Vec<BypassActor>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<RulesetRule>,
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
