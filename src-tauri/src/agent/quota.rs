use super::*;

pub(super) async fn query_custom_quota(
    request: &AgentProviderCheckRequest,
    urls: &agent_core::ProviderQuotaUrls,
    api_key: &str,
    access_token: Option<&str>,
    checked_at: &str,
) -> Result<AgentQuotaView, String> {
    let client = crate::runtime_env::configured_http_client_builder()?
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法创建查询请求")?;
    let mut queries = Vec::new();
    for (label, endpoint) in [("余额", &urls.balance_url), ("消费", &urls.spending_url)] {
        if let Some(endpoint) = endpoint {
            queries.push(query_custom_quota_endpoint(
                &client,
                request,
                endpoint,
                api_key,
                access_token,
                label,
            ));
        }
    }
    let mut view = AgentQuotaView {
        status: "unsupported".into(),
        source: Some("custom".into()),
        currency: request.currency.clone(),
        checked_at: checked_at.into(),
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        ..Default::default()
    };
    let mut notices = Vec::new();
    for (label, result) in futures_util::future::join_all(queries).await {
        match result {
            Ok(values) => {
                if view
                    .currency
                    .as_ref()
                    .zip(values.currency.as_ref())
                    .is_some_and(|(left, right)| !left.eq_ignore_ascii_case(right))
                {
                    notices.push(format!("{label}接口币种不同，未合并"));
                    continue;
                }
                view.currency = view.currency.or(values.currency);
                if label == "余额" {
                    view.remaining_amount = values.remaining_amount;
                    view.remaining_tokens = values.remaining_tokens;
                    view.total_amount = values.total_amount;
                    if urls.spending_url.is_none() {
                        view.used_amount = values.used_amount;
                    }
                } else {
                    view.used_amount = values.used_amount;
                }
            }
            Err(error) => notices.push(format!("{label}查询：{error}")),
        }
    }
    let available = view.remaining_amount.is_some()
        || view.remaining_tokens.is_some()
        || view.used_amount.is_some();
    view.status = if available {
        "available"
    } else if notices.is_empty() {
        "unsupported"
    } else {
        "error"
    }
    .into();
    view.unit = view.currency.clone();
    view.message = if notices.is_empty() {
        if available {
            "已读取自定义查询接口"
        } else {
            "接口未返回可识别的余额或消费字段"
        }
        .into()
    } else {
        notices.join("；")
    };
    Ok(view)
}

pub(super) async fn query_custom_quota_endpoint(
    client: &Client,
    request: &AgentProviderCheckRequest,
    endpoint: &str,
    api_key: &str,
    access_token: Option<&str>,
    label: &'static str,
) -> (&'static str, Result<RelayQuotaValues, String>) {
    let result = async {
        let url = agent_core::ProviderQuotaUrls::resolve(&request.base_url, endpoint)?;
        let is_new_api = url.path().trim_end_matches('/').ends_with("/api/user/self");
        let base = Url::parse(&request.base_url).map_err(|_| "Base URL 无效")?;
        let token = custom_quota_token(&base, &url, api_key, access_token)?;
        let mut query = client.get(url).header("Accept", "application/json");
        if !token.is_empty() {
            query = query.bearer_auth(token);
        }
        let mut response = query.send().await.map_err(|_| "接口网络请求失败或超时")?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "读取接口响应失败")? {
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err("接口响应超过 1MB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes).map_err(|_| "接口没有返回有效 JSON")?;
        parse_relay_quota(&body, is_new_api)
            .ok_or_else(|| "接口未返回可识别的余额或消费字段".into())
    }
    .await;
    (label, result)
}

pub(super) fn custom_quota_token<'a>(
    base: &Url,
    endpoint: &Url,
    api_key: &'a str,
    access_token: Option<&'a str>,
) -> Result<&'a str, String> {
    if let Some(token) = access_token.filter(|token| !token.trim().is_empty()) {
        return Ok(token.trim());
    }
    if base.origin() == endpoint.origin() {
        return Ok(api_key);
    }
    Err("跨域接口需配置独立查询令牌，不会发送模型 API Key".into())
}

