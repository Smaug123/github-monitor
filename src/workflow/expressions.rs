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
    secret_regex().is_match(expression)
}

pub fn secret_tokens(expression: &str) -> impl Iterator<Item = String> + '_ {
    secret_regex().captures_iter(expression).map(|cap| {
        let ident = cap.get(1).expect("regex group 1 always present").as_str();
        format!("secrets.{ident}")
    })
}

fn secret_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?:^|[^A-Za-z0-9_.])secrets\.([A-Za-z_][A-Za-z0-9_]*)")
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
    fn references_secret_requires_dot_identifier() {
        assert!(!references_secret("secrets"));
        assert!(!references_secret("secrets."));
    }

    #[test]
    fn secret_tokens_returns_each_match() {
        let tokens: Vec<_> = secret_tokens("uses ${{ secrets.A }} and ${{ secrets.B }}").collect();
        // The iteration is across one expression string, so callers normally
        // call this per-block; here we exercise the regex over a longer string.
        assert_eq!(tokens, vec!["secrets.A".to_owned(), "secrets.B".to_owned()]);
    }
}
