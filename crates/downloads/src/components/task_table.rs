use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use fluxdown_ui_components::{CheckState, FluxIcon, check_mark, tabular_numbers};
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Edges, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement, Pixels, Render,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled, WeakEntity, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ElementExt as _, Icon, IconName, Sizable as _, Size, h_flex,
    menu::{PopupMenu, PopupMenuItem},
    scroll::Scrollbar,
    spinner::Spinner,
    table::{Column, DataTable, TableDelegate, TableState},
    tooltip::Tooltip,
    v_flex,
};

use crate::{
    controller::DownloadsCommand,
    model::{
        CategoryIndex, DownloadFilter, DownloadTaskView, RowId, RowKey, SidebarSelection, TaskKind,
        TaskProtocol, TaskSource, TaskState, TaskStore, format_bytes,
        view_prefs::{DateBucket, SortDir, ViewGroupBy, ViewPrefs, ViewSortKey, state_group_key},
    },
    pages::downloads::DownloadView,
    strings::DownloadStrings,
};

/// 固定左侧选择列宽（含表格左侧留白）：平时显示文件类型图标，行悬停 / 已选中 /
/// 存在任意选中时换成复选框。
const SELECTION_COLUMN_WIDTH: f32 = 36.;
/// 表头拖拽调宽的上限（下限按列取 [`DownloadColumnKind::min_width`]）。
const MAX_COLUMN_WIDTH: f32 = 480.;
/// 文件名列拖宽上限（长文件名需要比其他列更宽的空间）。
const FILE_NAME_MAX_WIDTH: f32 = 1600.;
/// 表头高。DataTable 的 `Size` 同时决定表头行高；任务行高由 `render_tr` 按密度覆盖。
const TABLE_HEADER_HEIGHT: f32 = 28.;
/// 表格右侧留白（`render_last_empty_col`），与选择列内含的左侧留白对称（≈ spacing.sm）。
const TABLE_TRAILING_GUTTER: f32 = 8.;
/// 表头列分隔线高（常驻的拖宽提示，悬停时由拖宽柄全高高亮接管）。
const COLUMN_DIVIDER_HEIGHT: f32 = 14.;
/// 数据列左右内边距：显式设给每列，进度条宽度据此推算。
const CELL_PADDING_X: f32 = 8.;
/// 进度列百分比区宽：容纳 12px 等宽数字「99%」。
const PROGRESS_LABEL_WIDTH: f32 = 32.;
/// 进度条与百分比的间距。
const PROGRESS_GAP: f32 = 8.;
/// 进度条高度（详情窗口复用，保证与主表格一致）。
pub(crate) const PROGRESS_BAR_HEIGHT: f32 = 4.;
/// 暂停态进度条：muted_foreground 的 40%。
const PAUSED_BAR_ALPHA: f32 = 0.4;
/// 进度轨道：muted_foreground 的淡化派生；半透明，叠在行悬停底色上仍可见。
const PROGRESS_TRACK_ALPHA: f32 = 0.16;
/// 行悬停操作按钮边长。
const ROW_ACTION_SIZE: f32 = 24.;
/// 选中底色左右内缩（inset 样式）。
const SELECTED_INSET_X: f32 = 4.;
/// 分组头内容高；行槽位（与任务行等高，uniform_list 要求）多出的部分作上方留白。
const GROUP_HEADER_HEIGHT: f32 = 28.;
/// 任务行 group 名：选择列复选框与行操作按钮随行悬停显隐。
const ROW_GROUP: &str = "download-task-row";
/// 表头全选格 group 名：悬停该格时显示全选框。
const SELECT_ALL_GROUP: &str = "download-select-all";
/// 超过一天的剩余时间估算不可信，不显示。
const MAX_ETA_SECS: u64 = 86_400;
/// 表头按下与抬起的位移超过它视为拖动列（不触发排序）。
const HEADER_CLICK_SLOP: f32 = 4.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DownloadColumnKind {
    FileName,
    Progress,
    Size,
    Speed,
    Eta,
    Status,
    Created,
    Protocol,
    Source,
    Queue,
}

impl DownloadColumnKind {
    /// 默认列顺序（`columns` 偏好为空或缺列时按此补齐）。
    pub(crate) const ALL: [Self; 10] = [
        Self::FileName,
        Self::Size,
        Self::Progress,
        Self::Status,
        Self::Created,
        Self::Speed,
        Self::Eta,
        Self::Protocol,
        Self::Source,
        Self::Queue,
    ];

    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::FileName => "file_name",
            Self::Progress => "progress",
            Self::Size => "size",
            Self::Speed => "speed",
            Self::Eta => "eta",
            Self::Status => "status",
            Self::Created => "created",
            Self::Protocol => "protocol",
            Self::Source => "source",
            Self::Queue => "queue",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    /// 速度 / 剩余时间已并入「状态」列的活动文案，默认隐藏（仍可在列设置打开）。
    fn default_visible(self) -> bool {
        matches!(
            self,
            Self::FileName | Self::Size | Self::Progress | Self::Status | Self::Created
        )
    }

    /// 文件名列的值只在容器宽度未知时使用；已知后由 [`DownloadTableDelegate`]
    /// 按容器宽回算（吸收剩余宽度），用户拖过后改用 `ViewPrefs::file_name_width`。
    fn default_width(self) -> f32 {
        match self {
            Self::FileName => 240.,
            Self::Progress => 150.,
            Self::Size => 84.,
            Self::Speed => 90.,
            Self::Eta => 84.,
            Self::Status => 140.,
            Self::Created => 150.,
            Self::Protocol => 64.,
            Self::Source => 148.,
            Self::Queue => 88.,
        }
    }

    fn min_width(self) -> f32 {
        match self {
            Self::FileName => 160.,
            Self::Progress => 110.,
            Self::Size => 64.,
            Self::Speed => 72.,
            Self::Eta => 64.,
            Self::Status => 84.,
            Self::Created => 140.,
            Self::Protocol => 56.,
            Self::Source => 96.,
            Self::Queue => 72.,
        }
    }

    /// 数字列：单元格与表头右对齐。
    fn is_numeric(self) -> bool {
        matches!(self, Self::Size | Self::Speed | Self::Eta | Self::Created)
    }

    /// 点击表头切换到的排序键；「状态」列回到智能排序（状态优先级）。
    fn sort_key(self) -> Option<ViewSortKey> {
        match self {
            Self::FileName => Some(ViewSortKey::Name),
            Self::Progress => Some(ViewSortKey::Progress),
            Self::Size => Some(ViewSortKey::Size),
            Self::Speed => Some(ViewSortKey::Speed),
            Self::Created => Some(ViewSortKey::Created),
            Self::Status => Some(ViewSortKey::Smart),
            _ => None,
        }
    }

    /// 首次切到该列排序时的方向：名称 A→Z，数值类从大到新。
    fn default_sort_dir(self) -> SortDir {
        match self {
            Self::FileName => SortDir::Asc,
            _ => SortDir::Desc,
        }
    }

    pub(crate) fn label(self, strings: &DownloadStrings) -> SharedString {
        match self {
            Self::FileName => strings.col_file_name.clone(),
            Self::Progress => strings.col_progress.clone(),
            Self::Size => strings.col_size.clone(),
            Self::Speed => strings.col_speed.clone(),
            Self::Eta => strings.col_eta.clone(),
            Self::Status => strings.col_status.clone(),
            Self::Created => strings.col_created.clone(),
            Self::Protocol => strings.col_protocol.clone(),
            Self::Source => strings.col_source.clone(),
            Self::Queue => strings.col_queue.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DownloadColumn {
    pub(crate) kind: DownloadColumnKind,
    pub(crate) width: f32,
    /// 用户开关（右上角列设置）；是否显示只由它决定，超宽走横向滚动。
    pub(crate) visible: bool,
}

impl DownloadColumn {
    fn new(kind: DownloadColumnKind) -> Self {
        Self {
            kind,
            width: kind.default_width(),
            visible: kind.default_visible(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DraggedColumnMenuItem {
    pub(crate) kind: DownloadColumnKind,
    pub(crate) label: SharedString,
}

impl Render for DraggedColumnMenuItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        h_flex()
            .px(tokens.spacing.md)
            .py(tokens.spacing.xs)
            .gap(tokens.spacing.sm)
            .rounded(tokens.radius.sm)
            .border_1()
            .border_color(tokens.colors.border)
            .bg(tokens.colors.surface)
            .shadow_sm()
            .text_size(tokens.typography.sm.size)
            .line_height(tokens.typography.sm.line_height)
            .text_color(tokens.colors.surface_foreground)
            .child(Icon::new(FluxIcon::GripVertical).size(theme.extended().icon.md))
            .child(self.label.clone())
    }
}

/// 表格可见行：任务行或分组头。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum VisibleRow {
    Task(RowId),
    GroupHeader {
        key: String,
        label: SharedString,
        count: usize,
        collapsed: bool,
    },
}

/// 分组计算结果。
struct GroupBucket {
    key: String,
    label: SharedString,
    order: i64,
}

/// 侧栏选中项到表格筛选的投影。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TableFilter {
    Download(DownloadFilter),
    Queue(String),
    Device(String),
    Group(String),
}

impl From<&SidebarSelection> for TableFilter {
    fn from(selection: &SidebarSelection) -> Self {
        match selection {
            SidebarSelection::Download(filter) => Self::Download(filter.clone()),
            SidebarSelection::Queue(id) => Self::Queue(id.clone()),
            SidebarSelection::Device(id) => Self::Device(id.clone()),
        }
    }
}

/// 右键菜单单任务事实：足以判定菜单项可见性的最小状态快照，与
/// [`DownloadTaskView`] 解耦以便纯函数测试。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TaskMenuFacts {
    pub(crate) is_local: bool,
    pub(crate) state: TaskState,
    pub(crate) boosted: bool,
    /// `url` 是否为 BT 哨兵值 `torrent-file://…`（不可重新下载）。
    pub(crate) is_torrent_sentinel: bool,
    /// 失败态且错误消息带插件重试前缀（与
    /// `lib/src/widgets/task_list_item.dart` 的 `_pluginErrorPrefix` 同源）。
    pub(crate) is_plugin_retry_error: bool,
}

/// 引擎/daemon 插件系统失败任务的错误消息前缀。
pub(crate) const PLUGIN_ERROR_PREFIX: &str = "[插件]";

/// 任务右键菜单条目；数组顺序即渲染顺序。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuEntry {
    Resume,
    Pause,
    IgnorePluginRetry,
    Boost,
    CancelBoost,
    OpenFile,
    OpenFolder,
    Rename,
    Redownload,
    CopyUrl,
    MoveToQueue,
    Separator,
    Delete,
    DeleteWithFiles,
    OpenInWindow,
}

/// 纯函数：按选中任务集合的事实计算应显示的右键菜单项。
///
/// 多选时每一项都要求对**全部**选中任务成立（交集语义），resume/pause 例外
/// ——只要选中集合中**存在**一个可恢复/可暂停的任务就显示（与工具栏按钮行为
/// 一致：工具栏无条件对每个选中项下发命令，无效状态的任务被服务端忽略）。
pub(crate) fn context_menu_items(selection: &[TaskMenuFacts]) -> Vec<MenuEntry> {
    if selection.is_empty() {
        return Vec::new();
    }
    let mut items = Vec::new();
    if selection
        .iter()
        .any(|task| !matches!(task.state, TaskState::Completed | TaskState::Downloading))
    {
        items.push(MenuEntry::Resume);
    }
    if selection
        .iter()
        .any(|task| matches!(task.state, TaskState::Downloading | TaskState::Pending))
    {
        items.push(MenuEntry::Pause);
    }
    if let [only] = selection {
        if only.state == TaskState::Failed && only.is_plugin_retry_error {
            items.push(MenuEntry::IgnorePluginRetry);
        }
        if only.is_local {
            items.push(if only.boosted {
                MenuEntry::CancelBoost
            } else {
                MenuEntry::Boost
            });
        }
    }
    if selection
        .iter()
        .all(|task| task.is_local && task.state == TaskState::Completed)
    {
        items.push(MenuEntry::OpenFile);
    }
    if selection.iter().all(|task| task.is_local) {
        items.push(MenuEntry::OpenFolder);
    }
    if let [only] = selection
        && only.is_local
    {
        items.push(MenuEntry::Rename);
    }
    if selection.iter().all(|task| {
        task.is_local
            && matches!(task.state, TaskState::Completed | TaskState::Failed)
            && !task.is_torrent_sentinel
    }) {
        items.push(MenuEntry::Redownload);
    }
    items.push(MenuEntry::CopyUrl);
    if selection.iter().all(|task| task.is_local) {
        items.push(MenuEntry::MoveToQueue);
    }
    items.push(MenuEntry::Separator);
    items.push(MenuEntry::Delete);
    items.push(MenuEntry::DeleteWithFiles);
    if selection.iter().any(|task| task.is_local) {
        items.push(MenuEntry::OpenInWindow);
    }
    items
}

/// 构造一个分组头右键菜单项：点击时把 `group_id` 回调进 [`DownloadView`] 的方法
/// （无需 `Window`，用于批量命令类操作）。
fn group_menu_item(
    label: SharedString,
    icon: FluxIcon,
    host: &WeakEntity<DownloadView>,
    group_id: &str,
    action: fn(&mut DownloadView, String, &mut Context<DownloadView>),
) -> PopupMenuItem {
    let host = host.clone();
    let group_id = group_id.to_owned();
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, _, cx| {
            let group_id = group_id.clone();
            let _ = host.update(cx, |view, cx| action(view, group_id, cx));
        })
}

