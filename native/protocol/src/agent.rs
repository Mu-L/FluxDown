//! 官方 agent 暴露给 UI 的无令牌 DTO。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// FluxCloud 用户状态。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum CloudUserStatus {
    #[default]
    Active,
    Disabled,
    Pending,
}

/// FluxCloud 用户公开资料。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudUser {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub status: CloudUserStatus,
    #[serde(default)]
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub origin_id: Option<i64>,
    #[serde(default)]
    pub origin_id_changed: bool,
    pub membership_ordinal: Option<i64>,
}

/// 前向兼容的套餐权益集合。未知字段必须原样保留。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(transparent)]
pub struct Entitlements(pub BTreeMap<String, Value>);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanCampaignStage {
    pub label: String,
    pub price_minor: i64,
    pub quota: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudPlanCampaign {
    pub name: String,
    pub end_at: Option<String>,
    pub stages: Vec<CloudPlanCampaignStage>,
    pub sold_total: i64,
    pub stage_sold: Vec<i64>,
    pub current_stage_index: i64,
    pub effective_price_minor: i64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudPlan {
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub badge: Option<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub color: String,
    #[serde(default = "default_badge_style")]
    pub badge_style: String,
    #[serde(default)]
    pub badge_color: String,
    #[serde(default)]
    pub badge_numbered: bool,
    #[serde(default = "default_badge_number_digits")]
    pub badge_number_digits: i64,
    #[serde(default)]
    pub price_minor: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub highlights: Vec<String>,
    #[serde(default, rename = "entitlements")]
    pub entitlements_raw: BTreeMap<String, Value>,
    #[serde(default)]
    pub sort: i64,
    pub campaign: Option<CloudPlanCampaign>,
    #[serde(default = "default_true")]
    pub purchasable: bool,
}

fn default_badge_style() -> String {
    "outline".to_owned()
}

fn default_badge_number_digits() -> i64 {
    4
}

fn default_currency() -> String {
    "CNY".to_owned()
}

fn default_true() -> bool {
    true
}

/// 当前账户资料与套餐快照（`GET /me`）。FluxCloud 把用户字段平铺在顶层，
/// `entitlements` / `currentPlan` / `purchaseCreditMinor` 为同级字段（与 Flutter
/// `CloudProfile.fromJson` 一致），因此 `user` 用 `flatten` 映射。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudProfile {
    #[serde(flatten)]
    pub user: CloudUser,
    #[serde(default)]
    pub entitlements: Entitlements,
    pub current_plan: Option<CloudPlan>,
    #[serde(default)]
    pub purchase_credit_minor: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct OriginIdCheckResult {
    pub available: bool,
    pub reason: Option<String>,
}

/// 受信任设备公开投影。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudDevice {
    pub id: String,
    pub device_id: String,
    #[serde(default)]
    pub name: String,
    pub platform: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_seen_at: String,
    pub last_ip: Option<String>,
    pub app_version: Option<String>,
    #[serde(default)]
    pub is_online: bool,
    #[serde(default)]
    pub is_current: bool,
}

/// 无 access/refresh token 的本地会话视图。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionDto {
    pub user: CloudUser,
    #[serde(default)]
    pub entitlements: Entitlements,
    pub current_plan: Option<CloudPlan>,
    pub device: CloudDevice,
}

/// 邮箱或设备验证步骤的无令牌元数据。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AuthVerificationDto {
    pub ttl_seconds: u64,
    #[serde(default)]
    pub will_replace_devices: bool,
}

/// 登录的本地安全结果。成功结果只包含无令牌会话。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentLoginResult {
    Ok {
        session: Box<AgentSessionDto>,
    },
    DeviceVerificationRequired {
        ttl_seconds: u64,
        #[serde(default)]
        will_replace_devices: bool,
    },
}

/// 购买订单。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudOrder {
    pub order_no: String,
    pub plan_code: String,
    #[serde(default)]
    pub plan_name: String,
    #[serde(default = "default_order_status")]
    pub status: String,
    #[serde(default)]
    pub amount_minor: i64,
    #[serde(default)]
    pub list_price_minor: i64,
    #[serde(default)]
    pub credit_minor: i64,
    pub upgrade_from_plan: Option<String>,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub campaign_name: Option<String>,
    pub stage_label: Option<String>,
    pub referral_code: Option<String>,
    #[serde(default)]
    pub referral_discount_minor: i64,
    pub code_url: Option<String>,
    #[serde(default)]
    pub created_at: String,
    pub paid_at: Option<String>,
    #[serde(default)]
    pub expires_at: String,
}

