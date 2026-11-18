//! 当前会话中挂载的读取视图；按实体分发命令了结后的失效通知。

use std::rc::{Rc, Weak};

use dioxus::prelude::*;

use super::{CommandNotice, PendingCommandGate, PendingCommandHost, PendingCommands};

pub(super) struct CommandSource {
    pub(super) reload: Signal<u64>,
    pub(super) gate: Option<PendingCommandGate>,
    pub(super) rejections: Signal<Vec<CommandNotice>>,
    matches_entity: Box<dyn Fn(&str) -> bool>,
}

/// 注册当前挂载的读取视图。投影按实体或集合匹配已了结的命令。
pub(crate) fn use_pending_commands(
    reload: Signal<u64>,
    matches_entity: impl Fn(&str) -> bool + 'static,
) -> PendingCommands {
    use_pending_commands_with_gate(reload, None, matches_entity)
}

/// 页级门禁随本次挂载结束；当前页面的重读订阅由实体匹配决定。
pub(crate) fn use_pending_commands_with_gate(
    reload: Signal<u64>,
    gate: Option<PendingCommandGate>,
    matches_entity: impl Fn(&str) -> bool + 'static,
) -> PendingCommands {
    let host = use_context::<PendingCommandHost>();
    let rejections = use_signal(Vec::new);
    let source = use_signal(|| {
        Rc::new(CommandSource {
            reload,
            gate,
            rejections,
            matches_entity: Box::new(matches_entity),
        })
    });
    let registration = use_hook(move || {
        let mut sources = host.sources;
        let registration = Rc::downgrade(&source.peek());
        sources.write().push(registration.clone());
        registration
    });
    use_drop(move || {
        let mut sources = host.sources;
        // 显式注销让外壳立即重新计算重试和回执入口。同一会话的在途请求仍由宿主持有。
        if let Ok(mut sources) = sources.try_write() {
            sources.retain(|source| !Weak::ptr_eq(source, &registration));
        }
    });
    use_context_provider(|| PendingCommands { host, source })
}

impl PendingCommandHost {
    pub(super) fn live_source(self, source: &Weak<CommandSource>) -> Option<Rc<CommandSource>> {
        self.sources.read().iter().find(|mounted| Weak::ptr_eq(mounted, source))?.upgrade()
    }

    pub(super) fn reloads_for(self, entity: &str) -> Vec<(Signal<u64>, u64)> {
        self.sources
            .peek()
            .iter()
            .filter_map(Weak::upgrade)
            .filter_map(|source| {
                if !(source.matches_entity)(entity) {
                    return None;
                }
                let generation = *source.reload.try_peek().ok()?;
                Some((source.reload, generation))
            })
            .collect()
    }
}

/// 草稿清理和当前读取世代一起推进，防止较早的读取结果重新装配旧数据。
pub(in crate::surface) fn advance_command_reload(mut reload: Signal<u64>) {
    if let Ok(mut generation) = reload.try_write() {
        *generation = generation.saturating_add(1);
    }
}