/// 同 [`group_menu_item`]，但回调需要 `Window`（打开确认对话框 / 独立窗口）。
fn group_menu_item_windowed(
    label: SharedString,
    icon: FluxIcon,
    host: &WeakEntity<DownloadView>,
    group_id: &str,
    action: fn(&mut DownloadView, String, &mut Window, &mut Context<DownloadView>),
) -> PopupMenuItem {
    let host = host.clone();
    let group_id = group_id.to_owned();
    PopupMenuItem::new(label)
        .icon(icon)
        .on_click(move |_, window, cx| {
            let group_id = group_id.clone();
            let _ = host.update(cx, |view, cx| action(view, group_id, window, cx));
        })
}

pub(crate) struct DownloadTableDelegate {
    pub(crate) strings: DownloadStrings,
    pub(crate) columns: Vec<DownloadColumn>,
    store: Rc<TaskStore>,
    categories: Rc<CategoryIndex>,
    visible: Vec<VisibleRow>,
    seen_generation: u64,
    view_dirty: bool,
    selected_tasks: HashSet<RowKey>,
    selection_anchor: Option<RowKey>,
    filter: TableFilter,
    query: String,
    prefs: ViewPrefs,
    /// queue_id → 名称（列 / 分组标签）。
    queue_names: Vec<(String, String)>,
    /// group_id → 名称。
    group_names: HashMap<String, String>,
    /// 远程任务来源设备匹配用：设备 id / 指纹 → 别名集合。
    device_aliases: HashMap<String, Vec<String>>,
    /// 表格容器宽（`render_download_table` 在 prepaint 测得、帧外写入）；0 = 尚未测得。
    viewport_width: f32,
    /// 最近一次交给表格的文件名列宽（`column()` 写入）；与期望值不同才需 refresh。
    applied_file_width: Cell<f32>,
    /// 列配置（宽度 / 可见性 / 顺序 / 排序指示）需要重建表头 `col_groups`。
    /// 纯行数据变化不置位：`TableState::refresh` 会用代理宽度覆盖表格内部
    /// 实时宽度，进度节拍若每次都刷新会把用户正在拖拽的列宽弹回去。
    columns_dirty: bool,
    /// 上级页面弱引用：右键菜单需要参数的命令（移动到队列 / 忽略插件重试 /
    /// 任务组操作）经它回调 `DownloadView` 的方法执行；`None` 时这些项不渲染。
    host: Option<WeakEntity<DownloadView>>,
    /// 右键菜单动作分派目标；`None` 时菜单项不带 action_context（仍可点击，
    /// 只是动作从菜单自身的焦点路径冒泡）。
    action_context: Option<FocusHandle>,
}

impl DownloadTableDelegate {
    pub(crate) fn new(strings: DownloadStrings, store: Rc<TaskStore>) -> Self {
        Self {
            strings,
            columns: DownloadColumnKind::ALL
                .into_iter()
                .map(DownloadColumn::new)
                .collect(),
            store,
            categories: Rc::new(CategoryIndex::default()),
            visible: Vec::new(),
            seen_generation: u64::MAX,
            view_dirty: true,
            selected_tasks: HashSet::new(),
            selection_anchor: None,
            filter: TableFilter::Download(DownloadFilter::ALL),
            query: String::new(),
            prefs: ViewPrefs::default(),
            queue_names: Vec::new(),
            group_names: HashMap::new(),
            device_aliases: HashMap::new(),
            viewport_width: 0.,
            applied_file_width: Cell::new(0.),
            columns_dirty: false,
            host: None,
            action_context: None,
        }
    }

    pub(crate) fn set_strings(&mut self, strings: DownloadStrings) {
        self.strings = strings;
        self.view_dirty = true;
    }

    /// 注入宿主弱引用；右键菜单中需要参数的命令经它回调
    /// [`DownloadView`] 的方法执行。
    pub(crate) fn set_host(&mut self, host: WeakEntity<DownloadView>) {
        self.host = Some(host);
    }

    /// 注入右键菜单动作分派目标（根元素的 focus handle）。
    pub(crate) fn set_action_context(&mut self, handle: FocusHandle) {
        self.action_context = Some(handle);
    }

    pub(crate) fn set_categories(&mut self, categories: Rc<CategoryIndex>) {
        if !Rc::ptr_eq(&self.categories, &categories) {
            self.categories = categories;
            self.view_dirty = true;
        }
    }

    pub(crate) fn set_queue_names(&mut self, queues: Vec<(String, String)>) {
        if self.queue_names != queues {
            self.queue_names = queues;
            self.view_dirty = true;
        }
    }

    pub(crate) fn set_group_names(&mut self, groups: HashMap<String, String>) {
        if self.group_names != groups {
            self.group_names = groups;
            self.view_dirty = true;
        }
    }

    pub(crate) fn set_device_aliases(&mut self, aliases: HashMap<String, Vec<String>>) {
        if self.device_aliases != aliases {
            self.device_aliases = aliases;
            self.view_dirty = true;
        }
    }

    pub(crate) fn set_filter(&mut self, filter: TableFilter) {
        if self.filter != filter {
            self.filter = filter;
            self.view_dirty = true;
        }
    }

    pub(crate) fn set_query(&mut self, query: &str) {
        let query = query.trim().to_lowercase();
        if self.query != query {
            self.query = query;
            self.view_dirty = true;
        }
    }

    pub(crate) fn prefs(&self) -> &ViewPrefs {
        &self.prefs
    }

    /// 替换视图偏好（密度 / 分组 / 排序 / 列 / 折叠）。
    pub(crate) fn set_prefs(&mut self, prefs: ViewPrefs) {
        if self.prefs == prefs {
            return;
        }
        self.apply_column_prefs(&prefs.columns);
        self.columns_dirty = true;
        self.prefs = prefs;
        self.view_dirty = true;
    }

    pub(crate) fn prefs_mut(&mut self) -> &mut ViewPrefs {
        self.view_dirty = true;
        &mut self.prefs
    }

    /// 列配置是否需要 `TableState::refresh`（读取后清零）。
    pub(crate) fn take_columns_dirty(&mut self) -> bool {
        std::mem::take(&mut self.columns_dirty)
    }

    /// 表头拖拽调宽结束后，把表格内部列宽（含首列勾选列）写回代理列配置，
    /// 作为后续刷新与持久化的事实源。返回是否有变化。
    ///
    /// 文件名列默认由容器宽回算（吸收剩余宽度）；表格给出的宽度与最近一次应用值
    /// 不同即说明用户拖过它，此后固定为 `prefs.file_name_width`，不再随容器伸缩。
    /// 其他列变宽后，下一帧 `render_download_table` 的 prepaint 发现未固定的
    /// 文件名列宽过期，帧外 refresh 补足。拖拽进行中代理宽度不变，因此不会触发
    /// refresh 把拖拽中的列宽弹回。
    pub(crate) fn sync_column_widths(&mut self, widths: &[Pixels]) -> bool {
        let applied_file_width = self.applied_file_width.get();
        let mut file_name_width = None;
        let mut changed = false;
        let shown = self.columns.iter_mut().filter(|column| column.visible);
        for (column, width) in shown.zip(widths.iter().skip(1)) {
            let width = f32::from(*width);
            if column.kind == DownloadColumnKind::FileName {
                if (width - applied_file_width).abs() >= 0.5 {
                    file_name_width = Some(width.clamp(
                        DownloadColumnKind::FileName.min_width(),
                        FILE_NAME_MAX_WIDTH,
                    ));
                }
                continue;
            }
            let width = width.clamp(column.kind.min_width(), MAX_COLUMN_WIDTH);
            if (column.width - width).abs() > f32::EPSILON {
                column.width = width;
                changed = true;
            }
        }
        if let Some(width) = file_name_width {
            self.prefs.file_name_width = Some(width);
            self.applied_file_width.set(width);
            changed = true;
        }
        changed
    }

    /// 容器宽为 `viewport` 时文件名列应得的宽度。用户拖过则取固定宽；否则
    /// `max(min_width, 容器宽 − 选择列 − 其他可见列 − 右侧留白 − 竖滚动条预留)`，
    /// 容器宽未知（≤ 0）时退回列配置宽度。
    fn file_name_width_for(&self, viewport: f32) -> f32 {
        let min_width = DownloadColumnKind::FileName.min_width();
        if let Some(width) = self.prefs.file_name_width {
            return width.clamp(min_width, FILE_NAME_MAX_WIDTH);
        }
        if viewport <= 0. {
            return self
                .columns
                .iter()
                .find(|column| column.kind == DownloadColumnKind::FileName)
                .map_or(min_width, |column| column.width.max(min_width));
        }
        let others: f32 = self
            .columns
            .iter()
            .filter(|column| column.visible && column.kind != DownloadColumnKind::FileName)
            .map(|column| column.width)
            .sum();
        let reserved =
            SELECTION_COLUMN_WIDTH + others + TABLE_TRAILING_GUTTER + f32::from(Scrollbar::width());
        (viewport - reserved).max(min_width)
    }

    fn file_name_visible(&self) -> bool {
        self.columns
            .iter()
            .any(|column| column.visible && column.kind == DownloadColumnKind::FileName)
    }

    /// prepaint 判定：容器宽变化，或文件名列宽与期望不符（例如刚同步了其他列宽）。
    fn viewport_needs_sync(&self, viewport: f32) -> bool {
        (self.viewport_width - viewport).abs() >= 0.5
            || (self.file_name_visible()
                && (self.file_name_width_for(viewport) - self.applied_file_width.get()).abs()
                    >= 0.5)
    }

    /// 按当前容器宽计算文件名列宽并记为「已应用」（`column()` 构建表格列时调用）。
    fn apply_file_name_width(&self) -> f32 {
        let width = self.file_name_width_for(self.viewport_width);
        self.applied_file_width.set(width);
        width
    }

    /// 写入容器宽；返回表格是否需要 `refresh` 以应用新的文件名列宽。
    pub(crate) fn set_viewport_width(&mut self, viewport: f32) -> bool {
        self.viewport_width = viewport;
        self.file_name_visible()
            && (self.file_name_width_for(viewport) - self.applied_file_width.get()).abs() >= 0.5
    }

    /// 表头点击排序：已是当前排序键则翻转方向（智能排序无方向，保持不变），
    /// 否则切到该键并取列的默认方向。返回偏好是否改变。
    fn toggle_sort(&mut self, kind: DownloadColumnKind) -> bool {
        let Some(key) = kind.sort_key() else {
            return false;
        };
        if self.prefs.sort_key == key {
            if key == ViewSortKey::Smart {
                return false;
            }
            self.prefs.sort_dir = match self.prefs.sort_dir {
                SortDir::Asc => SortDir::Desc,
                SortDir::Desc => SortDir::Asc,
            };
        } else {
            self.prefs.sort_key = key;
            self.prefs.sort_dir = kind.default_sort_dir();
        }
        self.view_dirty = true;
        true
    }

    fn apply_column_prefs(&mut self, prefs: &[crate::model::view_prefs::ColumnPref]) {
        if prefs.is_empty() {
            return;
        }
        let mut columns: Vec<DownloadColumn> = prefs
            .iter()
            .filter_map(|pref| {
                let kind = DownloadColumnKind::from_key(&pref.key)?;
                let mut column = DownloadColumn::new(kind);
                column.visible = pref.visible;
                if pref.width >= kind.min_width() {
                    column.width = pref.width;
                }
                Some(column)
            })
            .collect();
        for kind in DownloadColumnKind::ALL {
            if !columns.iter().any(|column| column.kind == kind) {
                columns.push(DownloadColumn::new(kind));
            }
        }
        if !columns.iter().any(|column| column.visible) {
            columns[0].visible = true;
        }
        self.columns = columns;
    }

