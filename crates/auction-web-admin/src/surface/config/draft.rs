//! 当前管理会话内的非敏感配置草稿；不使用浏览器持久化存储。

use dioxus::prelude::*;

use super::{ConfigCommand, ConfigNotice, model::RuntimeConfigForm};
use crate::surface::pending::{PendingCommands, use_pending_commands};
use crate::surface::presentation::use_unload_warning;

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
    pub(super) pending: PendingCommands,
    pub(super) refresh: Signal<u64>,
    pub(super) validating: Signal<bool>,
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
    let refresh = use_signal(|| 0_u64);
    let pending = use_pending_commands(refresh);
    let validating = use_signal(|| false);
    let dirty = use_memo(move || {
        baseline.read().as_ref().is_some_and(|initial| !initial.same_edits_as(&form.read()))
    });
    use_context_provider(|| ConfigDraft {
        form,
        baseline,
        fingerprint,
        checked_signature,
        command,
        notice,
        restart_required,
        dirty,
        pending,
        refresh,
        validating,
    });
    use_unload_warning(dirty);
}
