use serde::{Deserialize, Serialize};

use crate::facts::RepoFacts;

use super::RuleResult;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileCheck {
    FileExists { path: String },
    NixFlakeExists,
    NixFlakeHasCheck,
}

pub(super) fn evaluate(rule: &FileCheck, facts: &RepoFacts) -> RuleResult {
    match rule {
        FileCheck::FileExists { path } => {
            if facts.files_present.contains(path) {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: format!("required file `{path}` is missing"),
                }
            }
        }
        FileCheck::NixFlakeExists => {
            if facts.files_present.contains("flake.nix") {
                RuleResult::Pass
            } else {
                RuleResult::Fail {
                    reason: "required file `flake.nix` is missing".to_owned(),
                }
            }
        }
        FileCheck::NixFlakeHasCheck => {
            if !facts.files_present.contains("flake.nix") {
                RuleResult::Fail {
                    reason: "cannot observe flake checks because `flake.nix` is missing".to_owned(),
                }
            } else if workflows_run_nix_flake_check(facts) {
                RuleResult::Pass
            } else {
                RuleResult::Skip {
                    reason: "RepoFacts does not yet capture flake outputs; only explicit `nix flake check` workflow steps can prove this rule".to_owned(),
                }
            }
        }
    }
}

fn workflows_run_nix_flake_check(facts: &RepoFacts) -> bool {
    facts.workflows.iter().any(|workflow_file| {
        workflow_file
            .workflow
            .jobs
            .values()
            .flat_map(|job| job.steps().iter())
            .filter_map(|step| step.run())
            .any(|run| run.contains("nix flake check"))
    })
}
