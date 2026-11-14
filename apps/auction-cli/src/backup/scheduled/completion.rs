//! 校验后的副本完成边界；异机失败保留本地备份，旧运行终态只追加补充事实。

use auction_config::AppConfig;

use super::{
    filesystem::PrivateDirectory,
    journal::Journal,
    model::{Outcome, RunReceipt},
    offsite,
};

/// 所有已启用的副本步骤成功后才允许进入本次本地保留清理。
pub(super) async fn after_verify(
    journal: &Journal,
    config: &AppConfig,
    output: &PrivateDirectory,
    receipt: &mut RunReceipt,
    prepared: Option<&offsite::Prepared>,
) -> anyhow::Result<()> {
    offsite::after_verify(journal, config, output, receipt, prepared).await
}

/// 后续关闭 offsite 不能放宽旧任务的副本要求；重试追加的完成事实不能改写失败回执。
pub(super) fn allows_cleanup(journal: &Journal, receipt: &RunReceipt) -> anyhow::Result<bool> {
    if !receipt.offsite.enabled {
        return Ok(receipt.outcome == Outcome::Completed);
    }
    if receipt.outcome == Outcome::Completed && receipt.offsite.daily.is_some() {
        return Ok(true);
    }
    offsite::completed_for(journal, receipt)
}
