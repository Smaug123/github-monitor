use regex::Regex;

/// Build the regex that matches a block-style `uses:` line for a given action
/// reference. This is the single source of truth shared by the workflow-pin
/// rewriter (which uses it to perform replacements) and the fact gatherer
/// (which uses it to detect inline-flow-style workflows the rewriter cannot
/// handle).
pub(crate) fn block_uses_line_regex(action_ref: &str) -> Regex {
    Regex::new(&format!(
        r#"(?m)^([ \t-]*uses:[ \t]*['"]?){}(['"]?)([ \t]*(?:#[^\r\n]*)?)(\r?)$"#,
        regex::escape(action_ref)
    ))
    .expect("escaped action reference must yield a valid regex")
}
