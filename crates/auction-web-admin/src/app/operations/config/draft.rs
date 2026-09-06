//! 当前管理会话内的非敏感配置草稿；不使用浏览器持久化存储。

use dioxus::prelude::*;

use super::{ConfigCommand, ConfigNotice, model::RuntimeConfigForm};

/// 草稿由认证外壳持有，站内切页不丢失输入与重试命令；退出当前会话后随外壳销毁。
#[derive(Clone, Copy)]
pub(super) struct ConfigDraft {
    pub(super) form: Signal<RuntimeConfigForm>,
    pub(super) baseline: Signal<Option<RuntimeConfigForm>>,
    pub(super) fingerprint: Signal<Option<String>>,
    pub(super) checked_signature: Signal<String>,
    pub(super) command: Signal<Option<ConfigCommand>>,
    pub(super) notice: Signal<Option<ConfigNotice>>,
    pub(super) restart_required: Signal<bool>,
    pub(super) dirty: Memo<bool>,
}

/// 安装一次草稿上下文；浏览器整页离开时由原生提示保护尚未保存的输入。
pub(super) fn use_config_draft() {
    let form = use_signal(RuntimeConfigForm::default);
    let baseline = use_signal(|| None::<RuntimeConfigForm>);
    let fingerprint = use_signal(|| None::<String>);
    let checked_signature = use_signal(String::new);
    let command = use_signal(|| None::<ConfigCommand>);
    let notice = use_signal(|| None::<ConfigNotice>);
    let restart_required = use_signal(|| false);
    let dirty =
        use_memo(move || baseline.read().as_ref().is_some_and(|initial| initial != &*form.read()));
    use_context_provider(|| ConfigDraft {
        form,
        baseline,
        fingerprint,
        checked_signature,
        command,
        notice,
        restart_required,
        dirty,
    });
    use_unload_warning(dirty);
}

fn use_unload_warning(_dirty: Memo<bool>) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::{cell::RefCell, rc::Rc};
        use wasm_bindgen::{JsCast, closure::Closure};

        let guard = use_hook(|| Rc::new(RefCell::new(None::<UnloadWarning>)));
        let cleanup = Rc::clone(&guard);
        use_drop(move || {
            cleanup.borrow_mut().take();
        });
        use_effect(move || {
            if !_dirty() {
                guard.borrow_mut().take();
                return;
            }
            if guard.borrow().is_some() {
                return;
            }
            let Some(window) = web_sys::window() else { return };
            let callback = Closure::<dyn FnMut(web_sys::BeforeUnloadEvent)>::new(
                move |event: web_sys::BeforeUnloadEvent| {
                    if *_dirty.peek() {
                        event.prevent_default();
                        // 提示文本由浏览器控制；非空 returnValue 兼容仍依赖该属性的浏览器。
                        event.set_return_value(" ");
                    }
                },
            );
            if window
                .add_event_listener_with_callback("beforeunload", callback.as_ref().unchecked_ref())
                .is_ok()
            {
                guard.replace(Some(UnloadWarning { window, callback }));
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
struct UnloadWarning {
    window: web_sys::Window,
    callback: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::BeforeUnloadEvent)>,
}

#[cfg(target_arch = "wasm32")]
impl Drop for UnloadWarning {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast;
        let _ = self.window.remove_event_listener_with_callback(
            "beforeunload",
            self.callback.as_ref().unchecked_ref(),
        );
    }
}
