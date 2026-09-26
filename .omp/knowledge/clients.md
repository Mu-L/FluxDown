# FluxDown internals · Flutter 前端 · 扩展 · 用户脚本 · Web SPA · 官网

> 本文件是 `FluxDown/AGENTS.md` 的深挖附录：只放**枚举性 / 可从源码复原**的细节，硬不变式与红线在 AGENTS.md。
> 路径以 `FluxDown/` 为根（cwd=工作区根时前置 `FluxDown/`）。事实层以源码为准，文档给坐标。

---

## GPUI PC 客户端（`crates/`，三进程本机链路）

依赖方向固定为 `i18n` / `theme` → `components` → `shell` / capability crates → `app`。`app` 只做窗口与单一 agent 会话装配；capability 之间不互相依赖。

- `i18n`：`build.rs` 自动嵌入 `assets/i18n/*.json`；locale 规范化、英文键级回退、空值回退和插值与 Flutter 基线同契约。
- `theme` / `components`：公开完整 `SemanticThemeTokens`，通用组件只取活动 token；gpui-component 初始化已包含 gpui-base 初始化。正文基线 `typography.sm` = 13/18（gpui-component rem 同取 sm），`typography.xs` = 12/16。`FluxThemeState::extended()` 是 FluxDown 自有扩展 token（从 Base token 与明暗派生、随界面缩放，不进主题文件 schema）：`success/warning/text_tertiary/hairline/row_hover/chrome/nav_hover/nav_selected`、`caption`(11/14)、`title`(15/20)、图标三档 `icon.{sm12,md14,lg16}`；业务 crate 不用 `cx.theme().success/warning/danger`，也不写裸字号。theme 同时把 DataTable 投影为无网格（`table_row_border` 透明、表头与内容同底）、面板分隔把手投影为 hairline。
- 图标：`components::FluxIcon`（Lucide 1.48.0 子集，ISC，线宽 1.75，实现 `IconNamed`，资源由 `ComponentAssets` 提供、app 组合进 `DesktopAssets`）；gpui-component 自带 ~99 个图标语义不全，文件类型 / 任务状态 / 新增图标一律走 FluxIcon（新增：下载 SVG 到 `crates/components/assets/icons/` 并登记 `flux_icons!` 表）。`components::category_icon` 是分类图标唯一映射（侧栏与设置分类编辑器共用）；会刷新的数字统一 `font_features(tabular_numbers())`（MiSans 数字为比例宽）。
- 控件套件（`components/src/kit.rs`，所有窗口必须用）：全应用唯一一档控件高度 `CONTROL_HEIGHT`=28：带文字按钮 / 输入框 / 下拉（outline + `dropdown_caret`）/ 数字输入一律 `.control(cx)`（28 高、13px；`Input` 的高度必须走 `Styled::h`——它自带的同名 `h()` 只作用于多行，单行会静默退回 26px；gpui-component 按钮字号由 `Size` 写在内部 label 上，故套件用 `Size::Medium`，业务禁止 `.small()/.xsmall()`/自定义高度）；纯图标按钮 `.control_icon(cx)`，chrome 区用 `toolbar_action_button`；标签页一律 `segmented_tabs`（不用 `TabBar`/自造按钮组）；复选框一律 `check_mark`/`check_row`（不用 gpui-component `Checkbox`）；对话框标题 `dialog_title`、底栏 `dialog_footer(cancel, ok, DialogIntent, cx)`（取代 `DialogButtonProps`，其默认按钮 32 高且默认不显示取消）；卡片 `card`（surface + hairline + radius.lg，无阴影）。theme 把 gpui-component `background` 投影为 `surface`（对话框/输入框白底），亮色模式主色自动压暗到与白字对比 ≥ 4.5（`ensure_primary_contrast`）。
- `shell`：只拥有窗口 chrome、路由与内容槽，不知道下载、设置、账户、RSS 或扩展。主窗口是 40px 统一顶栏（`SHELL_TITLE_BAR_HEIGHT`，交通灯按它垂直居中；Windows/Linux 为 logo + `AppMenuBar` + 自带窗口按钮），`ShellRoute::with_title_bar(view)` 让活跃路由往顶栏挂插槽；插槽内可交互元素必须拦截左键 `mouse_down` 冒泡，空白处保持拖拽/双击。活动栏、侧栏、顶栏、状态栏同为 `chrome` 底，内容区 `surface`。
- 下载页布局：顶栏插槽 `components/title_bar.rs::DownloadTitleBar`（搜索、「视图」弹层：列/分组/排序/密度/详情面板、主按钮新建）；批量操作只在选中 ≥1 时以内容区底部浮动选择条出现（`selection_bar.rs`，依赖 `SelectionSummary{count,any,any_local,any_active,any_resumable}`）；状态栏只放全局速度、全部暂停/开始、限速、完成后关机、剩余空间。任务表：文件名列按容器宽回算吸收剩余宽度（不可手动拖宽）；表头排序由 `render_th` 自绘（只在当前排序列显示箭头）；选择列平时显示文件类型图标、悬停/有选中时换复选框；行悬停浮出单行操作；已完成不画进度条，只有下载中（primary）与失败（destructive）着色；紧凑 30 单行 / 舒适 44 双行。侧栏状态项用语义图标（非文件夹），悬停状态项时图标位换成分类展开箭头（不占额外缩进列），分类子项仅比父项多缩进 `spacing.lg` 且悬停/选中底色铺满整行，计数为 0 不显示。
- `downloads` / `settings` / `account` / `rss` / `extensions`：各自拥有视图模型、controller 与 capability-local port；只消费 `fluxdown_protocol` DTO。
- `rss`：活动栏仍进入独立 RSS 页面，订阅源列表与条目流只在内容区内分栏，不并入下载分类。`controller.rs` 以订阅 ID / 加载版本 / 操作代际隔离异步结果；RSS 的「已建任务」状态必须关联实际任务状态显示，任务删除或文件缺失不能继续显示完成。`view.rs` 提供搜索、稳定日期排序、可见条目全选与批量动作，条目使用虚拟列表；`editor.rs` 的新建验证绑定 URL 与认证/代理参数，保存目录支持手填及原生目录选择，编辑保留 provider 配置与运行态字段。
- Webhook 只从主窗口活动栏进入（`app/windows/main.rs` → `settings::WebhookView`），不出现在下载页或设置分类；设置中的通知页仅管理系统通知。独立页面复用 app 常驻的 `SettingsStore` 与既有 agent 会话，关闭设置窗口不影响配置保存、投递事件或重连快照。端点表和编辑器沿用 `settings/sections/webhook*.rs`。
- `app`：创建唯一 `AgentClient`；`session.rs` 的 `Entity<AgentSession>` 是快照/事件唯一入口（`EventEmitter<SessionSignal>`）。daemon、agent 与 UI 复用 `protocol/event.rs` 的纯 DTO 投影；每个有效事件同时推进最新快照及游标，`SessionConsumer::attach()` 直接订阅并使用当前投影，晚开窗口不再另发 `system.snapshot`。传输层缓冲握手期间通知、过滤快照已覆盖的帧，序号缺口/epoch 改变触发重连；旧任务采样不再向视图广播。订阅随视图销毁解除，主窗口关闭不影响其他窗口；事件泵按帧批处理（≤256 条一次 `update`）。
- 启动与连接态：`AgentClient` 在 GPUI 初始化前就开始连接，连接失败前 5s 以 100ms 快速重试（覆盖回环重连、agent 冷启动 / 重启），之后指数退避。`AgentSession` 对连接态有 800ms 宽限（`OFFLINE_NOTICE_GRACE`）：启动时的首个快照、断线后的重连快照在宽限内到达，视图就不收到 `SessionSignal::Stale`、不显示「正在连接」只读态（期间命令排队、重连后送达）；超时 / `Fatal` 才广播离线。`attach()` 只在已广播离线时标记视图只读。主窗口（含 `--minimized`）在会话就绪（`is_settled`：有快照或已确认离线）后才打开——热启动首帧即完整数据、主题与语言，冷启动最多等宽限后以连接态出现。
- 跨窗口刷新：`native/daemon/src/event_hub.rs` 在生产边界把队列、任务组、Webhook 投递列表转换为对应顶层 `DaemonEvent`，不可仅包装成 `DaemonEvent::Engine`（快照会更新，但监听顶层事件的已有窗口会漏刷）。消费者同时处理增量事件与 daemon 重连的 `DaemonSnapshotReplaced`；移动任务及删除队列迁移任务后，通过 `TaskQueueChanged` 同步任务归属与调度缓存，删除已选队列时主窗回退「全部」。新增状态事件需验证真实 sink 广播与多个消费者一致，不能靠操作后重开窗口或额外拉快照补救。
- `app/windows/`：`WindowRegistry`（`Global`）按 `WindowKey{Main, Settings, NewDownload, QueueManager, Selection(id), TaskDetail(id), GroupDetail(id), Progress(id)}` 去重、`on_window_closed` 清理；**退出判定唯一入口**：最后一个用户窗口关闭 → `lifecycle::quit_ui`（只退出界面；延后一轮执行，`lifecycle::keep_alive` 登记的在途请求——新建下载提交、捕获确认 / 忽略——未完成前推迟退出，防止关掉最后一个窗口后请求丢失）。主窗口/设置窗口边界 500ms 防抖写 `desktop.window.<main|settings>`（`agent.preferences.patch` + `sync:false`），恢复时校验可见区域 ≥100×100。主窗口关闭策略 `main::should_close` 只看 agent 快照的 `shell.resident`：驻留 → 直接关窗（界面进程随最后一个窗口退出，后台继续）；非驻留 → 等同「退出」。原生关闭按钮走 `on_window_should_close`，⌘W / 菜单「关闭窗口」走 `WindowRegistry::close_active_window` 再调同一判定——gpui 的 `remove_window` 不触发 `windowShouldClose:`，两条路径必须共用一份策略。
- 退出语义（`app/lifecycle.rs`）：菜单「退出」/ ⌘Q / 非驻留关主窗 = **完全退出**（有活跃任务先 `confirm_active_tasks`，再 `system.shutdown` → agent 先关停 daemon 再退出，界面随后 `cx.quit()`，3s 内未受理也退出）；agent 以 `CLOSE_REASON_SERVICE_QUIT` 关闭连接（托盘「退出」、版本替换）时 `AgentClientEvent::ServiceStopped` → 界面立即退出且 `ServiceBootstrap::stop()` 不再重拉 agent；SIGTERM 只退 agent（`agent-shutdown`），界面照常重拉。握手版本不兼容时桌面先对旧 agent 发握手前 `system.shutdown`、等监听关闭，再由 bootstrap 拉起同级新版本（每进程只替换一次）。
- 托盘、剪贴板监听、完成后关机的状态机与执行都在 agent（见 `hosts-and-api.md`「agent 外壳」）；界面只保留 `power.rs`（`AgentSnapshot.power` → 状态栏 `SharedShutdownStatus` 与主窗口倒计时通知，请求转 `agent.power.*`）与设置页的托盘开关（`shell.tray_available` 决定可用，不可用时说明文案换成 `trayUnavailable*` 原因）。
- 窗口分级：主窗口 / 设置 / 新建下载 / 队列管理 `Normal`；引擎发起的选择（HLS/BT/变体）每 request 一个 `Floating` 窗口；任务「详情」窗口 `TaskDetail(id)` 为普通窗口，任意数量并存，信息区恒展开，不提供折叠或置顶开关，开始/结束时间按本地时间显示到秒。窗口级事件绑定具体句柄，旧视图的延迟回调不得操作新窗口。
- 独立下载进度 / 完成窗口 `Progress(id)`：普通、不可调整大小、可最小化，**无置顶**（锁定的 gpui 没有运行时置顶 API，crates 禁 unsafe）。开窗决策 `downloads::ProgressWindowTracker`（纯状态机）由 `app/progress_windows.rs` 喂会话任务状态（`TaskChanged` / `Engine::TaskProgress` / 快照替换）；意图只来自 `DownloadHostActions.on_user_started`——`DownloadView::execute_commands` 恰好一条单任务 Resume / Redownload 成功、`create_download` 恰好建出一个立即开始的任务、任务详情「继续」，以及无主窗口时 `new_download::submit` 的同规则直提交（`DownloadsResult::created_task_ids` 读 `{taskId}` / `{taskIds}`）。批量、队列调度、自动恢复、RSS 不开窗；免打扰只免二次确认表单，不免进度窗口：agent `create_all`（`Direct` / 静默 `External`）建成后发布一次性 `AgentEvent::CaptureTasksStarted(task_ids)`（不进快照，协议 v5），UI 对单任务同样 `user_started`；此时没有 UI 连接且 `desktop.progress_window` 未关时 agent 以 `[--capture] --progress-task <id>` 拉起界面（与确认拉起共用冷却），`LaunchOptions.progress_task` 在首个快照后按用户开始处理，由意图保活推迟 `--capture` 的「无待确认即退出」。任务进入活跃态开进度窗口（激活）；完成时已开窗口原地切完成视图（设备本地偏好 `desktop.completion_window` 或窗口内复选框覆盖为关则关窗），用户中途关掉的任务完成时弹一次不抢焦点（`focus:false`）的完成窗口；层级：界面常是后台应用，进度窗口开后走 `windows::bring_to_front`（macOS 需 `cx.activate(true)`，否则压在浏览器下），完成窗口在 macOS / Windows 用 `WindowKind::PopUp`（不激活应用的浮动面板 / TOPMOST）浮于所有应用之上，Linux X11 PopUp 为 override-redirect 不可拖动，保持普通窗口 + `request_attention`；`desktop.progress_window` 关闭只停进度窗口。窗口高度由 `ProgressWindowView` 按内容自然高度在 prepaint 时 `window.resize` 校正（测量层为内容区内 `absolute().inset_0()` 的 canvas；不能用 `ElementExt::on_prepaint`——它的测量层只写 `absolute().size_full()`，作为最后一个子元素时落在静态位置（内容之后），量出的高度翻倍；窗口选项清掉辅助窗口默认 720×520 最小尺寸）；意图登记时任务常仍在排队，界面无窗口时（捕获拉起、表单提交后即关闭）经 `lifecycle::keep_alive` 保活至开窗 / 意图丢弃，最长 60s，否则进程在开窗前退出；「停止」= 暂停 + 关窗；打开文件 / 文件夹成功后视图发 `HandedOff`，宿主等窗口失活（对方程序已接管前台）再关窗（1.5s 超时兜底）——本应用在前台时关闭当前窗口，系统会把主窗口提到前面压住刚打开的程序。开关在设置「通知」页「下载窗口」分区。
- agent 捕获分流（`native/agent/src/capture.rs::CaptureOrigin`）：`Direct`（系统打开链接 / 拖入，RPC `silent:true`）直接建任务；`External`（HTTP `/download[/batch]`、NMH）按偏好 `download.silent_download` 静默建任务或入确认队列，静默时 `unattended` 取设备本地偏好 `download.silent_skip_selection`；`Prompt`（剪贴板监听，RPC `silent:false`）恒入确认队列。换行连接的批量 URL 先逐条拆分；保存目录 = 捕获方指定 > 分类目录（`category_dir.rs`，与 Flutter `resolveCategorySaveDir` 同语义，确认路径写进 `PendingCaptureDto.saveDir` 供表单预填）> 静默时「跟随上次」> daemon 默认。
- 外部捕获（扩展 / NMH / 剪贴板）**没有独立确认窗口**，直接进「新建下载」窗口（`app/windows/new_download.rs::install_captures`）：`AgentSnapshot.pending_captures` / `PendingCapturesChanged` 中新出现的事务按链接行（`out=` 文件名）追加进同一个表单（`NewDownloadView::add_captures`，原文逐字保留；无表单则按 `new_download_context_from_snapshot` 开一个并置前，无主窗口也行；置前 = `new_download::bring_to_front`：macOS 必须 `cx.activate(true)` + `activate_window`（单独 `activate_window` 只在应用内排序，后台应用的窗口仍被遮挡）；Windows `activate_window` 自带 SetForegroundWindow + 模拟输入绕过前台锁；Linux X11 `_NET_ACTIVE_WINDOW` / Wayland xdg-activation 可能被防抢焦点拒绝，额外 `request_attention` 让任务栏高亮）；已从 agent 消失的事务只移除上下文。提交时普通链接照旧 `daemon.task.create`，捕获条目 `agent.capture.resolve {accepted:true, request}`；表单释放时未确认的捕获 `accepted:false`。`PendingCaptureDto` 只带摘要（`hasCookies` / `headerNames` / `saveDir`），Cookie / 请求头 / 请求体只留在 agent 事务里，由 `native/agent/src/capture.rs::merge_confirmed_request` 以捕获原请求为底合并（url/method/body/audioUrl 恒取捕获值，表单留空回退捕获值，头同名表单优先，表单 UA 替换捕获 UA 头）。HTTP 认证框：单条 http(s) 链接按 `daemon.siteAuth.match`（与引擎 `site_key` 同一实现）去抖回填已保存站点凭据（用户手动编辑后不再覆盖）；单条且浏览器请求已带 `Authorization` 时不回填，沿用浏览器认证。
- 单实例：`launch.rs` 文件锁 + `instance_ipc.rs` 激活通道（Unix socket / Windows 命名管道，一行 JSON `{urls,files,activate}`）；次实例等待主实例处理并确认后退出，主实例激活/重建主窗口。锁错误不放行第二个 UI；端点未就绪可有界重试，写出请求后的确认失败不重放捕获。`--activate-existing` 只激活（成功 0、无主实例 3、失败非零），不启动 UI 或后台。`--capture` 由 agent 为待确认捕获 / 选择拉起，只开确认窗口；启动链接提交完、首个快照后仍无窗口即退出。`--minimized` 由 agent 自启判定需要界面时传入，会话就绪后开最小化主窗口。链接判定 / `fluxdown:` 解码统一用 `fluxdown_protocol::capture_link`。
- 动作/菜单：`crates/downloads/src/actions.rs` + `crates/app/src/actions.rs`（`actions!`）；键位与菜单树在 `app/menus.rs`（macOS `cx.set_menus` 原生，Windows/Linux `AppMenuBar` 注入 shell 标题栏）；字母键只在 `"DownloadView"` 上下文且无聚焦输入框时生效。**gpui 不自带 macOS 标准窗口键位**（⌘W/⌘M/⌘H/⌥⌘H/⌃⌘F/⌘Q 都得像 Zed 一样显式 `KeyBinding` + 菜单项，AppKit 只是从 keymap 读出 key equivalent 显示在菜单上）；macOS 菜单树镜像 Flutter `_buildMacMenus`：App 菜单含 隐藏/隐藏其他/全部显示，File 含「关闭窗口」，View 含「切换全屏」，独立「窗口」菜单（最小化/缩放/前置全部窗口）。**全局 `cx.on_action` 里操作活动窗口必须 `cx.defer`**：键盘触发的动作跑在该窗口自己的 update 栈内（窗口已从 `cx.windows` 取走），同步 `handle.update` 返回 Err 静默失败；菜单点击不在栈内所以「菜单能用、快捷键不能用」就是这个坑。取焦点窗口用 `WindowRegistry::focused_window`（macOS `cx.active_window()` 不认 `NSPanel`，`Floating`/`PopUp` 窗口会返回 `None`）。
- 下载数据层：`DownloadsController` 持 `Rc<TaskStore>`（哈希索引 + 逐行增量 + `generation`），表格代理只在 generation/筛选/排序/分组变化时重算 `visible`（含分组头），`render_td` 借 `Ref` 不克隆行。视图偏好（密度/分组/排序/列/详情面板/侧栏宽）全局单套存 `desktop.downloads.view`（设备本地）。
- 下载运行态：`DaemonSnapshot.task_runtime` / `TaskRuntimeChanged` 是主列表与独立详情的共同事实源。`sample_sequence` 在引擎采集处单调分配，不能用 UTC 毫秒或接收顺序替代；未知并发显示 `—`，BT 节点与传输数分开，暂停/终态不复活旧活跃状态，失联使实时值失效。`downloads/components/segment_progress.rs` 按真实字节区间绘制 IDM 式分段，窄列按像素聚合，不按配置上限或分段数伪造并发。
- 详情日志：`pages/task_detail_activity.rs` 通过 capability port 查询 `daemon.task.activity`，与实时 `TaskActivityAdded` 按持久 ID 合并；支持最新页、加载更早、断线补齐及失败重试，切换任务/失联作废在途结果。时间取源端 `timestamp_ms`；保留截断与源端队列缺口明确展示。关闭重开不丢历史，不再使用 View 私有状态变化记录冒充引擎日志。
- 运行链路：`fluxdown-desktop` 探活/单飞启动 `fluxdown-agent`；agent 探活/单飞启动 `fluxdownd`（Unix 下两级子进程都进独立进程组，终端 Ctrl-C 不连带后台）。关闭全部窗口是否终止后两者由 agent 驻留策略决定（`close_to_tray` 且托盘可用 → 驻留）。
- 开发入口：根目录 `cargo desktop-dev`（`scripts/desktop-dev`，无额外依赖）序列化开发构建（agent 带 `--features fluxdown_agent/desktop`，托盘与剪贴板监听只在该 feature 下编译）；已有 UI 时只唤起，否则先构建三个二进制再启动。`--build-only` 不启动或激活窗口。后台保留常驻/复用语义，不强杀或热替换；运行代码变更需先退出对应进程（托盘「退出」即完全退出），详情见 `CONTRIBUTING.md`。
- 三个二进制作为同级文件进入 Windows/macOS/Linux app 包；agent/daemon 使用独立 bearer 文件，云 Token 只保存在 agent 私有状态。
- gpui-base 尚未发布，依赖暂走固定 gpui-component git commit；Zed workspace 必须在 `Cargo.lock` 统一为单一提交，否则 `gpui` 类型会分裂。

