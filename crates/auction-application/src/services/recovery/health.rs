//! 持久资金策略控制现金观察；记账模式仍刷新恢复积压，不读取余额或伪造现金证明。

use std::sync::Arc;

use auction_domain::SettlementWorld;
use chrono::{DateTime, Utc};

use crate::{
    ApplicationError, ApplicationFuture, RecoveryBalanceStatus, RecoveryEvidenceStatus,
    RecoveryHealthConfig, RecoveryHealthRefreshPort, RecoveryHealthStore, RecoveryHealthUpdate,
    RepositoryError, SettlementFulfillmentEvidenceReader,
};

/// 刷新 recovery health；仅严格现金策略读取外部 Balance。
pub struct RecoveryHealthService {
    config: RecoveryHealthConfig,
    store: Arc<dyn RecoveryHealthStore>,
    balances: Arc<dyn SettlementFulfillmentEvidenceReader>,
    funding_policy_reader: Option<Arc<dyn crate::FundingPolicyReader>>,
}

impl RecoveryHealthService {
    /// 建立 health 刷新服务；外部读取失败通常落为 unavailable，账号战斗中则落为 busy。
    pub fn new(
        config: RecoveryHealthConfig,
        store: Arc<dyn RecoveryHealthStore>,
        balances: Arc<dyn SettlementFulfillmentEvidenceReader>,
    ) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self { config, store, balances, funding_policy_reader: None })
    }

    /// 与履约和投递共用数据库持久策略，避免健康轮询单独采集记账账号余额。
    pub fn with_funding_policy_reader(
        mut self,
        reader: Arc<dyn crate::FundingPolicyReader>,
    ) -> Self {
        self.funding_policy_reader = Some(reader);
        self
    }
}

impl RecoveryHealthRefreshPort for RecoveryHealthService {
    fn refresh(&self, now: DateTime<Utc>) -> ApplicationFuture<'_, ()> {
        Box::pin(async move {
            if now.timestamp_nanos_opt().is_none() {
                return Err(ApplicationError::Repository(RepositoryError::Failed(
                    "recovery health 刷新时间无效".to_owned(),
                )));
            }

            let observe_cash = match &self.funding_policy_reader {
                Some(reader) => reader
                    .funding_policy()
                    .await
                    .map_err(|_| {
                        ApplicationError::Repository(RepositoryError::Failed(
                            "recovery health 资金策略暂不可用".to_owned(),
                        ))
                    })?
                    .observes_cash(),
                None => true,
            };
            // 记账模式不读取任何外部余额，两个结算世界都只投影 not_applicable 并重算积压，
            // 与是否配置邮箱无关；现金模式只观察已配置邮箱的世界，未配置世界保持未知以便告警。
            const ALL_WORLDS: [SettlementWorld; 2] =
                [SettlementWorld::Main, SettlementWorld::Isekai];
            let worlds: &[SettlementWorld] =
                if observe_cash { &self.config.worlds } else { &ALL_WORLDS };
            for world in worlds.iter().copied() {
                let mut update = RecoveryHealthUpdate {
                    world,
                    balance_credits: None,
                    balance_observed_at: None,
                    balance_status: RecoveryBalanceStatus::Unavailable,
                    evidence_last_success_at: None,
                    evidence_status: RecoveryEvidenceStatus::Unavailable,
                    updated_at: now,
                };

                if !observe_cash {
                    update.balance_status = RecoveryBalanceStatus::NotApplicable;
                    update.evidence_status = RecoveryEvidenceStatus::NotApplicable;
                    // 不写新余额或成功时间；原历史值由存储保留，状态明确其已不参与当前健康。
                    update.validate().map_err(|message| {
                        ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
                    })?;
                    self.store.refresh_world_health(update).await?;
                    continue;
                }

                match self.balances.read_balance(world, now).await {
                    Ok(snapshot) => {
                        let valid_snapshot = snapshot.world == world
                            && snapshot.credits >= 0
                            && snapshot.observed_at.timestamp_nanos_opt().is_some()
                            && snapshot.observed_at <= now;
                        if valid_snapshot {
                            let observed_at = snapshot.observed_at;
                            update.balance_credits = Some(snapshot.credits);
                            update.balance_observed_at = Some(observed_at);
                            update.evidence_last_success_at = Some(observed_at);
                            if now
                                .signed_duration_since(observed_at)
                                .to_std()
                                .is_ok_and(|age| age > self.config.evidence_stale_after)
                            {
                                update.balance_status = RecoveryBalanceStatus::Stale;
                                update.evidence_status = RecoveryEvidenceStatus::Stale;
                            } else {
                                update.balance_status =
                                    if snapshot.credits < self.config.low_balance_threshold {
                                        RecoveryBalanceStatus::Low
                                    } else {
                                        RecoveryBalanceStatus::Ok
                                    };
                                update.evidence_status = RecoveryEvidenceStatus::Fresh;
                            }
                        }
                    }
                    Err(ApplicationError::FulfillmentAccountBusy) => {
                        update.balance_status = RecoveryBalanceStatus::Busy;
                    }
                    Err(_) => {}
                }

                update.validate().map_err(|message| {
                    ApplicationError::Repository(RepositoryError::Failed(message.to_owned()))
                })?;
                self.store.refresh_world_health(update).await?;
            }
            Ok(())
        })
    }
}
