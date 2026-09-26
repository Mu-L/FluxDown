use fluxdown_ui_i18n::{Translator, keys};
use gpui::SharedString;

/// 「聚焦搜索」快捷键提示（与 app `menus.rs` 的 `secondary-f` 绑定一致），显示在顶栏搜索框右侧徽标里。
#[cfg(target_os = "macos")]
pub(crate) const SEARCH_SHORTCUT_HINT: &str = "⌘F";
#[cfg(not(target_os = "macos"))]
pub(crate) const SEARCH_SHORTCUT_HINT: &str = "Ctrl+F";

#[derive(Clone)]
pub(crate) struct DownloadStrings {
    pub(crate) category_archive: SharedString,
    pub(crate) category_audio: SharedString,
    pub(crate) category_document: SharedString,
    pub(crate) category_image: SharedString,
    pub(crate) category_other: SharedString,
    pub(crate) category_program: SharedString,
    pub(crate) category_video: SharedString,
    pub(crate) col_created: SharedString,
    pub(crate) col_eta: SharedString,
    pub(crate) col_file_name: SharedString,
    pub(crate) col_progress: SharedString,
    pub(crate) col_protocol: SharedString,
    pub(crate) col_queue: SharedString,
    pub(crate) col_size: SharedString,
    pub(crate) col_source: SharedString,
    pub(crate) col_speed: SharedString,
    pub(crate) col_status: SharedString,
    pub(crate) active_transfers: SharedString,
    pub(crate) connected_peers: SharedString,
    pub(crate) delete: SharedString,
    pub(crate) delete_task: SharedString,
    pub(crate) delete_task_and_file: SharedString,
    delete_confirm_with_file: SharedString,
    batch_delete_confirm_with_file: SharedString,
    pub(crate) too_many_windows_hint: SharedString,
    pub(crate) confirm: SharedString,
    pub(crate) cancel: SharedString,
    pub(crate) disconnected: SharedString,
    pub(crate) action_failed: SharedString,
    pub(crate) metadata_loading: SharedString,
    eta_seconds: SharedString,
    eta_minutes: SharedString,
    eta_hours: SharedString,
    pub(crate) later_queue: SharedString,
    pub(crate) pause: SharedString,
    pub(crate) pause_all: SharedString,
    pub(crate) resume: SharedString,
    pub(crate) resume_all: SharedString,
    pub(crate) main_queue: SharedString,
    pub(crate) open_file: SharedString,
    pub(crate) open_folder: SharedString,
    pub(crate) remote_tasks: SharedString,
    pub(crate) sidebar_queues: SharedString,
    pub(crate) status_all: SharedString,
    pub(crate) status_completed: SharedString,
    pub(crate) status_downloading: SharedString,
    pub(crate) status_error: SharedString,
    pub(crate) status_incomplete: SharedString,
    pub(crate) status_paused: SharedString,
    pub(crate) status_pending: SharedString,
    pub(crate) today: SharedString,
    pub(crate) yesterday: SharedString,
    pub(crate) this_week: SharedString,
    pub(crate) this_month: SharedString,
    pub(crate) older: SharedString,
    pub(crate) ungrouped: SharedString,
    pub(crate) site_bt: SharedString,
    pub(crate) ignore_plugin_retry: SharedString,
    pub(crate) ignore_plugin_retry_title: SharedString,
    pub(crate) ignore_plugin_retry_msg: SharedString,
    pub(crate) boost_download: SharedString,
    pub(crate) cancel_boost: SharedString,
    pub(crate) rename_task: SharedString,
    pub(crate) rename_task_title: SharedString,
    pub(crate) rename_task_placeholder: SharedString,
    pub(crate) redownload_task: SharedString,
    pub(crate) copy_url: SharedString,
    pub(crate) move_to_queue: SharedString,
    pub(crate) open_in_window: SharedString,
    pub(crate) group_pause_all: SharedString,
    pub(crate) group_resume_all: SharedString,
    pub(crate) group_retry_failed: SharedString,
    pub(crate) group_open_folder: SharedString,
    pub(crate) group_copy_source_link: SharedString,
    pub(crate) group_delete: SharedString,
    pub(crate) group_delete_with_files: SharedString,
    group_delete_confirm_with_file: SharedString,
    pub(crate) open_group_in_window: SharedString,
    pub(crate) search_tasks_placeholder: SharedString,
    pub(crate) unsupported_drop_hint: SharedString,
    pub(crate) sidebar_status: SharedString,
    pub(crate) sidebar_devices: SharedString,
    pub(crate) this_device: SharedString,
    pub(crate) add_category: SharedString,
    pub(crate) edit_category: SharedString,
    pub(crate) hide_section: SharedString,
    pub(crate) start_queue_action: SharedString,
    pub(crate) stop_queue_action: SharedString,
    pub(crate) manage_queue_action: SharedString,
    pub(crate) delete_queue_action: SharedString,
    pub(crate) queue_delete_confirm_desc: SharedString,
    pub(crate) tab_failed: SharedString,
    pub(crate) empty_title: SharedString,
    pub(crate) empty_subtitle: SharedString,
}