---

## Flutter 前端架构（`lib/src`）

**状态管理**：ChangeNotifier + ListenableBuilder（无 Provider/Riverpod/Bloc），`_safeNotifyListeners()` 防已释放。Provider 统一模式：订阅 rinf 信号 + 单向 `sendSignalToRust` 写（`SettingsProvider`/`PluginProvider`/`ComponentController`/`download_controller`/…）。

**两套配置平面**：引擎 config（`SettingsProvider`，~80 键，经 rinf → `db.rs config` 表）vs Dart-only 客户端偏好（主题、云 token/设备 ID、analytics、update——存 `KvStore`）。

**Rust 宿主兼容与退出**：Flutter 继续消费既有 `TaskProgress`/`SegmentProgress`，`RinfEventSink` 显式跳过已被这些信号覆盖的运行态通知和已由引擎落库的活动通知，不逐帧打印未处理日志。桌面退出在窗口销毁前调用 `finalizeRust()`；hub 经 `AuxSignal` 停止 actor，等待下载取消、进度报告器排空和活动最终落库，再释放引擎写租约。关闭到托盘不停止引擎。

### 存储：`services/kv_store.dart`
SharedPreferences 门面，**便携模式**（`portable` 标记）写 `<exe>/portable_data/settings.json`（400ms 防抖），安装模式透传。init() 全量入内存缓存，`runApp` 前必须 await。是 theme/cloud/analytics/update/device 的存储层。

