#![allow(dead_code)]

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraUrlInfo {
    pub url: String,
    pub host: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StoredCredential {
    pub(super) username: Option<String>,
    pub(super) token: String,
}

pub fn parse_jira_url(raw: String) -> Result<JiraUrlInfo, String> {
    parse_jira_url_value(&raw)
}

pub(super) fn parse_jira_url_value(raw: &str) -> Result<JiraUrlInfo, String> {
    let value = raw.trim();
    let parsed = Url::parse(value).map_err(|_| "Jira URL 无法解析".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Jira URL 只允许使用 http 或 https".to_string());
    }

    let host = parsed
        .host_str()
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| "Jira URL 缺少 host".to_string())?
        .to_string();
    let segments = parsed
        .path_segments()
        .ok_or_else(|| "Jira URL 缺少 browse 路径".to_string())?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let browse_index = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("browse"))
        .ok_or_else(|| "Jira URL 必须包含 /browse/{issueKey}".to_string())?;
    let key = segments
        .get(browse_index + 1)
        .copied()
        .ok_or_else(|| "Jira URL 缺少 issue key".to_string())?;
    if !is_issue_key(key) {
        return Err("issue key 格式无效，应为 [A-Z][A-Z0-9]+-[0-9]+".to_string());
    }

    Ok(JiraUrlInfo {
        url: value.to_string(),
        host,
        key: key.to_string(),
    })
}

pub(super) fn is_issue_key(value: &str) -> bool {
    let Some((project, number)) = value.split_once('-') else {
        return false;
    };
    if project.len() < 2 || number.is_empty() {
        return false;
    }
    let mut chars = project.chars();
    chars.next().is_some_and(|first| first.is_ascii_uppercase())
        && chars.all(|character| character.is_ascii_uppercase() || character.is_ascii_digit())
        && number.chars().all(|character| character.is_ascii_digit())
}

pub fn jira_description_to_text(value: &Value) -> String {
    let mut output = String::new();
    append_jira_value(value, &mut output);
    normalize_plain_text(&output)
}

pub(super) fn append_jira_value(value: &Value, output: &mut String) {
    match value {
        Value::String(text) => output.push_str(&strip_markup(text)),
        Value::Array(values) => {
            for value in values {
                append_jira_value(value, output);
            }
        }
        Value::Object(object) => {
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                output.push_str(&strip_markup(text));
            } else if let Some(content) = object.get("content") {
                append_jira_value(content, output);
            }

            match object.get("type").and_then(Value::as_str) {
                Some("hardBreak") | Some("paragraph") | Some("heading") | Some("listItem")
                | Some("blockquote") | Some("codeBlock") => output.push('\n'),
                _ => {}
            }
        }
        _ => {}
    }
}

pub(super) fn strip_markup(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for character in value.chars() {
        if character == '<' {
            in_tag = true;
            tag.clear();
            continue;
        }
        if in_tag {
            if character == '>' {
                in_tag = false;
                let normalized = tag.trim().to_ascii_lowercase();
                if normalized.starts_with("br")
                    || normalized.starts_with("/p")
                    || normalized.starts_with("/li")
                    || normalized.starts_with("/div")
                    || normalized.starts_with("/h")
                    || normalized.starts_with("/pre")
                {
                    text.push('\n');
                }
            } else {
                tag.push(character);
            }
            continue;
        }
        text.push(character);
    }

    let text = text
        .replace("**", "")
        .replace("__", "")
        .replace('`', "")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    strip_markdown_links(&text)
}

pub(super) fn strip_markdown_links(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '[' {
            let mut label = String::new();
            while let Some(next) = chars.next() {
                if next == ']' {
                    break;
                }
                label.push(next);
            }
            if chars.peek() == Some(&'(') {
                chars.next();
                for next in chars.by_ref() {
                    if next == ')' {
                        break;
                    }
                }
                output.push_str(&label);
                continue;
            }
            output.push('[');
            output.push_str(&label);
            continue;
        }
        output.push(character);
    }
    output
}

