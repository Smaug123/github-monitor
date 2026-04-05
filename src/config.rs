use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub repos: Vec<RepoConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoConfig {
    pub owner: String,
    pub name: String,
    pub disabled_rules: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn identifier() -> impl Strategy<Value = String> {
        "[a-zA-Z][a-zA-Z0-9_-]{0,30}"
    }

    fn repo_config_strategy() -> impl Strategy<Value = RepoConfig> {
        (
            identifier(),
            identifier(),
            proptest::option::of(proptest::collection::vec(identifier(), 0..5)),
        )
            .prop_map(|(owner, name, disabled_rules)| RepoConfig {
                owner,
                name,
                disabled_rules,
            })
    }

    fn config_strategy() -> impl Strategy<Value = Config> {
        proptest::collection::vec(repo_config_strategy(), 0..5).prop_map(|repos| Config { repos })
    }

    proptest! {
        #[test]
        fn toml_roundtrip(config in config_strategy()) {
            let serialized = toml::to_string(&config).unwrap();
            let deserialized: Config = toml::from_str(&serialized).unwrap();
            prop_assert_eq!(deserialized, config);
        }
    }
}