### 主题：双层 token 系统（schema v2）
- `flux_theme_tokens.dart`：Layer0 **颜色** token（~30 字段 + 嵌套 metric），5 内置预设工厂（defaultDark/Light、midnightBlue、nord、warmLight），JSON per-field 回退，`FluxThemeScope` InheritedWidget 下发。
- `flux_metric_tokens.dart`：Layer1 **非颜色**度量 token（~60 字段：15 圆角/2 描边/5 间距/3 按钮高/~22 alpha/8 移动几何），private raw + clamped getter。
- `app_colors.dart`/`app_metrics.dart`：读门面（`.of(context)`）；`AppMetrics.soft/muted/scrim(color)` 由 base+alpha 派生半透明色，消灭魔法数。
- `theme_provider.dart`：5 内置 × 5 accent（blue/green/violet/rose/custom）+ 导入自定义主题（`imported_themes_v2`）+ uiScale；`activeTokens` 优先级 导入主题 > 内置+accent。
- `segment_palette.dart`：黄金角生成最多 256 个对比安全的 per-thread 颜色。

### 云同步：`services/cloud/`（**已落地并接线**，contract v1，见 `ops.md`「设计文档实现状态」）
`config_sync_service.dart`（SSE 驱动实时配置同步，状态机 + 退避 + 防回声）、`cloud_client.dart`（REST + 401 自动刷新，base 由 `--dart-define FLUXCLOUD_BASE_URL`）、`cloud_auth_service.dart`（账号会话，登录即启用云）、`sync_catalog.dart`（per-key 读写绑定，**显式排除**设备本地键：路径/端口/token/代理/behavior）、`cloud_models.dart`、`device_identity.dart`（持久 deviceId/name/platform）、`nickname_pool.dart`。仅同步引擎配置的**跨设备通用**子集；下载数据不同步。

