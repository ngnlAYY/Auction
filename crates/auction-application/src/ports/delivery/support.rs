// 玩家支持只读汇总端口。
//
// 按用户名（或用户 ID）或一个参考编号汇总出价、成交、义务、投递与通知，供 Admin 处理
// 玩家咨询与核对冒充 Bot 的邮件。所有分区都有固定行数上限，且只返回安全投影：不含投递
// payload、正文、Cookie、登录 key、会话凭据或代理上限。

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::ports::delivery::DeliveryAdminSnapshot;

/// 每个分区最多返回的行数；超过时置 `truncated`，不静默分页。
pub const MAX_PLAYER_SUPPORT_ROWS: i64 = 50;
/// 用户名筛选的最大字符数；与 Admin 输入框 maxlength 对齐。
pub const MAX_PLAYER_SUPPORT_USER_LEN: usize = 256;

/// 只读汇总的检索入口；两者互斥，必须严格提供一个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerSupportSelector {
    /// 已裁剪的用户名或规范用户 UUID 文本。
    User(String),
    /// 投递、义务、通知、lot 或场次的规范 UUID 参考编号。
    Reference(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportRequest {
    pub selector: PlayerSupportSelector,
}

impl PlayerSupportRequest {
    /// 校验单一检索入口与有界文本；带连字符的用户输入必须是规范 UUID，参考编号不能为 nil。
    pub fn validate(&self) -> bool {
        match &self.selector {
            PlayerSupportSelector::User(value) => {
                !value.is_empty()
                    && value == value.trim()
                    && value.chars().count() <= MAX_PLAYER_SUPPORT_USER_LEN
                    && !value.chars().any(char::is_control)
                    && (!value.contains('-')
                        || Uuid::parse_str(value).is_ok_and(|parsed| {
                            !parsed.is_nil() && parsed.hyphenated().to_string() == *value
                        }))
            }
            PlayerSupportSelector::Reference(reference) => !reference.is_nil(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportUser {
    pub id: Uuid,
    pub username: String,
    pub status: String,
    /// 游戏内身份；MoogleMail 收件人与结算义务按它关联。
    pub game_player_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportBid {
    pub bid_id: Uuid,
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub world_id: String,
    pub amount: i64,
    pub mechanism: String,
    pub lot_status: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportDeal {
    pub lot_id: Uuid,
    pub auction_id: Uuid,
    pub lot_number: i64,
    pub world_id: String,
    pub equipment_name: String,
    pub final_price: i64,
    pub effective_end_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportObligation {
    pub obligation_id: Uuid,
    pub auction_id: Uuid,
    pub lot_id: Uuid,
    pub world_id: String,
    pub obligation_kind: String,
    pub business_state: String,
    pub amount: i64,
    pub fulfillment_state: Option<String>,
    pub fulfillment_error_code: Option<String>,
    pub delivery_id: Option<Uuid>,
}

/// 通知分区统一投影；`channel` 与 `state` 是闭集字符串，`state_reason` 只含已校验的失败或退役原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportNotification {
    pub notification_id: Uuid,
    /// `mm`（MoogleMail 投递）、`pm`（论坛私信）或 `pending`（待定提醒）。
    pub channel: String,
    /// 冻结模板键或通知身份名。
    pub kind: Option<String>,
    pub lot_id: Option<Uuid>,
    pub auction_id: Option<Uuid>,
    pub state: String,
    pub state_reason: Option<String>,
    pub delivery_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 命中的参考编号实体类型；让操作员确认这个编号究竟指向哪类记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerSupportReference {
    pub kind: String,
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayerSupportBundle {
    pub user: Option<PlayerSupportUser>,
    pub bids: Vec<PlayerSupportBid>,
    pub deals: Vec<PlayerSupportDeal>,
    pub obligations: Vec<PlayerSupportObligation>,
    pub deliveries: Vec<DeliveryAdminSnapshot>,
    pub notifications: Vec<PlayerSupportNotification>,
    pub references: Vec<PlayerSupportReference>,
    /// 任一分区达到行数上限时置真；调用方不得把它当成完整历史。
    pub truncated: bool,
}