pub(super) fn normalize_plain_text(value: &str) -> String {
    let mut lines = Vec::new();
    for line in value.replace('\r', "").lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

pub(super) fn jira_api_url(info: &JiraUrlInfo) -> Result<Url, String> {
    let mut parsed = Url::parse(&info.url).map_err(|_| "Jira URL 无法解析".to_string())?;
    let segments = parsed
        .path_segments()
        .ok_or_else(|| "Jira URL 缺少路径".to_string())?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let browse_index = segments
        .iter()
        .position(|segment| segment.eq_ignore_ascii_case("browse"))
        .ok_or_else(|| "Jira URL 缺少 browse 路径".to_string())?;
    let prefix = segments[..browse_index].join("/");
    let path = if prefix.is_empty() {
        format!("/rest/api/2/issue/{}", info.key)
    } else {
        format!("/{prefix}/rest/api/2/issue/{}", info.key)
    };
    parsed.set_path(&path);
    parsed.set_query(Some(
        "fields=summary,description,status,priority,issuetype,components,labels,environment",
    ));
    parsed.set_fragment(None);
    Ok(parsed)
}

pub(super) fn jira_credential(host: &str) -> Option<StoredCredential> {
    let entry = Entry::new(JIRA_CREDENTIAL_SERVICE, host).ok()?;
    let password = entry.get_password().ok()?;
    serde_json::from_str(&password).ok()
}

pub async fn fetch_jira_issue(url: String) -> Result<JiraIssueContext, String> {
    let info = parse_jira_url_value(&url)?;
    let endpoint = jira_api_url(&info)?;
    let mut request = crate::runtime_env::configured_http_client_builder()?
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法创建 Jira 请求: {error}"))?
        .get(endpoint)
        .header("accept", "application/json");

    if let Some(credentials) = jira_credential(&info.host) {
        request = if let Some(username) = credentials.username {
            request.basic_auth(username, Some(credentials.token))
        } else {
            request.bearer_auth(credentials.token)
        };
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("Jira 网络请求失败: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(jira_status_message(status));
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("Jira 返回数据无法解析: {error}"))?;
    let fields = body.get("fields").unwrap_or(&Value::Null);

    Ok(JiraIssueContext {
        url: info.url,
        host: info.host,
        key: info.key,
        summary: string_field(fields, "summary"),
        description: jira_description_to_text(fields.get("description").unwrap_or(&Value::Null)),
        status: nested_name(fields, "status"),
        priority: nested_name(fields, "priority"),
        issue_type: nested_name(fields, "issuetype"),
        components: array_names(fields, "components"),
        labels: array_strings(fields, "labels"),
        environment: optional_text(fields.get("environment")),
        updated_at: fields
            .get("updated")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

pub(super) fn jira_status_message(status: StatusCode) -> String {
    match status {
        StatusCode::UNAUTHORIZED => "Jira 认证失败（401），请在设置中保存凭据".to_string(),
        StatusCode::FORBIDDEN => "Jira 无权限访问此单（403）".to_string(),
        StatusCode::NOT_FOUND => "Jira 单不存在或当前账号无权查看（404）".to_string(),
        _ => format!("Jira 返回 HTTP {}", status.as_u16()),
    }
}

pub(super) fn string_field(fields: &Value, name: &str) -> String {
    fields
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(super) fn nested_name(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(super) fn array_names(fields: &Value, name: &str) -> Vec<String> {
    fields
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    value
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn array_strings(fields: &Value, name: &str) -> Vec<String> {
    fields
        .get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn optional_text(value: Option<&Value>) -> Option<String> {
    value
        .map(|value| jira_description_to_text(value))
        .filter(|value| !value.is_empty())
}

pub fn save_jira_credentials(
    host: String,
    username: Option<String>,
    token: String,
) -> Result<(), String> {
    let host = host.trim();
    if host.is_empty() || token.trim().is_empty() {
        return Err("Jira host 和凭据不能为空".to_string());
    }
    let payload = serde_json::to_string(&StoredCredential {
        username: username.filter(|value| !value.trim().is_empty()),
        token,
    })
    .map_err(|error| format!("无法保存 Jira 凭据: {error}"))?;
    Entry::new(JIRA_CREDENTIAL_SERVICE, host)
        .map_err(|error| format!("无法访问系统凭据库: {error}"))?
        .set_password(&payload)
        .map_err(|error| format!("无法保存 Jira 凭据到系统凭据库: {error}"))
}
