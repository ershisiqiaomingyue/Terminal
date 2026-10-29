use agent_core::execute_process;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum MatchMode {
    #[default]
    Any,
    All,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaptureRequest {
    serial: Option<String>,
    #[serde(default)]
    terms: Vec<String>,
    #[serde(default)]
    match_mode: MatchMode,
    limit: Option<usize>,
    max_lines: Option<usize>,
    timeout_ms: Option<u64>,
}

impl CaptureRequest {
    fn parse(mut input: Value) -> Result<Self, String> {
        if let Some(object) = input.as_object_mut() {
            object.remove("__agent_call_id");
        }
        let mut request: Self = serde_json::from_value(input)
            .map_err(|error| format!("Logcat 抓取参数无效：{error}"))?;
        request.serial = request
            .serial
            .map(|serial| serial.trim().to_string())
            .filter(|serial| !serial.is_empty());
        if request.serial.as_ref().is_some_and(|serial| {
            serial.len() > 256 || serial.starts_with('-') || serial.chars().any(char::is_whitespace)
        }) {
            return Err("设备序列号无效。".to_string());
        }
        if request.terms.len() > 32 || request.terms.iter().any(|term| term.len() > 256) {
            return Err("一次 Logcat 查询最多包含 32 个关键词，每个不超过 256 字节。".to_string());
        }
        request.terms = request
            .terms
            .into_iter()
            .map(|term| term.trim().to_string())
            .filter(|term| !term.is_empty())
            .collect();
        request.limit = Some(request.limit.unwrap_or(50).clamp(1, 500));
        request.max_lines = Some(request.max_lines.unwrap_or(2_000).clamp(1, 10_000));
        request.timeout_ms = Some(request.timeout_ms.unwrap_or(10_000).clamp(1_000, 15_000));
        Ok(request)
    }

    fn evidence(&self, serial: &str, text: &str, capture_partial: bool) -> Value {
        const EVIDENCE_BYTES: usize = 128 * 1024;
        let terms = self
            .terms
            .iter()
            .map(|term| term.to_lowercase())
            .collect::<Vec<_>>();
        let mut term_found = vec![false; terms.len()];
        let mut evidence = Vec::new();
        let mut matched_count = 0;
        let mut scanned_lines = 0;
        let mut bytes = 0;
        for line in text
            .lines()
            .filter(|line| !line.starts_with("--------- beginning of"))
        {
            scanned_lines += 1;
            let lower = line.to_lowercase();
            let hits = terms
                .iter()
                .map(|term| lower.contains(term))
                .collect::<Vec<_>>();
            for (found, hit) in term_found.iter_mut().zip(&hits) {
                *found |= hit;
            }
            let matches = terms.is_empty()
                || match self.match_mode {
                    MatchMode::Any => hits.iter().any(|hit| *hit),
                    MatchMode::All => hits.iter().all(|hit| *hit),
                };
            if !matches {
                continue;
            }
            matched_count += 1;
            if evidence.len() < self.limit.unwrap_or(50) && bytes + line.len() <= EVIDENCE_BYTES {
                bytes += line.len();
                evidence.push(line);
            }
        }
        let truncated = capture_partial || matched_count > evidence.len();
        let missing_terms = self
            .terms
            .iter()
            .zip(term_found)
            .filter_map(|(term, found)| (!found).then_some(term))
            .collect::<Vec<_>>();
        json!({
            "source":"live_device_buffer", "serial":serial,
            "capturedAt":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
            "terms":self.terms, "matchMode":if matches!(self.match_mode, MatchMode::Any) {"any"} else {"all"},
            "found":matched_count > 0, "matchedCount":matched_count,
            "scannedLines":scanned_lines, "maxLines":self.max_lines, "evidence":evidence,
            "missingTerms":missing_terms, "truncated":truncated, "partial":truncated,
            "scope":"本次读取设备当前日志缓冲；未清空日志，未启动持续采集，也不代表未来日志或全部历史。",
            "nextStep":if matched_count == 0 {
                "本次范围内没有匹配结果，请直接报告该事实；只有复现操作产生新日志或用户要求扩大范围时才再次抓取，不要逐个关键词反复查询旧会话。"
            } else if truncated {
                "证据达到返回或抓取上限，只能依据已返回范围判断。"
            } else {
                "依据本次日志证据在当前对话给出结果，无需打开 Logcat 页面。"
            }
        })
    }
}

fn single_online_serial(text: &str) -> Result<String, String> {
    let devices = text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            (fields.next()? == "device").then(|| serial.to_string())
        })
        .collect::<Vec<_>>();
    match devices.as_slice() {
        [serial] => Ok(serial.clone()),
        [] => Err("未发现已授权的在线 Android 设备，请连接设备并确认 USB 调试授权。".to_string()),
        _ => Err(format!(
            "发现多个在线设备，请指定 serial：{}",
            devices.join("、")
        )),
    }
}

