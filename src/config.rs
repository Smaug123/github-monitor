use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::RepoRef;

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

impl Config {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let raw = fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn repo_refs(&self) -> Vec<RepoRef> {
        self.repos.iter().map(RepoConfig::repo_ref).collect()
    }
}

impl RepoConfig {
    pub fn repo_ref(&self) -> RepoRef {
        RepoRef::new(self.owner.clone(), self.name.clone())
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to read config {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse config {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
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

    #[test]
    fn repo_config_produces_repo_ref() {
        let repo = RepoConfig {
            owner: "example-org".to_owned(),
            name: "example-repo".to_owned(),
            disabled_rules: None,
        };

        assert_eq!(repo.repo_ref(), RepoRef::new("example-org", "example-repo"));
    }
}
