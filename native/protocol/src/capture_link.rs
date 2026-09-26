//! 外部捕获链接语法：系统 URL scheme、命令行参数与剪贴板共用的判定与 `fluxdown:` 深链解码。
//!
//! 浏览器扩展、桌面进程与 agent 对同一段链接必须得出同一结论，因此只在这里定义一次。

use std::path::PathBuf;

/// 可直接建下载任务的链接前缀（大小写不敏感）。
const CAPTURE_SCHEMES: [&str; 7] = [
    "magnet:",
    "ed2k://",
    "fluxdown:",
    "http://",
    "https://",
    "ftp://",
    "ftps://",
];

/// 判定 `value` 是否为可直接建任务的链接。
///
/// ```
/// use fluxdown_protocol::capture_link::is_capture_url;
/// assert!(is_capture_url("MAGNET:?xt=urn:btih:abc"));
/// assert!(!is_capture_url("/tmp/a.torrent"));
/// ```
#[must_use]
pub fn is_capture_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    CAPTURE_SCHEMES
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

/// `fluxdown:` 深链 → 实际下载链接：`fluxdown://download?url=<encoded>` 或
/// `fluxdown:<url>`；其他 scheme 原样返回。
///
/// ```
/// use fluxdown_protocol::capture_link::normalize_capture_url;
/// assert_eq!(
///     normalize_capture_url("fluxdown://download?url=https%3A%2F%2Fa.b%2Fc"),
///     "https://a.b/c"
/// );
/// ```
#[must_use]
pub fn normalize_capture_url(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    if !lower.starts_with("fluxdown:") {
        return value.to_owned();
    }
    let rest = &value["fluxdown:".len()..];
    let rest = rest.trim_start_matches('/');
    if let Some(query) = rest.strip_prefix("download?") {
        for pair in query.split('&') {
            if let Some(encoded) = pair.strip_prefix("url=") {
                return percent_decode(encoded);
            }
        }
    }
    percent_decode(rest)
}

/// `.torrent` 本机路径或 `file://` URL → 路径（不检查文件是否存在）。
///
/// ```
/// use fluxdown_protocol::capture_link::torrent_file_path;
/// assert_eq!(
///     torrent_file_path("file:///tmp/a%20b.torrent"),
///     Some(std::path::PathBuf::from("/tmp/a b.torrent"))
/// );
/// assert_eq!(torrent_file_path("https://a.b/c.torrent.html"), None);
/// ```
#[must_use]
pub fn torrent_file_path(value: &str) -> Option<PathBuf> {
    let raw = value.strip_prefix("file://").unwrap_or(value);
    let decoded = percent_decode(raw);
    decoded
        .to_ascii_lowercase()
        .ends_with(".torrent")
        .then(|| PathBuf::from(decoded))
}

/// `%XX` 百分号解码；非法序列原样保留，结果不是 UTF-8 时返回原文。
#[must_use]
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Some(hex) = value.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_fluxdown_scheme_variants() {
        assert_eq!(
            normalize_capture_url("fluxdown://download?url=https%3A%2F%2Fa.b%2Fc"),
            "https://a.b/c"
        );
        assert_eq!(
            normalize_capture_url("fluxdown:https://a.b/c"),
            "https://a.b/c"
        );
        assert_eq!(normalize_capture_url("magnet:?x"), "magnet:?x");
    }

    #[test]
    fn percent_decode_keeps_malformed_and_multibyte_sequences() {
        assert_eq!(percent_decode("a%2"), "a%2");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
        // 百分号紧邻多字节字符：切片不能落在字符中间。
        assert_eq!(percent_decode("%中文"), "%中文");
    }

    #[test]
    fn torrent_paths_require_torrent_suffix() {
        assert_eq!(
            torrent_file_path("/tmp/X.TORRENT"),
            Some(PathBuf::from("/tmp/X.TORRENT"))
        );
        assert_eq!(torrent_file_path("/tmp/x.zip"), None);
    }
}
