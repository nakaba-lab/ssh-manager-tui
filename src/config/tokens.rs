//! Argument tokenizing & quoting for ssh_config values.
//!
//! OpenSSH groups arguments with double quotes; backslash is **not** an escape
//! character in ssh_config (unlike a shell), so bare Windows paths such as
//! `C:\Users\me\.ssh\id` round-trip untouched whether quoted or not.

/// Split an argument string into tokens, honoring double-quote grouping.
/// Backslashes are literal. Empty input yields an empty vector.
pub fn tokenize_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    out.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        out.push(cur);
    }
    out
}

/// Detokenize an argument string into a single displayable value: strip quote
/// grouping and collapse runs of whitespace to single spaces. Suitable for
/// single-valued options (HostName/User/Port/...) and for forward specs
/// (e.g. `8080 localhost:80`), which round-trip as a single editable string.
pub fn detok_value(args: &str) -> String {
    tokenize_args(args).join(" ")
}

/// Quote a single value for writing if it contains whitespace, `#`, or a quote.
/// (Backslashes are left as-is; embedded quotes are not escapable in ssh_config,
/// so we wrap best-effort.)
pub fn quote_if_needed(s: &str) -> String {
    let needs = s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '#' || c == '"');
    if needs {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_tokens() {
        assert_eq!(tokenize_args("web1 web* !web9"), ["web1", "web*", "!web9"]);
    }

    #[test]
    fn quoted_group() {
        assert_eq!(
            tokenize_args("\"C:\\path with space\\id\""),
            ["C:\\path with space\\id"]
        );
    }

    #[test]
    fn bare_windows_path_no_escape() {
        // Backslashes are literal, not escapes.
        assert_eq!(
            tokenize_args("C:\\Users\\me\\.ssh\\id_ed25519"),
            ["C:\\Users\\me\\.ssh\\id_ed25519"]
        );
    }

    #[test]
    fn forward_spec_two_tokens() {
        assert_eq!(detok_value("8080   localhost:80"), "8080 localhost:80");
    }

    #[test]
    fn empty() {
        assert!(tokenize_args("").is_empty());
        assert_eq!(detok_value(""), "");
    }

    #[test]
    fn quote_when_space() {
        assert_eq!(quote_if_needed("plain"), "plain");
        assert_eq!(quote_if_needed("C:\\a b\\id"), "\"C:\\a b\\id\"");
        assert_eq!(quote_if_needed(""), "\"\"");
    }
}
