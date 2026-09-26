//! 分类匹配索引：由偏好 `custom_categories` 构建，按扩展名 / 正则把任务归类。

use std::collections::HashSet;

use fluxdown_protocol::CustomCategoryDto;

use super::DownloadTaskView;

enum Matcher {
    /// `builtin_all`：匹配所有任务。
    All,
    /// `builtin_other`：不命中任何其他分类的任务。
    Other,
    Extensions(HashSet<String>),
    /// 编译失败视为不匹配。
    Regex(Option<regex::Regex>),
}

pub(crate) struct CategoryRule {
    pub(crate) dto: CustomCategoryDto,
    matcher: Matcher,
}

/// 按 `position` 排序的分类规则集合。
#[derive(Default)]
pub(crate) struct CategoryIndex {
    rules: Vec<CategoryRule>,
}

impl CategoryIndex {
    pub(crate) fn from_preference(value: Option<&serde_json::Value>) -> Self {
        Self::from_dtos(CustomCategoryDto::from_preference(value))
    }

    pub(crate) fn from_dtos(dtos: Vec<CustomCategoryDto>) -> Self {
        let rules = dtos
            .into_iter()
            .map(|dto| {
                let matcher = match (dto.builtin_type.as_deref(), dto.match_mode.as_str()) {
                    (Some("all"), _) => Matcher::All,
                    (Some("other"), _) => Matcher::Other,
                    // 与 Flutter `CustomCategory.matches` 及 agent 分类目录解析一致：正则不区分
                    // 大小写，空模式不匹配（Rust 空正则会匹配一切）。
                    (_, "regex") => Matcher::Regex(
                        (!dto.regex_pattern.is_empty())
                            .then(|| {
                                regex::RegexBuilder::new(&dto.regex_pattern)
                                    .case_insensitive(true)
                                    .build()
                                    .ok()
                            })
                            .flatten(),
                    ),
                    _ => Matcher::Extensions(
                        dto.extensions
                            .iter()
                            .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
                            .collect(),
                    ),
                };
                CategoryRule { dto, matcher }
            })
            .collect();
        Self { rules }
    }

    pub(crate) fn rules(&self) -> &[CategoryRule] {
        &self.rules
    }

    /// 可见规则（`visible=false` 的隐藏）。
    pub(crate) fn visible(&self) -> impl Iterator<Item = &CategoryRule> {
        self.rules.iter().filter(|rule| rule.dto.visible)
    }

    fn rule_matches(rule: &CategoryRule, task: &DownloadTaskView) -> bool {
        match &rule.matcher {
            Matcher::All => true,
            Matcher::Other => false,
            Matcher::Extensions(extensions) => task
                .extension()
                .is_some_and(|extension| extensions.contains(&extension)),
            Matcher::Regex(regex) => regex
                .as_ref()
                .is_some_and(|regex| regex.is_match(&task.name)),
        }
    }

    /// 任务是否属于分类 `id`；未知 id 视为不匹配。
    pub(crate) fn matches(&self, id: &str, task: &DownloadTaskView) -> bool {
        let Some(rule) = self.rules.iter().find(|rule| rule.dto.id == id) else {
            return false;
        };
        match rule.matcher {
            Matcher::Other => !self
                .rules
                .iter()
                .filter(|other| !matches!(other.matcher, Matcher::All | Matcher::Other))
                .any(|other| Self::rule_matches(other, task)),
            _ => Self::rule_matches(rule, task),
        }
    }

    /// 任务命中的首个（非 all/other）分类 id；无 → `builtin_other`。
    pub(crate) fn category_of(&self, task: &DownloadTaskView) -> &str {
        self.rules
            .iter()
            .filter(|rule| !matches!(rule.matcher, Matcher::All | Matcher::Other))
            .find(|rule| Self::rule_matches(rule, task))
            .map_or("builtin_other", |rule| rule.dto.id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::CustomCategoryDto;

    use super::CategoryIndex;
    use crate::model::DownloadTaskView;

    fn task(name: &str) -> DownloadTaskView {
        let dto = serde_json::from_value::<fluxdown_protocol::TaskDto>(serde_json::json!({
            "taskId":"t","url":"https://example.com/x","fileName":name,
            "saveDir":"/tmp","status":3,"downloadedBytes":1,"totalBytes":1,
            "errorMessage":"","createdAt":"1","proxyUrl":"","queueId":"main","checksum":""
        }))
        .expect("task");
        DownloadTaskView::local(&dto, None, false)
    }

    #[test]
    fn builtin_extension_and_other_matching() {
        let index = CategoryIndex::from_dtos(CustomCategoryDto::builtin_defaults());
        assert!(index.matches("builtin_all", &task("a.mp4")));
        assert!(index.matches("builtin_video", &task("a.MP4")));
        assert!(!index.matches("builtin_video", &task("a.zip")));
        assert!(index.matches("builtin_other", &task("README")));
        assert!(!index.matches("builtin_other", &task("a.zip")));
        assert_eq!(index.category_of(&task("a.zip")), "builtin_archive");
    }

    #[test]
    fn regex_category_and_invalid_pattern() {
        let mut dtos = CustomCategoryDto::builtin_defaults();
        dtos.push(CustomCategoryDto {
            id: "c1".to_owned(),
            name: "Episodes".to_owned(),
            icon: "file".to_owned(),
            match_mode: "regex".to_owned(),
            extensions: Vec::new(),
            regex_pattern: r"S\d+E\d+".to_owned(),
            position: 9,
            visible: true,
            is_builtin: false,
            builtin_type: None,
            save_dir: String::new(),
        });
        dtos.push(CustomCategoryDto {
            id: "bad".to_owned(),
            name: "Bad".to_owned(),
            icon: "file".to_owned(),
            match_mode: "regex".to_owned(),
            extensions: Vec::new(),
            regex_pattern: "(".to_owned(),
            position: 10,
            visible: true,
            is_builtin: false,
            builtin_type: None,
            save_dir: String::new(),
        });
        dtos.push(CustomCategoryDto {
            id: "empty".to_owned(),
            name: "Empty".to_owned(),
            icon: "file".to_owned(),
            match_mode: "regex".to_owned(),
            extensions: Vec::new(),
            regex_pattern: String::new(),
            position: 11,
            visible: true,
            is_builtin: false,
            builtin_type: None,
            save_dir: String::new(),
        });
        let index = CategoryIndex::from_dtos(dtos);
        assert!(index.matches("c1", &task("show.S01E02.mkv")));
        assert!(index.matches("c1", &task("show.s01e02.mkv")));
        assert!(!index.matches("bad", &task("anything")));
        assert!(!index.matches("empty", &task("anything.mkv")));
    }
}