impl DownloadStrings {
    pub(crate) fn from_translator(translator: &Translator) -> Self {
        Self {
            category_archive: shared(translator.text(keys::CATEGORY_ARCHIVE)),
            category_audio: shared(translator.text(keys::CATEGORY_AUDIO)),
            category_document: shared(translator.text(keys::CATEGORY_DOCUMENT)),
            category_image: shared(translator.text(keys::CATEGORY_IMAGE)),
            category_other: shared(translator.text(keys::CATEGORY_OTHER)),
            category_program: shared(translator.text(keys::CATEGORY_PROGRAM)),
            category_video: shared(translator.text(keys::CATEGORY_VIDEO)),
            col_created: shared(translator.text(keys::COL_CREATED)),
            col_eta: shared(translator.text(keys::COL_ETA)),
            col_file_name: shared(translator.text(keys::COL_FILE_NAME)),
            col_progress: shared(translator.text("colProgress")),
            col_protocol: shared(translator.text("colProtocol")),
            col_queue: shared(translator.text("colQueue")),
            col_size: shared(translator.text(keys::COL_SIZE)),
            col_source: shared(translator.text("colSource")),
            col_speed: shared(translator.text(keys::COL_SPEED)),
            active_transfers: shared(translator.text("taskActiveTransfers")),
            connected_peers: shared(translator.text("taskConnectedPeers")),
            confirm: shared(translator.text("confirm")),
            empty_title: shared(translator.text("emptyTitle")),
            empty_subtitle: shared(translator.text("emptySubtitle")),
            cancel: shared(translator.text("cancel")),
            disconnected: shared(translator.text("localServiceDisconnected")),
            action_failed: shared(translator.text("localServiceActionFailed")),
            metadata_loading: shared(translator.text("statusPreparing")),
            eta_seconds: shared(translator.text("etaSeconds")),
            eta_minutes: shared(translator.text("etaMinutes")),
            eta_hours: shared(translator.text("etaHours")),
            col_status: shared(translator.text(keys::COL_STATUS)),
            later_queue: shared(translator.text(keys::LATER_QUEUE)),
            delete: shared(translator.text(keys::DELETE)),
            delete_task: shared(translator.text("deleteTask")),
            delete_task_and_file: shared(translator.text("deleteTaskAndFile")),
            delete_confirm_with_file: shared(translator.text("deleteConfirmDescWithFile")),
            batch_delete_confirm_with_file: shared(
                translator.text("batchDeleteConfirmDescWithFile"),
            ),
            too_many_windows_hint: shared(translator.text("tooManyWindowsHint")),
            main_queue: shared(translator.text(keys::MAIN_QUEUE)),
            pause: shared(translator.text(keys::PAUSE)),
            pause_all: shared(translator.text("pauseAll")),
            resume: shared(translator.text(keys::RESUME)),
            resume_all: shared(translator.text("resumeAll")),
            open_file: shared(translator.text("openFile")),
            open_folder: shared(translator.text("openFolder")),
            remote_tasks: shared(translator.text("remoteTasksGroup")),
            sidebar_queues: shared(translator.text(keys::SIDEBAR_QUEUES)),
            status_all: shared(translator.text(keys::TAB_ALL)),
            status_completed: shared(translator.text(keys::STATUS_COMPLETED)),
            status_downloading: shared(translator.text(keys::STATUS_DOWNLOADING)),
            status_error: shared(translator.text(keys::STATUS_ERROR)),
            status_incomplete: shared(translator.text("tabDownloading")),
            status_paused: shared(translator.text(keys::STATUS_PAUSED)),
            status_pending: shared(translator.text("statusPending")),
            today: shared(translator.text(keys::TODAY)),
            yesterday: shared(translator.text("yesterday")),
            this_week: shared(translator.text("thisWeek")),
            this_month: shared(translator.text("thisMonth")),
            older: shared(translator.text("older")),
            ungrouped: shared(translator.text("ungroupedTasks")),
            site_bt: shared(translator.text("viewSiteBt")),
            ignore_plugin_retry: shared(translator.text("taskIgnorePluginRetry")),
            ignore_plugin_retry_title: shared(translator.text("taskIgnorePluginRetryTitle")),
            ignore_plugin_retry_msg: shared(translator.text("taskIgnorePluginRetryMsg")),
            boost_download: shared(translator.text("boostDownload")),
            cancel_boost: shared(translator.text("cancelBoost")),
            rename_task: shared(translator.text("renameTask")),
            rename_task_title: shared(translator.text("renameTaskTitle")),
            rename_task_placeholder: shared(translator.text("renameTaskPlaceholder")),
            redownload_task: shared(translator.text("redownloadTask")),
            copy_url: shared(translator.text("copyUrl")),
            move_to_queue: shared(translator.text("moveToQueueAction")),
            open_in_window: shared(translator.text("detail")),
            group_pause_all: shared(translator.text("groupPauseAll")),
            group_resume_all: shared(translator.text("groupResumeAll")),
            group_retry_failed: shared(translator.text("groupRetryFailed")),
            group_open_folder: shared(translator.text("groupOpenFolder")),
            group_copy_source_link: shared(translator.text("groupCopySourceLink")),
            group_delete: shared(translator.text("groupDelete")),
            group_delete_with_files: shared(translator.text("groupDeleteWithFiles")),
            group_delete_confirm_with_file: shared(translator.text("deleteConfirmDescWithFile")),
            open_group_in_window: shared(translator.text("openGroupInWindowAction")),
            search_tasks_placeholder: shared(translator.text("searchTasksPlaceholder")),
            unsupported_drop_hint: shared(translator.text("unsupportedDropHint")),
            sidebar_status: shared(translator.text(keys::SIDEBAR_STATUS)),
            sidebar_devices: shared(translator.text("deviceSection")),
            this_device: shared(translator.text("thisDevice")),
            add_category: shared(translator.text("addCategory")),
            edit_category: shared(translator.text("editCategory")),
            hide_section: shared(translator.text("hideSection")),
            start_queue_action: shared(translator.text("startQueueAction")),
            stop_queue_action: shared(translator.text("stopQueueAction")),
            manage_queue_action: shared(translator.text("manageQueueAction")),
            delete_queue_action: shared(translator.text("deleteQueueAction")),
            queue_delete_confirm_desc: shared(translator.text("queueDeleteConfirmDesc")),
            tab_failed: shared(translator.text("tabError")),
        }
    }