### 快速下载小窗：`popup/`（第二 Flutter 引擎）
原生宿主以 `--quick-popup` 拉起 `runQuickPopupApp()`，**零插件注册 + 不初始化 Rust**，经 MethodChannel `fluxdown/popup_child` 与主引擎通信（主引擎侧 `services/popup_window_service.dart`）。payload（主题 tokens/语言/队列/目录/URL）JSON 注入；复用 `quick_download_form`/`manifest_select_view` 与同一 token→ShadTheme 管线。清单预解析命中时原窗切 ManifestSelectView。

### 其它服务/模型（新）
`analytics_service.dart`（两条匿名事件，`ANALYTICS_APP_KEY` define + `analytics_enabled` 门控）、`update_service.dart`（changelog vs `APP_VERSION`，`update_channel` stable/frontier）、`platform_utils.dart`（便携检测 + 数据目录迁移，与 `data_dir.rs` 同步）、`resolve_variant_service.dart`（rinf 信号驱动全局弹窗）；`models/`：`plugin_provider`、`components_provider`（Ffmpeg/Ytdlp 控制器）、`ua_presets`（UA 单一事实源）、`custom_category`、`manifest_breadcrumb`。

### 桌面 widgets 架构（不逐文件，按族看）
- **视图系统**：`task_list` + `task_list_item`（行）、`task_columns`（列注册表，表头/行单一事实源）、`view_options_panel`（UI，backed by `models/view_prefs`）、`task_tab_bar`、`status_bar`、`sidebar`、`header_bar`。列表/网格双形态 + 舒适/紧凑双密度 + 多维分组吸顶 + 动态列。
- **manifest 对话框族**：`manifest_select_dialog`/`manifest_select_view`（与 popup 共享）/`manifest_dialog_chrome`/`manifest_browse_list`/`manifest_advanced_panel`（backed by `models/manifest_selection`+`manifest_breadcrumb`）。
- **组件**：`task_group_card`/`group_detail_panel`（backed by `models/task_group`）。
- **详情**：`detail_panel`/`bt_file_list_widget`。
- **对话框族**：`new_download_dialog`、`quick_download_dialog`+`quick_download_form`（与 popup 共享）、`queue_manager_dialog`、`plugin_detail_dialog`/`plugin_setting_form`/`plugin_list_view`、`resolve_variant_dialog`、`hls_quality_dialog`、`bt_file_selection_dialog`、`category_edit_dialog`、`update_changelog_dialog`、`feedback_dialog`。
- **原语**：`flux_sonner`（toast）、`context_menu`、`split_action_button`、`number_selector`、`ui_scale_widget`、`dir_picker_field`。

