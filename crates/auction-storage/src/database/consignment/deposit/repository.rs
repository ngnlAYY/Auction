//! 4B 存储端口组合：同一顶层事务、同一预约密码边界。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentError, ReservationSecretAad, ReservationSecretCrypto, ReservationSecretCryptoError,
    SealedReservationSecret,
};
use auction_domain::consignment::{FeeBps, ListingPrice, TokenText};

use crate::database::DatabaseStore;
use crate::database::consignment::purchase::{
    FirstDispatchContent, FirstDispatchContentSource, FirstDispatchRequest,
    WithdrawalDispatchRequest,
};

/// 寄售收件与代管的启动冻结配置。
///
/// 费率、价格边界与跨世界活动额度由装配根从同一份配置注入；存储不得回退到硬编码默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositSettings {
    /// 寄售成交费率。
    pub fee_bps: FeeBps,
    /// 允许登记的最低标价。
    pub min_price: ListingPrice,
    /// 允许登记的最高标价。
    pub max_price: ListingPrice,
    /// 新寄件预约的最长有效期小时数。
    pub reservation_ttl_hours: i64,
    /// 每个卖家跨世界允许的活动挂单数。
    pub max_open_per_user: i64,
}

impl DepositSettings {
    /// 组合启动时校验过的配置值。
    pub const fn new(
        fee_bps: FeeBps,
        min_price: ListingPrice,
        max_price: ListingPrice,
        reservation_ttl_hours: i64,
        max_open_per_user: i64,
    ) -> Self {
        Self { fee_bps, min_price, max_price, reservation_ttl_hours, max_open_per_user }
    }

    /// 用领域定义的标价上下界构造；价格区间没有独立配置。
    pub fn with_standard_price_bounds(
        fee_bps: FeeBps,
        reservation_ttl_hours: i64,
        max_open_per_user: i64,
    ) -> Result<Self, auction_domain::consignment::ListingError> {
        Ok(Self::new(
            fee_bps,
            ListingPrice::new(ListingPrice::MIN)?,
            ListingPrice::new(ListingPrice::MAX)?,
            reservation_ttl_hours,
            max_open_per_user,
        ))
    }
}

/// 无密钥时的密码占位：封装与解封一律失败，调用方按固定不可用分类处理。
struct UnavailableReservationCrypto;

impl ReservationSecretCrypto for UnavailableReservationCrypto {
    fn seal(
        &self,
        _token: &TokenText,
        _aad: ReservationSecretAad,
    ) -> Result<SealedReservationSecret, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }

    fn open(
        &self,
        _sealed: &SealedReservationSecret,
        _aad: ReservationSecretAad,
    ) -> Result<TokenText, ReservationSecretCryptoError> {
        Err(ReservationSecretCryptoError)
    }
}

/// 未注入派发内容来源时的占位：一律 `Unavailable`。
///
/// 不变量 4、6：没有冻结内容就不能建撤回首轮，撤回（含取得事务里兑现的意愿）整笔回滚、不写任何
/// 派发事实，调用方以同命令 ID 重试；不在存储里自造文本。
struct UnavailableDispatchContent;

impl FirstDispatchContentSource for UnavailableDispatchContent {
    fn content(
        &self,
        _request: &FirstDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Err(ConsignmentError::Unavailable)
    }

    fn withdrawal_content(
        &self,
        _request: &WithdrawalDispatchRequest<'_>,
    ) -> Result<FirstDispatchContent, ConsignmentError> {
        Err(ConsignmentError::Unavailable)
    }
}

/// 寄售收件与代管的 PostgreSQL 端口实现。
///
/// 预约令牌的生成在存储侧完成，封装/解封由 4D 注入的
/// [`ReservationSecretCrypto`] 负责；没有密码实现时预约签发与本人指引失败关闭。
#[derive(Clone)]
pub struct DepositRepository {
    store: DatabaseStore,
    crypto: Arc<dyn ReservationSecretCrypto>,
    settings: DepositSettings,
    dispatch_content: Arc<dyn FirstDispatchContentSource>,
}

impl DepositRepository {
    /// 组合共享存储、预约令牌密码适配器与启动冻结配置。
    pub fn new(
        store: DatabaseStore,
        crypto: Arc<dyn ReservationSecretCrypto>,
        settings: DepositSettings,
    ) -> Self {
        Self { store, crypto, settings, dispatch_content: Arc::new(UnavailableDispatchContent) }
    }

    /// 无预约码密码实现的装配：预约签发与本人指引失败关闭。
    ///
    /// 寄售停用时进程不要求凭据域密钥，但数据库可用时 v2 读取与撤回、解除止售仍须照常
    /// （D9 §6.3、§7）；这些路径不涉及预约码封装，新承诺由处理器按有效开关拒绝。
    pub fn without_reservation_crypto(store: DatabaseStore, settings: DepositSettings) -> Self {
        Self {
            store,
            crypto: Arc::new(UnavailableReservationCrypto),
            settings,
            dispatch_content: Arc::new(UnavailableDispatchContent),
        }
    }

    /// 注入撤回件首轮的冻结内容来源（与购买首轮同一渲染实现）。未注入时撤回整笔失败关闭。
    #[must_use]
    pub fn with_dispatch_content(mut self, content: Arc<dyn FirstDispatchContentSource>) -> Self {
        self.dispatch_content = content;
        self
    }

    /// 借用底层共享存储；4C 的寄售端口仍在同一 store 上实现。
    pub fn store(&self) -> &DatabaseStore {
        &self.store
    }

    pub(crate) fn crypto(&self) -> &dyn ReservationSecretCrypto {
        self.crypto.as_ref()
    }

    /// 撤回件首轮的冻结内容来源。
    pub(crate) fn dispatch_content(&self) -> &Arc<dyn FirstDispatchContentSource> {
        &self.dispatch_content
    }

    /// 启动冻结的费率、价格边界与额度。
    pub(crate) const fn settings(&self) -> DepositSettings {
        self.settings
    }
}
