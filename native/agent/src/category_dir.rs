//! 外部捕获的分类保存目录：与 Flutter `SettingsProvider.resolveCategorySaveDir` /
//! `CustomCategory.matches` 同语义。
//!
//! 只看可见分类，按 `position` 顺序；先找首个配置了目录且命中的普通分类（非 all / other），
//! 否则当文件不命中任何普通分类时取 `other` 的目录。文件名为空或不含 `.` 时用 URL 路径末段
//! （解码后且含 `.`）参与匹配。扩展名比较不区分大小写，正则不区分大小写，非法正则视为不命中。

use std::collections::BTreeMap;

use fluxdown_protocol::{CUSTOM_CATEGORIES_PREF_KEY, CustomCategoryDto};
use serde_json::Value;

/// 命中的分类保存目录；无命中 / 命中分类未配置目录时返回 `None`。
pub(crate) fn category_save_dir(
    preferences: &BTreeMap<String, Value>,
    file_name: &str,
    url: &str,
) -> Option<String> {
    let name = if file_name.contains('.') {
        file_name.to_owned()
    } else {
        file_name_from_url(url).unwrap_or_else(|| file_name.to_owned())
    };
    if name.is_empty() {
        return None;
    }
    let categories =
        CustomCategoryDto::from_preference(preferences.get(CUSTOM_CATEGORIES_PREF_KEY))
            .into_iter()
            .filter(|category| category.visible)
            .collect::<Vec<_>>();
    let is_special = |category: &CustomCategoryDto| {
        matches!(category.builtin_type.as_deref(), Some("all" | "other"))
    };
    let normals = categories
        .iter()
        .filter(|category| !is_special(category))
        .collect::<Vec<_>>();
    if let Some(category) = normals
        .iter()
        .find(|category| !category.save_dir.is_empty() && matches(category, &name))
    {
        return Some(category.save_dir.clone());
    }
    categories
        .iter()
        .find(|category| category.builtin_type.as_deref() == Some("other"))
        .filter(|other| !other.save_dir.is_empty())
        .filter(|_| !normals.iter().any(|category| matches(category, &name)))
        .map(|other| other.save_dir.clone())
}

fn matches(category: &CustomCategoryDto, name: &str) -> bool {
    if category.match_mode == "regex" {
        if category.regex_pattern.is_empty() {
            return false;
        }
        return regex::RegexBuilder::new(&category.regex_pattern)
            .case_insensitive(true)
            .build()
            .is_ok_and(|regex| regex.is_match(name));
    }
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !extension.is_empty()
        && category.extensions.iter().any(|candidate| {
            candidate
                .trim_start_matches('.')
                .eq_ignore_ascii_case(extension)
        })
}

/// URL 路径末段（百分号解码）且含 `.` 时作为文件名。
fn file_name_from_url(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url.trim()).ok()?;
    let last = parsed.path_segments()?.next_back()?;
    let decoded = percent_decode(last);
    decoded.contains('.').then_some(decoded)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::category_save_dir;

    fn prefs(
        categories: serde_json::Value,
    ) -> std::collections::BTreeMap<String, serde_json::Value> {
        std::collections::BTreeMap::from([("custom_categories".to_owned(), categories)])
    }

    fn category(
        id: &str,
        builtin: Option<&str>,
        extensions: &[&str],
        save_dir: &str,
        position: i64,
    ) -> serde_json::Value {
        json!({
            "id": id, "name": id, "matchMode": "extension", "extensions": extensions,
            "position": position, "visible": true, "isBuiltin": builtin.is_some(),
            "builtinType": builtin, "saveDir": save_dir,
        })
    }

    #[test]
    fn first_matching_category_with_dir_wins_in_position_order() {
        let preferences = prefs(json!([
            category("video2", None, &["mp4"], "/second", 5),
            category("video", Some("video"), &["mp4", "mkv"], "/videos", 1),
            category("docs", Some("document"), &["pdf"], "", 2),
        ]));
        assert_eq!(
            category_save_dir(&preferences, "Movie.MP4", "").as_deref(),
            Some("/videos")
        );
        // 命中但未配置目录 → 不回退到其他分类的目录。
        assert_eq!(category_save_dir(&preferences, "a.pdf", ""), None);
    }

    #[test]
    fn other_dir_applies_only_when_no_normal_category_matches() {
        let preferences = prefs(json!([
            category("video", Some("video"), &["mp4"], "", 1),
            category("other", Some("other"), &[], "/other", 7),
        ]));
        assert_eq!(
            category_save_dir(&preferences, "setup.exe", "").as_deref(),
            Some("/other")
        );
        assert_eq!(category_save_dir(&preferences, "clip.mp4", ""), None);
    }

    #[test]
    fn url_segment_fills_missing_name_and_hidden_categories_are_ignored() {
        let mut hidden = category("music", None, &["flac"], "/music-hidden", 1);
        hidden["visible"] = json!(false);
        let preferences = prefs(json!([
            hidden,
            category("audio", Some("audio"), &["flac"], "/music", 2),
        ]));
        assert_eq!(
            category_save_dir(&preferences, "", "https://x.test/a/My%20Song.flac?x=1").as_deref(),
            Some("/music")
        );
        assert_eq!(
            category_save_dir(&preferences, "", "https://x.test/download"),
            None
        );
    }

    #[test]
    fn regex_matches_case_insensitively_and_invalid_regex_never_matches() {
        let preferences = prefs(json!([
            {"id": "bad", "name": "bad", "matchMode": "regex", "regexPattern": "(",
             "position": 1, "saveDir": "/bad"},
            {"id": "iso", "name": "iso", "matchMode": "regex", "regexPattern": "^ubuntu-.*\\.iso$",
             "position": 2, "saveDir": "/iso"},
        ]));
        assert_eq!(
            category_save_dir(&preferences, "Ubuntu-24.04.ISO", "").as_deref(),
            Some("/iso")
        );
    }
}
