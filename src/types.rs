use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Owner(String);

impl Owner {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for RepoName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BranchName(String);

impl BranchName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleId(String);

impl RuleId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: Owner,
    pub name: RepoName,
}

impl RepoRef {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: Owner::new(owner),
            name: RepoName::new(name),
        }
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn identifier() -> impl Strategy<Value = String> {
        "[a-zA-Z][a-zA-Z0-9_-]{0,30}"
    }

    proptest! {
        #[test]
        fn repo_ref_display(owner in identifier(), name in identifier()) {
            let repo_ref = RepoRef::new(owner.clone(), name.clone());
            prop_assert_eq!(format!("{repo_ref}"), format!("{owner}/{name}"));
        }

        #[test]
        fn rule_id_display_preserves_value(s in "[A-Z]{2}[0-9]{3}") {
            let id = RuleId::new(s.clone());
            prop_assert_eq!(id.to_string(), s);
        }

        #[test]
        fn owner_display(s in identifier()) {
            let owner = Owner::new(s.clone());
            prop_assert_eq!(owner.to_string(), s);
        }

        #[test]
        fn repo_name_display(s in identifier()) {
            let name = RepoName::new(s.clone());
            prop_assert_eq!(name.to_string(), s);
        }

        #[test]
        fn branch_name_display(s in identifier()) {
            let name = BranchName::new(s.clone());
            prop_assert_eq!(name.to_string(), s);
        }
    }
}
