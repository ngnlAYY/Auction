//! MoogleMail 业务分组发送节奏；仅约束 Send 的开始，不替代账号串行或 HTTP 限速。

use serde::{Deserialize, Serialize};

/// 同类两封邮件进入 sending 的最小间距，单位秒；Main/Isekai 共用同一账号节奏。
/// 全零保持旧指纹与默认吞吐；0 不附加等待，取件和原件退回不受此策略影响。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DeliverySendIntervals {
    /// 超价提醒；默认 0，提交后优先唤醒。
    pub outbid_seconds: u64,
    /// 账号注册、登录 Key 与重绑回复。
    pub account_seconds: u64,
    /// 寄售接收/拒绝回执及论坛竞价通知。
    pub intake_seconds: u64,
    /// 结算付款、退款与付款提醒。
    pub settlement_seconds: u64,
    /// 装备交付、重新寄送和流拍寄回。
    pub equipment_seconds: u64,
    /// 运维告警。
    pub operations_seconds: u64,
    /// 手工及未归入其他业务组的发送。
    pub manual_seconds: u64,
}

impl DeliverySendIntervals {
    /// 稳定分组键供持久化适配器映射；每项允许 0–86400 秒。
    pub const fn values(&self) -> [(&'static str, u64); 7] {
        [
            ("outbid", self.outbid_seconds),
            ("account", self.account_seconds),
            ("intake", self.intake_seconds),
            ("settlement", self.settlement_seconds),
            ("equipment", self.equipment_seconds),
            ("operations", self.operations_seconds),
            ("manual", self.manual_seconds),
        ]
    }

    /// 配置边界严格拒绝越界，不静默缩短等待。
    pub fn is_valid(&self) -> bool {
        self.values().iter().all(|(_, seconds)| *seconds <= 86_400)
    }

    /// 缺省全零不改变既有 runtime 文档指纹。
    pub fn is_zero(&self) -> bool {
        self.values().iter().all(|(_, seconds)| *seconds == 0)
    }
}

/// 提交后通知仅用于加速唤醒；丢失/断线仍由持久队列及周期领取恢复。
pub trait DeliveryWakeup: Send {
    /// 等待下一次提示，提示不携带用户、邮件正文或凭据信息。
    fn wait(&mut self) -> crate::ports::auction::ApplicationFuture<'_, ()>;
}
