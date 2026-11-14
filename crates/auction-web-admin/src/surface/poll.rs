//! Admin 运维页面的可选自动轮询：开关与间隔状态由页面持有（入 URL query），本模块提供
//! 轮询调度 hook 与开关控件；可见性感知与分片睡眠复用共享层。
//! 每个页面拥有独立的可见性监听，卸载时释放回调；开关和任务替换不改变其它页面的状态。

use std::cell::Cell;
use std::rc::Rc;

use auction_web_shared::{PageVisibility, sleep_while_visible, t};
use dioxus::{core::Task, prelude::*};

/// 可选的自动轮询间隔（秒）；0 表示关闭。与 URL query `poll` 键的合法值一一对应。
pub(crate) const POLL_INTERVAL_VALUES: &[u64] = &[10, 30, 60];

/// 可选自动轮询 hook：间隔为 0 时关闭；开启后每隔 `interval_seconds` 秒执行一次
/// `on_poll`。页面隐藏期间计时挂起而非继续打请求，恢复可见后走完剩余分片继续轮询。
/// 间隔变化（含关闭）时先取消旧任务，再按新间隔从头计时；卸载仍由页面 Scope 取消任务。
/// 取消只释放失效循环的等待与捕获，不补发轮询，也不改变当前请求或页面可见性策略。
pub(crate) fn use_auto_poll(interval_seconds: Memo<u64>, on_poll: impl FnMut() + Clone + 'static) {
    let visibility = use_hook(PageVisibility::new);
    // 只安装一次可见性监听；effect 不读取任何信号，挂载后不会重跑。
    use_effect({
        let visibility = visibility.clone();
        move || visibility.install(|| {})
    });
    {
        let cleanup_visibility = visibility.clone();
        use_drop(move || cleanup_visibility.cleanup());
    }
    // Task 不放入响应式信号；替换任务不应触发组件重渲染或再次执行本 effect。
    let poll_task = use_hook(|| Rc::new(Cell::new(None::<Task>)));
    use_effect(move || {
        let seconds = interval_seconds();
        if let Some(task) = poll_task.take() {
            task.cancel();
        }
        if seconds == 0 {
            return;
        }
        let delay_millis = u32::try_from(seconds.saturating_mul(1_000)).unwrap_or(u32::MAX);
        let visibility = visibility.clone();
        let mut on_poll = on_poll.clone();
        poll_task.set(Some(spawn(async move {
            loop {
                sleep_while_visible(&visibility, delay_millis).await;
                on_poll();
            }
        })));
    });
}

/// 自动轮询开关控件：单个下拉，可选关闭与 10/30/60 秒；默认值由页面 query 决定
/// （仪表盘默认 30 秒自动轮询），变更由页面写回 URL query。
#[component]
pub(crate) fn AutoPollControls(poll_seconds: u64, on_change: EventHandler<u64>) -> Element {
    rsx! {
        label { class: "admin-poll-control",
            {t!("admin-poll-label")}
            select {
                value: poll_seconds.to_string(),
                onchange: move |event| {
                    // 下拉只提供合法闭集；解析失败保持现状。
                    if let Ok(seconds) = event.value().parse::<u64>() {
                        on_change.call(seconds);
                    }
                },
                option { value: "0", {t!("admin-poll-off")} }
                option { value: "10", {t!("admin-poll-interval", seconds: 10)} }
                option { value: "30", {t!("admin-poll-interval", seconds: 30)} }
                option { value: "60", {t!("admin-poll-interval", seconds: 60)} }
            }
        }
    }
}