    /// 当前列配置 → 偏好条目。
    pub(crate) fn column_prefs(&self) -> Vec<crate::model::view_prefs::ColumnPref> {
        self.columns
            .iter()
            .map(|column| crate::model::view_prefs::ColumnPref {
                key: column.kind.key().to_owned(),
                visible: column.visible,
                width: column.width,
            })
            .collect()
    }

    /// 存储变化或视图参数变化时重算可见行。返回是否重算。
    pub(crate) fn refresh_view(&mut self) -> bool {
        let generation = self.store.generation();
        if !self.view_dirty && generation == self.seen_generation {
            return false;
        }
        self.seen_generation = generation;
        self.view_dirty = false;
        self.visible = self.compute_visible();
        // 选中集只保留当前筛选 + 搜索下仍在视图内的任务（已删除、被侧栏切换 / 搜索 /
        // 状态变化筛掉的一律移出），选择条计数与快捷键批量动作都只作用于看得见的任务。
        // 折叠分组内的任务仍属于当前视图，不因折叠而取消选中。
        let mut selected = std::mem::take(&mut self.selected_tasks);
        selected.retain(|key| {
            self.store
                .get(key)
                .is_some_and(|task| self.matches_filter(&task) && self.matches_query(&task))
        });
        self.selected_tasks = selected;
        self.selection_anchor = self
            .selection_anchor
            .take()
            .filter(|key| self.selected_tasks.contains(key));
        true
    }

    fn matches_query(&self, task: &DownloadTaskView) -> bool {
        if self.query.is_empty() {
            return true;
        }
        task.name.to_lowercase().contains(&self.query)
            || task.url.to_lowercase().contains(&self.query)
            || task.site.to_lowercase().contains(&self.query)
    }

    fn matches_filter(&self, task: &DownloadTaskView) -> bool {
        match &self.filter {
            TableFilter::Download(filter) => filter.matches(task, &self.categories),
            TableFilter::Queue(queue_id) => {
                task.source == TaskSource::Local && task.queue_id == *queue_id
            }
            TableFilter::Device(device) => {
                if device == SidebarSelection::LOCAL_DEVICE {
                    task.source == TaskSource::Local
                } else {
                    task.source == TaskSource::Remote
                        && (task.from_device == *device
                            || self
                                .device_aliases
                                .get(device)
                                .is_some_and(|aliases| aliases.contains(&task.from_device)))
                }
            }
            TableFilter::Group(group_id) => task.group_id == *group_id,
        }
    }

