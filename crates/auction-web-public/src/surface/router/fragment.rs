//! 页内片段只在对应章节首次可见时定位，随后把滚动控制权交还用户。

use std::borrow::Cow;
#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
#[cfg(target_arch = "wasm32")]
use std::rc::{Rc, Weak};

use auction_web_shared::query::location_parts;
#[cfg(target_arch = "wasm32")]
use dioxus::prelude::History;
use dioxus::router::exports::percent_encoding::percent_decode_str;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};
#[cfg(target_arch = "wasm32")]
use web_sys::{Element, Event, MutationObserver, MutationObserverInit, Window};

#[derive(Default)]
struct FragmentTarget {
    destination: Option<(String, String)>,
    waiting: bool,
    position: (f64, f64),
}

impl FragmentTarget {
    fn enter(&mut self, location: &str, position: (f64, f64)) -> bool {
        let (path, _, hash) = location_parts(location);
        let destination = fragment_id(hash).map(|id| (path.to_owned(), id.into_owned()));
        if self.destination == destination {
            return false;
        }
        self.waiting = destination.is_some();
        self.destination = destination;
        self.position = position;
        true
    }

    fn pending_id(&self) -> Option<&str> {
        if !self.waiting {
            return None;
        }
        self.destination.as_ref().map(|(_, id)| id.as_str())
    }

    fn cancel(&mut self) {
        self.waiting = false;
    }

    fn pending_at(&self, location: &str) -> Option<&str> {
        let (path, _, hash) = location_parts(location);
        let (destination_path, _) = self.destination.as_ref()?;
        let id = self.pending_id()?;
        (path == destination_path && fragment_id(hash).as_deref() == Some(id)).then_some(id)
    }

    fn target_for_frame(&mut self, location: &str, position: (f64, f64)) -> Option<&str> {
        // 即使 scroll 事件还在队列里，已经发生的滚动也优先于章节定位。
        if self.position != position {
            self.cancel();
        }
        self.pending_at(location)
    }
}

fn fragment_id(hash: &str) -> Option<Cow<'_, str>> {
    let hash = hash.strip_prefix('#').filter(|hash| !hash.is_empty())?;
    Some(percent_decode_str(hash).decode_utf8().unwrap_or(Cow::Borrowed(hash)))
}

#[cfg(target_arch = "wasm32")]
const LOCATION_EVENTS: &[&str] = &["popstate", "hashchange", "pagehide"];

/// 观察异步挂载的章节，等待期间允许浏览器或用户随时取消。
#[cfg(target_arch = "wasm32")]
pub(super) struct FragmentScroll {
    window: Window,
    history: Rc<dyn History>,
    target: RefCell<FragmentTarget>,
    frame: Cell<Option<i32>>,
    on_frame: Closure<dyn FnMut()>,
    on_mutation: Closure<dyn FnMut()>,
    on_location: Closure<dyn FnMut(Event)>,
    on_scroll: Closure<dyn FnMut()>,
    observer: RefCell<Option<MutationObserver>>,
}

