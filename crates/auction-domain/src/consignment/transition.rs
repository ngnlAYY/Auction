//! 定义挂单状态迁移、期限与强制终止原因的优先顺序。

use auction_time::{Duration, Timestamp};

use crate::consignment::errors::{ListingError, ListingReason};
use crate::consignment::listing::{
    DEFAULT_DEPOSIT_TTL_DAYS, DEFAULT_DISPLAY_TTL_DAYS, LOOKUP_TTL, ListingMode, ListingPrice,
    ListingSnapshot, ListingState, ListingVersion,
};

/// 核验通过时使用的两种可配置期限，取值范围见合同第 6 节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListingPublicationTtls {
    display: Duration,
    deposit: Duration,
}

impl ListingPublicationTtls {
    /// 校验配置的展示天数和待寄天数。
    pub fn new(display_ttl_days: i64, deposit_ttl_days: i64) -> Result<Self, ListingError> {
        if !(1..=90).contains(&display_ttl_days) || !(1..=30).contains(&deposit_ttl_days) {
            return Err(ListingError::InvalidTransition {
                from: "resolving",
                action: "configure_publication_ttl",
            });
        }
        Ok(Self {
            display: Duration::days(display_ttl_days),
            deposit: Duration::days(deposit_ttl_days),
        })
    }
}

impl Default for ListingPublicationTtls {
    fn default() -> Self {
        Self {
            display: Duration::days(DEFAULT_DISPLAY_TTL_DAYS),
            deposit: Duration::days(DEFAULT_DEPOSIT_TTL_DAYS),
        }
    }
}

/// 同一事务快照内经核验的强制终止事实，调用方须传入这一快照上的全部适用事实。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ForcedTerminationFacts {
    /// 发现登记基线以外的已提交拍卖 lot，实际关联由调用方保存。
    pub auction_lot_created: bool,
    /// 卖家已失去资格，或存在登记后新增的拉黑事实。
    pub seller_ineligible: bool,
    /// 服务已停用，或挂单属于停用前的旧代次。
    pub service_disabled: bool,
}

/// 调用方经鉴权、命令重放仲裁以及适用的证据或租约校验后请求执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingTrigger {
    /// 创建一条已通过准入的全新登记，当前快照须为空。
    Register {
        /// 卖家所选的不可变模式。
        mode: ListingMode,
        /// 卖家登记的整数标价。
        price: ListingPrice,
    },
    /// Worker 在活租约内取得可发布的观察，使用锁内最新的标价和版本。
    LookupAccepted(ListingPublicationTtls),
    /// Worker 在活租约内得到合规观察拒绝或最终解析失败的安全分类。
    LookupRejected(ListingReason),
    /// 系统根据已核实的强制终止事实或当前态截止执行清退。
    AutoClose,
    /// 本人修改标价。
    Reprice {
        /// 命令所见的原版本。
        expected_version: ListingVersion,
        /// 新标价。
        price: ListingPrice,
    },
    /// 本人取消挂单。
    Withdraw {
        /// 命令所见的原版本。
        expected_version: ListingVersion,
    },
    /// 管理员下架，调用方负责校验并保存私有运营说明。
    AdminDelist {
        /// 命令所见的原版本。
        expected_version: ListingVersion,
    },
}

impl ListingTrigger {
    fn action(self) -> &'static str {
        match self {
            Self::Register { .. } => "register",
            Self::LookupAccepted(_) | Self::LookupRejected(_) => "apply_lookup",
            Self::AutoClose => "auto_close",
            Self::Reprice { .. } => "reprice",
            Self::Withdraw { .. } => "withdraw",
            Self::AdminDelist { .. } => "admin_delist",
        }
    }

    fn expected_version(self) -> Option<ListingVersion> {
        match self {
            Self::Reprice { expected_version, .. }
            | Self::Withdraw { expected_version }
            | Self::AdminDelist { expected_version } => Some(expected_version),
            _ => None,
        }
    }
}

/// 应随挂单写入的内部事件种类，终态原因从更新后的快照读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingChange {
    /// 以版本 1 创建登记。
    Registered,
    /// 冻结新状态的期限，写入当前安全快照。
    Published,
    /// 按拒绝分类决定是否保存当前观察的安全快照。
    Rejected {
        /// 合规观察在拒挂后仍对本人和管理员可见。
        keep_snapshot: bool,
    },
    /// 到达当前状态截止，保留原快照。
    Expired,
    /// 挂单关闭，保留原快照。
    Closed,
    /// 标价变化，状态与期限不变。
    Repriced,
}

