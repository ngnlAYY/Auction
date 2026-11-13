//! Public 命令成功后的页面级提示条：弹窗关闭后在视口右下角给出结果，几秒后自行消失。
//! 提示条只是已提交结果的回显，不保存命令状态；目录行与详情抽屉仍从权威投影读取价格。

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

/// 自动消失前的停留时间；内容只有一两句，且同一结果随后出现在目录行上。
const TOAST_MILLIS: u32 = 5_000;

#[derive(Clone, PartialEq)]
pub(super) struct CatalogToast {
    /// 单调序号：区分连续两条内容相同的提示，旧计时器不会提前清掉新提示。
    id: u64,
    lot_code: String,
    message: String,
    /// 需要与主句并列给出的补充事实（例如未领先时的当前价）。
    detail: Option<String>,
}

/// 提示条写入口。信号由身份宿主持有，身份切换时上一身份的结果不会留在页面上。
#[derive(Clone, Copy)]
pub(super) struct CatalogToastContext {
    current: Signal<Option<CatalogToast>>,
    sequence: Signal<u64>,
}

impl CatalogToastContext {
    /// 新提示直接替换旧提示：同一时刻只回显最近一次结果。
    pub(super) fn show(mut self, lot_code: String, message: String, detail: Option<String>) {
        let id = self.sequence.peek().wrapping_add(1);
        self.sequence.set(id);
        self.current.set(Some(CatalogToast { id, lot_code, message, detail }));
    }
}

/// 在身份宿主建立提示条上下文，返回供 [`CatalogToastHost`] 渲染的信号。
pub(super) fn use_catalog_toast_provider() -> Signal<Option<CatalogToast>> {
    let current = use_signal(|| None::<CatalogToast>);
    let sequence = use_signal(|| 0_u64);
    use_context_provider(|| CatalogToastContext { current, sequence });
    current
}

#[component]
pub(super) fn CatalogToastHost(mut toast: Signal<Option<CatalogToast>>) -> Element {
    // 计时任务归常驻的提示条宿主所有：发出提示的命令弹窗随即卸载，不能由它持有计时器。
    use_effect(move || {
        let Some(id) = toast.read().as_ref().map(|toast| toast.id) else {
            return;
        };
        spawn(async move {
            TimeoutFuture::new(TOAST_MILLIS).await;
            if toast.peek().as_ref().is_some_and(|current| current.id == id) {
                toast.set(None);
            }
        });
    });
    let current = toast.read().clone();

    rsx! {
        // 播报区域常驻 DOM，内容出现时读屏才会朗读；它不抢焦点，也不拦截指针。
        div {
            class: "public-toast-region",
            role: "status",
            aria_live: "polite",
            aria_atomic: "true",
            if let Some(current) = current {
                div { class: "public-toast",
                    b { class: "public-toast__code", "{current.lot_code}" }
                    span { class: "public-toast__message", "{current.message}" }
                    if let Some(detail) = current.detail {
                        span { class: "public-toast__detail", "{detail}" }
                    }
                }
            }
        }
    }
}
