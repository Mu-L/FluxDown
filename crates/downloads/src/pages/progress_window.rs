//! 独立下载进度窗口：用户开始的下载一个任务一个窗口，下载中显示进度视图，完成后同一窗口
//! 切换为「下载完成」视图。
//!
//! 由 app 经 `session::attach` 驱动（[`SessionConsumer`] 三个同名 `pub fn`），自带私有
//! [`DownloadsController`] 维护任务状态。窗口高度随内容：内容区按自然高度排版，每帧预绘制时
//! 与视口比较，不一致即请求窗口改高（进度 / 完成视图切换、展开分段列表时自动跟随）。
//! 开窗与关窗策略由宿主决定（见 [`crate::ProgressWindowTracker`]），本视图只在任务被删除、
//! 用户点「停止」或打开文件 / 文件夹后发出 [`ProgressWindowEvent::Close`]。

use std::sync::Arc;

use fluxdown_protocol::{AgentEvent, AgentSnapshot, DaemonEvent, ServiceEvent, TaskRuntimeDto};
use fluxdown_ui_components::{ControlExt as _, FluxIcon, card, check_row, tabular_numbers};
use fluxdown_ui_i18n::Translator;
use fluxdown_ui_theme::active_theme;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement as _, Styled, Window,
    canvas, div, prelude::FluentBuilder as _, px, size,
};
use gpui_component::{
    Disableable as _, Icon,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::{
    components::{
        segment_progress::render_segment_progress,
        task_table::{kind_icon, progress_bar_color, progress_track_color, status_color},
    },
    controller::{DownloadsCommand, DownloadsController, DownloadsPort},
    model::{DownloadTaskView, RowKey, TaskProtocol, TaskState, format_bytes},
    strings::DownloadStrings,
};

/// 窗口宽度（逻辑像素）；进度与完成视图共用，切换时只改高度。
pub const PROGRESS_WINDOW_WIDTH: f32 = 480.;
/// 首帧前的窗口高度估计（首帧预绘制后按内容校正）。
pub const PROGRESS_WINDOW_INITIAL_HEIGHT: f32 = 340.;

/// 进度条高度：比任务表更醒目。
const BAR_HEIGHT: f32 = 8.;
/// 文件类型图标底块边长。
const ICON_TILE: f32 = 40.;
/// 完成视图的状态角标边长。
const BADGE: f32 = 18.;
/// 分段列表最大高度，超出后内部滚动。
const PARTS_MAX_HEIGHT: f32 = 168.;
/// 分段列表行高。
const PART_ROW_HEIGHT: f32 = 24.;
/// 分段序号列宽。
const PART_INDEX_WIDTH: f32 = 36.;
/// 信息卡键值行高与标签列宽（标签为「路径」「地址」这类短词）。
const INFO_ROW_HEIGHT: f32 = 22.;
const INFO_LABEL_WIDTH: f32 = 44.;
/// 视口与内容高度的容差：小于此差值不改窗口，避免亚像素抖动。
const RESIZE_EPSILON: f32 = 1.;
/// 超过此值的剩余时间不可信，不显示。
const MAX_ETA_SECS: u64 = 86_400;

/// 视图对宿主的事件。
pub enum ProgressWindowEvent {
    /// 关闭承载窗口（任务已删除 / 用户停止）。
    Close,
    /// 已把文件 / 所在文件夹交给系统打开：宿主应关窗，但要等打开的程序接管前台后再关，
    /// 否则关闭本应用的前台窗口会让系统把主窗口提到前面。
    HandedOff,
    /// 用户切换本窗口的「完成后显示完成窗口」。
    ShowCompletionChanged(bool),
}

/// 命令成功后的窗口去留。
#[derive(Clone, Copy)]
enum AfterSuccess {
    /// 立即关窗（停止）。
    Close,
    /// 交给系统程序后关窗（打开文件 / 文件夹）。
    HandOff,
}

pub struct ProgressWindowView {
    task_id: String,
    controller: DownloadsController,
    port: Arc<dyn DownloadsPort>,
    translator: Entity<Translator>,
    strings: DownloadStrings,
    runtime: Option<TaskRuntimeDto>,
    show_completion: bool,
    parts_expanded: bool,
    closed: bool,
    last_error: Option<SharedString>,
}

impl EventEmitter<ProgressWindowEvent> for ProgressWindowView {}

impl ProgressWindowView {
    /// `show_completion` 为宿主按全局设置与窗口覆盖值算出的初值。
    pub fn new(
        translator: Entity<Translator>,
        task_id: String,
        port: Arc<dyn DownloadsPort>,
        show_completion: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let strings = DownloadStrings::from_translator(translator.read(cx));
        cx.observe(&translator, |this, translator, cx| {
            this.strings = DownloadStrings::from_translator(translator.read(cx));
            cx.notify();
        })
        .detach();
        Self {
            task_id,
            controller: DownloadsController::new(Arc::clone(&port)),
            port,
            translator,
            strings,
            runtime: None,
            show_completion,
            parts_expanded: false,
            closed: false,
            last_error: None,
        }
    }

    #[must_use]
    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    /// 窗口标题：下载中带百分比（任务栏 / 窗口切换器里可直接看进度）。
    #[must_use]
    pub fn title(&self) -> Option<String> {
        let row = self.row()?;
        if row.name.is_empty() {
            return None;
        }
        Some(
            if row.state == TaskState::Completed || row.size_bytes == 0 {
                row.name.clone()
            } else {
                format!("{} {}", row.progress_label, row.name)
            },
        )
    }

    pub fn set_show_completion(&mut self, value: bool, cx: &mut Context<Self>) {
        if self.show_completion != value {
            self.show_completion = value;
            cx.notify();
        }
    }

    fn row(&self) -> Option<std::cell::Ref<'_, DownloadTaskView>> {
        self.controller
            .store()
            .get(&RowKey::Local(self.task_id.clone()))
    }

    fn t(&self, cx: &App, key: &str) -> SharedString {
        SharedString::from(self.translator.read(cx).text(key).to_owned())
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.closed {
            self.closed = true;
            cx.emit(ProgressWindowEvent::Close);
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.runtime = self.controller.task_runtime(&self.task_id).cloned();
        if self.row().is_none() {
            self.close(cx);
            return;
        }
        cx.notify();
    }

    // ---- SessionConsumer（app 经 `attach` 驱动）----

    pub fn replace_snapshot(&mut self, snapshot: &AgentSnapshot, cx: &mut Context<Self>) {
        self.controller.replace_snapshot(snapshot);
        self.last_error = (!snapshot.daemon_connected).then(|| self.strings.disconnected.clone());
        self.refresh(cx);
    }

    pub fn apply_event(&mut self, event: &ServiceEvent, cx: &mut Context<Self>) {
        if let ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskDeleted { task_id })) = event
            && *task_id == self.task_id
        {
            self.close(cx);
            return;
        }
        if let ServiceEvent::Agent(AgentEvent::DaemonConnectionChanged(connected)) = event {
            self.last_error = (!connected).then(|| self.strings.disconnected.clone());
        }
        if self.controller.apply_event(event) {
            self.refresh(cx);
        }
    }

    pub fn mark_stale(&mut self, cx: &mut Context<Self>) {
        self.controller.mark_stale();
        self.runtime = None;
        self.last_error = Some(self.strings.disconnected.clone());
        cx.notify();
    }

    // ---- 动作 ----

    /// 执行命令；成功后按 `then` 发出关窗类事件（`None` = 保持窗口）。
    fn run(
        &mut self,
        command: DownloadsCommand,
        then: Option<AfterSuccess>,
        cx: &mut Context<Self>,
    ) {
        let future = self.port.execute(command);
        cx.spawn(async move |this, cx| {
            let failed = future.await.is_err();
            let _ = this.update(cx, |this, cx| {
                if failed {
                    this.last_error = Some(this.strings.action_failed.clone());
                    cx.notify();
                    return;
                }
                match then {
                    Some(AfterSuccess::Close) => this.close(cx),
                    Some(AfterSuccess::HandOff) if !this.closed => {
                        this.closed = true;
                        cx.emit(ProgressWindowEvent::HandedOff);
                    }
                    Some(AfterSuccess::HandOff) | None => {}
                }
            });
        })
        .detach();
    }

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self
            .row()
            .map(|row| matches!(row.state, TaskState::Downloading | TaskState::Pending))
        else {
            return;
        };
        let task_id = self.task_id.clone();
        let command = if active {
            DownloadsCommand::Pause { task_id }
        } else {
            DownloadsCommand::Resume { task_id }
        };
        self.run(command, None, cx);
    }

    /// 停止：暂停任务（保留进度，可稍后继续）并关窗。
    fn stop(&mut self, cx: &mut Context<Self>) {
        let task_id = self.task_id.clone();
        self.run(
            DownloadsCommand::Pause { task_id },
            Some(AfterSuccess::Close),
            cx,
        );
    }

    fn open_file(&mut self, cx: &mut Context<Self>) {
        let task_id = self.task_id.clone();
        self.run(
            DownloadsCommand::OpenTask { task_id },
            Some(AfterSuccess::HandOff),
            cx,
        );
    }

    fn reveal(&mut self, cx: &mut Context<Self>) {
        let task_id = self.task_id.clone();
        self.run(
            DownloadsCommand::RevealTask { task_id },
            Some(AfterSuccess::HandOff),
            cx,
        );
    }

    fn toggle_show_completion(&mut self, value: bool, cx: &mut Context<Self>) {
        self.show_completion = value;
        cx.emit(ProgressWindowEvent::ShowCompletionChanged(value));
        cx.notify();
    }

    // ---- 渲染 ----

    fn render_icon_tile(kind_icon: FluxIcon, cx: &App) -> gpui::Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended();
        div()
            .flex_none()
            .size(px(ICON_TILE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(tokens.radius.lg)
            .bg(extended.colors.nav_hover)
            .child(
                Icon::new(kind_icon)
                    .size(extended.icon.lg)
                    .text_color(tokens.colors.muted_foreground),
            )
    }

    fn render_title(name: SharedString, cx: &App) -> gpui::Div {
        let title = active_theme(cx).extended().title;
        div()
            .min_w_0()
            .truncate()
            .text_size(title.size)
            .line_height(title.line_height)
            .font_weight(title.weight)
            .child(name)
    }

    fn render_progress(
        &self,
        row: &DownloadTaskView,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let downloading = row.state == TaskState::Downloading;
        let is_bt = row.protocol == TaskProtocol::Bt;

        let size_label = if row.size_bytes > 0 {
            format!("{} / {}", format_bytes(row.downloaded_bytes), row.size)
        } else {
            format_bytes(row.downloaded_bytes)
        };
        let meta = SharedString::from(format!("{} · {size_label}", row.protocol.label()));
        let speed = row
            .speed_bytes_per_second
            .filter(|speed| downloading && *speed > 0)
            .map(|speed| format!("{}/s", format_bytes(speed)));
        let eta = row
            .eta_seconds
            .filter(|seconds| downloading && *seconds <= MAX_ETA_SECS)
            .map(|seconds| self.strings.format_eta(seconds));
        let active_transfers = row.active_transfers().filter(|_| downloading).map(|count| {
            self.runtime
                .as_ref()
                .and_then(|runtime| runtime.parallelism_limit)
                .map_or_else(|| count.to_string(), |limit| format!("{count} / {limit}"))
        });
        let peers = self
            .runtime
            .as_ref()
            .filter(|_| downloading && row.runtime_connected && is_bt)
            .and_then(|runtime| runtime.connected_peers);
        let segments = self
            .runtime
            .as_ref()
            .filter(|runtime| !is_bt && runtime.segments.len() > 1)
            .map(|runtime| runtime.segments.len());
        let active = matches!(row.state, TaskState::Downloading | TaskState::Pending);
        let bar_width =
            (f32::from(window.viewport_size().width) - 2. * f32::from(tokens.spacing.lg)).max(0.);

        let stat = |label: SharedString, value: SharedString, emphasize: bool| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap(tokens.spacing.xxs)
                .child(
                    div()
                        .text_size(extended.caption.size)
                        .line_height(extended.caption.line_height)
                        .text_color(extended.colors.text_tertiary)
                        .child(label),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(tokens.typography.sm.size)
                        .line_height(tokens.typography.sm.line_height)
                        .font_weight(FontWeight::MEDIUM)
                        .font_features(tabular_numbers())
                        .text_color(if emphasize {
                            tokens.colors.primary
                        } else {
                            tokens.colors.foreground
                        })
                        .child(value),
                )
        };
        let dash = SharedString::from("—");
        let connections = if is_bt {
            (
                self.t(cx, "detailConnectedPeers"),
                peers.map_or_else(|| dash.clone(), |count| count.to_string().into()),
            )
        } else {
            (
                self.t(cx, "detailActiveTransfers"),
                active_transfers.map_or_else(|| dash.clone(), SharedString::from),
            )
        };

        v_flex()
            .child(
                h_flex()
                    .gap(tokens.spacing.md)
                    .px(tokens.spacing.lg)
                    .pt(tokens.spacing.lg)
                    .child(Self::render_icon_tile(kind_icon(row.kind), cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(tokens.spacing.xxs)
                            .child(Self::render_title(SharedString::from(row.name.clone()), cx))
                            .child(
                                h_flex()
                                    .gap(tokens.spacing.xs)
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .font_features(tabular_numbers())
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(status_color(row.state, cx))
                                            .child(self.strings.state_label(row.state)),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(extended.colors.text_tertiary)
                                            .child(meta),
                                    ),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap(tokens.spacing.xs)
                    .px(tokens.spacing.lg)
                    .pt(tokens.spacing.lg)
                    .child(div().rounded(tokens.radius.sm).overflow_hidden().child(
                        render_segment_progress(
                            self.runtime.as_ref(),
                            row.progress,
                            bar_width,
                            BAR_HEIGHT,
                            progress_bar_color(row.state, cx),
                            progress_track_color(cx),
                        ),
                    ))
                    .child(
                        h_flex()
                            .justify_between()
                            .text_size(tokens.typography.xs.size)
                            .line_height(tokens.typography.xs.line_height)
                            .font_features(tabular_numbers())
                            .text_color(tokens.colors.muted_foreground)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(tokens.colors.foreground)
                                    .child(SharedString::from(row.progress_label.clone())),
                            )
                            .when_some(segments, |this, count| {
                                let count = count.to_string();
                                this.child(
                                    div().child(SharedString::from(
                                        self.translator
                                            .read(cx)
                                            .text_with("progressWindowSegments", &[("n", &count)]),
                                    )),
                                )
                            }),
                    ),
            )
            .child(
                div().px(tokens.spacing.lg).pt(tokens.spacing.md).child(
                    card(cx)
                        .px(tokens.spacing.md)
                        .py(tokens.spacing.sm)
                        .child(
                            h_flex()
                                .gap(tokens.spacing.md)
                                .pb(tokens.spacing.sm)
                                .child(stat(
                                    self.t(cx, "infoSpeed"),
                                    speed.map_or_else(|| dash.clone(), SharedString::from),
                                    downloading,
                                ))
                                .child(stat(
                                    self.t(cx, "infoRemaining"),
                                    eta.unwrap_or_else(|| dash.clone()),
                                    false,
                                ))
                                .child(stat(connections.0, connections.1, false)),
                        )
                        .child(div().h(px(1.)).bg(extended.colors.hairline))
                        .child(
                            v_flex()
                                .pt(tokens.spacing.xs)
                                .child(Self::info_row(
                                    self.t(cx, "infoPath"),
                                    SharedString::from(row.save_dir.clone()),
                                    cx,
                                ))
                                .child(Self::info_row(
                                    self.t(cx, "infoUrl"),
                                    SharedString::from(row.share_url().to_owned()),
                                    cx,
                                )),
                        ),
                ),
            )
            .when(segments.is_some(), |this| this.child(self.render_parts(cx)))
            .when(
                row.state == TaskState::Failed && !row.error_message.is_empty(),
                |this| {
                    this.child(Self::render_error(
                        SharedString::from(row.error_message.clone()),
                        cx,
                    ))
                },
            )
            .when_some(self.last_error.clone(), |this, error| {
                this.child(Self::render_error(error, cx))
            })
            .child(self.render_progress_footer(active, cx))
            .into_any_element()
    }

    /// 信息卡的单行键值：窄标签列 + 单行截断的值（路径 / 链接）。
    fn info_row(label: SharedString, value: SharedString, cx: &App) -> gpui::Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        h_flex()
            .gap(tokens.spacing.sm)
            .h(px(INFO_ROW_HEIGHT))
            .text_size(tokens.typography.xs.size)
            .line_height(tokens.typography.xs.line_height)
            .child(
                div()
                    .flex_none()
                    .w(px(INFO_LABEL_WIDTH))
                    .text_color(theme.extended().colors.text_tertiary)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(tokens.colors.muted_foreground)
                    .child(value),
            )
    }

    fn render_error(message: SharedString, cx: &App) -> gpui::Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        h_flex()
            .items_start()
            .gap(tokens.spacing.xs)
            .px(tokens.spacing.lg)
            .pt(tokens.spacing.sm)
            .text_size(tokens.typography.xs.size)
            .line_height(tokens.typography.xs.line_height)
            .text_color(tokens.colors.destructive)
            .child(
                div()
                    .flex_none()
                    .h(tokens.typography.xs.line_height)
                    .flex()
                    .items_center()
                    .child(Icon::new(FluxIcon::CircleAlert).size(theme.extended().icon.sm)),
            )
            .child(div().min_w_0().flex_1().line_clamp(2).child(message))
    }

    /// 分段列表：折叠开关 + 每段状态 / 已下载 / 大小 / 行内进度。
    fn render_parts(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let expanded = self.parts_expanded;
        let label = if expanded {
            self.t(cx, "progressWindowPartsHide")
        } else {
            self.t(cx, "progressWindowPartsShow")
        };
        let toggle = h_flex()
            .id("progress-parts-toggle")
            .flex_none()
            .gap(tokens.spacing.xxs)
            .px(tokens.spacing.xs)
            .h(px(24.))
            .rounded(tokens.radius.md)
            .cursor_pointer()
            .text_size(tokens.typography.xs.size)
            .text_color(tokens.colors.muted_foreground)
            .hover(move |style| style.bg(extended.colors.row_hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.parts_expanded = !this.parts_expanded;
                cx.notify();
            }))
            .child(
                Icon::new(if expanded {
                    FluxIcon::ChevronDown
                } else {
                    FluxIcon::ChevronRight
                })
                .size(extended.icon.sm),
            )
            .child(label);

        let mut section = v_flex()
            .px(tokens.spacing.lg)
            .pt(tokens.spacing.sm)
            .gap(tokens.spacing.xs)
            .child(h_flex().child(toggle));
        if expanded {
            section = section.child(self.render_parts_table(cx));
        }
        section.into_any_element()
    }

    fn render_parts_table(&self, cx: &App) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let running = self
            .row()
            .is_some_and(|row| row.state == TaskState::Downloading);
        let mut segments = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.segments.clone())
            .unwrap_or_default();
        segments.sort_by_key(|segment| segment.index);

        let columns =
            |index: AnyElement, status: AnyElement, done: AnyElement, total: AnyElement| {
                h_flex()
                    .gap(tokens.spacing.sm)
                    .child(div().flex_none().w(px(PART_INDEX_WIDTH)).child(index))
                    .child(div().flex_1().min_w_0().child(status))
                    .child(div().flex_1().min_w_0().child(done))
                    .child(div().w(px(72.)).flex_none().text_right().child(total))
            };
        let header_text = |text: SharedString| {
            div()
                .text_size(extended.caption.size)
                .line_height(extended.caption.line_height)
                .text_color(extended.colors.text_tertiary)
                .child(text)
                .into_any_element()
        };

        let rows = segments.into_iter().map(|segment| {
            let size = (segment.end_byte - segment.start_byte + 1).max(0) as u64;
            let downloaded = segment.downloaded_bytes.max(0) as u64;
            let fraction = if size > 0 {
                (downloaded as f32 / size as f32).clamp(0., 1.)
            } else {
                0.
            };
            let status_key = if size > 0 && downloaded >= size {
                "statusCompleted"
            } else if !running {
                "statusPaused"
            } else if segment.active == Some(true) {
                "statusDownloading"
            } else {
                "progressWindowPartWaiting"
            };
            let active = segment.active == Some(true) && running;
            let cell = |text: SharedString| {
                div()
                    .truncate()
                    .text_size(tokens.typography.xs.size)
                    .font_features(tabular_numbers())
                    .text_color(tokens.colors.muted_foreground)
                    .child(text)
                    .into_any_element()
            };
            div()
                .h(px(PART_ROW_HEIGHT))
                .flex()
                .items_center()
                .px(tokens.spacing.md)
                .child(columns(
                    cell(SharedString::from(format!("#{}", segment.index + 1))),
                    div()
                        .truncate()
                        .text_size(tokens.typography.xs.size)
                        .text_color(if active {
                            tokens.colors.primary
                        } else {
                            tokens.colors.muted_foreground
                        })
                        .child(self.t(cx, status_key))
                        .into_any_element(),
                    h_flex()
                        .gap(tokens.spacing.sm)
                        .child(
                            div()
                                .flex_none()
                                .child(cell(SharedString::from(format_bytes(downloaded)))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .h(px(3.))
                                .rounded(px(1.5))
                                .bg(progress_track_color(cx))
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(fraction))
                                        .rounded(px(1.5))
                                        .bg(if active {
                                            tokens.colors.primary
                                        } else {
                                            extended.colors.text_tertiary
                                        }),
                                ),
                        )
                        .into_any_element(),
                    cell(SharedString::from(format_bytes(size))),
                ))
        });

        card(cx)
            .child(
                div()
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.xs)
                    .border_b_1()
                    .border_color(extended.colors.hairline)
                    .child(columns(
                        header_text(SharedString::from("#")),
                        header_text(self.t(cx, "infoStatus")),
                        header_text(self.t(cx, "infoDownloaded")),
                        header_text(self.t(cx, "infoSize")),
                    )),
            )
            .child(
                div()
                    .id("progress-parts-list")
                    .max_h(px(PARTS_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .py(tokens.spacing.xxs)
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_progress_footer(&self, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let this = cx.weak_entity();
        Self::footer(cx)
            .child(
                div().flex_1().min_w_0().child(check_row(
                    "progress-show-completion",
                    self.show_completion,
                    div()
                        .truncate()
                        .text_size(tokens.typography.xs.size)
                        .text_color(tokens.colors.muted_foreground)
                        .child(self.t(cx, "progressWindowShowCompletion")),
                    move |value, _, cx| {
                        let _ = this.update(cx, |this, cx| this.toggle_show_completion(value, cx));
                    },
                    cx,
                )),
            )
            .child(
                Button::new("progress-stop")
                    .outline()
                    .control(cx)
                    .label(self.t(cx, "progressWindowStop"))
                    .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            )
            .child(
                Button::new("progress-toggle-pause")
                    .primary()
                    .control(cx)
                    .icon(if active {
                        FluxIcon::Pause
                    } else {
                        FluxIcon::Play
                    })
                    .label(if active {
                        self.strings.pause.clone()
                    } else {
                        self.strings.resume.clone()
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_pause(cx))),
            )
            .into_any_element()
    }

    fn footer(cx: &App) -> gpui::Div {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        h_flex()
            .mt(tokens.spacing.lg)
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.lg)
            .py(tokens.spacing.md)
            .border_t_1()
            .border_color(theme.extended().colors.hairline)
            .bg(theme.extended().colors.chrome)
    }

    fn render_completed(&self, row: &DownloadTaskView, cx: &mut Context<Self>) -> AnyElement {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let extended = theme.extended().clone();
        let missing = row.file_missing;
        let size = if row.size_bytes > 0 {
            row.size.clone()
        } else {
            format_bytes(row.downloaded_bytes)
        };
        let elapsed = row.completed_at_secs - row.created_at_secs;
        let meta = if elapsed > 0 {
            format!(
                "{size} · {} {}",
                self.t(cx, "infoDuration"),
                format_elapsed(elapsed as u64)
            )
        } else {
            size
        };
        let (badge_icon, badge_color, headline) = if missing {
            (
                FluxIcon::CircleAlert,
                extended.colors.warning,
                self.t(cx, "statusFileMissing"),
            )
        } else {
            (
                FluxIcon::Check,
                extended.colors.success,
                self.t(cx, "downloadCompleted"),
            )
        };

        v_flex()
            .child(
                h_flex()
                    .items_start()
                    .gap(tokens.spacing.md)
                    .px(tokens.spacing.lg)
                    .pt(tokens.spacing.lg)
                    .child(
                        div()
                            .relative()
                            .flex_none()
                            .child(Self::render_icon_tile(kind_icon(row.kind), cx))
                            .child(
                                div()
                                    .absolute()
                                    .right(px(-BADGE / 4.))
                                    .bottom(px(-BADGE / 4.))
                                    .size(px(BADGE))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .border_2()
                                    .border_color(tokens.colors.surface)
                                    .bg(badge_color)
                                    .child(
                                        Icon::new(badge_icon)
                                            .size(px(10.))
                                            .text_color(tokens.colors.surface),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(tokens.spacing.xxs)
                            .child(
                                div()
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(badge_color)
                                    .child(headline),
                            )
                            .child(Self::render_title(SharedString::from(row.name.clone()), cx))
                            .child(
                                div()
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .font_features(tabular_numbers())
                                    .text_color(tokens.colors.muted_foreground)
                                    .child(SharedString::from(meta)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(tokens.typography.xs.size)
                                    .line_height(tokens.typography.xs.line_height)
                                    .text_color(extended.colors.text_tertiary)
                                    .child(SharedString::from(row.save_dir.clone())),
                            ),
                    ),
            )
            .when_some(self.last_error.clone(), |this, error| {
                this.child(Self::render_error(error, cx))
            })
            .child(
                Self::footer(cx)
                    .justify_end()
                    .child(
                        Button::new("progress-open-folder")
                            .outline()
                            .control(cx)
                            .icon(FluxIcon::FolderOpen)
                            .label(self.strings.open_folder.clone())
                            .on_click(cx.listener(|this, _, _, cx| this.reveal(cx))),
                    )
                    .child(
                        Button::new("progress-open-file")
                            .primary()
                            .control(cx)
                            .icon(FluxIcon::File)
                            .label(self.strings.open_file.clone())
                            .disabled(missing)
                            .on_click(cx.listener(|this, _, _, cx| this.open_file(cx))),
                    ),
            )
            .into_any_element()
    }
}

/// 耗时：`m:ss`，超过一小时 `h:mm:ss`。
fn format_elapsed(seconds: u64) -> String {
    let (hours, minutes, secs) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

impl gpui::Render for ProgressWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let surface = active_theme(cx).tokens().colors.surface;
        let content = match self.row() {
            Some(row) if row.state == TaskState::Completed => {
                let row = row.clone();
                self.render_completed(&row, cx)
            }
            Some(row) => {
                let row = row.clone();
                self.render_progress(&row, window, cx)
            }
            None => div().into_any_element(),
        };
        // 列方向 flex 下 `flex_none` 的内容区保持自然高度（不被拉伸到视口）。测量层必须显式
        // `inset_0` 钉在内容区左上角：只写 `absolute()` 时它落在静态位置（内容之后），
        // 量出的顶边会多出一整个内容高度。
        v_flex().size_full().bg(surface).child(
            div().relative().flex_none().w_full().child(content).child(
                canvas(
                    |bounds, window, cx| {
                        let wanted = f32::from(bounds.origin.y + bounds.size.height);
                        let viewport = window.viewport_size();
                        if (wanted - f32::from(viewport.height)).abs() < RESIZE_EPSILON {
                            return;
                        }
                        let width = viewport.width;
                        window.defer(cx, move |window, _| {
                            window.resize(size(width, px(wanted.ceil())));
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::format_elapsed;

    #[test]
    fn elapsed_switches_to_hours_past_sixty_minutes() {
        assert_eq!(format_elapsed(59), "0:59");
        assert_eq!(format_elapsed(201), "3:21");
        assert_eq!(format_elapsed(3_725), "1:02:05");
    }
}