    fn compute_visible(&self) -> Vec<VisibleRow> {
        let local = self.store.local();
        let remote = self.store.remote();
        let mut rows: Vec<(RowId, &DownloadTaskView)> = local
            .iter()
            .enumerate()
            .map(|(ix, task)| (RowId::Local(ix), task))
            .chain(
                remote
                    .iter()
                    .enumerate()
                    .map(|(ix, task)| (RowId::Remote(ix), task)),
            )
            .filter(|(_, task)| self.matches_filter(task) && self.matches_query(task))
            .collect();
        rows.sort_by(|(_, left), (_, right)| self.prefs.compare(left, right));

        if self.prefs.group_by == ViewGroupBy::None {
            return rows
                .into_iter()
                .map(|(id, _)| VisibleRow::Task(id))
                .collect();
        }

        let mut buckets: Vec<(GroupBucket, Vec<RowId>)> = Vec::new();
        for (id, task) in rows {
            let bucket = self.group_bucket(task);
            match buckets
                .iter_mut()
                .find(|(existing, _)| existing.key == bucket.key)
            {
                Some((_, ids)) => ids.push(id),
                None => buckets.push((bucket, vec![id])),
            }
        }
        buckets.sort_by(|(left, _), (right, _)| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.label.cmp(&right.label))
        });
        let mut visible = Vec::new();
        for (bucket, ids) in buckets {
            let collapsed = self.prefs.is_group_collapsed(&bucket.key);
            visible.push(VisibleRow::GroupHeader {
                key: bucket.key,
                label: bucket.label,
                count: ids.len(),
                collapsed,
            });
            if !collapsed {
                visible.extend(ids.into_iter().map(VisibleRow::Task));
            }
        }
        visible
    }

    fn group_bucket(&self, task: &DownloadTaskView) -> GroupBucket {
        let strings = &self.strings;
        match self.prefs.group_by {
            ViewGroupBy::None => GroupBucket {
                key: String::new(),
                label: SharedString::default(),
                order: 0,
            },
            ViewGroupBy::Status => GroupBucket {
                key: format!("status:{}", state_group_key(task.state)),
                label: strings.state_label(task.state),
                order: i64::from(task.state.smart_rank()),
            },
            ViewGroupBy::Date => {
                let bucket = DateBucket::of(task.created_at_secs);
                GroupBucket {
                    key: format!("date:{}", bucket.key()),
                    label: strings.date_bucket_label(bucket),
                    order: bucket as i64,
                }
            }
            ViewGroupBy::Type => {
                let id = self.categories.category_of(task);
                let (label, order) = self
                    .categories
                    .rules()
                    .iter()
                    .enumerate()
                    .find(|(_, rule)| rule.dto.id == id)
                    .map_or_else(
                        || (strings.category_other.clone(), i64::MAX),
                        |(ix, rule)| (strings.category_label(&rule.dto), ix as i64),
                    );
                GroupBucket {
                    key: format!("type:{id}"),
                    label,
                    order,
                }
            }
            ViewGroupBy::Queue => {
                if task.source == TaskSource::Remote {
                    return GroupBucket {
                        key: "queue:remote".to_owned(),
                        label: strings.remote_tasks.clone(),
                        order: i64::MAX,
                    };
                }
                let (label, order) = self
                    .queue_names
                    .iter()
                    .enumerate()
                    .find(|(_, (id, _))| *id == task.queue_id)
                    .map_or_else(
                        || (SharedString::from(task.queue_id.clone()), i64::MAX - 1),
                        |(ix, (_, name))| (SharedString::from(name.clone()), ix as i64),
                    );
                GroupBucket {
                    key: format!("queue:{}", task.queue_id),
                    label,
                    order,
                }
            }
            ViewGroupBy::Site => {
                let site = task.source_site();
                let label = if site.is_empty() {
                    strings.site_unknown(task)
                } else {
                    SharedString::from(site.to_owned())
                };
                GroupBucket {
                    key: format!("site:{}", label),
                    label,
                    order: 0,
                }
            }
            ViewGroupBy::Group => {
                if task.group_id.is_empty() {
                    return GroupBucket {
                        key: "group:".to_owned(),
                        label: strings.ungrouped.clone(),
                        order: i64::MAX,
                    };
                }
                let label = self
                    .group_names
                    .get(&task.group_id)
                    .cloned()
                    .unwrap_or_else(|| task.group_id.clone());
                GroupBucket {
                    key: format!("group:{}", task.group_id),
                    label: SharedString::from(label),
                    order: 0,
                }
            }
        }
    }

    fn visible_task_ids(&self) -> impl Iterator<Item = RowId> + '_ {
        self.visible.iter().filter_map(|row| match row {
            VisibleRow::Task(id) => Some(*id),
            VisibleRow::GroupHeader { .. } => None,
        })
    }

    fn visible_task_keys(&self) -> Vec<RowKey> {
        self.visible_task_ids()
            .filter_map(|id| self.store.row(id).map(|row| row.key.clone()))
            .collect()
    }

    pub(crate) fn count_matching(&self, filter: &DownloadFilter) -> usize {
        let local = self.store.local();
        let remote = self.store.remote();
        local
            .iter()
            .chain(remote.iter())
            .filter(|task| filter.matches(task, &self.categories))
            .count()
    }

    pub(crate) fn count_where(&self, predicate: impl Fn(&DownloadTaskView) -> bool) -> usize {
        let local = self.store.local();
        let remote = self.store.remote();
        local
            .iter()
            .chain(remote.iter())
            .filter(|t| predicate(t))
            .count()
    }

    pub(crate) fn count_in_queue(&self, queue_id: &str) -> usize {
        self.store
            .local()
            .iter()
            .filter(|task| task.queue_id == queue_id)
            .count()
    }

    fn shown_columns_count(&self) -> usize {
        self.columns.iter().filter(|column| column.visible).count()
    }

    fn shown_column(&self, col_ix: usize) -> Option<&DownloadColumn> {
        if col_ix == 0 {
            return None;
        }
        self.columns
            .iter()
            .filter(|column| column.visible)
            .nth(col_ix - 1)
    }

    fn move_shown_column(&mut self, from_ix: usize, to_ix: usize) {
        if from_ix == 0 || to_ix == 0 {
            return;
        }
        let positions: Vec<_> = self
            .columns
            .iter()
            .enumerate()
            .filter_map(|(ix, column)| column.visible.then_some(ix))
            .collect();
        let Some(&from_position) = positions.get(from_ix - 1) else {
            return;
        };
        let Some(&to_position) = positions.get(to_ix - 1) else {
            return;
        };
        let column = self.columns.remove(from_position);
        self.columns.insert(to_position, column);
    }

    pub(crate) fn move_column_kind(&mut self, from: DownloadColumnKind, to: DownloadColumnKind) {
        let Some(from_ix) = self.columns.iter().position(|column| column.kind == from) else {
            return;
        };
        let Some(to_ix) = self.columns.iter().position(|column| column.kind == to) else {
            return;
        };
        if from_ix == to_ix {
            return;
        }
        let column = self.columns.remove(from_ix);
        self.columns.insert(to_ix, column);
    }

    pub(crate) fn set_column_visible(&mut self, kind: DownloadColumnKind, visible: bool) -> bool {
        if !visible && self.columns.iter().filter(|column| column.visible).count() <= 1 {
            return false;
        }
        let Some(column) = self.columns.iter_mut().find(|column| column.kind == kind) else {
            return false;
        };
        if column.visible == visible {
            return false;
        }
        column.visible = visible;
        true
    }

    /// 恢复默认列集；文件名列同时回到「吸收剩余宽度」模式。
    pub(crate) fn reset_columns(&mut self) {
        self.columns = DownloadColumnKind::ALL
            .into_iter()
            .map(DownloadColumn::new)
            .collect();
        self.prefs.file_name_width = None;
    }

    pub(crate) fn select_all_tasks(&mut self) {
        self.selected_tasks.clear();
        self.selected_tasks.extend(self.visible_task_keys());
        self.selection_anchor = None;
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected_tasks.clear();
        self.selection_anchor = None;
    }

    fn select_task(&mut self, key: RowKey, modifiers: Modifiers) {
        if modifiers.shift
            && let Some(anchor) = self.selection_anchor.clone()
        {
            let keys = self.visible_task_keys();
            if let Some(anchor_index) = keys.iter().position(|item| *item == anchor)
                && let Some(task_index) = keys.iter().position(|item| *item == key)
            {
                let (start, end) = if anchor_index <= task_index {
                    (anchor_index, task_index)
                } else {
                    (task_index, anchor_index)
                };
                self.selected_tasks.clear();
                self.selected_tasks
                    .extend(keys[start..=end].iter().cloned());
                self.selection_anchor = Some(key);
                return;
            }
        }

        if modifiers.secondary() {
            if !self.selected_tasks.remove(&key) {
                self.selected_tasks.insert(key.clone());
            }
        } else {
            self.selected_tasks.clear();
            self.selected_tasks.insert(key.clone());
        }
        self.selection_anchor = Some(key);
    }

    pub(crate) fn row_key_at(&self, row_ix: usize) -> Option<RowKey> {
        match self.visible.get(row_ix)? {
            VisibleRow::Task(id) => self.store.row(*id).map(|row| row.key.clone()),
            VisibleRow::GroupHeader { .. } => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn group_key_at(&self, row_ix: usize) -> Option<&str> {
        match self.visible.get(row_ix)? {
            VisibleRow::GroupHeader { key, .. } => Some(key),
            VisibleRow::Task(_) => None,
        }
    }

    pub(crate) fn select_task_for_context_menu(&mut self, key: RowKey) {
        if !self.selected_tasks.contains(&key) {
            self.selected_tasks.clear();
            self.selected_tasks.insert(key.clone());
        }
        self.selection_anchor = Some(key);
    }

    pub(crate) fn selected_keys(&self) -> Vec<RowKey> {
        let mut keys: Vec<RowKey> = self.selected_tasks.iter().cloned().collect();
        keys.sort();
        keys
    }

    /// 选中集合投影（选择条 / 工具栏）：只统计仍存在于 store 的选中任务
    /// （已删除任务不算），完整遍历以得到数量与各类可用性。
    pub(crate) fn selection_summary(&self) -> SelectionSummary {
        let mut summary = SelectionSummary::default();
        for key in &self.selected_tasks {
            let Some(row) = self.store.get(key) else {
                continue;
            };
            summary.count += 1;
            summary.any = true;
            summary.any_local |= key.is_local();
            match row.state {
                TaskState::Downloading | TaskState::Pending => summary.any_active = true,
                TaskState::Paused | TaskState::Failed => summary.any_resumable = true,
                TaskState::Completed => {}
            }
        }
        summary
    }

    pub(crate) fn toggle_group_collapsed(&mut self, key: &str) {
        self.prefs.toggle_group_collapsed(key);
        self.view_dirty = true;
    }

    /// 文件类型图标与类别名。
    fn kind_visual(&self, kind: TaskKind) -> (FluxIcon, SharedString) {
        let strings = &self.strings;
        let label = match kind {
            TaskKind::Video => &strings.category_video,
            TaskKind::Audio => &strings.category_audio,
            TaskKind::Document => &strings.category_document,
            TaskKind::Image => &strings.category_image,
            TaskKind::Archive | TaskKind::DiskImage => &strings.category_archive,
            TaskKind::Application | TaskKind::Mobile => &strings.category_program,
            TaskKind::Other => &strings.category_other,
        };
        (kind_icon(kind), label.clone())
    }

    /// 状态列主文案：下载中显示「速度 · 剩余时间」（只显示已知部分），其余为状态名。
    fn status_label(&self, task: &DownloadTaskView) -> SharedString {
        if task.state != TaskState::Downloading {
            return self.strings.state_label(task.state);
        }
        let speed = task
            .speed_bytes_per_second
            .filter(|speed| *speed > 0)
            .map(|speed| format!("{}/s", format_bytes(speed)));
        let eta = task
            .eta_seconds
            .filter(|seconds| *seconds <= MAX_ETA_SECS)
            .map(|seconds| self.strings.format_eta(seconds));
        match (speed, eta) {
            (Some(speed), Some(eta)) => SharedString::from(format!("{speed} · {eta}")),
            (Some(speed), None) => SharedString::from(speed),
            (None, Some(eta)) => eta,
            (None, None) => self.strings.status_downloading.clone(),
        }
    }

    /// 状态列第二行（舒适密度）/ 紧凑密度的悬停提示：已下 / 总量、并发连接或 BT
    /// 节点、失败原因首行。
    fn status_detail(&self, task: &DownloadTaskView) -> Option<String> {
        match task.state {
            TaskState::Downloading => {
                let bytes = bytes_progress(task);
                Some(match self.transfer_detail(task) {
                    Some(transfers) => format!("{bytes} · {transfers}"),
                    None => bytes,
                })
            }
            TaskState::Paused => Some(bytes_progress(task)),
            TaskState::Failed => task
                .error_message
                .lines()
                .next()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned),
            TaskState::Pending | TaskState::Completed => None,
        }
    }

    /// 实时并发：本地分段连接数与 BT 已连节点数。
    fn transfer_detail(&self, task: &DownloadTaskView) -> Option<String> {
        let active = task.active_transfers();
        let peers = task
            .runtime
            .as_ref()
            .filter(|_| task.runtime_connected && task.protocol == TaskProtocol::Bt)
            .and_then(|runtime| runtime.connected_peers);
        match (active, peers) {
            (Some(active), Some(peers)) => Some(format!(
                "{active} {} · {peers} {}",
                self.strings.active_transfers, self.strings.connected_peers
            )),
            (Some(active), None) => Some(format!("{active} {}", self.strings.active_transfers)),
            (None, Some(peers)) => Some(format!("{peers} {}", self.strings.connected_peers)),
            (None, None) => None,
        }
    }

    /// 固定左列：平时显示文件类型图标（元数据加载中为 Spinner）；行悬停时经
    /// `group_hover` 换成复选框；行已选中或存在任意选中时复选框常显。
    /// 隐藏（`invisible`）的元素不绘制、不注册鼠标监听，不会拦截行点击。
    fn render_selection_cell(
        &self,
        row_ix: usize,
        task: &DownloadTaskView,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let theme = active_theme(cx);
        let muted = theme.tokens().colors.muted_foreground;
        let icon_sizes = theme.extended().icon;
        let key = task.key.clone();
        let selected = self.selected_tasks.contains(&key);
        let checkbox_pinned = selected || !self.selected_tasks.is_empty();
        let glyph = (!checkbox_pinned).then(|| {
            if task.metadata_pending {
                Spinner::new()
                    .with_size(icon_sizes.md)
                    .icon(IconName::LoaderCircle)
                    .color(muted)
                    .into_any_element()
            } else {
                Icon::new(self.kind_visual(task.kind).0)
                    .size(icon_sizes.lg)
                    .text_color(muted)
                    .into_any_element()
            }
        });
        let hover_border = theme.tokens().colors.foreground.opacity(0.7);
        let checkbox = div()
            .id(("download-task-multi-select", row_ix))
            .p(px(4.))
            .cursor_pointer()
            .child(
                check_mark(
                    if selected {
                        CheckState::Checked
                    } else {
                        CheckState::Unchecked
                    },
                    cx,
                )
                .when(!selected, |mark| {
                    mark.hover(move |style| style.border_color(hover_border))
                }),
            )
            .on_click(cx.listener(move |table, _: &ClickEvent, _, cx| {
                cx.stop_propagation();
                let delegate = table.delegate_mut();
                if !delegate.selected_tasks.remove(&key) {
                    delegate.selected_tasks.insert(key.clone());
                }
                delegate.selection_anchor = Some(key.clone());
                cx.notify();
            }));
        div()
            .size_full()
            .relative()
            .when_some(glyph, |this, glyph| {
                this.child(
                    h_flex()
                        .absolute()
                        .inset_0()
                        .justify_center()
                        .group_hover(ROW_GROUP, |style| style.invisible())
                        .child(glyph),
                )
            })
            .child(
                h_flex()
                    .absolute()
                    .inset_0()
                    .justify_center()
                    .when(!checkbox_pinned, |this| {
                        this.invisible()
                            .group_hover(ROW_GROUP, |style| style.visible())
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(checkbox),
            )
            .into_any_element()
    }

    fn render_file_cell(&self, task: &DownloadTaskView, cx: &App) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let (name, name_color) = if task.metadata_pending {
            (
                self.strings.metadata_loading.clone(),
                tokens.colors.muted_foreground,
            )
        } else {
            (
                SharedString::from(task.name.clone()),
                tokens.colors.foreground,
            )
        };
        let meta = (self.prefs.density.two_line() && !task.metadata_pending).then(|| {
            let (_, category) = self.kind_visual(task.kind);
            let site = task.source_site();
            if site.is_empty() {
                category
            } else {
                SharedString::from(format!("{category} · {site}"))
            }
        });
        v_flex()
            .size_full()
            .min_w_0()
            .justify_center()
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(tokens.typography.sm.size)
                    .line_height(tokens.typography.sm.line_height)
                    .font_weight(tokens.typography.sm.weight)
                    .text_color(name_color)
                    .child(name),
            )
            .when_some(meta, |this, meta| {
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(tokens.typography.xs.size)
                        .line_height(tokens.typography.xs.line_height)
                        .text_color(theme.extended().colors.text_tertiary)
                        .child(meta),
                )
            })
            .into_any_element()
    }

    /// 活动列：主文案按状态着色（仅下载中 primary、失败 destructive），舒适密度
    /// 第二行给详情；紧凑密度把详情放进悬停提示，失败总是提示完整原因。
    fn render_status_cell(&self, task: &DownloadTaskView, cx: &App) -> AnyElement {
        let theme = active_theme(cx);
        let typography = &theme.tokens().typography;
        let two_line = self.prefs.density.two_line();
        let detail = self.status_detail(task);
        let tooltip = if task.state == TaskState::Failed && !task.error_message.trim().is_empty() {
            Some(SharedString::from(task.error_message.clone()))
        } else if two_line {
            None
        } else {
            detail.clone().map(SharedString::from)
        };
        v_flex()
            .id(SharedString::from(format!(
                "download-status-{}",
                task.key.task_id()
            )))
            .size_full()
            .min_w_0()
            .justify_center()
            .text_size(typography.xs.size)
            .line_height(typography.xs.line_height)
            .font_features(tabular_numbers())
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(status_color(task.state, cx))
                    .child(self.status_label(task)),
            )
            .when_some(detail.filter(|_| two_line), |this, detail| {
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.extended().colors.text_tertiary)
                        .child(detail),
                )
            })
            .when_some(tooltip, |this, tooltip| {
                this.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            })
            .into_any_element()
    }

    /// 进度单元格：4px 满圆角条 + 整数百分比；完成态整格留空（完成只由状态列表达）。
    fn render_progress_cell(&self, task: &DownloadTaskView, cx: &App) -> AnyElement {
        if task.state == TaskState::Completed {
            return div().into_any_element();
        }
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let colors = &tokens.colors;
        let bar_color = progress_bar_color(task.state, cx);
        let column_width = self
            .columns
            .iter()
            .find(|column| column.kind == DownloadColumnKind::Progress)
            .map_or(DownloadColumnKind::Progress.min_width(), |column| {
                column.width
            });
        // 条宽 = 列内容宽（扣左右内边距）− 百分比区 − 间距；与下方布局同一组常量。
        let bar_width =
            (column_width - 2. * CELL_PADDING_X - PROGRESS_LABEL_WIDTH - PROGRESS_GAP).max(0.);
        h_flex()
            .size_full()
            .min_w_0()
            .gap(px(PROGRESS_GAP))
            .child(
                crate::components::segment_progress::render_segment_progress(
                    task.runtime.as_deref(),
                    task.progress,
                    bar_width,
                    PROGRESS_BAR_HEIGHT,
                    bar_color,
                    progress_track_color(cx),
                ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(PROGRESS_LABEL_WIDTH))
                    .text_right()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .font_features(tabular_numbers())
                    .text_color(colors.muted_foreground)
                    .child(percent_label(task.progress)),
            )
            .into_any_element()
    }

    /// 行悬停操作（最后一个可见列右端浮层）：暂停 / 继续 / 重试 / 打开 + 在文件夹中
    /// 显示。底色与行悬停一致（选中时叠加选中色），点击不改变选中。
    fn render_row_actions(&self, task: &DownloadTaskView, cx: &App) -> Option<AnyElement> {
        let host = self.host.as_ref()?;
        let mut actions = row_actions(task.state, task.key.is_local()).peekable();
        actions.peek()?;
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let (muted, foreground) = (tokens.colors.muted_foreground, tokens.colors.foreground);
        let pressed = extended.colors.nav_selected;
        let selected = self.selected_tasks.contains(&task.key);
        let buttons = actions.map(|action| {
            let (icon, tooltip) = match action {
                RowAction::Pause => (FluxIcon::Pause, self.strings.pause.clone()),
                RowAction::Resume => (FluxIcon::Play, self.strings.resume.clone()),
                RowAction::Retry => (FluxIcon::RotateCw, self.strings.resume.clone()),
                RowAction::Open => (FluxIcon::ExternalLink, self.strings.open_file.clone()),
                RowAction::Reveal => (FluxIcon::FolderOpen, self.strings.open_folder.clone()),
            };
            let host = host.clone();
            let key = task.key.clone();
            h_flex()
                .id(action.id())
                .flex_none()
                .size(px(ROW_ACTION_SIZE))
                .justify_center()
                .rounded(tokens.radius.sm)
                .cursor_pointer()
                .text_color(muted)
                .hover(move |style| style.bg(pressed).text_color(foreground))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let Some(command) = task_command(&key, action.command()) else {
                        return;
                    };
                    let _ = host.update(cx, |view, cx| view.execute_commands(vec![command], cx));
                })
                .child(Icon::new(icon).size(extended.icon.md))
        });
        Some(
            h_flex()
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .gap(tokens.spacing.xxs)
                .pl(tokens.spacing.xs)
                .bg(extended.colors.row_hover)
                .invisible()
                .group_hover(ROW_GROUP, |style| style.visible())
                .when(selected, |this| {
                    this.child(div().absolute().inset_0().bg(tokens.colors.accent))
                })
                .children(buttons)
                .into_any_element(),
        )
    }

    /// 分组头：xs 中等字重正文色标签 + 三级色计数，28 高内容贴底，行槽位多出的
    /// 高度作上方留白；不加底色块。
    fn render_group_header(
        &self,
        row_ix: usize,
        key: &str,
        label: &SharedString,
        count: usize,
        collapsed: bool,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        let group_progress =
            key.strip_prefix("group:")
                .filter(|id| !id.is_empty())
                .map(|group_id| {
                    self.store
                        .local()
                        .iter()
                        .filter(|task| task.group_id == group_id)
                        .fold((0usize, 0usize), |(completed, total), task| {
                            let completed = if task.state == TaskState::Completed {
                                completed + 1
                            } else {
                                completed
                            };
                            (completed, total + 1)
                        })
                });
        let key = key.to_owned();
        let on_click = cx.listener(move |table, _: &ClickEvent, _, cx| {
            table.delegate_mut().toggle_group_collapsed(&key);
            table.delegate_mut().refresh_view();
            table.refresh(cx);
            cx.notify();
        });
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        let tertiary = extended.colors.text_tertiary;
        div()
            .id(("download-group-header", row_ix))
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(on_click)
            .child(
                h_flex()
                    .h(px(GROUP_HEADER_HEIGHT))
                    .min_w_0()
                    .gap(tokens.spacing.xs)
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .child(
                        Icon::new(if collapsed {
                            FluxIcon::ChevronRight
                        } else {
                            FluxIcon::ChevronDown
                        })
                        .size(extended.icon.sm)
                        .text_color(tertiary),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(tokens.colors.foreground)
                            .child(label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_features(tabular_numbers())
                            .text_color(tertiary)
                            .child(count.to_string()),
                    )
                    .when_some(group_progress, |this, (completed, total)| {
                        this.child(
                            div()
                                .flex_none()
                                .font_features(tabular_numbers())
                                .text_color(tertiary)
                                .child(format!("{completed}/{total}")),
                        )
                    }),
            )
            .into_any_element()
    }

    /// 选中集合 → 任务事实；选择滞后于存储事件时任一键缺失即返回 `None`。
    fn selected_menu_facts(&self) -> Option<(Vec<RowKey>, Vec<TaskMenuFacts>)> {
        let selected = self.selected_keys();
        if selected.is_empty() {
            return None;
        }
        let facts = selected
            .iter()
            .map(|key| self.task_menu_facts(key))
            .collect::<Option<Vec<_>>>()?;
        Some((selected, facts))
    }

    fn task_menu_facts(&self, key: &RowKey) -> Option<TaskMenuFacts> {
        let row = self.store.get(key)?;
        Some(TaskMenuFacts {
            is_local: key.is_local(),
            state: row.state,
            boosted: row.boosted,
            is_torrent_sentinel: row.url.starts_with("torrent-file://"),
            is_plugin_retry_error: row.state == TaskState::Failed
                && row.error_message.starts_with(PLUGIN_ERROR_PREFIX),
        })
    }

    /// 任务行右键菜单：按 [`context_menu_items`] 的纯函数结果逐项渲染。
    fn task_context_menu(&self, menu: PopupMenu) -> PopupMenu {
        let Some((selected, facts)) = self.selected_menu_facts() else {
            return menu;
        };
        context_menu_items(&facts)
            .into_iter()
            .fold(menu, |menu, entry| {
                self.apply_menu_entry(menu, entry, &selected)
            })
    }

    fn apply_menu_entry(
        &self,
        menu: PopupMenu,
        entry: MenuEntry,
        selected: &[RowKey],
    ) -> PopupMenu {
        match entry {
            MenuEntry::Resume => menu.menu_with_icon(
                self.strings.resume.clone(),
                FluxIcon::Play,
                Box::new(crate::actions::ResumeSelected),
            ),
            MenuEntry::Pause => menu.menu_with_icon(
                self.strings.pause.clone(),
                FluxIcon::Pause,
                Box::new(crate::actions::PauseSelected),
            ),
            MenuEntry::IgnorePluginRetry => {
                let (Some(host), Some(task_id)) = (
                    self.host.clone(),
                    selected.first().map(|key| key.task_id().to_owned()),
                ) else {
                    return menu;
                };
                menu.item(
                    PopupMenuItem::new(self.strings.ignore_plugin_retry.clone())
                        .icon(FluxIcon::CircleAlert)
                        .on_click(move |_, window, cx| {
                            let task_id = task_id.clone();
                            let _ = host.update(cx, |view, cx| {
                                view.confirm_ignore_plugin_retry(task_id, window, cx);
                            });
                        }),
                )
            }
            MenuEntry::Boost | MenuEntry::CancelBoost => {
                let cancel = matches!(entry, MenuEntry::CancelBoost);
                menu.menu_with_icon(
                    if cancel {
                        self.strings.cancel_boost.clone()
                    } else {
                        self.strings.boost_download.clone()
                    },
                    FluxIcon::Zap,
                    Box::new(crate::actions::ToggleBoostSelected),
                )
            }
            MenuEntry::OpenFile => menu.menu_with_icon(
                self.strings.open_file.clone(),
                FluxIcon::ExternalLink,
                Box::new(crate::actions::OpenSelected),
            ),
            MenuEntry::OpenFolder => menu.menu_with_icon(
                self.strings.open_folder.clone(),
                FluxIcon::FolderOpen,
                Box::new(crate::actions::RevealSelected),
            ),
            MenuEntry::Rename => menu.menu_with_icon(
                self.strings.rename_task.clone(),
                FluxIcon::Pen,
                Box::new(crate::actions::RenameSelected),
            ),
            MenuEntry::Redownload => menu.menu_with_icon(
                self.strings.redownload_task.clone(),
                FluxIcon::RotateCw,
                Box::new(crate::actions::RedownloadSelected),
            ),
            MenuEntry::CopyUrl => menu.menu_with_icon(
                self.strings.copy_url.clone(),
                FluxIcon::Copy,
                Box::new(crate::actions::CopySelectedUrl),
            ),
            MenuEntry::MoveToQueue => self.append_move_to_queue(menu),
            MenuEntry::Separator => menu.separator(),
            MenuEntry::Delete => menu.menu_with_icon(
                self.strings.delete_task.clone(),
                FluxIcon::Trash2,
                Box::new(crate::actions::DeleteSelected),
            ),
            MenuEntry::DeleteWithFiles => menu.menu_with_icon(
                self.strings.delete_task_and_file.clone(),
                FluxIcon::Trash2,
                Box::new(crate::actions::DeleteSelectedWithFiles),
            ),
            MenuEntry::OpenInWindow => menu.menu_with_icon(
                self.strings.open_in_window.clone(),
                FluxIcon::AppWindow,
                Box::new(crate::actions::OpenSelectedInWindow),
            ),
        }
    }

    /// 「移动到队列」：固定版 gpui-component 的 `PopupMenu::submenu` 需要
    /// `Context<PopupMenu>`，而 `TableDelegate::context_menu` 只提供
    /// `Context<TableState<Self>>`（两者是不同实体的上下文，无法互转），因此
    /// 这里无法构造真正的嵌套子菜单，改为「小节标题 + 缩进平铺项」。
    fn append_move_to_queue(&self, menu: PopupMenu) -> PopupMenu {
        let Some(host) = self.host.clone() else {
            return menu;
        };
        if self.queue_names.is_empty() {
            return menu;
        }
        let mut menu = menu.label(self.strings.move_to_queue.clone());
        for (queue_id, name) in &self.queue_names {
            let host = host.clone();
            let queue_id = queue_id.clone();
            menu = menu.item(
                PopupMenuItem::new(SharedString::from(format!("    {name}"))).on_click(
                    move |_, _, cx| {
                        let queue_id = queue_id.clone();
                        let _ =
                            host.update(cx, |view, cx| view.move_selected_to_queue(queue_id, cx));
                    },
                ),
            );
        }
        menu
    }

    /// 分组头行右键菜单（P1.8）；`group_id` 为空（未分组桶）不显示菜单。
    fn group_context_menu(&self, group_id: &str, menu: PopupMenu) -> PopupMenu {
        if group_id.is_empty() {
            return menu;
        }
        let Some(host) = self.host.as_ref() else {
            return menu;
        };
        let has_failed_member = self
            .store
            .local()
            .iter()
            .any(|task| task.group_id == group_id && task.state == TaskState::Failed);

        let mut menu = menu.item(group_menu_item(
            self.strings.group_pause_all.clone(),
            FluxIcon::Pause,
            host,
            group_id,
            DownloadView::group_pause_all,
        ));
        menu = menu.item(group_menu_item(
            self.strings.group_resume_all.clone(),
            FluxIcon::Play,
            host,
            group_id,
            DownloadView::group_resume_all,
        ));
        if has_failed_member {
            menu = menu.item(group_menu_item(
                self.strings.group_retry_failed.clone(),
                FluxIcon::RotateCw,
                host,
                group_id,
                DownloadView::group_retry_failed,
            ));
        }
        menu = menu.item(group_menu_item(
            self.strings.group_open_folder.clone(),
            FluxIcon::FolderOpen,
            host,
            group_id,
            DownloadView::group_open_folder,
        ));
        menu = menu.item(group_menu_item(
            self.strings.group_copy_source_link.clone(),
            FluxIcon::Copy,
            host,
            group_id,
            DownloadView::group_copy_source_link,
        ));
        menu = menu.separator();
        menu = menu.item(group_menu_item(
            self.strings.group_delete.clone(),
            FluxIcon::Trash2,
            host,
            group_id,
            DownloadView::group_delete,
        ));
        menu = menu.item(group_menu_item_windowed(
            self.strings.group_delete_with_files.clone(),
            FluxIcon::Trash2,
            host,
            group_id,
            DownloadView::confirm_group_delete_with_files,
        ));
        menu.item(group_menu_item_windowed(
            self.strings.open_group_in_window.clone(),
            FluxIcon::AppWindow,
            host,
            group_id,
            DownloadView::open_group_in_window,
        ))
    }
}