### 移动端 `mobile/`（Android 已发布）
`mobile_app`（`Platform.isAndroid||isIOS` 路由入口）、`mobile_shell`（任务/设置双屏 + 悬浮 Dock）、`mobile_ui`、`screens/`、`pages/`、`sheets/`、`services/`（share_intent、mobile_storage）。无窗口/托盘/autostart/NMH；保留 HLS/BT/variant 全局弹窗。复用 models/i18n/theme/bindings。

### 设置项（单一事实源 = `models/settings_provider.dart` load switch + `db.rs config` 表）
~80 键，分类：**下载**（default_save_dir/segments、auto_max_connections、domain_conn_caps、max_concurrent_tasks、speed_limit_bytes、max_auto_retries、auto_retry_delay_secs、auto_resume_on_start、remember/last_save_dir、default_queue_id、global_user_agent、cdn_multi_enabled、cdn_max_nodes［0=自动］、cdn_resolver_endpoints/cdn_ecs_subnets/cdn_hints_base［云端下发，Dart 云拉取落库］、cdn_node_health/cdn_pending_reports/auto_route_health［引擎学习/遥测缓存，UI 不读写］）、**App/系统**（close_to_tray、start_minimized_to_tray、auto_startup、auto_check_update、update_channel、analytics_enabled、notify_on_complete、silent_download_enabled、silent_skip_selection［免打扰子开关：跳过 BT/HLS/变体二次选择；设备本地，不入云同步目录］、use_server_time、keep_awake_while_downloading、log_max_size_mb、reveal_file_cmd）、**悬浮球/剪贴板**、**侧栏/标题栏可见性**、**自定义分类**、**代理**、**BT**（含 tracker 订阅键）、**ED2K**（server_list/订阅/kad/upnp/…

