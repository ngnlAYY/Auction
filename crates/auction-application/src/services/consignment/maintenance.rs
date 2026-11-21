//! 启动准入同步和有界的到期、碰撞、失资格与停用清退。

use std::sync::Arc;

use auction_domain::consignment::ListingVersion;

use crate::ports::consignment::{
    ConsignmentError, LISTING_MAINTENANCE_SCAN_LIMIT, ListingAdmission, ListingCommandStore,
    ListingMaintenance,
};
use crate::services::consignment::encoding::auto_close_command;

/// 一页维护已完成后交给 Worker 的推进提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingMaintenanceProgress {
    /// 本页扫描到的挂单数。
    pub scanned: u32,
    /// 得到已提交清退决定的候选数，包含原成功回执的重放。
    pub processed: u32,
    /// 持久游标尚未回绕，建议继续扫描。
    pub has_more: bool,
}

/// 启动时同步代次，运行时每次扫描并清退一页。
pub struct ListingMaintenanceService {
    maintenance: Arc<dyn ListingMaintenance>,
    commands: Arc<dyn ListingCommandStore>,
}

impl ListingMaintenanceService {
    /// 注入读取候选和同步代次的端口，以及负责自动清退的命令仓储。
    pub fn new(
        maintenance: Arc<dyn ListingMaintenance>,
        commands: Arc<dyn ListingCommandStore>,
    ) -> Self {
        Self { maintenance, commands }
    }

    /// 在服务恢复前提交配置切换。只有成功返回才可用持久准入和代次装配服务。
    pub async fn sync_admission(
        &self,
        enabled: bool,
        artifact_capable: bool,
    ) -> Result<ListingAdmission, ConsignmentError> {
        if enabled && !artifact_capable {
            return Err(ConsignmentError::Disabled);
        }
        // 不变量 2、5、6：无库存探针时，不能把无法读取数据库视作空库或切换成功。
        // 启动方应在旧角色停止后、监听或领取之前等待此事务完成。
        let admission = self.maintenance.sync_admission(enabled).await?;
        if admission.admitting != enabled
            || !(1..=ListingVersion::MAX).contains(&admission.service_epoch)
        {
            return Err(ConsignmentError::Unavailable);
        }
        Ok(admission)
    }

    /// 扫描一页并逐单提交，错误交给调用方。端口已持久化游标，后续回绕补扫遗漏条件。
    pub async fn sweep_page(&self) -> Result<ListingMaintenanceProgress, ConsignmentError> {
        let page = self.maintenance.scan_candidates().await?;
        if page.scanned > LISTING_MAINTENANCE_SCAN_LIMIT
            || page.candidates.len() > page.scanned as usize
        {
            return Err(ConsignmentError::Unavailable);
        }
        let mut processed = 0;
        for candidate in page.candidates {
            // 不变量 1、2、3、5、6、7、8：候选不构成写入授权，每单重新举证并独立提交；
            // 已提交的前缀不因后续错误回滚，尚无证据的命令不占账本。
            match self.commands.auto_close(auto_close_command(candidate)).await?.into_result() {
                Ok(_effect) => processed += 1,
                Err(ConsignmentError::ConditionNotMet) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(ListingMaintenanceProgress {
            scanned: page.scanned,
            processed,
            has_more: page.scan_after.is_some(),
        })
    }
}