impl TableDelegate for DownloadTableDelegate {
    fn columns_count(&self, _cx: &App) -> usize {
        self.shown_columns_count() + 1
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.visible.len()
    }

    /// 空态：图标 + 标题 + 引导（与 Flutter 桌面端同文案）。
    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(tokens.spacing.sm)
            .child(
                Icon::new(FluxIcon::Download)
                    .size(px(32.))
                    .text_color(theme.extended().colors.text_tertiary),
            )
            .child(
                div()
                    .text_size(tokens.typography.sm.size)
                    .line_height(tokens.typography.sm.line_height)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(tokens.colors.foreground)
                    .child(self.strings.empty_title.clone()),
            )
            .child(
                div()
                    .text_size(tokens.typography.xs.size)
                    .line_height(tokens.typography.xs.line_height)
                    .text_color(tokens.colors.muted_foreground)
                    .child(self.strings.empty_subtitle.clone()),
            )
    }

    /// 排序指示由 `render_th` 自绘，列不设 `sort`（去掉 DataTable 常驻排序图标）。
    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        if col_ix == 0 {
            // 不设 `fixed_left`：gpui-component 会给固定列画一条 `border` 色竖线，
            // 而文件名列已吸收剩余宽度，常规列集不会出现横向滚动。
            return Column::new("selection", "")
                .width(px(SELECTION_COLUMN_WIDTH))
                .min_width(px(SELECTION_COLUMN_WIDTH))
                .max_width(px(SELECTION_COLUMN_WIDTH))
                .resizable(false)
                .movable(false)
                .selectable(false)
                .p_0();
        }

        let Some(column) = self.shown_column(col_ix) else {
            return Column::new("missing", "").resizable(false).movable(false);
        };
        let base = Column::new(column.kind.key(), column.kind.label(&self.strings))
            .min_width(px(column.kind.min_width()))
            .paddings(Edges {
                top: px(0.),
                right: px(CELL_PADDING_X),
                bottom: px(0.),
                left: px(CELL_PADDING_X),
            });
        if column.kind == DownloadColumnKind::FileName {
            // 默认吸收剩余宽度（其他列拖宽后由它补足）；拖过后固定为用户宽度。
            return base
                .width(px(self.apply_file_name_width()))
                .max_width(px(FILE_NAME_MAX_WIDTH));
        }
        base.width(px(column.width)).max_width(px(MAX_COLUMN_WIDTH))
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if col_ix == 0 {
            let keys = self.visible_task_keys();
            let selected_count = keys
                .iter()
                .filter(|key| self.selected_tasks.contains(*key))
                .count();
            let state = match selected_count {
                0 => CheckState::Unchecked,
                n if n == keys.len() => CheckState::Checked,
                _ => CheckState::Indeterminate,
            };
            let hover_border = active_theme(cx).tokens().colors.foreground.opacity(0.7);
            // 全选框平时隐藏，悬停表头该格或已有选中时出现，避免表头常驻一个空框。
            return h_flex()
                .id("select-all-download-tasks")
                .group(SELECT_ALL_GROUP)
                .size_full()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .when(state == CheckState::Unchecked, |this| {
                            this.invisible()
                                .group_hover(SELECT_ALL_GROUP, |style| style.visible())
                        })
                        .child(check_mark(state, cx).when(
                            state == CheckState::Unchecked,
                            |mark| {
                                mark.group_hover(SELECT_ALL_GROUP, move |style| {
                                    style.border_color(hover_border)
                                })
                            },
                        )),
                )
                .on_click(cx.listener(move |table, _: &ClickEvent, _, cx| {
                    let delegate = table.delegate_mut();
                    if state == CheckState::Checked {
                        delegate.clear_selection();
                    } else {
                        delegate.select_all_tasks();
                    }
                    cx.notify();
                }))
                .into_any_element();
        }

        let Some(kind) = self.shown_column(col_ix).map(|column| column.kind) else {
            return div().into_any_element();
        };
        let numeric = kind.is_numeric();
        let sort_key = kind.sort_key();
        // 只有当前排序列显示方向箭头；智能排序没有方向，不显示。
        let arrow = sort_key
            .filter(|key| *key == self.prefs.sort_key && *key != ViewSortKey::Smart)
            .map(|_| match self.prefs.sort_dir {
                SortDir::Asc => FluxIcon::ArrowUp,
                SortDir::Desc => FluxIcon::ArrowDown,
            });
        let on_click = sort_key.map(|_| {
            cx.listener(move |table, event: &ClickEvent, _, cx| {
                // 拖动列头换位后在原列松开也会产生 click，位移过大时不当作排序。
                if let ClickEvent::Mouse(click) = event {
                    let delta = click.up.position - click.down.position;
                    if f32::from(delta.x).abs() + f32::from(delta.y).abs() > HEADER_CLICK_SLOP {
                        return;
                    }
                }
                let delegate = table.delegate_mut();
                if !delegate.toggle_sort(kind) {
                    return;
                }
                delegate.refresh_view();
                // 偏好写回走宿主的防抖持久化；表格实体此刻正被更新，放到帧外调用。
                if let Some(host) = delegate.host.clone() {
                    cx.defer(move |cx| {
                        let _ = host.update(cx, |view, cx| view.schedule_persist_prefs(cx));
                    });
                }
                cx.notify();
            })
        });
        let theme = active_theme(cx);
        let typography = &theme.tokens().typography;
        let extended = theme.extended();
        let label = div().min_w_0().truncate().child(kind.label(&self.strings));
        let arrow = arrow.map(|icon| Icon::new(icon).size(extended.icon.sm));
        // 每列右缘常驻一条短分隔线，提示此处可拖宽。gpui-component 的拖宽柄
        // 取 `table_row_border`（为去网格线已设透明，且同色还控制行线与填充行，不能改），
        // 仅悬停时显示；这里自绘同位置的线：th 内容右缘向外偏 `CELL_PADDING_X`
        // 恰为单元格右缘，与拖宽柄的 1px 线重合，悬停时被其全高高亮覆盖。
        let divider = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right(px(-CELL_PADDING_X))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(1.))
                    .h(px(COLUMN_DIVIDER_HEIGHT))
                    .bg(extended.colors.hairline),
            );
        h_flex()
            .id(("download-column-header", col_ix))
            .size_full()
            .relative()
            .min_w_0()
            .gap(theme.tokens().spacing.xxs)
            .text_size(typography.xs.size)
            .line_height(typography.xs.line_height)
            .font_weight(FontWeight::MEDIUM)
            .text_color(extended.colors.text_tertiary)
            // 数字列右对齐：箭头放在标签左侧，保持标签右缘与数值对齐。
            .map(|this| {
                if numeric {
                    this.justify_end().children(arrow).child(label)
                } else {
                    this.child(label).children(arrow)
                }
            })
            .when_some(on_click, |this, on_click| {
                this.cursor_pointer().on_click(on_click)
            })
            .child(divider)
            .into_any_element()
    }

    /// 行高按密度覆盖 DataTable 的统一尺寸（DataTable 用 `refine_style` 合并本样式，
    /// 高度以这里为准）；分组头行与任务行等高（uniform_list 要求）。
    fn render_tr(
        &mut self,
        row_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let row_height = px(self.prefs.density.row_height());
        let Some(VisibleRow::Task(id)) = self.visible.get(row_ix).cloned() else {
            return div().id(("download-group-row", row_ix)).h(row_height);
        };
        let Some(key) = self.store.row(id).map(|row| row.key.clone()) else {
            return div().id(("download-task-row", row_ix)).h(row_height);
        };
        let selected = self.selected_tasks.contains(&key);
        let tokens = active_theme(cx).tokens();
        let (accent, radius) = (tokens.colors.accent, tokens.radius.md);

        div()
            .id(("download-task-row", row_ix))
            .h(row_height)
            .group(ROW_GROUP)
            .relative()
            .when(selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .left(px(SELECTED_INSET_X))
                        .right(px(SELECTED_INSET_X))
                        .rounded(radius)
                        .bg(accent),
                )
            })
            .on_click(cx.listener(move |table, event: &ClickEvent, _, cx| {
                table
                    .delegate_mut()
                    .select_task(key.clone(), event.modifiers());
                cx.notify();
            }))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let id = match self.visible.get(row_ix) {
            Some(VisibleRow::Task(id)) => *id,
            Some(VisibleRow::GroupHeader {
                key,
                label,
                count,
                collapsed,
            }) => {
                if col_ix == 1 {
                    let (key, label, count, collapsed) =
                        (key.clone(), label.clone(), *count, *collapsed);
                    return self.render_group_header(row_ix, &key, &label, count, collapsed, cx);
                }
                return div().into_any_element();
            }
            None => return div().into_any_element(),
        };
        let Some(task) = self.store.row(id) else {
            return div().into_any_element();
        };
        let task: &DownloadTaskView = &task;

        if col_ix == 0 {
            return self.render_selection_cell(row_ix, task, cx);
        }

        let Some(kind) = self.shown_column(col_ix).map(|column| column.kind) else {
            return div().into_any_element();
        };
        let theme = active_theme(cx);
        let muted = theme.tokens().colors.muted_foreground;
        let tertiary = theme.extended().colors.text_tertiary;
        let downloading = task.state == TaskState::Downloading;
        let cell = match kind {
            DownloadColumnKind::FileName => self.render_file_cell(task, cx),
            DownloadColumnKind::Progress => self.render_progress_cell(task, cx),
            DownloadColumnKind::Size if task.size_bytes > 0 => {
                meta_cell(task.size.clone(), muted, true, cx)
            }
            DownloadColumnKind::Status => self.render_status_cell(task, cx),
            DownloadColumnKind::Speed => match task
                .speed_bytes_per_second
                .filter(|speed| downloading && *speed > 0)
            {
                Some(speed) => meta_cell(format!("{}/s", format_bytes(speed)), muted, true, cx),
                None => meta_cell("—", tertiary, true, cx),
            },
            DownloadColumnKind::Eta => match task
                .eta_seconds
                .filter(|seconds| downloading && *seconds <= MAX_ETA_SECS)
            {
                Some(seconds) => meta_cell(self.strings.format_eta(seconds), muted, true, cx),
                None => meta_cell("—", tertiary, true, cx),
            },
            DownloadColumnKind::Size => meta_cell("—", tertiary, true, cx),
            DownloadColumnKind::Created if task.created_at_secs > 0 => meta_cell(
                DownloadStrings::format_datetime(task.created_at_secs),
                muted,
                true,
                cx,
            ),
            DownloadColumnKind::Created => meta_cell("—", tertiary, true, cx),
            DownloadColumnKind::Protocol => meta_cell(task.protocol.label(), muted, false, cx),
            DownloadColumnKind::Source => {
                meta_cell(task.source_site().to_owned(), muted, false, cx)
            }
            DownloadColumnKind::Queue => {
                let name = self
                    .queue_names
                    .iter()
                    .find(|(id, _)| *id == task.queue_id)
                    .map_or_else(|| task.queue_id.clone(), |(_, name)| name.clone());
                meta_cell(name, muted, false, cx)
            }
        };
        if col_ix == self.shown_columns_count()
            && let Some(actions) = self.render_row_actions(task, cx)
        {
            return div()
                .size_full()
                .relative()
                .child(cell)
                .child(actions)
                .into_any_element();
        }
        cell
    }

    /// 右侧留白与选择列内含的左侧留白对称。
    fn render_last_empty_col(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .w(px(TABLE_TRAILING_GUTTER))
            .h_full()
            .flex_shrink_0()
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) {
        self.move_shown_column(col_ix, to_ix);
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let menu = match self.action_context.clone() {
            Some(handle) => menu.action_context(handle),
            None => menu,
        };
        match self.visible.get(row_ix).cloned() {
            Some(VisibleRow::GroupHeader { key, .. }) => self.group_context_menu(&key, menu),
            Some(VisibleRow::Task(_)) => self.task_context_menu(menu),
            None => menu,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ToolbarCommand {
    Resume,
    Pause,
    PauseAll,
    ResumeAll,
    Delete,
    Open,
    Reveal,
}

/// 选中集合投影（选择条 / 工具栏 / 快捷键）：只统计仍存在于 store 的选中任务。
/// - `count`：选中数量；`any`：是否有选中（删除 / 取消选择）。
/// - `any_local`：含本地任务（打开文件 / 在文件夹中显示；远程任务没有本机文件）。
/// - `any_active`：含下载中 / 排队（暂停）；`any_resumable`：含暂停 / 失败（继续）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SelectionSummary {
    pub(crate) count: usize,
    pub(crate) any: bool,
    pub(crate) any_local: bool,
    pub(crate) any_active: bool,
    pub(crate) any_resumable: bool,
}

/// 行悬停操作；执行时映射为 [`ToolbarCommand`]，与工具栏 / 右键菜单同一命令路径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowAction {
    Pause,
    Resume,
    /// 失败任务的「继续」，图标换成重试。
    Retry,
    Open,
    Reveal,
}