fn default_order_status() -> String {
    "pending".to_owned()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralRule {
    pub plan_code: String,
    pub plan_name: String,
    pub price_minor: i64,
    pub discount_minor: i64,
    pub reward_percent: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralSummary {
    pub enabled: bool,
    pub description: String,
    pub reward_enabled: bool,
    pub contact: String,
    pub invited_count: i64,
    pub pending_reward_minor: i64,
    pub paid_reward_minor: i64,
    pub total_reward_minor: i64,
    pub rules: Vec<CloudReferralRule>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralCode {
    pub id: String,
    pub code: String,
    pub paid_order_count: i64,
    pub reward_minor: i64,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralCodesResult {
    pub total: i64,
    pub items: Vec<CloudReferralCode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralRecord {
    pub id: String,
    pub buyer_label: String,
    pub order_amount_minor: i64,
    pub reward_minor: i64,
    pub reward_percent: i64,
    pub status: String,
    pub created_at: String,
    pub paid_at: Option<String>,
    pub referral_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralRecordsResult {
    pub total: i64,
    pub items: Vec<CloudReferralRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudReferralValidateResult {
    pub valid: bool,
    pub discount_minor: i64,
    pub reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum RemoteTaskStatus {
    Accepted,
    Downloading,
    Paused,
    Completed,
    Failed,
    Canceled,
    #[default]
    #[serde(other)]
    Pending,
}

/// 跨设备任务的 UI 投影。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RemoteTaskDto {
    pub id: String,
    #[serde(default)]
    pub from_device: String,
    #[serde(default)]
    pub to_device: String,
    #[serde(default)]
    pub url: String,
    pub save_dir: Option<String>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub status: RemoteTaskStatus,
    pub total_bytes: Option<i64>,
    #[serde(default)]
    pub downloaded_bytes: i64,
    #[serde(default)]
    pub speed: i64,
    #[serde(default)]
    pub progress: f64,
    pub error: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// UI Gateway 运行状态；永远不携带 token 文本。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatusDto {
    pub takeover_enabled: bool,
    pub jsonrpc_enabled: bool,
    pub api_enabled: bool,
    pub mcp_enabled: bool,
    pub cors_enabled: bool,
    pub user_token_configured: bool,
    /// 当前监听端口（只读；由启动环境决定）。
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    /// 是否对局域网开放兼容 API；修改后下次 agent 启动生效。
    #[serde(default)]
    pub lan_enabled: bool,
}

fn default_gateway_port() -> u16 {
    17800
}

/// 原子修改 agent 托管的兼容网关开关与用户 token。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GatewayPatchParams {
    pub takeover_enabled: Option<bool>,
    pub jsonrpc_enabled: Option<bool>,
    pub api_enabled: Option<bool>,
    pub mcp_enabled: Option<bool>,
    pub cors_enabled: Option<bool>,
    pub lan_enabled: Option<bool>,
    /// `Some("")` 清除用户 token；省略则保持。
    pub user_token: Option<String>,
    /// `true` 生成新的随机用户 token（优先于 `user_token`）。
    #[serde(default)]
    pub regenerate_user_token: bool,
}

/// agent 托盘不可用的原因。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum TrayUnavailableReason {
    /// agent 未编译托盘支持（headless 构建）：关闭全部 UI 后后台恒驻留。
    NotBuilt,
    /// 没有图形会话（无 `DISPLAY` / `WAYLAND_DISPLAY`）。
    NoDisplay,
    /// 桌面环境没有 StatusNotifier 托盘宿主（如未启用 AppIndicator 扩展的 GNOME）。
    NoHost,
    /// 平台托盘初始化失败（如缺少 appindicator 运行库）。
    InitFailed,
}

/// agent 系统外壳状态：托盘可用性，以及关闭全部官方 UI 后后台是否继续驻留。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ShellStatusDto {
    /// 托盘图标可用（`close_to_tray` 偏好可以生效）。
    pub tray_available: bool,
    pub tray_unavailable_reason: Option<TrayUnavailableReason>,
    /// `true`：关闭全部 UI 只退出界面，agent + daemon 继续运行；`false`：随界面一起退出。
    pub resident: bool,
}

/// 完成后关机的调度状态（agent 拥有状态机与执行）。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PowerStatusDto {
    /// 已排定的延迟（秒）；`None` = 未启用。
    pub armed_delay_secs: Option<u64>,
    /// 全部任务结束、真正开始倒计时后的剩余秒数；等待任务完成阶段为 `None`。
    pub countdown_remaining_secs: Option<u64>,
}

/// `agent.power.arm` 参数：全部任务完成后再等待 `delay_secs` 秒关机（0 = 立即）。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PowerArmParams {
    pub delay_secs: u64,
}

/// agent 配置同步状态投影。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct SyncStatusDto {
    pub enabled: bool,
    pub revision: u64,
    pub dirty_keys: Vec<String>,
    pub last_error: Option<String>,
}

/// FluxCloud 服务地址；`editable=false`（正式构建）时 `base_url` 恒等于 `default_base_url`。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudEndpointDto {
    pub base_url: String,
    pub default_base_url: String,
    pub editable: bool,
}

