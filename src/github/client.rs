use std::fmt;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use ureq::http::Response;
use ureq::http::header::HeaderMap;
use ureq::{Agent, Body, Error as UreqError};

use crate::github::types::{
    GitTree, Repository, RepositoryContents, RepositoryDirectoryEntry, RepositoryFileContent,
    Ruleset,
};
use crate::types::RepoRef;

const GITHUB_API_BASE_URL: &str = "https://api.github.com";
const GITHUB_API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const RATE_LIMIT_BUFFER: u32 = 5;
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_BACKOFF_MS: u64 = 250;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPath {
    segments: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonRootRepoPath(RepoPath);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoPathError {
    EmptySegment,
    RootPathNotAllowed,
}

impl RepoPath {
    pub fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    pub fn new(path: impl AsRef<str>) -> Result<Self, RepoPathError> {
        let trimmed = path.as_ref().trim_matches('/');
        if trimmed.is_empty() {
            return Ok(Self::root());
        }

        let segments = trimmed.split('/').map(str::to_owned).collect::<Vec<_>>();

        if segments.iter().any(|segment| segment.is_empty()) {
            return Err(RepoPathError::EmptySegment);
        }

        Ok(Self { segments })
    }

    fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    fn to_api_path(&self) -> String {
        self.segments
            .iter()
            .map(|segment| percent_encode_path_segment(segment))
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl NonRootRepoPath {
    pub fn new(path: impl AsRef<str>) -> Result<Self, RepoPathError> {
        let path = RepoPath::new(path)?;
        if path.is_root() {
            return Err(RepoPathError::RootPathNotAllowed);
        }

        Ok(Self(path))
    }

    fn as_repo_path(&self) -> &RepoPath {
        &self.0
    }
}

impl fmt::Display for RepoPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySegment => f.write_str("repository paths must not contain empty segments"),
            Self::RootPathNotAllowed => {
                f.write_str("this operation requires a non-root repository path")
            }
        }
    }
}

impl std::error::Error for RepoPathError {}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    token: GitHubToken,
    rate_limit: RateLimitState,
    agent: Agent,
}

#[derive(Clone, PartialEq, Eq)]
pub struct GitHubToken(String);

impl GitHubToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn from_env(var_name: &str) -> Option<Self> {
        std::env::var(var_name).ok().map(Self)
    }

    fn as_bearer_header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for GitHubToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitHubToken([REDACTED])")
    }
}

#[allow(dead_code)]
impl GitHubClient {
    pub fn new(token: GitHubToken) -> Self {
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();

        Self {
            token,
            rate_limit: RateLimitState::default(),
            agent,
        }
    }

    pub fn get_repo(&mut self, repo: &RepoRef) -> Result<Repository, GitHubClientError> {
        self.get_json(&format!("{GITHUB_API_BASE_URL}/repos/{repo}"))
    }

    pub fn list_rulesets(&mut self, repo: &RepoRef) -> Result<Vec<Ruleset>, GitHubClientError> {
        self.get_paginated_json(&format!("{GITHUB_API_BASE_URL}/repos/{repo}/rulesets"))
    }

    pub fn get_ruleset(
        &mut self,
        repo: &RepoRef,
        ruleset_id: u64,
    ) -> Result<Ruleset, GitHubClientError> {
        self.get_json(&format!(
            "{GITHUB_API_BASE_URL}/repos/{repo}/rulesets/{ruleset_id}"
        ))
    }

    pub fn get_file_contents(
        &mut self,
        repo: &RepoRef,
        path: &NonRootRepoPath,
    ) -> Result<RepositoryFileContent, GitHubClientError> {
        let url = contents_url(repo, path.as_repo_path());
        match self.get_json::<RepositoryContents>(&url)? {
            RepositoryContents::File(file) => Ok(file),
            RepositoryContents::Directory(_) => Err(GitHubClientError::UnexpectedContentsShape {
                url,
                expected: "file",
            }),
        }
    }