impl RowAction {
    fn command(self) -> ToolbarCommand {
        match self {
            Self::Pause => ToolbarCommand::Pause,
            Self::Resume | Self::Retry => ToolbarCommand::Resume,
            Self::Open => ToolbarCommand::Open,
            Self::Reveal => ToolbarCommand::Reveal,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Pause => "download-row-action-pause",
            Self::Resume => "download-row-action-resume",
            Self::Retry => "download-row-action-retry",
            Self::Open => "download-row-action-open",
            Self::Reveal => "download-row-action-reveal",
        }
    }
}

/// 行悬停操作集合：下载中 / 排队 → 暂停；暂停 → 继续；失败 → 重试；完成 → 打开
/// 文件（仅本地）；本地任务再加「在文件夹中显示」。远程任务只给可用操作。
pub(crate) fn row_actions(state: TaskState, is_local: bool) -> impl Iterator<Item = RowAction> {
    let primary = match state {
        TaskState::Downloading | TaskState::Pending => Some(RowAction::Pause),
        TaskState::Paused => Some(RowAction::Resume),
        TaskState::Failed => Some(RowAction::Retry),
        TaskState::Completed => is_local.then_some(RowAction::Open),
    };
    primary
        .into_iter()
        .chain(is_local.then_some(RowAction::Reveal))
}

