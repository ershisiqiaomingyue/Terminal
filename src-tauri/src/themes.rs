use futures_util::StreamExt;
pub(crate) mod storage;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::Duration;

const MAX_THEME_BYTES: usize = 1024 * 1024;
const MAX_PACKAGE_BYTES: usize = 16 * 1024 * 1024;

async fn download(url: String, limit: usize) -> Result<Vec<u8>, String> {
    let url = url::Url::parse(url.trim()).map_err(|_| "主题地址无效。".to_string())?;
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err("主题地址必须使用 HTTPS。".into());
    }
    let client = agent_core::http_client_builder()?
        .https_only(true)
        .user_agent("Terminal-Theme-Importer")
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("无法创建主题下载连接：{error}"))?;
    for attempt in 0..2 {
        match download_once(&client, &url, limit).await {
            Ok(bytes) => return Ok(bytes),
            Err((true, _)) if attempt == 0 => tokio::time::sleep(Duration::from_millis(400)).await,
            Err((_, message)) => return Err(message),
        }
    }
    unreachable!()
}

async fn download_once(client: &reqwest::Client, url: &url::Url, limit: usize) -> Result<Vec<u8>, (bool, String)> {
    let response = client.get(url.clone()).send().await
        .map_err(|error| (error.is_timeout() || error.is_connect(), format!("主题下载失败：{error}")))?;
    let status = response.status();
    if !status.is_success() {
        let detail = if status == reqwest::StatusCode::NOT_FOUND { "，仓库可能未提交生成后的主题文件" } else { "" };
        return Err((status.is_server_error(), format!("主题下载失败：HTTP {}{detail}（{url}）", status.as_u16())));
    }
    if response.content_length().is_some_and(|size| size > limit as u64) {
        return Err((false, "主题下载超过大小限制。".into()));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| (true, format!("主题读取失败：{error}")))?;
        if bytes.len() + chunk.len() > limit { return Err((false, "主题下载超过大小限制。".into())); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn fetch_theme_text(url: String) -> Result<String, String> {
    String::from_utf8(download(url, MAX_THEME_BYTES).await?).map_err(|_| "主题文件必须是 UTF-8 文本。".into())
}

// Read only bounded JSON data in memory; never install or execute extension code.
fn read_theme_package(bytes: Vec<u8>) -> Result<HashMap<String, String>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "主题发布包不是有效的 VSIX。".to_string())?;
    if archive.len() > 4096 { return Err("主题发布包文件过多。".into()); }
    let mut files = HashMap::new();
    let mut total = 0;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| format!("无法读取主题发布包：{error}"))?;
        let name = file.name().to_string();
        if !name.starts_with("extension/") || !(name.ends_with(".json") || name.ends_with(".jsonc")) || file.is_dir() { continue; }
        if file.enclosed_name().is_none() || file.size() > MAX_THEME_BYTES as u64 { return Err("主题发布包文件无效或过大。".into()); }
        let mut data = Vec::new();
        (&mut file).take((MAX_THEME_BYTES + 1) as u64).read_to_end(&mut data).map_err(|error| format!("主题解压失败：{error}"))?;
        total += data.len();
        if data.len() > MAX_THEME_BYTES || total > MAX_PACKAGE_BYTES { return Err("主题发布包解压超过大小限制。".into()); }
        let text = String::from_utf8(data).map_err(|_| "主题文件必须是 UTF-8 文本。".to_string())?;
        files.insert(name, text);
    }
    Ok(files)
}

#[tauri::command]
pub async fn fetch_theme_package(url: String) -> Result<HashMap<String, String>, String> {
    let parsed = url::Url::parse(&url).map_err(|_| "主题发布包地址无效。".to_string())?;
    if parsed.host_str() != Some("open-vsx.org") { return Err("主题发布包必须来自 Open VSX。".into()); }
    let bytes = download(url, MAX_PACKAGE_BYTES).await?;
    tokio::task::spawn_blocking(move || read_theme_package(bytes)).await.map_err(|error| format!("主题解压失败：{error}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn package_reads_only_json_without_extracting_code() {
        let files = read_theme_package(archive(&[("extension/themes/dark.json", b"{}"), ("extension/main.js", b"code"), ("other.json", b"{}")])).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files["extension/themes/dark.json"], "{}");
    }

    #[test]
    fn package_rejects_invalid_paths_and_oversized_entries() {
        assert!(read_theme_package(archive(&[("extension/../../bad.json", b"{}")])).is_err());
        assert!(read_theme_package(archive(&[("extension/big.json", &vec![b' '; MAX_THEME_BYTES + 1])])).is_err());
        assert!(read_theme_package(vec![1, 2, 3]).is_err());
    }
}
