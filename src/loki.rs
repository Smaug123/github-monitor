use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, SystemTimeError, UNIX_EPOCH};

use serde::Serialize;
use ureq::Agent;

use crate::report::RepoReport;
use crate::rules::{RuleOutput, RuleResult};

pub const DEFAULT_JOB_LABEL: &str = "github-infra";
const PUSH_PATH: &str = "/loki/api/v1/push";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LokiPayload {
    pub streams: Vec<LokiStream>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LokiStream {
    pub stream: BTreeMap<String, String>,
    pub values: Vec<LokiValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LokiValue(pub String, pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct RuleLogLine {
    rule_id: String,
    name: String,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

pub fn build_payload(reports: &[RepoReport], job_label: &str, now_ns: u128) -> LokiPayload {
    let mut streams = Vec::with_capacity(reports.len());
    let mut next_ns = now_ns;

    for report in reports {
        if report.rules.is_empty() {
            continue;
        }

        let mut labels = BTreeMap::new();
        labels.insert("job".to_owned(), job_label.to_owned());
        labels.insert("repo".to_owned(), report.repo.to_string());

        let mut values = Vec::with_capacity(report.rules.len());
        for rule in &report.rules {
            let line = serde_json::to_string(&rule_log_line(rule))
                .expect("RuleLogLine serialization is infallible");
            values.push(LokiValue(next_ns.to_string(), line));
            next_ns += 1;
        }

        streams.push(LokiStream {
            stream: labels,
            values,
        });
    }

    LokiPayload { streams }
}

pub fn render(payload: &LokiPayload) -> String {
    let mut out = serde_json::to_string(payload).expect("LokiPayload serialization is infallible");
    out.push('\n');
    out
}

pub fn current_time_ns() -> Result<u128, SystemTimeError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
}

fn rule_log_line(rule: &RuleOutput) -> RuleLogLine {
    let (status, reason) = match &rule.result {
        RuleResult::Pass => ("pass", None),
        RuleResult::Fail { reason } => ("fail", Some(reason.clone())),
        RuleResult::Skip { reason } => ("skip", Some(reason.clone())),
        RuleResult::Error { reason } => ("error", Some(reason.clone())),
    };

    RuleLogLine {
        rule_id: rule.id.to_string(),
        name: rule.name.clone(),
        status,
        reason,
    }
}

pub fn build_push_agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into()
}

pub fn push(agent: &Agent, base_url: &str, payload: &LokiPayload) -> Result<(), LokiPushError> {
    let url = format!("{}{}", base_url.trim_end_matches('/'), PUSH_PATH);
    let body = serde_json::to_string(payload).expect("LokiPayload serialization is infallible");

    let response = agent
        .post(&url)
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|source| LokiPushError::Transport {
            url: url.clone(),
            source: Box::new(source),
        })?;

    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(LokiPushError::Status { url, status })
    }
}

#[derive(Debug)]
pub enum LokiPushError {
    Transport {
        url: String,
        source: Box<ureq::Error>,
    },
    Status {
        url: String,
        status: u16,
    },
}

impl std::fmt::Display for LokiPushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport { url, source } => {
                write!(f, "failed to push to Loki at {url}: {source}")
            }
            Self::Status { url, status } => {
                write!(f, "Loki at {url} returned non-success status {status}")
            }
        }
    }
}