/// `agent.cloud.endpointSet` 参数；`base_url` 为空即恢复默认地址。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CloudEndpointSetParams {
    #[serde(default)]
    pub base_url: String,
}

/// agent 自有偏好设置及其原子版本。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentPreferencesDto {
    pub revision: u64,
    pub values: BTreeMap<String, Value>,
}

/// 等待官方 UI 确认的外部捕获请求。
///
/// 不含 cookie / header / 请求体原文（这些只留在 agent 的捕获事务里，确认时由 agent
/// 合并进建任务参数），只给出携带摘要供 UI 提示。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PendingCaptureDto {
    pub transaction_id: String,
    pub url: String,
    #[serde(default)]
    pub file_name: String,
    pub created_at_unix_ms: i64,
    /// 捕获方声明的文件大小（字节，0 = 未知）。
    #[serde(default)]
    pub file_size: i64,
    /// 来源页面。
    #[serde(default)]
    pub referrer: String,
    /// 捕获方指定的保存目录（空 = 未指定）。
    #[serde(default)]
    pub save_dir: String,
    /// 是否携带浏览器 Cookie。
    #[serde(default)]
    pub has_cookies: bool,
    /// 携带的请求头名（不含值）。
    #[serde(default)]
    pub header_names: Vec<String>,
}

impl PendingCaptureDto {
    /// 浏览器请求已带 `Authorization` 头（HTTP 认证沿用浏览器原值）。
    #[must_use]
    pub fn has_authorization(&self) -> bool {
        self.header_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("authorization"))
    }
}

/// `agent.capture.resolve` 参数。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CaptureResolveParams {
    pub transaction_id: String,
    pub accepted: bool,
    /// 确认时官方 UI 表单产出的建任务参数（`None` = 按捕获原请求建任务）。
    ///
    /// agent 以捕获原请求为底合并：`url` / `method` / `body` / `audioUrl` 恒取捕获值；
    /// `fileName` / `saveDir` / `cookies` / `referrer` 为空时取捕获值；`headers` 以捕获头
    /// 为底、同名（忽略大小写）以表单为准；`userAgent` 非空时替换捕获的 `User-Agent` 头。
    #[serde(default)]
    pub request: Option<crate::daemon::CreateTaskRequest>,
}

/// 桌面系统集成状态（开机自启、`.torrent` 关联、URL scheme 注册）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlatformIntegrationDto {
    pub autostart_supported: bool,
    pub autostart_enabled: bool,
    pub file_association_supported: bool,
    pub torrent_associated: bool,
    pub url_protocol_supported: bool,
    /// scheme → 是否已注册到 FluxDown（`magnet` / `ed2k` / `fluxdown`）。
    pub url_protocols: BTreeMap<String, bool>,
    /// 注册目标可执行文件；空表示未找到桌面程序。
    pub desktop_executable: String,
}