    pub(crate) fn format_eta(&self, seconds: u64) -> SharedString {
        let (template, value) = if seconds < 60 {
            (&self.eta_seconds, seconds.to_string())
        } else if seconds < 3600 {
            (&self.eta_minutes, (seconds / 60).to_string())
        } else {
            (&self.eta_hours, format!("{:.1}", seconds as f64 / 3600.0))
        };
        SharedString::from(template.replace("{n}", &value))
    }

    /// 「删除任务及文件」确认文案：单个带文件名，多个带数量。
    pub(crate) fn delete_with_files_description(
        &self,
        keys: &[crate::model::RowKey],
        store: &crate::model::TaskStore,
    ) -> SharedString {
        if let [key] = keys {
            let name = store
                .get(key)
                .map(|row| row.name.clone())
                .unwrap_or_default();
            SharedString::from(self.delete_confirm_with_file.replace("{fileName}", &name))
        } else {
            SharedString::from(
                self.batch_delete_confirm_with_file
                    .replace("{count}", &keys.len().to_string()),
            )
        }
    }

    /// 「删除任务组及文件」确认文案：复用单任务模板，代入组名。
    pub(crate) fn group_delete_with_files_description(&self, group_name: &str) -> SharedString {
        SharedString::from(
            self.group_delete_confirm_with_file
                .replace("{fileName}", group_name),
        )
    }

