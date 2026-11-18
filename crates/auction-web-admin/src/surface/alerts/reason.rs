//! 告警备注的字节校验、提交焦点与错误播报。

use std::rc::Rc;

use auction_web_shared::t;
use dioxus::core::Task;
use dioxus::html::MountedData;
use dioxus::prelude::*;

const REASON_INPUT_ID: &str = "alert-console-reason";
/// 与服务端确认/静默命令的备注长度上限一致。
const MAX_ALERT_REASON_BYTES: usize = 2_048;

/// 按去除首尾空白后实际提交的备注校验 UTF-8 字节数。
pub(super) fn valid_alert_reason_length(value: &str) -> bool {
    value.trim().len() <= MAX_ALERT_REASON_BYTES
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReasonError {
    Clear,
    Unannounced,
    Announced,
}

impl ReasonError {
    fn visible(self) -> bool {
        self != Self::Clear
    }

    fn corrected_by(self, value: &str) -> bool {
        self.visible() && valid_alert_reason_length(value)
    }
}

/// 校验与播报记录跟随页级草稿，切换筛选不会重新播报同一个错误。
#[derive(Clone, Copy, PartialEq)]
pub(super) struct AlertReasonDraft {
    pub(super) value: Signal<String>,
    error: Signal<ReasonError>,
}

pub(super) fn use_alert_reason_draft() -> AlertReasonDraft {
    AlertReasonDraft { value: use_signal(String::new), error: use_signal(|| ReasonError::Clear) }
}

impl AlertReasonDraft {
    pub(super) fn mark_invalid(mut self) {
        if *self.error.peek() == ReasonError::Clear {
            self.error.set(ReasonError::Unannounced);
        }
    }
}

/// 共享备注输入。提交的校验结果由告警台设置，字段负责在渲染错误后移动焦点或播报。
#[component]
pub(super) fn AlertReasonInput(
    draft: AlertReasonDraft,
    mut focus_requested: Signal<bool>,
) -> Element {
    let mut input = use_signal(|| None::<Rc<MountedData>>);
    let mut reason = draft.value;
    let mut error = draft.error;
    // live region 在挂载时为空；播报任务随字段卸载取消，只在送出消息时记录已播报。
    let mut live = use_signal(|| false);
    let mut announcement_task = use_signal(|| None::<Task>);
    let announce_error = use_callback(move |()| {
        clear_announcement(announcement_task, live);
        announcement_task.set(Some(spawn(async move {
            // 让读屏先播报失焦后到达的控件，显式提交的兜底也先清空再写入。
            #[cfg(target_arch = "wasm32")]
            auction_web_shared::timer::TimeoutFuture::new(500).await;
            if error.peek().visible() {
                live.set(true);
                error.set(ReasonError::Announced);
            }
            announcement_task.set(None);
        })));
    });
    use_effect(move || {
        if !focus_requested() {
            return;
        }
        focus_requested.set(false);
        clear_announcement(announcement_task, live);
        let mounted = input.peek().clone();
        spawn(async move {
            if !error.peek().visible() {
                return;
            }
            let moved = move_reason_focus(mounted.as_deref(), reason_has_focus).await;
            if !error.peek().visible() {
                return;
            }
            if moved {
                // 聚焦成功由字段描述播报，之后的失焦共用同一条已播报记录。
                error.set(ReasonError::Announced);
            } else {
                announce_error.call(());
            }
        });
    });

    let invalid_now = error().visible();
    let message = t!("alert-console-reason-too-long", maximum: MAX_ALERT_REASON_BYTES.to_string());
    rsx! {
        p { id: "alert-console-reason-limit", class: "admin-panel__note",
            {t!("alert-console-reason-limit", maximum: MAX_ALERT_REASON_BYTES.to_string())}
        }
        div { class: "admin-alert-console__reason",
            label { for: REASON_INPUT_ID, {t!("alert-console-reason")} }
            input {
                id: REASON_INPUT_ID,
                r#type: "text",
                value: reason(),
                aria_invalid: invalid_now.to_string(),
                aria_describedby: if invalid_now {
                    "alert-console-reason-limit alert-console-reason-error"
                } else {
                    "alert-console-reason-limit"
                },
                onmounted: move |event| input.set(Some(event.data())),
                oninput: move |event| {
                    let value = event.value();
                    if error.peek().corrected_by(&value) {
                        error.set(ReasonError::Clear);
                        clear_announcement(announcement_task, live);
                    }
                    reason.set(value);
                },
                onblur: move |_| {
                    if !valid_alert_reason_length(&reason.peek()) {
                        draft.mark_invalid();
                        // 等待期间也只排一次，连续失焦不会推迟第一次播报。
                        if *error.peek() == ReasonError::Unannounced && announcement_task.peek().is_none() {
                            announce_error.call(());
                        }
                    }
                },
            }
        }
        if invalid_now {
            p { id: "alert-console-reason-error", class: "admin-field-error", {message.clone()} }
        }
        p {
            style: "position:absolute; inline-size:1px; block-size:1px; overflow:hidden; clip-path:inset(50%); white-space:nowrap",
            aria_live: "polite",
            aria_atomic: "true",
            if live() {
                {message}
            }
        }
    }
}

fn clear_announcement(mut task: Signal<Option<Task>>, mut live: Signal<bool>) {
    if let Some(previous) = task.write().take() {
        previous.cancel();
    }
    if *live.peek() {
        live.set(false);
    }
}

/// 成功迁移焦点时由字段描述播报。焦点已在字段上、控件未挂载或迁移失败时走 live region。
async fn move_reason_focus(input: Option<&MountedData>, has_focus: impl Fn() -> bool) -> bool {
    if has_focus() {
        return false;
    }
    let Some(input) = input else { return false };
    input.set_focus(true).await.is_ok() && has_focus()
}

fn reason_has_focus() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element())
            .is_some_and(|element| element.id() == REASON_INPUT_ID)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;
    use std::future::{Future, ready};
    use std::pin::{Pin, pin};
    use std::rc::Rc;
    use std::task::{Context, Poll, Waker};

    use dioxus::html::{MountedData, MountedError, MountedResult, RenderedElementBacking};

    use super::{ReasonError, move_reason_focus, valid_alert_reason_length};

    #[test]
    fn alert_reason_limit_counts_utf8_bytes_of_the_trimmed_payload() {
        let exactly = format!("{}ab", "告".repeat(682));
        assert_eq!(exactly.len(), 2_048);
        assert!(valid_alert_reason_length(&exactly));
        assert!(!valid_alert_reason_length(&format!("{exactly}c")));
        assert!(valid_alert_reason_length(&format!("  {exactly}  ")));
        assert!(valid_alert_reason_length(&"😀".repeat(512)));
        assert!(!valid_alert_reason_length(&"😀".repeat(513)));
        assert!(valid_alert_reason_length(""));
    }

    #[test]
    fn reason_error_is_cleared_only_when_the_draft_is_corrected() {
        for error in [ReasonError::Unannounced, ReasonError::Announced] {
            assert!(!error.corrected_by(&"😀".repeat(513)));
            assert!(error.corrected_by(&"😀".repeat(512)));
        }
    }

    struct Focusable {
        focused: Rc<Cell<bool>>,
        calls: Rc<Cell<usize>>,
        fails: bool,
    }

    impl RenderedElementBacking for Focusable {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn set_focus(&self, focus: bool) -> Pin<Box<dyn Future<Output = MountedResult<()>>>> {
            self.calls.set(self.calls.get() + 1);
            if self.fails {
                return Box::pin(ready(Err(MountedError::NotSupported)));
            }
            self.focused.set(focus);
            Box::pin(ready(Ok(())))
        }
    }

    #[test]
    fn blocked_submit_moves_focus_or_uses_the_announcement() {
        for (already_focused, fails, moved, calls_expected) in
            [(false, false, true, 1), (true, false, false, 0), (false, true, false, 1)]
        {
            let focused = Rc::new(Cell::new(already_focused));
            let calls = Rc::new(Cell::new(0));
            let input = MountedData::new(Focusable {
                focused: focused.clone(),
                calls: calls.clone(),
                fails,
            });
            let mut focus = pin!(move_reason_focus(Some(&input), || focused.get()));
            assert_eq!(
                focus.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                Poll::Ready(moved)
            );
            assert_eq!(calls.get(), calls_expected);
        }
    }
}