impl ListingChange {
    /// 返回合同第 4.1 节的内部事件种类代码。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Published => "published",
            Self::Rejected { .. } => "rejected",
            Self::Expired => "expired",
            Self::Closed => "closed",
            Self::Repriced => "repriced",
        }
    }
}

/// 单次迁移的结果，区分应提交的事实与原命令的回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingTransition {
    /// 首次登记使用版本 1，其他已接受的事实变化增加一个版本。
    Changed {
        /// 提交后的持久快照。
        listing: ListingSnapshot,
        /// 应追加的事件种类。
        change: ListingChange,
    },
    /// 同价命令成功，回执为 `unchanged`，无需更新投影或追加版本事件。
    Unchanged(ListingSnapshot),
    /// 原动作被强制终止抢先，须提交终止事件和原命令的 `listing_not_active` 拒绝回执。
    ForcedTermination {
        /// 应提交的终态快照，版本已增加一个。
        listing: ListingSnapshot,
        /// 应追加的系统事件种类。
        change: ListingChange,
    },
}

/// 根据锁内快照、数据库时刻与核验事实决定迁移，不执行读写或命令重放。
pub fn transition_listing(
    current: Option<ListingSnapshot>,
    trigger: ListingTrigger,
    now: Timestamp,
    facts: ForcedTerminationFacts,
) -> Result<ListingTransition, ListingError> {
    // 现有错误闭集以固定动作码区分坏快照、时刻越界和版本耗尽，避免误当作已终结。
    let reject = |action| ListingError::InvalidTransition {
        from: current.map_or("absent", |listing| listing.state.as_str()),
        action,
    };
    let invalid = || reject(trigger.action());

    let Some(mut listing) = current else {
        let ListingTrigger::Register { mode, price } = trigger else {
            return Err(invalid());
        };
        if facts != ForcedTerminationFacts::default() {
            return Err(invalid());
        }
        // 不变量 1、2、3：调用方先完成仲裁和准入，再把 v1 与首条事件同事务持久化。
        return Ok(ListingTransition::Changed {
            listing: ListingSnapshot {
                mode,
                state: ListingState::Resolving,
                state_version: ListingVersion::FIRST,
                price,
                expires_at: now
                    .checked_add_signed(LOOKUP_TTL)
                    .ok_or_else(|| reject("set_deadline"))?,
                reason: None,
            },
            change: ListingChange::Registered,
        });
    };

    if !valid_snapshot(listing) {
        return Err(reject("decode_snapshot"));
    }
    if matches!(trigger, ListingTrigger::Register { .. }) {
        return Err(invalid());
    }
    // 不变量 6：旧 v1 变更不对第 4 期代管状态执行新语义；这些迁移由寄售 deposit 领域负责。
    if listing.state.is_custody_managed() {
        return Err(invalid());
    }

    // 不变量 2、3、6：锁内强制事实抢先于主动操作的版本检查，已终结记录保留初次原因。
    if let Some(reason) = forced_reason(listing, now, facts) {
        let change = if reason.terminal_state() == "expired" {
            listing.state = ListingState::Expired;
            ListingChange::Expired
        } else {
            listing.state = ListingState::Closed;
            ListingChange::Closed
        };
        listing.reason = Some(reason);
        listing.state_version =
            listing.state_version.next().ok_or_else(|| reject("advance_version"))?;
        return Ok(if trigger == ListingTrigger::AutoClose {
            ListingTransition::Changed { listing, change }
        } else {
            ListingTransition::ForcedTermination { listing, change }
        });
    }

    if let Some(expected_version) = trigger.expected_version()
        && expected_version != listing.state_version
    {
        return Err(ListingError::StaleVersion {
            expected: expected_version.value(),
            actual: listing.state_version.value(),
        });
    }
    if !listing.state.is_active() {
        return Err(invalid());
    }

    let change = match trigger {
        ListingTrigger::LookupAccepted(ttls) if listing.state == ListingState::Resolving => {
            let (state, ttl) = match listing.mode {
                ListingMode::Direct => (ListingState::Displayed, ttls.display),
                ListingMode::Consigned => (ListingState::AwaitingDeposit, ttls.deposit),
            };
            listing.state = state;
            listing.expires_at =
                now.checked_add_signed(ttl).ok_or_else(|| reject("set_deadline"))?;
            ListingChange::Published
        }
        ListingTrigger::LookupRejected(reason)
            if listing.state == ListingState::Resolving
                && reason.terminal_state() == "rejected" =>
        {
            listing.state = ListingState::Rejected;
            listing.reason = Some(reason);
            let keep_snapshot = matches!(
                reason,
                ListingReason::NotTradeable
                    | ListingReason::OwnerMismatch
                    | ListingReason::HistoryOwnerMismatch
                    | ListingReason::HistoryAliasConflict
                    | ListingReason::HistoryUnproven
                    | ListingReason::EquipmentBusy
            );
            ListingChange::Rejected { keep_snapshot }
        }
        ListingTrigger::Reprice { price, .. } if price == listing.price => {
            return Ok(ListingTransition::Unchanged(listing));
        }
        ListingTrigger::Reprice { price, .. } => {
            listing.price = price;
            ListingChange::Repriced
        }
        ListingTrigger::Withdraw { .. } | ListingTrigger::AdminDelist { .. } => {
            listing.state = ListingState::Closed;
            listing.reason = Some(if matches!(trigger, ListingTrigger::Withdraw { .. }) {
                ListingReason::SellerCancelled
            } else {
                ListingReason::AdminDelisted
            });
            ListingChange::Closed
        }
        _ => return Err(invalid()),
    };

    // 不变量 3、6、8：每个新事实只推进一版；首次终态及其原因由调用方同事件一起提交。
    listing.state_version =
        listing.state_version.next().ok_or_else(|| reject("advance_version"))?;
    Ok(ListingTransition::Changed { listing, change })
}

