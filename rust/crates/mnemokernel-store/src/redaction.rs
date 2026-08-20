use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::cmp::Reverse;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct RedactionSpan {
    pub category: String,
    pub start_char: usize,
    pub end_char: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RedactionResult {
    pub content: String,
    pub spans: Vec<RedactionSpan>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    start: usize,
    end: usize,
    category: &'static str,
    priority: u8,
}

fn regex(pattern: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static redaction regex must compile"))
}

fn add_matches(
    candidates: &mut Vec<Candidate>,
    expression: &Regex,
    content: &str,
    capture: usize,
    category: &'static str,
    priority: u8,
) {
    for captures in expression.captures_iter(content) {
        if let Some(found) = captures.get(capture) {
            candidates.push(Candidate {
                start: found.start(),
                end: found.end(),
                category,
                priority,
            });
        }
    }
}

fn high_entropy(candidate: &str) -> bool {
    let bytes = candidate.as_bytes();
    if bytes.len() < 40 {
        return false;
    }
    let has_lower = bytes.iter().any(u8::is_ascii_lowercase);
    let has_upper = bytes.iter().any(u8::is_ascii_uppercase);
    let has_digit = bytes.iter().any(u8::is_ascii_digit);
    if usize::from(has_lower) + usize::from(has_upper) + usize::from(has_digit) < 3 {
        return false;
    }
    let mut counts = [0_u32; 256];
    for byte in bytes {
        counts[usize::from(*byte)] += 1;
    }
    let length = bytes.len() as f64;
    let entropy = counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = f64::from(count) / length;
            -probability * probability.log2()
        })
        .sum::<f64>();
    entropy >= 4.2
}

pub(crate) fn redact(content: &str) -> RedactionResult {
    static PEM: OnceLock<Regex> = OnceLock::new();
    static JWT: OnceLock<Regex> = OnceLock::new();
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static LABELED_SECRET: OnceLock<Regex> = OnceLock::new();
    static URI_PASSWORD: OnceLock<Regex> = OnceLock::new();
    static PASSWORD_PARAMETER: OnceLock<Regex> = OnceLock::new();
    static PREFIXED_KEY: OnceLock<Regex> = OnceLock::new();
    static TOKEN: OnceLock<Regex> = OnceLock::new();

    let mut candidates = Vec::new();
    add_matches(
        &mut candidates,
        regex(
            r"(?s)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----",
            &PEM,
        ),
        content,
        0,
        "private_key",
        0,
    );
    add_matches(
        &mut candidates,
        regex(
            r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{8,}\b",
            &JWT,
        ),
        content,
        0,
        "jwt",
        1,
    );
    add_matches(
        &mut candidates,
        regex(r"(?i)\bbearer[ \t]+([A-Za-z0-9._~+/=-]{16,})", &BEARER),
        content,
        1,
        "bearer_token",
        1,
    );
    add_matches(
        &mut candidates,
        regex(
            r"(?i)\b(?:api[_-]?key|secret|token)[ \t]*[:=][ \t]*([A-Za-z0-9._~+/=-]{16,})",
            &LABELED_SECRET,
        ),
        content,
        1,
        "api_key",
        1,
    );
    add_matches(
        &mut candidates,
        regex(
            r"(?i)\b[a-z][a-z0-9+.-]*://[^:\s/@]+:([^@\s/]+)@",
            &URI_PASSWORD,
        ),
        content,
        1,
        "connection_password",
        1,
    );
    add_matches(
        &mut candidates,
        regex(
            r"(?i)\b(?:password|passwd|pwd)[ \t]*=[ \t]*([^;\s&]{4,})",
            &PASSWORD_PARAMETER,
        ),
        content,
        1,
        "password",
        1,
    );
    add_matches(
        &mut candidates,
        regex(
            r"\b(?:sk|pk|ghp|github_pat|xox[baprs])[-_][A-Za-z0-9_-]{16,}\b",
            &PREFIXED_KEY,
        ),
        content,
        0,
        "api_key",
        2,
    );
    for found in regex(r"\b[A-Za-z0-9_-]{40,}\b", &TOKEN).find_iter(content) {
        if high_entropy(found.as_str()) {
            candidates.push(Candidate {
                start: found.start(),
                end: found.end(),
                category: "high_entropy_token",
                priority: 3,
            });
        }
    }

    candidates.sort_by_key(|candidate| {
        (
            candidate.start,
            candidate.priority,
            Reverse(candidate.end - candidate.start),
        )
    });
    let mut selected = Vec::new();
    let mut cursor = 0_usize;
    for candidate in candidates {
        if candidate.start >= cursor && candidate.start < candidate.end {
            cursor = candidate.end;
            selected.push(candidate);
        }
    }

    let mut redacted = String::with_capacity(content.len());
    let mut spans = Vec::with_capacity(selected.len());
    let mut byte_cursor = 0_usize;
    for candidate in selected {
        redacted.push_str(&content[byte_cursor..candidate.start]);
        redacted.push_str("[REDACTED:");
        redacted.push_str(candidate.category);
        redacted.push(']');
        spans.push(RedactionSpan {
            category: candidate.category.to_owned(),
            start_char: content[..candidate.start].chars().count(),
            end_char: content[..candidate.end].chars().count(),
        });
        byte_cursor = candidate.end;
    }
    redacted.push_str(&content[byte_cursor..]);
    RedactionResult {
        content: redacted,
        spans,
    }
}

