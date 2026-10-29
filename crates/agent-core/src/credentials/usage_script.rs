use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use url::{Host, Url};

/// CCSwitch-compatible usage script result. Fields are intentionally optional:
/// relay services expose different subsets of balance, token and plan data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageScriptResult {
    #[serde(rename = "isValid", default)]
    pub is_valid: Option<bool>,
    #[serde(rename = "invalidMessage", default)]
    pub invalid_message: Option<String>,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub total: Option<f64>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(rename = "planName", default)]
    pub plan_name: Option<String>,
    #[serde(default)]
    pub extra: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RequestConfig {
    url: String,
    method: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

/// Execute the same two-stage script contract used by CCSwitch:
///
/// ```js
/// ({
///   request: { url, method, headers, body },
///   extractor: (response) => ({ remaining, unit })
/// })
/// ```
///
/// The JavaScript runtime is created only while extracting request/response
/// data. It is dropped before the network await, so the core remains Send and
/// does not hold a JS VM across an async suspension.
pub async fn execute_usage_script(
    script_code: &str,
    api_key: &str,
    base_url: &str,
    timeout_secs: u64,
    access_token: Option<&str>,
    user_id: Option<&str>,
) -> Result<Vec<UsageScriptResult>, String> {
    if script_code.trim().is_empty() {
        return Err("用量脚本为空".to_string());
    }
    let script = build_script_with_vars(script_code, api_key, base_url, access_token, user_id);
    let request_json = evaluate_request(&script)?;
    let request: RequestConfig = serde_json::from_str(&request_json)
        .map_err(|error| format!("用量脚本 request 配置无效: {error}"))?;
    validate_request_url(&request.url, base_url)?;

    let method = request
        .method
        .parse::<reqwest::Method>()
        .map_err(|_| format!("用量脚本使用了不支持的 HTTP 方法: {}", request.method))?;
    let client = crate::network::http_client_builder()?
        .timeout(std::time::Duration::from_secs(timeout_secs.clamp(2, 30)))
        .build()
        .map_err(|error| format!("无法创建用量脚本请求: {error}"))?;
    let mut builder = client.request(method, &request.url);
    for (name, value) in request.headers {
        builder = builder.header(name, value);
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }
    let response = builder
        .send()
        .await
        .map_err(|error| format!("用量脚本请求失败: {error}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| format!("读取用量脚本响应失败: {error}"))?;
    if !status.is_success() {
        let preview = text.chars().take(240).collect::<String>();
        return Err(format!(
            "用量脚本接口返回 HTTP {}: {preview}",
            status.as_u16()
        ));
    }

    evaluate_extractor(&script, &text)
}

fn evaluate_request(script: &str) -> Result<String, String> {
    let runtime = Runtime::new().map_err(|error| format!("创建用量脚本运行时失败: {error}"))?;
    let context =
        Context::full(&runtime).map_err(|error| format!("创建用量脚本上下文失败: {error}"))?;
    context.with(|ctx| {
        let config: rquickjs::Object = ctx
            .eval(script.to_string())
            .map_err(|error| format!("解析用量脚本失败: {error}"))?;
        let request: rquickjs::Object = config
            .get("request")
            .map_err(|error| format!("用量脚本缺少 request 配置: {error}"))?;
        ctx.json_stringify(request)
            .map_err(|error| format!("序列化用量 request 失败: {error}"))?
            .ok_or_else(|| "用量 request 序列化结果为空".to_string())?
            .get()
            .map_err(|error| format!("读取用量 request 配置失败: {error}"))
    })
}

fn evaluate_extractor(script: &str, response_text: &str) -> Result<Vec<UsageScriptResult>, String> {
    let runtime = Runtime::new().map_err(|error| format!("创建用量脚本运行时失败: {error}"))?;
    let context =
        Context::full(&runtime).map_err(|error| format!("创建用量脚本上下文失败: {error}"))?;
    context.with(|ctx| {
        let config: rquickjs::Object = ctx
            .eval(script.to_string())
            .map_err(|error| format!("重新解析用量脚本失败: {error}"))?;
        let extractor: Function = config
            .get("extractor")
            .map_err(|error| format!("用量脚本缺少 extractor 函数: {error}"))?;
        let response: rquickjs::Value = ctx
            .json_parse(response_text)
            .map_err(|error| format!("用量脚本响应不是有效 JSON: {error}"))?;
        let result: rquickjs::Value = extractor
            .call((response,))
            .map_err(|error| format!("执行用量 extractor 失败: {error}"))?;
        let json: String = ctx
            .json_stringify(result)
            .map_err(|error| format!("序列化用量 extractor 结果失败: {error}"))?
            .ok_or_else(|| "用量 extractor 返回为空".to_string())?
            .get()
            .map_err(|error| format!("读取用量 extractor 结果失败: {error}"))?;
        let value: Value = serde_json::from_str(&json)
            .map_err(|error| format!("用量 extractor 结果不是有效 JSON: {error}"))?;
        parse_results(value)
    })
}

fn parse_results(value: Value) -> Result<Vec<UsageScriptResult>, String> {
    let values = match value {
        Value::Array(values) => values,
        value => vec![value],
    };
    if values.is_empty() {
        return Err("用量 extractor 返回了空数组".to_string());
    }
    values
        .into_iter()
        .map(|value| {
            let object = value
                .as_object()
                .cloned()
                .ok_or_else(|| "用量 extractor 必须返回对象或对象数组".to_string())?;
            let result: UsageScriptResult = serde_json::from_value(value)
                .map_err(|error| format!("用量 extractor 字段格式无效: {error}"))?;
            if result.is_valid == Some(false) || result.invalid_message.is_some() {
                return Ok(result);
            }
            for key in ["remaining", "total", "used"] {
                if let Some(value) = object.get(key) {
                    if !value.is_null() && value.as_f64().is_none() {
                        return Err(format!("用量 extractor 的 {key} 必须是数字或 null"));
                    }
                }
            }
            if let Some(unit) = &result.unit {
                if unit.len() > 64 {
                    return Err("用量 extractor 的 unit 过长".to_string());
                }
            }
            Ok(result)
        })
        .collect()
}

fn build_script_with_vars(
    script: &str,
    api_key: &str,
    base_url: &str,
    access_token: Option<&str>,
    user_id: Option<&str>,
) -> String {
    script
        .replace("{{apiKey}}", api_key)
        .replace("{{baseUrl}}", base_url)
        .replace("{{accessToken}}", access_token.unwrap_or_default())
        .replace("{{userId}}", user_id.unwrap_or_default())
}

fn validate_request_url(request_url: &str, base_url: &str) -> Result<(), String> {
    let request = Url::parse(request_url).map_err(|error| format!("用量脚本 URL 无效: {error}"))?;
    let base = Url::parse(base_url).map_err(|error| format!("Provider Base URL 无效: {error}"))?;
    let request_loopback = request.host().is_some_and(is_loopback_host);
    if request.scheme() != "https" && !request_loopback {
        return Err("用量脚本请求必须使用 HTTPS（本机地址除外）".to_string());
    }
    if request.host_str() != base.host_str()
        || request.port_or_known_default() != base.port_or_known_default()
    {
        return Err("用量脚本请求必须与 Provider Base URL 同源".to_string());
    }
    Ok(())
}

fn is_loopback_host(host: Host<&str>) -> bool {
    match host {
        Host::Domain(value) => value.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(value) => value.is_loopback(),
        Host::Ipv6(value) => value.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_all_ccswitch_variables() {
        let script = "{{baseUrl}} {{apiKey}} {{accessToken}} {{userId}}";
        let value = build_script_with_vars(
            script,
            "key",
            "https://relay.test/v1",
            Some("token"),
            Some("u1"),
        );
        assert_eq!(value, "https://relay.test/v1 key token u1");
    }

    #[test]
    fn accepts_single_and_array_results() {
        let one = parse_results(serde_json::json!({"remaining": 10.0, "unit": "USD"})).unwrap();
        assert_eq!(one.len(), 1);
        let many = parse_results(serde_json::json!([
            {"remaining": 10.0},
            {"remaining": 20.0}
        ]))
        .unwrap();
        assert_eq!(many.len(), 2);
    }

    #[test]
    fn rejects_cross_origin_script_requests() {
        assert!(
            validate_request_url("https://other.test/api/user/self", "https://relay.test/v1")
                .is_err()
        );
    }
}