/// 单任务命令：远程任务只有暂停 / 继续 / 删除经 `agent.remote.command` 转发，
/// 打开 / 显示没有本机文件（返回 `None`）；全局命令不属于单任务。
fn task_command(key: &RowKey, action: ToolbarCommand) -> Option<DownloadsCommand> {
    let task_id = key.task_id().to_owned();
    if !key.is_local() {
        let remote_action = match action {
            ToolbarCommand::Resume => "resume",
            ToolbarCommand::Pause => "pause",
            ToolbarCommand::Delete => "delete",
            ToolbarCommand::Open
            | ToolbarCommand::Reveal
            | ToolbarCommand::PauseAll
            | ToolbarCommand::ResumeAll => return None,
        };
        return Some(DownloadsCommand::RemoteCommand(serde_json::json!({
            "taskId": task_id,
            "action": remote_action,
        })));
    }
    Some(match action {
        ToolbarCommand::Resume => DownloadsCommand::Resume { task_id },
        ToolbarCommand::Pause => DownloadsCommand::Pause { task_id },
        ToolbarCommand::Delete => DownloadsCommand::Delete {
            task_id,
            delete_files: false,
        },
        ToolbarCommand::Open => DownloadsCommand::OpenTask { task_id },
        ToolbarCommand::Reveal => DownloadsCommand::RevealTask { task_id },
        ToolbarCommand::PauseAll | ToolbarCommand::ResumeAll => return None,
    })
}

/// 进度条颜色：下载中 primary、失败 destructive、暂停弱化中性、其余三级文字。
/// 主表格与详情窗口共用。
pub(crate) fn progress_bar_color(state: TaskState, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    let colors = &theme.tokens().colors;
    match state {
        TaskState::Downloading => colors.primary,
        TaskState::Paused => colors.muted_foreground.opacity(PAUSED_BAR_ALPHA),
        TaskState::Failed => colors.destructive,
        TaskState::Pending | TaskState::Completed => theme.extended().colors.text_tertiary,
    }
}

/// 文件类型图标：任务表与独立进度窗口共用。
pub(crate) fn kind_icon(kind: TaskKind) -> FluxIcon {
    match kind {
        TaskKind::Video => FluxIcon::FilePlay,
        TaskKind::Audio => FluxIcon::FileMusic,
        TaskKind::Document => FluxIcon::FileText,
        TaskKind::Image => FluxIcon::FileImage,
        TaskKind::Archive => FluxIcon::FileArchive,
        TaskKind::DiskImage => FluxIcon::Disc3,
        TaskKind::Application => FluxIcon::AppWindow,
        TaskKind::Mobile => FluxIcon::Smartphone,
        TaskKind::Other => FluxIcon::File,
    }
}

/// 进度轨道颜色：muted_foreground 的淡化派生。主表格与详情窗口共用。
pub(crate) fn progress_track_color(cx: &App) -> Hsla {
    active_theme(cx)
        .tokens()
        .colors
        .muted_foreground
        .opacity(PROGRESS_TRACK_ALPHA)
}

/// 状态文字色：只有下载中（primary）与失败（destructive）着色，暂停为二级文字，
/// 排队 / 完成退为三级文字。主表格与详情窗口共用。
pub(crate) fn status_color(state: TaskState, cx: &App) -> Hsla {
    let theme = active_theme(cx);
    let colors = &theme.tokens().colors;
    match state {
        TaskState::Downloading => colors.primary,
        TaskState::Failed => colors.destructive,
        TaskState::Paused => colors.muted_foreground,
        TaskState::Pending | TaskState::Completed => theme.extended().colors.text_tertiary,
    }
}

/// 已下 / 总量；总量未知时只给已下。
fn bytes_progress(task: &DownloadTaskView) -> String {
    if task.size_bytes > 0 {
        format!("{} / {}", format_bytes(task.downloaded_bytes), task.size)
    } else {
        format_bytes(task.downloaded_bytes)
    }
}

/// 整数百分比（向下取整，未完成不会显示 100%）；介于 0 与 1% 之间显示 `<1%`。
fn percent_label(progress: f32) -> SharedString {
    let percent = progress.clamp(0., 1.) * 100.;
    if percent > 0. && percent < 1. {
        SharedString::from("<1%")
    } else {
        SharedString::from(format!("{}%", percent.floor() as u32))
    }
}

/// 单行元信息单元格（xs）；数字列右对齐并用等宽数字。
fn meta_cell(text: impl Into<SharedString>, color: Hsla, numeric: bool, cx: &App) -> AnyElement {
    let typography = &active_theme(cx).tokens().typography;
    h_flex()
        .size_full()
        .min_w_0()
        .when(numeric, |this| {
            this.justify_end().font_features(tabular_numbers())
        })
        .text_size(typography.xs.size)
        .line_height(typography.xs.line_height)
        .text_color(color)
        .child(div().min_w_0().truncate().child(text.into()))
        .into_any_element()
}

/// 下载任务表（主窗口与任务组详情共用）：表头 28、行高按密度、表格底色 surface。
///
/// 文件名列吸收剩余宽度：prepaint 测得容器宽后只做比较，确需更新时用
/// `window.defer` 在帧外写入代理并 `refresh`（不在 prepaint 里同步更新实体），
/// 写入后期望值与已应用值一致，下一帧不再触发，避免逐帧重算或抖动。
pub(crate) fn render_download_table(
    id: &'static str,
    table_state: &Entity<TableState<DownloadTableDelegate>>,
    cx: &App,
) -> Stateful<Div> {
    let observed = table_state.clone();
    div()
        .id(id)
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
        .bg(active_theme(cx).tokens().colors.surface)
        .child(
            div().absolute().inset_0().child(
                DataTable::new(table_state)
                    .with_size(Size::Size(px(TABLE_HEADER_HEIGHT)))
                    .stripe(false)
                    .bordered(false)
                    .scrollbar_visible(true, true),
            ),
        )
        .on_prepaint(move |bounds, window, cx| {
            let width = f32::from(bounds.size.width);
            if !observed.read(cx).delegate().viewport_needs_sync(width) {
                return;
            }
            window.defer(cx, move |_, cx| {
                observed.update(cx, |table, cx| {
                    if table.delegate_mut().set_viewport_width(width) {
                        table.refresh(cx);
                    }
                });
            });
        })
}

impl DownloadView {
    /// 选中集合 → 命令列表（远程任务走 `agent.remote.command`）。
    pub(crate) fn commands_for_selection(
        &self,
        action: ToolbarCommand,
        cx: &Context<Self>,
    ) -> Vec<DownloadsCommand> {
        match action {
            ToolbarCommand::PauseAll => vec![DownloadsCommand::PauseAll],
            ToolbarCommand::ResumeAll => vec![DownloadsCommand::ResumeAll],
            _ => self
                .table_state
                .read(cx)
                .delegate()
                .selected_keys()
                .iter()
                .filter_map(|key| task_command(key, action))
                .collect(),
        }
    }

    pub(crate) fn execute_toolbar(&mut self, action: ToolbarCommand, cx: &mut Context<Self>) {
        let commands = self.commands_for_selection(action, cx);
        self.execute_commands(commands, cx);
    }

