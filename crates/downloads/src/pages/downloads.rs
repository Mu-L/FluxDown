use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    actions::{
        ClearFinished, ClearSelection, CopySelectedUrl, CycleDensity, CycleGroupBy, CycleSort,
        DeleteSelected, DeleteSelectedWithFiles, FocusSearch, KEY_CONTEXT, NewDownload,
        OpenQueueManager, OpenSelected, OpenSelectedInWindow, OpenTorrentFile, PauseAll,
        PauseSelected, RedownloadSelected, RenameSelected, ResumeAll, ResumeSelected,
        RevealSelected, SelectAllTasks, ToggleBoostSelected, ToggleDetailPanel,
        TogglePauseSelected,
    },
    components::{
        task_table::{DownloadTableDelegate, SelectionSummary, TableFilter, ToolbarCommand},
        title_bar::DownloadTitleBar,
    },
    controller::{DownloadsCommand, DownloadsController, DownloadsPort},
    model::{
        DownloadFilter, DownloadStatusFilter, RowKey, SidebarSection, SidebarSelection,
        StatusFolderMotion, TaskState,
        view_prefs::{DetailPlacement, VIEW_PREFS_KEY, ViewGroupBy, ViewPrefs},
    },
    pages::new_download::{NewDownloadContext, NewDownloadSubmission, build_new_download_context},
    pages::task_detail::TaskDetailView,
    strings::DownloadStrings,
};
use fluxdown_ui_components::{ControlExt as _, FluxIcon, TOOLBAR_BUTTON_SIZE};
use fluxdown_ui_i18n::Translator;
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, ExternalPaths, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement, PathPromptOptions, Render, SharedString,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon, ResizableState, WindowExt as _, h_flex, h_resizable,
    input::{Input, InputEvent, InputState},
    resizable_panel,
    table::{TableEvent, TableState},
    v_flex, v_resizable,
};

pub(crate) const SIDEBAR_MOTION_DURATION: Duration = Duration::from_millis(200);
const PREFS_DEBOUNCE: Duration = Duration::from_millis(300);
/// 搜索框输入防抖。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);
/// 「在独立窗口打开」一次最多开的窗口数。
pub const MAX_TASK_WINDOWS_PER_ACTION: usize = 8;

/// 宿主注入的「新建下载」入口：由 app 打开独立对话框窗口。
/// 表单初值由下载页在点击瞬间算好传入，打开方不得再回读 `DownloadView`
///（此时实体正处于 update 中）。
pub type NewDownloadOpener = Rc<dyn Fn(NewDownloadContext, &mut Window, &mut App)>;
/// 以 id 打开一个窗口 / 编辑器（任务窗口、组窗口、分类编辑）。
pub type IdOpener = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// 无参数窗口入口（队列管理）。
pub type PlainOpener = Rc<dyn Fn(&mut Window, &mut App)>;
/// 分类编辑入口：`Some(id)` 编辑现有分类，`None` 新建。
pub type CategoryEditorOpener = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

/// app 注入的跨窗口 / 跨能力入口；未注入的入口对应按钮无动作。
#[derive(Clone, Default)]
pub struct DownloadHostActions {
    pub open_new_download: Option<NewDownloadOpener>,
    pub open_task_window: Option<IdOpener>,
    pub open_group_window: Option<IdOpener>,
    pub open_queue_manager: Option<PlainOpener>,
    /// `Some(id)` 编辑现有分类，`None` 新建。
    pub open_category_editor: Option<CategoryEditorOpener>,
    /// 完成后关机的只读状态投影（`None` = Resident 未装配）。
    pub shutdown_status: Option<crate::model::shutdown::SharedShutdownStatus>,
    /// 状态栏发起关机请求的端口。
    pub shutdown: Option<crate::model::shutdown::ShutdownPort>,
}

/// 下载能力的顶层页面。
pub struct DownloadView {
    pub(crate) controller: DownloadsController,
    /// 详情面板 / 弹出任务窗口按需构造 `TaskDetailView` 用（`DownloadsController`
    /// 不暴露内部 port）。
    pub(crate) port: Arc<dyn DownloadsPort>,
    /// 状态栏新增文案的实时查表源（现有渲染热路径用 `strings` 缓存字段）。
    pub(crate) translator: Entity<Translator>,
    pub(crate) strings: DownloadStrings,
    pub(crate) selected_item: SidebarSelection,
    pub(crate) expanded_status: Option<DownloadStatusFilter>,
    pub(crate) folder_motion: StatusFolderMotion,
    pub(crate) folder_motion_started_at: Option<Instant>,
    pub(crate) section_expanded: HashMap<SidebarSection, bool>,
    pub(crate) section_motion_from: HashMap<SidebarSection, f32>,
    pub(crate) section_motion_started_at: HashMap<SidebarSection, Instant>,
    pub(crate) table_state: Entity<TableState<DownloadTableDelegate>>,
    pub(crate) host: DownloadHostActions,
    pub(crate) last_error: Option<SharedString>,
    pub(crate) resizable_state: Entity<ResizableState>,
    resizable_state_initialized: bool,
    /// 根元素 focus handle：右键菜单 action_context 分派目标，`escape`
    /// 清空搜索框后也交回给它。
    pub(crate) focus_handle: FocusHandle,
    /// 顶栏搜索框状态（输入框渲染在 [`DownloadTitleBar`] 插槽里）。
    pub(crate) search_input: Entity<InputState>,
    /// 已下发给搜索框的 placeholder（语言切换后需重下发）。
    search_placeholder: SharedString,
    /// 搜索防抖代数。
    search_generation: Rc<Cell<u64>>,
    prefs_generation: Rc<Cell<u64>>,
    /// 已从偏好加载过视图设置（缺省分组维度只在首次决定）。
    prefs_loaded: bool,
    /// 停靠详情面板（`None` = 尚未选中过任何任务）。
    pub(crate) detail: Option<Entity<TaskDetailView>>,
    /// 详情面板 / 主内容拆分的独立 resizable 状态。
    pub(crate) detail_resizable_state: Entity<ResizableState>,
    /// 上次渲染时的选中投影；表格选中变化时与之比较，变了才重绘本页（浮动选择条）。
    pub(crate) selection_summary: Cell<SelectionSummary>,
}

