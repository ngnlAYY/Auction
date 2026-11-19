//! 展开的页面面板共用文档级 Escape，按焦点和打开顺序决定收起哪一个。

#[cfg(any(target_arch = "wasm32", test))]
use std::cell::RefCell;
#[cfg(all(test, not(target_arch = "wasm32")))]
use std::rc::Rc;

use auction_web_shared::a11y::focus_element_by_id;
#[cfg(any(target_arch = "wasm32", test))]
use auction_web_shared::a11y::{EscapeUnsubscribe, use_document_escape};
#[cfg(target_arch = "wasm32")]
use auction_web_shared::a11y::{bind_document_escape, top_modal};
use dioxus::prelude::*;

pub(super) fn use_panel_escape(
    _is_open: impl Fn() -> bool + 'static,
    _trigger_id: &'static str,
    _panel_id: &'static str,
    _dismiss: Callback<(), bool>,
) {
    #[cfg(any(target_arch = "wasm32", test))]
    use_document_escape(_is_open, move || {
        attach(OpenPanel { trigger_id: _trigger_id, panel_id: _panel_id, dismiss: _dismiss })
    });
}

pub(super) fn focus_panel_trigger(id: &'static str) {
    focus_element_by_id(id);
    #[cfg(all(test, not(target_arch = "wasm32")))]
    if let Some(browser) = try_consume_context::<Rc<PanelBrowser>>() {
        browser.focus.set(Some(id));
        browser.focused_panel.set(None);
    }
}

#[cfg(any(target_arch = "wasm32", test))]
struct OpenPanel {
    trigger_id: &'static str,
    panel_id: &'static str,
    dismiss: Callback<(), bool>,
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Default)]
struct OpenPanels {
    panels: Vec<OpenPanel>,
    unsubscribe: Option<EscapeUnsubscribe>,
}

#[cfg(any(target_arch = "wasm32", test))]
thread_local! {
    static OPEN: RefCell<OpenPanels> = RefCell::new(OpenPanels::default());
}

#[cfg(any(target_arch = "wasm32", test))]
fn attach(panel: OpenPanel) -> Option<EscapeUnsubscribe> {
    let trigger_id = panel.trigger_id;
    OPEN.with_borrow_mut(|open| {
        if open.panels.is_empty() {
            open.unsubscribe = Some(listen()?);
        }
        open.panels.push(panel);
        Some(())
    })?;
    Some(Box::new(move || {
        let unsubscribe = OPEN.with_borrow_mut(|open| {
            open.panels.retain(|panel| panel.trigger_id != trigger_id);
            if open.panels.is_empty() { open.unsubscribe.take() } else { None }
        });
        if let Some(unsubscribe) = unsubscribe {
            unsubscribe();
        }
    }))
}

#[cfg(any(target_arch = "wasm32", test))]
fn dismiss_panel(has_focus: impl Fn(&OpenPanel) -> bool) -> bool {
    // 还焦会同步触发事件；调用组件回调前须释放登记表的借用。
    let dismiss = OPEN.with_borrow(|open| {
        open.panels
            .iter()
            .rev()
            .find(|panel| has_focus(panel))
            .or_else(|| open.panels.last())
            .map(|panel| panel.dismiss)
    });
    dismiss.is_some_and(|callback| callback.call(()))
}

#[cfg(target_arch = "wasm32")]
fn listen() -> Option<EscapeUnsubscribe> {
    let document = web_sys::window()?.document()?;
    // 冒泡阶段接手：给获焦控件、模态及其长文本提示优先处理。
    bind_document_escape(document.clone(), false, move || {
        if top_modal(&document).is_some() {
            return false;
        }
        let focused = document.active_element();
        dismiss_panel(|panel| {
            focused.as_ref().is_some_and(|focused| {
                [panel.trigger_id, panel.panel_id].iter().any(|id| {
                    document
                        .get_element_by_id(id)
                        .is_some_and(|node| node.contains(Some(focused.as_ref())))
                })
            })
        })
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[derive(Default)]
pub(super) struct PanelBrowser {
    pub(super) escape: RefCell<Option<Rc<dyn Fn() -> bool>>>,
    pub(super) focus: std::cell::Cell<Option<&'static str>>,
    pub(super) focused_panel: std::cell::Cell<Option<&'static str>>,
    pub(super) modal_open: std::cell::Cell<bool>,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
fn listen() -> Option<EscapeUnsubscribe> {
    let browser = try_consume_context::<Rc<PanelBrowser>>()?;
    let events = browser.clone();
    browser.escape.replace(Some(Rc::new(move || {
        !events.modal_open.get()
            && dismiss_panel(|panel| {
                events.focus.get() == Some(panel.trigger_id)
                    || events.focused_panel.get() == Some(panel.panel_id)
            })
    })));
    Some(Box::new(move || {
        browser.escape.take();
    }))
}
