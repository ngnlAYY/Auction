//! 宿主渲染的截断提示 DOM 替身，供共享组件与使用它的页面挂载生产逻辑。

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

#[cfg(test)]
use dioxus::prelude::Signal;

use crate::a11y::{ARIA_MODAL_SELECTOR, EscapeUnsubscribe, FOCUSABLE_SELECTOR};

pub(super) const BODY_ID: &str = "body";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ElementKind {
    Plain,
    Control,
    #[cfg(test)]
    Link,
    #[cfg(test)]
    AriaDialog,
    #[cfg(test)]
    NativeDialog {
        open: bool,
        modal: bool,
    },
}

#[derive(Clone)]
pub(super) struct DomElement {
    pub(super) id: String,
    kind: ElementKind,
    pub(super) parent: Option<Rc<DomElement>>,
    pub(super) modal_selector_unsupported: bool,
    portal_text: Option<String>,
    #[cfg(test)]
    pub(super) rectangle: Rc<Cell<(f64, f64, f64, f64)>>,
    #[cfg(test)]
    pub(super) rectangle_reads: Rc<Cell<usize>>,
    #[cfg(test)]
    pub(super) overflow: Rc<Cell<(&'static str, &'static str)>>,
    #[cfg(test)]
    pub(super) overflow_reads: Rc<Cell<usize>>,
    #[cfg(test)]
    pub(super) portal_writes: Option<Rc<PortalWrites>>,
}

impl DomElement {
    fn ancestors(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |element| element.parent.as_deref())
    }

    fn is_focusable(&self) -> bool {
        match self.kind {
            ElementKind::Control => true,
            #[cfg(test)]
            ElementKind::Link => true,
            _ => false,
        }
    }

    fn matches(&self, selector: &str) -> bool {
        match selector {
            #[cfg(test)]
            ARIA_MODAL_SELECTOR => self.kind == ElementKind::AriaDialog,
            #[cfg(test)]
            "dialog:modal" => self.kind == (ElementKind::NativeDialog { open: true, modal: true }),
            #[cfg(test)]
            "dialog[open]" => matches!(self.kind, ElementKind::NativeDialog { open: true, .. }),
            #[cfg(not(test))]
            ARIA_MODAL_SELECTOR | "dialog:modal" | "dialog[open]" => false,
            FOCUSABLE_SELECTOR => self.is_focusable(),
            _ => panic!("测试未实现选择器 {selector}"),
        }
    }

    pub(super) fn closest(&self, selector: &str) -> Result<Option<Self>, ()> {
        if selector.starts_with("dialog:modal") && self.modal_selector_unsupported {
            return Err(());
        }
        Ok(self
            .ancestors()
            .find(|element| selector.split(", ").any(|part| element.matches(part)))
            .cloned())
    }

    pub(super) fn contains(&self, other: &Self) -> bool {
        other.ancestors().any(|element| element.id == self.id)
    }

    pub(super) fn bounding_box(&self) -> [f64; 4] {
        #[cfg(test)]
        {
            self.rectangle_reads.set(self.rectangle_reads.get() + 1);
            self.rectangle.get().into()
        }
        #[cfg(not(test))]
        [20.0, 50.0, 120.0, 80.0]
    }

    pub(super) fn scroll_width(&self) -> i32 {
        120
    }

    pub(super) fn client_width(&self) -> i32 {
        100
    }
}

#[derive(Clone)]
pub(super) struct DomPortal {
    browser: Rc<TooltipBrowserFixture>,
    pub(super) id: String,
    #[cfg(test)]
    writes: Rc<PortalWrites>,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct PortalWrites {
    pub(super) values: RefCell<BTreeMap<String, String>>,
    pub(super) count: Cell<usize>,
}

#[cfg(test)]
impl PortalWrites {
    pub(super) fn set_property(
        &self,
        name: &str,
        value: &str,
    ) -> Result<(), std::convert::Infallible> {
        self.values.borrow_mut().insert(name.to_owned(), value.to_owned());
        self.count.set(self.count.get() + 1);
        Ok(())
    }
}

impl DomPortal {
    #[cfg(test)]
    pub(super) fn set_attribute(
        &self,
        name: &str,
        value: &str,
    ) -> Result<(), std::convert::Infallible> {
        self.writes.set_property(name, value)
    }

    #[cfg(test)]
    pub(super) fn style(&self) -> &PortalWrites {
        &self.writes
    }