impl DownloadView {
    /// 创建下载页面，并订阅共享翻译状态。
    pub fn new(
        translator: Entity<Translator>,
        port: Arc<dyn DownloadsPort>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let strings = DownloadStrings::from_translator(translator.read(cx));
        let controller = DownloadsController::new(Arc::clone(&port));
        let store = Rc::clone(controller.store());
        let focus_handle = cx.focus_handle();
        let table_state = cx.new(|cx| {
            TableState::new(
                DownloadTableDelegate::new(strings.clone(), store),
                window,
                cx,
            )
            .row_selectable(false)
            .col_selectable(false)
        });
        let weak_self = cx.weak_entity();
        table_state.update(cx, |table, _| {
            table.delegate_mut().set_host(weak_self);
            table
                .delegate_mut()
                .set_action_context(focus_handle.clone());
        });
        let strings_placeholder = strings.search_tasks_placeholder.clone();
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(strings.search_tasks_placeholder.clone())
        });
        cx.subscribe(&search_input, Self::handle_search_changed)
            .detach();

        cx.observe(&translator, |this, translator, cx| {
            this.set_strings(DownloadStrings::from_translator(translator.read(cx)), cx);
        })
        .detach();
        cx.subscribe_in(&table_state, window, Self::handle_table_event)
            .detach();
        // 选中变化只通知表格实体；浮动选择条依赖它，按投影差异重绘本页，
        // 避免表格滚动 / 悬停等高频 notify 带着整页重绘。
        cx.observe(&table_state, |this, table_state, cx| {
            let selection = table_state.read(cx).delegate().selection_summary();
            if this.selection_summary.get() != selection {
                this.selection_summary.set(selection);
                cx.notify();
            }
        })
        .detach();

        Self {
            controller,
            port,
            translator: translator.clone(),
            strings,
            selected_item: SidebarSelection::Download(DownloadFilter::ALL),
            expanded_status: Some(DownloadStatusFilter::All),
            folder_motion: StatusFolderMotion::settled(Some(DownloadStatusFilter::All)),
            folder_motion_started_at: None,
            section_expanded: SidebarSection::ALL
                .into_iter()
                .map(|section| (section, true))
                .collect(),
            section_motion_from: HashMap::new(),
            section_motion_started_at: HashMap::new(),
            table_state,
            host: DownloadHostActions::default(),
            last_error: None,
            resizable_state: cx.new(|_| ResizableState::default()),
            resizable_state_initialized: false,
            focus_handle,
            search_input,
            search_placeholder: strings_placeholder,
            search_generation: Rc::new(Cell::new(0)),
            prefs_generation: Rc::new(Cell::new(0)),
            prefs_loaded: false,
            detail: None,
            detail_resizable_state: cx.new(|_| ResizableState::default()),
            selection_summary: Cell::new(SelectionSummary::default()),
        }
    }

    /// 注入宿主入口（新建下载 / 任务窗口 / 队列管理…）。
    pub fn set_host_actions(&mut self, host: DownloadHostActions) {
        self.host = host;
    }

    /// 创建挂到 shell 统一顶栏的下载页插槽（搜索、视图选项、新建）。
    pub fn new_title_bar(&self, cx: &mut Context<Self>) -> Entity<DownloadTitleBar> {
        let view = cx.entity();
        let translator = self.translator.clone();
        let search_input = self.search_input.clone();
        let table_state = self.table_state.clone();
        cx.new(|cx| DownloadTitleBar::new(&view, translator, search_input, table_state, cx))
    }

    /// 「新建下载」表单的环境快照：保存目录 / 默认队列 / 线程数初值与队列候选；
    /// 队列优先侧栏当前筛选（规则见 [`build_new_download_context`]）。
    #[must_use]
    pub fn new_download_context(&self) -> NewDownloadContext {
        let controller = &self.controller;
        let selected_queue = match &self.selected_item {
            SidebarSelection::Queue(queue_id) => Some(queue_id.as_str()),
            _ => None,
        };
        build_new_download_context(
            controller.config(),
            &controller.runtime_stats().save_dir,
            controller.preferences(),
            controller.queues(),
            selected_queue,
        )
    }

    /// 打开「新建下载」窗口（可预填链接）。
    pub(crate) fn open_new_download_with(
        &self,
        initial_urls: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(opener) = self.host.open_new_download.clone() else {
            return;
        };
        let mut context = self.new_download_context();
        context.initial_urls = initial_urls;
        opener(context, window, cx);
    }

    pub(crate) fn open_new_download(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_download_with(Vec::new(), window, cx);
    }

    /// 按表单提交创建任务 / 确认外部捕获；对话框确认后由宿主调用。
    ///
    /// 命令逐条执行，任一失败即在页面横幅提示；同时把本次保存目录记入本机偏好（无条件
    /// 记录，开关开启后立即生效）。返回的 future 在全部完成后给出是否全部成功。
    pub fn create_download(
        &mut self,
        submission: NewDownloadSubmission,
        cx: &mut Context<Self>,
    ) -> gpui::Task<bool> {
        if self.controller.is_stale() {
            return gpui::Task::ready(false);
        }
        if let Some(remember) = submission.remember_save_dir_command() {
            // 记录目录是尽力而为：失败不影响任务创建，也不进横幅。
            let remember = self.controller.execute(remember);
            cx.background_spawn(async move {
                let _ = remember.await;
            })
            .detach();
        }
        let futures = submission
            .into_commands()
            .into_iter()
            .map(|command| self.controller.execute(command))
            .collect::<Vec<_>>();
        cx.spawn(async move |this, cx| {
            let mut failed = false;
            for future in futures {
                failed |= future.await.is_err();
            }
            let _ = this.update(cx, |this, cx| {
                this.last_error = failed.then(|| this.strings.action_failed.clone());
                cx.notify();
            });
            !failed
        })
    }

    pub fn replace_snapshot(
        &mut self,
        snapshot: &fluxdown_protocol::AgentSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.controller.replace_snapshot(snapshot);
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.replace_snapshot(snapshot, cx));
        }
        self.reconcile_queue_selection();
        self.last_error = (!snapshot.daemon_connected).then(|| self.strings.disconnected.clone());
        self.load_view_prefs(cx);
        self.sync_delegate_context(cx);
        self.refresh_tasks(cx);
        self.sync_detail_panel(cx);
    }

    pub fn apply_event(&mut self, event: &fluxdown_protocol::ServiceEvent, cx: &mut Context<Self>) {
        let table_changed = self.controller.apply_event(event);
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.apply_event(event, cx));
        }
        if let fluxdown_protocol::ServiceEvent::Agent(
            fluxdown_protocol::AgentEvent::PreferencesChanged(_),
        ) = event
        {
            self.load_view_prefs(cx);
        }
        if table_changed {
            if matches!(
                event,
                fluxdown_protocol::ServiceEvent::Agent(
                    fluxdown_protocol::AgentEvent::Daemon(
                        fluxdown_protocol::DaemonEvent::QueuesChanged(_)
                            | fluxdown_protocol::DaemonEvent::SnapshotReplaced(_)
                    ) | fluxdown_protocol::AgentEvent::DaemonSnapshotReplaced(_)
                )
            ) {
                self.reconcile_queue_selection();
            }
            self.sync_delegate_context(cx);
            self.refresh_tasks(cx);
            self.sync_detail_panel(cx);
        } else {
            cx.notify();
        }
    }

    /// 队列删除/daemon 快照更替后，不能继续筛选已不存在的队列。
    fn reconcile_queue_selection(&mut self) {
        if let SidebarSelection::Queue(queue_id) = &self.selected_item
            && !self
                .controller
                .queues()
                .iter()
                .any(|queue| &queue.queue_id == queue_id)
        {
            self.selected_item = SidebarSelection::Download(DownloadFilter::ALL);
        }
    }

    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        self.last_error = Some(self.strings.disconnected.clone());
        self.controller.mark_stale();
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.mark_stale(cx));
        }
        cx.notify();
    }

    /// 偏好 → 表格视图设置；无偏好时首次按「存在组任务」决定默认分组维度。
    fn load_view_prefs(&mut self, cx: &mut Context<Self>) {
        let prefs = match self.controller.preference(VIEW_PREFS_KEY) {
            Some(value) => ViewPrefs::from_value(value),
            None if self.prefs_loaded => return,
            None => {
                let mut prefs = ViewPrefs::default();
                let has_groups = self
                    .controller
                    .store()
                    .local()
                    .iter()
                    .any(|task| !task.group_id.is_empty());
                if has_groups {
                    prefs.group_by = ViewGroupBy::Group;
                }
                prefs
            }
        };
        self.prefs_loaded = true;
        self.table_state.update(cx, |table, _| {
            table.delegate_mut().set_prefs(prefs);
        });
    }

    /// 队列名 / 组名 / 分类 / 设备别名同步进表格代理（只在变化时触发重算）。
    fn sync_delegate_context(&mut self, cx: &mut Context<Self>) {
        let queues: Vec<(String, String)> = self
            .controller
            .queues()
            .iter()
            .map(|queue| (queue.queue_id.clone(), queue.name.clone()))
            .collect();
        let groups: HashMap<String, String> = self
            .controller
            .groups()
            .iter()
            .map(|group| (group.group_id.clone(), group.name.clone()))
            .collect();
        let mut aliases: HashMap<String, Vec<String>> = HashMap::new();
        for device in self.controller.cloud_devices() {
            aliases
                .entry(device.device_id.clone())
                .or_default()
                .push(device.id.clone());
        }
        for device in self.controller.linked_devices() {
            aliases
                .entry(device.fingerprint.clone())
                .or_default()
                .push(device.name.clone());
        }
        let categories = Rc::clone(self.controller.categories());
        if let Some(detail) = self.detail.clone() {
            detail.update(cx, |detail, cx| detail.set_queue_names(queues.clone(), cx));
        }
        self.table_state.update(cx, |table, _| {
            let delegate = table.delegate_mut();
            delegate.set_queue_names(queues);
            delegate.set_group_names(groups);
            delegate.set_device_aliases(aliases);
            delegate.set_categories(categories);
        });
    }

    /// 停靠详情面板：把当前任务的最新 DTO 推给面板刷新（事件 / 快照后调用）。
    fn sync_detail_panel(&mut self, cx: &mut Context<Self>) {
        if self.controller.is_stale() {
            return;
        }
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let task_id = detail.read(cx).task_id().to_owned();
        let dto = self.controller.task_dto(&task_id).cloned();
        let runtime = self.controller.task_runtime(&task_id).cloned();
        detail.update(cx, |detail, cx| {
            detail.sync(dto, cx);
            detail.sync_runtime(runtime, cx);
        });
    }

    pub(crate) fn refresh_tasks(&mut self, cx: &mut Context<Self>) {
        let filter = TableFilter::from(&self.selected_item);
        self.table_state.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.set_filter(filter);
            delegate.refresh_view();
            // 行变化只需重绘（行数每帧从代理读取）；只有列配置变了才重建
            // `col_groups`，否则进度节拍会覆盖拖拽中的列宽。
            if delegate.take_columns_dirty() {
                table.refresh(cx);
            }
        });
        cx.notify();
    }

    pub(crate) fn select_sidebar_item(
        &mut self,
        selection: SidebarSelection,
        cx: &mut Context<Self>,
    ) {
        if self.selected_item == selection {
            return;
        }
        self.selected_item = selection;
        self.refresh_tasks(cx);
    }

    fn set_strings(&mut self, strings: DownloadStrings, cx: &mut Context<Self>) {
        self.strings = strings.clone();
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().set_strings(strings);
            table.delegate_mut().refresh_view();
            table.refresh(cx);
        });
        cx.notify();
    }

    fn handle_table_event(
        &mut self,
        table_state: &Entity<TableState<DownloadTableDelegate>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::RightClickedRow(Some(row_ix)) => {
                // 注意：这里不调用 `set_right_clicked_row(None, ..)`——该方法专为
                // 「打开表头菜单时抑制同时出现的行菜单」设计（见其文档），若在
                // 行右键后立即清空，会在 gpui-component 内部 `window.defer` 读取
                // `right_clicked_row` 构建菜单之前把它清掉，导致右键菜单永远不
                // 会出现。这里只需要更新选中集合，行高亮 / 菜单锚点交给表格自身
                // 维护的 `right_clicked_row` 状态。
                let row_ix = *row_ix;
                table_state.update(cx, |table, _| {
                    if let Some(key) = table.delegate().row_key_at(row_ix) {
                        table.delegate_mut().select_task_for_context_menu(key);
                    }
                });
            }
            TableEvent::DoubleClickedRow(row_ix) => {
                let Some(key) = table_state.read(cx).delegate().row_key_at(*row_ix) else {
                    return;
                };
                let completed = self
                    .controller
                    .store()
                    .get(&key)
                    .is_some_and(|row| row.state == TaskState::Completed);
                if completed && key.is_local() {
                    self.execute_commands(
                        vec![DownloadsCommand::OpenTask {
                            task_id: key.task_id().to_owned(),
                        }],
                        cx,
                    );
                } else {
                    self.open_detail_for(key, window, cx);
                }
            }
            TableEvent::ColumnWidthsChanged(widths) => {
                table_state.update(cx, |table, _| {
                    table.delegate_mut().sync_column_widths(widths);
                });
                self.schedule_persist_prefs(cx);
            }
            TableEvent::MoveColumn(..) => {
                self.schedule_persist_prefs(cx);
            }
            _ => {}
        }
    }

    /// 双击非完成行：打开 / 聚焦停靠详情面板并切换到该任务。
    pub(crate) fn open_detail_for(
        &mut self,
        key: RowKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !key.is_local() {
            return;
        }
        let task_id = key.task_id().to_owned();
        match self.detail.clone() {
            Some(detail) => {
                detail.update(cx, |detail, cx| detail.set_task(task_id.clone(), cx));
            }
            None => {
                let store = Rc::clone(self.controller.store());
                let host = self.host.clone();
                let translator = self.translator.clone();
                let port = Arc::clone(&self.port);
                let task_id_for_view = task_id.clone();
                let detail = cx.new(|cx| {
                    TaskDetailView::new_docked(
                        translator,
                        task_id_for_view,
                        store,
                        port,
                        host,
                        window,
                        cx,
                    )
                });
                if self.controller.is_stale() {
                    detail.update(cx, |detail, cx| detail.mark_stale(cx));
                }
                self.detail = Some(detail);
            }
        }
        self.sync_detail_panel(cx);
        self.mutate_prefs(|prefs| prefs.detail_open = true, cx);
    }

    fn on_toggle_detail_panel(
        &mut self,
        _: &ToggleDetailPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        self.mutate_prefs(|prefs| prefs.detail_open = !prefs.detail_open, cx);
    }

    fn on_close_detail_panel(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.mutate_prefs(|prefs| prefs.detail_open = false, cx);
    }

    fn on_toggle_detail_placement(&mut self, cx: &mut Context<Self>) {
        self.mutate_prefs(
            |prefs| prefs.detail_placement = prefs.detail_placement.toggled(),
            cx,
        );
    }

    /// 面板「弹出为窗口」：交给宿主开独立任务窗口后关闭面板。
    fn on_pop_out_detail(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let task_id = detail.read(cx).task_id().to_owned();
        if let Some(opener) = self.host.open_task_window.clone() {
            opener(task_id, window, cx);
        }
        self.on_close_detail_panel(cx);
    }

    /// 视图偏好写回：300ms 防抖后合并成一次 `SetLocalPreference`。
    pub(crate) fn schedule_persist_prefs(&mut self, cx: &mut Context<Self>) {
        let generation = self.prefs_generation.get().wrapping_add(1);
        self.prefs_generation.set(generation);
        let marker = Rc::clone(&self.prefs_generation);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREFS_DEBOUNCE).await;
            if marker.get() != generation {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let value = this.table_state.update(cx, |table, _| {
                    let delegate = table.delegate_mut();
                    let columns = delegate.column_prefs();
                    delegate.prefs_mut().columns = columns;
                    delegate.prefs().to_value()
                });
                let future = this
                    .controller
                    .execute(DownloadsCommand::SetLocalPreference {
                        key: VIEW_PREFS_KEY,
                        value,
                    });
                cx.background_spawn(async move {
                    let _ = future.await;
                })
                .detach();
            });
        })
        .detach();
    }

    pub(crate) fn mutate_prefs(
        &mut self,
        mutate: impl FnOnce(&mut ViewPrefs),
        cx: &mut Context<Self>,
    ) {
        self.table_state.update(cx, |table, cx| {
            mutate(table.delegate_mut().prefs_mut());
            table.delegate_mut().refresh_view();
            table.refresh(cx);
        });
        self.schedule_persist_prefs(cx);
        cx.notify();
    }

    /// 搜索框内容变化：150ms 防抖后写入表格代理并重算可见行。
    fn handle_search_changed(
        &mut self,
        _input: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let generation = self.search_generation.get().wrapping_add(1);
        self.search_generation.set(generation);
        let marker = Rc::clone(&self.search_generation);
        let input = self.search_input.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            if marker.get() != generation {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                let query = input.read(cx).value().to_string();
                this.table_state.update(cx, |table, cx| {
                    table.delegate_mut().set_query(&query);
                    if table.delegate_mut().refresh_view() {
                        table.refresh(cx);
                    }
                });
            });
        })
        .detach();
    }

    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// 搜索框内 `escape`：清空查询并把焦点交回下载页根元素。
    pub(crate) fn on_search_escape(
        &mut self,
        _: &gpui_component::input::Escape,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().set_query("");
            if table.delegate_mut().refresh_view() {
                table.refresh(cx);
            }
        });
        self.focus_handle.focus(window, cx);
    }

    /// 内联重命名对话框（单选本地任务）；确认后写 `DownloadsCommand::Rename`。
    fn on_rename_selected(
        &mut self,
        _: &RenameSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let [key] = selected.as_slice() else {
            return;
        };
        if !key.is_local() {
            return;
        }
        let task_id = key.task_id().to_owned();
        let current_name = self
            .controller
            .store()
            .get(key)
            .map(|row| row.name.clone())
            .unwrap_or_default();
        let title = self.strings.rename_task_title.clone();
        let field_label = self.strings.col_file_name.clone();
        let placeholder = self.strings.rename_task_placeholder.clone();
        let ok_label = self.strings.confirm.clone();
        let cancel_label = self.strings.cancel.clone();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(current_name)
                .placeholder(placeholder)
        });
        let dialog_input = input.clone();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let content_input = dialog_input.clone();
            let ok_input = dialog_input.clone();
            let this = this.clone();
            let task_id = task_id.clone();
            let field_label = field_label.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .w(px(520.))
                .content(move |content, _, cx| {
                    content.child(fluxdown_ui_components::form(cx).child(
                        fluxdown_ui_components::form_field(
                            field_label.clone(),
                            Input::new(&content_input).control(cx).w_full(),
                            None,
                            cx,
                        ),
                    ))
                })
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Confirm,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let file_name = ok_input.read(cx).value().trim().to_owned();
                    if file_name.is_empty() {
                        return false;
                    }
                    let task_id = task_id.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::Rename { task_id, file_name }],
                            cx,
                        );
                    });
                    true
                })
        });
        input.update(cx, |state, cx| state.focus(window, cx));
    }

    /// 插件重试挂起确认（右键菜单经 [`DownloadTableDelegate`] 回调）。
    pub(crate) fn confirm_ignore_plugin_retry(
        &mut self,
        task_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.strings.ignore_plugin_retry_title.clone();
        let description = self.strings.ignore_plugin_retry_msg.clone();
        let ok_label = self.strings.ignore_plugin_retry.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let task_id = task_id.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Confirm,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let task_id = task_id.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::IgnorePluginRetry { task_id }],
                            cx,
                        );
                    });
                    true
                })
        });
    }

    /// 将当前选中的本地任务批量移动到目标队列（右键菜单「移动到队列」）。
    pub(crate) fn move_selected_to_queue(&mut self, queue_id: String, cx: &mut Context<Self>) {
        let commands: Vec<DownloadsCommand> = self
            .table_state
            .read(cx)
            .delegate()
            .selected_keys()
            .into_iter()
            .filter(RowKey::is_local)
            .map(|key| DownloadsCommand::MoveToQueue {
                task_id: key.task_id().to_owned(),
                queue_id: queue_id.clone(),
            })
            .collect();
        self.execute_commands(commands, cx);
    }

    pub(crate) fn group_pause_all(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(vec![DownloadsCommand::GroupPause { group_id }], cx);
    }

    pub(crate) fn group_resume_all(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(vec![DownloadsCommand::GroupResume { group_id }], cx);
    }

    /// 组内失败成员逐个恢复。
    pub(crate) fn group_retry_failed(&mut self, group_id: String, cx: &mut Context<Self>) {
        let commands: Vec<DownloadsCommand> = self
            .controller
            .store()
            .local()
            .iter()
            .filter(|task| task.group_id == group_id && task.state == TaskState::Failed)
            .map(|task| DownloadsCommand::Resume {
                task_id: task.key.task_id().to_owned(),
            })
            .collect();
        self.execute_commands(commands, cx);
    }

    /// 打开组内首个成员所在目录。
    pub(crate) fn group_open_folder(&mut self, group_id: String, cx: &mut Context<Self>) {
        let Some(task_id) = self
            .controller
            .store()
            .local()
            .iter()
            .find(|task| task.group_id == group_id)
            .map(|task| task.key.task_id().to_owned())
        else {
            return;
        };
        self.execute_commands(vec![DownloadsCommand::RevealTask { task_id }], cx);
    }

    pub(crate) fn group_copy_source_link(&mut self, group_id: String, cx: &mut Context<Self>) {
        let Some(url) = self
            .controller
            .group_summaries()
            .iter()
            .find(|group| group.id == group_id)
            .map(|group| group.origin_url.clone())
            .filter(|url| !url.is_empty())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
    }

    pub(crate) fn group_delete(&mut self, group_id: String, cx: &mut Context<Self>) {
        self.execute_commands(
            vec![DownloadsCommand::GroupDelete {
                group_id,
                delete_files: false,
            }],
            cx,
        );
    }

    /// 「删除组及文件」二次确认。
    pub(crate) fn confirm_group_delete_with_files(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .controller
            .group_summaries()
            .iter()
            .find(|group| group.id == group_id)
            .map_or_else(|| group_id.clone(), |group| group.name.clone());
        let title = self.strings.group_delete_with_files.clone();
        let description = self.strings.group_delete_with_files_description(&name);
        let ok_label = self.strings.delete.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let group_id = group_id.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let group_id = group_id.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.execute_commands(
                            vec![DownloadsCommand::GroupDelete {
                                group_id,
                                delete_files: true,
                            }],
                            cx,
                        );
                    });
                    true
                })
        });
    }

    pub(crate) fn open_group_in_window(
        &mut self,
        group_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(opener) = self.host.open_group_window.clone() {
            opener(group_id, window, cx);
        }
    }

    /// 拖放导入：`.torrent` 直接建任务；`.txt`/`.url`/`.list`（≤1MB）按行提取
    /// 受支持的链接后打开新建下载窗口预填；其他文件类型提示不支持。
    fn on_paths_dropped(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut torrent_commands = Vec::new();
        let mut urls = Vec::new();
        let mut unsupported = false;
        for path in paths.paths() {
            let ext = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase);
            match ext.as_deref() {
                Some("torrent") => torrent_commands.push(DownloadsCommand::SubmitTorrentFile {
                    path: path.display().to_string(),
                }),
                Some("txt" | "url" | "list") => match read_drop_text_file(path) {
                    Some(text) => urls.extend(parse_drop_urls(&text)),
                    None => unsupported = true,
                },
                _ => unsupported = true,
            }
        }
        if !torrent_commands.is_empty() {
            self.execute_commands(torrent_commands, cx);
        }
        if !urls.is_empty() {
            self.open_new_download_with(urls, window, cx);
        }
        if unsupported {
            window.push_notification(self.strings.unsupported_drop_hint.clone(), cx);
        }
    }

    fn guard_letter_key(window: &mut Window, cx: &mut Context<Self>) -> bool {
        if window.has_focused_input(cx) {
            cx.propagate();
            return false;
        }
        true
    }

    fn on_select_all(&mut self, _: &SelectAllTasks, _: &mut Window, cx: &mut Context<Self>) {
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().select_all_tasks();
            cx.notify();
        });
    }

    fn on_clear_selection(&mut self, _: &ClearSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_table_selection(cx);
    }

    /// 清空表格选中（Esc 快捷键与浮动选择条「取消选择」共用）。
    pub(crate) fn clear_table_selection(&mut self, cx: &mut Context<Self>) {
        self.table_state.update(cx, |table, cx| {
            table.delegate_mut().clear_selection();
            cx.notify();
        });
    }

    fn on_pause_selected(&mut self, _: &PauseSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Pause, cx);
    }

    fn on_resume_selected(&mut self, _: &ResumeSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Resume, cx);
    }

    fn on_toggle_pause_selected(
        &mut self,
        _: &TogglePauseSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let store = self.controller.store();
        let any_active = selected.iter().any(|key| {
            store
                .get(key)
                .is_some_and(|row| matches!(row.state, TaskState::Downloading | TaskState::Pending))
        });
        let action = if any_active {
            ToolbarCommand::Pause
        } else {
            ToolbarCommand::Resume
        };
        self.execute_toolbar(action, cx);
    }

    fn on_delete_selected(&mut self, _: &DeleteSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Delete, cx);
    }

    fn on_delete_selected_with_files(
        &mut self,
        _: &DeleteSelectedWithFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_selected_with_files(window, cx);
    }

    /// 对当前选中集合发起「删除任务和文件」（带二次确认）；空选择为 no-op。
    pub(crate) fn delete_selected_with_files(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        if selected.is_empty() {
            return;
        }
        self.confirm_delete_with_files(selected, window, cx);
    }

    /// 「删除任务及文件」二次确认。
    pub(crate) fn confirm_delete_with_files(
        &mut self,
        keys: Vec<RowKey>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = self.strings.delete_task_and_file.clone();
        let description = self
            .strings
            .delete_with_files_description(&keys, self.controller.store());
        let ok_label = self.strings.delete.clone();
        let cancel_label = self.strings.cancel.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, cx| {
            let this = this.clone();
            let keys = keys.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .description(description.clone())
                .footer(fluxdown_ui_components::dialog_footer(
                    Some(cancel_label.clone()),
                    ok_label.clone(),
                    fluxdown_ui_components::DialogIntent::Destructive,
                    cx,
                ))
                .on_ok(move |_, _, cx| {
                    let commands: Vec<DownloadsCommand> = keys
                        .iter()
                        .map(|key| {
                            if key.is_local() {
                                DownloadsCommand::Delete {
                                    task_id: key.task_id().to_owned(),
                                    delete_files: true,
                                }
                            } else {
                                DownloadsCommand::RemoteCommand(serde_json::json!({
                                    "taskId": key.task_id(),
                                    "action": "delete",
                                    "deleteFiles": true,
                                }))
                            }
                        })
                        .collect();
                    let _ = this.update(cx, |this, cx| this.execute_commands(commands, cx));
                    true
                })
        });
    }

    fn on_pause_all(&mut self, _: &PauseAll, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::PauseAll, cx);
    }

    fn on_resume_all(&mut self, _: &ResumeAll, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::ResumeAll, cx);
    }

    fn on_open_selected(&mut self, _: &OpenSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Open, cx);
    }

    fn on_reveal_selected(&mut self, _: &RevealSelected, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_toolbar(ToolbarCommand::Reveal, cx);
    }

    fn on_open_selected_in_window(
        &mut self,
        _: &OpenSelectedInWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(opener) = self.host.open_task_window.clone() else {
            return;
        };
        let selected: Vec<RowKey> = self
            .table_state
            .read(cx)
            .delegate()
            .selected_keys()
            .into_iter()
            .filter(RowKey::is_local)
            .collect();
        if selected.len() > MAX_TASK_WINDOWS_PER_ACTION {
            window.push_notification(self.strings.too_many_windows_hint.clone(), cx);
        }
        for key in selected.into_iter().take(MAX_TASK_WINDOWS_PER_ACTION) {
            opener(key.task_id().to_owned(), window, cx);
        }
    }

    fn on_copy_selected_url(
        &mut self,
        _: &CopySelectedUrl,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let store = self.controller.store();
        let urls: Vec<String> = selected
            .iter()
            .filter_map(|key| store.get(key).map(|row| row.share_url().to_owned()))
            .collect();
        if !urls.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(urls.join("\n")));
        }
    }

    fn on_redownload_selected(
        &mut self,
        _: &RedownloadSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.redownload_commands(cx);
        self.execute_commands(commands, cx);
    }

    /// 选中集合中可重新下载的本地任务（完成 / 失败且非 `torrent-file://` 哨兵）。
    pub(crate) fn redownload_commands(&self, cx: &Context<Self>) -> Vec<DownloadsCommand> {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        selected
            .iter()
            .filter(|key| key.is_local())
            .filter_map(|key| {
                let task_id = key.task_id();
                let row = self.controller.store().get(key)?;
                if !matches!(row.state, TaskState::Completed | TaskState::Failed)
                    || row.url.starts_with("torrent-file://")
                {
                    return None;
                }
                let dto = self.controller.task_dto(task_id)?;
                let request = fluxdown_protocol::CreateTaskRequest {
                    url: if dto.origin_url.is_empty() {
                        dto.url.clone()
                    } else {
                        dto.origin_url.clone()
                    },
                    file_name: dto.file_name.clone(),
                    save_dir: dto.save_dir.clone(),
                    segments: 0,
                    cookies: String::new(),
                    referrer: dto.referrer.clone(),
                    proxy_url: dto.proxy_url.clone(),
                    user_agent: String::new(),
                    queue_id: dto.queue_id.clone(),
                    checksum: dto.checksum.clone(),
                    ignore_tls_errors: dto.ignore_tls_errors,
                    headers: None,
                    torrent_b64: None,
                    method: None,
                    body: None,
                    audio_url: None,
                    start_paused: false,
                    http_user: String::new(),
                    http_password: String::new(),
                    save_site_auth: false,
                };
                Some(DownloadsCommand::Redownload(
                    Box::new(request),
                    task_id.to_owned(),
                ))
            })
            .collect()
    }

    fn on_toggle_boost_selected(
        &mut self,
        _: &ToggleBoostSelected,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.table_state.read(cx).delegate().selected_keys();
        let Some(key) = selected.iter().find(|key| key.is_local()) else {
            return;
        };
        self.execute_commands(
            vec![DownloadsCommand::ToggleBoost {
                task_id: key.task_id().to_owned(),
            }],
            cx,
        );
    }

    fn on_cycle_density(&mut self, _: &CycleDensity, window: &mut Window, cx: &mut Context<Self>) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        self.mutate_prefs(ViewPrefs::cycle_density, cx);
    }

    fn on_cycle_group_by(&mut self, _: &CycleGroupBy, window: &mut Window, cx: &mut Context<Self>) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        self.mutate_prefs(ViewPrefs::cycle_group_by, cx);
    }

    fn on_cycle_sort(&mut self, _: &CycleSort, window: &mut Window, cx: &mut Context<Self>) {
        if !Self::guard_letter_key(window, cx) {
            return;
        }
        self.mutate_prefs(ViewPrefs::cycle_sort, cx);
    }

    fn on_new_download(&mut self, _: &NewDownload, window: &mut Window, cx: &mut Context<Self>) {
        self.open_new_download(window, cx);
    }

    fn on_open_torrent_file(
        &mut self,
        _: &OpenTorrentFile,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let commands: Vec<DownloadsCommand> = paths
                .into_iter()
                .filter(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("torrent"))
                })
                .map(|path| DownloadsCommand::SubmitTorrentFile {
                    path: path.display().to_string(),
                })
                .collect();
            let _ = this.update(cx, |this, cx| this.execute_commands(commands, cx));
        })
        .detach();
    }

    fn on_open_queue_manager(
        &mut self,
        _: &OpenQueueManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(opener) = self.host.open_queue_manager.clone() {
            opener(window, cx);
        }
    }

    fn on_clear_finished(&mut self, _: &ClearFinished, _: &mut Window, cx: &mut Context<Self>) {
        let commands: Vec<DownloadsCommand> = self
            .controller
            .store()
            .local()
            .iter()
            .filter(|row| row.state == TaskState::Completed)
            .map(|row| DownloadsCommand::Delete {
                task_id: row.key.task_id().to_owned(),
                delete_files: false,
            })
            .collect();
        self.execute_commands(commands, cx);
    }

    /// 错误 / 断连提示：紧凑的 destructive 文字条，可关闭。
    fn render_error_strip(&self, error: SharedString, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = fluxdown_ui_theme::active_theme(cx);
        let tokens = theme.tokens();
        let spacing = tokens.spacing;
        let text_size = tokens.typography.xs.size;
        let line_height = tokens.typography.xs.line_height;
        let destructive = tokens.colors.destructive;
        let icon_size = theme.extended().icon.sm;
        let close_label = SharedString::from(self.translator.read(cx).text("close").to_owned());
        let dismiss = self.icon_action(
            "download-error-dismiss",
            close_label,
            Icon::new(FluxIcon::X).size(icon_size),
            false,
            |this, _, cx| {
                this.last_error = None;
                cx.notify();
            },
            cx,
        );
        h_flex()
            .w_full()
            .flex_none()
            .items_center()
            .gap(spacing.sm)
            .pl(spacing.md)
            .pr(spacing.xs)
            .text_size(text_size)
            .line_height(line_height)
            .text_color(destructive)
            .child(div().flex_1().min_w_0().truncate().child(error))
            .child(dismiss)
            .into_any_element()
    }

    pub(crate) fn render_main(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let surface = fluxdown_ui_theme::active_theme(cx).tokens().colors.surface;
        let prefs = self.table_state.read(cx).delegate().prefs().clone();
        let error_strip = self
            .last_error
            .clone()
            .map(|error| self.render_error_strip(error, cx));
        let table = self.render_table(cx);
        let selection_bar = self.render_selection_bar(cx);
        let content = v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .children(error_strip)
            .child(
                // 表格区域：浮动选择条相对它底部居中定位。
                v_flex()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(table)
                    .children(selection_bar),
            );

        let body: gpui::AnyElement = if prefs.detail_open {
            let panel = self.render_detail_panel(cx);
            let on_resize = cx.listener(|this, state: &Entity<ResizableState>, _, cx| {
                if let Some(size) = state.read(cx).sizes().get(1).copied() {
                    let size = f32::from(size);
                    if size > 0. {
                        this.mutate_prefs(|prefs| prefs.detail_size = size, cx);
                    }
                }
            });
            match prefs.detail_placement {
                DetailPlacement::Bottom => v_resizable("downloads-detail-split")
                    .with_state(&self.detail_resizable_state)
                    .on_resize(on_resize)
                    .child(resizable_panel().child(content))
                    .child(
                        resizable_panel()
                            .size(px(prefs.detail_size))
                            .flex_none()
                            .size_range(px(160.)..px(480.))
                            .child(panel),
                    )
                    .into_any_element(),
                DetailPlacement::Right => h_resizable("downloads-detail-split")
                    .with_state(&self.detail_resizable_state)
                    .on_resize(on_resize)
                    .child(resizable_panel().child(content))
                    .child(
                        resizable_panel()
                            .size(px(prefs.detail_size))
                            .flex_none()
                            .size_range(px(220.)..px(560.))
                            .child(panel),
                    )
                    .into_any_element(),
            }
        } else {
            content.into_any_element()
        };

        // 侧栏 | 内容的结构线由 resizable 把手绘制（主题已映射为 hairline），这里不再画边框。
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(surface)
            .child(body)
            .into_any_element()
    }

    fn render_detail_panel(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = fluxdown_ui_theme::active_theme(cx);
        let tokens = theme.tokens().clone();
        let icon_size = theme.extended().icon.md;
        let hairline = theme.extended().colors.hairline;
        let tertiary = theme.extended().colors.text_tertiary;
        let placement = self
            .table_state
            .read(cx)
            .delegate()
            .prefs()
            .detail_placement;
        let translator = self.translator.read(cx);
        let title = translator.text("detail").to_owned();
        let hint = translator.text("selectTaskHint").to_owned();
        // 切换按钮指向「切换后」的位置。
        let (placement_icon, placement_label) = if placement == DetailPlacement::Bottom {
            (
                FluxIcon::PanelRight,
                translator.text("viewDetailRight").to_owned(),
            )
        } else {
            (
                FluxIcon::PanelBottom,
                translator.text("viewDetailBottom").to_owned(),
            )
        };
        let close_label = SharedString::from(translator.text("close").to_owned());
        let pop_out_label = self.strings.open_in_window.clone();
        let toggle_position = self.icon_action(
            "detail-panel-toggle-position",
            SharedString::from(placement_label),
            Icon::new(placement_icon).size(icon_size),
            false,
            |this, _, cx| this.on_toggle_detail_placement(cx),
            cx,
        );
        let pop_out = self.icon_action(
            "detail-panel-pop-out",
            pop_out_label,
            Icon::new(FluxIcon::AppWindow).size(icon_size),
            false,
            |this, window, cx| this.on_pop_out_detail(window, cx),
            cx,
        );
        let close = self.icon_action(
            "detail-panel-close",
            close_label,
            Icon::new(FluxIcon::X).size(icon_size),
            false,
            |this, _, cx| this.on_close_detail_panel(cx),
            cx,
        );

        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(tokens.colors.surface)
            .child(
                h_flex()
                    // 面板头：28 的图标按钮上下各留 spacing.xxs。
                    .h(TOOLBAR_BUTTON_SIZE + tokens.spacing.xs)
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .pl(tokens.spacing.md)
                    .pr(tokens.spacing.xxs)
                    .border_b_1()
                    .border_color(hairline)
                    .child(
                        div()
                            .text_size(tokens.typography.sm.size)
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        h_flex()
                            .gap(tokens.spacing.xxs)
                            .child(toggle_position)
                            .child(pop_out)
                            .child(close),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when_some(self.detail.clone(), |this, detail| this.child(detail))
                    .when(self.detail.is_none(), |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .items_center()
                                .justify_center()
                                .gap(tokens.spacing.sm)
                                .child(Icon::new(FluxIcon::File).size(px(32.)).text_color(tertiary))
                                .child(
                                    div()
                                        .text_size(tokens.typography.xs.size)
                                        .line_height(tokens.typography.xs.line_height)
                                        .text_color(tokens.colors.muted_foreground)
                                        .child(hint),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for DownloadView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sizes = self.resizable_state.read(cx).sizes().to_vec();
        let main_panel_measured = sizes.get(1).is_some_and(|size| *size > px(1.));
        if !self.resizable_state_initialized && main_panel_measured {
            self.resizable_state
                .update(cx, |state, cx| state.reset_panel(1, cx));
            self.resizable_state_initialized = true;
        }
        // 语言切换后同步搜索框 placeholder（InputState 只在构造时取一次）。
        if self.search_placeholder != self.strings.search_tasks_placeholder {
            self.search_placeholder = self.strings.search_tasks_placeholder.clone();
            let placeholder = self.search_placeholder.clone();
            self.search_input.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx);
            });
        }
        let sidebar_width = self.table_state.read(cx).delegate().prefs().sidebar_width;
        v_flex()
            .key_context(KEY_CONTEXT)
            .size_full()
            .min_w_0()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_select_all))
            .on_action(cx.listener(Self::on_clear_selection))
            .on_action(cx.listener(Self::on_pause_selected))
            .on_action(cx.listener(Self::on_resume_selected))
            .on_action(cx.listener(Self::on_toggle_pause_selected))
            .on_action(cx.listener(Self::on_delete_selected))
            .on_action(cx.listener(Self::on_delete_selected_with_files))
            .on_action(cx.listener(Self::on_pause_all))
            .on_action(cx.listener(Self::on_resume_all))
            .on_action(cx.listener(Self::on_open_selected))
            .on_action(cx.listener(Self::on_reveal_selected))
            .on_action(cx.listener(Self::on_open_selected_in_window))
            .on_action(cx.listener(Self::on_copy_selected_url))
            .on_action(cx.listener(Self::on_redownload_selected))
            .on_action(cx.listener(Self::on_toggle_boost_selected))
            .on_action(cx.listener(Self::on_cycle_density))
            .on_action(cx.listener(Self::on_cycle_group_by))
            .on_action(cx.listener(Self::on_cycle_sort))
            .on_action(cx.listener(Self::on_new_download))
            .on_action(cx.listener(Self::on_open_torrent_file))
            .on_action(cx.listener(Self::on_open_queue_manager))
            .on_action(cx.listener(Self::on_clear_finished))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_rename_selected))
            .on_action(cx.listener(Self::on_toggle_detail_panel))
            .on_drop(cx.listener(Self::on_paths_dropped))
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                let tokens = fluxdown_ui_theme::active_theme(cx).tokens();
                style.bg(tokens.colors.accent.opacity(0.2))
            })
            .child(
                div().flex_1().min_h_0().min_w_0().child(
                    h_resizable("downloads-content")
                        .with_state(&self.resizable_state)
                        .on_resize(cx.listener(|this, state: &Entity<ResizableState>, _, cx| {
                            state.update(cx, |state, cx| state.reset_panel(1, cx));
                            if let Some(width) = state.read(cx).sizes().first().copied() {
                                let width = f32::from(width);
                                if width > 0. {
                                    this.mutate_prefs(|prefs| prefs.sidebar_width = width, cx);
                                }
                            }
                        }))
                        .child(
                            resizable_panel()
                                .size(px(sidebar_width))
                                .flex_none()
                                .size_range(px(176.)..px(300.))
                                .child(self.render_sidebar(window, cx)),
                        )
                        .child(resizable_panel().child(self.render_main(cx))),
                ),
            )
            .child(self.render_status_bar(cx))
    }
}

