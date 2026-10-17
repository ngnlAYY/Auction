//! URL query 参数与组件状态的双向同步。
//! 只读写当前地址的 query 段，不接管路由。多键共存靠「声明各自管理的键」：写回只替换本
//! hook 声明的键，其余键（其他 hook 或路由持有）原样保留。状态形状与解析/序列化规则由
//! 调用方注入，本模块不含业务键名。

use std::rc::Rc;

#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure};

/// 写回地址栏时创建还是替换浏览器历史项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryWriteMode {
    /// 用户主动切换筛选/分页：创建可返回的浏览器历史项。
    Push,
    /// 程序化纠偏（canonical 化、越界钳制等）：替换当前项，不污染历史栈。
    Replace,
}

/// 读取当前地址栏的全部 query 键值对；非 WASM 构建恒为空。
pub fn read_query_pairs() -> Vec<(String, String)> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Vec::new();
        };
        let search = window.location().search().unwrap_or_default();
        let raw = search.strip_prefix('?').unwrap_or_default();
        url::form_urlencoded::parse(raw.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

/// 把 `pairs` 合并写回当前地址的 query 段。
/// `keys` 声明本次写回管理的键：这些键的旧值整体替换为 `pairs`，其余键原样保留。
/// 非 WASM 构建为空操作。
pub fn write_query_pairs(keys: &[&str], pairs: &[(String, String)], mode: QueryWriteMode) {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(history) = window.history() else {
            return;
        };
        let location = window.location();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in read_query_pairs() {
            if !keys.contains(&key.as_str()) {
                serializer.append_pair(&key, &value);
            }
        }
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        let encoded = serializer.finish();
        let pathname = location.pathname().unwrap_or_else(|_| "/".to_owned());
        let hash = location.hash().unwrap_or_default();
        let url = if encoded.is_empty() {
            format!("{pathname}{hash}")
        } else {
            format!("{pathname}?{encoded}{hash}")
        };
        // 保留既有 history state（Router 可能在其中记录滚动位置等信息）。
        let state = history.state().unwrap_or(JsValue::NULL);
        match mode {
            QueryWriteMode::Push => {
                let _ = history.push_state_with_url(&state, "", Some(&url));
            }
            QueryWriteMode::Replace => {
                let _ = history.replace_state_with_url(&state, "", Some(&url));
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    let _ = (keys, pairs, mode);
}

/// 状态 → 写回键值对的序列化器（省略等于默认值的键）。
type SerializeQuery<T> = dyn Fn(&T) -> Vec<(String, String)>;

/// 全部 query 键值对 → 状态的解析器（挂载初值与 popstate 恢复共用）。
type ParseQuery<T> = dyn Fn(&[(String, String)]) -> T;

/// [`use_query_state`] 返回的句柄：一个随地址栏双向同步的状态信号。
#[derive(Clone)]
pub struct QueryState<T: 'static> {
    /// 当前状态；挂载时取自地址栏初值，浏览器前进/后退（popstate）时自动恢复。
    pub state: Signal<T>,
    keys: &'static [&'static str],
    serialize: Rc<SerializeQuery<T>>,
}

impl<T: 'static> QueryState<T> {
    /// 更新状态并按 `mode` 写回地址栏（只替换本 hook 声明管理的键）。
    pub fn set(&mut self, value: T, mode: QueryWriteMode) {
        write_query_pairs(self.keys, &(self.serialize)(&value), mode);
        self.state.set(value);
    }

    /// 用户主动变更：更新状态并创建可返回的浏览器历史项。
    pub fn push(&mut self, value: T) {
        self.set(value, QueryWriteMode::Push);
    }

    /// 程序化纠偏：更新状态并替换当前历史项。
    pub fn replace(&mut self, value: T) {
        self.set(value, QueryWriteMode::Replace);
    }

    /// 只更新状态信号、不写地址栏（用于从地址栏恢复等内部路径）。
    pub fn set_silently(&mut self, value: T) {
        self.state.set(value);
    }
}

/// 挂载与 URL query 双向同步的状态。
///
/// `keys` 是本 hook 管理的 query 键，写回只替换这些键；`parse` 与 popstate 恢复走同一解析路径，
/// 失败回退默认值；`serialize` 输出键值对并省略等于默认值的键。非 WASM 构建不触碰浏览器地址，
/// 初值为 `parse(&[])`，写回只更新状态信号。
pub fn use_query_state<T: Clone + PartialEq + 'static>(
    keys: &'static [&'static str],
    parse: impl Fn(&[(String, String)]) -> T + 'static,
    serialize: impl Fn(&T) -> Vec<(String, String)> + 'static,
) -> QueryState<T> {
    let parse = use_hook(|| Rc::new(parse) as Rc<ParseQuery<T>>);
    let serialize = use_hook(|| Rc::new(serialize) as Rc<SerializeQuery<T>>);
    let state = use_signal({
        let parse = Rc::clone(&parse);
        move || parse(&read_query_pairs())
    });

    #[cfg(target_arch = "wasm32")]
    {
        // parse 可能读取调用方的 Context；原生事件必须恢复创建组件的 Runtime 与 Scope，
        // 让浏览器前进/后退与挂载初值使用相同的解析上下文。
        let on_popstate = use_callback({
            let parse = Rc::clone(&parse);
            let mut state = state;
            move |()| {
                let value = parse(&read_query_pairs());
                if value != *state.peek() {
                    state.set(value);
                }
            }
        });
        let guard = use_hook(|| Rc::new(RefCell::new(None::<PopstateGuard>)));
        let cleanup_guard = Rc::clone(&guard);
        use_drop(move || {
            if let Some(guard) = cleanup_guard.borrow_mut().take() {
                guard.cleanup();
            }
        });
        let effect_guard = Rc::clone(&guard);
        use_effect(move || {
            // 只安装一次监听；effect 重跑（本 hook 无响应式依赖，不会重跑）时守卫已存在。
            if effect_guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else {
                return;
            };
            let callback = Closure::<dyn FnMut()>::new(move || {
                on_popstate.call(());
            });
            if window
                .add_event_listener_with_callback("popstate", callback.as_ref().unchecked_ref())
                .is_ok()
            {
                effect_guard.replace(Some(PopstateGuard { window, callback }));
            }
        });
    }

    QueryState { state, keys, serialize }
}

/// popstate 监听的生命周期守卫：组件卸载时移除监听并释放闭包。
#[cfg(target_arch = "wasm32")]
struct PopstateGuard {
    window: web_sys::Window,
    callback: Closure<dyn FnMut()>,
}

#[cfg(target_arch = "wasm32")]
impl PopstateGuard {
    fn cleanup(self) {
        let _ = self.window.remove_event_listener_with_callback(
            "popstate",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}
