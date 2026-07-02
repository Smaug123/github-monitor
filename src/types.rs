use std::fmt;

use serde::{Deserialize, Serialize};

/// A fact gathered from GitHub. Distinguishes three outcomes that must never be
/// conflated: the value is `Present`; it was queried and verified `Absent`; or it
/// could not be determined (`Unknown` — e.g. the token lacks the permission to
/// read it, so the endpoint 404s). Unlike a bare `Option<T>` — which serde
/// silently reads as `None` for a missing field — a snapshot that omits a
/// `Gathered` field fails to load, so a never-recorded fact can never masquerade
/// as `Absent`. A rule that reads an `Unknown` fact must return `Error`, never a
/// definite verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gathered<T> {
    Present(T),
    Absent,
    Unknown,
}

impl<T> Gathered<T> {
    /// Maps `Some`/`None` to `Present`/`Absent`. Only for endpoints whose `None`
    /// genuinely means "verified absent"; a `None` that means "could not read"
    /// must be mapped to [`Gathered::Unknown`] explicitly.
    pub fn from_option(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::Present(value),
            None => Self::Absent,
        }
    }

    /// The value when `Present`, else `None`. `Absent` and `Unknown` both yield
    /// `None`; a caller that must distinguish them has to match explicitly.
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Present(value) => Some(value),
            Self::Absent | Self::Unknown => None,
        }
    }
}

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
    #[cfg(test)]
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

    pub fn as_str(&self) -> &str {
        &self.0
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
