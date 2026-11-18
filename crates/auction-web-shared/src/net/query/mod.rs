//! URL query 参数与组件状态的双向同步。
//! 只读写当前地址的 query 段，不接管路由。多键共存靠「声明各自管理的键」：写回只替换本
//! hook 声明的键，其余键（其他 hook 或路由持有）原样保留。状态形状与解析/序列化规则由
//! 调用方注入，本模块不含业务键名。

use std::borrow::Cow;
#[cfg(target_arch = "wasm32")]
use std::cell::RefCell;
use std::rc::Rc;

use dioxus::dioxus_core::Runtime;
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

/// 由 surface 路由提供的同页地址读写入口。写入返回 `true` 时，路由也已收到新地址。
pub trait QueryLocation {
    /// 当前根相对地址，包含查询和片段。
    fn current(&self) -> String;

    /// 按给定模式提交同页地址，保留视口位置。
    fn write(&self, location: &str, mode: QueryWriteMode) -> bool;
}

/// 取得当前组件所属路由的地址读写入口，供该 surface 的查询状态复用。
pub fn query_location() -> Option<Rc<dyn QueryLocation>> {
    Runtime::try_current()?.try_current_scope_id()?;
    try_consume_context()
}

/// 以首个分隔符拆出路径、查询和片段，查询和片段各自保留其分隔符。
pub fn location_parts(location: &str) -> (&str, &str, &str) {
    let (path_query, fragment) = location.split_at(location.find('#').unwrap_or(location.len()));
    let (path, search) = path_query.split_at(path_query.find('?').unwrap_or(path_query.len()));
    (path, search, fragment)
}

/// 从完整 search 中只剥掉第一个问号，其余问号属于查询键或值。
pub fn parse_query_pairs(search: &str) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    let raw = search.strip_prefix('?').unwrap_or_default();
    url::form_urlencoded::parse(raw.as_bytes())
}

/// 读取当前地址栏的全部 query 键值对；没有路由上下文的非 WASM 构建返回空列表。
pub fn read_query_pairs() -> Vec<(String, String)> {
    if let Some(location) = query_location() {
        return owned_query_pairs(location_parts(&location.current()).1);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return Vec::new();
        };
        let search = window.location().search().unwrap_or_default();
        owned_query_pairs(&search)
    }

    #[cfg(not(target_arch = "wasm32"))]
    Vec::new()
}

fn owned_query_pairs(search: &str) -> Vec<(String, String)> {
    parse_query_pairs(search).map(|(key, value)| (key.into_owned(), value.into_owned())).collect()
}

/// 把 `pairs` 合并写回当前地址的 query 段。
/// `keys` 声明本次写回管理的键：这些键的旧值整体替换为 `pairs`，其余键原样保留。
/// 返回地址栏写入是否成功。没有路由上下文的非 WASM 构建返回 `false`。
pub fn write_query_pairs(keys: &[&str], pairs: &[(String, String)], mode: QueryWriteMode) -> bool {
    if let Some(location) = query_location() {
        return location.write(&patch_query(&location.current(), keys, pairs, false), mode);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Ok(history) = window.history() else {
            return false;
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
        write_history_entry(&history, &state, Some(&url), mode)
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (keys, pairs, mode);
        false
    }
}

/// 经 surface 路由替换指定查询键，连同空段保留其余查询片段。没有路由上下文时拒绝写入。
pub fn write_query_pairs_preserving_segments(
    keys: &[&str],
    pairs: &[(String, String)],
    mode: QueryWriteMode,
) -> bool {
    query_location().is_some_and(|location| {
        location.write(&patch_query(&location.current(), keys, pairs, true), mode)
    })
}

fn patch_query(
    location: &str,
    keys: &[&str],
    pairs: &[(String, String)],
    preserve_empty: bool,
) -> String {
    let (path, search, hash) = location_parts(location);
    let raw = search.strip_prefix('?').unwrap_or_default();
    let mut fields: Vec<_> = raw
        .split('&')
        .filter(|field| {
            (preserve_empty && !raw.is_empty() && field.is_empty())
                || url::form_urlencoded::parse(field.as_bytes())
                    .next()
                    .is_some_and(|(key, _)| !keys.contains(&key.as_ref()))
        })
        .collect();
    let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs).finish();
    if !encoded.is_empty() {
        fields.push(&encoded);
    }
    if fields.is_empty() {
        format!("{path}{hash}")
    } else {
        format!("{path}?{}{hash}", fields.join("&"))
    }
}