fn valid_snapshot(listing: ListingSnapshot) -> bool {
    let valid_mode = match listing.state {
        ListingState::Displayed => listing.mode == ListingMode::Direct,
        ListingState::AwaitingDeposit
        | ListingState::DepositAccepted
        | ListingState::Available
        | ListingState::SalePending
        | ListingState::HeldOffMarket
        | ListingState::WithdrawalPending => listing.mode == ListingMode::Consigned,
        _ => true,
    };
    valid_mode
        && match listing.reason {
            Some(reason) => reason.terminal_state() == listing.state.as_str(),
            None => listing.state.is_active(),
        }
}

fn forced_reason(
    listing: ListingSnapshot,
    now: Timestamp,
    facts: ForcedTerminationFacts,
) -> Option<ListingReason> {
    // 不变量 6、8：已经提交的终态优先；同一活动快照按合同第 3.2 节裁定。
    if !listing.state.is_active() {
        return None;
    }
    // 代管状态没有旧 TTL；止售、季末与损失由第 4–7 期的专用迁移收口。
    if listing.state.is_custody_managed() {
        return None;
    }
    if facts.auction_lot_created {
        return Some(ListingReason::AuctionLotCreated);
    }
    if facts.seller_ineligible {
        return Some(ListingReason::SellerIneligible);
    }
    if facts.service_disabled {
        return Some(ListingReason::ServiceDisabled);
    }
    if now >= listing.expires_at {
        return match listing.state {
            ListingState::Resolving => Some(ListingReason::LookupDeadline),
            ListingState::Displayed => Some(ListingReason::DisplayExpired),
            ListingState::AwaitingDeposit => Some(ListingReason::DepositExpired),
            ListingState::DepositAccepted
            | ListingState::Available
            | ListingState::SalePending
            | ListingState::HeldOffMarket
            | ListingState::WithdrawalPending
            | ListingState::Rejected
            | ListingState::Expired
            | ListingState::Closed => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use auction_time::{Duration, Timestamp};

    use crate::consignment::errors::{ListingError, ListingReason};
    use crate::consignment::listing::{
        ListingMode, ListingPrice, ListingSnapshot, ListingState, ListingVersion,
    };
    use crate::consignment::transition::{
        ForcedTerminationFacts, ListingChange, ListingPublicationTtls, ListingTransition,
        ListingTrigger, transition_listing,
    };

    const NOW: Timestamp = Timestamp::UNIX_EPOCH;
    const MODES: [ListingMode; 2] = [ListingMode::Direct, ListingMode::Consigned];

    fn snapshot(mode: ListingMode, state: ListingState) -> Result<ListingSnapshot, ListingError> {
        Ok(ListingSnapshot {
            mode,
            state,
            state_version: ListingVersion::new(3)?,
            price: ListingPrice::new(101)?,
            expires_at: NOW + Duration::seconds(60),
            reason: None,
        })
    }

    fn active_listings() -> Result<[(ListingSnapshot, ListingReason); 4], ListingError> {
        Ok([
            (
                snapshot(ListingMode::Direct, ListingState::Resolving)?,
                ListingReason::LookupDeadline,
            ),
            (
                snapshot(ListingMode::Consigned, ListingState::Resolving)?,
                ListingReason::LookupDeadline,
            ),
            (
                snapshot(ListingMode::Direct, ListingState::Displayed)?,
                ListingReason::DisplayExpired,
            ),
            (
                snapshot(ListingMode::Consigned, ListingState::AwaitingDeposit)?,
                ListingReason::DepositExpired,
            ),
        ])
    }

    fn independent_close_facts() -> [(ForcedTerminationFacts, ListingReason); 2] {
        [
            (
                ForcedTerminationFacts { auction_lot_created: true, ..Default::default() },
                ListingReason::AuctionLotCreated,
            ),
            (
                ForcedTerminationFacts { seller_ineligible: true, ..Default::default() },
                ListingReason::SellerIneligible,
            ),
        ]
    }

    fn decide(
        current: Option<ListingSnapshot>,
        trigger: ListingTrigger,
        now: Timestamp,
    ) -> Result<ListingTransition, ListingError> {
        transition_listing(current, trigger, now, ForcedTerminationFacts::default())
    }

    #[test]
    fn register_starts_both_modes_at_version_one_with_fixed_deadline() -> Result<(), ListingError> {
        let price = ListingPrice::new(101)?;
        for mode in MODES {
            let trigger = ListingTrigger::Register { mode, price };
            assert_eq!(
                decide(None, trigger, NOW),
                Ok(ListingTransition::Changed {
                    listing: ListingSnapshot {
                        mode,
                        state: ListingState::Resolving,
                        state_version: ListingVersion::new(1)?,
                        price,
                        expires_at: NOW + Duration::seconds(86_400),
                        reason: None,
                    },
                    change: ListingChange::Registered,
                })
            );
            assert_eq!(
                transition_listing(
                    None,
                    trigger,
                    NOW,
                    ForcedTerminationFacts { service_disabled: true, ..Default::default() },
                ),
                Err(ListingError::InvalidTransition { from: "absent", action: "register" })
            );
            assert_eq!(
                decide(None, trigger, Timestamp::MAX_UTC),
                Err(ListingError::InvalidTransition { from: "absent", action: "set_deadline" })
            );
        }
        Ok(())
    }

    #[test]
    fn registration_rejects_known_ineligible_sellers_in_both_modes() -> Result<(), ListingError> {
        let price = ListingPrice::new(101)?;
        let facts = ForcedTerminationFacts { seller_ineligible: true, ..Default::default() };
        for mode in MODES {
            assert_eq!(
                transition_listing(None, ListingTrigger::Register { mode, price }, NOW, facts),
                Err(ListingError::InvalidTransition { from: "absent", action: "register" }),
                "{mode:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn lookup_uses_current_price_and_starts_ttl_at_publication() -> Result<(), ListingError> {
        for (mode, state, default_days, configured_days) in [
            (ListingMode::Direct, ListingState::Displayed, 30, 90),
            (ListingMode::Consigned, ListingState::AwaitingDeposit, 7, 1),
        ] {
            let original = snapshot(mode, ListingState::Resolving)?;
            let current_price = ListingPrice::new(119)?;
            let repriced = decide(
                Some(original),
                ListingTrigger::Reprice {
                    expected_version: original.state_version,
                    price: current_price,
                },
                NOW,
            )?;
            let ListingTransition::Changed { listing, change: ListingChange::Repriced } = repriced
            else {
                panic!("预期解析期间可改价，实际 {repriced:?}");
            };
            let publish_at = NOW + Duration::seconds(42);
            for (ttls, days) in [
                (ListingPublicationTtls::default(), default_days),
                (ListingPublicationTtls::new(90, 1)?, configured_days),
            ] {
                assert_eq!(
                    decide(Some(listing), ListingTrigger::LookupAccepted(ttls), publish_at),
                    Ok(ListingTransition::Changed {
                        listing: ListingSnapshot {
                            state,
                            state_version: ListingVersion::new(5)?,
                            price: current_price,
                            expires_at: publish_at + Duration::days(days),
                            ..listing
                        },
                        change: ListingChange::Published,
                    })
                );
            }
        }
        Ok(())
    }

    #[test]
    fn publication_ttls_reject_out_of_range_and_unrepresentable_deadline()
    -> Result<(), ListingError> {
        assert!(ListingPublicationTtls::new(1, 30).is_ok());
        for (display, deposit) in [(0, 7), (91, 7), (30, 0), (30, 31), (-1, 7), (30, i64::MAX)] {
            assert_eq!(
                ListingPublicationTtls::new(display, deposit),
                Err(ListingError::InvalidTransition {
                    from: "resolving",
                    action: "configure_publication_ttl",
                })
            );
        }
        for mode in MODES {
            let listing = ListingSnapshot {
                expires_at: Timestamp::MAX_UTC,
                ..snapshot(mode, ListingState::Resolving)?
            };
            assert_eq!(
                decide(
                    Some(listing),
                    ListingTrigger::LookupAccepted(ListingPublicationTtls::default()),
                    Timestamp::MAX_UTC - Duration::milliseconds(1),
                ),
                Err(ListingError::InvalidTransition { from: "resolving", action: "set_deadline" })
            );
        }
        Ok(())
    }

    #[test]
    fn lookup_rejections_preserve_only_compliant_snapshots() -> Result<(), ListingError> {
        for mode in MODES {
            let listing = snapshot(mode, ListingState::Resolving)?;
            for (reason, keep_snapshot) in [
                (ListingReason::NotTradeable, true),
                (ListingReason::OwnerMismatch, true),
                (ListingReason::HistoryOwnerMismatch, true),
                (ListingReason::HistoryAliasConflict, true),
                (ListingReason::HistoryUnproven, true),
                (ListingReason::EquipmentBusy, true),
                (ListingReason::EquipmentNotFound, false),
                (ListingReason::SourceRequestRejected, false),
                (ListingReason::SourceHttpInvalid, false),
                (ListingReason::SourceSchemaInvalid, false),
                (ListingReason::SourceTooLarge, false),
                (ListingReason::SourceShapeUnconfirmed, false),
                (ListingReason::SourceIdentityMismatch, false),
                (ListingReason::OwnerMissing, false),
                (ListingReason::CredentialUnavailable, false),
                (ListingReason::LookupExhausted, false),
            ] {
                assert_eq!(
                    decide(Some(listing), ListingTrigger::LookupRejected(reason), NOW),
                    Ok(ListingTransition::Changed {
                        listing: ListingSnapshot {
                            state: ListingState::Rejected,
                            state_version: ListingVersion::new(4)?,
                            reason: Some(reason),
                            ..listing
                        },
                        change: ListingChange::Rejected { keep_snapshot },
                    }),
                    "{mode:?}: {reason:?}"
                );
            }
            for reason in [
                ListingReason::LookupDeadline,
                ListingReason::DisplayExpired,
                ListingReason::DepositExpired,
                ListingReason::AuctionLotCreated,
                ListingReason::SellerIneligible,
                ListingReason::ServiceDisabled,
                ListingReason::SellerCancelled,
                ListingReason::AdminDelisted,
            ] {
                assert_eq!(
                    decide(Some(listing), ListingTrigger::LookupRejected(reason), NOW),
                    Err(ListingError::InvalidTransition {
                        from: "resolving",
                        action: "apply_lookup"
                    })
                );
            }
        }
        Ok(())
    }

    #[test]
    fn each_activity_expires_at_its_own_deadline() -> Result<(), ListingError> {
        for (listing, reason) in active_listings()? {
            assert_eq!(
                decide(
                    Some(listing),
                    ListingTrigger::AutoClose,
                    listing.expires_at - Duration::milliseconds(1)
                ),
                Err(ListingError::InvalidTransition {
                    from: listing.state.as_str(),
                    action: "auto_close"
                })
            );
            for now in [listing.expires_at, listing.expires_at + Duration::milliseconds(1)] {
                assert_eq!(
                    decide(Some(listing), ListingTrigger::AutoClose, now),
                    Ok(ListingTransition::Changed {
                        listing: ListingSnapshot {
                            state: ListingState::Expired,
                            state_version: ListingVersion::new(4)?,
                            reason: Some(reason),
                            ..listing
                        },
                        change: ListingChange::Expired,
                    }),
                    "{listing:?}，时刻 {now}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn forced_causes_precede_expiration_and_stale_active_actions() -> Result<(), ListingError> {
        for (listing, expiry_reason) in active_listings()? {
            for (facts, reason, state, change) in [
                (
                    ForcedTerminationFacts {
                        auction_lot_created: true,
                        seller_ineligible: true,
                        service_disabled: true,
                    },
                    ListingReason::AuctionLotCreated,
                    ListingState::Closed,
                    ListingChange::Closed,
                ),
                (
                    ForcedTerminationFacts {
                        seller_ineligible: true,
                        service_disabled: true,
                        ..Default::default()
                    },
                    ListingReason::SellerIneligible,
                    ListingState::Closed,
                    ListingChange::Closed,
                ),
                (
                    ForcedTerminationFacts { service_disabled: true, ..Default::default() },
                    ListingReason::ServiceDisabled,
                    ListingState::Closed,
                    ListingChange::Closed,
                ),
                (
                    ForcedTerminationFacts::default(),
                    expiry_reason,
                    ListingState::Expired,
                    ListingChange::Expired,
                ),
            ] {
                let expected = ListingSnapshot {
                    state,
                    state_version: ListingVersion::new(4)?,
                    reason: Some(reason),
                    ..listing
                };
                let now = if facts == ForcedTerminationFacts::default() {
                    listing.expires_at
                } else {
                    NOW
                };
                assert_eq!(
                    transition_listing(Some(listing), ListingTrigger::AutoClose, now, facts),
                    Ok(ListingTransition::Changed { listing: expected, change })
                );
                let expected_version = ListingVersion::new(2)?;
                for trigger in [
                    ListingTrigger::Reprice { expected_version, price: listing.price },
                    ListingTrigger::Withdraw { expected_version },
                    ListingTrigger::AdminDelist { expected_version },
                    ListingTrigger::LookupAccepted(ListingPublicationTtls::default()),
                    ListingTrigger::LookupRejected(ListingReason::NotTradeable),
                ] {
                    assert_eq!(
                        transition_listing(Some(listing), trigger, listing.expires_at, facts),
                        Ok(ListingTransition::ForcedTermination { listing: expected, change }),
                        "{trigger:?}: {facts:?}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn independent_close_facts_close_each_activity() -> Result<(), ListingError> {
        for (listing, _) in active_listings()? {
            for (facts, reason) in independent_close_facts() {
                assert_eq!(
                    transition_listing(Some(listing), ListingTrigger::AutoClose, NOW, facts),
                    Ok(ListingTransition::Changed {
                        listing: ListingSnapshot {
                            state: ListingState::Closed,
                            state_version: ListingVersion::new(4)?,
                            reason: Some(reason),
                            ..listing
                        },
                        change: ListingChange::Closed,
                    }),
                    "{listing:?}，原因 {reason:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn independent_close_facts_preempt_active_writes() -> Result<(), ListingError> {
        let price = ListingPrice::new(173)?;
        for (listing, _) in active_listings()? {
            let expected_version = listing.state_version;
            for (facts, reason) in independent_close_facts() {
                for trigger in [
                    ListingTrigger::Reprice { expected_version, price },
                    ListingTrigger::AdminDelist { expected_version },
                ] {
                    assert_eq!(
                        transition_listing(Some(listing), trigger, NOW, facts),
                        Ok(ListingTransition::ForcedTermination {
                            listing: ListingSnapshot {
                                state: ListingState::Closed,
                                state_version: ListingVersion::new(4)?,
                                reason: Some(reason),
                                ..listing
                            },
                            change: ListingChange::Closed,
                        }),
                        "{listing:?}，动作 {trigger:?}，原因 {reason:?}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn independent_close_facts_preempt_lookup_publication() -> Result<(), ListingError> {
        let trigger = ListingTrigger::LookupAccepted(ListingPublicationTtls::default());
        for mode in MODES {
            let listing = snapshot(mode, ListingState::Resolving)?;
            for (facts, reason) in independent_close_facts() {
                assert_eq!(
                    transition_listing(Some(listing), trigger, NOW, facts),
                    Ok(ListingTransition::ForcedTermination {
                        listing: ListingSnapshot {
                            state: ListingState::Closed,
                            state_version: ListingVersion::new(4)?,
                            reason: Some(reason),
                            ..listing
                        },
                        change: ListingChange::Closed,
                    }),
                    "{mode:?}，原因 {reason:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn current_version_reprices_without_extending_any_deadline() -> Result<(), ListingError> {
        for (listing, _) in active_listings()? {
            let expected_version = listing.state_version;
            let now = listing.expires_at - Duration::milliseconds(1);
            let price = ListingPrice::new(173)?;
            assert_eq!(
                decide(Some(listing), ListingTrigger::Reprice { expected_version, price }, now),
                Ok(ListingTransition::Changed {
                    listing: ListingSnapshot {
                        price,
                        state_version: ListingVersion::new(4)?,
                        ..listing
                    },
                    change: ListingChange::Repriced,
                })
            );
            assert_eq!(
                decide(
                    Some(listing),
                    ListingTrigger::Reprice { expected_version, price: listing.price },
                    now
                ),
                Ok(ListingTransition::Unchanged(listing))
            );
        }
        Ok(())
    }

    #[test]
    fn current_version_allows_seller_and_admin_closures() -> Result<(), ListingError> {
        for (listing, _) in active_listings()? {
            let expected_version = listing.state_version;
            for (trigger, reason) in [
                (ListingTrigger::Withdraw { expected_version }, ListingReason::SellerCancelled),
                (ListingTrigger::AdminDelist { expected_version }, ListingReason::AdminDelisted),
            ] {
                assert_eq!(
                    decide(Some(listing), trigger, NOW),
                    Ok(ListingTransition::Changed {
                        listing: ListingSnapshot {
                            state: ListingState::Closed,
                            state_version: ListingVersion::new(4)?,
                            reason: Some(reason),
                            ..listing
                        },
                        change: ListingChange::Closed,
                    })
                );
            }
        }
        Ok(())
    }

    #[test]
    fn version_mismatch_rejects_all_mutations_including_same_price() -> Result<(), ListingError> {
        for (listing, _) in active_listings()? {
            for version in [2, 4] {
                let expected_version = ListingVersion::new(version)?;
                for trigger in [
                    ListingTrigger::Reprice { expected_version, price: ListingPrice::new(102)? },
                    ListingTrigger::Reprice { expected_version, price: listing.price },
                    ListingTrigger::Withdraw { expected_version },
                    ListingTrigger::AdminDelist { expected_version },
                ] {
                    assert_eq!(
                        decide(Some(listing), trigger, NOW),
                        Err(ListingError::StaleVersion { expected: version, actual: 3 })
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn terminal_listings_reject_every_trigger_and_preserve_first_version()
    -> Result<(), ListingError> {
        let facts = ForcedTerminationFacts {
            auction_lot_created: true,
            seller_ineligible: true,
            service_disabled: true,
        };
        for mode in MODES {
            for (state, reason) in [
                (ListingState::Rejected, ListingReason::NotTradeable),
                (ListingState::Expired, ListingReason::LookupDeadline),
                (ListingState::Closed, ListingReason::SellerCancelled),
            ] {
                let listing = ListingSnapshot { reason: Some(reason), ..snapshot(mode, state)? };
                let expected_version = listing.state_version;
                for (trigger, action) in [
                    (
                        ListingTrigger::Reprice {
                            expected_version,
                            price: ListingPrice::new(102)?,
                        },
                        "reprice",
                    ),
                    (ListingTrigger::Reprice { expected_version, price: listing.price }, "reprice"),
                    (ListingTrigger::Withdraw { expected_version }, "withdraw"),
                    (ListingTrigger::AdminDelist { expected_version }, "admin_delist"),
                    (
                        ListingTrigger::LookupAccepted(ListingPublicationTtls::default()),
                        "apply_lookup",
                    ),
                    (ListingTrigger::LookupRejected(ListingReason::OwnerMismatch), "apply_lookup"),
                    (ListingTrigger::AutoClose, "auto_close"),
                    (ListingTrigger::Register { mode, price: listing.price }, "register"),
                ] {
                    assert_eq!(
                        transition_listing(Some(listing), trigger, listing.expires_at, facts),
                        Err(ListingError::InvalidTransition { from: state.as_str(), action }),
                        "{mode:?}: {trigger:?}"
                    );
                }

                let expected_version = ListingVersion::new(2)?;
                for trigger in [
                    ListingTrigger::Reprice { expected_version, price: listing.price },
                    ListingTrigger::Withdraw { expected_version },
                    ListingTrigger::AdminDelist { expected_version },
                ] {
                    assert_eq!(
                        transition_listing(Some(listing), trigger, listing.expires_at, facts),
                        Err(ListingError::StaleVersion { expected: 2, actual: 3 })
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn rejects_wrong_source_state_and_inconsistent_snapshots() -> Result<(), ListingError> {
        let price = ListingPrice::new(101)?;
        let expected_version = ListingVersion::new(3)?;
        let accepted = ListingTrigger::LookupAccepted(ListingPublicationTtls::default());
        let rejected = ListingTrigger::LookupRejected(ListingReason::NotTradeable);
        for (trigger, action) in [
            (accepted, "apply_lookup"),
            (rejected, "apply_lookup"),
            (ListingTrigger::AutoClose, "auto_close"),
            (ListingTrigger::Reprice { expected_version, price }, "reprice"),
            (ListingTrigger::Withdraw { expected_version }, "withdraw"),
            (ListingTrigger::AdminDelist { expected_version }, "admin_delist"),
        ] {
            assert_eq!(
                decide(None, trigger, NOW),
                Err(ListingError::InvalidTransition { from: "absent", action })
            );
        }
        for (listing, _) in active_listings()? {
            assert_eq!(
                decide(Some(listing), ListingTrigger::Register { mode: listing.mode, price }, NOW),
                Err(ListingError::InvalidTransition {
                    from: listing.state.as_str(),
                    action: "register"
                })
            );
            if listing.state != ListingState::Resolving {
                for trigger in [accepted, rejected] {
                    assert_eq!(
                        decide(Some(listing), trigger, NOW),
                        Err(ListingError::InvalidTransition {
                            from: listing.state.as_str(),
                            action: "apply_lookup"
                        })
                    );
                }
            }
        }
        let direct = snapshot(ListingMode::Direct, ListingState::Resolving)?;
        let consigned = snapshot(ListingMode::Consigned, ListingState::Resolving)?;
        for listing in [
            ListingSnapshot { state: ListingState::AwaitingDeposit, ..direct },
            ListingSnapshot { state: ListingState::Displayed, ..consigned },
            ListingSnapshot { reason: Some(ListingReason::NotTradeable), ..direct },
            ListingSnapshot {
                state: ListingState::Rejected,
                reason: Some(ListingReason::SellerCancelled),
                ..direct
            },
            ListingSnapshot { state: ListingState::Expired, ..consigned },
        ] {
            assert_eq!(
                decide(Some(listing), ListingTrigger::Withdraw { expected_version }, NOW),
                Err(ListingError::InvalidTransition {
                    from: listing.state.as_str(),
                    action: "decode_snapshot"
                })
            );
        }
        Ok(())
    }

    #[test]
    fn version_ceiling_rejects_facts_but_allows_exact_noop() -> Result<(), ListingError> {
        let expected_version = ListingVersion::new(9_007_199_254_740_991)?;
        let listing = ListingSnapshot {
            state_version: expected_version,
            ..snapshot(ListingMode::Direct, ListingState::Displayed)?
        };
        assert_eq!(
            decide(
                Some(listing),
                ListingTrigger::Reprice { expected_version, price: ListingPrice::new(191)? },
                NOW
            ),
            Err(ListingError::InvalidTransition { from: "displayed", action: "advance_version" })
        );
        assert_eq!(
            decide(Some(listing), ListingTrigger::AutoClose, listing.expires_at),
            Err(ListingError::InvalidTransition { from: "displayed", action: "advance_version" })
        );
        assert_eq!(
            decide(
                Some(listing),
                ListingTrigger::Reprice { expected_version, price: listing.price },
                NOW
            ),
            Ok(ListingTransition::Unchanged(listing))
        );
        Ok(())
    }

    #[test]
    fn custody_managed_states_reject_every_legacy_trigger() -> Result<(), ListingError> {
        let expected_version = ListingVersion::new(3)?;
        let price = ListingPrice::new(101)?;
        for state in [
            ListingState::DepositAccepted,
            ListingState::Available,
            ListingState::SalePending,
            ListingState::HeldOffMarket,
            ListingState::WithdrawalPending,
        ] {
            let listing = ListingSnapshot {
                mode: ListingMode::Consigned,
                state,
                state_version: expected_version,
                price,
                expires_at: NOW + Duration::seconds(60),
                reason: None,
            };
            for trigger in [
                ListingTrigger::Reprice { expected_version, price },
                ListingTrigger::Withdraw { expected_version },
                ListingTrigger::AdminDelist { expected_version },
                ListingTrigger::AutoClose,
                ListingTrigger::LookupAccepted(ListingPublicationTtls::default()),
            ] {
                assert!(
                    decide(Some(listing), trigger, NOW).is_err(),
                    "{state:?} 不得执行旧 v1 迁移"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn change_codes_match_internal_event_contract() {
        for (event, code) in [
            (ListingChange::Registered, "registered"),
            (ListingChange::Published, "published"),
            (ListingChange::Rejected { keep_snapshot: true }, "rejected"),
            (ListingChange::Rejected { keep_snapshot: false }, "rejected"),
            (ListingChange::Expired, "expired"),
            (ListingChange::Closed, "closed"),
            (ListingChange::Repriced, "repriced"),
        ] {
            assert_eq!(event.as_str(), code);
        }
    }
}