/// `agent.platform.setAutostart` 参数。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlatformToggleParams {
    pub enabled: bool,
}

/// `agent.platform.setUrlProtocol` 参数。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlatformUrlProtocolParams {
    pub scheme: String,
    pub enabled: bool,
}

/// `agent.platform.openPath` 参数。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlatformOpenPathParams {
    pub path: String,
    /// `true` 在文件管理器中定位而非直接打开。
    #[serde(default)]
    pub reveal: bool,
}

/// Doctor 检查项级别。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticLevel {
    #[default]
    Info,
    Ok,
    Warn,
    Error,
}

/// 单条 Doctor 检查结果。`id` 稳定，UI 据此选择标题文案与修复动作。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheckDto {
    pub id: String,
    /// 同一 `id` 下的子目标（如浏览器名、scheme）；无则为空。
    #[serde(default)]
    pub target: String,
    pub level: DiagnosticLevel,
    pub detail: String,
    /// 既有 hint code（如 `HINT_CHECK_DISK`）；无则为空。
    #[serde(default)]
    pub hint: String,
    /// 可用的就地修复动作（`agent.diagnostics.repair` 的 `action`）；无则为 None。
    #[serde(default)]
    pub repair: Option<DiagnosticRepairParams>,
}

/// `agent.diagnostics.repair` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRepairParams {
    pub action: String,
    #[serde(default)]
    pub target: String,
}

/// `agent.diagnostics.run` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReportDto {
    pub generated_at_unix_ms: i64,
    pub app_version: String,
    pub platform: String,
    pub agent_data_dir: String,
    pub daemon_connected: bool,
    pub checks: Vec<DiagnosticCheckDto>,
}

/// `agent.diagnostics.exportLogs` 参数：目标文件路径（`.zip`）。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogExportParams {
    pub target_path: String,
}

/// `agent.diagnostics.exportLogs` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogExportResult {
    pub path: String,
    pub bytes: u64,
}

/// 日志目录信息（`agent.diagnostics.logPaths`）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct LogPathsDto {
    pub agent_log_dir: String,
    pub daemon_log_dir: String,
}

/// `agent.update.check` 参数。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckParams {
    /// `stable` | `frontier`；省略取偏好 `general.update_channel`。
    #[serde(default)]
    pub channel: Option<String>,
}

/// 单个版本的更新说明。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReleaseNoteDto {
    pub version: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub body: String,
}

/// `agent.update.check` 结果。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResultDto {
    pub channel: String,
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    #[serde(default)]
    pub download_url: String,
    #[serde(default)]
    pub release_page_url: String,
    #[serde(default)]
    pub notes: Vec<ReleaseNoteDto>,
}

/// 偏好键：自定义分类列表（JSON 字符串或数组，与 Flutter `custom_categories` 同形）。
pub const CUSTOM_CATEGORIES_PREF_KEY: &str = "custom_categories";

/// 自定义分类（与 `lib/src/models/custom_category.dart` 同 JSON 形状）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct CustomCategoryDto {
    pub id: String,
    pub name: String,
    #[serde(default = "default_category_icon")]
    pub icon: String,
    /// `extension` | `regex`。
    #[serde(default = "default_category_match_mode")]
    pub match_mode: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub regex_pattern: String,
    #[serde(default)]
    pub position: i64,
    #[serde(default = "default_category_visible")]
    pub visible: bool,
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default)]
    pub builtin_type: Option<String>,
    #[serde(default)]
    pub save_dir: String,
}

fn default_category_icon() -> String {
    "file".to_owned()
}
fn default_category_match_mode() -> String {
    "extension".to_owned()
}
fn default_category_visible() -> bool {
    true
}

