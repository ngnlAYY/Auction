//! Public 竞价资源失效与有界合并读取；不修改事件游标或推导竞价结果。

use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;

/// 从首次待处理通知起合并 100 毫秒；后续竞价不延后截止时间。
const BID_REFRESH_COALESCE_MILLIS: u32 = 100;

/// Public 外壳持有的竞价失效修订号，与生命周期和人工刷新的全局修订号分离。
#[derive(Clone, Copy)]
pub(crate) struct PublicBidRefresh(pub(crate) Signal<u64>);

impl PublicBidRefresh {
    pub(crate) fn notify(mut self) {
        let next = self.0.peek().saturating_add(1);
        self.0.set(next);
    }
}

/// 仅刷新已挂载且启用的竞价资源；进行中的读取先完成，再补读尚未消费的通知。
/// 竞价修订号不能直接成为 `use_resource` 的依赖，否则持续竞价会反复取消慢请求。
/// 延后任务由消费组件拥有：卸载会取消任务，新挂载的资源自行读取最新权威快照。
pub(crate) fn use_bid_refresh<T: 'static>(mut resource: Resource<T>, enabled: bool) {
    let refresh = use_context::<PublicBidRefresh>();
    let enabled = use_memo(use_reactive((&enabled,), |(enabled,)| enabled));
    let mut consumed = use_signal(|| *refresh.0.peek());
    let mut scheduled = use_signal(|| false);

    use_effect(move || {
        if !enabled() {
            // 闭合的代理面板不订阅竞价或资源状态，避免每行随通知执行空刷新。
            consumed.set(*refresh.0.peek());
            return;
        }
        let revision = (refresh.0)();
        let state = resource.state().cloned();
        if revision == *consumed.peek() || *scheduled.peek() || state != UseResourceState::Ready {
            return;
        }

        scheduled.set(true);
        spawn(async move {
            TimeoutFuture::new(BID_REFRESH_COALESCE_MILLIS).await;
            scheduled.set(false);
            let revision = *refresh.0.peek();
            // 等待期间可能切换查询、关闭代理面板或触发全局刷新；只重启空闲资源。
            if *enabled.peek()
                && revision != *consumed.peek()
                && *resource.state().peek() == UseResourceState::Ready
            {
                consumed.set(revision);
                resource.restart();
            }
        });
    });
}