    pub fn list_directory_contents(
        &mut self,
        repo: &RepoRef,
        path: &RepoPath,
    ) -> Result<Vec<RepositoryDirectoryEntry>, GitHubClientError> {
        let url = contents_url(repo, path);
        match self.get_json::<RepositoryContents>(&url)? {
            RepositoryContents::Directory(entries) => Ok(entries),
            RepositoryContents::File(_) => Err(GitHubClientError::UnexpectedContentsShape {
                url,
                expected: "directory",
            }),
        }
    }

    pub fn get_git_tree(
        &mut self,
        repo: &RepoRef,
        sha: &str,
    ) -> Result<GitTree, GitHubClientError> {
        self.get_json(&format!(
            "{GITHUB_API_BASE_URL}/repos/{repo}/git/trees/{sha}?recursive=1"
        ))
    }

    fn get_json<T>(&mut self, url: &str) -> Result<T, GitHubClientError>
    where
        T: DeserializeOwned,
    {
        let mut response = self.send_get(url)?;
        response
            .body_mut()
            .read_json()
            .map_err(|source| GitHubClientError::Request {
                url: url.to_owned(),
                source,
            })
    }

    fn get_paginated_json<T>(&mut self, first_page_url: &str) -> Result<Vec<T>, GitHubClientError>
    where
        T: DeserializeOwned,
    {
        let mut values = Vec::new();
        let mut next_page = Some(first_page_url.to_owned());

        while let Some(page_url) = next_page {
            let mut response = self.send_get(&page_url)?;
            next_page = response
                .headers()
                .get("link")
                .and_then(|value| value.to_str().ok())
                .and_then(parse_next_link);

            let mut page_values: Vec<T> =
                response
                    .body_mut()
                    .read_json()
                    .map_err(|source| GitHubClientError::Request {
                        url: page_url.clone(),
                        source,
                    })?;
            values.append(&mut page_values);
        }

        Ok(values)
    }

    fn send_get(&mut self, url: &str) -> Result<Response<Body>, GitHubClientError> {
        let mut retries = 0;

        loop {
            self.sleep_for_rate_limit_if_needed();

            match self.call_once(url) {
                Ok(mut response) => {
                    self.update_rate_limit(response.headers());

                    if !response.status().is_success() {
                        if let Some(delay) =
                            retry_delay(retries, RetryTrigger::Status(response.status().as_u16()))
                        {
                            retries += 1;
                            thread::sleep(delay);
                            continue;
                        }

                        let body = response.body_mut().read_to_string().unwrap_or_default();
                        return Err(GitHubClientError::UnexpectedStatus {
                            url: url.to_owned(),
                            status: response.status().as_u16(),
                            body,
                        });
                    }

                    return Ok(response);
                }
                Err(source) => {
                    if is_retryable_transport_error(&source)
                        && let Some(delay) = retry_delay(retries, RetryTrigger::Network)
                    {
                        retries += 1;
                        thread::sleep(delay);
                        continue;
                    }

                    return Err(GitHubClientError::Request {
                        url: url.to_owned(),
                        source,
                    });
                }
            }
        }
    }

    fn call_once(&self, url: &str) -> Result<Response<Body>, UreqError> {
        self.agent
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", self.token.as_bearer_header())
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header("User-Agent", USER_AGENT)
            .call()
    }

    fn sleep_for_rate_limit_if_needed(&self) {
        if let Some(delay) = self.rate_limit.sleep_duration() {
            thread::sleep(delay);
        }
    }

    fn update_rate_limit(&mut self, headers: &HeaderMap) {
        let remaining = headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok());
        let reset = headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok());

        if let Some(parsed) = parse_rate_limit_headers(remaining, reset) {
            self.rate_limit.record(parsed);
        }
    }
}

#[derive(Debug)]
pub enum GitHubClientError {
    Request {
        url: String,
        source: UreqError,
    },
    UnexpectedStatus {
        url: String,
        status: u16,
        body: String,
    },
    UnexpectedContentsShape {
        url: String,
        expected: &'static str,
    },
}

