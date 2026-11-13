//! 三角色共用的通知模板周期刷新；叠加与替换规则由应用服务拥有。

use crate::worker::diagnostics;
use auction_application::core::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use auction_application::mail_templates::{
    LiveMailTemplates, MailTemplateRefreshService, MailTemplateStore,
};
use std::{sync::Arc, time::Duration};

/// 刷新间隔：Admin 保存模板后，各角色最迟在这个时间内用上新正文。
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// 单次读取的等待上限；超时与失败一样保留上一次成功的快照。
const REFRESH_TIMEOUT: Duration = Duration::from_secs(1);

/// 启动即读取一次，随后周期刷新；调用方用 TaskGuard 管理返回任务。
///
/// 内存定时器只负责唤醒：模板以数据库里的修订事实为准，刷新失败时继续使用上一次成功的
/// 快照（从未成功读取时是内置默认正文），不影响任何金额、资格或幂等判断。
pub async fn start_mail_template_refresh(
    store: Arc<dyn MailTemplateStore>,
    templates: LiveMailTemplates,
) -> tokio::task::JoinHandle<()> {
    let service = MailTemplateRefreshService::new(store, templates);
    let mut state = RefreshState::default();
    refresh(&service, &mut state).await;
    tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + REFRESH_INTERVAL,
            REFRESH_INTERVAL,
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            refresh(&service, &mut state).await;
        }
    })
}

/// 只用于日志去重：同一组被跳过的模板、连续的读取失败都只在变化时记录一次。
#[derive(Default)]
struct RefreshState {
    skipped: Vec<String>,
    failing: bool,
}

async fn refresh(service: &MailTemplateRefreshService, state: &mut RefreshState) {
    match tokio::time::timeout(REFRESH_TIMEOUT, service.refresh()).await {
        Ok(Ok(refresh)) => {
            if state.failing {
                tracing::info!("通知模板读取已恢复");
            }
            state.failing = false;
            if refresh.changed {
                tracing::info!("通知模板快照已更新");
            }
            if refresh.skipped != state.skipped {
                if !refresh.skipped.is_empty() {
                    // 只记录模板键，不记录正文。
                    tracing::warn!(
                        templates = refresh.skipped.join(","),
                        "数据库里的通知模板不满足现行占位符规则，已改用内置默认正文"
                    );
                }
                state.skipped = refresh.skipped;
            }
        }
        attempt => {
            let (phase, safe_kind) = match &attempt {
                Ok(Err(cause)) => ("refresh_snapshot", cause.diagnostic_kind()),
                Err(elapsed) => ("refresh_snapshot_timeout", diagnostics::timeout_kind(elapsed)),
                Ok(Ok(_)) => return,
            };
            if !state.failing {
                record_error(
                    DiagnosticLevel::Warn,
                    safe_kind,
                    "mail_templates",
                    phase,
                    DiagnosticContext::default(),
                );
            }
            state.failing = true;
        }
    }
}
