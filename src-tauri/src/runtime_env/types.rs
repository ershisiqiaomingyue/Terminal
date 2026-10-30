use super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolQuery {
    #[serde(default)]
    pub include_remote: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolPathRequest {
    pub tool_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolDownloadRequest {
    pub tool_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentNetworkSettingsRequest {
    pub proxy_url: Option<String>,
    #[serde(default)]
    pub proxy_enabled: Option<bool>,
    #[serde(default)]
    pub proxy_child_processes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentNetworkSettings {
    pub proxy_url: Option<String>,
    pub proxy_enabled: bool,
    pub proxy_child_processes: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub local_path: Option<String>,
    pub local_version: Option<String>,
    pub local_status: String,
    pub local_message: String,
    pub remote_version: Option<String>,
    pub remote_message: Option<String>,
    pub supports_download: bool,
    pub selection_hint: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentToolDownloadProgress {
    pub tool_id: String,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}
