//! 主内容读取状态的唯一登记处：各页把“当前查询还没有可展示的结果”登记到这里，外壳在顶栏
//! 显示同一个加载提示；正文不再插入会被数据替换的占位块，避免加载块撑开又收回容器。

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

use dioxus::prelude::*;

/// 仍在等待首个结果的读取集合；外壳只关心它是否为空。
#[derive(Clone, Copy)]
pub(crate) struct PublicContentLoading(pub Signal<BTreeSet<u64>>);

static NEXT_LOADER_ID: AtomicU64 = AtomicU64::new(0);

/// 把一次读取的等待状态登记到顶栏。`loading` 只应在当前查询没有任何可展示结果时为真：
/// 同查询的后台重读保留旧结果，不登记。每次调用占一个独立登记项，组件卸载时撤销，
/// 中途离开页面不会把提示留在顶栏。
pub(crate) fn use_content_loading(loading: bool) {
    let mut active = use_context::<PublicContentLoading>().0;
    let loader_id = use_hook(|| NEXT_LOADER_ID.fetch_add(1, Ordering::Relaxed));
    use_effect(use_reactive((&loading,), move |(loading,)| {
        // 状态未变化时不写信号，避免外壳为无效更新重渲染。
        if active.peek().contains(&loader_id) == loading {
            return;
        }
        let mut loaders = active.write();
        if loading {
            loaders.insert(loader_id);
        } else {
            loaders.remove(&loader_id);
        }
    }));
    use_drop(move || {
        if let Ok(mut loaders) = active.try_write() {
            loaders.remove(&loader_id);
        }
    });
}
