//! Regenerates the vendored GitHub request-body JSON Schemas under
//! `tests/fixtures/openapi/`, used by the test suite to validate every payload
//! the autofix executor can emit against GitHub's published API description.
//!
//! Source of truth is GitHub's *dereferenced* OpenAPI description, pinned to a
//! specific commit for reproducibility. The dereferenced form inlines every
//! `$ref`, so each endpoint's request-body schema is already self-contained.
//!
//! Run with `cargo run --bin regen_openapi_fixtures`. The ~70 MB description is
//! downloaded from the pin by default; set `OPENAPI_DEREF_PATH` to a local copy
//! to skip the download.
//!
//! The published schema is looser than GitHub's runtime validator in a few
//! spots (e.g. `ref_name.exclude` is optional in the schema but required at
//! runtime — the 422 that motivated commit 94d775f). Those cases are encoded as
//! `LOCAL_TIGHTENINGS`, baked into the vendored fixtures here so the test-time
//! loader only ever sees the tightened schema.

use std::error::Error;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

/// Pin for the upstream description. Bump these together and re-run to refresh.
const PIN_REPO: &str = "github/rest-api-description";
const PIN_SHA: &str = "b150df103aaab9f965a4d85aaaffa768e714fa7f";
const API_VERSION: &str = "2022-11-28";

/// `(fixture stem, HTTP method, templated path)` for every body-bearing request
/// the client in `src/github/client.rs` can issue.
const ENDPOINTS: &[(&str, &str, &str)] = &[
    ("create-ruleset", "post", "/repos/{owner}/{repo}/rulesets"),
    (
        "update-ruleset",
        "put",
        "/repos/{owner}/{repo}/rulesets/{ruleset_id}",
    ),
    ("update-repository", "patch", "/repos/{owner}/{repo}"),
    ("create-git-ref", "post", "/repos/{owner}/{repo}/git/refs"),
    (
        "update-file-contents",
        "put",
        "/repos/{owner}/{repo}/contents/{path}",
    ),
    ("create-pull-request", "post", "/repos/{owner}/{repo}/pulls"),
    (
        "set-fork-pr-approval",
        "put",
        "/repos/{owner}/{repo}/actions/permissions/fork-pr-contributor-approval",
    ),
];

/// Constraints GitHub's runtime enforces but its published schema omits. Each is
/// applied to the named fixtures after normalization. `pointer` is a
/// JSON-pointer to the sub-schema; `required` is added to that sub-schema.
struct Tightening {
    fixtures: &'static [&'static str],
    pointer: &'static str,
    required: &'static [&'static str],
    /// The 422 message / fix commit that justifies this tightening.
    reason: &'static str,
}

const LOCAL_TIGHTENINGS: &[Tightening] = &[Tightening {
    fixtures: &["create-ruleset", "update-ruleset"],
    pointer: "/properties/conditions/properties/ref_name",
    required: &["include", "exclude"],
    reason: "422 \"Missing required parameter `exclude`\" when ref_name is present (commit 94d775f)",
}];

/// Annotation keywords that do not affect validation; stripped for compact,
/// readable, stable fixtures.
const ANNOTATION_KEYS: &[&str] = &["description", "example", "examples", "title", "deprecated"];

fn deref_url() -> String {
    format!(
        "https://raw.githubusercontent.com/{PIN_REPO}/{PIN_SHA}/descriptions/api.github.com/dereferenced/api.github.com.{API_VERSION}.deref.json"
    )
}

fn load_description() -> Result<Value, Box<dyn Error>> {
    if let Ok(path) = std::env::var("OPENAPI_DEREF_PATH") {
        eprintln!("reading description from {path}");
        let bytes = fs::read(path)?;
        return Ok(serde_json::from_slice(&bytes)?);
    }
    let url = deref_url();
    eprintln!("downloading description from {url}");
    let body = ureq::get(&url)
        .call()?
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_vec()?;
    Ok(serde_json::from_slice(&body)?)
}

/// Rewrites OpenAPI 3.0 `nullable: true` into a JSON-Schema null union and drops
/// annotation keywords, recursively.
fn normalize(node: &mut Value) {
    match node {
        Value::Object(map) => {
            if map.get("nullable") == Some(&Value::Bool(true))
                && let Some(ty) = map.get("type").and_then(Value::as_str)
            {
                map.insert("type".to_owned(), json!([ty, "null"]));
            }
            map.remove("nullable");
            for key in ANNOTATION_KEYS {
                map.remove(*key);
            }
            for value in map.values_mut() {
                normalize(value);
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        _ => {}
    }
}

fn request_schema(description: &Value, method: &str, path: &str) -> Result<Value, Box<dyn Error>> {
    description
        .pointer(&format!(
            "/paths/{}/{}/requestBody/content/{}/schema",
            escape_pointer(path),
            method,
            escape_pointer("application/json"),
        ))
        .cloned()
        .ok_or_else(|| {
            format!(
                "no JSON request-body schema for {} {path}",
                method.to_uppercase()
            )
            .into()
        })
}

/// Escapes `/` and `~` per RFC 6901 so path templates work as pointer tokens.
fn escape_pointer(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

fn apply_tightening(schema: &mut Value, tightening: &Tightening) -> Result<(), Box<dyn Error>> {
    let target = schema
        .pointer_mut(tightening.pointer)
        .ok_or_else(|| format!("tightening pointer {} not found", tightening.pointer))?;
    let obj = target
        .as_object_mut()
        .ok_or_else(|| format!("tightening target {} is not an object", tightening.pointer))?;
    obj.insert(
        "required".to_owned(),
        Value::Array(tightening.required.iter().map(|f| json!(f)).collect()),
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let description = load_description()?;
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/openapi");
    fs::create_dir_all(&out_dir)?;

    for (stem, method, path) in ENDPOINTS {
        let mut schema = request_schema(&description, method, path)?;
        normalize(&mut schema);
        for tightening in LOCAL_TIGHTENINGS {
            if tightening.fixtures.contains(stem) {
                apply_tightening(&mut schema, tightening)?;
            }
        }
        let mut text = serde_json::to_string_pretty(&schema)?;
        text.push('\n');
        fs::write(out_dir.join(format!("{stem}.request.json")), text)?;
        eprintln!("wrote {stem}.request.json");
    }

    let tightenings: Vec<Value> = LOCAL_TIGHTENINGS
        .iter()
        .map(|t| {
            json!({
                "fixtures": t.fixtures,
                "pointer": t.pointer,
                "required": t.required,
                "reason": t.reason,
            })
        })
        .collect();
    let pin = json!({
        "note": "Regenerate with `cargo run --bin regen_openapi_fixtures`. Do not edit by hand.",
        "source_repo": PIN_REPO,
        "source_commit": PIN_SHA,
        "api_version": API_VERSION,
        "local_tightenings": tightenings,
    });
    let mut pin_text = serde_json::to_string_pretty(&pin)?;
    pin_text.push('\n');
    fs::write(out_dir.join("pin.json"), pin_text)?;
    eprintln!("wrote pin.json");

    Ok(())
}