pub(crate) fn sanitize_metadata(metadata: &Value) -> Value {
    let Some(source) = metadata.as_object() else {
        return Value::Object(Map::new());
    };
    let mut sanitized = Map::new();
    if let Some(value) = source.get("unified_origin").and_then(Value::as_str) {
        sanitized.insert(
            "unified_origin".into(),
            Value::String(value.chars().take(512).collect()),
        );
    }
    if let Some(values) = source.get("component_types").and_then(Value::as_array) {
        let values = values
            .iter()
            .filter_map(Value::as_str)
            .take(32)
            .map(|value| Value::String(value.chars().take(64).collect()))
            .collect();
        sanitized.insert("component_types".into(), Value::Array(values));
    }
    for key in ["reply_state", "mention_state"] {
        if let Some(value @ ("present" | "missing")) = source.get(key).and_then(Value::as_str) {
            sanitized.insert(key.into(), Value::String(value.into()));
        }
    }
    if let Some(value) = source.get("result_content_type").and_then(Value::as_str) {
        sanitized.insert(
            "result_content_type".into(),
            Value::String(value.chars().take(64).collect()),
        );
    }
    Value::Object(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_supported_secret_classes_without_copying_secret_to_map() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let pem = "-----BEGIN PRIVATE KEY-----\nABCDEF0123456789\n-----END PRIVATE KEY-----";
        let content = format!(
            "Bearer abcdefghijklmnopqrstuvwxyz123456 api_key=sk-ABCDEFGHIJKLMNOP123456 {jwt} postgres://alice:secret-pass@db/x {pem}"
        );
        let result = redact(&content);
        for secret in [
            "abcdefghijklmnopqrstuvwxyz123456",
            "sk-ABCDEFGHIJKLMNOP123456",
            jwt,
            "secret-pass",
            pem,
        ] {
            assert!(!result.content.contains(secret));
            assert!(
                !serde_json::to_string(&result.spans)
                    .unwrap()
                    .contains(secret)
            );
        }
        assert!(result.spans.len() >= 5);
    }

    #[test]
    fn unicode_offsets_are_character_offsets_and_overlaps_are_single() {
        let result = redact("前缀 Bearer abcdefghijklmnopqrstuvwxyz123456 后缀");
        assert_eq!(result.spans.len(), 1);
        assert_eq!(result.spans[0].start_char, 10);
        assert!(result.content.starts_with("前缀 Bearer [REDACTED:"));
    }

    #[test]
    fn metadata_uses_allowlist_and_bounds_values() {
        let value = sanitize_metadata(&json!({
            "unified_origin": "origin",
            "component_types": ["plain", "at"],
            "reply_state": "present",
            "unknown_platform_object": {"secret": "do-not-copy"}
        }));
        assert_eq!(value["unified_origin"], "origin");
        assert!(value.get("unknown_platform_object").is_none());
    }
}
