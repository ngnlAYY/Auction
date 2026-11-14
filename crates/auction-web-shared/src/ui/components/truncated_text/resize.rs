//! 截断文本的共享尺寸监听：全页只注册一个 window `resize` 监听和一个 `ResizeObserver`，
//! 各实例只登记自己的重测回调，卸载时注销；最后一个实例注销后释放全部监听。
//! window 回调按登记顺序执行，与逐实例注册监听时的派发顺序一致。

use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use wasm_bindgen::{JsCast, JsValue, closure::Closure};

type Measure = Rc<dyn Fn()>;

struct SharedListeners {
    window: web_sys::Window,
    observer: web_sys::ResizeObserver,
    _observer_callback: Closure<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>,
    resize_callback: Closure<dyn FnMut()>,
    /// 元素到登记号的弱映射；不阻止已移除的 DOM 节点被回收。
    ids: js_sys::WeakMap,
}

impl SharedListeners {
    fn install() -> Option<Self> {
        let window = web_sys::window()?;
        let observer_callback = Closure::<dyn FnMut(js_sys::Array, web_sys::ResizeObserver)>::new(
            |entries, _observer| dispatch_entries(&entries),
        );
        let observer =
            web_sys::ResizeObserver::new(observer_callback.as_ref().unchecked_ref()).ok()?;
        let resize_callback = Closure::<dyn FnMut()>::new(dispatch_all);
        // 窗口监听注册失败时仍保留元素尺寸观察。
        let _ = window
            .add_event_listener_with_callback("resize", resize_callback.as_ref().unchecked_ref());
        Some(Self {
            window,
            observer,
            _observer_callback: observer_callback,
            resize_callback,
            ids: js_sys::WeakMap::new(),
        })
    }
}

impl Drop for SharedListeners {
    fn drop(&mut self) {
        self.observer.disconnect();
        let _ = self.window.remove_event_listener_with_callback(
            "resize",
            self.resize_callback.as_ref().unchecked_ref(),
        );
    }
}

#[derive(Default)]
struct Registry {
    next_id: u32,
    measures: BTreeMap<u32, Measure>,
    listeners: Option<SharedListeners>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

/// 登记守卫：drop 时停止观察元素并注销重测回调。
pub(super) struct MeasureSubscription {
    id: u32,
    element: web_sys::Element,
}

/// 观察元素尺寸并在窗口尺寸变化时重测；浏览器不支持时返回 `None`，实例保留挂载时的测量结果。
pub(super) fn observe(
    element: &web_sys::Element,
    measure: impl Fn() + 'static,
) -> Option<MeasureSubscription> {
    REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.listeners.is_none() {
            registry.listeners = Some(SharedListeners::install()?);
        }
        let id = registry.next_id;
        registry.next_id = id.checked_add(1)?;
        let listeners = registry.listeners.as_ref()?;
        listeners.ids.set(element, &JsValue::from(id));
        listeners.observer.observe(element);
        registry.measures.insert(id, Rc::new(measure));
        Some(MeasureSubscription { id, element: element.clone() })
    })
}

impl Drop for MeasureSubscription {
    fn drop(&mut self) {
        let _ = REGISTRY.try_with(|registry| {
            let Ok(mut registry) = registry.try_borrow_mut() else {
                return;
            };
            registry.measures.remove(&self.id);
            if let Some(listeners) = &registry.listeners {
                // 同一元素重新挂载后已换成新登记号；旧守卫不能停止观察新登记。
                if listeners.ids.get(&self.element).as_f64() == Some(f64::from(self.id)) {
                    listeners.ids.delete(&self.element);
                    listeners.observer.unobserve(&self.element);
                }
            }
            if registry.measures.is_empty() {
                registry.listeners = None;
            }
        });
    }
}

/// 回调前先释放登记表借用，重测过程中注销或新登记都不会重入借用。
fn dispatch_all() {
    let measures =
        REGISTRY.with(|registry| registry.borrow().measures.values().cloned().collect::<Vec<_>>());
    for measure in measures {
        measure();
    }
}

fn dispatch_entries(entries: &js_sys::Array) {
    let measures = REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let Some(listeners) = &registry.listeners else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|entry| {
                let entry = entry.dyn_into::<web_sys::ResizeObserverEntry>().ok()?;
                let id = listeners.ids.get(&entry.target()).as_f64()?;
                registry.measures.get(&(id as u32)).cloned()
            })
            .collect::<Vec<_>>()
    });
    for measure in measures {
        measure();
    }
}