impl fmt::Display for GitHubClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Request { url, source } => write!(f, "request to {url} failed: {source}"),
            Self::UnexpectedStatus { url, status, body } => {
                if body.is_empty() {
                    write!(f, "request to {url} returned unexpected status {status}")
                } else {
                    write!(
                        f,
                        "request to {url} returned unexpected status {status}: {body}"
                    )
                }
            }
            Self::UnexpectedContentsShape { url, expected } => {
                write!(
                    f,
                    "request to {url} returned contents that were not a {expected}"
                )
            }
        }
    }
}

impl std::error::Error for GitHubClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Request { source, .. } => Some(source),
            Self::UnexpectedStatus { .. } | Self::UnexpectedContentsShape { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct RateLimitState {
    remaining: Option<u32>,
    reset_epoch_seconds: Option<u64>,
}

impl RateLimitState {
    fn record(&mut self, snapshot: RateLimitSnapshot) {
        self.remaining = Some(snapshot.remaining);
        self.reset_epoch_seconds = Some(snapshot.reset_epoch_seconds);
    }

    fn sleep_duration(&self) -> Option<Duration> {
        let remaining = self.remaining?;
        if remaining > RATE_LIMIT_BUFFER {
            return None;
        }

        let reset_epoch_seconds = self.reset_epoch_seconds?;
        let now = unix_time_now();

        if reset_epoch_seconds <= now {
            return None;
        }

        Some(Duration::from_secs(
            reset_epoch_seconds.saturating_sub(now).saturating_add(1),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RateLimitSnapshot {
    remaining: u32,
    reset_epoch_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryTrigger {
    Network,
    Status(u16),
}

fn parse_rate_limit_headers(
    remaining: Option<&str>,
    reset: Option<&str>,
) -> Option<RateLimitSnapshot> {
    let remaining = remaining?.trim().parse().ok()?;
    let reset_epoch_seconds = reset?.trim().parse().ok()?;

    Some(RateLimitSnapshot {
        remaining,
        reset_epoch_seconds,
    })
}

fn parse_next_link(link_header: &str) -> Option<String> {
    for part in link_header.split(',') {
        let mut segments = part.trim().split(';');
        let url = segments
            .next()
            .map(str::trim)
            .and_then(|value| value.strip_prefix('<'))
            .and_then(|value| value.strip_suffix('>'))?;

        let mut is_next = false;

        for segment in segments {
            let (name, value) = match segment.trim().split_once('=') {
                Some(pair) => pair,
                None => continue,
            };

            if name.trim() != "rel" {
                continue;
            }

            let relations = value.trim().trim_matches('"');
            is_next = relations
                .split_whitespace()
                .any(|relation| relation == "next");
        }

        if is_next {
            return Some(url.to_owned());
        }
    }

    None
}

fn retry_delay(retries_so_far: u32, trigger: RetryTrigger) -> Option<Duration> {
    if retries_so_far >= MAX_RETRIES {
        return None;
    }

    let should_retry = match trigger {
        RetryTrigger::Network => true,
        RetryTrigger::Status(status) => (500..=599).contains(&status),
    };

    if !should_retry {
        return None;
    }

    Some(Duration::from_millis(
        INITIAL_RETRY_BACKOFF_MS * (1_u64 << retries_so_far),
    ))
}

fn is_retryable_transport_error(error: &UreqError) -> bool {
    matches!(
        error,
        UreqError::Io(_)
            | UreqError::Timeout(_)
            | UreqError::HostNotFound
            | UreqError::ConnectionFailed
            | UreqError::Protocol(_)
    )
}

fn unix_time_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn contents_url(repo: &RepoRef, path: &RepoPath) -> String {
    let base = format!("{GITHUB_API_BASE_URL}/repos/{repo}/contents");
    if path.is_root() {
        base
    } else {
        format!("{base}/{}", path.to_api_path())
    }
}

fn percent_encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());

    for byte in segment.bytes() {
        if is_path_segment_byte_unreserved(byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }

    encoded
}

fn is_path_segment_byte_unreserved(byte: u8) -> bool {
    matches!(
        byte,
        b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@'
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RepoRef;
    use proptest::prelude::*;

    fn url_path() -> impl Strategy<Value = String> {
        "[A-Za-z0-9._~/-]{1,40}"
    }

    proptest! {
        #[test]
        fn parses_next_link_for_generated_urls(path in url_path()) {
            let url = format!("https://example.com/{path}");
            let header = format!("<{url}>; rel=\"next\"");

            prop_assert_eq!(parse_next_link(&header), Some(url));
        }

        #[test]
        fn parses_rate_limit_headers_for_generated_values(remaining in any::<u32>(), reset in any::<u64>()) {
            let parsed = parse_rate_limit_headers(
                Some(&remaining.to_string()),
                Some(&reset.to_string()),
            );

            prop_assert_eq!(
                parsed,
                Some(RateLimitSnapshot {
                    remaining,
                    reset_epoch_seconds: reset,
                })
            );
        }
    }

    #[test]
    fn ignores_link_header_without_next_relation() {
        let header =
            "<https://example.com/prev>; rel=\"prev\", <https://example.com/last>; rel=\"last\"";

        assert_eq!(parse_next_link(header), None);
    }

    #[test]
    fn parses_next_link_when_rel_contains_multiple_values() {
        let header = "<https://example.com/prev>; rel=\"prev\", <https://example.com/next>; rel=\"prev next\"";

        assert_eq!(
            parse_next_link(header),
            Some("https://example.com/next".to_owned())
        );
    }

    #[test]
    fn retries_network_failures_up_to_the_bound() {
        assert_eq!(
            retry_delay(0, RetryTrigger::Network),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_delay(1, RetryTrigger::Network),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            retry_delay(2, RetryTrigger::Network),
            Some(Duration::from_millis(1000))
        );
        assert_eq!(retry_delay(3, RetryTrigger::Network), None);
    }

    #[test]
    fn retries_only_server_errors() {
        assert_eq!(
            retry_delay(0, RetryTrigger::Status(500)),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_delay(0, RetryTrigger::Status(503)),
            Some(Duration::from_millis(250))
        );
        assert_eq!(retry_delay(0, RetryTrigger::Status(404)), None);
        assert_eq!(retry_delay(0, RetryTrigger::Status(429)), None);
    }

    #[test]
    fn repo_path_percent_encodes_reserved_characters() {
        let path = RepoPath::new("/dir name/workflow#1?.yml/").unwrap();

        assert_eq!(path.to_api_path(), "dir%20name/workflow%231%3F.yml");
    }

    #[test]
    fn non_root_repo_path_rejects_root() {
        assert_eq!(
            NonRootRepoPath::new("/").unwrap_err(),
            RepoPathError::RootPathNotAllowed
        );
    }

    #[test]
    fn contents_url_encodes_each_path_segment() {
        let repo = RepoRef::new("owner", "repo");
        let path = RepoPath::new("dir name/file#1?.txt").unwrap();

        assert_eq!(
            contents_url(&repo, &path),
            "https://api.github.com/repos/owner/repo/contents/dir%20name/file%231%3F.txt"
        );
    }

    #[test]
    fn token_debug_is_redacted() {
        let token = GitHubToken::new("ghp_secret_token");

        assert_eq!(format!("{token:?}"), "GitHubToken([REDACTED])");
    }

    #[test]
    fn client_debug_redacts_token() {
        let client = GitHubClient::new(GitHubToken::new("ghp_secret_token"));
        let debug = format!("{client:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("ghp_secret_token"));
    }

    #[test]
    #[ignore = "requires GITHUB_TOKEN and network access"]
    fn fetches_public_repo_metadata() {
        let token = GitHubToken::from_env("GITHUB_TOKEN").expect("GITHUB_TOKEN must be set");
        let mut client = GitHubClient::new(token);
        let repo = client
            .get_repo(&RepoRef::new("rust-lang", "cargo"))
            .unwrap();

        assert!(!repo.default_branch.to_string().is_empty());
    }
}