impl std::error::Error for LokiPushError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport { source, .. } => Some(source.as_ref()),
            Self::Status { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remediation::RepoFix;
    use crate::types::{RepoRef, RuleId};
    use proptest::prelude::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn pass_rule(id: &str, name: &str) -> RuleOutput {
        RuleOutput {
            id: RuleId::new(id),
            name: name.to_owned(),
            result: RuleResult::Pass,
        }
    }

    fn fail_rule(id: &str, name: &str, reason: &str) -> RuleOutput {
        RuleOutput {
            id: RuleId::new(id),
            name: name.to_owned(),
            result: RuleResult::Fail {
                reason: reason.to_owned(),
            },
        }
    }

    fn sample_reports() -> Vec<RepoReport> {
        vec![
            RepoReport::new(
                RepoRef::new("example-org", "good-repo"),
                vec![
                    pass_rule("RS001", "Rulesets exist"),
                    fail_rule("WF003", "No checkout in PR target", "unsafe.yml"),
                ],
                Vec::<RepoFix>::new(),
            ),
            RepoReport::new(
                RepoRef::new("example-org", "bad-repo"),
                vec![fail_rule("ST001", "Auto-merge", "disabled")],
                Vec::new(),
            ),
        ]
    }

    #[test]
    fn build_payload_emits_one_stream_per_repo_with_low_cardinality_labels() {
        let payload = build_payload(&sample_reports(), "github-infra", 1_700_000_000_000_000_000);

        assert_eq!(payload.streams.len(), 2);
        for stream in &payload.streams {
            let keys: Vec<&str> = stream.stream.keys().map(String::as_str).collect();
            assert_eq!(keys, vec!["job", "repo"]);
            assert_eq!(
                stream.stream.get("job").map(String::as_str),
                Some("github-infra")
            );
        }
        assert_eq!(
            payload.streams[0].stream.get("repo").map(String::as_str),
            Some("example-org/good-repo")
        );
    }

    #[test]
    fn build_payload_skips_repos_with_no_rules() {
        let reports = vec![RepoReport::new(
            RepoRef::new("example-org", "empty"),
            Vec::new(),
            Vec::new(),
        )];
        let payload = build_payload(&reports, "github-infra", 0);
        assert!(payload.streams.is_empty());
    }

    #[test]
    fn build_payload_uses_strictly_monotonic_timestamps_within_each_stream() {
        let payload = build_payload(&sample_reports(), "github-infra", 1_000);

        for stream in &payload.streams {
            let timestamps: Vec<u128> = stream
                .values
                .iter()
                .map(|LokiValue(ts, _)| ts.parse::<u128>().unwrap())
                .collect();
            for pair in timestamps.windows(2) {
                assert!(pair[0] < pair[1], "timestamps must be strictly increasing");
            }
        }
    }

    #[test]
    fn pass_result_has_no_reason_field() {
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let pass_line = &payload.streams[0].values[0].1;
        assert!(!pass_line.contains("reason"), "got line {pass_line}");
        assert!(pass_line.contains("\"status\":\"pass\""));
    }

    #[test]
    fn fail_result_includes_reason_field() {
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let fail_line = &payload.streams[0].values[1].1;
        assert!(fail_line.contains("\"status\":\"fail\""));
        assert!(fail_line.contains("\"reason\":\"unsafe.yml\""));
    }

    #[test]
    fn render_appends_trailing_newline() {
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let rendered = render(&payload);
        assert!(rendered.ends_with('\n'));
    }

    fn rule_output_strategy() -> impl Strategy<Value = RuleOutput> {
        let id = "[A-Z]{2}[0-9]{3}".prop_map(RuleId::new);
        let name = "[a-z]{1,30}";
        let result = prop_oneof![
            Just(RuleResult::Pass),
            "[a-z ]{1,30}".prop_map(|r| RuleResult::Fail { reason: r }),
            "[a-z ]{1,30}".prop_map(|r| RuleResult::Skip { reason: r }),
            "[a-z ]{1,30}".prop_map(|r| RuleResult::Error { reason: r }),
        ];
        (id, name, result).prop_map(|(id, name, result)| RuleOutput { id, name, result })
    }

    fn repo_report_strategy() -> impl Strategy<Value = RepoReport> {
        let owner = "[a-z][a-z0-9-]{0,15}";
        let name = "[a-z][a-z0-9-]{0,15}";
        let rules = proptest::collection::vec(rule_output_strategy(), 0..6);
        (owner, name, rules).prop_map(|(owner, name, rules)| {
            RepoReport::new(RepoRef::new(owner, name), rules, Vec::new())
        })
    }

    proptest! {
        #[test]
        fn total_values_equals_total_rule_outputs(
            reports in proptest::collection::vec(repo_report_strategy(), 0..5),
            now in any::<u64>(),
        ) {
            let total_rules: usize = reports.iter().map(|r| r.rules.len()).sum();
            let payload = build_payload(&reports, "github-infra", u128::from(now));
            let total_values: usize = payload.streams.iter().map(|s| s.values.len()).sum();
            prop_assert_eq!(total_rules, total_values);
        }

        #[test]
        fn every_stream_is_nonempty(
            reports in proptest::collection::vec(repo_report_strategy(), 0..5),
        ) {
            let payload = build_payload(&reports, "github-infra", 0);
            for stream in &payload.streams {
                prop_assert!(!stream.values.is_empty());
            }
        }

        #[test]
        fn timestamps_are_globally_unique(
            reports in proptest::collection::vec(repo_report_strategy(), 0..5),
        ) {
            let payload = build_payload(&reports, "github-infra", 0);
            let mut all_ts = Vec::new();
            for stream in &payload.streams {
                for LokiValue(ts, _) in &stream.values {
                    all_ts.push(ts.clone());
                }
            }
            let unique: std::collections::HashSet<&String> = all_ts.iter().collect();
            prop_assert_eq!(all_ts.len(), unique.len());
        }
    }

    struct RecordedHttp {
        method: String,
        path: String,
        body: String,
    }

    fn spawn_mock_loki(status_code: u16) -> (String, thread::JoinHandle<RecordedHttp>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut header_buf = Vec::new();
            let mut byte = [0_u8; 1];
            while !header_buf.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                header_buf.push(byte[0]);
            }
            let header_text = String::from_utf8(header_buf).unwrap();
            let mut lines = header_text.split("\r\n");
            let request_line = lines.next().unwrap().to_owned();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap().to_owned();
            let path = parts.next().unwrap().to_owned();

            let mut content_length: usize = 0;
            for line in lines {
                let lower = line.to_ascii_lowercase();
                if let Some(rest) = lower.strip_prefix("content-length:") {
                    content_length = rest.trim().parse().unwrap();
                }
            }

            let mut body_buf = vec![0_u8; content_length];
            stream.read_exact(&mut body_buf).unwrap();
            let body = String::from_utf8(body_buf).unwrap();

            let response = format!(
                "HTTP/1.1 {} OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                status_code
            );
            stream.write_all(response.as_bytes()).unwrap();
            let _ = stream.flush();

            RecordedHttp { method, path, body }
        });

        (format!("http://{address}"), handle)
    }

    #[test]
    fn push_posts_payload_to_loki_push_endpoint() {
        let (base_url, handle) = spawn_mock_loki(204);
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let agent = build_push_agent();

        push(&agent, &base_url, &payload).unwrap();

        let recorded = handle.join().unwrap();
        assert_eq!(recorded.method, "POST");
        assert_eq!(recorded.path, "/loki/api/v1/push");
        let parsed: serde_json::Value = serde_json::from_str(&recorded.body).unwrap();
        assert_eq!(parsed["streams"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn push_trims_trailing_slash_from_base_url() {
        let (base_url, handle) = spawn_mock_loki(204);
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let agent = build_push_agent();

        let with_slash = format!("{base_url}/");
        push(&agent, &with_slash, &payload).unwrap();
        let recorded = handle.join().unwrap();
        assert_eq!(recorded.path, "/loki/api/v1/push");
    }

    #[test]
    fn push_reports_status_error_for_non_2xx() {
        let (base_url, handle) = spawn_mock_loki(500);
        let payload = build_payload(&sample_reports(), "github-infra", 0);
        let agent = build_push_agent();

        let err = push(&agent, &base_url, &payload).unwrap_err();
        match err {
            LokiPushError::Status { status, .. } => assert_eq!(status, 500),
            LokiPushError::Transport { .. } => panic!("expected Status error, got Transport"),
        }
        let _ = handle.join();
    }
}
