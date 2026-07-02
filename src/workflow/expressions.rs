use std::sync::OnceLock;

use regex::Regex;

pub fn expression_blocks(input: &str) -> ExpressionBlocks<'_> {
    ExpressionBlocks { remaining: input }
}

pub struct ExpressionBlocks<'a> {
    remaining: &'a str,
}

impl<'a> Iterator for ExpressionBlocks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let open = self.remaining.find("${{")?;
        let after_open = &self.remaining[open + 3..];
        let Some(close) = after_open.find("}}") else {
            self.remaining = "";
            return None;
        };
        let inner = &after_open[..close];
        self.remaining = &after_open[close + 2..];
        Some(inner)
    }
}

pub fn references_secret(expression: &str) -> bool {
    secret_regex().is_match(&strip_string_literals(expression))
}

pub fn secret_tokens(expression: &str) -> Vec<String> {
    secret_regex()
        .captures_iter(&strip_string_literals(expression))
        .map(|cap| {
            if let Some(name) = cap.get(1) {
                format!("secrets.{}", name.as_str())
            } else {
                "secrets".to_owned()
            }
        })
        .collect()
}

/// Replaces every single-quoted string literal (GitHub Actions expression
/// syntax, with `''` escaping a quote) with a single space, so the word
/// `secrets` appearing *inside* a literal — e.g. `contains(title, 'secrets')` —
/// is not mistaken for a reference to the `secrets` context. The space keeps
/// identifier boundaries intact for the regex.
fn strip_string_literals(expression: &str) -> String {
    let mut out = String::with_capacity(expression.len());
    let mut chars = expression.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next(); // `''` is an escaped quote — stay in the literal.
                } else {
                    in_string = false;
                }
            }
            // Any other character is literal content and is dropped.
        } else if c == '\'' {
            in_string = true;
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

// Matches any reference to the `secrets` context, not just `secrets.NAME`:
// `toJSON(secrets)` (dump everything), `secrets[expr]` (bracket / dynamic
// index — the `[` hits the boundary branch), and a bare `secrets` identifier
// all leak secrets. The leading `(?:^|[^A-Za-z0-9_.])` rejects member access
// (`outputs.secrets`) and longer identifiers (`xsecrets`); the trailing
// alternation names the specific secret via `.NAME` (group 1) and otherwise
// anchors on a non-identifier boundary so `secretsfoo` is not a match. (The
// `regex` crate has no lookaround, so boundaries are spelled out. Callers pass
// input through `strip_string_literals` first, so quoted text never matches.)
fn secret_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?:^|[^A-Za-z0-9_.])secrets(?:\.([A-Za-z_][A-Za-z0-9_]*)|[^A-Za-z0-9_.]|$)")
            .expect("secrets regex compiles")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_blocks_yields_inner_text_in_order() {
        let input = "foo ${{ a }} bar ${{ b.c }} baz";
        let blocks: Vec<_> = expression_blocks(input).collect();
        assert_eq!(blocks, vec![" a ", " b.c "]);
    }

    #[test]
    fn expression_blocks_handles_consecutive_blocks() {
        let input = "${{ a }}${{ b }}";
        let blocks: Vec<_> = expression_blocks(input).collect();
        assert_eq!(blocks, vec![" a ", " b "]);
    }

    #[test]
    fn expression_blocks_ignores_unclosed_braces() {
        let input = "before ${{ never closes";
        let blocks: Vec<_> = expression_blocks(input).collect();
        assert!(blocks.is_empty());
    }

    #[test]
    fn expression_blocks_yields_nothing_for_plain_text() {
        let blocks: Vec<_> = expression_blocks("nothing interesting here").collect();
        assert!(blocks.is_empty());
    }

    #[test]
    fn references_secret_matches_top_level_identifier() {
        assert!(references_secret(" secrets.FOO "));
        assert!(references_secret("(secrets.FOO != '')"));
        assert!(references_secret("secrets.FOO"));
    }

    #[test]
    fn references_secret_rejects_member_access() {
        assert!(!references_secret(" outputs.secrets.FOO "));
        assert!(!references_secret("matrix.secrets.X"));
        assert!(!references_secret("steps.x.outputs.secrets.Y"));
    }

    #[test]
    fn references_secret_rejects_substring_only() {
        assert!(!references_secret("xsecrets.FOO"));
        assert!(!references_secret("_secrets.FOO"));
        assert!(!references_secret("9secrets.FOO"));
    }

    #[test]
    fn references_secret_matches_whole_context_access() {
        // Any reference to the `secrets` context counts, not just `secrets.NAME`.
        // These are the classic dump-everything / dynamic-index exfil vectors.
        assert!(references_secret("secrets"));
        assert!(references_secret("toJSON(secrets)"));
        assert!(references_secret("secrets['MY-SECRET']"));
        assert!(references_secret("secrets[format('X')]"));
    }

    #[test]
    fn references_secret_rejects_trailing_dot_without_name() {
        // A bare `secrets.` is malformed and not a whole-word context reference.
        assert!(!references_secret("secrets."));
    }

    #[test]
    fn references_secret_ignores_quoted_secrets_text() {
        // The word `secrets` inside a string literal is data, not a reference to
        // the `secrets` context.
        assert!(!references_secret(
            "contains(github.event.pull_request.title, 'secrets')"
        ));
        assert!(!references_secret("'secrets'"));
        // Real access adjacent to a string literal is still caught.
        assert!(references_secret("secrets[format('t-{0}', matrix.env)]"));
    }

    #[test]
    fn secret_tokens_returns_each_match() {
        let tokens = secret_tokens("uses ${{ secrets.A }} and ${{ secrets.B }}");
        // The iteration is across one expression string, so callers normally
        // call this per-block; here we exercise the regex over a longer string.
        assert_eq!(tokens, vec!["secrets.A".to_owned(), "secrets.B".to_owned()]);
    }

    #[test]
    fn secret_tokens_labels_whole_context_access() {
        // A named secret is reported precisely; whole-context / index access is
        // reported as the bare `secrets` context.
        assert_eq!(secret_tokens("secrets.FOO"), vec!["secrets.FOO".to_owned()]);
        assert_eq!(secret_tokens("toJSON(secrets)"), vec!["secrets".to_owned()]);
        assert_eq!(
            secret_tokens("secrets['MY-SECRET']"),
            vec!["secrets".to_owned()]
        );
    }
}