    pub(crate) fn state_label(&self, state: crate::model::TaskState) -> SharedString {
        match state {
            crate::model::TaskState::Pending => self.status_pending.clone(),
            crate::model::TaskState::Downloading => self.status_downloading.clone(),
            crate::model::TaskState::Paused => self.status_paused.clone(),
            crate::model::TaskState::Completed => self.status_completed.clone(),
            crate::model::TaskState::Failed => self.status_error.clone(),
        }
    }

    pub(crate) fn date_bucket_label(
        &self,
        bucket: crate::model::view_prefs::DateBucket,
    ) -> SharedString {
        use crate::model::view_prefs::DateBucket;
        match bucket {
            DateBucket::Today => self.today.clone(),
            DateBucket::Yesterday => self.yesterday.clone(),
            DateBucket::ThisWeek => self.this_week.clone(),
            DateBucket::ThisMonth => self.this_month.clone(),
            DateBucket::Older => self.older.clone(),
        }
    }

    /// 分类显示名：内置项走 i18n（`categoryVideo`…），自定义项用 `name`。
    pub(crate) fn category_label(
        &self,
        dto: &fluxdown_protocol::CustomCategoryDto,
    ) -> SharedString {
        match dto.builtin_type.as_deref() {
            Some("all") => self.status_all.clone(),
            Some("video") => self.category_video.clone(),
            Some("audio") => self.category_audio.clone(),
            Some("document") => self.category_document.clone(),
            Some("image") => self.category_image.clone(),
            Some("program") => self.category_program.clone(),
            Some("archive") => self.category_archive.clone(),
            Some("other") => self.category_other.clone(),
            _ => SharedString::from(dto.name.clone()),
        }
    }

    /// 无 host 任务的站点标签：BT 用「BT · Magnet」，其余 `—`。
    pub(crate) fn site_unknown(&self, task: &crate::model::DownloadTaskView) -> SharedString {
        if task.protocol == crate::model::TaskProtocol::Bt {
            self.site_bt.clone()
        } else {
            SharedString::from("—")
        }
    }

    /// 完整本地时间 `YYYY-MM-DD HH:MM:SS`（Unix 秒）：列表「创建时间」列与任务详情共用。
    pub(crate) fn format_datetime(timestamp_secs: i64) -> SharedString {
        use chrono::{Local, TimeZone};
        Local
            .timestamp_opt(timestamp_secs, 0)
            .single()
            .map(|at| SharedString::from(at.format("%Y-%m-%d %H:%M:%S").to_string()))
            .unwrap_or_else(|| SharedString::from("—"))
    }
}

/// 「新建下载」表单文案，键集与 `lib/src/widgets/new_download_dialog.dart` 一致。
#[derive(Clone)]
pub(crate) struct NewDownloadStrings {
    pub(crate) subtitle: SharedString,
    pub(crate) url_label: SharedString,
    pub(crate) url_placeholder: SharedString,
    url_count: SharedString,
    pub(crate) no_valid_url: SharedString,
    pub(crate) open_torrent: SharedString,
    pub(crate) select_torrent: SharedString,
    pub(crate) import_txt: SharedString,
    pub(crate) import_txt_none: SharedString,
    import_txt_found: SharedString,
    pub(crate) save_dir: SharedString,
    pub(crate) save_dir_placeholder: SharedString,
    pub(crate) browse: SharedString,
    pub(crate) threads: SharedString,
    pub(crate) threads_auto: SharedString,
    pub(crate) threads_custom: SharedString,
    pub(crate) threads_custom_hint: SharedString,
    pub(crate) rename: SharedString,
    pub(crate) rename_placeholder: SharedString,
    pub(crate) advanced: SharedString,
    pub(crate) http_auth: SharedString,
    pub(crate) http_auth_desc: SharedString,
    pub(crate) http_auth_user: SharedString,
    pub(crate) http_auth_password: SharedString,
    pub(crate) http_auth_save: SharedString,
    pub(crate) capture_auth_hint: SharedString,
    pub(crate) capture_context_hint: SharedString,
    pub(crate) proxy: SharedString,
    pub(crate) proxy_desc: SharedString,
    pub(crate) proxy_placeholder: SharedString,
    pub(crate) proxy_follow: SharedString,
    pub(crate) proxy_direct: SharedString,
    pub(crate) proxy_system: SharedString,
    pub(crate) proxy_global_manual: SharedString,
    pub(crate) proxy_custom: SharedString,
    pub(crate) proxy_not_configured: SharedString,
    pub(crate) ignore_tls: SharedString,
    pub(crate) ignore_tls_desc: SharedString,
    pub(crate) user_agent: SharedString,
    pub(crate) user_agent_desc: SharedString,
    pub(crate) ua_inherit: SharedString,
    pub(crate) ua_chrome: SharedString,
    pub(crate) ua_firefox: SharedString,
    pub(crate) ua_edge: SharedString,
    pub(crate) ua_safari: SharedString,
    pub(crate) ua_custom: SharedString,
    pub(crate) cookie: SharedString,
    pub(crate) cookie_desc: SharedString,
    pub(crate) cookie_placeholder: SharedString,
    pub(crate) checksum: SharedString,
    pub(crate) checksum_desc: SharedString,
    pub(crate) checksum_placeholder: SharedString,
    pub(crate) headers: SharedString,
    pub(crate) headers_desc: SharedString,
    pub(crate) header_name: SharedString,
    pub(crate) header_value: SharedString,
    pub(crate) add_header: SharedString,
    pub(crate) cancel: SharedString,
    pub(crate) download_later: SharedString,
    pub(crate) start_download: SharedString,
    start_batch: SharedString,
    later_tooltip: SharedString,
    start_tooltip: SharedString,
    pub(crate) main_queue: SharedString,
    pub(crate) later_queue: SharedString,
}

