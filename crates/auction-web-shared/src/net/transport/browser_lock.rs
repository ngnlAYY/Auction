//! 原生 Web Locks 的有界、可取消独占守卫；不以标签页内互斥或浏览器存储伪造跨页锁。

/// 原生锁尚未获取，请求尚未发送；不能把这类前置失败当作未知外写。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserLockError {
    /// 非安全上下文、浏览器不支持或原生锁请求被拒绝。
    Unavailable,
    /// 等待其他标签页释放锁超过调用方预算。
    Timeout,
}

/// 同源、同名原生独占锁；须先获取，再构造请求，并持有到响应完整读取或 fetch 已取消。
/// 调用方按声明逆序释放请求 future 与此守卫；未获取的等待在 future 丢弃时取消。
pub struct BrowserLock {
    #[cfg(target_arch = "wasm32")]
    _native: native::NativeLock,
}

impl BrowserLock {
    /// 只使用浏览器原生 `navigator.locks`；不支持时明确失败，非 WASM 也不返回假锁。
    pub async fn acquire(name: &str, timeout_millis: u32) -> Result<Self, BrowserLockError> {
        #[cfg(target_arch = "wasm32")]
        {
            native::acquire(name, timeout_millis).await.map(|native| Self { _native: native })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, timeout_millis);
            Err(BrowserLockError::Unavailable)
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    use std::{
        cell::RefCell,
        rc::Rc,
        task::{Poll, Waker},
    };

    use futures_util::{
        future::{Either, poll_fn, select},
        pin_mut,
    };
    use gloo_timers::future::TimeoutFuture;
    use js_sys::{
        Function, Object, Promise, Reflect,
        futures::{JsFuture, spawn_local},
    };
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};

    use super::BrowserLockError;

    #[derive(Default)]
    struct GrantState {
        granted: bool,
        waker: Option<Waker>,
    }

    pub(super) struct NativeLock {
        controller: web_sys::AbortController,
        release: Function,
        request: Option<JsFuture>,
        callback: Option<Closure<dyn FnMut(JsValue) -> Promise>>,
    }

    pub(super) async fn acquire(
        name: &str,
        timeout_millis: u32,
    ) -> Result<NativeLock, BrowserLockError> {
        let unavailable = |_| BrowserLockError::Unavailable;
        let window = web_sys::window().ok_or(BrowserLockError::Unavailable)?;
        if Reflect::get(window.as_ref(), &JsValue::from_str("isSecureContext"))
            .map_err(unavailable)?
            .as_bool()
            != Some(true)
        {
            return Err(BrowserLockError::Unavailable);
        }
        // web-sys 0.3.106 的 LockManager 仍要求全局 unstable cfg；这里只绑定原生属性和方法。
        // 属性不存在、访问被阻止或 request 拒绝均失败关闭，不注入脚本或降级成非原子存储锁。
        let navigator =
            Reflect::get(window.as_ref(), &JsValue::from_str("navigator")).map_err(unavailable)?;
        let manager = Reflect::get(&navigator, &JsValue::from_str("locks")).map_err(unavailable)?;
        let request = Reflect::get(&manager, &JsValue::from_str("request"))
            .map_err(unavailable)?
            .dyn_into::<Function>()
            .map_err(unavailable)?;
        let controller = web_sys::AbortController::new().map_err(unavailable)?;
        let options = Object::new();
        Reflect::set(&options, &JsValue::from_str("signal"), controller.signal().as_ref())
            .map_err(unavailable)?;
        let mut release = None;
        let held = Promise::new(&mut |resolve, _| release = Some(resolve));
        let release = release.ok_or(BrowserLockError::Unavailable)?;
        let state = Rc::new(RefCell::new(GrantState::default()));
        let granted_state = Rc::clone(&state);
        let callback = Closure::<dyn FnMut(JsValue) -> Promise>::new(move |lock: JsValue| {
            if !lock.is_object() {
                return Promise::resolve(&JsValue::UNDEFINED);
            }
            let waker = {
                let mut state = granted_state.borrow_mut();
                state.granted = true;
                state.waker.take()
            };
            if let Some(waker) = waker {
                waker.wake();
            }
            // 原生锁持有到此 Promise 完成；Rust 请求完成或取消后由守卫 Drop 释放。
            held.clone()
        });
        let request = request
            .call3(&manager, &JsValue::from_str(name), &options, callback.as_ref())
            .map_err(unavailable)?
            .dyn_into::<Promise>()
            .map_err(unavailable)?;
        let mut guard = NativeLock {
            controller,
            release,
            request: Some(JsFuture::from(request)),
            callback: Some(callback),
        };
        let (acquired, settled) = {
            let granted = poll_fn(|context| {
                let mut state = state.borrow_mut();
                if state.granted {
                    Poll::Ready(())
                } else {
                    if state.waker.as_ref().is_none_or(|waker| !waker.will_wake(context.waker())) {
                        state.waker = Some(context.waker().clone());
                    }
                    Poll::Pending
                }
            });
            let request = guard.request.as_mut().ok_or(BrowserLockError::Unavailable)?;
            let wait = select(granted, request);
            let timeout = TimeoutFuture::new(timeout_millis);
            pin_mut!(wait, timeout);
            match select(wait, timeout).await {
                Either::Left((Either::Left(_), _)) => (Ok(()), false),
                Either::Left((Either::Right(_), _)) => (Err(BrowserLockError::Unavailable), true),
                Either::Right(_) => (Err(BrowserLockError::Timeout), false),
            }
        };
        if settled {
            // 已观察到 request 完成，不能再次 poll 已消费结果的 JsFuture。
            guard.request.take();
            guard.callback.take();
        }
        acquired?;
        Ok(guard)
    }

    impl Drop for NativeLock {
        fn drop(&mut self) {
            // signal 只取消排队；已获取时必须完成 waiting Promise，不能 steal 其他标签页。
            self.controller.abort();
            let _ = self.release.call1(&JsValue::UNDEFINED, &JsValue::UNDEFINED);
            if let (Some(request), Some(callback)) = (self.request.take(), self.callback.take()) {
                // grant 回调可能已经排队；保留到原生 request settle，避免释放 Rust 闭包后被调用。
                spawn_local(async move {
                    let _ = request.await;
                    drop(callback);
                });
            }
        }
    }
}