---

## 浏览器扩展（`fluxDown/`）与用户脚本（`userscript/`）

### 扩展（WXT，Chrome + Firefox MV3）
- **通信**：全平台走 NMH。扩展 →（stdin/stdout）→ `fluxdown_nmh` →（Windows Named Pipe / Linux-mac UDS）→ App。消息 = 4 字节 LE 长度 + JSON。action：`ping`（只探不拉起）/`download`/`batch_download`（换行 join 单确认，按 700KB+1000 条分块防 1MB 帧上限，旧 App 回退逐条）/`warmup`（本地应答重叠冷启动）。
- **三层拦截**：`onHeadersReceived`（缓存元数据 + Firefox `webRequestBlocking` cancel）→ `onDeterminingFilename`（Chrome 先发起 `downloads.cancel`，再调用 `suggest()` 释放文件名管线；取消成功才投递客户端）→ `onCreated+onChanged` 兜底 + 页面态 `fetch-interceptor.ts`。`suggest` 不支持 `cancel` 字段；取消失败保留原生下载，投递失败仍走既有浏览器回退。重定向下载以 `finalUrl` 为目标，原始 URL 保留用于请求事务缓存查找。
- **资源嗅探**（`media-sniff.ts`）：视频/音频/HLS/DASH/大文件，按 tabId 分组 + badge。
- Chrome ID 经 manifest key 钉住（匹配 NMH `allowed_origins`）；Alt+Shift+D 切换拦截；`Alt+Click` 15s 放行；声明零数据采集。

