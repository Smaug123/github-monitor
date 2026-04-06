use regex::Regex;

pub(super) fn branch_matches_filters(filters: &[String], branch: &str) -> bool {
    if filters.is_empty() {
        return true;
    }

    let mut matched = false;
    let mut saw_positive_pattern = false;

    for filter in filters {
        let (negated, pattern) = if let Some(pattern) = filter.strip_prefix('!') {
            (true, pattern)
        } else {
            saw_positive_pattern = true;
            (false, filter.as_str())
        };

        if branch_pattern_matches(pattern, branch) {
            matched = !negated;
        }
    }

    saw_positive_pattern && matched
}

pub(super) fn branch_pattern_matches(pattern: &str, branch: &str) -> bool {
    branch_pattern_regex(pattern).is_some_and(|regex| regex.is_match(branch))
}

fn branch_pattern_regex(pattern: &str) -> Option<Regex> {
    let body = github_pattern_to_regex(pattern)?;
    Regex::new(&format!("^{body}$")).ok()
}

fn github_pattern_to_regex(pattern: &str) -> Option<String> {
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut regex = String::new();
    let mut index = 0usize;
    let mut previous_token_is_quantifiable = false;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let escaped = chars.get(index + 1).copied().unwrap_or('\\');
                push_escaped_char(&mut regex, escaped);
                previous_token_is_quantifiable = true;
                index += if index + 1 < chars.len() { 2 } else { 1 };
            }
            '*' => {
                if chars.get(index + 1) == Some(&'*') {
                    regex.push_str(".*");
                    index += 2;
                } else {
                    regex.push_str("[^/]*");
                    index += 1;
                }
                previous_token_is_quantifiable = false;
            }
            '?' | '+' => {
                if !previous_token_is_quantifiable {
                    return None;
                }

                regex.push(chars[index]);
                previous_token_is_quantifiable = false;
                index += 1;
            }
            '[' => {
                let (class_regex, next_index) = parse_character_class(&chars, index)?;
                regex.push_str(&class_regex);
                previous_token_is_quantifiable = true;
                index = next_index;
            }
            ch => {
                push_escaped_char(&mut regex, ch);
                previous_token_is_quantifiable = true;
                index += 1;
            }
        }
    }

    Some(regex)
}

fn parse_character_class(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut regex = String::from("[");
    let mut index = start + 1;
    let mut saw_content = false;

    if chars.get(index) == Some(&'!') {
        regex.push('^');
        index += 1;
    }

    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let escaped = chars.get(index + 1).copied().unwrap_or('\\');
                push_regex_class_literal(&mut regex, escaped);
                saw_content = true;
                index += if index + 1 < chars.len() { 2 } else { 1 };
            }
            ']' if saw_content => {
                regex.push(']');
                return Some((regex, index + 1));
            }
            '[' | '^' => {
                push_regex_class_literal(&mut regex, chars[index]);
                saw_content = true;
                index += 1;
            }
            '-' => {
                regex.push('-');
                saw_content = true;
                index += 1;
            }
            ch => {
                regex.push(ch);
                saw_content = true;
                index += 1;
            }
        }
    }

    None
}

fn push_escaped_char(regex: &mut String, ch: char) {
    regex.push_str(&regex::escape(&ch.to_string()));
}

fn push_regex_class_literal(regex: &mut String, ch: char) {
    match ch {
        '\\' | '[' | ']' | '^' | '-' => {
            regex.push('\\');
            regex.push(ch);
        }
        _ => regex.push(ch),
    }
}
