use super::*;

pub(super) fn provider_error_message(body: &str, fallback: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        let message = body.trim();
        return if message.is_empty() {
            fallback.to_string()
        } else {
            redact_response_message(message)
        };
    };

    provider_error_value_message(&value, fallback)
}

pub(super) fn provider_error_value_message(value: &Value, fallback: &str) -> String {
    let error = value
        .get("error")
        .filter(|error| !error.is_null())
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("error"))
        })
        .filter(|error| !error.is_null());
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| error.and_then(Value::as_str))
        .or_else(|| value.get("message").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("response")
                .and_then(|response| response.get("message"))
                .and_then(Value::as_str)
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(fallback);

    let code = error
        .and_then(|error| error.get("code"))
        .or_else(|| value.get("code"))
        .and_then(value_scalar_string);
    let error_type = error
        .and_then(|error| error.get("type"))
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .filter(|error_type| !error_type.is_empty());

    let mut detail = message.to_string();
    if let Some(error_type) = error_type {
        detail.push_str(&format!(" (type: {error_type}"));
        if let Some(code) = code.as_deref() {
            detail.push_str(&format!(", code: {code}"));
        }
        detail.push(')');
    } else if let Some(code) = code {
        detail.push_str(&format!(" (code: {code})"));
    }
    redact_response_message(&detail)
}

pub(super) fn value_scalar_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
}

pub(super) fn redact_response_message(message: &str) -> String {
    message
        .replace('\n', " ")
        .replace('\r', " ")
        .chars()
        .take(512)
        .collect()
}
