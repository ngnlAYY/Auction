//! 展开期间的动画帧守卫；页面隐藏时仅保留可见性监听。

use std::cell::Cell;
use std::rc::{Rc, Weak};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};

use super::document::Document;

#[cfg(target_arch = "wasm32")]
type Callback = Closure<dyn FnMut()>;
#[cfg(not(target_arch = "wasm32"))]
type Callback = Rc<dyn Fn()>;

pub(super) struct AnchorFrames {
    frames: Rc<FrameLoop>,
    #[cfg(not(target_arch = "wasm32"))]
    listener_id: u32,
}

struct FrameLoop {
    document: Document,
    #[cfg(target_arch = "wasm32")]
    window: web_sys::Window,
    active: Cell<bool>,
    pending: Cell<Option<i32>>,
    on_frame: Callback,
    on_visibility: Callback,
}

impl AnchorFrames {
    pub(super) fn start(document: Document, update: impl Fn(&Document) + 'static) -> Option<Self> {
        #[cfg(target_arch = "wasm32")]
        let window = document.default_view()?;
        let frames = Rc::new_cyclic(|weak: &Weak<FrameLoop>| {
            let frame = weak.clone();
            let visibility = weak.clone();
            FrameLoop {
                document,
                #[cfg(target_arch = "wasm32")]
                window,
                active: Cell::new(true),
                pending: Cell::new(None),
                on_frame: callback(move || {
                    if let Some(frames) = frame.upgrade() {
                        frames.pending.set(None);
                        if frames.active.get() && !frames.hidden() {
                            update(&frames.document);
                            frames.sync();
                        }
                    }
                }),
                on_visibility: callback(move || {
                    if let Some(frames) = visibility.upgrade() {
                        frames.sync();
                    }
                }),
            }
        });
        #[cfg(target_arch = "wasm32")]
        frames
            .document
            .add_event_listener_with_callback(
                "visibilitychange",
                frames.on_visibility.as_ref().unchecked_ref(),
            )
            .ok()?;
        #[cfg(not(target_arch = "wasm32"))]
        let listener_id = {
            let id = frames.document.next_listener.get();
            frames.document.next_listener.set(id + 1);
            frames.document.visibility.borrow_mut().insert(id, Rc::clone(&frames.on_visibility));
            id
        };
        frames.sync();
        Some(Self {
            frames,
            #[cfg(not(target_arch = "wasm32"))]
            listener_id,
        })
    }
}

impl Drop for AnchorFrames {
    fn drop(&mut self) {
        // 帧内收起也必须终止循环；当前回调返回后不能再次登记。
        self.frames.active.set(false);
        self.frames.cancel();
        #[cfg(target_arch = "wasm32")]
        let _ = self.frames.document.remove_event_listener_with_callback(
            "visibilitychange",
            self.frames.on_visibility.as_ref().unchecked_ref(),
        );
        #[cfg(not(target_arch = "wasm32"))]
        self.frames.document.visibility.borrow_mut().remove(&self.listener_id);
    }
}

impl FrameLoop {
    fn hidden(&self) -> bool {
        cfg_select! {
            target_arch = "wasm32" => self.document.hidden(),
            _ => self.document.hidden.get(),
        }
    }

    fn sync(&self) {
        if !self.active.get() || self.hidden() {
            self.cancel();
        } else if self.pending.get().is_none() {
            #[cfg(target_arch = "wasm32")]
            let pending =
                self.window.request_animation_frame(self.on_frame.as_ref().unchecked_ref()).ok();
            #[cfg(not(target_arch = "wasm32"))]
            let pending = {
                let id = self.document.next_frame.get();
                self.document.next_frame.set(id + 1);
                self.document.frames.borrow_mut().insert(id, Rc::clone(&self.on_frame));
                Some(id)
            };
            self.pending.set(pending);
        }
    }

    fn cancel(&self) {
        if let Some(id) = self.pending.take() {
            #[cfg(target_arch = "wasm32")]
            let _ = self.window.cancel_animation_frame(id);
            #[cfg(not(target_arch = "wasm32"))]
            self.document.frames.borrow_mut().remove(&id);
        }
    }
}

fn callback(run: impl Fn() + 'static) -> Callback {
    cfg_select! {
        target_arch = "wasm32" => Closure::new(run),
        _ => Rc::new(run),
    }
}