### 用户脚本（`userscript/fluxdown.user.js`，Tampermonkey）
页面态**扩展替代**（不能/不愿装扩展的用户）。`GM_xmlhttpRequest` POST 到本机 RPC `:17800/download`（带 `X-FluxDown-Client` 头 + 可选 token），拦截 DOM 下载 + hook fetch/XHR/MediaSource 嗅探。局限：无法拦截内核发起（Content-Disposition）下载、仅非 httpOnly cookie。

---

## Web SPA（`web/`）

React 19 + Vite 8 + TanStack（Router/Query/Table/Virtual/Form）+ Tailwind v4 + Radix + bun + oxlint + react-compiler。`bun run build` → `web/dist`，由 `fluxdown_server` **编译期内嵌**进二进制托管（SPA fallback→index.html；`FLUXDOWN_WEBROOT` 可覆盖成磁盘目录，见 hosts-and-api.md）——改了前端要重编服务器才生效。路由：`/login`、`/`（TasksScreen）、`/settings`（token 门禁，401→清凭据→/login）。`src/lib`：`api.ts`（typed REST）、`ws.ts`（可重连 WS live store）、`cloud/`（L2 云同步 client）、`i18n`、`task-group`、`manifest-selection`、`view-prefs`、`theme`、`format`。

**双端信息架构对齐（硬约束）**：同一功能在 web 与桌面 App 的**归属位置必须一致，基准 = 桌面**——设置项跟随桌面 `settings_page.dart` 的分类（web 设置分区组件与桌面侧边栏分类一一对应：GeneralSettings↔通用、DownloadSettings↔下载、ProxySettings↔代理…），对话框字段的分区/排序跟随桌面对应对话框。给双端并行开发（含 subagent 派发）写任务时，**归属分类/排序必须写成一份共享契约**（明确"桌面 X 分类 + web 对应分区组件"），禁止两份各自措辞留给执行者解读。交付前自查：桌面截图里该功能在哪个菜单，web 就必须在哪个菜单。