pub(super) async fn query_usage_script(
    request: &AgentProviderCheckRequest,
    base_url: &str,
    api_key: &str,
    access_token: &str,
    user_id: Option<&str>,
    checked_at: &str,
    script: &str,
) -> Result<AgentQuotaView, String> {
    let results = agent_core::execute_usage_script(
        script,
        api_key,
        base_url,
        10,
        Some(access_token),
        user_id,
    )
    .await
    .map_err(|error| format!("自定义额度脚本执行失败：{error}"))?;
    let result = results
        .iter()
        .find(|value| {
            value.is_valid != Some(false)
                && (value.remaining.is_some() || value.total.is_some() || value.used.is_some())
        })
        .or_else(|| results.first())
        .cloned()
        .unwrap_or_default();
    if result.is_valid == Some(false) {
        return Ok(AgentQuotaView {
            status: "error".to_string(),
            source: Some("自定义额度脚本".to_string()),
            message: result
                .invalid_message
                .unwrap_or_else(|| "额度脚本返回无效结果".to_string()),
            checked_at: checked_at.to_string(),
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            ..Default::default()
        });
    }
    let remaining = result.remaining.or_else(|| {
        result
            .total
            .zip(result.used)
            .map(|(total, used)| (total - used).max(0.0))
    });
    let unit = result.unit.clone().filter(|value| !value.trim().is_empty());
    let is_token_unit = unit
        .as_deref()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "token" | "tokens" | "token(s)" | "次"
            )
        })
        .unwrap_or(false);
    let remaining_tokens = is_token_unit
        .then(|| remaining.and_then(|value| u64::try_from(value.max(0.0).round() as i128).ok()))
        .flatten();
    let remaining_amount = (!is_token_unit).then_some(remaining).flatten();
    let plan = result
        .plan_name
        .clone()
        .filter(|value| !value.trim().is_empty());
    let extra = result
        .extra
        .clone()
        .filter(|value| !value.trim().is_empty());
    let mut message = "已通过自定义额度脚本读取中转服务返回值".to_string();
    if let Some(plan_name) = &plan {
        message.push_str(&format!("；套餐：{plan_name}"));
    }
    if let Some(extra) = &extra {
        message.push_str(&format!("；{extra}"));
    }
    let status = if remaining_amount.is_some() || remaining_tokens.is_some() {
        "available"
    } else {
        "unsupported"
    };
    Ok(AgentQuotaView {
        status: status.to_string(),
        source: Some("自定义额度脚本".to_string()),
        remaining_amount,
        remaining_tokens,
        total_amount: result.total,
        used_amount: result.used,
        unit: unit.clone(),
        plan_name: plan,
        extra,
        currency: if is_token_unit {
            request.currency.clone()
        } else {
            unit.or(request.currency.clone())
        },
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        message,
        checked_at: checked_at.to_string(),
    })
}

/// 中转服务没有统一的余额协议。CCSwitch 对这类服务使用可配置的用量脚本，
/// 其中最常见的 New API 约定是 `/api/user/self`，部分服务则沿用 `/user/balance`。
/// 这里仅尝试明确的只读 GET 端点，并且只在响应中存在余额字段时报告额度。
pub(super) async fn query_relay_quota(
    request: &AgentProviderCheckRequest,
    base_url: &str,
    api_key: &str,
    checked_at: &str,
) -> Result<Option<AgentQuotaView>, String> {
    if api_key.trim().is_empty() || is_known_official_provider_base_url(base_url) {
        return Ok(None);
    }
    let endpoints = relay_quota_endpoints(base_url);
    if endpoints.is_empty() {
        return Ok(None);
    }
    let client = crate::runtime_env::configured_http_client_builder()?
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|error| format!("无法创建中转额度请求: {error}"))?;
    let mut requests = futures_util::stream::FuturesUnordered::new();
    for (endpoint, is_new_api) in endpoints {
        requests.push(query_relay_quota_endpoint(
            client.clone(),
            endpoint,
            is_new_api,
            request,
            api_key,
            checked_at,
        ));
    }
    let mut auth_error = None;
    let mut not_reported = None;
    let mut response_error = None;
    let mut request_error: Option<String> = None;
    while let Some(result) = requests.next().await {
        match result {
            Ok(Some(quota)) if quota.status == "available" => return Ok(Some(quota)),
            Ok(Some(quota)) if quota.status == "error" => {
                if quota.message.to_ascii_lowercase().contains("认证失败") {
                    auth_error = Some(quota);
                } else {
                    response_error = Some(quota);
                }
            }
            Ok(Some(quota)) => not_reported = Some(quota),
            Ok(None) => {}
            Err(error) => request_error = Some(error),
        }
    }
    if let Some(quota) = auth_error.or(not_reported) {
        return Ok(Some(quota));
    }
    if response_error.is_some() {
        return Ok(response_error);
    }
    Ok(request_error.map(|error| AgentQuotaView {
        status: "error".to_string(),
        source: None,
        remaining_amount: None,
        remaining_tokens: None,
        currency: request.currency.clone(),
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        message: format!("中转额度读取失败：{error}"),
        checked_at: checked_at.to_string(),
        ..Default::default()
    }))
}

