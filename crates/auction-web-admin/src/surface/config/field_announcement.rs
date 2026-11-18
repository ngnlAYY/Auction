//! 运维配置和凭据表单的字段错误播报。

#[cfg(target_arch = "wasm32")]
use auction_web_shared::timer::TimeoutFuture;
#[cfg(any(target_arch = "wasm32", test))]
use dioxus::core::Task;
use dioxus::prelude::*;

use crate::surface::validation::focus_field;

/// 当前表单的播报状态，重复提交同一错误时也会先清空再播报。
#[derive(Clone, Copy, PartialEq)]
pub(in crate::surface) struct FieldAnnouncer {
    message: Signal<String>,
    field: Signal<Option<String>>,
    #[cfg(any(target_arch = "wasm32", test))]
    task: Signal<Option<Task>>,
}

/// 为表单保留一个常驻的礼貌播报区及其更新任务。
pub(in crate::surface) fn use_field_announcer() -> FieldAnnouncer {
    FieldAnnouncer {
        message: use_signal(String::new),
        field: use_signal(|| None),
        #[cfg(any(target_arch = "wasm32", test))]
        task: use_signal(|| None),
    }
}

impl FieldAnnouncer {
    /// 表单提交或替换消息时撤销先前的播报。
    pub(in crate::surface) fn clear(mut self) {
        #[cfg(any(target_arch = "wasm32", test))]
        {
            let task = self.task.write().take();
            if let Some(task) = task {
                task.cancel();
            }
        }
        if !self.message.peek().is_empty() {
            self.message.set(String::new());
        }
        if self.field.peek().is_some() {
            self.field.set(None);
        }
    }

    /// 字段纠正不撤销其他字段的播报；未标记的单字段表单播报沿用原撤销行为。
    pub(in crate::surface) fn clear_field(self, field_id: &str) {
        if self.field.peek().as_deref().is_none_or(|pending| pending == field_id) {
            self.clear();
        }
    }

    /// 让同一条文案的连续两次播报各自产生内容更新。
    pub(in crate::surface) fn announce(self, message: String) {
        self.schedule(None, move || message, 50);
    }

    /// 离焦时先让读屏软件读出接收焦点的控件。
    pub(in crate::surface) fn announce_on_blur(self, field_id: &str, message: String) {
        self.announce_localized_on_blur(field_id, move || message);
    }

    /// 焦点播报的等待期内语言可能切换；保留字段错误，在真正播报前翻译。
    pub(in crate::surface::config) fn announce_localized_on_blur(
        self,
        field_id: &str,
        message: impl FnOnce() -> String + 'static,
    ) {
        self.schedule(Some(field_id), message, 500);
    }

    /// 提交失败时由移焦播报字段错误。已聚焦或控件禁用时，通过 live region 播报。
    pub(in crate::surface) fn invalid_submit(
        self,
        field_id: &'static str,
        message: String,
        focusable: bool,
    ) {
        if focusable && !field_has_focus(field_id) {
            self.clear();
            focus_error_field(field_id);
        } else {
            self.schedule(Some(field_id), move || message, 50);
        }
    }

    fn schedule(
        mut self,
        field_id: Option<&str>,
        message: impl FnOnce() -> String + 'static,
        _delay_millis: u32,
    ) {
        self.clear();
        self.field.set(field_id.map(str::to_owned));
        #[cfg(any(target_arch = "wasm32", test))]
        {
            self.task.set(Some(spawn(async move {
                wait_for_announcement(_delay_millis).await;
                self.message.set(message());
            })));
        }
        #[cfg(not(any(target_arch = "wasm32", test)))]
        self.message.set(message());
    }
}

fn field_has_focus(_field_id: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .is_some_and(|element| element.id() == _field_id)
    }
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(browser) = test_form::browser() {
        return browser.focused.get() == _field_id;
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

fn focus_error_field(field_id: &'static str) {
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(browser) = test_form::browser() {
        browser.moved_focus_to.set(Some(field_id));
        browser.focused.set(field_id);
        return;
    }
    focus_field(field_id);
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_announcement(delay_millis: u32) {
    TimeoutFuture::new(delay_millis).await;
}

#[cfg(all(test, not(target_arch = "wasm32")))]
async fn wait_for_announcement(_delay_millis: u32) {
    if let Some(browser) = test_form::browser() {
        browser.wait_for_timer().await;
    }
}

/// 供辅助技术读取的播报节点，从文档布局中移出。
#[component]
pub(in crate::surface) fn FieldLiveRegion(announcer: FieldAnnouncer) -> Element {
    rsx! {
        div {
            aria_live: "polite",
            aria_atomic: "true",
            style: "position:absolute;width:1px;height:1px;margin:-1px;overflow:hidden;padding:0;border:0;clip-path:inset(50%);white-space:nowrap",
            {announcer.message.cloned()}
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(in crate::surface) mod test_form {
    use std::cell::{Cell, RefCell};
    use std::future::poll_fn;
    use std::rc::Rc;
    use std::task::{Poll, Waker};

    use dioxus::core::{ElementId, Mutation, VirtualDom};
    use dioxus::prelude::try_consume_context;

    use crate::surface::test_events::dispatch_form_event;

    #[derive(Default)]
    pub(in crate::surface) struct FormBrowser {
        pub(in crate::surface) focused: Cell<&'static str>,
        pub(in crate::surface) moved_focus_to: Cell<Option<&'static str>>,
        timer_tick: Cell<u32>,
        timer_waker: RefCell<Option<Waker>>,
    }

    impl FormBrowser {
        pub(in crate::surface) fn fire_timer(&self) {
            self.timer_tick.set(self.timer_tick.get() + 1);
            self.timer_waker.borrow_mut().take().unwrap().wake();
        }

        pub(super) async fn wait_for_timer(&self) {
            let tick = self.timer_tick.get();
            poll_fn(|context| {
                if self.timer_tick.get() != tick {
                    return Poll::Ready(());
                }
                self.timer_waker.replace(Some(context.waker().clone()));
                Poll::Pending
            })
            .await;
        }
    }

    pub(super) fn browser() -> Option<Rc<FormBrowser>> {
        try_consume_context()
    }

    pub(in crate::surface) fn listener(edits: &[Mutation], event: &str) -> ElementId {
        edits
            .iter()
            .find_map(|edit| match edit {
                Mutation::NewEventListener { name, id } if name == event => Some(*id),
                _ => None,
            })
            .unwrap()
    }

    pub(in crate::surface) fn dispatch(
        dom: &VirtualDom,
        target: ElementId,
        name: &str,
        value: &str,
    ) {
        let event = dispatch_form_event(dom, target, name, value);
        if name == "submit" {
            assert!(!event.default_action_enabled());
        }
    }
}
