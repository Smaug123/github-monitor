use std::collections::HashMap;
use std::time::SystemTime;

use ureq::Agent;

use crate::github::app_auth::{
    AppAuthError, GitHubAppCredentials, InstallationId, InstallationToken,
    fetch_installation_token, lookup_installation_id, mint_jwt,
};
use crate::github::client::GitHubToken;
use crate::types::RepoRef;

pub enum GitHubAuth {
    Token(GitHubToken),
    App(GitHubAppAuthState),
}

pub struct GitHubAppAuthState {
    credentials: GitHubAppCredentials,
    installations: HashMap<RepoRef, InstallationCacheEntry>,
}

struct InstallationCacheEntry {
    id: InstallationId,
    token: InstallationToken,
}

impl GitHubAuth {
    pub fn token(token: GitHubToken) -> Self {
        Self::Token(token)
    }

    pub fn app(credentials: GitHubAppCredentials) -> Self {
        Self::App(GitHubAppAuthState {
            credentials,
            installations: HashMap::new(),
        })
    }

    pub(crate) fn resolve_bearer(
        &mut self,
        repo: &RepoRef,
        agent: &Agent,
        api_base_url: &str,
        now: SystemTime,
    ) -> Result<String, AppAuthError> {
        match self {
            Self::Token(token) => Ok(token.as_bearer_header()),
            Self::App(state) => state.resolve_bearer(repo, agent, api_base_url, now),
        }
    }
}

impl GitHubAppAuthState {
    fn resolve_bearer(
        &mut self,
        repo: &RepoRef,
        agent: &Agent,
        api_base_url: &str,
        now: SystemTime,
    ) -> Result<String, AppAuthError> {
        if let Some(entry) = self.installations.get(repo)
            && !entry.token.needs_refresh(now)
        {
            return Ok(format!("Bearer {}", entry.token.token));
        }

        let jwt = mint_jwt(&self.credentials, now)?;
        let installation_id = match self.installations.get(repo).map(|entry| entry.id) {
            Some(id) => id,
            None => lookup_installation_id(agent, api_base_url, &jwt, repo)?,
        };
        let installation_token =
            fetch_installation_token(agent, api_base_url, &jwt, installation_id)?;
        let bearer = format!("Bearer {}", installation_token.token);

        self.installations.insert(
            repo.clone(),
            InstallationCacheEntry {
                id: installation_id,
                token: installation_token,
            },
        );

        Ok(bearer)
    }
}

impl std::fmt::Debug for GitHubAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Token(token) => f.debug_tuple("Token").field(token).finish(),
            Self::App(state) => f
                .debug_struct("App")
                .field("credentials", &state.credentials)
                .field("cached_installations", &state.installations.len())
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use ureq::Agent;

    use super::*;
    use crate::github::app_auth::INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS;

    const TEST_PRIVATE_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----\n\
