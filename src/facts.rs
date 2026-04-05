use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::types::{BranchName, RepoRef};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoSettings;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoFacts {
    pub repo: RepoRef,
    pub settings: RepoSettings,
    pub rulesets: Vec<Ruleset>,
    pub default_branch: BranchName,
    pub workflows: Vec<(String, ())>,
    pub files_present: HashSet<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn identifier() -> impl Strategy<Value = String> {
        "[a-zA-Z][a-zA-Z0-9_-]{0,30}"
    }

    fn repo_facts_strategy() -> impl Strategy<Value = RepoFacts> {
        (
            identifier(),
            identifier(),
            proptest::collection::vec(identifier(), 0..5),
            identifier(),
            proptest::collection::hash_set(identifier(), 0..10),
        )
            .prop_map(
                |(owner, name, workflow_names, branch, files_present)| RepoFacts {
                    repo: RepoRef::new(owner, name),
                    settings: RepoSettings,
                    rulesets: Vec::new(),
                    default_branch: BranchName::new(branch),
                    workflows: workflow_names.into_iter().map(|n| (n, ())).collect(),
                    files_present,
                },
            )
    }

    proptest! {
        #[test]
        fn repo_facts_json_roundtrip(facts in repo_facts_strategy()) {
            let json = serde_json::to_string(&facts).unwrap();
            let deserialized: RepoFacts = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(deserialized, facts);
        }
    }
}
