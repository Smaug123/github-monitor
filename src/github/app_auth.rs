use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use ureq::{Agent, Error as UreqError};

use crate::github::client::{GITHUB_API_VERSION, USER_AGENT};
use crate::types::RepoRef;

const JWT_BACKDATE_SECONDS: u64 = 60;
const JWT_LIFETIME_SECONDS: u64 = 9 * 60;
pub(crate) const INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS: u64 = 5 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstallationId(pub u64);

impl fmt::Display for InstallationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct GitHubAppCredentials {
    app_id: u64,
    encoding_key: EncodingKey,
}

impl GitHubAppCredentials {
    pub fn from_pem(app_id: u64, pem: &[u8]) -> Result<Self, AppAuthError> {
        let encoding_key =
            EncodingKey::from_rsa_pem(pem).map_err(|source| AppAuthError::InvalidPrivateKey {
                source: source.to_string(),
            })?;
        Ok(Self {
            app_id,
            encoding_key,
        })
    }
}

impl fmt::Debug for GitHubAppCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubAppCredentials")
            .field("app_id", &self.app_id)
            .field("encoding_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone)]
pub struct InstallationToken {
    pub token: String,
    pub expires_at: SystemTime,
}

impl fmt::Debug for InstallationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstallationToken")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl InstallationToken {
    pub fn needs_refresh(&self, now: SystemTime) -> bool {
        match self.expires_at.duration_since(now) {
            Ok(remaining) => {
                remaining <= Duration::from_secs(INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS)
            }
            Err(_) => true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct JwtClaims {
    iss: String,
    iat: u64,
    exp: u64,
}

#[derive(Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct InstallationLookupResponse {
    id: u64,
}

pub fn mint_jwt(
    credentials: &GitHubAppCredentials,
    now: SystemTime,
) -> Result<String, AppAuthError> {
    let now_secs = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppAuthError::ClockBeforeEpoch)?
        .as_secs();
    let iat = now_secs.saturating_sub(JWT_BACKDATE_SECONDS);
    let exp = now_secs.saturating_add(JWT_LIFETIME_SECONDS);

    let claims = JwtClaims {
        iss: credentials.app_id.to_string(),
        iat,
        exp,
    };

    jsonwebtoken::encode(
        &Header::new(Algorithm::RS256),
        &claims,
        &credentials.encoding_key,
    )
    .map_err(|source| AppAuthError::Jwt {
        source: source.to_string(),
    })
}

pub fn fetch_installation_token(
    agent: &Agent,
    base_url: &str,
    jwt: &str,
    installation_id: InstallationId,
) -> Result<InstallationToken, AppAuthError> {
    let url = format!("{base_url}/app/installations/{installation_id}/access_tokens");
    let response = agent
        .post(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {jwt}"))
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header("User-Agent", USER_AGENT)
        .send_empty()
        .map_err(|source| AppAuthError::Request {
            url: url.clone(),
            source: source.to_string(),
        })?;

    let body: InstallationTokenResponse = read_success_json(&url, response)?;
    let expires_at =
        parse_iso8601_utc(&body.expires_at).ok_or_else(|| AppAuthError::MalformedExpiresAt {
            value: body.expires_at.clone(),
        })?;

    Ok(InstallationToken {
        token: body.token,
        expires_at,
    })
}

/// Parses the strict `YYYY-MM-DDTHH:MM:SSZ` UTC format that the GitHub
/// installation-token endpoint returns. Returns `None` for any other
/// shape, including offsets, fractional seconds, or invalid civil dates.
fn parse_iso8601_utc(value: &str) -> Option<SystemTime> {
    if value.len() != 20 {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'Z'
    {
        return None;
    }

    let year: i32 = value.get(0..4)?.parse().ok()?;
    let month: u32 = value.get(5..7)?.parse().ok()?;
    let day: u32 = value.get(8..10)?.parse().ok()?;
    let hour: u32 = value.get(11..13)?.parse().ok()?;
    let minute: u32 = value.get(14..16)?.parse().ok()?;
    let second: u32 = value.get(17..19)?.parse().ok()?;

    if !(1..=12).contains(&month) || hour >= 24 || minute >= 60 || second >= 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if civil_from_days(days) != (year, month, day) {
        return None;
    }

    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600)?
        .checked_add(i64::from(minute) * 60)?
        .checked_add(i64::from(second))?;

    if seconds < 0 {
        return None;
    }

    Some(UNIX_EPOCH + Duration::from_secs(seconds as u64))
}

/// Howard Hinnant's `days_from_civil`: maps a proleptic Gregorian
/// `(year, month, day)` to the count of days since 1970-01-01.
/// See <https://howardhinnant.github.io/date_algorithms.html#days_from_civil>.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`. Used purely to validate that a
/// parsed `(year, month, day)` round-trips — i.e. rejects nonsense
/// like 2024-02-30.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (if m <= 2 { y + 1 } else { y }) as i32;
    (year, m, d)
}

pub fn lookup_installation_id(
    agent: &Agent,
    base_url: &str,
    jwt: &str,
    repo: &RepoRef,
) -> Result<InstallationId, AppAuthError> {
    let url = format!("{base_url}/repos/{repo}/installation");
    let response = agent
        .get(&url)
        .config()
        .http_status_as_error(false)
        .build()
        .header("Accept", "application/vnd.github+json")
        .header("Authorization", format!("Bearer {jwt}"))
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .header("User-Agent", USER_AGENT)
        .call()
        .map_err(|source| AppAuthError::Request {
            url: url.clone(),
            source: source.to_string(),
        })?;

    let body: InstallationLookupResponse = read_success_json(&url, response)?;
    Ok(InstallationId(body.id))
}

fn read_success_json<T>(
    url: &str,
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<T, AppAuthError>
where
    T: for<'de> Deserialize<'de>,
{
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(AppAuthError::UnexpectedStatus {
            url: url.to_owned(),
            status,
            body,
        });
    }

    response
        .body_mut()
        .read_json()
        .map_err(|source: UreqError| AppAuthError::Request {
            url: url.to_owned(),
            source: source.to_string(),
        })
}

#[derive(Debug)]
pub enum AppAuthError {
    InvalidPrivateKey {
        source: String,
    },
    Jwt {
        source: String,
    },
    Request {
        url: String,
        source: String,
    },
    UnexpectedStatus {
        url: String,
        status: u16,
        body: String,
    },
    MalformedExpiresAt {
        value: String,
    },
    ClockBeforeEpoch,
}

impl fmt::Display for AppAuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrivateKey { source } => {
                write!(f, "failed to parse GitHub App private key: {source}")
            }
            Self::Jwt { source } => write!(f, "failed to mint GitHub App JWT: {source}"),
            Self::Request { url, source } => write!(f, "request to {url} failed: {source}"),
            Self::UnexpectedStatus { url, status, body } => write!(
                f,
                "request to {url} returned unexpected status {status}: {body}"
            ),
            Self::MalformedExpiresAt { value } => write!(
                f,
                "installation token response had unparseable expires_at: {value:?}"
            ),
            Self::ClockBeforeEpoch => f.write_str("system clock is before the UNIX epoch"),
        }
    }
}