pub(super) async fn query_relay_quota_endpoint(
    client: Client,
    endpoint: String,
    is_new_api: bool,
    request: &AgentProviderCheckRequest,
    api_key: &str,
    checked_at: &str,
) -> Result<Option<AgentQuotaView>, String> {
    let response = client
        .get(&endpoint)
        .bearer_auth(api_key)
        .header("x-api-key", api_key)
        .header("Accept", "application/json")
        .header("User-Agent", "Android-Toolkit/2.0")
        .send()
        .await
        .map_err(|error| format!("中转额度接口网络失败: {error}"))?;
    let status = response.status();
    if status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED {
        return Ok(None);
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Ok(Some(AgentQuotaView {
            status: "error".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: format!(
                "中转额度认证失败（HTTP {}），请检查 API Key",
                status.as_u16()
            ),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return Ok(Some(AgentQuotaView {
            status: "error".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: format!("中转额度接口暂时不可用（HTTP {}）", status.as_u16()),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    }
    if !status.is_success() {
        return Ok(None);
    }
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| format!("中转额度接口返回格式无效: {error}"))?;
    if let Some(message) = relay_auth_failure_message(&body) {
        return Ok(Some(AgentQuotaView {
            status: "error".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: format!("中转额度认证失败：{message}"),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    }
    if let Some(code) = relay_response_status_code(&body).filter(|code| *code >= 400) {
        return Ok(Some(AgentQuotaView {
            status: "error".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: format!("中转额度接口返回错误（code {code}）"),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    }
    if relay_response_declares_failure(&body) {
        return Ok(Some(AgentQuotaView {
            status: "unsupported".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: "中转额度接口返回失败，未上报可识别的剩余额度".to_string(),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    }
    let Some(quota) = parse_relay_quota(&body, is_new_api) else {
        return Ok(Some(AgentQuotaView {
            status: "unsupported".to_string(),
            source: Some(endpoint),
            remaining_amount: None,
            remaining_tokens: None,
            currency: request.currency.clone(),
            input_price_per_million: request.input_price_per_million,
            output_price_per_million: request.output_price_per_million,
            message: "中转额度接口已响应，但未上报可识别的剩余额度".to_string(),
            checked_at: checked_at.to_string(),
            ..Default::default()
        }));
    };
    Ok(Some(AgentQuotaView {
        status: "available".to_string(),
        source: Some(endpoint),
        remaining_amount: quota.remaining_amount,
        remaining_tokens: quota.remaining_tokens,
        total_amount: quota.total_amount,
        used_amount: quota.used_amount,
        currency: quota.currency.or(request.currency.clone()),
        input_price_per_million: request.input_price_per_million,
        output_price_per_million: request.output_price_per_million,
        message: "已读取中转服务额度；数值来自中转服务返回值".to_string(),
        checked_at: checked_at.to_string(),
        ..Default::default()
    }))
}

pub(super) fn relay_quota_endpoints(base_url: &str) -> Vec<(String, bool)> {
    let Ok(mut url) = Url::parse(base_url) else {
        return Vec::new();
    };
    let mut parts: Vec<String> = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    while parts.last().is_some_and(|segment| {
        matches!(
            segment.to_ascii_lowercase().as_str(),
            "v1" | "v2" | "v3" | "v4" | "anthropic" | "openai" | "responses"
        )
    }) {
        parts.pop();
    }
    if parts
        .last()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("api"))
    {
        parts.pop();
    }
    let path = if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}/", parts.join("/"))
    };
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    let root = url.to_string().trim_end_matches('/').to_string();
    let configured = base_url.trim_end_matches('/');
    let configured_path_is_protocol_suffix = Url::parse(configured)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|segments| segments.last().map(str::to_ascii_lowercase))
        })
        .is_some_and(|segment| {
            matches!(
                segment.as_str(),
                "v1" | "v2" | "v3" | "v4" | "anthropic" | "openai" | "responses"
            )
        });
    let configured_root = if configured_path_is_protocol_suffix {
        root.clone()
    } else {
        configured.to_string()
    };
    // Keep this list deliberately small and explicit. CCSwitch/New API commonly
    // exposes `/api/user/self`; the remaining paths cover established balance
    // conventions without probing arbitrary provider endpoints.
    let candidates = [
        (format!("{root}/api/user/self"), true),
        (format!("{configured_root}/api/user/self"), true),
        (format!("{root}/api/usage/token/"), true),
        (format!("{configured_root}/api/usage/token/"), true),
        (format!("{root}/api/user/usage"), false),
        (format!("{configured_root}/api/user/usage"), false),
        (format!("{root}/api/usage"), false),
        (format!("{configured_root}/api/usage"), false),
        (format!("{root}/v1/usage"), false),
        (format!("{configured_root}/v1/usage"), false),
        (format!("{root}/user/balance"), false),
        (format!("{configured_root}/user/balance"), false),
        (format!("{root}/balance"), false),
        (format!("{configured_root}/balance"), false),
    ];
    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|(endpoint, _)| seen.insert(endpoint.clone()))
        .collect()
}

#[derive(Debug, Default)]
pub(super) struct RelayQuotaValues {
    pub(super) remaining_amount: Option<f64>,
    pub(super) remaining_tokens: Option<u64>,
    pub(super) total_amount: Option<f64>,
    pub(super) used_amount: Option<f64>,
    pub(super) currency: Option<String>,
}

pub(super) fn parse_relay_quota(body: &Value, is_new_api: bool) -> Option<RelayQuotaValues> {
    if relay_response_declares_failure(body) {
        return None;
    }
    // LiteLLM virtual-key spend is USD; a missing budget means no known balance.
    if let Some(info) = body.get("info").filter(|info| info.is_object()) {
        let used_amount = info.get("spend").and_then(value_as_f64);
        let total_amount = info.get("max_budget").and_then(value_as_f64);
        if used_amount.is_some() || total_amount.is_some() {
            return Some(RelayQuotaValues {
                remaining_amount: total_amount
                    .zip(used_amount)
                    .map(|(total, used)| (total - used).max(0.0)),
                total_amount,
                used_amount,
                currency: Some("USD".into()),
                ..Default::default()
            });
        }
    }
    let scopes = relay_response_scopes(body);
    let find_f64 = |keys: &[&str]| {
        scopes.iter().find_map(|scope| {
            keys.iter()
                .find_map(|key| scope.get(*key).and_then(value_as_f64))
        })
    };
    let find_u64 = |keys: &[&str]| {
        scopes.iter().find_map(|scope| {
            keys.iter()
                .find_map(|key| scope.get(*key).and_then(value_as_u64))
        })
    };
    let currency = scopes.iter().find_map(|scope| {
        ["currency", "unit"]
            .iter()
            .find_map(|key| scope.get(*key).and_then(Value::as_str).map(str::to_string))
    });
    if let Some(token) = scopes
        .iter()
        .find(|scope| scope.get("object").and_then(Value::as_str) == Some("token_usage"))
    {
        let amount = |field: &str| {
            token
                .get(field)
                .and_then(value_as_f64)
                .filter(|value| *value >= 0.0)
                .map(|value| value / NEW_API_QUOTA_UNITS_PER_CURRENCY)
        };
        let unlimited = token
            .get("unlimited_quota")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let values = RelayQuotaValues {
            remaining_amount: if unlimited {
                None
            } else {
                amount("total_available")
            },
            total_amount: if unlimited {
                None
            } else {
                amount("total_granted")
            },
            used_amount: amount("total_used"),
            currency,
            ..Default::default()
        };
        return (values.remaining_amount.is_some() || values.used_amount.is_some())
            .then_some(values);
    }
    let total_amount = find_f64(&[
        "total_amount",
        "totalAmount",
        "total_quota",
        "totalQuota",
        "limit",
    ]);
    let used_amount = find_f64(&[
        "used_amount",
        "usedAmount",
        "used_quota",
        "usedQuota",
        "total_usage",
        "totalUsage",
        "used",
        "spent",
        "spending",
        "consumption",
        "cost",
    ]);
    if is_new_api {
        let remaining = find_f64(&[
            "remaining_quota",
            "remainingQuota",
            "remain_quota",
            "remainQuota",
        ]);
        if remaining.is_some() {
            return Some(RelayQuotaValues {
                remaining_amount: remaining,
                total_amount,
                used_amount,
                currency,
                ..Default::default()
            });
        }
        // New API/One API reports `quota` as the remaining quota in internal
        // units. It is not a total that should be reduced by `used_quota`.
        if let Some(quota) = find_f64(&["quota"]) {
            return Some(RelayQuotaValues {
                remaining_amount: Some((quota / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0)),
                total_amount: total_amount
                    .map(|value| (value / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0)),
                used_amount: used_amount
                    .map(|value| (value / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0)),
                currency,
                ..Default::default()
            });
        }
        let quota = find_f64(&["total_quota", "totalQuota"]);
        let used = find_f64(&["used_quota", "usedQuota", "total_usage", "totalUsage"]);
        if let Some(quota) = quota {
            if let Some(used) = used {
                return Some(RelayQuotaValues {
                    remaining_amount: Some(
                        ((quota - used) / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0),
                    ),
                    total_amount: Some((quota / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0)),
                    used_amount: Some((used / NEW_API_QUOTA_UNITS_PER_CURRENCY).max(0.0)),
                    currency,
                    ..Default::default()
                });
            }
        }
    }
    let remaining_tokens = [
        "remaining_tokens",
        "remainingTokens",
        "tokens_remaining",
        "tokensRemaining",
    ]
    .iter()
    .find_map(|key| find_u64(&[*key]));
    let remaining_amount = [
        "remaining_quota",
        "remainingQuota",
        "remain_quota",
        "remainQuota",
        "remaining",
        "remaining_balance",
        "remainingBalance",
        "balance",
        "credits",
        "credit",
        "total_balance",
        "totalBalance",
    ]
    .iter()
    .find_map(|key| find_f64(&[*key]));
    let remaining_amount = remaining_amount.or_else(|| {
        scopes.iter().find_map(|scope| {
            scope
                .get("quota")
                .and_then(|quota| quota.get("remaining"))
                .and_then(value_as_f64)
        })
    });
    let remaining_amount = remaining_amount.or_else(|| {
        total_amount
            .zip(used_amount)
            .map(|(total, used)| (total - used).max(0.0))
    });
    (remaining_amount.is_some()
        || remaining_tokens.is_some()
        || (!is_new_api && used_amount.is_some()))
    .then_some(RelayQuotaValues {
        remaining_amount,
        remaining_tokens,
        total_amount,
        used_amount,
        currency,
    })
}

pub(super) fn relay_response_scopes<'a>(body: &'a Value) -> Vec<&'a Value> {
    let mut scopes = vec![body];
    if let Some(balances) = body.get("balance_infos").and_then(Value::as_array) {
        if let Some(balance) = balances.first() {
            scopes.push(balance);
        }
    }
    let wrappers = [
        "data", "result", "user", "account", "balance", "quota", "usage", "limits", "error",
    ];
    for key in wrappers {
        if let Some(scope) = body.get(key) {
            scopes.push(scope);
            for nested_key in [
                "user", "account", "balance", "quota", "usage", "limits", "error",
            ] {
                if let Some(nested) = scope.get(nested_key) {
                    scopes.push(nested);
                }
            }
        }
    }
    scopes
}

pub(super) fn relay_auth_failure_message(body: &Value) -> Option<String> {
    let scopes = relay_response_scopes(body);
    let code = relay_response_status_code(body);
    if matches!(code, Some(401 | 403)) {
        return Some("服务返回未授权".to_string());
    }
    let text = scopes.iter().find_map(|scope| {
        [
            "message",
            "error",
            "detail",
            "error_message",
            "errorMessage",
        ]
        .iter()
        .find_map(|key| relay_error_text(scope.get(*key)?))
    })?;
    let normalized = text.to_ascii_lowercase();
    let looks_like_auth_failure = [
        "unauthorized",
        "forbidden",
        "invalid api key",
        "invalid api_key",
        "invalid token",
        "authentication failed",
        "未授权",
        "认证失败",
        "api key 无效",
        "令牌无效",
    ]
    .iter()
    .any(|marker| normalized.contains(marker));
    looks_like_auth_failure.then_some(text)
}

pub(super) fn relay_response_status_code(body: &Value) -> Option<u64> {
    relay_response_scopes(body).iter().find_map(|scope| {
        [
            "code",
            "status",
            "status_code",
            "statusCode",
            "http_code",
            "httpCode",
        ]
        .iter()
        .find_map(|key| scope.get(*key).and_then(value_as_u64))
    })
}

pub(super) fn relay_response_declares_failure(body: &Value) -> bool {
    relay_response_scopes(body).iter().any(|scope| {
        ["success", "ok"]
            .iter()
            .any(|key| scope.get(*key).and_then(Value::as_bool) == Some(false))
    })
}

pub(super) fn relay_error_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        Value::Object(object) => ["message", "detail", "type", "code"]
            .iter()
            .find_map(|key| object.get(*key).and_then(relay_error_text)),
        _ => None,
    }
}