/// 向浏览器提交历史项。调用方在成功后才发布地址和查询状态。
#[cfg(target_arch = "wasm32")]
pub fn write_history_entry(
    history: &web_sys::History,
    state: &JsValue,
    url: Option<&str>,
    mode: QueryWriteMode,
) -> bool {
    match mode {
        QueryWriteMode::Push => history.push_state_with_url(state, "", url).is_ok(),
        QueryWriteMode::Replace => history.replace_state_with_url(state, "", url).is_ok(),
    }
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
        use_popstate_listener(on_popstate);
    }

    QueryState { state, keys, serialize }
}

/// 浏览器前进/后退时递增调用方持有的位置版本；组件卸载时移除监听。
/// surface 只按版本变化重读自身状态，本 hook 不解析地址，也不接管路由。非 WASM 构建为空操作。
pub fn use_popstate_revision(revision: Signal<u64>) {
    #[cfg(target_arch = "wasm32")]
    {
        let mut revision = revision;
        use_popstate_listener(use_callback(move |()| {
            revision.set(revision().saturating_add(1));
        }));
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = revision;
}

/// 每个组件只安装一次 popstate 监听，卸载时由守卫移除；回调经 `Callback` 恢复创建组件的 Scope。
#[cfg(target_arch = "wasm32")]
fn use_popstate_listener(on_popstate: Callback<()>) {
    let guard = use_hook(|| Rc::new(RefCell::new(None::<PopstateGuard>)));
    let cleanup_guard = Rc::clone(&guard);
    use_drop(move || {
        if let Some(guard) = cleanup_guard.borrow_mut().take() {
            guard.cleanup();
        }
    });
    use_effect(move || {
        // effect 无响应式依赖不会重跑，守卫已存在时直接返回。
        if guard.borrow().is_some() {
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
            guard.replace(Some(PopstateGuard { window, callback }));
        }
    });
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

#[cfg(test)]
mod tests {
    use super::{location_parts, owned_query_pairs, patch_query};

    #[test]
    fn shared_query_patch_preserves_literal_question_marks_and_fragment() {
        let size = [("page_size".to_owned(), "50".to_owned())];
        let next = patch_query("/??page=2&ref=x#title", &["page", "page_size"], &size, false);
        assert_eq!(next, "/??page=2&ref=x&page_size=50#title");
        assert_eq!(
            owned_query_pairs(location_parts(&next).1),
            [
                ("?page".to_owned(), "2".to_owned()),
                ("ref".to_owned(), "x".to_owned()),
                size[0].clone()
            ]
        );
        assert_eq!(
            patch_query("/?page=2#title", &["page", "page_size"], &size, false),
            "/?page_size=50#title"
        );
    }

    #[test]
    fn scoped_patch_preserves_other_raw_segments_including_empty_ones() {
        let initial = "/?world=main&%74ype=%63lo&&keep=%2b%20x&keep=#lots";
        let pairs = [("world".to_owned(), "isekai".to_owned())];
        assert_eq!(
            patch_query(initial, &["world"], &pairs, true),
            "/?%74ype=%63lo&&keep=%2b%20x&keep=&world=isekai#lots",
        );
        assert_eq!(
            patch_query(initial, &["world"], &pairs, false),
            "/?%74ype=%63lo&keep=%2b%20x&keep=&world=isekai#lots",
        );
        assert_eq!(patch_query("/#lots", &["world"], &pairs, true), "/?world=isekai#lots");
    }
}
