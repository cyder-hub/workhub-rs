use serde_json::{Map, Value, json};

use crate::{
    observability::schema::LogEvent,
    upstream::redaction::{REDACTED, current_env_secret_values, redact_text_with_secrets},
};

const MAX_STRING_CHARS: usize = 1_000;
const MAX_JSON_DEPTH: usize = 8;
const MAX_ARRAY_ITEMS: usize = 50;
const TRUNCATED: &str = "[truncated]";

#[derive(Debug, Default)]
struct SanitizeState {
    truncated: bool,
}

pub(crate) fn sanitize_event(event: &LogEvent) -> LogEvent {
    sanitize_event_with_secrets(event, current_env_secret_values())
}

pub(crate) fn sanitize_event_with_secrets<I, S>(event: &LogEvent, secrets: I) -> LogEvent
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let secrets = secrets
        .into_iter()
        .map(|secret| secret.as_ref().to_string())
        .collect::<Vec<_>>();
    let mut state = SanitizeState::default();
    let mut sanitized = event.clone();

    sanitized.message = sanitize_string(&sanitized.message, &secrets, &mut state);
    sanitized.error_message =
        sanitize_optional_string(sanitized.error_message, &secrets, &mut state);
    sanitized.cause_summary =
        sanitize_optional_string(sanitized.cause_summary, &secrets, &mut state);
    sanitized.cause_chain = sanitized
        .cause_chain
        .iter()
        .map(|value| sanitize_string(value, &secrets, &mut state))
        .collect();
    sanitized.impact = sanitize_optional_string(sanitized.impact, &secrets, &mut state);
    sanitized.remediation_evidence =
        sanitize_optional_string(sanitized.remediation_evidence, &secrets, &mut state);
    sanitized.related_line_hint =
        sanitize_optional_string(sanitized.related_line_hint, &secrets, &mut state);
    sanitized.fields = sanitized
        .fields
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                sanitize_value_for_key(key, value, 0, &secrets, &mut state),
            )
        })
        .collect();

    sanitized.redaction_applied = true;
    if state.truncated {
        sanitized.truncated = Some(true);
    }
    sanitized
}

fn sanitize_optional_string(
    value: Option<String>,
    secrets: &[String],
    state: &mut SanitizeState,
) -> Option<String> {
    value.map(|value| sanitize_string(&value, secrets, state))
}

fn sanitize_value_for_key(
    key: &str,
    value: &Value,
    depth: usize,
    secrets: &[String],
    state: &mut SanitizeState,
) -> Value {
    if is_sensitive_key(key) {
        return Value::String(REDACTED.to_string());
    }

    if is_business_body_key(key) {
        state.truncated = true;
        return business_body_summary(value);
    }

    sanitize_value(value, depth, secrets, state)
}

fn sanitize_value(
    value: &Value,
    depth: usize,
    secrets: &[String],
    state: &mut SanitizeState,
) -> Value {
    if depth > MAX_JSON_DEPTH {
        state.truncated = true;
        return Value::String(TRUNCATED.to_string());
    }

    match value {
        Value::String(value) => Value::String(sanitize_string(value, secrets, state)),
        Value::Array(values) => {
            let mut sanitized = values
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|value| sanitize_value(value, depth + 1, secrets, state))
                .collect::<Vec<_>>();
            if values.len() > MAX_ARRAY_ITEMS {
                state.truncated = true;
                sanitized.push(json!({
                    "truncated_items": values.len() - MAX_ARRAY_ITEMS,
                }));
            }
            Value::Array(sanitized)
        }
        Value::Object(object) => {
            let mut sanitized = Map::new();
            for (key, value) in object {
                sanitized.insert(
                    key.clone(),
                    sanitize_value_for_key(key, value, depth + 1, secrets, state),
                );
            }
            Value::Object(sanitized)
        }
        value => value.clone(),
    }
}

fn sanitize_string(value: &str, secrets: &[String], state: &mut SanitizeState) -> String {
    let redacted = redact_text_with_secrets(value, secrets);
    if redacted.chars().count() <= MAX_STRING_CHARS {
        return redacted;
    }

    state.truncated = true;
    let mut truncated = redacted.chars().take(MAX_STRING_CHARS).collect::<String>();
    truncated.push_str(TRUNCATED);
    truncated
}