    pub(super) fn remove(self) {
        self.browser.nodes.borrow_mut().remove(&self.id);
        if *self.browser.focused.borrow() == self.id {
            self.browser.focused.replace(BODY_ID.to_owned());
        }
    }
}

type EscapeCallback = Rc<dyn Fn() -> bool>;

#[cfg(test)]
type BrowserCallback = Rc<dyn Fn()>;

#[derive(Default)]
/// 在宿主回归中替代截断提示使用的浏览器焦点、几何与节点操作。
pub struct TooltipBrowserFixture {
    #[cfg(test)]
    pub(super) inside_dialog: bool,
    #[cfg(test)]
    pub(super) native_dialog: bool,
    pub(super) modal_selector_unsupported: bool,
    #[cfg(test)]
    pub(super) open_dialog: Cell<Option<Signal<bool>>>,
    pub(super) nodes: RefCell<BTreeMap<String, DomElement>>,
    pub(super) focused: RefCell<String>,
    pub(super) keydown: RefCell<BTreeMap<u32, EscapeCallback>>,
    pub(super) next_listener: Cell<u32>,
    #[cfg(test)]
    pub(super) frames: RefCell<BTreeMap<i32, BrowserCallback>>,
    #[cfg(test)]
    pub(super) visibility: RefCell<BTreeMap<u32, BrowserCallback>>,
    #[cfg(test)]
    pub(super) hidden: Cell<bool>,
    #[cfg(test)]
    pub(super) next_frame: Cell<i32>,
}

impl TooltipBrowserFixture {
    /// 创建带正文节点的文档。
    pub fn new() -> Self {
        let document = Self::default();
        document.add_element(BODY_ID, ElementKind::Plain, None);
        document.focused.replace(BODY_ID.to_owned());
        document
    }

    /// 登记一个浏览器可聚焦的元素，供已挂载的触发器或外部控件使用。
    pub fn add_focusable(&self, id: &str) {
        self.add_element(id, ElementKind::Control, self.body());
    }

    /// 聚焦已挂载且可承接焦点的节点。
    pub fn focus_by_id(&self, id: &str) {
        if let Some(element) = self.get_element_by_id(id) {
            self.focus(&element);
        }
    }

    /// 返回当前焦点元素的 ID。
    pub fn focus_id(&self) -> String {
        self.focused.borrow().clone()
    }

    /// 返回仍挂载在文档中的 portal 正文。
    pub fn portal_text(&self, id: &str) -> Option<String> {
        self.nodes.borrow().get(id)?.portal_text.clone()
    }

    pub(super) fn add_element(&self, id: &str, kind: ElementKind, parent: Option<DomElement>) {
        self.nodes.borrow_mut().insert(
            id.to_owned(),
            DomElement {
                id: id.to_owned(),
                kind,
                parent: parent.map(Rc::new),
                modal_selector_unsupported: self.modal_selector_unsupported,
                portal_text: None,
                #[cfg(test)]
                rectangle: Rc::new(Cell::new((20.0, 50.0, 120.0, 80.0))),
                #[cfg(test)]
                rectangle_reads: Rc::new(Cell::new(0)),
                #[cfg(test)]
                overflow: Rc::new(Cell::new(("visible", "visible"))),
                #[cfg(test)]
                overflow_reads: Rc::new(Cell::new(0)),
                #[cfg(test)]
                portal_writes: None,
            },
        );
    }

    pub(super) fn get_element_by_id(&self, id: &str) -> Option<DomElement> {
        self.nodes.borrow().get(id).cloned()
    }

    pub(super) fn last_matching(
        &self,
        root: Option<&DomElement>,
        selector: &str,
    ) -> Result<Option<DomElement>, ()> {
        if selector == "dialog:modal" && self.modal_selector_unsupported {
            return Err(());
        }
        Ok(self
            .nodes
            .borrow()
            .values()
            .rfind(|element| {
                element.matches(selector)
                    && root.is_none_or(|root| root.id != element.id && root.contains(element))
            })
            .cloned())
    }

    pub(super) fn active_element(&self) -> Option<DomElement> {
        self.get_element_by_id(&self.focused.borrow())
    }

    pub(super) fn focus(&self, element: &DomElement) {
        if element.is_focusable() {
            self.focused.replace(element.id.clone());
        }
    }

    pub(super) fn body(&self) -> Option<DomElement> {
        self.get_element_by_id(BODY_ID)
    }

    pub(super) fn append_portal(
        self: &Rc<Self>,
        parent: &DomElement,
        id: &str,
        text: &str,
    ) -> DomPortal {
        self.add_element(id, ElementKind::Control, Some(parent.clone()));
        #[cfg(test)]
        let writes = Rc::new(PortalWrites::default());
        if let Some(portal) = self.nodes.borrow_mut().get_mut(id) {
            portal.portal_text = Some(text.to_owned());
            #[cfg(test)]
            {
                portal.portal_writes = Some(Rc::clone(&writes));
            }
        }
        DomPortal {
            browser: Rc::clone(self),
            id: id.to_owned(),
            #[cfg(test)]
            writes,
        }
    }

    pub(super) fn listen_escape(
        self: Rc<Self>,
        on_escape: impl Fn() -> bool + 'static,
    ) -> EscapeUnsubscribe {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.keydown.borrow_mut().insert(id, Rc::new(on_escape));
        Box::new(move || {
            self.keydown.borrow_mut().remove(&id);
        })
    }

    #[cfg(test)]
    pub(super) fn dispatch_escape(&self) -> bool {
        let listeners: Vec<_> = self.keydown.borrow().values().cloned().collect();
        listeners.iter().any(|callback| callback())
    }

    #[cfg(test)]
    pub(super) fn advance_frame(&self) {
        let pending: Vec<_> = self.frames.borrow().keys().copied().collect();
        for id in pending {
            let callback = self.frames.borrow_mut().remove(&id);
            if let Some(callback) = callback {
                callback();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn set_hidden(&self, hidden: bool) {
        self.hidden.set(hidden);
        let callbacks: Vec<_> = self.visibility.borrow().values().cloned().collect();
        for callback in callbacks {
            callback();
        }
    }
}
