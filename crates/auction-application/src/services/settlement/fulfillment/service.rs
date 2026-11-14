//! 结算履约 prepare、activate、reconcile 服务编排。

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::validation::repository_error;
use crate::errors::ApplicationError;
use crate::errors::diagnostics::{
    DiagnosticContext, DiagnosticError, DiagnosticLevel, record_error,
};
use crate::ports::auction::ApplicationFuture;
use crate::ports::core::Clock;
use crate::ports::settlement::{
    SettlementFulfillmentActivationResult, SettlementFulfillmentClaim,
    SettlementFulfillmentCommandPort, SettlementFulfillmentCustodyReader,
    SettlementFulfillmentEvidenceReader, SettlementFulfillmentPhaseResult,
    SettlementFulfillmentRecipientReader, SettlementFulfillmentReconcileResult,
    SettlementFulfillmentStore,
};

/// 结算履约应用服务：制作 prepared delivery outbox、激活投递并按 Sent 证据对账。
/// 不读取密钥、不直接发 HTTP，也不把 `prepared` 误报为外部发送成功；外部发送由
/// delivery Worker 负责。
pub struct SettlementFulfillmentService {
    pub(super) store: Arc<dyn SettlementFulfillmentStore>,
    pub(super) recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader>,
    pub(super) custody_reader: Arc<dyn SettlementFulfillmentCustodyReader>,
    pub(super) evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) internal: Option<(
        Arc<dyn crate::ports::settlement::SettlementInternalFulfillmentStore>,
        Arc<dyn crate::ports::settlement::SettlementInternalEvidenceReader>,
    )>,
    pub(super) funding_policy_reader:
        Option<Arc<dyn crate::ports::settlement::FundingPolicyReader>>,
    /// Admin runtime 配置的 bot 显示名；命中时不做外部寄信，直接走内部履约。
    pub(super) bot_players: Vec<String>,
    /// 等待买家支付 COD 或卖家领取货款的共享 MM 追踪窗口；默认五分钟。
    pub(super) evidence_poll_delay: Duration,
}

impl SettlementFulfillmentService {
    /// 绑定履约存储端口，保持 Worker 装配根不依赖具体数据库类型。
    pub fn new(
        store: Arc<dyn SettlementFulfillmentStore>,
        recipient_reader: Arc<dyn SettlementFulfillmentRecipientReader>,
        custody_reader: Arc<dyn SettlementFulfillmentCustodyReader>,
        evidence_reader: Arc<dyn SettlementFulfillmentEvidenceReader>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            store,
            recipient_reader,
            custody_reader,
            evidence_reader,
            clock,
            internal: None,
            funding_policy_reader: None,
            bot_players: Vec::new(),
            evidence_poll_delay: Duration::seconds(300),
        }
    }

    /// 覆盖等待买家/卖家在 MM 领取资产的共享追踪窗口。
    pub fn with_evidence_poll_delay(mut self, delay: Duration) -> Self {
        self.evidence_poll_delay = delay;
        self
    }

    /// 余额是否参与履约由数据库持久策略决定，默认保留严格现金检查。
    pub fn with_funding_policy_reader(
        mut self,
        reader: Arc<dyn crate::ports::settlement::FundingPolicyReader>,
    ) -> Self {
        self.funding_policy_reader = Some(reader);
        self
    }

    pub(super) async fn funding_policy(
        &self,
    ) -> Result<crate::ports::settlement::FundingPolicy, ApplicationError> {
        match &self.funding_policy_reader {
            Some(reader) => reader.funding_policy().await.map_err(|error| {
                record_error(
                    DiagnosticLevel::Warn,
                    error.diagnostic_kind(),
                    "fulfillment",
                    "read_funding_policy",
                    DiagnosticContext::default(),
                );
                repository_error("资金记账策略暂不可用")
            }),
            None => Ok(crate::ports::settlement::FundingPolicy {
                mode: crate::ports::settlement::FundingAccountingMode::StrictCash,
                expected_uid: None,
                policy_version: 0,
            }),
        }
    }

    /// COD 付款与卖家收款共用同一追踪窗口；新邮件也等到下一个窗口再读取。
    pub(super) fn collection_next_poll(
        &self,
        claim: &SettlementFulfillmentClaim,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        if !matches!(
            claim.kind,
            auction_domain::SettlementObligationKind::WinnerEquipmentCod
                | auction_domain::SettlementObligationKind::WinnerPaymentCarrierCod
                | auction_domain::SettlementObligationKind::SellerPayout
        ) {
            return None;
        }
        let window = self.evidence_poll_delay.num_seconds();
        if window <= 0 {
            return None;
        }
        let seconds = now.timestamp();
        let next = (seconds / window).checked_add(1)?.checked_mul(window)?;
        now.checked_add_signed(Duration::seconds(next.checked_sub(seconds)?))
    }

    /// 绑定 Admin runtime 的 bot 显示名列表；比较时大小写不敏感。
    pub fn with_bot_players(mut self, bot_players: Vec<String>) -> Self {
        self.bot_players = bot_players;
        self
    }

    pub(super) fn is_configured_bot(&self, player_name: &str) -> bool {
        self.bot_players.iter().any(|bot| bot.eq_ignore_ascii_case(player_name))
    }
}

impl SettlementFulfillmentCommandPort for SettlementFulfillmentService {
    fn prepare_next(
        &self,
        claim: SettlementFulfillmentClaim,
        prepared_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<'_, crate::ports::settlement::SettlementFulfillmentPreparationOutcome>
    {
        Box::pin(async move { self.prepare_classified(claim, prepared_at, retry_delay).await })
    }

    fn activate(
        &self,
        claim: SettlementFulfillmentClaim,
        activated_at: DateTime<Utc>,
        retry_delay: Duration,
    ) -> ApplicationFuture<
        '_,
        SettlementFulfillmentPhaseResult<SettlementFulfillmentActivationResult>,
    > {
        Box::pin(async move { self.activate_now(claim, activated_at, retry_delay).await })
    }

    fn reconcile(
        &self,
        claim: SettlementFulfillmentClaim,
        observed_at: DateTime<Utc>,
        retry_at: DateTime<Utc>,
    ) -> ApplicationFuture<'_, SettlementFulfillmentReconcileResult> {
        Box::pin(async move { self.reconcile_now(claim, observed_at, retry_at).await })
    }
}