impl CustomCategoryDto {
    /// 内置分类基线（与 Flutter `CustomCategory.builtinDefaults` 同序同扩展名）。
    #[must_use]
    pub fn builtin_defaults() -> Vec<Self> {
        let make = |id: &str, icon: &str, exts: &[&str], position: i64| Self {
            id: format!("builtin_{id}"),
            name: String::new(),
            icon: icon.to_owned(),
            match_mode: "extension".to_owned(),
            extensions: exts.iter().map(|ext| (*ext).to_owned()).collect(),
            regex_pattern: String::new(),
            position,
            visible: true,
            is_builtin: true,
            builtin_type: Some(id.to_owned()),
            save_dir: String::new(),
        };
        vec![
            make("all", "folders", &[], 0),
            make(
                "video",
                "film",
                &[
                    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "m3u8",
                ],
                1,
            ),
            make(
                "audio",
                "music",
                &["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus"],
                2,
            ),
            make(
                "document",
                "fileText",
                &[
                    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "md",
                ],
                3,
            ),
            make(
                "image",
                "image",
                &[
                    "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "heic", "avif",
                ],
                4,
            ),
            make(
                "program",
                "cpu",
                &["exe", "msi", "dmg", "pkg", "deb", "rpm", "apk", "appimage"],
                5,
            ),
            make(
                "archive",
                "archive",
                &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso"],
                6,
            ),
            make("other", "file", &[], 7),
        ]
    }

    /// 从偏好值解析分类列表（JSON 字符串或数组）；空 / 损坏时回退内置基线，按 `position` 排序。
    #[must_use]
    pub fn from_preference(value: Option<&serde_json::Value>) -> Vec<Self> {
        let parsed = match value {
            Some(serde_json::Value::String(text)) => serde_json::from_str::<Vec<Self>>(text).ok(),
            Some(value) => serde_json::from_value::<Vec<Self>>(value.clone()).ok(),
            None => None,
        };
        let mut list = parsed
            .filter(|list| !list.is_empty())
            .unwrap_or_else(Self::builtin_defaults);
        list.sort_by_key(|entry| entry.position);
        list
    }
}

#[cfg(test)]
mod capture_dto_tests {
    use serde_json::json;

    use super::{CaptureResolveParams, PendingCaptureDto};

    #[test]
    fn resolve_params_without_request_field_deserializes() {
        let params: CaptureResolveParams = serde_json::from_value(json!({
            "transactionId": "tx-1",
            "accepted": true,
        }))
        .expect("deserialize without request");
        assert_eq!(params.transaction_id, "tx-1");
        assert!(params.accepted);
        assert!(params.request.is_none());
    }

    #[test]
    fn pending_capture_without_optional_fields_deserializes_with_defaults() {
        let capture: PendingCaptureDto = serde_json::from_value(json!({
            "transactionId": "tx-2",
            "url": "https://example.com/a.bin",
            "fileName": "a.bin",
            "createdAtUnixMs": 1_700_000_000_000_i64,
        }))
        .expect("deserialize legacy payload without optional fields");
        assert_eq!(capture.transaction_id, "tx-2");
        assert_eq!(capture.file_size, 0);
        assert_eq!(capture.referrer, "");
        assert!(!capture.has_cookies);
        assert!(!capture.has_authorization());
    }

    #[test]
    fn authorization_header_name_matches_case_insensitively() {
        let capture: PendingCaptureDto = serde_json::from_value(json!({
            "transactionId": "tx-3",
            "url": "https://example.com/a.bin",
            "createdAtUnixMs": 0,
            "headerNames": ["accept", "AUTHORIZATION"],
        }))
        .expect("deserialize capture with header names");
        assert!(capture.has_authorization());
    }
}

#[cfg(test)]
mod cloud_profile_tests {
    use serde_json::json;

    use super::CloudProfile;

    /// FluxCloud `GET /me` 把用户字段平铺在顶层（同 Flutter `CloudProfile.fromJson`）。
    #[test]
    fn profile_parses_flat_me_payload() {
        let profile: CloudProfile = serde_json::from_value(json!({
            "id": "u1",
            "email": "user@example.com",
            "nickname": "User",
            "plan": "founder",
            "originId": 88888888,
            "membershipOrdinal": 18,
            "entitlements": { "originIdEdit": true },
            "currentPlan": { "code": "founder", "name": "创始会员", "badge": "创始会员" },
            "purchaseCreditMinor": 0
        }))
        .expect("flat /me payload");
        assert_eq!(profile.user.email, "user@example.com");
        assert_eq!(profile.user.origin_id, Some(88888888));
        assert_eq!(
            profile.current_plan.as_ref().map(|plan| plan.code.as_str()),
            Some("founder")
        );
    }
}
