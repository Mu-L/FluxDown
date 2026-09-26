//! 窗口注册表与界面进程退出判定。
//!
//! 所有顶层窗口按 [`WindowKey`] 去重；最后一个用户窗口关闭时界面进程退出（后台是否驻留由
//! agent 决定，见 `crate::lifecycle`）。gpui 不会在最后一个窗口关闭时自动退出，这里是唯一的
//! 「关窗即退出」判定点。

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gpui::{
    AnyWindowHandle, App, Bounds, Context, DisplayId, Entity, Global, Pixels, Render, Size,
    Subscription, Window, WindowBounds, WindowHandle, WindowId, WindowOptions, point, px, size,
};
use gpui_component::WindowExt as _;
use serde_json::{Value, json};

use crate::{agent_client::AgentClient, app::Desktop};

pub mod group_detail;
pub mod main;
pub mod new_download;
pub mod progress;
pub mod queue_manager;
pub mod selection;
pub mod settings;
pub mod task_detail;

/// 顶层窗口身份：同 key 只允许一个实例。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum WindowKey {
    Main,
    Settings,
    NewDownload,
    QueueManager,
    Selection(String),
    TaskDetail(String),
    GroupDetail(String),
    /// 独立下载进度 / 完成窗口（每任务一个）。
    Progress(String),
}

impl WindowKey {
    /// 边界持久化偏好键；只有主窗口与设置窗口持久化。
    fn bounds_pref_key(&self) -> Option<&'static str> {
        match self {
            Self::Main => Some("desktop.window.main"),
            Self::Settings => Some("desktop.window.settings"),
            _ => None,
        }
    }
}

pub struct WindowRegistry {
    open: HashMap<WindowKey, AnyWindowHandle>,
    ids: HashMap<WindowId, WindowKey>,
    /// 正在显示「下载仍在进行」确认框的窗口：重复 ⌘W / ⌘Q 不叠第二个对话框。
    confirming: HashSet<WindowId>,
    /// 防抖中尚未落盘的窗口边界（退出时强制写一次）。
    pending_bounds: Rc<RefCell<HashMap<&'static str, Value>>>,
    _closed_sub: Subscription,
    _quit_sub: Subscription,
}

impl Global for WindowRegistry {}

impl WindowRegistry {
    /// 安装全局注册表：窗口关闭清理 + 退出判定 + 退出时落盘边界。
    pub fn init(cx: &mut App, client: Arc<AgentClient>) {
        let closed_sub = cx.on_window_closed(|cx, window_id| {
            let last_closed = {
                let registry = cx.global_mut::<Self>();
                if let Some(key) = registry.ids.remove(&window_id) {
                    registry.open.remove(&key);
                }
                registry.confirming.remove(&window_id);
                registry.open.is_empty()
            };
            if last_closed {
                crate::lifecycle::quit_ui(cx);
            }
        });
        let pending_bounds = Rc::new(RefCell::new(HashMap::new()));
        let quit_pending = Rc::clone(&pending_bounds);
        let quit_sub = cx.on_app_quit(move |_| {
            let pending: HashMap<&'static str, Value> = quit_pending.borrow_mut().drain().collect();
            let client = Arc::clone(&client);
            async move {
                for (key, value) in pending {
                    let _ = patch_local_preference(&client, key, value).await;
                }
            }
        });
        cx.set_global(Self {
            open: HashMap::new(),
            ids: HashMap::new(),
            confirming: HashSet::new(),
            pending_bounds,
            _closed_sub: closed_sub,
            _quit_sub: quit_sub,
        });
    }

