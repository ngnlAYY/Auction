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
pub const DEFAULT_FINAL_PHASE_LEAD: Duration = Duration::minutes(1);
pub const SMALL_BID_INCREMENT: i64 = 30_000;
pub const REGULAR_BID_INCREMENT: i64 = 50_000;
pub const LARGE_BID_THRESHOLD: i64 = 2_000_000;
pub const SMALL_BID_THRESHOLD: i64 = 200_000;
pub const PROXY_LAST_INCREMENT: i64 = 1_000;
pub const DEFAULT_SOFT_CLOSE_WINDOW: Duration = Duration::minutes(1);
pub const DEFAULT_SOFT_CLOSE_EXTENSION: Duration = Duration::minutes(1);
pub const DEFAULT_CANCEL_CUTOFF: Duration = Duration::hours(24);
pub const DEFAULT_PROXY_SLOW_INTERVAL: Duration = Duration::minutes(10);

/// 一场拍卖的不可变经济和时间规则快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuctionRules {
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
        if self.default_start_price.value() <= 0
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

        if !matches!(self.proxy_default_mode, ProxyMode::Fast | ProxyMode::Slow) {
            return Err(DomainError::InvalidRules("代理默认模式无效"));
        }

        Ok(())
    }

    /// 返回 lot 快照的起始价；没有 lot 覆盖值时使用规则默认值。
    pub fn starting_price(self, lot_start_price: Option<BidAmount>) -> BidAmount {
        lot_start_price.unwrap_or(self.default_start_price)
    }

    /// 计算下一次允许的最低出价；固定增量和 PPM 网格增量都保留旧价格余数。
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

        if proxy_last {
            return current_price.checked_add(PROXY_LAST_INCREMENT);
        }
        if self.bid_increment_ppm > 0 {
            return ppm_minimum(current_price, self.bid_increment_ppm);
        }
        current_price.checked_add(self.minimum_increment)
    }

    /// 校验一笔出价并计算软截止后的有效结束时间。
    pub fn validate_bid(
        self,
        state: LotBidState,
        bidder_id: UserId,
        amount: BidAmount,
        now: DateTime<Utc>,
    ) -> Result<BidDecision, DomainError> {
        self.validate()?;
        state.validate()?;
        if bidder_id.is_nil() {
            return Err(DomainError::InvalidBidState("出价人标识不能是 nil UUID"));
        }
        if now.timestamp_nanos_opt().is_none() {
            return Err(DomainError::InvalidBidState("当前时间超出可表示范围"));
        }
        if now > state.effective_end_at {
            return Err(DomainError::AuctionClosed);
        }

        let proxy_last = state.leader_proxy_last && state.leader_id == Some(bidder_id);
        let minimum_price = self.next_minimum_bid(
            Some(state.start_price),
            state.current_price,
            state.material,
            proxy_last,
        )?;
        if amount < minimum_price {
            return Err(DomainError::BidTooLow { minimum: minimum_price });
        }

        let mut effective_end_at = state.effective_end_at;
        let mut extended = false;
        let soft_close_start = state
            .effective_end_at
            .checked_sub_signed(self.soft_close_window)
            .ok_or(DomainError::InvalidSchedule("软截止窗口超出时间范围"))?;
        if self.soft_close_window > Duration::zero() && now >= soft_close_start {
            let candidate = now
                .checked_add_signed(self.soft_close_extension)
                .ok_or(DomainError::InvalidSchedule("软截止延长超出时间范围"))?;
            if candidate > effective_end_at {
                effective_end_at = candidate;
                extended = true;
            }
        }

        Ok(BidDecision { minimum_price, accepted_price: amount, effective_end_at, extended })
    }

    /// 判断当前时刻是否仍在取消截止点之前；恰好到达截止点时不可取消。
    pub fn can_cancel(
        self,
        scheduled_end_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<bool, DomainError> {
        self.validate()?;
        let cutoff_at = scheduled_end_at
            .checked_sub_signed(self.cancel_cutoff)
            .ok_or(DomainError::InvalidSchedule("取消截止超出时间范围"))?;
        Ok(now < cutoff_at)
    }
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

/// 出价规则计算结果，供应用层在同一事务内写入事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BidDecision {
    pub minimum_price: BidAmount,
    pub accepted_price: BidAmount,
    pub effective_end_at: DateTime<Utc>,
    pub extended: bool,
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
