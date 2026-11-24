//! 收件、隔离、代管与季界的领域错误。
//!
//! 错误只携带安全分类，不回显邮件正文、装备 key、预约码或原始路由输入。应用层把这里的
//! 语义分类映射为 D9 §6.2 的固定拒绝码。

use thiserror::Error;

use crate::delivery::OriginalMailKeyError;

/// 寄售收件与代管领域拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DepositError {
    /// 预约码前缀不是 `C1-`。
    #[error("预约码版本或前缀无效")]
    TokenPrefix,
    /// 预约码正文长度不是 43 个字符。
    #[error("预约码长度无效")]
    TokenLength,
    /// 预约码含 URL 安全 base64 字母表以外的字符；`=` 也在此拒绝。
    #[error("预约码字符或填充无效")]
    TokenAlphabet,
    /// 43 个字符的最后两个填充位非零，不是唯一规范编码。
    #[error("预约码不是规范编码")]
    TokenNonCanonical,
    /// 正文为空或只有结尾换行。
    #[error("寄件正文为空")]
    BodyEmpty,
    /// 正文超过协议或固定字节上限。
    #[error("寄件正文超过字节上限")]
    BodyTooLarge,
    /// 正文行数超过协议或固定件数上限。
    #[error("寄件正文超过件数上限")]
    TooManyItems,
    /// 正文含空行，包括两个及以上结尾换行。
    #[error("寄件正文含空行")]
    BodyBlankLine,
    /// 正文含不构成 CRLF 的孤立回车。
    #[error("寄件正文含孤立回车")]
    BodyIsolatedCr,
    /// 同一预约码在正文中重复。
    #[error("寄件正文重复引用同一预约码")]
    BodyDuplicate,
    /// 正文含未知版本或自由文本。
    #[error("寄件正文含未知版本或自由文本")]
    BodyFreeText,
    /// 请求版本与锁内事实不一致。
    #[error("寄售版本已变化")]
    StaleVersion {
        /// 命令预期版本。
        expected: u64,
        /// 锁内当前版本。
        actual: u64,
    },
    /// 挂单当前状态不允许该动作。
    #[error("寄售挂单当前不接受该动作")]
    ListingNotActive,
    /// 卖家不具备寄售资格。
    #[error("卖家不具备寄售资格")]
    SellerIneligible,
    /// 认证 Bot 不能作为寄售交易当事人。
    #[error("认证 Bot 不能作为寄售交易当事人")]
    BotParty,
    /// 旧季已关闭，不能建立新授权。
    #[error("寄售季已关闭")]
    SeasonClosed,
    /// 季界无法判定，失败关闭。
    #[error("寄售季界无法判定")]
    SeasonUnknown,
    /// 邮箱或证明能力当前不可用。
    #[error("寄售收件能力暂不可用")]
    MailUnavailable,
    /// 装备已被其他责任占用。
    #[error("装备已被占用或仍有未结清责任")]
    EquipmentBusy,
    /// 来源与本操作不匹配。
    #[error("来源与本操作不匹配")]
    SourceMismatch,
    /// 缺少完成该动作所需的证明。
    #[error("缺少完成该动作所需的证明")]
    EvidenceUnproven,
    /// 两份可靠证据互相矛盾。
    #[error("证据互相矛盾")]
    EvidenceConflict,
    /// 已存在相反或不同的最终处置。
    #[error("已存在相反的最终处置")]
    DispositionConflict,
    /// 指定的运营止售与当前有效止售不一致。
    #[error("运营止售不匹配")]
    HoldMismatch,
    /// 当前状态或角色不允许该动作。
    #[error("当前状态或角色不允许该动作")]
    ActionNotAllowed,
    /// 起写 fence 与锁内当前 fence 不一致。
    #[error("投递 fence 不匹配")]
    FenceMismatch,
    /// 第 4 期没有真实寄售合同或原轮，不能产生 `acquire_return` 成功。
    #[error("缺少真实寄售合同或原轮")]
    AcquireReturnUnavailable,
    /// 整封取得证明形状无效。
    #[error("整封取得证明无效")]
    InvalidProof,
    /// position 当前状态不能取得处分权。
    #[error("position 当前状态不能取得处分权")]
    InvalidPositionState,
    /// 季窗输入无法产生严格有序的 UTC 边界。
    #[error("寄售季窗无效")]
    InvalidSeasonWindow,
    /// main 世界没有寄售季窗。
    #[error("main 世界没有寄售季窗")]
    NotSeasonal,
    /// 日历输入缺失或无法解析。
    #[error("寄售日历输入缺失或无效")]
    InvalidCalendar,
    /// 预约已过期或尚未生效。
    #[error("预约已过期或尚未生效")]
    ReservationExpired,
    /// 预约已被撤销或消费。
    #[error("预约已不再有效")]
    ReservationNotActive,
    /// 预约与邮件中的装备、卖家、世界或版本不匹配。
    #[error("预约与邮件清单不匹配")]
    ReservationMismatch,
    /// 邮件件数、规范 EID 数与预约数不一致。
    #[error("寄件件数与预约数不一致")]
    ReservationCountMismatch,
    /// 请求形状无效。
    #[error("寄售请求无效")]
    InvalidRequest,
}

impl From<OriginalMailKeyError> for DepositError {
    fn from(_error: OriginalMailKeyError) -> Self {
        Self::InvalidRequest
    }
}
