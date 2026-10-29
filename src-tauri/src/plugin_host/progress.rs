use super::*;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Download {
    downloaded_bytes: u64,
    bytes_per_second: f64,
    completed_requests: u64,
    active_requests: u64,
}

pub(super) struct InstallProgress<'a> {
    app: &'a AppHandle,
    request_id: &'a str,
    download: Download,
    last_poll: Instant,
    phase: String,
}

impl<'a> InstallProgress<'a> {
    pub(super) fn new(app: &'a AppHandle, request_id: &'a str) -> Self {
        Self {
            app,
            request_id,
            download: Download::default(),
            last_poll: Instant::now(),
            phase: String::new(),
        }
    }

    pub(super) fn phase(&mut self, phase: &str) {
        self.phase = phase.into();
        self.download.bytes_per_second = 0.0;
        self.emit(phase);
    }

    pub(super) fn poll(&mut self, directory: &Path, force: bool) {
        if !force && self.last_poll.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_poll = Instant::now();
        // A concurrent snapshot write can be partial; keep the last valid sample.
        if let Ok(bytes) = fs::read(directory.join("download-progress.json")) {
            if let Ok(download) = serde_json::from_slice::<Download>(&bytes) {
                self.download = download;
                self.emit(if self.phase == "downloading-host" {
                    "downloading-host"
                } else {
                    "downloading"
                });
            }
        }
    }

    fn emit(&self, phase: &str) {
        let _ = self.app.emit(
            "plugin-install-progress",
            serde_json::json!({
                "requestId": self.request_id, "phase": phase,
                "downloadedBytes": self.download.downloaded_bytes,
                "bytesPerSecond": self.download.bytes_per_second,
                "completedRequests": self.download.completed_requests,
                "activeRequests": self.download.active_requests,
                // npm discovers dependencies dynamically and may satisfy them from
                // cache; there is no reliable total for the entire installation.
                "totalBytes": null,
            }),
        );
    }
}