impl NewDownloadStrings {
    pub(crate) fn from_translator(translator: &Translator) -> Self {
        Self {
            subtitle: shared(translator.text("batchDownloadDesc")),
            url_label: shared(translator.text("downloadUrl")),
            url_placeholder: shared(translator.text("batchUrlPlaceholder")),
            url_count: shared(translator.text("urlCount")),
            no_valid_url: shared(translator.text("newDownloadNoValidUrl")),
            open_torrent: shared(translator.text("openTorrentFile")),
            select_torrent: shared(translator.text("selectTorrentFile")),
            import_txt: shared(translator.text("importTxtFile")),
            import_txt_none: shared(translator.text("importTxtNoUrls")),
            import_txt_found: shared(translator.text("importTxtFound")),
            save_dir: shared(translator.text("saveDir")),
            save_dir_placeholder: shared(translator.text("selectSaveDir")),
            browse: shared(translator.text("browse")),
            threads: shared(translator.text("threads")),
            threads_auto: shared(translator.text("auto")),
            threads_custom: shared(translator.text("customThreads")),
            threads_custom_hint: shared(translator.text("customThreadsHint")),
            rename: shared(translator.text("renameOptional")),
            rename_placeholder: shared(translator.text("autoDetectFilename")),
            advanced: shared(translator.text("taskProxyAdvanced")),
            http_auth: shared(translator.text("taskHttpAuth")),
            http_auth_desc: shared(translator.text("taskHttpAuthDesc")),
            http_auth_user: shared(translator.text("taskHttpAuthUser")),
            http_auth_password: shared(translator.text("taskHttpAuthPassword")),
            http_auth_save: shared(translator.text("taskHttpAuthSaveForSite")),
            capture_auth_hint: shared(translator.text("newDownloadCaptureAuthHint")),
            capture_context_hint: shared(translator.text("newDownloadCaptureContextHint")),
            proxy: shared(translator.text("taskProxy")),
            proxy_desc: shared(translator.text("taskProxyDesc")),
            proxy_placeholder: shared(translator.text("taskProxyPlaceholder")),
            proxy_follow: shared(translator.text("taskProxyChoiceFollow")),
            proxy_direct: shared(translator.text("taskProxyChoiceDirect")),
            proxy_system: shared(translator.text("taskProxyChoiceSystem")),
            proxy_global_manual: shared(translator.text("taskProxyChoiceGlobalManual")),
            proxy_custom: shared(translator.text("taskProxyChoiceCustom")),
            proxy_not_configured: shared(translator.text("proxyNotConfigured")),
            ignore_tls: shared(translator.text("taskIgnoreTlsErrors")),
            ignore_tls_desc: shared(translator.text("taskIgnoreTlsErrorsDesc")),
            user_agent: shared(translator.text("userAgent")),
            user_agent_desc: shared(translator.text("userAgentTaskPlaceholder")),
            ua_inherit: shared(translator.text("queueUaInheritGlobal")),
            ua_chrome: shared(translator.text("userAgentPresetChrome")),
            ua_firefox: shared(translator.text("userAgentPresetFirefox")),
            ua_edge: shared(translator.text("userAgentPresetEdge")),
            ua_safari: shared(translator.text("userAgentPresetSafari")),
            ua_custom: shared(translator.text("userAgentPresetCustom")),
            cookie: shared(translator.text("taskCookie")),
            cookie_desc: shared(translator.text("taskCookieDesc")),
            cookie_placeholder: shared(translator.text("taskCookiePlaceholder")),
            checksum: shared(translator.text("taskChecksum")),
            checksum_desc: shared(translator.text("taskChecksumDesc")),
            checksum_placeholder: shared(translator.text("taskChecksumPlaceholder")),
            headers: shared(translator.text("taskHeaders")),
            headers_desc: shared(translator.text("taskHeadersDesc")),
            header_name: shared(translator.text("taskHeadersKeyPlaceholder")),
            header_value: shared(translator.text("taskHeadersValuePlaceholder")),
            add_header: shared(translator.text("taskHeadersAdd")),
            cancel: shared(translator.text("cancel")),
            download_later: shared(translator.text("downloadLater")),
            start_download: shared(translator.text("startDownload")),
            start_batch: shared(translator.text("startBatchDownload")),
            later_tooltip: shared(translator.text("laterIntoQueueTooltip")),
            start_tooltip: shared(translator.text("startIntoQueueTooltip")),
            main_queue: shared(translator.text(keys::MAIN_QUEUE)),
            later_queue: shared(translator.text(keys::LATER_QUEUE)),
        }
    }

