//! 履约进度的逐件事实：lot 终态、结算明细、各义务的履约原值、recovery 与装备损失。
//!
//! 状态原值（义务种类与业务状态、履约执行状态、投递状态、recovery 阶段与撤销结论）保持数据库
//! 闭集字符串，由 `status` 模块解析，遇到未知值时失败关闭；这里只校验标识、文本与数值形状。

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// 单个 lot 的一项结算义务，以及它当前履约行与关联投递的状态原值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentObligation {
    /// `settlement_obligations.obligation_kind` 原值。
    pub kind: String,
    /// `settlement_obligations.business_state` 原值。
    pub business_state: String,
    /// 履约行的 `execution_state` 原值；尚未建立履约行时为 `None`。
    pub execution_state: Option<String>,
    /// 履约行关联投递的 `status` 原值；尚未关联投递时为 `None`。
    pub delivery_status: Option<String>,
}

/// 成交 lot 的付款恢复事实；每件至多一行，历史成交可能缺失。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentRecovery {
    /// `settlement_recoveries.stage` 原值。
    pub stage: String,
    /// 撤销结论原值；首次落定即表示成交已撤销。
    pub resolution: Option<String>,
    /// 当前付款期限；COD 重寄时从重寄时刻重新起算。
    pub payment_deadline_at: DateTime<Utc>,
    pub cod_return_count: i32,
    /// 该 recovery 冻结的 COD 退回上限。
    pub cod_return_limit: i32,
    /// 阻断码原值，只用于判定、不进入公开输出；付款提醒失败同样写它，因此只在撤销与退件阶段
    /// 生效。
    pub blocking_code: Option<String>,
    /// 是否登记了待人工对账的投递；只用于判定，不携带投递标识。
    pub manual_reconcile: bool,
    /// 成交撤销后是否已重新上架。
    pub relisted: bool,
}

impl PublicFulfillmentRecovery {
    /// 校验退回计数与冻结上限的数据库约束，以及期限与阻断码形状。
    fn validate(&self) -> Result<(), &'static str> {
        if !(1..=10).contains(&self.cod_return_limit)
            || !(0..=self.cod_return_limit).contains(&self.cod_return_count)
            || self.payment_deadline_at.timestamp_micros() == 0
            || self
                .blocking_code
                .as_deref()
                .is_some_and(|code| !auction_domain::valid_text(code, 128))
        {
            return Err("公开履约 recovery 事实无效");
        }
        Ok(())
    }
}

/// 单个已结算 lot 的公开展示字段与判定所需的全部事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicFulfillmentLot {
    pub lot_id: Uuid,
    /// 类别内编号，与竞价日志、补偿池明细同一口径。
    pub lot_number: i32,
    pub equipment_name: String,
    pub equipment_category: String,
    /// 投稿卖家名；没有投稿记录的历史 lot 为 `—`，与补偿池明细同一口径。
    pub seller_name: String,
    /// `lots.status` 原值；读取范围只含 `sold` 与 `unsold`。
    pub lot_status: String,
    /// 成交 lot 的买家当前用户名；流拍为 `None`。
    pub buyer_name: Option<String>,
    /// lot 结算明细的成交价；缺结算事实时为 `None`，流拍为 0。
    pub final_price: Option<i64>,
    /// 是否存在人工装备损失处置事实。
    pub equipment_lost: bool,
    pub recovery: Option<PublicFulfillmentRecovery>,
    /// 该 lot 的全部结算义务；每种至多一项，重复由状态解析失败关闭。
    pub obligations: Vec<PublicFulfillmentObligation>,
}

impl PublicFulfillmentLot {
    /// 校验标识、展示文本、金额与履约/投递原值的成对关系；状态闭集由状态解析校验。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.lot_id.is_nil()
            || self.lot_number <= 0
            || !auction_domain::valid_text(&self.equipment_name, 256)
            || !auction_domain::valid_text(&self.equipment_category, 64)
            || !auction_domain::valid_text(&self.seller_name, 256)
            || self.buyer_name.as_deref().is_some_and(|name| !auction_domain::valid_text(name, 256))
            || self.final_price.is_some_and(|price| price < 0)
            || self.obligations.iter().any(|obligation| {
                obligation.execution_state.is_none() && obligation.delivery_status.is_some()
            })
        {
            return Err("公开履约 lot 事实无效");
        }
        self.recovery.as_ref().map_or(Ok(()), PublicFulfillmentRecovery::validate)
    }
}