#[cfg(target_arch = "wasm32")]
impl FragmentScroll {
    pub(super) fn new(history: Rc<dyn History>) -> Option<Rc<Self>> {
        let window = web_sys::window()?;
        Some(Rc::new_cyclic(|weak: &Weak<Self>| {
            let scroll = weak.clone();
            let on_frame = Closure::<dyn FnMut()>::new(move || {
                if let Some(scroll) = scroll.upgrade() {
                    scroll.attempt_frame();
                }
            });
            let scroll = weak.clone();
            let on_mutation = Closure::<dyn FnMut()>::new(move || {
                if let Some(scroll) = scroll.upgrade() {
                    scroll.queue_frame();
                }
            });
            let scroll = weak.clone();
            let on_location = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
                if let Some(scroll) = scroll.upgrade() {
                    let kind = event.type_();
                    if kind == "pagehide" {
                        scroll.cancel();
                        return;
                    }
                    let native = kind == "hashchange" || scroll.native_fragment_entry();
                    scroll.enter(&scroll.history.current_route(), native);
                }
            });
            let scroll = weak.clone();
            let on_scroll = Closure::<dyn FnMut()>::new(move || {
                if let Some(scroll) = scroll.upgrade() {
                    // 浏览器或 Dioxus 恢复历史位置时也必须让出视口。
                    scroll.cancel();
                }
            });
            Self {
                window,
                history,
                target: RefCell::default(),
                frame: Cell::new(None),
                on_frame,
                on_mutation,
                on_location,
                on_scroll,
                observer: RefCell::default(),
            }
        }))
    }

    pub(super) fn listen(&self) {
        for event in LOCATION_EVENTS {
            self.window
                .add_event_listener_with_callback(event, self.on_location.as_ref().unchecked_ref())
                .ok();
        }
        self.window
            .add_event_listener_with_callback_and_bool(
                "scroll",
                self.on_scroll.as_ref().unchecked_ref(),
                true,
            )
            .ok();
    }

    pub(super) fn enter(&self, location: &str, native: bool) {
        if !self.target.borrow_mut().enter(location, self.position()) {
            return;
        }
        self.stop_observing();
        let native_target_ready = native
            && self
                .target
                .borrow()
                .pending_at(location)
                .is_some_and(|id| self.visible_element(id).is_some());
        if native_target_ready {
            self.cancel();
            return;
        }
        self.queue_frame();
    }

    fn queue_frame(&self) {
        if self.frame.get().is_none() && self.target.borrow().pending_id().is_some() {
            self.frame.set(
                self.window.request_animation_frame(self.on_frame.as_ref().unchecked_ref()).ok(),
            );
        }
    }

    fn position(&self) -> (f64, f64) {
        (self.window.scroll_x().unwrap_or_default(), self.window.scroll_y().unwrap_or_default())
    }

    fn native_fragment_entry(&self) -> bool {
        // 原生同页锚点没有 Dioxus 的 [x, y] 状态。目标已挂载时浏览器会自行定位。
        self.window
            .history()
            .ok()
            .and_then(|history| history.state().ok())
            .is_some_and(|state| state.is_null())
    }

    fn visible_element(&self, id: &str) -> Option<Element> {
        self.window
            .document()?
            .get_element_by_id(id)
            .filter(|element| element.get_client_rects().length() != 0)
    }

    fn attempt_frame(&self) {
        self.frame.set(None);
        let location = self.history.current_route();
        let element = self
            .target
            .borrow_mut()
            .target_for_frame(&location, self.position())
            .map(|id| self.visible_element(id));
        match element {
            Some(Some(element)) => {
                // 定位也会触发 scroll，先释放等待，保证每个目标只定位一次。
                self.cancel();
                element.scroll_into_view();
            }
            Some(None) => self.observe(),
            None => self.cancel(),
        }
    }

    fn observe(&self) {
        if self.observer.borrow().is_some() {
            return;
        }
        let Some(root) = self.window.document().and_then(|document| document.body()) else {
            return;
        };
        let Ok(observer) = MutationObserver::new(self.on_mutation.as_ref().unchecked_ref()) else {
            return;
        };
        let options = MutationObserverInit::new();
        options.set_child_list(true);
        options.set_subtree(true);
        options.set_attributes(true);
        options.set_attribute_filter(&js_sys::Array::of2(&"id".into(), &"hidden".into()));
        if observer.observe_with_options(root.as_ref(), &options).is_ok() {
            self.observer.replace(Some(observer));
        }
    }

    fn cancel(&self) {
        self.target.borrow_mut().cancel();
        self.stop_observing();
    }

    fn stop_observing(&self) {
        if let Some(frame) = self.frame.take() {
            self.window.cancel_animation_frame(frame).ok();
        }
        if let Some(observer) = self.observer.borrow_mut().take() {
            observer.disconnect();
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for FragmentScroll {
    fn drop(&mut self) {
        self.stop_observing();
        for event in LOCATION_EVENTS {
            self.window
                .remove_event_listener_with_callback(
                    event,
                    self.on_location.as_ref().unchecked_ref(),
                )
                .ok();
        }
        self.window
            .remove_event_listener_with_callback_and_bool(
                "scroll",
                self.on_scroll.as_ref().unchecked_ref(),
                true,
            )
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::FragmentTarget;

    #[test]
    fn a_fragment_is_consumed_once_and_query_writes_cannot_restart_it() {
        let mut target = FragmentTarget::default();
        assert!(target.enter("/me#account%2Dbids-title", (0.0, 0.0)));
        assert_eq!(target.pending_id(), Some("account-bids-title"));
        assert!(!target.enter("/me?bid_page=2#account-bids-title", (0.0, 0.0)));
        target.cancel();
        assert!(!target.enter("/me?bid_page=3#account-bids-title", (0.0, 0.0)));
        assert_eq!(target.pending_id(), None);
    }

    #[test]
    fn changing_pages_replaces_the_waiting_target_and_can_return_to_it() {
        let mut target = FragmentTarget::default();
        assert!(target.enter("/me#login-duration", (0.0, 0.0)));
        assert_eq!(target.pending_at("/rules#login-duration"), None);
        assert_eq!(target.pending_at("/me?ref=x#login-duration"), Some("login-duration"));
        assert!(target.enter("/rules#schedule", (0.0, 0.0)));
        assert_eq!(target.pending_id(), Some("schedule"));
        assert!(target.enter("/rules", (0.0, 0.0)));
        assert_eq!(target.pending_id(), None);
        assert!(target.enter("/me#login-duration", (0.0, 0.0)));
        assert_eq!(target.pending_id(), Some("login-duration"));
    }

    #[test]
    fn mounting_a_target_and_scrolling_before_its_frame_keeps_the_viewport() {
        let location = "/me#login-duration";
        let mut target = FragmentTarget::default();
        let initial_position = (0.0, 25.0);
        target.enter(location, initial_position);
        assert_eq!(target.target_for_frame(location, initial_position), Some("login-duration"));

        // 章节挂载后观察器排帧。用户已滚动，scroll 事件尚未到达。
        assert_eq!(target.target_for_frame(location, (0.0, 470.0)), None);
        assert_eq!(target.target_for_frame(location, initial_position), None);

        // 后续导航重新允许等待。任意滚动事件均可取消，坐标回到起点也不会重启等待。
        target.enter("/rules#schedule", initial_position);
        target.cancel();
        assert_eq!(target.target_for_frame("/rules#schedule", initial_position), None);
    }
}