fn is_sensitive_key(key: &str) -> bool {
    let key = normalized_key(key);
    if matches!(
        key.as_str(),
        "page_token" | "next_page_token" | "nextpagetoken"
    ) {
        return false;
    }

    [
        "authorization",
        "cookie",
        "set_cookie",
        "password",
        "secret",
        "token",
        "api_key",
        "api_token",
        "access_key",
        "private_key",
        "personal_token",
        "pat",
        "signature",
        "session",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn is_business_body_key(key: &str) -> bool {
    let key = normalized_key(key);
    [
        "body",
        "content",
        "description",
        "attachment_body",
        "request_body",
        "response_body",
        "diff",
        "patch",
        "raw_text",
    ]
    .iter()
    .any(|body_key| key == *body_key || key.ends_with(&format!("_{body_key}")))
}

fn normalized_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .replace(['.', '-', ' '], "_")
}

fn business_body_summary(value: &Value) -> Value {
    match value {
        Value::String(value) => json!({
            "omitted": true,
            "reason": "business_body",
            "original_chars": value.chars().count(),
        }),
        Value::Array(values) => json!({
            "omitted": true,
            "reason": "business_body",
            "original_items": values.len(),
        }),
        Value::Object(object) => json!({
            "omitted": true,
            "reason": "business_body",
            "original_keys": object.len(),
        }),
        Value::Null => json!({
            "omitted": true,
            "reason": "business_body",
            "original_kind": "null",
        }),
        Value::Bool(_) => json!({
            "omitted": true,
            "reason": "business_body",
            "original_kind": "bool",
        }),
        Value::Number(_) => json!({
            "omitted": true,
            "reason": "business_body",
            "original_kind": "number",
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::observability::schema::{LogKind, LogLevel, RuntimeMode};

    fn event_with_fields(fields: Vec<(&str, Value)>) -> LogEvent {
        let mut event = LogEvent::new_at(
            "2026-06-22T06:03:12.481Z",
            LogLevel::Info,
            LogKind::Diagnostic,
            "diagnostic.payload",
            "Authorization: Bearer message-token env-secret",
            "workhub::observability",
            RuntimeMode::Test,
            "test",
            "run_test",
            1,
        );
        event.error_message = Some("failed token=message-token".to_string());
        for (key, value) in fields {
            event.fields.insert(key.to_string(), value);
        }
        event
    }

    #[test]
    fn sanitize_event_redacts_secret_values_and_sensitive_keys() {
        let event = event_with_fields(vec![
            (
                "http.url",
                json!("https://jira.example/rest?token=query-secret&client=visible"),
            ),
            (
                "headers",
                json!({
                    "Authorization": "Bearer header-secret",
                    "Cookie": "session=cookie-secret",
                    "X-Trace": "visible",
                }),
            ),
            (
                "nested",
                json!({"api_token": "nested-secret", "visible": "env-secret"}),
            ),
        ]);

        let sanitized = sanitize_event_with_secrets(&event, ["env-secret", "message-token"]);
        let output = serde_json::to_string(&sanitized).unwrap();

        assert!(sanitized.redaction_applied);
        assert!(output.contains("<redacted>"));
        assert!(output.contains("visible"));
        assert!(!output.contains("message-token"));
        assert!(!output.contains("env-secret"));
        assert!(!output.contains("query-secret"));
        assert!(!output.contains("header-secret"));
        assert!(!output.contains("cookie-secret"));
        assert!(!output.contains("nested-secret"));
        assert_eq!(sanitized.fields["headers"]["X-Trace"], "visible");
    }

    #[test]
    fn sanitize_event_omits_business_body_content() {
        let event = event_with_fields(vec![
            (
                "description",
                json!("customer reported secret business text"),
            ),
            ("attachment_body", json!("binary-ish attachment body")),
            ("summary", json!("safe summary")),
        ]);

        let sanitized = sanitize_event_with_secrets(&event, std::iter::empty::<&str>());
        let output = serde_json::to_string(&sanitized).unwrap();

        assert_eq!(sanitized.fields["description"]["omitted"], true);
        assert_eq!(sanitized.fields["description"]["reason"], "business_body");
        assert_eq!(sanitized.fields["attachment_body"]["omitted"], true);
        assert_eq!(sanitized.fields["summary"], "safe summary");
        assert!(!output.contains("customer reported secret business text"));
        assert!(!output.contains("binary-ish attachment body"));
    }

    #[test]
    fn sanitize_event_truncates_long_strings_and_deep_json() {
        let deep = json!({
            "a": {"b": {"c": {"d": {"e": {"f": {"g": {"h": {"i": {"j": "too deep"}}}}}}}}},
        });
        let event = event_with_fields(vec![
            ("long", json!("x".repeat(MAX_STRING_CHARS + 10))),
            ("deep", deep),
            (
                "array",
                json!((0..(MAX_ARRAY_ITEMS + 2)).collect::<Vec<_>>()),
            ),
        ]);

        let sanitized = sanitize_event_with_secrets(&event, std::iter::empty::<&str>());
        let output = serde_json::to_string(&sanitized).unwrap();

        assert_eq!(sanitized.truncated, Some(true));
        assert!(
            sanitized.fields["long"]
                .as_str()
                .unwrap()
                .ends_with(TRUNCATED)
        );
        assert!(output.contains(TRUNCATED));
        assert!(output.contains("truncated_items"));
    }

    #[test]
    fn pagination_tokens_are_not_treated_as_auth_tokens() {
        let event = event_with_fields(vec![
            ("page_token", json!("page-2")),
            ("nextPageToken", json!("page-3")),
            ("personal_token", json!("real-secret")),
        ]);

        let sanitized = sanitize_event_with_secrets(&event, std::iter::empty::<&str>());

        assert_eq!(sanitized.fields["page_token"], "page-2");
        assert_eq!(sanitized.fields["nextPageToken"], "page-3");
        assert_eq!(sanitized.fields["personal_token"], REDACTED);
    }
}