MIIEowIBAAKCAQEAvqaEU0WKFfiipJqYA+pyoN7VBtFsv7SUdC90dHUXMwZhvUrx\n\
OkfqHkRXWXXpcl8SzouAx4/5q9Bvp1DIUQEtLh/T75Hp13hM8TvqytJbPbDJw2Zk\n\
3nStv9adhO2zBnOQ0idtFxE4te0jSd+Rt+HStgmSHYZKQGnMT6mD7+sE+MLCzHxn\n\
35K/G3BuyXjQAZBmp/tbOUSMa9Ar72kXkydPPnP/3QmPKfUqD6ARpvgsb27gYp7B\n\
k8iAKN2oXFnvjgPD/jNjRD393Ki7QIAufTZAxOlMVXKH7QetdGsmsjlzdNOqBCYx\n\
HZb5UGGhp2sfjhmv1+4iJk2Es5JYugim9CQFHwIDAQABAoIBAQCoxALTK/WmqWhg\n\
SbFTlhBOs7LjzDN2KEZZ60AtbxFQS8/tnw+XRd3LWTfxq10xr1OYnwkqnxqmq2aL\n\
OAl7G42BDQ+xPPtBj+6chSu8yyWVoI+ad2PHQIYmEbdy2m/lwBtszRXWm5oWAYuB\n\
c3Usz6yVFDfSBvRnvL/trONsWCEYitV3ZHpfnfcE5xKsMgv/VWvooyOxtL587uKz\n\
7EnxSj2klyPb4hNPbIRidQRxu1haA7l/ZoRHp8nPliSVyu2KKik4IpUO68/3rbNp\n\
dJAaXIMZkcg3SAxyZIfVCllkdi7pRTz1PJwuFwKMALEQGvjvrLGOwsYRtDsDCzhn\n\
J5j2ppShAoGBAPo2t3ucmaWQBouCvV4iCBBjaU1tMBMUZtfARl0ACE0NDvPLtorZ\n\
C1VZClc5ULDQwPqdHpgLmAuWCpfjp1QPJMZ1cIx/a5TIZeCOmDWqhLuejvup1jhK\n\
WFsTpPTwXnU2ZLKqoAN/wi9rxRDfkUikzbX4gWqkHlV9Ro4WKBasheEnAoGBAMMP\n\
LmfhHWnqB+sUy2sEvkXO4OGjw2uOuSyiOVudWVl7W376uz1/l41R3xTZxFnAJSPz\n\
YBMEelXqqhiHhs9CyeZ2kpyDoh0YHozkuSVjMuCQQ2Dy2Td8sh5Tzw2fS/z79+Ih\n\
lx3ndOx5utdK5CJbdupQ3Zl9tV9QZFQqYs8kc0dJAoGAO8nPVi45WKJtrfBzp4ai\n\
PqhChUnN7wE1AeDj710Onrq8E+1dlRf/6Uj5e5YqfdWkBz58DQDYOAyGQ30WgrOL\n\
qhBt8GSSJF8uWNY58LjqNprQt7oBgjnhmwG6rPyy1XdF4Jt82NkyYXpzAHErmhwn\n\
O5BB/GVzCiKBNXp94c0fwIkCgYB/j+8OOjcNK+LPxwKc0zZH2tpQVdOYBHdvDAws\n\
sMNc9IJKkVhgCJAo+FDGhv+Unkbrst6ysSv8AgIJFqB/7LKzB/orZx5enoZkJ7Q5\n\
Eh2UpGOcBFUvp1mo4bA3vWRpZrKebM8x3Esn1xfsceqt2Vj0NbwmBALX+XATZsDF\n\
rJXDGQKBgHA9zsVO+x2uLDhBfCBVRqOnJs8WhFThP39RbMKWcHBsij7qcjUF/2el\n\
74RBqeX463bL9ga4h90RrucowKQg5ALC5b9hBjoSygWGIh6+J2U+uMWmmF5D8g8w\n\
pz5HgO77QkZ1RfS4q9KbofjdaZN2XyqYi03wT/BzsK9aopwBLPc4\n\
-----END RSA PRIVATE KEY-----\n";

    fn test_credentials() -> GitHubAppCredentials {
        GitHubAppCredentials::from_pem(123, TEST_PRIVATE_KEY.as_bytes()).unwrap()
    }

    fn test_agent() -> Agent {
        Agent::config_builder().build().into()
    }

    fn fixed_now(epoch_seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(epoch_seconds)
    }

    #[test]
    fn token_auth_returns_static_bearer() {
        let mut auth = GitHubAuth::token(GitHubToken::new("ghp_abc"));
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        let bearer = auth
            .resolve_bearer(
                &repo,
                &agent,
                "http://unused.invalid",
                fixed_now(1_700_000_000),
            )
            .unwrap();

        assert_eq!(bearer, "Bearer ghp_abc");

        // Second call still works and makes no HTTP calls (agent points nowhere).
        let other_repo = RepoRef::new("other", "name");
        let bearer = auth
            .resolve_bearer(
                &other_repo,
                &agent,
                "http://unused.invalid",
                fixed_now(1_700_000_001),
            )
            .unwrap();
        assert_eq!(bearer, "Bearer ghp_abc");
    }

    #[test]
    fn app_auth_first_call_performs_lookup_then_fetch() {
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":42}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/42/access_tokens",
                201,
                r#"{"token":"ghs_installation","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        let bearer = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_000))
            .unwrap();
        assert_eq!(bearer, "Bearer ghs_installation");

        let recorded = server.into_recorded();
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(recorded[0].path, "/repos/owner/repo/installation");
        assert_eq!(recorded[1].method, "POST");
        assert_eq!(recorded[1].path, "/app/installations/42/access_tokens");
        // The POST should authenticate with the same minted JWT we used for the lookup.
        assert_eq!(
            header_value(&recorded[0].headers, "authorization"),
            header_value(&recorded[1].headers, "authorization"),
        );
    }

    #[test]
    fn app_auth_caches_token_for_subsequent_calls_to_same_repo() {
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":7}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/7/access_tokens",
                201,
                r#"{"token":"ghs_one","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        let first = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_000))
            .unwrap();
        let second = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_100))
            .unwrap();

        assert_eq!(first, "Bearer ghs_one");
        assert_eq!(second, "Bearer ghs_one");

        // Only the two original requests should have hit the server.
        assert_eq!(server.into_recorded().len(), 2);
    }

    #[test]
    fn app_auth_refreshes_token_inside_renew_margin_but_keeps_installation_id() {
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":99}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/99/access_tokens",
                201,
                // expires_at = 1970-01-01T00:00:00Z + 1000 + 100 = 1970-01-01T00:18:20Z
                r#"{"token":"ghs_first","expires_at":"1970-01-01T00:18:20Z"}"#,
            ),
            ExpectedRequest::new(
                "POST",
                "/app/installations/99/access_tokens",
                201,
                r#"{"token":"ghs_second","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        let first = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_000))
            .unwrap();
        // At t = 1_100, expires_at - now = 0s, well inside the renew margin.
        let second = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_100))
            .unwrap();

        assert_eq!(first, "Bearer ghs_first");
        assert_eq!(second, "Bearer ghs_second");

        let recorded = server.into_recorded();
        assert_eq!(recorded.len(), 3, "expected lookup + 2 token fetches");
        assert_eq!(recorded[0].path, "/repos/owner/repo/installation");
        assert_eq!(recorded[1].path, "/app/installations/99/access_tokens");
        // Crucially, the second refresh skipped the installation lookup.
        assert_eq!(recorded[2].path, "/app/installations/99/access_tokens");
    }

    #[test]
    fn app_auth_caches_per_repo_independently() {
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/a/x/installation", 200, r#"{"id":1}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/1/access_tokens",
                201,
                r#"{"token":"ghs_a","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
            ExpectedRequest::new("GET", "/repos/b/y/installation", 200, r#"{"id":2}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/2/access_tokens",
                201,
                r#"{"token":"ghs_b","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();

        let bearer_a = auth
            .resolve_bearer(
                &RepoRef::new("a", "x"),
                &agent,
                &server.base_url(),
                fixed_now(1_700_000_000),
            )
            .unwrap();
        let bearer_b = auth
            .resolve_bearer(
                &RepoRef::new("b", "y"),
                &agent,
                &server.base_url(),
                fixed_now(1_700_000_000),
            )
            .unwrap();

        assert_eq!(bearer_a, "Bearer ghs_a");
        assert_eq!(bearer_b, "Bearer ghs_b");
        assert_eq!(server.into_recorded().len(), 4);
    }

    #[test]
    fn app_auth_propagates_lookup_failure() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "GET",
            "/repos/owner/repo/installation",
            404,
            r#"{"message":"Not Found"}"#,
        )]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        let error = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_000))
            .unwrap_err();

        match error {
            AppAuthError::UnexpectedStatus { status, .. } => assert_eq!(status, 404),
            other => panic!("expected UnexpectedStatus, got {other:?}"),
        }
    }

    #[test]
    fn app_auth_token_fetch_failure_does_not_pollute_cache() {
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":17}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/17/access_tokens",
                500,
                r#"{"message":"boom"}"#,
            ),
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":17}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/17/access_tokens",
                201,
                r#"{"token":"ghs_recovered","expires_at":"2099-01-01T00:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        // First attempt: lookup succeeds, token fetch fails. Cache stays empty.
        let error = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_000))
            .unwrap_err();
        assert!(matches!(
            error,
            AppAuthError::UnexpectedStatus { status: 500, .. }
        ));

        // Second attempt: retries the full lookup + fetch sequence and succeeds.
        let bearer = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(1_700_000_001))
            .unwrap();
        assert_eq!(bearer, "Bearer ghs_recovered");

        assert_eq!(server.into_recorded().len(), 4);
    }

    #[test]
    fn app_auth_fresh_token_is_not_refreshed_outside_margin() {
        // Verifies the boundary the cache check uses against the renew margin.
        let server = TestServer::spawn(vec![
            ExpectedRequest::new("GET", "/repos/owner/repo/installation", 200, r#"{"id":5}"#),
            ExpectedRequest::new(
                "POST",
                "/app/installations/5/access_tokens",
                201,
                // expires_at = 1970-01-01T01:00:00Z = 3600s
                r#"{"token":"ghs_long","expires_at":"1970-01-01T01:00:00Z"}"#,
            ),
        ]);
        let mut auth = GitHubAuth::app(test_credentials());
        let agent = test_agent();
        let repo = RepoRef::new("owner", "repo");

        // First call at t=0; token expires at t=3600.
        let _ = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(0))
            .unwrap();

        // Second call at t = expires_at - margin - 1: still fresh; no refresh.
        let t = 3600 - INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS - 1;
        let bearer = auth
            .resolve_bearer(&repo, &agent, &server.base_url(), fixed_now(t))
            .unwrap();
        assert_eq!(bearer, "Bearer ghs_long");

        assert_eq!(server.into_recorded().len(), 2);
    }

    fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    struct ExpectedRequest {
        method: &'static str,
        path: &'static str,
        status_code: u16,
        response_body: &'static str,
    }

    impl ExpectedRequest {
        fn new(
            method: &'static str,
            path: &'static str,
            status_code: u16,
            response_body: &'static str,
        ) -> Self {
            Self {
                method,
                path,
                status_code,
                response_body,
            }
        }
    }

    struct RecordedRequest {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
    }

    struct TestServer {
        base_url: String,
        handle: Option<JoinHandle<Vec<RecordedRequest>>>,
    }

    impl TestServer {
        fn spawn(expectations: Vec<ExpectedRequest>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let handle = thread::spawn(move || {
                let mut recorded = Vec::with_capacity(expectations.len());
                for expected in expectations {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_request(&mut stream);
                    assert_eq!(request.method, expected.method);
                    assert_eq!(request.path, expected.path);

                    let response = format!(
                        "HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        expected.status_code,
                        expected.response_body.len(),
                        expected.response_body
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    recorded.push(request);
                }
                recorded
            });

            Self {
                base_url: format!("http://{address}"),
                handle: Some(handle),
            }
        }

        fn base_url(&self) -> String {
            self.base_url.clone()
        }

        fn into_recorded(mut self) -> Vec<RecordedRequest> {
            self.handle.take().unwrap().join().unwrap()
        }
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn read_request(stream: &mut impl Read) -> RecordedRequest {
        let mut buffer = Vec::new();
        let mut byte = [0_u8; 1];
        while !buffer.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            buffer.push(byte[0]);
        }

        let header_text = String::from_utf8(buffer).unwrap();
        let mut lines = header_text.split("\r\n");
        let request_line = lines.next().unwrap();
        let mut request_parts = request_line.split_whitespace();
        let method = request_parts.next().unwrap().to_owned();
        let path = request_parts.next().unwrap().to_owned();

        let mut headers = Vec::new();
        let mut content_length = 0_usize;
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim().to_owned();
                let value = value.trim().to_owned();
                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().unwrap_or(0);
                }
                headers.push((name, value));
            }
        }

        if content_length > 0 {
            let mut body = vec![0_u8; content_length];
            stream.read_exact(&mut body).unwrap();
        }

        RecordedRequest {
            method,
            path,
            headers,
        }
    }
}
