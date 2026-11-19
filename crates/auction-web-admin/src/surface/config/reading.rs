//! 运行配置读取世代与表单快照装配。

use auction_contracts::AdminRuntimeConfigResponse;
use auction_contracts::AdminWorkerRestartStateDto;
use dioxus::prelude::*;

use super::draft::ConfigDraft;
use super::model::RuntimeConfigForm;
use super::validation::FieldIssue;
use crate::api::fetch_admin_runtime_config;
use crate::surface::pending::PendingCommands;

/// 读取完成后的投影处理结果。
pub(super) enum ConfigReadStatus {
    Ready(String),
    Conflict,
    InvalidForm,
    Error(String),
}

/// 本轮读取已处理，可据此显示状态或校验写入基线。
pub(super) struct CompletedConfigRead {
    pub(super) generation: u64,
    pub(super) status: ConfigReadStatus,
    pub(super) restart_required: bool,
    pub(super) worker_restart_state: Option<AdminWorkerRestartStateDto>,
}

impl CompletedConfigRead {
    /// 只有本轮读取接受的基线指纹能授权配置操作。
    pub(super) fn matches_baseline(&self, generation: u64, fingerprint: &str) -> bool {
        self.generation == generation
            && matches!(&self.status, ConfigReadStatus::Ready(loaded) if loaded == fingerprint)
    }
}

/// 记录本次挂载是否已有编辑器，以及已处理的读取结果。新读取世代在结果处理完前不能发起操作。
pub(super) fn use_config_read(
    draft: ConfigDraft,
    pending: PendingCommands,
    issues: Signal<Vec<FieldIssue>>,
) -> (Signal<bool>, Signal<Option<CompletedConfigRead>>) {
    let mut editor_mounted = use_signal(|| false);
    let mut completed = use_signal(|| None::<CompletedConfigRead>);
    let config = use_resource(move || {
        let generation = (draft.refresh)();
        async move {
            #[cfg(test)]
            if generation != 0
                && let Some(test) = try_consume_context::<ConfigReadGate>()
                && let Err(code) = test.0.fetch().await
            {
                return (generation, Err(crate::api::AdminApiFailure { status: None, code }));
            }
            (generation, fetch_admin_runtime_config().await)
        }
    });

    use_effect(move || {
        let generation = (draft.refresh)();
        let snapshot = config.read();
        if config.state().cloned() != UseResourceState::Ready {
            return;
        }
        let Some((loaded_generation, result)) = snapshot.as_ref() else { return };
        // Resource 在挂起期间保留旧值。只准本轮结果装配草稿及开放操作。
        if *loaded_generation != generation {
            return;
        }
        let (status, restart_required, worker_restart_state) = match result {
            Ok(value) => (
                apply_config_read(draft, pending, issues, value),
                value.restart_required,
                value.worker_restart_state,
            ),
            // 失败不能替换最近一次成功读取的 Worker 状态。
            Err(error) => (
                ConfigReadStatus::Error(error.code.clone()),
                false,
                completed.peek().as_ref().and_then(|read| read.worker_restart_state),
            ),
        };
        if matches!(status, ConfigReadStatus::Ready(_) | ConfigReadStatus::Conflict) {
            editor_mounted.set(true);
        }
        completed.set(Some(CompletedConfigRead {
            generation,
            status,
            restart_required,
            worker_restart_state,
        }));
    });

    (editor_mounted, completed)
}

fn apply_config_read(
    mut draft: ConfigDraft,
    pending: PendingCommands,
    mut issues: Signal<Vec<FieldIssue>>,
    value: &AdminRuntimeConfigResponse,
) -> ConfigReadStatus {
    let matches_baseline = draft.fingerprint.peek().as_deref() == Some(value.fingerprint.as_str());
    let has_baseline = draft.baseline.peek().is_some();
    // 未决命令保留提交时的基线、指纹和冻结负载，直到原命令了结。
    if has_baseline && draft.command.peek().is_some() && pending.contains("runtime-config") {
        return if matches_baseline {
            ConfigReadStatus::Ready(value.fingerprint.clone())
        } else {
            ConfigReadStatus::Conflict
        };
    }
    let Some(taxonomy) = value.equipment_taxonomy.as_ref() else {
        return ConfigReadStatus::InvalidForm;
    };
    if has_baseline {
        let dirty = draft
            .baseline
            .peek()
            .as_ref()
            .is_some_and(|initial| !initial.same_edits_as(&draft.form.peek()));
        if matches_baseline || dirty {
            // 词典独立于文档指纹演进。保持表单与基线的原始值及编辑记录。
            draft.form.write().refresh_equipment_taxonomy(taxonomy);
            if let Some(initial) = draft.baseline.write().as_mut() {
                initial.refresh_equipment_taxonomy(taxonomy);
            }
            if matches_baseline {
                return ConfigReadStatus::Ready(value.fingerprint.clone());
            }
            // 已观察到基线变化，旧预校验不能授权写入，即使以后又读到原指纹。
            draft.checked_signature.set(String::new());
            return ConfigReadStatus::Conflict;
        }
    }
    let Ok(next_form) = RuntimeConfigForm::from_response(value) else {
        return ConfigReadStatus::InvalidForm;
    };
    draft.baseline.set(Some(next_form.clone()));
    draft.form.set(next_form);
    draft.fingerprint.set(Some(value.fingerprint.clone()));
    draft.remember_restart_notice(value.restart_required);
    draft.checked_signature.set(String::new());
    draft.command.set(None);
    draft.notice.set(None);
    issues.set(Vec::new());
    ConfigReadStatus::Ready(value.fingerprint.clone())
}

#[cfg(test)]
#[derive(Clone)]
/// 宿主回归可等待并控制后台 GET 完成，成功读仍走生产投影解码。
pub(super) struct ConfigReadGate(
    pub(super) std::rc::Rc<crate::surface::auctions::draft_page_tests::RecordRead<()>>,
);