    /// `{count} 个链接`。
    pub(crate) fn format_url_count(&self, count: usize) -> SharedString {
        SharedString::from(self.url_count.replace("{count}", &count.to_string()))
    }

    /// `已导入 {count} 个链接`。
    pub(crate) fn format_import_found(&self, count: usize) -> SharedString {
        SharedString::from(self.import_txt_found.replace("{count}", &count.to_string()))
    }

    /// 开始按钮标签：单条 = `开始下载`，多条 = `下载 {count} 个文件`。
    pub(crate) fn format_start(&self, count: usize) -> SharedString {
        if count > 1 {
            SharedString::from(self.start_batch.replace("{count}", &count.to_string()))
        } else {
            self.start_download.clone()
        }
    }

    /// 「稍后下载」按钮提示：`创建任务但不开始，加入「{name}」…`。
    pub(crate) fn format_later_tooltip(&self, queue_name: &str) -> SharedString {
        SharedString::from(self.later_tooltip.replace("{name}", queue_name))
    }

    /// 「开始下载」按钮提示：`下载到「{name}」…`。
    pub(crate) fn format_start_tooltip(&self, queue_name: &str) -> SharedString {
        SharedString::from(self.start_tooltip.replace("{name}", queue_name))
    }

    /// 队列显示名：内置队列本地化，自定义队列用用户命名。
    pub(crate) fn queue_name(&self, queue_id: &str, name: &str) -> SharedString {
        match queue_id {
            fluxdown_protocol::MAIN_QUEUE_ID => self.main_queue.clone(),
            fluxdown_protocol::LATER_QUEUE_ID => self.later_queue.clone(),
            _ => shared(name),
        }
    }
}

fn shared(value: &str) -> SharedString {
    SharedString::from(value.to_owned())
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::DownloadStrings;

    #[test]
    fn datetime_keeps_seconds_across_midnight_and_rejects_out_of_range() {
        let before = Local
            .with_ymd_and_hms(2026, 3, 18, 23, 59, 59)
            .single()
            .expect("valid local time");
        let after = Local
            .with_ymd_and_hms(2026, 3, 19, 0, 0, 0)
            .single()
            .expect("valid local time");
        assert_eq!(
            DownloadStrings::format_datetime(before.timestamp()).as_ref(),
            "2026-03-18 23:59:59"
        );
        assert_eq!(
            DownloadStrings::format_datetime(after.timestamp()).as_ref(),
            "2026-03-19 00:00:00"
        );
        assert_eq!(DownloadStrings::format_datetime(i64::MAX).as_ref(), "—");
    }
}