async fn read_adb(
    adb: &Path,
    args: Vec<String>,
    timeout_ms: u64,
    cancellation: Arc<AtomicBool>,
) -> Result<(String, bool), String> {
    if cancellation.load(Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    let output = execute_process(
        adb.to_path_buf(),
        &args,
        adb.parent().unwrap_or(Path::new(".")),
        timeout_ms,
        cancellation,
        None,
    )
    .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).into_owned()
        } else {
            stderr.into_owned()
        };
        return Err(format!(
            "后台 Logcat ADB 读取失败（退出码 {:?}）：{}",
            output.status.code(),
            detail.trim()
        ));
    }
    Ok((
        String::from_utf8_lossy(&output.stdout).into_owned(),
        output.stdout_capture_truncated,
    ))
}

pub(crate) async fn capture_logcat(
    input: Value,
    cancellation: Arc<AtomicBool>,
) -> Result<Value, String> {
    let request = CaptureRequest::parse(input)?;
    if cancellation.load(Ordering::Relaxed) {
        return Err("请求已取消".to_string());
    }
    let adb = crate::android_tools::find_adb()?;
    let serial = match &request.serial {
        Some(serial) => serial.clone(),
        None => {
            let (devices, partial) = read_adb(
                &adb,
                vec!["devices".into()],
                request.timeout_ms.unwrap(),
                cancellation.clone(),
            )
            .await?;
            if partial {
                return Err("设备列表超出读取上限，请明确指定设备序列号。".to_string());
            }
            single_online_serial(&devices)?
        }
    };
    let (text, partial) = read_adb(
        &adb,
        vec![
            "-s".into(),
            serial.clone(),
            "logcat".into(),
            "-d".into(),
            "-v".into(),
            "threadtime".into(),
            "-t".into(),
            request.max_lines.unwrap().to_string(),
        ],
        request.timeout_ms.unwrap(),
        cancellation,
    )
    .await?;
    Ok(request.evidence(&serial, &text, partial))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_capture_batches_alternatives_and_reports_absence() {
        let request =
            CaptureRequest::parse(json!({"terms":["accept(", "onNewSession", "setProtocol"]}))
                .unwrap();
        let result = request.evidence(
            "qa",
            "I/Test: accept(\nI/Test: onNewSession\nI/Test: unrelated\n",
            false,
        );
        assert_eq!(result["matchedCount"], 2);
        assert_eq!(result["missingTerms"], json!(["setProtocol"]));
        assert_eq!(result["found"], true);
        let result = request.evidence("qa", "I/Test: unrelated\n", false);
        assert_eq!(result["found"], false);
        assert_eq!(result["evidence"], json!([]));
        assert_eq!(result["partial"], false);
    }

    #[test]
    fn background_capture_respects_all_matching_and_output_budgets() {
        let request =
            CaptureRequest::parse(json!({"terms":["car", "ready"], "matchMode":"all", "limit":1}))
                .unwrap();
        let result = request.evidence("qa", "car only\ncar ready\nCAR READY\n", false);
        assert_eq!(result["matchedCount"], 2);
        assert_eq!(result["evidence"], json!(["car ready"]));
        assert_eq!(result["partial"], true);
        assert_eq!(request.evidence("qa", "", true)["partial"], true);
        assert!(CaptureRequest::parse(json!({"serial":"-bad"})).is_err());
        assert!(CaptureRequest::parse(json!({"terms":vec!["x";33]})).is_err());
        assert!(CaptureRequest::parse(json!({"sessionId":"old"})).is_err());
        assert!(
            CaptureRequest::parse(json!({"__agent_call_id":"call-1", "terms":["ready"]})).is_ok()
        );
    }

    #[test]
    fn automatic_device_selection_never_guesses_between_devices() {
        assert_eq!(
            single_online_serial("List of devices attached\na device\nb offline\n").unwrap(),
            "a"
        );
        assert!(single_online_serial("a device\nb device\n").is_err());
        assert!(single_online_serial("a unauthorized\n").is_err());
    }

    #[tokio::test]
    async fn cancelled_capture_does_not_discover_or_spawn_adb() {
        let error = capture_logcat(json!({}), Arc::new(AtomicBool::new(true)))
            .await
            .unwrap_err();
        assert_eq!(error, "请求已取消");
    }
}
