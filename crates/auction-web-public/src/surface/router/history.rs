//! Public 路由的完整地址与查询写回共用同一份浏览器历史。

use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;

use auction_web_shared::QueryWriteMode;
#[cfg(target_arch = "wasm32")]
use auction_web_shared::query::write_history_entry;
use auction_web_shared::query::{QueryLocation, location_parts};
use dioxus::history::History;
use dioxus::prelude::Routable;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;

use super::PublicHistoryEntry;

type WriteLocation = dyn Fn(&str, QueryWriteMode, bool) -> bool;
type RouterUpdate = Arc<dyn Fn() + Send + Sync>;

/// 适配 Dioxus 的类型化路由，地址只规范路径，查询与片段保留原来字节。
pub(super) struct PublicHistory<R> {
    inner: Rc<dyn History>,
    write_location: Box<WriteLocation>,
    initial: RefCell<Option<String>>,
    notify: RefCell<Option<RouterUpdate>>,
    entry: PublicHistoryEntry,
    route: PhantomData<R>,
}

impl<R: Routable> PublicHistory<R> {
    pub(super) fn new(
        inner: Rc<dyn History>,
        write_location: impl Fn(&str, QueryWriteMode, bool) -> bool + 'static,
    ) -> Self {
        Self {
            inner,
            write_location: Box::new(write_location),
            initial: RefCell::default(),
            notify: RefCell::default(),
            entry: PublicHistoryEntry::default(),
            route: PhantomData,
        }
    }

    pub(super) fn entry(&self) -> PublicHistoryEntry {
        self.entry.clone()
    }

    fn navigate(&self, location: &str, mode: QueryWriteMode, preserve_scroll: bool) -> bool {
        let canonical = canonical_location::<R>(location);
        if canonical == self.current_route()
            || !(self.write_location)(&canonical, mode, preserve_scroll)
        {
            return false;
        }
        if mode == QueryWriteMode::Push {
            self.entry.advance();
        }
        true
    }
}

impl<R: Routable> History for PublicHistory<R> {
    fn current_route(&self) -> String {
        self.inner.current_route()
    }

    fn current_prefix(&self) -> Option<String> {
        self.inner.current_prefix()
    }

    fn go_back(&self) {
        if self.inner.can_go_back() {
            self.entry.advance();
        }
        self.inner.go_back();
    }

    fn go_forward(&self) {
        if self.inner.can_go_forward() {
            self.entry.advance();
        }
        self.inner.go_forward();
    }

    fn can_go_back(&self) -> bool {
        self.inner.can_go_back()
    }

    fn can_go_forward(&self) -> bool {
        self.inner.can_go_forward()
    }

    fn external(&self, url: String) -> bool {
        self.inner.external(url)
    }

    fn push(&self, location: String) {
        self.navigate(&location, QueryWriteMode::Push, false);
    }

    fn replace(&self, location: String) {
        // Dioxus 在安装 updater 后，立即用类型化路由替换首个不匹配的地址。该地址不包含
        // 未声明的查询和片段，必须以启动时的完整地址为准，只规范化 pathname。
        let initial = self.initial.borrow_mut().take();
        let target = initial.as_deref().unwrap_or(&location);
        self.navigate(target, QueryWriteMode::Replace, initial.is_some());
    }

    fn updater(&self, callback: RouterUpdate) {
        let initial = self.current_route();
        if initial.parse::<R>().is_ok_and(|route| route.to_string() != initial) {
            self.initial.replace(Some(initial));
        }
        let entry = self.entry();
        let on_pop = Arc::clone(&callback);
        self.inner.updater(Arc::new(move || {
            entry.advance();
            on_pop();
        }));
        self.notify.replace(Some(callback));
    }
}

impl<R: Routable> QueryLocation for PublicHistory<R> {
    fn current(&self) -> String {
        self.current_route()
    }

    fn write(&self, location: &str, mode: QueryWriteMode) -> bool {
        if !self.navigate(location, mode, true) {
            return false;
        }
        // pushState/replaceState 没有 popstate。写成功才让路由及各查询订阅者重读地址。
        // 先复制回调并释放借用，路由重建触发 updater() 时不会碰到已借用的 RefCell。
        let notify = self.notify.borrow().clone();
        if let Some(notify) = notify {
            notify();
        }
        true
    }
}

fn canonical_location<R: Routable>(location: &str) -> String {
    let (path, search, hash) = location_parts(location);
    // 只有能往返为相同路径的路由才能去除尾斜杠。忽略路由模板附加的空查询标记。
    let path = path
        .strip_suffix('/')
        .filter(|candidate| {
            !candidate.is_empty()
                && !candidate.starts_with("//")
                && candidate
                    .parse::<R>()
                    .is_ok_and(|route| location_parts(&route.to_string()).0 == *candidate)
        })
        .unwrap_or(path);
    let search = if search == "?" { "" } else { search };
    format!("{path}{search}{hash}")
}

#[cfg(target_arch = "wasm32")]
pub(super) fn write_browser_location(
    location: &str,
    mode: QueryWriteMode,
    preserve_scroll: bool,
) -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Ok(history) = window.history() else { return false };
    let scroll = js_sys::Array::new();
    scroll.push(&JsValue::from_f64(window.scroll_x().unwrap_or_default()));
    scroll.push(&JsValue::from_f64(window.scroll_y().unwrap_or_default()));
    if !write_location_with_scroll(
        location,
        mode,
        preserve_scroll,
        scroll.into(),
        || history.state().unwrap_or(JsValue::NULL),
        |mode, state, location| write_history_entry(&history, state, location, mode),
    ) {
        return false;
    }
    // 带片段的新导航由章节定位决定落点，避免先滚顶触发对等待的取消。
    if !preserve_scroll && location_parts(location).2.is_empty() {
        window.scroll_to_with_x_and_y(0.0, 0.0);
    }
    true
}

/// 将视口位置与地址写入浏览器条目，坐标格式沿用 Dioxus 的 `[x, y]`。
fn write_location_with_scroll<T>(
    location: &str,
    mode: QueryWriteMode,
    preserve_scroll: bool,
    scroll: T,
    previous_state: impl FnOnce() -> T,
    mut write_entry: impl FnMut(QueryWriteMode, &T, Option<&str>) -> bool,
) -> bool {
    if mode == QueryWriteMode::Push {
        // 即使同页导航保留视口，旧条目也要记住用户离开时的实时坐标。
        write_entry(QueryWriteMode::Replace, &scroll, None);
    }
    let state =
        if preserve_scroll && mode == QueryWriteMode::Replace { previous_state() } else { scroll };
    write_entry(mode, &state, Some(location))
}

#[cfg(test)]
mod tests {
    use auction_web_shared::QueryWriteMode;

    use super::write_location_with_scroll;

    #[test]
    fn back_from_an_in_place_home_reset_restores_the_departure_scroll() {
        let from = "/?page=2&type=one";
        let mut entries = vec![(from.to_owned(), [0, 15])];
        let scrolled = [0, 1075];
        assert!(write_location_with_scroll(
            "/?page_size=20&movement_page_size=20",
            QueryWriteMode::Push,
            true,
            scrolled,
            || panic!("新条目不继承过期滚动位置"),
            |mode, position, location| {
                match mode {
                    QueryWriteMode::Replace => {
                        assert!(location.is_none());
                        entries.last_mut().unwrap().1 = *position;
                    }
                    QueryWriteMode::Push => entries.push((location.unwrap().to_owned(), *position)),
                }
                true
            },
        ));
        entries.pop();
        assert_eq!(entries.last(), Some(&(from.to_owned(), scrolled)));
    }
}
