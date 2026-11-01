use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    DomainError, ProxyAuthorizationId, ProxyMode, UserId,
    bidding::amount::{BidAmount, CREDIT_GRID},
};

/// 旧论坛竞价规则中的默认金额和时间边界。
pub const DEFAULT_START_PRICE: i64 = 1_000;
pub const DEFAULT_BID_INCREMENT_PPM: i64 = 50_000;
pub const DEFAULT_PROXY_MAX_OUTBID_USES: u32 = 3;
pub const DEFAULT_FINAL_PHASE_LEAD: Duration = Duration::minutes(15);
pub const PROXY_LAST_INCREMENT: i64 = 1_000;
/// 新场次固定最后15分钟触发；既有场次仍使用自己的冻结快照。
pub const DEFAULT_SOFT_CLOSE_WINDOW: Duration = Duration::minutes(15);
/// 候选截止为接受时刻加15分钟，只有晚于当前截止时才延长。
pub const DEFAULT_SOFT_CLOSE_EXTENSION: Duration = Duration::minutes(15);
pub const DEFAULT_CANCEL_CUTOFF: Duration = Duration::hours(24);
pub const DEFAULT_PROXY_SLOW_INTERVAL: Duration = Duration::minutes(10);

/// 一场拍卖的不可变经济和时间规则快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionRules {
    /// v1 保留既有代理增量；v2 统一最低价，自动代理可用尽上限但必须严格反超。
    #[serde(default = "legacy_bidding_version")]
    pub bidding_version: u32,
    pub default_start_price: BidAmount,
    pub bid_increment_ppm: i64,
    pub minimum_increment: i64,
    pub soft_close_window: Duration,
    pub soft_close_extension: Duration,
    pub cancel_cutoff: Duration,
    pub proxy_slow_interval: Duration,
    pub proxy_enabled: bool,
    pub proxy_default_mode: ProxyMode,
    pub proxy_max_outbid_uses: u32,
    pub final_phase_lead: Duration,
}

impl Default for AuctionRules {
    fn default() -> Self {
        Self {
            bidding_version: 2,
            default_start_price: BidAmount::from_validated(DEFAULT_START_PRICE),
            bid_increment_ppm: DEFAULT_BID_INCREMENT_PPM,
            minimum_increment: 0,
            soft_close_window: DEFAULT_SOFT_CLOSE_WINDOW,
            soft_close_extension: DEFAULT_SOFT_CLOSE_EXTENSION,
            cancel_cutoff: DEFAULT_CANCEL_CUTOFF,
            proxy_slow_interval: DEFAULT_PROXY_SLOW_INTERVAL,
            proxy_enabled: true,
            proxy_default_mode: ProxyMode::Fast,
            proxy_max_outbid_uses: DEFAULT_PROXY_MAX_OUTBID_USES,
            final_phase_lead: DEFAULT_FINAL_PHASE_LEAD,
        }
    }
}

impl AuctionRules {
    /// 校验规则快照，防止无效配置进入竞价计算。
    pub fn validate(self) -> Result<(), DomainError> {
        if !matches!(self.bidding_version, 1 | 2)
            || self.default_start_price.value() <= 0
            || !self.default_start_price.is_credit_grid_aligned()
            || !(0..=1_000_000).contains(&self.bid_increment_ppm)
            || self.minimum_increment < 0
            || self.minimum_increment % CREDIT_GRID != 0
            || (self.bid_increment_ppm == 0 && self.minimum_increment <= 0)
            || self.soft_close_window < Duration::zero()
            || self.soft_close_extension <= Duration::zero()
            || self.cancel_cutoff < Duration::zero()
            || self.proxy_slow_interval <= Duration::zero()
            || self.proxy_max_outbid_uses > 1_000
            || self.final_phase_lead <= Duration::zero()
        {
            return Err(DomainError::InvalidRules("金额或时间边界无效"));
        }

        Ok(())
    }

    /// 返回 lot 快照的起始价；没有 lot 覆盖值时使用规则默认值。
    pub fn starting_price(self, lot_start_price: Option<BidAmount>) -> BidAmount {
        lot_start_price.unwrap_or(self.default_start_price)
    }

