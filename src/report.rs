use serde::{Deserialize, Serialize};

use crate::remediation::{FixStatus, RepoFix};
use crate::rules::{RuleOutput, RuleResult};
use crate::types::RepoRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    pub fn parse(raw: &str) -> Result<Self, OutputFormatError> {
        match raw {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            _ => Err(OutputFormatError {
                value: raw.to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFormatError {
    value: String,
}

impl std::fmt::Display for OutputFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid output format `{}`; expected `text` or `json`",
            self.value
        )
    }
}

impl std::error::Error for OutputFormatError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoReport {
    pub repo: RepoRef,
    pub rules: Vec<RuleOutput>,
    #[serde(default)]
    pub fixes: Vec<RepoFix>,
}

impl RepoReport {
    pub fn new(repo: RepoRef, rules: Vec<RuleOutput>, fixes: Vec<RepoFix>) -> Self {
        Self { repo, rules, fixes }
    }

    pub fn counts(&self) -> RuleCounts {
        rule_counts(&self.rules)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuleCounts {
    pub pass: usize,
    pub fail: usize,
    pub skip: usize,
    pub error: usize,
}

impl RuleCounts {
    fn add_result(&mut self, result: &RuleResult) {
        match result {
            RuleResult::Pass => self.pass += 1,
            RuleResult::Fail { .. } => self.fail += 1,
            RuleResult::Skip { .. } => self.skip += 1,
            RuleResult::Error { .. } => self.error += 1,
        }
    }

    pub fn has_failures(self) -> bool {
        self.fail > 0 || self.error > 0
    }
}

pub fn render(format: OutputFormat, reports: &[RepoReport]) -> Result<String, ReportError> {
    match format {
        OutputFormat::Text => Ok(render_text(reports)),
        OutputFormat::Json => render_json(reports),
    }
}

pub fn overall_counts(reports: &[RepoReport]) -> RuleCounts {
    let mut counts = RuleCounts::default();
    for report in reports {
        for rule in &report.rules {
            counts.add_result(&rule.result);
        }
    }
    counts
}

pub fn has_failures(reports: &[RepoReport]) -> bool {
    overall_counts(reports).has_failures()
}

fn rule_counts(rules: &[RuleOutput]) -> RuleCounts {
    let mut counts = RuleCounts::default();
    for rule in rules {
        counts.add_result(&rule.result);
    }
    counts
}

fn render_text(reports: &[RepoReport]) -> String {
    let mut lines = Vec::new();

    for (index, report) in reports.iter().enumerate() {
        if index > 0 {
            lines.push(String::new());
        }

        lines.push(format!("Repository: {}", report.repo));
        lines.push(format!("{:<6}  {:<5}  {}", "STATUS", "RULE", "NAME"));

        for rule in &report.rules {
            lines.push(format!(
                "{:<6}  {:<5}  {}",
                result_label(&rule.result),
                rule.id,
                rule.name
            ));

            if let Some(reason) = result_reason(&rule.result) {
                lines.push(format!("        reason: {reason}"));
            }
        }

        if !report.fixes.is_empty() {
            lines.push("Fixes:".to_owned());
            lines.push(format!("{:<8}  {:<5}  {}", "STATUS", "RULE", "DESCRIPTION"));

            for fix in &report.fixes {
                lines.push(format!(
                    "{:<8}  {:<5}  {}",
                    fix_status_label(&fix.status),
                    fix.rule_id,
                    fix.description
                ));

                if let Some(reason) = fix_status_reason(&fix.status) {
                    lines.push(format!("          reason: {reason}"));
                }
            }
        }

        let counts = report.counts();
        lines.push(format!(
            "Summary: {} pass, {} fail, {} skip, {} error",
            counts.pass, counts.fail, counts.skip, counts.error
        ));
    }

    let overall = overall_counts(reports);
    if !reports.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!(
        "Overall: {} pass, {} fail, {} skip, {} error",
        overall.pass, overall.fail, overall.skip, overall.error
    ));

    format!("{}\n", lines.join("\n"))
}

fn render_json(reports: &[RepoReport]) -> Result<String, ReportError> {
    let mut output = serde_json::to_string_pretty(reports)
        .map_err(|source| ReportError::JsonSerialize { source })?;
    output.push('\n');
    Ok(output)
}

fn result_label(result: &RuleResult) -> &'static str {
    match result {
        RuleResult::Pass => "PASS",
        RuleResult::Fail { .. } => "FAIL",
        RuleResult::Skip { .. } => "SKIP",
        RuleResult::Error { .. } => "ERROR",
    }
}

fn result_reason(result: &RuleResult) -> Option<&str> {
    match result {
        RuleResult::Pass => None,
        RuleResult::Fail { reason }
        | RuleResult::Skip { reason }
        | RuleResult::Error { reason } => Some(reason),
    }
}

fn fix_status_label(status: &FixStatus) -> &'static str {
    match status {
        FixStatus::Planned => "PLANNED",
        FixStatus::Rejected { .. } => "REJECTED",
    }
}

