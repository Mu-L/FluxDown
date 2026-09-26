# FluxDown internals · HTTP API · 宿主与客户端 crate

> 本文件是 `FluxDown/AGENTS.md` 的深挖附录：只放**枚举性 / 可从源码复原**的细节，硬不变式与红线在 AGENTS.md。
> 路径以 `FluxDown/` 为根（cwd=工作区根时前置 `FluxDown/`）。事实层以源码为准，文档给坐标。

---

## HTTP API（`native/api`，`fluxdown_api`）

一个端口（桌面默认 17800 **仅 127.0.0.1**；server 默认 `0.0.0.0:17800`）、一个 axum 服务器，多组按配置独立启停的路由。`local_server_*` 配置变更时 actor 热重启监听（优雅停机 + 重绑，20×100ms 重试竞态）。

| 路由组 | 端点 | 开关（config 键） | 鉴权 |
|---|---|---|---|
| 探活 | `GET /ping` | 总开关 | 无 |
| 脚本接管 | `POST /download`、`/download/batch` | `local_server_takeover_enabled`（默认开） | `X-FluxDown-Client` 头 + 可选 token |
| aria2 兼容 | `POST /jsonrpc`（36 方法）+ `GET /jsonrpc`（WS 升级，`jsonrpc_ws.rs`：RPC + `onDownloadXxx` 通知推送） | `local_server_jsonrpc_enabled`（默认开） | 可选 token（`X-FluxDown-Token` 或 `params[0]="token:xxx"`） |
| 管理 API | `/api/v1/*`（info、tasks CRUD+pause/continue[all]、queues、resolve/preview、groups CRUD+pause/continue、plugins list/install/install-dev/enabled/settings/**auth**（begin/poll/cancel/logout/status）/uninstall + ignore-plugin-retry、**site-auth** list/save/get/delete（`GET /site-auth/{site}` 回传明文密码，仅定向查询用于编辑表单回填）、market list/install、**rss** CRUD+refresh+items+items/action+validate） | `local_server_api_enabled`（桌面默认**关**；server 强制开） | **强制** token（Bearer 或 `X-FluxDown-Token`） |
| MCP | `POST /mcp`（Streamable HTTP 无状态子集，协议 2025-06-18；12 工具：download_add/list/get/pause/resume/pause_all/resume_all/remove + queue_list + rss_list/rss_add/rss_remove） | `local_server_mcp_enabled`（桌面默认关；server 默认开） | 同管理 API token |
| OpenAPI | `GET /api/v1/openapi.json`（utoipa 3.1，含漂移守卫测试） | 随管理 API | 无 |

- **`ApiHost` trait**（`service.rs`）：必需方法（list/get/create/delete/pause/continue task、pause/continue all、list_queues、submit_external）+ 可默认降级方法（config/plugins/market/groups/resolve_preview/subscribe_task_events/…）。`UNKNOWN_ENDPOINT_MESSAGE` 区分未注册路由 404 与资源 404。
- **鉴权**（`auth.rs`）：常量时间比较；接管需 `X-FluxDown-Client` 头（利用 CORS 预检挡跨源 fetch）；管理/MCP 强制非空 token（403）。桌面默认绑 127.0.0.1（`local_server_lan_enabled` 可改绑 0.0.0.0），默认不返 CORS 头。
- **CORS 豁免开关**（`local_server_cors_allow_all`，默认 false）：开启后 `cors_and_preflight` 中间件对预检与真实响应都发 `Access-Control-Allow-Origin: *`（+ `Allow-Private-Network: true`、`Allow-Headers` 回显），等价 aria2 `--rpc-allow-origin-all`。这是安全模型第 2 条的显式豁免——供「用浏览器 `fetch` 探测 aria2」的网站识别本机服务，代价是任意网页可探测/提交下载（仍受确认框 + 管理 token 保护）。
- **语义区分**：脚本接管 → 外部下载流程（弹确认框）；aria2 `addUri`/管理 `POST /tasks` → 直接建任务返真实 ID（自动化预期无弹框）。`takeover.rs` 的 batch 两形态合并为单 `DownloadRequest`（url 换行 join，匹配 Dart 单确认约定）。
- **aria2 纯映射**（`aria2.rs`）：GID↔task_id 编解码、`METHOD_NAMES`=36、`NOTIFICATION_NAMES`=6、业务错误统一 `code:1`。

---

## 宿主与客户端 crate

### `native/hub`（桌面/移动 App，唯一 rinf）
`lib.rs`（`write_interface!`、current_thread runtime）；`signals/mod.rs`（信号定义——Dart 绑定契约，不可动）；`actors/download_actor.rs`（核心事件循环，**必须** drain resolve_rx/plugin_retry_rx）；`api_host.rs`（`HubApiHost`：读直查 Db，写经 command+oneshot 进 actor）；`rinf_sink.rs`（`EventSink`→Dart 信号）；`rinf_selection.rs`（`HostSelection`：HLS 60s 超时默认最高码率）；`signal_bridge.rs`（`From` 转换）；`native_messaging.rs`（Windows Named Pipe `\\.\pipe\fluxdown` / Unix socket；另有 `listener_endpoint()`/`probe_listener()` 供 Doctor 自连自 ping）；`nmh_registry.rs`（写 NMH 清单；另有只读 `diagnose()`。**Windows 清单落在 `<data_dir>\nmh\`（安装版 `%LOCALAPPDATA%\FluxDown\nmh`），绝不写 exe 同级**——全局安装在 `Program Files`，未提权进程写不进去，注册表键永远写不出来；agent `nmh.rs` 同规则，安装器 `[UninstallDelete]` 同步清理）；`file_association.rs`（.torrent 关联）；`protocol_registry.rs`（`fluxdown://`）；`diagnostics.rs`（**新**：设置页 Doctor 探针聚合——NMH 二进制/清单/各浏览器注册、pipe ping、本地 HTTP `/ping`、协议与 `.torrent` 关联、日志目录可写；由 `download_actor` 里一条独立后台泵消费 `RunDiagnostics`/`RepairNmhRegistration`，**不碰 Engine、不进主 `select!`、不进 `aux_tx`**）；`reveal_file.rs`；`updater.rs`（版本检查 + 多段并发下载 + 委托 `fluxdown_updater` helper）；`compat_flags.rs`（**新**：Windows 清除 PCA 误设的 RUNASADMIN AppCompatFlags，修 CreateProcess 740）；`logger.rs`（转发 engine 的 shim）。

> ⚠️ **`download_actor.rs` 的主 `tokio::select!` 已占满 tokio 的 64 分支硬上限**（`tokio/src/macros/select.rs` 的 `count_field!` 最后一格是 `_63`），再加一条就是编译错误 `up to 64 branches supported`。
> **新增任何 Dart 信号 / 定时节拍 / 回流通道都不许往主循环加分支**，一律并进既有的「辅助信号合并转发」：两个后台 `tokio::spawn` 泵（任务组 5 信号 / RSS 8 信号 + 60s 节拍 + 引擎回流）把消息合流进同一条 `aux_tx`，主循环只有一条 `Some(aux) = aux_rx.recv()`。照 `enum AuxSignal { Group(..), Rss(..) }` 加变体即可。

### `native/server`（headless，`fluxdown_server`）→ 见本文「Headless 服务器」

### `native/cli`（`fluxdown_cli`，二进制 `fluxdown`）
aria2c 风格。命令：ping/info/add(get)/list(ls)/status(stat)/pause/resume/rm/pause-all/resume-all/queue/watch/**config**(set/unset/get/list/path)。
- **A 模式**（默认）：typed HTTP client（复用 api `routes`+`types`），连运行中的 App/server。
- **B 模式**（`add --local`）：本进程内嵌 `fluxdown_engine::Engine`（`NoopSink`/`NoopSelection`）独立下载至终态，共享同一 SQLite（安装模式）。Ctrl-C → 暂停 + 退出码 7。
- env：`FLUXDOWN_URL`（默认 `http://127.0.0.1:17800`）/`FLUXDOWN_TOKEN`；`--json`；`K/M/G/T` 按 1024 解析；`.no_proxy()` 直连回环。退出码：0/1/2/3/5/7/24/32（aria2 风格，`exit.rs`）。

### `native/nmh`
浏览器 Native Messaging Host **中继二进制**（`com.fluxdown.nmh`）。浏览器 ↔（stdin/stdout 4 字节 LE 长度 + JSON）↔ nmh ↔（Named Pipe / UDS）↔ App。同步单线程；懒连 + 重连；除 `NO_LAUNCH_ACTIONS`(ping/tasks/task_op/open/reveal) 外未连接时自动拉起 App（50ms 轮询至 10s）；`warmup` 本地应答重叠冷启动；1MB 帧上限。
冷启动目标按 `APP_EXE_CANDIDATES` 在 nmh 同级目录顺序查找：**Flutter 本体在前**（`flux_down.exe` / `FluxDown` / `flux_down`），`fluxdown-agent` 只在无 Flutter 可执行时兜底——release 把两套二进制打进同一目录，顺序反了会让 agent→fluxdownd 抢走 `<data_dir>/engine.lock`，Flutter 引擎随后起不来。

### `native/fluxdown_updater`（**新**独立 helper）
依赖极简（zip/flate2/tar/windows-sys/libc，无 engine/api 依赖）。由 `hub/updater.rs` 在 App 退出前拉起 → 等父 PID 死 → 应用更新 + 重启。Action：PortableZip/Setup(NSIS 静默)/AppImage/tarball/deb/arch(pkexec)。用原生 helper 而非 PS/bat/sh 规避 MOTW/执行策略/引号问题。


### `native/daemon` + `native/agent`：设置面 JSON-RPC（GPUI 客户端唯一入口）
方法名、DTO 与错误码全在 `native/protocol`（`method.rs` / `daemon.rs` / `agent.rs` / `daemon_config.rs`）；GPUI 设置页只经 agent `ws://127.0.0.1:17800/rpc`，`daemon.*` 由 agent 透明转发。
- 本机协议基线由 `protocol/rpc.rs` 的版本常量控制；新增严格事件枚举即升版本（当前 v4：`ShellChanged` / `PowerChanged`），拒绝不能解析这些事件的旧进程混用，更新时 UI/agent/daemon 必须一起重启——版本不兼容时新进程对旧进程发**握手前** `system.shutdown`（daemon 与 agent 都受理，v4 起）替换它，每进程只替换一次。`task_activity.rs` 定义任务运行态与活动 DTO；`daemon.task.activity {taskId, beforeId?, afterId?, limit?}` 查询持久历史，两种游标互斥、页内 ID 升序，返回保留范围、更多页及截断信息。实时 `TaskActivityAdded` 仅在提交后广播，`TaskRuntimeChanged` 同时更新快照。
- **daemon 配置键目录 = `fluxdown_protocol::DAEMON_CONFIG_FIELDS`**（键、类型、范围、枚举、默认值、只读）。daemon `config.rs::validate_config_patch` 直接用 `normalize_daemon_config_patch` 校验；`public_config_values` 按 `is_public_daemon_config_key` 投影（含只读键 `domain_conn_caps`、`bt_tracker_sub_cache/updated_at`、`ed2k_server_sub_cache/updated_at`）。新增 daemon 设置 = 在目录加一行 + `actor.rs::apply_live_config` 按需 live-apply。`bt_seed_enabled`/`bt_auto_reseed` 落库前转 `"1"/"0"`（引擎按 `!= "0"` 读）。
- daemon 设置相关方法：`daemon.config.get/patch/proxyTest`、`daemon.config.connPolicy`（`domain_conn_caps` 有效条目数）/`clearConnPolicy`、`daemon.siteAuth.list/delete/clear`（`site_auth_credentials`，列表永不回传密码）/`match {url}`（按引擎 `site_key` 匹配单站点凭据、含明文密码，仅供本机官方 UI 新建下载表单回填）、`daemon.webhook.get/test/simulate/clearDeliveries`、`daemon.bt.trackerSubscription.refresh`、`daemon.ed2k.serverSubscription.refresh`、`daemon.component.*`、`daemon.plugin.*`（含 `daemon.plugin.auth` 的 `begin/poll/cancel/logout/status` 登录状态机——`logout` 在插件禁用时也放行，走宿主兜底删除凭据不进 JS；`poll` 回包省略 `challenge`/`challengeType` 时客户端保留上一帧）、`daemon.diagnos...
- agent 设置相关方法：`agent.preferences.patch {values, sync?}`（云同步目录键 `sync=true` 走同步链路——daemon 键也由 agent 转写 daemon；设备本地键 `sync=false`）、`agent.gateway.get/patch`（功能开关 + `lanEnabled`（下次启动生效）+ `regenerateUserToken`；`port` 只读）、`agent.platform.integrationGet/setAutostart/setFileAssociation/setUrlProtocol/openPath`（关联 / URL scheme 注册目标 = 同级 `fluxdown-desktop`，macOS 需在 `.app` 内；开机自启注册 **agent 自身** `--autostart`，旧版指向桌面的条目在 agent 启动时自动改写）、`agent.diagnostics.run/repair/logPaths/exportLogs`（结构化 Doctor：check id / hint / repair 稳定字符串，UI 映射 `doctorCheck*/doctorHint*/doctorAction*` 文案）、`agent.update.check`、`agent.power.arm {delaySecs}`（无活跃任务时返回 `{armed:false}`）/`agent.power.disarm`、`agent.capture.submitTorrentFile {path}` 与 `agent.plugin.installFile {path}`（agent 读文件 → daemon `/blobs/*` → 建任务/装插件；GPUI 不直连 daemon HTTP）。
- agent 外壳（`native/agent/src/{shell,lifecycle,power,clipboard_watch}.rs`）：`desktop` cargo feature（官方桌面包开启）下主线程跑 tao 事件循环持有 tray-icon 托盘（macOS Accessory 策略、无 Dock 图标），runtime 在工作线程；Linux 需要图形会话 + 会话总线上的 `org.kde.StatusNotifierWatcher` + (ayatana-)appindicator，否则 `TrayUnavailableReason::{NoDisplay,NoHost,InitFailed}`；headless 构建为 `NotBuilt`。`AgentSnapshot.shell.resident` = 托盘可用且 `close_to_tray`（默认 true），headless 恒 true；非驻留时无 UI 且无任务 / 捕获 / 选择 / 关机计划持续 15s 自动完全退出（兜底界面异常退出）。完全退出（`system.shutdown`、托盘「退出」、系统注销 `LoopDestroyed`）= 停监管 → daemon `system.shutdown` → 等 `daemon.lock` 释放（≤20s）→ 以 `CLOSE_REASON_SERVICE_QUIT` 关闭 UI 连接；SIGTERM 只退 agent。驻留时 agent 自己订阅 daemon 选择请求，无 UI 时以 `--capture` 拉起桌面弹选择窗口；待确认捕获同理（非驻留时改开主窗口）。自启 `--autostart`：托盘可见且 `start_minimized_to_tray` 只驻留托盘，否则拉起 `fluxdown-desktop --minimized`。macOS 上 agent 与桌面同 bundle，LaunchServices 可能把 reopen / openURLs 投给 agent：`Reopen` → 拉起桌面，`Opened` → 与桌面同规则静默建任务。剪贴板监听（arboard，`general.clipboard_watch`）与完成后关机状态机 / 执行（`FLUXDOWN_SHUTDOWN_DRY_RUN` 只记日志）都在 agent。
- agent 下载完成通知（`native/agent/src/{background_effects,notification}.rs`，偏好 `download.notify_on_complete`）：800ms 防抖、最长 3s 合并成一条，文案与 Flutter 同规则（`downloadCompleted` / `batchDownloadCompleted` + `andMoreFiles`，语言跟 `general.locale`；headless 构建固定英文）。开关未写过时默认开启（与设置页 / Flutter 一致）。平台投递：macOS **不用 notify-rust**（其 `NSUserNotification` 后端未设应用时弹「Where is use_default?」，设了也被新版系统拒收：`usernoted` 对已接入 UNUserNotificationCenter 的 bundle——Finder、`com.fluxdown.app`——记 `Legacy client … Denying message`），改用 `/usr/bin/osascript` `display notification`（标题正文经 argv 传入，显示为「脚本编辑器」身份），notify-rust 依赖仅限非 macOS 目标；Windows toast 用 AUMID `dev.zerx.fluxdown`，在 `HKCU\Software\Classes\AppUserModelId\dev.zerx.fluxdown` 登记 `DisplayName` + `IconUri`（数据目录 `notification_icon.png`），不登记会显示为「Windows PowerShell」，且刻意不与 Flutter 的 `Com.FluxDown.App`（挂有其 COM 激活器）共用；Linux 设置 appname、`desktop-entry=com.fluxdown.app` hint 与图标绝对路径。通知暂无点击 / 动作按钮。
- 桌面 `fluxdown-desktop`：`--minimized`（由 agent 自启拉起）打开最小化主窗口；`--capture` 只开确认窗口（外部捕获 → 新建下载窗口，引擎选择 → 选择窗口）；`magnet:/ed2k:/fluxdown:/http(s)` 参数与 `.torrent` 路径交 agent 建任务；单实例锁 `desktop.lock`（agent 数据目录同级），次实例转发后退出。
---

## Headless 服务器（`native/server`）

组装：Engine（feature plugins+components）+ `EngineEventSink`/`WsHostSelection`（都包 `WsHub`）+ actor + `api_router`（core）`.merge(extra_router).merge(demo_router?).fallback(SPA)`。

**Web UI 编译期内嵌（单二进制）**：`native/server/build.rs` 把 `FLUXDOWN_EMBED_WEBROOT`（缺省仓库 `web/dist`）**整棵目录递归全量** `include_bytes!` 进二进制——不按扩展名/文件名筛选，新增任何文件（含新建子目录、未知类型）下次编译自动进包，删除自动出包（每个文件 + 根目录都登记 `rerun-if-changed`）。运行期由 `web_assets.rs` 托管：内容哈希强 ETag + `If-None-Match` 304；缓存分档**由事实推导而非文件名清单**——`text/html` 一律 no-cache（多入口同样生效），文件名带 Rollup 内容哈希的 immutable 一年（与所在目录无关，`assetsDir` 改名不受影响），其余短缓存 + ETag 回源；未命中回 `index.html` 保 SPA 路由；不支持 Range。`content_type()` 那张扩展名表**只决定响应头、不决定是否嵌入**，表里没有的类型照嵌不误，只按 `application/octet-stream` 兜底并在构建期打 `cargo::warning`。前端改了**必须先 `cd web && bun run build` 再重编服务器**才可见。构建时目录缺失不报错，只生成空表 + warning，运行期给 503 自解释页（API/WS 不受影响）。

- **env**（`config.rs`）：`FLUXDOWN_DATA_DIR`、`FLUXDOWN_DATABASE_URL`（`sqlite:`/`postgres:`）、`FLUXDOWN_BIND`（默认 `0.0.0.0:17800`——**注意非回环**，与桌面不同）、`FLUXDOWN_SAVE_DIR`（首次启动播种 `default_save_dir`，库中已有则不覆盖；群晖 spk 用它传向导选定的共享文件夹）、`FLUXDOWN_WEBROOT`（**可选**覆盖：改从该磁盘目录托管 SPA；缺省用内嵌产物，**不再隐式探测 exe 同级 `./web`**——旧版残留目录会让升级后的服务器配上过期前端）、`FLUXDOWN_TOKEN`（预置访问密钥，仅库中无密钥时采纳）、`FLUXDOWN_DEMO`/`FLUXDOWN_DEMO_URL`、`FLUXDOWN_LANG`、`FLUXDOWN_LOG_LEVEL`。
- **访问密钥（`local_server_token`）是热更新的**：`fluxdown_api::auth::TokenCell` 由核心路由与 `routes_ext` 共享，首次设置 / 设置页改写 / `token/regenerate` 三条路径**立即生效，无需重启**（NAS 用户没有「重启容器」这一步）。密钥策略单一事实源 = `config.rs::validate_access_key`（ASCII 可见字符、8–128 位、字母+数字），Web 端镜像在 `web/src/lib/token-policy.ts`，**改一侧必须同步另一侧**；headless 侧禁止把密钥清空（`PUT /api/v1/config` 写空返回 400）。
- `actor.rs`：`run_actor` 独占 Engine，**必须** drain resolve_rx/plugin_retry_rx；`ActorCmd` 是 HTTP→引擎写路径（含 `ApplyConfig` live-apply 镜像桌面 SaveConfig）。live-apply 覆盖并发/限速/保存目录/CDN/UA/重试/代理/BT 全组 + **ED2K 订阅与 Kad nodes.dat 后台刷新** + `log_max_size_mb`（直接调 `logger::set_max_total_bytes`）；`ed2k_listen_port`/`ed2k_enable_upnp`/`ed2k_server_list` 由下载时现读，故意无分支。`main.rs` 启动时按 `ed2k_server_sub_startup_plan`（纯函数，缓存版本落后即清缓存）+ nodes.dat 24h 陈旧判定各后台刷新一次，镜像桌面 `download_actor`。
- `ws_hub.rs`：broadcast + `EngineEventSink`（EngineEvent→WS JSON）+ `WsHostSelection`（HLS/BT/variant 经 WS 往返，BT 60s 兜底）+ 维护 prev-state 表映射 aria2 WS 事件源。
- `routes_ext.rs`：管理 token 保护（config get/put、queues CRUD+启停/定时/排序、task 移队+boost、fs_list、proxy_test、token/regenerate、stats、logs、bt tracker-sub refresh、**ed2k server-sub refresh**（`POST /api/v1/ed2k/server-sub/refresh` → `Ed2kServerSubRefreshResponse`）、**component ffmpeg/ytdlp status/versions/install/uninstall**）；开放（`?token=` query，浏览器不能设头）：`GET /api/v1/ws`、`tasks/{id}/file` 流式取回、logs/export、openapi.json、Scalar `/docs`；**完全无鉴权**：`GET /api/v1/setup/status` + `POST /api/v1/setup`（首次运行向导，仅在密钥未设定时可写，设过即 409）。
- `analytics.rs`（**新**）：仅两条匿名部署遥测（`app_installed` 一次 + `app_active` 每日，`analytics_enabled` 门控），**绝不**采集下载/任务信息；匿名 device_id 存 config；`FLUXDOWN_ANALYTICS=off`/空 key 关。
- `demo.rs`（**新**）：`GET /demo/file` 确定性生成 64MiB 字节流（不落盘/不联网，支持 HEAD+Range，1MiB/s 限速），演练真实探测/分段/续传路径；仅 `FLUXDOWN_DEMO*` 设置时挂载。

**分发目标**（单个 server 二进制 + `FLUXDOWN_*` env，产物不含 `web/` 目录）：Docker（ghcr.io，amd64+arm64）、群晖 SPK、QNAP QPKG、OpenWrt IPK+LuCI、Unraid CA 模板、CasaOS、Scoop（`bucket/`）、Windows 安装器。脚本在 `packaging/`、`promotion/`、`docker/`。