    /// 计算下一次最低出价；只有冻结的 v1 策略保留自动代理的固定增量。
    pub fn next_minimum_bid(
        self,
        lot_start_price: Option<BidAmount>,
        current_price: Option<BidAmount>,
        _material: bool,
        proxy_last: bool,
    ) -> Result<BidAmount, DomainError> {
        self.validate()?;
        let Some(current_price) = current_price else {
            return Ok(self.starting_price(lot_start_price));
        };

        if self.bidding_version == 1 && proxy_last {
            return current_price.checked_add(PROXY_LAST_INCREMENT);
        }
        if self.bid_increment_ppm > 0 {
            return ppm_minimum(current_price, self.bid_increment_ppm);
        }
        current_price.checked_add(self.minimum_increment)
    }
}

fn legacy_bidding_version() -> u32 {
    1
}

/// 锁内读取到的最小竞价状态，不包含数据库或 HTTP 细节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LotBidState {
    pub lot_id: crate::LotId,
    #[serde(deserialize_with = "crate::bidding::amount::deserialize_persisted_positive")]
    pub start_price: BidAmount,
    #[serde(
        default,
        deserialize_with = "crate::bidding::amount::deserialize_optional_persisted_positive"
    )]
    pub current_price: Option<BidAmount>,
    pub leader_id: Option<UserId>,
    pub leader_proxy_last: bool,
    pub material: bool,
    pub effective_end_at: DateTime<Utc>,
    pub state_version: u64,
    pub last_proxy_sequence: u64,
    pub slow_generation: u64,
    pub slow_authorization_id: Option<ProxyAuthorizationId>,
    pub slow_due_at: Option<DateTime<Utc>>,
}

impl LotBidState {
    /// 校验从数据库锁内读取的竞价状态。
    pub fn validate(self) -> Result<(), DomainError> {
        if self.lot_id.is_nil() || self.state_version == 0 {
            return Err(DomainError::InvalidBidState("竞价状态缺少 lot 标识或状态版本"));
        }
        if self.effective_end_at.timestamp_nanos_opt().is_none() {
            return Err(DomainError::InvalidBidState("有效结束时间超出可表示范围"));
        }
        if self.current_price.is_some_and(|price| price < self.start_price) {
            return Err(DomainError::InvalidBidState("当前价低于起始价"));
        }
        if self.current_price.is_some() != self.leader_id.is_some()
            || self.leader_id.is_some_and(UserId::is_nil)
        {
            return Err(DomainError::InvalidBidState("当前价与领先者标识不一致"));
        }
        if self.leader_proxy_last && self.leader_id.is_none_or(|leader_id| leader_id.is_nil()) {
            return Err(DomainError::InvalidBidState("代理领先标记缺少有效领先者"));
        }
        if self.slow_generation == 0
            && (self.slow_authorization_id.is_some() || self.slow_due_at.is_some())
        {
            return Err(DomainError::InvalidBidState("慢速代理调度缺少有效世代"));
        }
        if self.slow_authorization_id.is_some() != self.slow_due_at.is_some() {
            return Err(DomainError::InvalidBidState("慢速代理调度字段不完整"));
        }
        if self.slow_authorization_id.is_some_and(ProxyAuthorizationId::is_nil) {
            return Err(DomainError::InvalidBidState("慢速代理授权标识不能是 nil UUID"));
        }
        if self.slow_due_at.is_some_and(|due_at| due_at.timestamp_nanos_opt().is_none()) {
            return Err(DomainError::InvalidBidState("慢速代理到期时间无效"));
        }

        Ok(())
    }
}

fn ppm_minimum(current_price: BidAmount, bid_increment_ppm: i64) -> Result<BidAmount, DomainError> {
    let current = i128::from(current_price.value());
    let ppm = i128::from(bid_increment_ppm);
    let grid = i128::from(CREDIT_GRID);
    // Auction-BF 的事务策略先把当前价向上折算为信用网格单位，再对增量单位执行 PPM
    // 向上取整，最后把整千增量加回原始 current；因此 1111 的 5% 下一口是 2111。
    let current_units = (current + grid - 1) / grid;
    let increment_units = ((current_units * ppm + 999_999) / 1_000_000).max(1);
    let candidate = current + increment_units * grid;
    if candidate <= current || candidate > i128::from(i64::MAX) {
        return Err(DomainError::AmountOverflow);
    }
    let value = i64::try_from(candidate).map_err(|_| DomainError::AmountOverflow)?;
    Ok(BidAmount::from_validated(value))
}
