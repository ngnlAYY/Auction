//! 当前管理会话内的非敏感配置草稿；不使用浏览器持久化存储。

use auction_contracts::AdminRuntimeConfigResponse;
use dioxus::prelude::*;
use serde_json::Value;

use super::model::RuntimeConfigForm;
use super::page::ConfigCommand;
use super::page::ConfigNotice;
use crate::api::{AdminApiFailure, validate_admin_runtime_config};
use crate::surface::pending::{advance_command_reload, set_if_alive};
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
    pub(super) refresh: Signal<u64>,
    pub(super) validating: Signal<bool>,
}

impl ConfigDraft {
    /// 在认证会话中预校验候选。回执只适用于发出时的基线和草稿内容。
    pub(super) fn validate(mut self, fingerprint: String, document: Value, signature: String) {
        let candidate = self.form.peek().clone();
        self.validating.set(true);
        dioxus::core::Runtime::current().in_scope(self.form.origin_scope(), || {
            spawn(async move {
                let result = validate_candidate(document).await;
                // 配置页可能已经卸载；只有会话基线及原始编辑仍相同时才能采用回执。
                let same_baseline =
                    self.fingerprint.peek().as_deref() == Some(fingerprint.as_str());
                if same_baseline && self.form.peek().same_edits_as(&candidate) {
                    let notice = match result {
                        Ok(result) => {
                            if let Some(taxonomy) = result.equipment_taxonomy.as_ref() {
                                self.form.write().refresh_equipment_taxonomy(taxonomy);
                                self.checked_signature.set(signature);
                                self.command.set(None);
                                ConfigNotice::Validated
                            } else {
                                ConfigNotice::InvalidForm
                            }
                        }
                        Err(error) if error.is_conflict() => ConfigNotice::Conflict,
                        Err(error) => ConfigNotice::ActionError(error.code),
                    };
                    self.notice.set(Some(notice));
                }
                self.validating.set(false);
            });
        });
    }

    pub(super) fn remember_restart_notice(self, required: bool) {
        // 通用重启提醒涵盖所有受影响的进程，Worker 冷配置状态不能清除它。
        if required {
            set_if_alive(self.restart_required, true);
        }
    }

    pub(super) fn apply_saved_config(
        self,
        result: AdminRuntimeConfigResponse,
        next_form: RuntimeConfigForm,
        requires_restart: bool,
    ) {
        // 只在写入成功后替换基线和版本围栏，同时解除本次命令及校验签名。
        set_if_alive(self.baseline, Some(next_form.clone()));
        set_if_alive(self.form, next_form);
        set_if_alive(self.checked_signature, String::new());
        set_if_alive(self.fingerprint, Some(result.fingerprint));
        // 写入响应采用通用重启标志。实际差异决定本次是否增加提醒，既有提醒继续保留。
        self.remember_restart_notice(result.restart_required && requires_restart);
        set_if_alive(self.command, None);
        advance_command_reload(self.refresh);
    }
}

async fn validate_candidate(
    document: Value,
) -> Result<AdminRuntimeConfigResponse, AdminApiFailure> {
    #[cfg(test)]
    if let Some(stub) = try_consume_context::<ConfigValidationStub>() {
        assert_eq!(document["operations"]["forum"]["lease_duration_seconds"], 600);
        return stub.0.fetch().await.map_err(|code| AdminApiFailure { status: None, code });
    }
    validate_admin_runtime_config(document).await
}

/// 宿主回归替换预校验传输，允许在请求挂起期间切换页面。
#[cfg(test)]
#[derive(Clone)]
pub(super) struct ConfigValidationStub(
    pub(super)  std::rc::Rc<
        crate::surface::auctions::draft_page_tests::RecordRead<AdminRuntimeConfigResponse>,
    >,
);

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
        refresh,
        validating,
    });
    use_unload_warning(dirty);
}