/// 拖入 `.txt`/`.url`/`.list` 文件（≤1MB）；超限或读取失败返回 `None`。
fn read_drop_text_file(path: &std::path::Path) -> Option<String> {
    const MAX_BYTES: u64 = 1_048_576;
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// 纯函数：从拖入文本按行提取受支持协议的下载链接（`http(s)://`、
/// `ftp(s)://`、`magnet:`、`ed2k://`）。
pub(crate) fn parse_drop_urls(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| is_supported_drop_url(line))
        .map(str::to_owned)
        .collect()
}

fn is_supported_drop_url(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("magnet:")
        || lower.starts_with("ed2k://")
}

#[cfg(test)]
mod drop_tests {
    use super::parse_drop_urls;

    #[test]
    fn extracts_only_supported_url_lines() {
        let text = "  https://example.com/a.zip  \n\
             not a url\n\
             \n\
             magnet:?xt=urn:btih:abc\n\
             ftp://files.example.com/b.iso\n\
             ed2k://|file|c.bin|123|abcdef|/\n\
             javascript:alert(1)\n";
        assert_eq!(
            parse_drop_urls(text),
            vec![
                "https://example.com/a.zip".to_owned(),
                "magnet:?xt=urn:btih:abc".to_owned(),
                "ftp://files.example.com/b.iso".to_owned(),
                "ed2k://|file|c.bin|123|abcdef|/".to_owned(),
            ]
        );
    }

    #[test]
    fn empty_or_unsupported_text_yields_no_urls() {
        assert!(parse_drop_urls("").is_empty());
        assert!(parse_drop_urls("just some notes\nno links here").is_empty());
    }

    #[test]
    fn is_case_insensitive_for_scheme() {
        assert_eq!(
            parse_drop_urls("HTTPS://EXAMPLE.COM/file"),
            vec!["HTTPS://EXAMPLE.COM/file".to_owned()]
        );
    }
}