impl std::error::Error for AppAuthError {}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, UNIX_EPOCH};

    use jsonwebtoken::{Algorithm, DecodingKey, Validation};
    use proptest::prelude::*;
    use ureq::Agent;

    use super::*;

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

    const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAvqaEU0WKFfiipJqYA+py\n\
oN7VBtFsv7SUdC90dHUXMwZhvUrxOkfqHkRXWXXpcl8SzouAx4/5q9Bvp1DIUQEt\n\
Lh/T75Hp13hM8TvqytJbPbDJw2Zk3nStv9adhO2zBnOQ0idtFxE4te0jSd+Rt+HS\n\
tgmSHYZKQGnMT6mD7+sE+MLCzHxn35K/G3BuyXjQAZBmp/tbOUSMa9Ar72kXkydP\n\
PnP/3QmPKfUqD6ARpvgsb27gYp7Bk8iAKN2oXFnvjgPD/jNjRD393Ki7QIAufTZA\n\
xOlMVXKH7QetdGsmsjlzdNOqBCYxHZb5UGGhp2sfjhmv1+4iJk2Es5JYugim9CQF\n\
HwIDAQAB\n\
-----END PUBLIC KEY-----\n";

    fn test_credentials(app_id: u64) -> GitHubAppCredentials {
        GitHubAppCredentials::from_pem(app_id, TEST_PRIVATE_KEY.as_bytes()).unwrap()
    }

    fn decode_jwt(token: &str) -> JwtClaims {
        let decoding_key = DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).unwrap();
        let mut validation = Validation::new(Algorithm::RS256);
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        jsonwebtoken::decode::<JwtClaims>(token, &decoding_key, &validation)
            .unwrap()
            .claims
    }

    proptest! {
        #[test]
        fn jwt_claims_round_trip(
            app_id in 1_u64..=u64::from(u32::MAX),
            now_secs in (JWT_BACKDATE_SECONDS + 1)..=u64::from(u32::MAX),
        ) {
            let credentials = test_credentials(app_id);
            let now = UNIX_EPOCH + Duration::from_secs(now_secs);
            let token = mint_jwt(&credentials, now).unwrap();
            let claims = decode_jwt(&token);

            prop_assert_eq!(claims.iss, app_id.to_string());
            prop_assert_eq!(claims.iat, now_secs - JWT_BACKDATE_SECONDS);
            prop_assert_eq!(claims.exp, now_secs + JWT_LIFETIME_SECONDS);
            prop_assert!(claims.iat < claims.exp);
            prop_assert!(claims.exp - claims.iat <= 600);
        }
    }

    #[test]
    fn jwt_uses_rs256_alg() {
        let credentials = test_credentials(42);
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let token = mint_jwt(&credentials, now).unwrap();
        let header = jsonwebtoken::decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::RS256);
    }

    #[test]
    fn invalid_pem_returns_invalid_private_key_error() {
        let error = GitHubAppCredentials::from_pem(1, b"not a pem").unwrap_err();
        assert!(matches!(error, AppAuthError::InvalidPrivateKey { .. }));
    }

    #[test]
    fn installation_token_needs_refresh_after_expiry() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let token = InstallationToken {
            token: "ghs_abc".to_owned(),
            expires_at: now + Duration::from_secs(10),
        };

        assert!(token.needs_refresh(now + Duration::from_secs(20)));
    }

    #[test]
    fn installation_token_needs_refresh_within_margin() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let token = InstallationToken {
            token: "ghs_abc".to_owned(),
            expires_at: now + Duration::from_secs(INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS),
        };

        assert!(token.needs_refresh(now));
    }

    #[test]
    fn installation_token_fresh_outside_margin() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let token = InstallationToken {
            token: "ghs_abc".to_owned(),
            expires_at: now + Duration::from_secs(INSTALLATION_TOKEN_RENEW_MARGIN_SECONDS + 1),
        };

        assert!(!token.needs_refresh(now));
    }

    #[test]
    fn fetch_installation_token_uses_server_expires_at() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "POST",
            "/app/installations/12345/access_tokens",
            201,
            r#"{"token":"ghs_secret","expires_at":"2099-01-01T00:00:00Z"}"#,
        )]);
        let agent: Agent = Agent::config_builder().build().into();

        let token = fetch_installation_token(
            &agent,
            &server.base_url(),
            "test.jwt.value",
            InstallationId(12345),
        )
        .unwrap();

        assert_eq!(token.token, "ghs_secret");
        assert_eq!(
            token.expires_at,
            UNIX_EPOCH + Duration::from_secs(4_070_908_800),
        );

        let recorded = server.into_recorded();
        let headers = &recorded[0].headers;
        assert_eq!(
            header_value(headers, "authorization").as_deref(),
            Some("Bearer test.jwt.value")
        );
        assert_eq!(
            header_value(headers, "accept").as_deref(),
            Some("application/vnd.github+json")
        );
    }

    #[test]
    fn fetch_installation_token_rejects_malformed_expires_at() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "POST",
            "/app/installations/1/access_tokens",
            201,
            r#"{"token":"ghs_secret","expires_at":"not a date"}"#,
        )]);
        let agent: Agent = Agent::config_builder().build().into();

        let error = fetch_installation_token(&agent, &server.base_url(), "jwt", InstallationId(1))
            .unwrap_err();

        match error {
            AppAuthError::MalformedExpiresAt { value } => assert_eq!(value, "not a date"),
            other => panic!("expected MalformedExpiresAt, got {other:?}"),
        }
    }

    #[test]
    fn fetch_installation_token_propagates_error_status() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "POST",
            "/app/installations/9/access_tokens",
            401,
            r#"{"message":"bad jwt"}"#,
        )]);
        // Use a default agent (status-as-error = true) to verify the per-request
        // override inside fetch_installation_token routes 4xx through UnexpectedStatus.
        let agent: Agent = Agent::config_builder().build().into();

        let error = fetch_installation_token(&agent, &server.base_url(), "x", InstallationId(9))
            .unwrap_err();

        match error {
            AppAuthError::UnexpectedStatus { status, body, .. } => {
                assert_eq!(status, 401);
                assert!(body.contains("bad jwt"), "body was {body:?}");
            }
            other => panic!("expected unexpected status, got {other:?}"),
        }
    }

    #[test]
    fn parse_iso8601_utc_handles_known_dates() {
        let cases = [
            ("1970-01-01T00:00:00Z", 0_u64),
            ("2000-01-01T00:00:00Z", 946_684_800),
            ("2024-02-29T12:34:56Z", 1_709_210_096),
            ("2099-12-31T23:59:59Z", 4_102_444_799),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_iso8601_utc(input),
                Some(UNIX_EPOCH + Duration::from_secs(expected)),
                "parse_iso8601_utc({input:?})"
            );
        }
    }

    #[test]
    fn parse_iso8601_utc_rejects_malformed_inputs() {
        let bad = [
            "",
            "2024-01-01",
            "2024-01-01T00:00:00",       // no Z
            "2024-01-01T00:00:00+00:00", // offset
            "2024-1-01T00:00:00Z",       // single-digit month
            "2024-02-30T00:00:00Z",      // not a real day
            "2023-02-29T00:00:00Z",      // not a leap year
            "2024-13-01T00:00:00Z",      // month 13
            "2024-01-01T24:00:00Z",      // hour 24
            "2024-01-01T00:60:00Z",      // minute 60
            "2024-01-01T00:00:60Z",      // second 60
            "2024-01-01T00:00:00z",      // lowercase z
        ];
        for input in bad {
            assert_eq!(
                parse_iso8601_utc(input),
                None,
                "expected None for malformed input {input:?}",
            );
        }
    }

    proptest! {
        #[test]
        fn parse_iso8601_utc_roundtrips_civil_dates(
            year in 1970_i32..=2200,
            month in 1_u32..=12,
            hour in 0_u32..=23,
            minute in 0_u32..=59,
            second in 0_u32..=59,
            day_seed in 0_u32..28,
        ) {
            // Always-valid day per month (1..=28) avoids the
            // varying-month-length issue without us building a calendar.
            let day = day_seed + 1;
            let formatted = format!(
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
            );

            let parsed = parse_iso8601_utc(&formatted).expect("known-valid date should parse");
            let expected_seconds = days_from_civil(year, month, day) * 86_400
                + i64::from(hour) * 3_600
                + i64::from(minute) * 60
                + i64::from(second);
            prop_assert_eq!(
                parsed,
                UNIX_EPOCH + Duration::from_secs(expected_seconds as u64)
            );
        }
    }

    #[test]
    fn lookup_installation_id_returns_id_for_repo() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "GET",
            "/repos/owner/repo/installation",
            200,
            r#"{"id":777,"account":{"login":"owner"}}"#,
        )]);
        let agent: Agent = Agent::config_builder().build().into();
        let repo = RepoRef::new("owner", "repo");

        let id = lookup_installation_id(&agent, &server.base_url(), "jwt-value", &repo).unwrap();

        assert_eq!(id, InstallationId(777));

        let recorded = server.into_recorded();
        assert_eq!(recorded[0].method, "GET");
        assert_eq!(recorded[0].path, "/repos/owner/repo/installation");
        assert_eq!(
            header_value(&recorded[0].headers, "authorization").as_deref(),
            Some("Bearer jwt-value")
        );
    }

    #[test]
    fn lookup_installation_id_propagates_error_status() {
        let server = TestServer::spawn(vec![ExpectedRequest::new(
            "GET",
            "/repos/owner/repo/installation",
            404,
            r#"{"message":"Not Found"}"#,
        )]);
        // Default agent (status-as-error = true) — exercise the per-request override.
        let agent: Agent = Agent::config_builder().build().into();
        let repo = RepoRef::new("owner", "repo");

        let error = lookup_installation_id(&agent, &server.base_url(), "x", &repo).unwrap_err();

        match error {
            AppAuthError::UnexpectedStatus { status, body, .. } => {
                assert_eq!(status, 404);
                assert!(body.contains("Not Found"), "body was {body:?}");
            }
            other => panic!("expected unexpected status, got {other:?}"),
        }
    }

    #[test]
    fn installation_token_debug_redacts_token() {
        let token = InstallationToken {
            token: "ghs_supersecret".to_owned(),
            expires_at: UNIX_EPOCH + Duration::from_secs(2_000_000_000),
        };
        let debug = format!("{token:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("ghs_supersecret"));
    }

    #[test]
    fn credentials_debug_redacts_key() {
        let credentials = test_credentials(123);
        let debug = format!("{credentials:?}");

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("PRIVATE KEY"));
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
