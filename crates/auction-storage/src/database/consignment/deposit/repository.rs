//! 4B 存储端口组合：同一顶层事务、同一预约密码边界。

use std::sync::Arc;

use auction_application::consignment::ReservationSecretCrypto;
use auction_domain::consignment::{FeeBps, ListingPrice};

use crate::database::DatabaseStore;

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

/// 寄售收件与代管的 PostgreSQL 端口实现。
///
/// 预约令牌的生成在存储侧完成，封装/解封由 4D 注入的
/// [`ReservationSecretCrypto`] 负责；没有密码实现时预约签发与本人指引失败关闭。
#[derive(Clone)]
pub struct DepositRepository {
    store: DatabaseStore,
    crypto: Arc<dyn ReservationSecretCrypto>,
    settings: DepositSettings,
}

impl DepositRepository {
    /// 组合共享存储、预约令牌密码适配器与启动冻结配置。
    pub fn new(
        store: DatabaseStore,
        crypto: Arc<dyn ReservationSecretCrypto>,
        settings: DepositSettings,
    ) -> Self {
        Self { store, crypto, settings }
    }

    /// 借用底层共享存储；4C 的寄售端口仍在同一 store 上实现。
    pub fn store(&self) -> &DatabaseStore {
        &self.store
    }

    pub(crate) fn crypto(&self) -> &dyn ReservationSecretCrypto {
        self.crypto.as_ref()
    }

    /// 启动冻结的费率、价格边界与额度。
    pub(crate) const fn settings(&self) -> DepositSettings {
        self.settings
    }
}
