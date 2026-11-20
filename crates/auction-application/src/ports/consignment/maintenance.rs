//! 公平补扫、停用清退与启动代次同步。

use uuid::Uuid;

use crate::ports::consignment::commands::AutoCloseCandidate;
use crate::ports::consignment::errors::ConsignmentFuture;

/// 补扫单页的固定挂单扫描上限。
pub const LISTING_MAINTENANCE_SCAN_LIMIT: u32 = 50;

/// 扫过的一页活动挂单及其中已经观察到的清退条件。
#[derive(Debug)]
pub struct ListingMaintenancePage {
    /// 按持久游标查看的活动挂单数，可能非零但本页没有候选。
    pub scanned: u32,
    /// 扫描所得候选，每单实际提交时重新查证据。
    pub candidates: Vec<AutoCloseCandidate>,
    /// 已推进的持久游标。到末端回绕为 `None`。
    pub scan_after: Option<Uuid>,
}

/// 启动时同步后的权威持久准入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingAdmission {
    /// 工作和挂单从属的启用代次。
    pub service_epoch: u64,
    /// 持久准入是否开放。
    pub admitting: bool,
}

/// 寄售启动和公平补扫端口。不变量 2、5、6、8：扫描的候选与游标不授权写入；
/// 每单交给六类命令的自动清退事务重新核实。
pub trait ListingMaintenance: Send + Sync {
    /// 用持久游标扫描至多 50 单，挑出碰撞、失资格、停用与已到期的登记，到尾回绕。
    /// 关闭且无存量时返回空页。后到的新证据由后续完整扫描处理。
    fn scan_candidates(&self) -> ConsignmentFuture<'_, ListingMaintenancePage>;

    /// 启动前短事务同步配置。仅 true→false 增加代次并使旧心跳失效。保留在途
    /// 许可时间栅栏，按工作记录的原始网络硬截止加余量处理重启后的关停。
    /// 旧单读侧即时关闭，每单的终结和凭证退休交给自动清退。
    fn sync_admission(&self, enabled: bool) -> ConsignmentFuture<'_, ListingAdmission>;
}