    /// 打开或聚焦窗口。已开 → `activate_window` 并返回 `None`。
    pub fn open_or_focus<V: Render>(
        cx: &mut App,
        key: WindowKey,
        options: WindowOptions,
        build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Option<WindowHandle<V>> {
        if let Some(handle) = cx.global::<Self>().open.get(&key).copied() {
            if handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
            {
                return None;
            }
            let registry = cx.global_mut::<Self>();
            registry.open.remove(&key);
            registry.ids.remove(&handle.window_id());
        }
        match cx.open_window(crate::app_icon::window_options(options), build) {
            Ok(handle) => {
                let any: AnyWindowHandle = handle.into();
                let registry = cx.global_mut::<Self>();
                registry.ids.insert(any.window_id(), key.clone());
                registry.open.insert(key, any);
                Some(handle)
            }
            Err(error) => {
                eprintln!("failed to open FluxDown window: {error:#}");
                None
            }
        }
    }

    pub fn close(cx: &mut App, key: &WindowKey) {
        if let Some(handle) = cx.global::<Self>().open.get(key).copied() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    #[must_use]
    pub fn is_open(cx: &App, key: &WindowKey) -> bool {
        cx.global::<Self>().open.contains_key(key)
    }

    #[must_use]
    pub fn handle(cx: &App, key: &WindowKey) -> Option<AnyWindowHandle> {
        cx.global::<Self>().open.get(key).copied()
    }

    /// 主窗口所在的显示器；无主窗口时调用方应让 GPUI 回退到主显示器。
    #[must_use]
    pub fn main_display_id(cx: &mut App) -> Option<DisplayId> {
        Self::handle(cx, &WindowKey::Main)
            .and_then(|handle| {
                handle
                    .update(cx, |_, window, cx| window.display(cx))
                    .ok()
                    .flatten()
            })
            .map(|display| display.id())
    }

    /// 窗口 id → 注册 key（未注册的窗口返回 `None`）。
    #[must_use]
    pub fn key_of(cx: &App, id: WindowId) -> Option<WindowKey> {
        cx.global::<Self>().ids.get(&id).cloned()
    }

    /// 当前获得焦点的窗口。macOS 的 `cx.active_window()` 只认 `NSWindow`，`Floating` /
    /// `PopUp` 是 `NSPanel`（选择框）会返回 `None`，此时按 gpui 记录的 key 态扫描。
    /// 只能在 defer 之后调用：正在 update 栈内的窗口不在 `cx.windows` 里，扫描会漏掉它。
    #[must_use]
    pub fn focused_window(cx: &mut App) -> Option<AnyWindowHandle> {
        cx.active_window().or_else(|| {
            cx.windows().into_iter().find(|handle| {
                handle
                    .update(cx, |_, window, _| window.is_window_active())
                    .unwrap_or(false)
            })
        })
    }

    /// 关闭当前活动窗口：主窗口走 [`main::should_close`] 关闭策略（与原生关闭按钮一致，
    /// `remove_window` 不会触发 `windowShouldClose:`），其他窗口直接关。
    ///
    /// 键盘触发时正处于该窗口自己的 update 栈内（窗口已被从 `cx.windows` 取走），同步
    /// `handle.update` 会拿不到窗口而静默失败；必须 defer 到本轮 update 结束再操作。
    pub fn close_active_window(cx: &mut App) {
        cx.defer(|cx| {
            let Some(handle) = Self::focused_window(cx) else {
                return;
            };
            let is_main = Self::key_of(cx, handle.window_id()) == Some(WindowKey::Main);
            let _ = handle.update(cx, |_, window, cx| {
                if !is_main || main::should_close(window, cx) {
                    window.remove_window();
                }
            });
        });
    }

    /// 用户窗口数量。
    #[must_use]
    pub fn open_count(cx: &App) -> usize {
        cx.global::<Self>().open.len()
    }

    /// 满足条件的已开窗口数量。
    #[must_use]
    pub fn count(cx: &App, predicate: impl Fn(&WindowKey) -> bool) -> usize {
        cx.global::<Self>()
            .open
            .keys()
            .filter(|key| predicate(key))
            .count()
    }

    /// 在根视图上挂窗口边界观察，500ms 防抖后写入设备本地偏好。
    pub fn persist_bounds<V: 'static>(
        key: &WindowKey,
        client: Arc<AgentClient>,
        root: &Entity<V>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(pref_key) = key.bounds_pref_key() else {
            return;
        };
        let pending = Rc::clone(&cx.global::<Self>().pending_bounds);
        let generation = Rc::new(Cell::new(0_u64));
        root.update(cx, |_, cx: &mut Context<V>| {
            cx.observe_window_bounds(window, move |_, window, cx| {
                let value = bounds_to_value(window.window_bounds());
                pending.borrow_mut().insert(pref_key, value.clone());
                let current = generation.get().wrapping_add(1);
                generation.set(current);
                let generation = Rc::clone(&generation);
                let pending = Rc::clone(&pending);
                let client = Arc::clone(&client);
                cx.spawn(async move |_, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    if generation.get() != current {
                        return;
                    }
                    pending.borrow_mut().remove(pref_key);
                    let _ = patch_local_preference(&client, pref_key, value).await;
                })
                .detach();
            })
            .detach();
        });
    }

    /// 从偏好恢复窗口边界；不可见 / 无记录时居中。
    #[must_use]
    pub fn restore_bounds(
        key: &WindowKey,
        preferences: Option<&Value>,
        default_size: Size<Pixels>,
        cx: &App,
    ) -> WindowBounds {
        let stored = key
            .bounds_pref_key()
            .and(preferences)
            .and_then(value_to_bounds);
        let displays: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
        match stored {
            Some((bounds, maximized)) if bounds_visible(&bounds, &displays) => {
                if maximized {
                    WindowBounds::Maximized(bounds)
                } else {
                    WindowBounds::Windowed(bounds)
                }
            }
            _ => WindowBounds::Windowed(Bounds::centered(None, default_size, cx)),
        }
    }
}

/// 把窗口连同应用一起置前（界面常是后台应用：外部捕获、静默下载发生时浏览器在前台）：
///
/// - macOS：`activate_window` 只在本应用内排序（`makeKeyAndOrderFront`），必须同时
///   `cx.activate(true)` 激活应用本身（`activateIgnoringOtherApps`），否则窗口压在浏览器下面。
/// - Windows：`cx.activate` 为空操作；gpui 的 `activate_window` 以 `SetForegroundWindow` +
///   模拟一次按键输入绕过前台锁，后台进程也能置前。
/// - Linux：X11 发 `_NET_ACTIVE_WINDOW`、Wayland 申请 xdg-activation token，窗口管理器的
///   防抢焦点策略可能拒绝；再请求注意（X11 置 urgency，任务栏 / 工作区高亮），被拒时
///   用户仍能看到有待确认的下载。
pub fn bring_to_front(window: &mut Window, cx: &mut App) {
    cx.activate(true);
    window.activate_window();
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    window.request_attention();
}

