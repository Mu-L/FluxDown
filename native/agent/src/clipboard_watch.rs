//! 剪贴板监听：`general.clipboard_watch` 打开时，检测复制的下载链接并提交给捕获队列
//! （非静默 —— 由官方 UI 在新建下载窗口确认；没有 UI 时 agent 按需拉起）。
//!
//! 归 agent 而不是界面：托盘驻留、界面全部关闭时仍要继续监听。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fluxdown_protocol::capture_link::{is_capture_url, normalize_capture_url};
use fluxdown_protocol::method;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::event_hub::AgentEventHub;
use crate::gateway::GatewayService;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const PREFERENCE_KEY: &str = "general.clipboard_watch";

/// 在专用线程上轮询剪贴板（各平台剪贴板 API 都是同步阻塞调用）。
pub fn spawn(events: AgentEventHub, gateway: Arc<GatewayService>, cancel: CancellationToken) {
    let runtime = tokio::runtime::Handle::current();
    let spawned = std::thread::Builder::new()
        .name("fluxdown-clipboard-watch".to_owned())
        .spawn(move || watch(&events, &gateway, &cancel, &runtime));
    if let Err(error) = spawned {
        tracing::warn!(error = %error, "clipboard watcher thread unavailable");
    }
}

fn watch(
    events: &AgentEventHub,
    gateway: &Arc<GatewayService>,
    cancel: &CancellationToken,
    runtime: &tokio::runtime::Handle,
) {
    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            tracing::info!(error = %error, "clipboard unavailable; clipboard watch disabled");
            return;
        }
    };
    // 启动时的剪贴板内容只作基线，不触发提交。
    let mut last_text = clipboard.get_text().ok();
    let mut seen = SeenUrls::new();
    while !cancel.is_cancelled() {
        std::thread::sleep(POLL_INTERVAL);
        let enabled = events.inspect(|snapshot| {
            snapshot
                .preferences
                .values
                .get(PREFERENCE_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        if !enabled {
            continue;
        }
        let Ok(text) = clipboard.get_text() else {
            continue;
        };
        if last_text.as_deref() == Some(text.as_str()) {
            continue;
        }
        let urls = extract_capture_urls(&text);
        last_text = Some(text);
        let now = Instant::now();
        for url in urls {
            let url = normalize_capture_url(&url);
            if !seen.insert(&url, now) {
                continue;
            }
            let gateway = Arc::clone(gateway);
            runtime.spawn(async move {
                let submitted = gateway
                    .dispatch_local(
                        method::AGENT_CAPTURE_SUBMIT,
                        serde_json::json!({ "request": { "url": url }, "silent": false }),
                    )
                    .await;
                if let Err(error) = submitted {
                    tracing::warn!(code = ?error.code, "clipboard capture rejected");
                }
            });
        }
    }
}

/// 从剪贴板文本中提取可捕获的下载链接：整段是单个可捕获链接，或按行拆分后每一行都是
/// 可捕获链接（逐行返回）；否则返回空。
fn extract_capture_urls(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = trimmed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.len() > 1 {
        if lines.iter().all(|line| is_capture_url(line)) {
            return lines.into_iter().map(str::to_owned).collect();
        }
        return Vec::new();
    }
    if is_capture_url(trimmed) {
        return vec![trimmed.to_owned()];
    }
    Vec::new()
}

/// 最近提交过的 URL 集合：30 分钟内去重，最多保留 50 条（超出淘汰最旧的）。
struct SeenUrls {
    entries: VecDeque<(String, Instant)>,
}

impl SeenUrls {
    const CAPACITY: usize = 50;
    const TTL: Duration = Duration::from_secs(30 * 60);

    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }

    /// `url` 在 TTL 内已出现过则返回 `false`（去重命中）；否则记录并返回 `true`。
    fn insert(&mut self, url: &str, now: Instant) -> bool {
        self.entries
            .retain(|(_, seen_at)| now.saturating_duration_since(*seen_at) < Self::TTL);
        if self.entries.iter().any(|(seen, _)| seen == url) {
            return false;
        }
        while self.entries.len() >= Self::CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back((url.to_owned(), now));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_single_url() {
        let urls = extract_capture_urls("  https://example.com/file.zip  ");
        assert_eq!(urls, vec!["https://example.com/file.zip".to_owned()]);
    }

    #[test]
    fn extract_magnet_url() {
        let urls = extract_capture_urls("magnet:?xt=urn:btih:abcdef");
        assert_eq!(urls, vec!["magnet:?xt=urn:btih:abcdef".to_owned()]);
    }

    #[test]
    fn extract_multiline_all_urls() {
        let text = "https://a.example.com/1.zip\nhttps://b.example.com/2.zip\n";
        let urls = extract_capture_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://a.example.com/1.zip".to_owned(),
                "https://b.example.com/2.zip".to_owned(),
            ]
        );
    }

    #[test]
    fn extract_multiline_mixed_rejected() {
        let text = "https://a.example.com/1.zip\nnot a url\n";
        assert!(extract_capture_urls(text).is_empty());
    }

    #[test]
    fn extract_plain_text_rejected() {
        assert!(extract_capture_urls("just some copied text").is_empty());
    }

    #[test]
    fn extract_empty_rejected() {
        assert!(extract_capture_urls("   \n  ").is_empty());
    }

    #[test]
    fn seen_urls_dedupes_within_ttl() {
        let mut seen = SeenUrls::new();
        let now = Instant::now();
        assert!(seen.insert("https://example.com/a", now));
        assert!(!seen.insert("https://example.com/a", now + Duration::from_secs(60)));
    }

    #[test]
    fn seen_urls_expires_after_ttl() {
        let mut seen = SeenUrls::new();
        let now = Instant::now();
        assert!(seen.insert("https://example.com/a", now));
        let later = now + SeenUrls::TTL + Duration::from_secs(1);
        assert!(seen.insert("https://example.com/a", later));
    }

    #[test]
    fn seen_urls_evicts_oldest_beyond_capacity() {
        let mut seen = SeenUrls::new();
        let now = Instant::now();
        for i in 0..SeenUrls::CAPACITY {
            assert!(seen.insert(&format!("https://example.com/{i}"), now));
        }
        // 容量已满：再插入一条新的，应淘汰最旧的一条（index 0）。
        assert!(seen.insert("https://example.com/new", now));
        assert!(seen.insert("https://example.com/0", now));
    }
}