**设置页布局**（`web/src/routes/settings.tsx` + `design.css` 的「设置」段）：左导航分类 = general/account/appearance/download/bt/**ed2k**/proxy/security/notify/extensions/about（与桌面侧边栏同序）。正文结构 `.settings-body`（滚动容器，高度确定）→ `.settings-cols`（**多列容器，高度必须自适应**——两者不能合并，否则 `column-count` 会按视口高度分列并横向溢出）。≥1200px 两列、≥1900px 三列的瀑布式排布：`.set-group` / `.set-section`（小标题+卡片+同组脚注的整体，`break-inside: avoid`）是列内元素，其余直接子元素（分区标题/说明/宽面板）`column-span: all` 整行铺满，超宽卡片显式加 `.set-wide`。异步卡片的 loading 态要与加载完成后**行数、title/desc 一致**（见 `ComponentsSettings`），否则首屏到货会重新均衡列高造成抖动。

---

## 官网（`website/`）

Astro SSR（`@astrojs/node` standalone，**自托管**非 Vercel；`deploy.sh`+Docker）。营销 + 文档 + 社区 API 站，**不属于**下载栈。
- **页面**：首页（多语言变体）、plugins、faq、themes/theme-builder、changelog、announcements、api-docs（Scalar over `public/openapi.json`）、sponsor/pay、vote、privacy/terms、feedback 等。
- **`/docs` 双语内容集**：`src/content/docs/{en,zh}/<section>/<page>.md`（纯 Markdown，禁 MDX/HTML）；section 枚举见 `content.config`；zh 带 `sourceHash`（en 正文 sha256[:12]，`npm run docs:hash`）驱动过期横幅；en-only 页回退 en + `noindex` + 排除 sitemap（`docs-fallback.ts` 单源）。
- **API 路由**（`src/pages/api/`）：feedback、changelog、release、plugins/themes/components 代理、sponsor/pay、vote、subscribe、issues、`webhooks/github`（**GitHub webhook 接收器**，HMAC——与任务事件 webhook 无关，见 `ops.md`「设计文档实现状态」）。
