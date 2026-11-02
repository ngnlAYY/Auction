//! 结算异常的显式运维参数；默认预览，执行必须完整声明人工责任与冻结版本。

use clap::{Args, Subcommand};
use uuid::Uuid;

#[derive(Debug, Subcommand)]
pub(crate) enum SettlementCommand {
    /// 装备已损失时预览人工收口；--execute 才记录责任并释放原冻结卖方货款。
    EquipmentLoss(EquipmentLossArgs),
    /// 用人工观察的 Sent 证据预览确认；--execute 才追加人工证明并恢复原履约。
    ConfirmSent(ConfirmSentArgs),
}

#[derive(Debug, Args)]
pub(crate) struct EquipmentLossArgs {
    /// 已结算成交 lot 的非零 UUID。
    pub(crate) lot_id: Uuid,
    /// 显式执行；省略时只读预览，执行参数不得用于预览。
    #[arg(long, requires_all = [
        "command_id", "actor", "reason", "funding_uid", "expected_recovery_version",
        "expected_payment_version", "expected_delivery_fencing_version", "seller_amount",
        "buyer_compensation_external", "confirm",
    ])]
    pub(crate) execute: bool,
    /// 稳定非零命令 UUID；结果未知时必须保持原 ID 与全部参数。
    #[arg(long, requires = "execute")]
    pub(crate) command_id: Option<Uuid>,
    /// 写入审计的操作者标识。
    #[arg(long, requires = "execute")]
    pub(crate) actor: Option<String>,
    /// 装备损失原因及人工处置说明；不应含凭据或秘密。
    #[arg(long, requires = "execute")]
    pub(crate) reason: Option<String>,
    /// preview 给出的已就绪 record_only 资金账户 UID。
    #[arg(long, requires = "execute")]
    pub(crate) funding_uid: Option<String>,
    /// preview 给出的恢复版本。
    #[arg(long, requires = "execute")]
    pub(crate) expected_recovery_version: Option<u64>,
    /// preview 给出的买方付款履约版本。
    #[arg(long, requires = "execute")]
    pub(crate) expected_payment_version: Option<u64>,
    /// preview 给出的原装备投递 fencing 版本。
    #[arg(long, requires = "execute")]
    pub(crate) expected_delivery_fencing_version: Option<i64>,
    /// preview 给出的原冻结卖方货款（Credits）；只是金额断言，不改变结算。
    #[arg(long, requires = "execute")]
    pub(crate) seller_amount: Option<i64>,
    /// 声明操作者承担买方线下补偿；不表示已补偿或系统已收取 COD。
    #[arg(long, requires = "execute")]
    pub(crate) buyer_compensation_external: bool,
    /// 逐字使用 preview 输出的确认短语。
    #[arg(long, value_name = "TEXT", requires = "execute")]
    pub(crate) confirm: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ConfirmSentArgs {
    /// 待确认投递的非零 UUID。
    pub(crate) delivery_id: Uuid,
    /// 人工观察 JSON 普通文件；最多 64 KiB，拒绝符号链接与特殊文件。
    #[arg(long, value_name = "JSON")]
    pub(crate) evidence: std::path::PathBuf,
    /// 显式执行；省略时只读预览。
    #[arg(long, requires_all = [
        "command_id", "expected_fencing_version", "actor", "reason", "confirm",
    ])]
    pub(crate) execute: bool,
    /// 稳定非零命令 UUID；结果未知时保持原 ID 与全部参数。
    #[arg(long, requires = "execute")]
    pub(crate) command_id: Option<Uuid>,
    /// preview 返回的投递 fencing 版本。
    #[arg(long, requires = "execute")]
    pub(crate) expected_fencing_version: Option<i64>,
    /// 写入审计的操作者标识。
    #[arg(long, requires = "execute")]
    pub(crate) actor: Option<String>,
    /// 原生证据来源及人工关联责任说明，不得含凭据。
    #[arg(long, requires = "execute")]
    pub(crate) reason: Option<String>,
    /// 明确接受已收取附件后无法自动核对原 EID 的人工责任。
    #[arg(long, requires = "execute")]
    pub(crate) accept_collected_assets: bool,
    /// 明确接受旧尝试关联不完整的人工责任；保留原尝试历史。
    #[arg(long, requires = "execute")]
    pub(crate) accept_attempt_history: bool,
    /// 逐字使用本证据 preview 返回的确认短语。
    #[arg(long, value_name = "TEXT", requires = "execute")]
    pub(crate) confirm: Option<String>,
}