/// 「下载仍在进行」确认框：用户确认后执行 `on_ok`（关窗 / 退出）。同一窗口已在提示中
/// （重复 ⌘W / ⌘Q、再点关闭按钮）则不再叠第二个对话框。
pub fn confirm_active_tasks(
    window: &mut Window,
    cx: &mut App,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let id = window.window_handle().window_id();
    if !cx.global_mut::<WindowRegistry>().confirming.insert(id) {
        return;
    }
    let translator = Desktop::global(cx).translator.read(cx).clone();
    let title = translator.text("closeWithActiveTasksTitle").to_owned();
    let hint = translator.text("closeWithActiveTasksHint").to_owned();
    let ok = translator.text("menuQuit").to_owned();
    let cancel = translator.text("cancel").to_owned();
    let on_ok = Rc::new(on_ok);
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let on_ok = Rc::clone(&on_ok);
        dialog
            .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
            .description(hint.clone())
            .footer(fluxdown_ui_components::dialog_footer(
                Some(cancel.clone().into()),
                ok.clone(),
                fluxdown_ui_components::DialogIntent::Confirm,
                cx,
            ))
            .on_ok(move |_, window, cx| {
                on_ok(window, cx);
                true
            })
            .on_close(move |_, _, cx| {
                cx.global_mut::<WindowRegistry>().confirming.remove(&id);
            })
    });
}

/// `agent.preferences.patch` 设备本地写入（`sync:false`，不进云同步）。
pub async fn patch_local_preference(
    client: &AgentClient,
    key: &str,
    value: Value,
) -> Result<(), fluxdown_protocol::RpcErrorData> {
    client
        .call::<Value, Value>(
            fluxdown_protocol::method::AGENT_PREFERENCES_PATCH,
            Some(json!({ "values": { key: value }, "sync": false })),
        )
        .await
        .map(|_| ())
}

fn bounds_to_value(bounds: WindowBounds) -> Value {
    let (rect, maximized) = match bounds {
        WindowBounds::Windowed(rect) => (rect, false),
        WindowBounds::Maximized(rect) | WindowBounds::Fullscreen(rect) => (rect, true),
    };
    json!({
        "x": f64::from(rect.origin.x),
        "y": f64::from(rect.origin.y),
        "w": f64::from(rect.size.width),
        "h": f64::from(rect.size.height),
        "maximized": maximized,
    })
}

fn value_to_bounds(value: &Value) -> Option<(Bounds<Pixels>, bool)> {
    let num = |key: &str| value.get(key).and_then(Value::as_f64);
    let (x, y, w, h) = (num("x")?, num("y")?, num("w")?, num("h")?);
    let maximized = value
        .get("maximized")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some((
        Bounds {
            origin: point(px(x as f32), px(y as f32)),
            size: size(px(w as f32), px(h as f32)),
        },
        maximized,
    ))
}

/// 与 Flutter `window_state_service` 同规则：坐标在 -500..20000、与任一显示器相交且
/// 可见区域 ≥ 100×100。
fn bounds_visible(bounds: &Bounds<Pixels>, displays: &[Bounds<Pixels>]) -> bool {
    let (x, y) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if !(-500.0..20_000.0).contains(&x)
        || !(-500.0..20_000.0).contains(&y)
        || !(100.0..20_000.0).contains(&w)
        || !(100.0..20_000.0).contains(&h)
    {
        return false;
    }
    displays.iter().any(|display| {
        if !display.intersects(bounds) {
            return false;
        }
        let visible = display.intersect(bounds);
        f32::from(visible.size.width) >= 100.0 && f32::from(visible.size.height) >= 100.0
    })
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, point, px, size};

    use super::{bounds_visible, value_to_bounds};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    #[test]
    fn offscreen_or_barely_visible_bounds_are_rejected() {
        let displays = [rect(0., 0., 1920., 1080.)];
        assert!(bounds_visible(&rect(100., 100., 800., 600.), &displays));
        assert!(!bounds_visible(&rect(1900., 1000., 800., 600.), &displays));
        assert!(!bounds_visible(&rect(-600., 100., 800., 600.), &displays));
        assert!(!bounds_visible(&rect(3000., 100., 800., 600.), &displays));
        assert!(!bounds_visible(&rect(100., 100., 50., 600.), &displays));
    }

    #[test]
    fn stored_bounds_round_trip_maximized_flag() {
        let value =
            serde_json::json!({"x": 10.0, "y": 20.0, "w": 800.0, "h": 600.0, "maximized": true});
        let (bounds, maximized) = value_to_bounds(&value).expect("bounds");
        assert_eq!(bounds, rect(10., 20., 800., 600.));
        assert!(maximized);
        assert!(value_to_bounds(&serde_json::json!({"x": 1})).is_none());
    }
}