    /// 逐条执行；任一失败在页面横幅提示。只含一条单任务「继续」/「重新下载」时，成功后
    /// 通知宿主这是一次交互式开始（批量选择不逐个弹进度窗口）。
    pub(crate) fn execute_commands(
        &mut self,
        commands: Vec<DownloadsCommand>,
        cx: &mut Context<Self>,
    ) {
        let interactive_start = match commands.as_slice() {
            [DownloadsCommand::Resume { task_id }] => Some(Some(task_id.clone())),
            [DownloadsCommand::Redownload(request, _)] if !request.start_paused => Some(None),
            _ => None,
        };
        for command in commands {
            let future = self.controller.execute(command);
            let interactive_start = interactive_start.clone();
            cx.spawn(async move |this, cx| {
                let result = future.await;
                let failed = result.is_err();
                let _ = this.update(cx, |this, cx| {
                    this.last_error = failed.then(|| this.strings.action_failed.clone());
                    if let (Ok(result), Some(resumed)) = (&result, interactive_start) {
                        // 继续：原任务 ID；重新下载：响应里的新任务 ID。
                        let started =
                            resumed.map_or_else(|| result.created_task_ids(), |id| vec![id]);
                        this.notify_user_started(&started, cx);
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(crate) fn render_table(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        render_download_table("download-task-table-container", &self.table_state, cx)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, rc::Rc};

    use fluxdown_ui_i18n::{I18nCatalog, I18nError};
    use gpui::Modifiers;

    use super::{
        DownloadColumnKind, DownloadTableDelegate, RowAction, SelectionSummary, VisibleRow,
        percent_label, row_actions,
    };
    use crate::{
        model::{
            DownloadTaskView, RowKey, TaskState, TaskStore,
            view_prefs::{SortDir, ViewGroupBy, ViewPrefs, ViewSortKey},
        },
        strings::DownloadStrings,
    };

    fn delegate(statuses: &[i32]) -> Result<DownloadTableDelegate, I18nError> {
        let catalog = std::sync::Arc::new(I18nCatalog::load_embedded()?);
        let strings = DownloadStrings::from_translator(&catalog.translator("en"));
        let store = Rc::new(TaskStore::default());
        let rows = statuses
            .iter()
            .enumerate()
            .map(|(index, status)| {
                let task =
                    serde_json::from_value::<fluxdown_protocol::TaskDto>(serde_json::json!({
                        "taskId": format!("t{index}"),
                        "url": "https://example.com/file",
                        "fileName": format!("f{index}.bin"),
                        "saveDir": "/tmp",
                        "status": status,
                        "downloadedBytes": 0,
                        "totalBytes": 1,
                        "errorMessage": "",
                        "createdAt": format!("{}", 100 - index),
                        "proxyUrl": "",
                        "queueId": "main",
                        "checksum": ""
                    }))
                    .expect("task");
                DownloadTaskView::local(&task, None, false)
            })
            .collect();
        store.replace_local(rows);
        let mut delegate = DownloadTableDelegate::new(strings, store);
        delegate.refresh_view();
        Ok(delegate)
    }

    #[test]
    fn context_menu_selection_preserves_selected_rows_and_replaces_unselected_rows()
    -> Result<(), I18nError> {
        let mut delegate = delegate(&[2, 2, 2])?;
        delegate
            .selected_tasks
            .extend([RowKey::Local("t0".into()), RowKey::Local("t1".into())]);

        delegate.select_task_for_context_menu(RowKey::Local("t1".into()));
        assert_eq!(
            delegate.selected_tasks,
            HashSet::from([RowKey::Local("t0".into()), RowKey::Local("t1".into())])
        );

        delegate.select_task_for_context_menu(RowKey::Local("t2".into()));
        assert_eq!(
            delegate.selected_tasks,
            HashSet::from([RowKey::Local("t2".into())])
        );
        Ok(())
    }

    #[test]
    fn selection_summary_counts_every_live_selected_task() -> Result<(), I18nError> {
        // t0 已暂停、t1 下载中。
        let mut delegate = delegate(&[2, 1])?;
        assert_eq!(delegate.selection_summary(), SelectionSummary::default());

        // 任务已被删除但选中集合仍残留其 key：不计入。
        delegate.selected_tasks.insert(RowKey::Local("gone".into()));
        assert_eq!(delegate.selection_summary(), SelectionSummary::default());

        delegate.selected_tasks.insert(RowKey::Local("t0".into()));
        assert_eq!(
            delegate.selection_summary(),
            SelectionSummary {
                count: 1,
                any: true,
                any_local: true,
                any_active: false,
                any_resumable: true,
            }
        );

        // 遇到本地任务后仍需继续遍历，否则后续下载中任务的可暂停性会丢失。
        delegate.selected_tasks.insert(RowKey::Local("t1".into()));
        assert_eq!(
            delegate.selection_summary(),
            SelectionSummary {
                count: 2,
                any: true,
                any_local: true,
                any_active: true,
                any_resumable: true,
            }
        );
        Ok(())
    }

    #[test]
    fn selection_is_pruned_to_tasks_left_in_view() -> Result<(), I18nError> {
        let mut delegate = delegate(&[2, 1, 2])?;
        delegate.select_all_tasks();
        assert_eq!(delegate.selection_summary().count, 3);

        // 搜索收窄：被筛掉的任务移出选中集，汇总与批量动作只看剩余可见任务。
        delegate.set_query("f1.bin");
        delegate.refresh_view();
        assert_eq!(delegate.selected_keys(), vec![RowKey::Local("t1".into())]);
        assert_eq!(delegate.selection_summary().count, 1);

        // 列表为空时不得再报告「已选 N 项」。
        delegate.set_query("no-such-task");
        delegate.refresh_view();
        assert_eq!(delegate.selection_summary(), SelectionSummary::default());

        // 清空搜索不会让已移出的任务「复活」为选中。
        delegate.set_query("");
        delegate.refresh_view();
        assert!(delegate.selected_keys().is_empty());
        Ok(())
    }

    #[test]
    fn collapsing_a_group_keeps_its_selected_members() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1, 3])?;
        delegate.set_prefs(ViewPrefs {
            group_by: ViewGroupBy::Status,
            ..ViewPrefs::default()
        });
        delegate.refresh_view();
        delegate.select_all_tasks();
        let key = delegate
            .group_key_at(0)
            .map(str::to_owned)
            .unwrap_or_default();
        delegate.toggle_group_collapsed(&key);
        delegate.refresh_view();
        // 两个分组各 1 项，折叠首组后只剩 2 个组头 + 1 行任务。
        assert_eq!(delegate.visible.len(), 3);
        assert_eq!(delegate.selection_summary().count, 2);
        Ok(())
    }

    #[test]
    fn grouping_inserts_headers_and_collapsing_hides_members() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1, 3, 1, 3])?;
        let mut prefs = ViewPrefs::default();
        prefs.group_by = ViewGroupBy::Status;
        delegate.set_prefs(prefs);
        delegate.refresh_view();
        assert_eq!(delegate.visible.len(), 6);
        assert!(matches!(
            delegate.visible[0],
            VisibleRow::GroupHeader {
                count: 2,
                collapsed: false,
                ..
            }
        ));

        let key = delegate
            .group_key_at(0)
            .map(str::to_owned)
            .expect("group key");
        delegate.toggle_group_collapsed(&key);
        delegate.refresh_view();
        assert_eq!(delegate.visible.len(), 4);
        assert!(matches!(
            delegate.visible[0],
            VisibleRow::GroupHeader {
                collapsed: true,
                ..
            }
        ));
        Ok(())
    }

    #[test]
    fn shift_range_selection_skips_group_headers() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1, 3, 1, 3])?;
        let mut prefs = ViewPrefs::default();
        prefs.group_by = ViewGroupBy::Status;
        delegate.set_prefs(prefs);
        delegate.refresh_view();
        let first = delegate.row_key_at(1).expect("first task");
        let last = delegate.row_key_at(5).expect("last task");
        delegate.select_task(first, Modifiers::default());
        delegate.select_task(
            last,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        assert_eq!(delegate.selected_tasks.len(), 4);
        Ok(())
    }

    fn width_of(delegate: &DownloadTableDelegate, kind: DownloadColumnKind) -> Option<f32> {
        delegate
            .columns
            .iter()
            .find(|column| column.kind == kind)
            .map(|column| column.width)
    }

    #[test]
    fn sync_column_widths_maps_shown_columns_and_clamps() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1])?;
        let _ = delegate.set_viewport_width(1400.);
        let applied = delegate.apply_file_name_width();
        let shown: Vec<DownloadColumnKind> = delegate
            .columns
            .iter()
            .filter(|column| column.visible)
            .map(|column| column.kind)
            .collect();
        let widths_with = |file_name: f32| -> Vec<gpui::Pixels> {
            // 首项是勾选列；大小列拖宽、状态列拖到下限以下。
            std::iter::once(gpui::px(36.))
                .chain(shown.iter().map(|kind| match kind {
                    DownloadColumnKind::Size => gpui::px(180.),
                    DownloadColumnKind::Status => gpui::px(10.),
                    DownloadColumnKind::FileName => gpui::px(file_name),
                    other => gpui::px(other.default_width()),
                }))
                .collect()
        };
        // 文件名列宽等于已应用的回算值：没被拖，保持自适应。
        let widths = widths_with(applied);
        assert!(delegate.sync_column_widths(&widths));
        assert_eq!(width_of(&delegate, DownloadColumnKind::Size), Some(180.));
        assert_eq!(
            width_of(&delegate, DownloadColumnKind::Status),
            Some(DownloadColumnKind::Status.min_width())
        );
        assert_eq!(delegate.prefs.file_name_width, None);
        // 隐藏列不参与映射，宽度不变。
        assert_eq!(
            width_of(&delegate, DownloadColumnKind::Speed),
            Some(DownloadColumnKind::Speed.default_width())
        );
        assert!(!delegate.sync_column_widths(&widths));

        // 文件名列被拖（宽度偏离回算值）→ 固定，不再随容器变化；过窄钳到下限。
        assert!(delegate.sync_column_widths(&widths_with(applied + 120.)));
        assert_eq!(delegate.prefs.file_name_width, Some(applied + 120.));
        assert_eq!(delegate.file_name_width_for(900.), applied + 120.);
        assert!(!delegate.viewport_needs_sync(1400.));
        assert!(delegate.sync_column_widths(&widths_with(10.)));
        assert_eq!(
            delegate.file_name_width_for(1400.),
            DownloadColumnKind::FileName.min_width()
        );

        // 重置列恢复自适应。
        delegate.reset_columns();
        assert_eq!(delegate.prefs.file_name_width, None);
        Ok(())
    }

    #[test]
    fn file_name_absorbs_remaining_width_and_respects_min() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1])?;
        let wide = delegate.file_name_width_for(1400.);
        let narrower = delegate.file_name_width_for(1300.);
        assert!((wide - narrower - 100.).abs() < 0.01);

        // 其他列拖宽 50 → 文件名列让出 50。
        let size = width_of(&delegate, DownloadColumnKind::Size).unwrap_or_default();
        if let Some(column) = delegate
            .columns
            .iter_mut()
            .find(|column| column.kind == DownloadColumnKind::Size)
        {
            column.width = size + 50.;
        }
        assert!((wide - delegate.file_name_width_for(1400.) - 50.).abs() < 0.01);

        // 容器过窄时不低于最小宽度（超出部分走横向滚动）。
        assert_eq!(
            delegate.file_name_width_for(300.),
            DownloadColumnKind::FileName.min_width()
        );

        // 容器宽写入后一次 refresh 即收敛：应用新宽度后不再报告需要同步。
        assert!(delegate.set_viewport_width(1400.));
        let _ = delegate.apply_file_name_width();
        assert!(!delegate.viewport_needs_sync(1400.));
        Ok(())
    }

    #[test]
    fn header_click_flips_current_key_and_switches_to_column_default() -> Result<(), I18nError> {
        let mut delegate = delegate(&[1])?;
        assert_eq!(delegate.prefs.sort_key, ViewSortKey::Smart);

        assert!(delegate.toggle_sort(DownloadColumnKind::FileName));
        assert_eq!(
            (delegate.prefs.sort_key, delegate.prefs.sort_dir),
            (ViewSortKey::Name, SortDir::Asc)
        );
        assert!(delegate.toggle_sort(DownloadColumnKind::FileName));
        assert_eq!(delegate.prefs.sort_dir, SortDir::Desc);

        assert!(delegate.toggle_sort(DownloadColumnKind::Size));
        assert_eq!(
            (delegate.prefs.sort_key, delegate.prefs.sort_dir),
            (ViewSortKey::Size, SortDir::Desc)
        );

        // 状态列回到智能排序；智能排序无方向，再点不变。
        assert!(delegate.toggle_sort(DownloadColumnKind::Status));
        assert_eq!(delegate.prefs.sort_key, ViewSortKey::Smart);
        assert!(!delegate.toggle_sort(DownloadColumnKind::Status));
        assert!(!delegate.toggle_sort(DownloadColumnKind::Protocol));
        Ok(())
    }

    #[test]
    fn row_actions_offer_only_available_operations() {
        let actions = |state, local| row_actions(state, local).collect::<Vec<_>>();
        assert_eq!(
            actions(TaskState::Downloading, true),
            [RowAction::Pause, RowAction::Reveal]
        );
        assert_eq!(actions(TaskState::Pending, false), [RowAction::Pause]);
        assert_eq!(
            actions(TaskState::Failed, true),
            [RowAction::Retry, RowAction::Reveal]
        );
        assert_eq!(
            actions(TaskState::Completed, true),
            [RowAction::Open, RowAction::Reveal]
        );
        // 远程已完成任务没有本机文件：无任何行操作。
        assert!(actions(TaskState::Completed, false).is_empty());
    }

    #[test]
    fn percent_label_floors_and_marks_sub_percent_progress() {
        assert_eq!(percent_label(0.).as_ref(), "0%");
        assert_eq!(percent_label(0.004).as_ref(), "<1%");
        assert_eq!(percent_label(0.01).as_ref(), "1%");
        assert_eq!(percent_label(0.999).as_ref(), "99%");
        assert_eq!(percent_label(1.).as_ref(), "100%");
    }
}

#[cfg(test)]
mod context_menu_tests {
    use super::{MenuEntry, PLUGIN_ERROR_PREFIX, TaskMenuFacts, context_menu_items};
    use crate::model::TaskState;

    fn task(is_local: bool, state: TaskState) -> TaskMenuFacts {
        TaskMenuFacts {
            is_local,
            state,
            boosted: false,
            is_torrent_sentinel: false,
            is_plugin_retry_error: false,
        }
    }

    #[test]
    fn empty_selection_has_no_menu_items() {
        assert!(context_menu_items(&[]).is_empty());
    }

    #[test]
    fn single_local_completed_task_shows_full_menu() {
        let items = context_menu_items(&[task(true, TaskState::Completed)]);
        assert_eq!(
            items,
            vec![
                MenuEntry::Boost,
                MenuEntry::OpenFile,
                MenuEntry::OpenFolder,
                MenuEntry::Rename,
                MenuEntry::Redownload,
                MenuEntry::CopyUrl,
                MenuEntry::MoveToQueue,
                MenuEntry::Separator,
                MenuEntry::Delete,
                MenuEntry::DeleteWithFiles,
                MenuEntry::OpenInWindow,
            ]
        );
    }

    #[test]
    fn multi_select_only_keeps_items_valid_for_every_selected_task() {
        // 一个本地已完成 + 一个远程下载中：单选专属项（重命名/加速/忽略插件重试）
        // 消失；要求全体本地的项（打开文件/目录/移动队列/重下载）因远程任务
        // 不满足而消失；resume 因两者都不满足「非完成非下载中」而消失；pause
        // 因远程任务处于下载中而显示（存在语义）；复制链接/删除类始终显示；
        // 「独立窗口打开」因存在本地任务而显示。
        let selection = [
            task(true, TaskState::Completed),
            task(false, TaskState::Downloading),
        ];
        let items = context_menu_items(&selection);
        assert_eq!(
            items,
            vec![
                MenuEntry::Pause,
                MenuEntry::CopyUrl,
                MenuEntry::Separator,
                MenuEntry::Delete,
                MenuEntry::DeleteWithFiles,
                MenuEntry::OpenInWindow,
            ]
        );
    }

    #[test]
    fn multi_select_paused_and_failed_shows_resume_but_not_pause() {
        let selection = [task(true, TaskState::Paused), task(true, TaskState::Failed)];
        let items = context_menu_items(&selection);
        assert!(items.contains(&MenuEntry::Resume));
        assert!(!items.contains(&MenuEntry::Pause));
    }

    #[test]
    fn ignore_plugin_retry_only_for_single_failed_plugin_task() {
        let mut plugin_failure = task(true, TaskState::Failed);
        plugin_failure.is_plugin_retry_error = true;
        assert!(context_menu_items(&[plugin_failure]).contains(&MenuEntry::IgnorePluginRetry));

        // 双选即使都满足条件也不显示（单选专属）。
        assert!(
            !context_menu_items(&[plugin_failure, plugin_failure])
                .contains(&MenuEntry::IgnorePluginRetry)
        );

        // 失败但错误消息不带插件前缀不显示。
        let plain_failure = task(true, TaskState::Failed);
        assert!(!context_menu_items(&[plain_failure]).contains(&MenuEntry::IgnorePluginRetry));
        assert_eq!(PLUGIN_ERROR_PREFIX, "[插件]");
    }

    #[test]
    fn boost_toggles_label_by_boosted_state() {
        let mut boosted = task(true, TaskState::Downloading);
        boosted.boosted = true;
        assert!(context_menu_items(&[boosted]).contains(&MenuEntry::CancelBoost));

        let not_boosted = task(true, TaskState::Downloading);
        assert!(context_menu_items(&[not_boosted]).contains(&MenuEntry::Boost));
    }

    #[test]
    fn torrent_sentinel_and_remote_tasks_cannot_redownload() {
        let mut sentinel = task(true, TaskState::Completed);
        sentinel.is_torrent_sentinel = true;
        assert!(!context_menu_items(&[sentinel]).contains(&MenuEntry::Redownload));

        let remote = task(false, TaskState::Completed);
        assert!(!context_menu_items(&[remote]).contains(&MenuEntry::Redownload));
    }
}