fn fix_status_reason(status: &FixStatus) -> Option<&str> {
    match status {
        FixStatus::Planned => None,
        FixStatus::Rejected { reason } => Some(reason),
    }
}

#[derive(Debug)]
pub enum ReportError {
    JsonSerialize { source: serde_json::Error },
}

impl std::fmt::Display for ReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JsonSerialize { source } => {
                write!(f, "failed to serialize JSON report: {source}")
            }
        }
    }
}

impl std::error::Error for ReportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::JsonSerialize { source } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remediation::{FixStatus, RepoFix};
    use crate::rules::{RuleOutput, RuleResult};
    use crate::types::{RepoRef, RuleId};

    fn sample_reports() -> Vec<RepoReport> {
        vec![
            RepoReport::new(
                RepoRef::new("example-org", "good-repo"),
                vec![
                    RuleOutput {
                        id: RuleId::new("RS001"),
                        name: "Rulesets exist".to_owned(),
                        result: RuleResult::Pass,
                    },
                    RuleOutput {
                        id: RuleId::new("NX002"),
                        name: "The flake has observable check coverage".to_owned(),
                        result: RuleResult::Skip {
                            reason: "flake outputs are not yet captured".to_owned(),
                        },
                    },
                ],
                vec![
                    RepoFix {
                        rule_id: RuleId::new("ST001"),
                        rule_name: "Auto-merge is enabled".to_owned(),
                        description: "set repository setting `allow_auto_merge` to true".to_owned(),
                        status: FixStatus::Planned,
                    },
                    RepoFix {
                        rule_id: RuleId::new("RS001"),
                        rule_name: "Rulesets exist".to_owned(),
                        description: "automatic fix unavailable".to_owned(),
                        status: FixStatus::Rejected {
                            reason: "automatic fixes for this rule are not implemented yet"
                                .to_owned(),
                        },
                    },
                ],
            ),
            RepoReport::new(
                RepoRef::new("example-org", "bad-repo"),
                vec![RuleOutput {
                    id: RuleId::new("WF003"),
                    name: "No pull_request_target workflow checks out code".to_owned(),
                    result: RuleResult::Fail {
                        reason: "unsafe.yml uses actions/checkout".to_owned(),
                    },
                }],
                vec![RepoFix {
                    rule_id: RuleId::new("ST002"),
                    rule_name: "Delete branch on merge is enabled".to_owned(),
                    description: "set repository setting `delete_branch_on_merge` to true"
                        .to_owned(),
                    status: FixStatus::Planned,
                }],
            ),
        ]
    }

    #[test]
    fn parses_output_formats() {
        assert_eq!(OutputFormat::parse("text").unwrap(), OutputFormat::Text);
        assert_eq!(OutputFormat::parse("json").unwrap(), OutputFormat::Json);
        assert_eq!(
            OutputFormat::parse("yaml").unwrap_err().to_string(),
            "invalid output format `yaml`; expected `text` or `json`"
        );
    }

    #[test]
    fn json_report_roundtrips() {
        let reports = sample_reports();
        let rendered = render(OutputFormat::Json, &reports).unwrap();
        let decoded: Vec<RepoReport> = serde_json::from_str(&rendered).unwrap();

        assert_eq!(decoded, reports);
    }

    #[test]
    fn text_report_lists_repos_rules_statuses_and_reasons() {
        let rendered = render(OutputFormat::Text, &sample_reports()).unwrap();

        assert!(rendered.contains("Repository: example-org/good-repo"));
        assert!(rendered.contains("PASS    RS001  Rulesets exist"));
        assert!(rendered.contains("SKIP    NX002  The flake has observable check coverage"));
        assert!(rendered.contains("reason: flake outputs are not yet captured"));
        assert!(rendered.contains("Fixes:"));
        assert!(
            rendered.contains("PLANNED   ST001  set repository setting `allow_auto_merge` to true")
        );
        assert!(rendered.contains("REJECTED  RS001  automatic fix unavailable"));
        assert!(rendered.contains("reason: automatic fixes for this rule are not implemented yet"));
        assert!(
            rendered.contains("FAIL    WF003  No pull_request_target workflow checks out code")
        );
        assert!(
            rendered.contains(
                "PLANNED   ST002  set repository setting `delete_branch_on_merge` to true"
            )
        );
        assert!(rendered.contains("Overall: 1 pass, 1 fail, 1 skip, 0 error"));
    }

    #[test]
    fn skip_does_not_count_as_failure() {
        let reports = vec![RepoReport::new(
            RepoRef::new("example-org", "good-repo"),
            vec![RuleOutput {
                id: RuleId::new("NX002"),
                name: "The flake has observable check coverage".to_owned(),
                result: RuleResult::Skip {
                    reason: "flake outputs are not yet captured".to_owned(),
                },
            }],
            Vec::new(),
        )];

        assert!(!has_failures(&reports));
    }
}
