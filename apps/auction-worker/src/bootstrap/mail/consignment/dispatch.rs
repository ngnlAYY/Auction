//! 寄售派发的外写传输适配：起写前读冻结发送请求、按冻结身份重渲染并核对摘要，再在同一独占会话里
//! 做写前预检与 Sent 基线；拿到一次性许可后经既有 MoogleMail 投递传输写一次并取 Sent 证明。
//!
//! 保护不变量 4、6、7：
//! - 发送请求与已领取工作的轮键、冻结身份不一致，渲染出的主题 / 模板 / 正文摘要不等于冻结值，或会话
//!   没有可锁验的持久凭据时，`prepare` 返回 `None`：不登记尝试、不起写、零 POST。
//! - 冻结身份带 Credits 的现金轮（载体 1 Credit + COD P+1、卖款 S、退款 P）与原件轮走同一 executor：
//!   发信入口按冻结 Credits 组装，并在同一独占会话里取新鲜资金证据、核对认证 UID 等于成交冻结的资金 UID
//!   （维护者 2026-11-25 决定）；发送请求缺冻结资金 UID 或记账方式时不发。证据随准备交给起写事务，由资金
//!   owner 按通用现金门核对余额，不满足整笔回滚、零许可。
//! - `execute` 只有“已证明”与“未知”两种结论，句柄不符、许可不符、写入失败一律未知，绝不重发；
//!   本适配器从不返回错误，许可在本次调用里被消费。
//!
//! 收件人、正文与装备编号只进入本次请求，日志只记固定分类码。

use std::sync::Arc;

use auction_application::consignment::{
    ConsignmentFuture, DispatchSendRequest, DispatchWorkStore, PostPermit,
};
use auction_application::core::Clock;
use auction_domain::consignment::FundingAccountingMode;
use auction_domain::consignment::dispatch::AttemptKey;
use auction_integrations::mooglemail::{
    ConsignmentDispatchMail, ConsignmentDispatchOutcome, MoogleMailDeliveryTransport,
    MoogleMailWorld, PreparedConsignmentDispatch, render_frozen_dispatch,
};
use auction_jobs::consignment::{
    DispatchExecution, DispatchTransport, DispatchWork, PreparedDispatch, SentObservation,
};
use uuid::Uuid;

/// 经 Worker 已装配的 MoogleMail 投递传输发送寄售派发邮件。
pub struct MoogleMailDispatchTransport {
    /// 发送请求只读读口。
    pub work: Arc<dyn DispatchWorkStore>,
    /// 与 delivery 循环同一个 transport 实例：凭据独占锁与发送器状态只有一份。
    pub transport: Arc<MoogleMailDeliveryTransport>,
    /// Sent 观察时刻来源。
    pub clock: Arc<dyn Clock>,
}

impl DispatchTransport for MoogleMailDispatchTransport {
    fn prepare<'a>(
        &'a self,
        work: &'a DispatchWork,
        attempt: AttemptKey,
        claim_token: Uuid,
    ) -> ConsignmentFuture<'a, Option<PreparedDispatch>> {
        Box::pin(async move {
            let Some(request) = self.work.dispatch_send_request(attempt.delivery_id).await? else {
                return Ok(None);
            };
            let Some(mail) = dispatch_mail(work, attempt, claim_token, request) else {
                return Ok(None);
            };
            let Some(mut prepared) = self.transport.prepare_consignment_dispatch(mail).await else {
                return Ok(None);
            };
            // 起写在锁内验认 active 凭据；配置 Cookie 等没有持久身份的会话不能起写，丢弃即释放会话。
            let Some(credential) = prepared.credential() else {
                tracing::warn!(
                    code = "dispatch_credential_unbound",
                    "寄售派发会话没有持久凭据绑定，不起写"
                );
                return Ok(None);
            };
            // 现金轮的同会话资金证据交起写事务核对；会话仍留在句柄里，直到写入结束。
            let cash_evidence = prepared.take_cash_evidence();
            Ok(Some(PreparedDispatch { credential, cash_evidence, handle: Box::new(prepared) }))
        })
    }

    fn execute(
        &self,
        prepared: PreparedDispatch,
        permit: PostPermit,
    ) -> ConsignmentFuture<'_, DispatchExecution> {
        Box::pin(async move {
            // 只有本适配器构造的句柄能回到这里；类型不符时许可随之丢弃，不写、按未知交人工核对。
            let Ok(prepared) = prepared.handle.downcast::<PreparedConsignmentDispatch>() else {
                tracing::warn!(code = "dispatch_handle_invalid", "寄售派发句柄不符，不写入");
                return Ok(DispatchExecution::Unknown);
            };
            Ok(match self.transport.execute_consignment_dispatch(*prepared, permit).await {
                ConsignmentDispatchOutcome::Proven(sent) => {
                    DispatchExecution::Proven(SentObservation {
                        sent_mid: sent.sent_mid,
                        observed_at: self.clock.now(),
                        validator_version: sent.validator_version.to_owned(),
                        evidence_digest: sent.evidence_digest,
                    })
                }
                ConsignmentDispatchOutcome::Unknown => DispatchExecution::Unknown,
            })
        })
    }
}

/// 由冻结发送请求组装待发邮件；任一核对不成立返回 `None`（不发）。
fn dispatch_mail(
    work: &DispatchWork,
    attempt: AttemptKey,
    claim_token: Uuid,
    request: DispatchSendRequest,
) -> Option<ConsignmentDispatchMail> {
    // 衔接给出的投递必须就是已领取工作的那一轮，冻结身份逐字相同。
    if request.delivery_id != attempt.delivery_id
        || request.key != work.key
        || request.frozen != work.frozen
    {
        tracing::warn!(code = "dispatch_request_mismatch", "寄售派发请求与已领取工作不符，不发送");
        return None;
    }
    // 现金轮必须有成交冻结的资金 UID 与记账方式（Sale 合同）；缺失即不发（不变量 6）。
    let cash = request.frozen.credits > 0;
    if cash && (request.funding_uid.is_none() || request.funding_mode.is_none()) {
        tracing::warn!(
            code = "dispatch_cash_funding_missing",
            "寄售现金轮缺成交冻结的资金账户，不发送"
        );
        return None;
    }
    let Some(body) = render_frozen_dispatch(&request.key, &request.frozen, &request.item_name)
    else {
        tracing::warn!(code = "dispatch_render_mismatch", "寄售派发正文与冻结摘要不符，不发送");
        return None;
    };
    let identity_digest = request.frozen.identity_digest();
    let frozen = request.frozen;
    Some(ConsignmentDispatchMail {
        world: MoogleMailWorld::from(frozen.world),
        delivery_id: attempt.delivery_id,
        fence: attempt.fence,
        recipient: request.recipient,
        subject: frozen.subject,
        body,
        credits: frozen.credits,
        cod: frozen.cod,
        equipment_ids: frozen.equipment_ids,
        claim_token,
        identity_digest,
        funding_uid: if cash { request.funding_uid } else { None },
        // 严格现金取 main 现金观察，记账模式不读余额（与通用现金门对两种模式的证据要求一致）。
        observe_main_cash: request.funding_mode == Some(FundingAccountingMode::StrictCash),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use auction_application::consignment::{
        ConsignmentFuture, DispatchSendRequest, DispatchWorkClaim, DispatchWorkStore,
        QueuedRedispatch, RedispatchPrepared,
    };
    use auction_application::core::{ApplicationError, ApplicationFuture, Clock};
    use auction_application::mail::{MailLoginKey, MailLoginKeyReader};
    use auction_domain::consignment::dispatch::{
        AttemptKey, DispatchRoundKey, FrozenDispatchIdentity, dispatch_ref,
    };
    use auction_domain::consignment::{ConsignmentObligation, ListingPrice};
    use auction_domain::{
        ConsignmentContractId, ConsignmentContractKind, SettlementAmount, SettlementId,
        SettlementObligationKind, SettlementObligationState, SettlementWorld,
    };
    use auction_integrations::mooglemail::{
        FirstDispatchFacts, MoogleMailClient, MoogleMailClientConfig, MoogleMailCredentialSet,
        MoogleMailDeliveryTransport, render_first_dispatch,
    };
    use auction_jobs::consignment::{DispatchTransport, DispatchWork};
    use auction_test_support::mooglemail::MailDouble;
    use auction_time::Timestamp;
    use uuid::Uuid;

    use super::MoogleMailDispatchTransport;

    const OBLIGATION: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0001);
    const CONTRACT: Uuid = Uuid::from_u128(0x4000_8000_0000_0000_0002);
    const DELIVERY: Uuid = Uuid::from_u128(0xd1);
    const LEASE: Uuid = Uuid::from_u128(0xa2);
    const ITEM: &str = "Fixture Phase Shoes";

    struct NoLoginKeys;

    impl MailLoginKeyReader for NoLoginKeys {
        fn read(&self, _user_id: Uuid) -> ApplicationFuture<'_, MailLoginKey> {
            Box::pin(async { Err(ApplicationError::CapabilityUnavailable("login key")) })
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            Timestamp::parse_from_rfc3339("2026-12-01T00:00:00Z").expect("时刻")
        }
    }

    /// 只回答发送请求读口；其余端口被调用即接线错误。
    struct Requests(DispatchSendRequest);

    impl DispatchWorkStore for Requests {
        fn claim_dispatch_work(
            &self,
            _now: Timestamp,
            _lease_until: Timestamp,
        ) -> ConsignmentFuture<'_, Option<DispatchWorkClaim>> {
            panic!("传输不领取")
        }

        fn dispatch_send_request(
            &self,
            _delivery_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<DispatchSendRequest>> {
            let request = self.0.clone();
            Box::pin(async move { Ok(Some(request)) })
        }

        fn queued_redispatches(&self, _limit: u32) -> ConsignmentFuture<'_, Vec<QueuedRedispatch>> {
            panic!("传输不读补寄待办")
        }

        fn prepare_redispatch(
            &self,
            _dispatch_id: Uuid,
        ) -> ConsignmentFuture<'_, Option<RedispatchPrepared>> {
            panic!("传输不备好补寄")
        }
    }

    fn key() -> DispatchRoundKey {
        DispatchRoundKey { contract_id: CONTRACT, obligation_id: OBLIGATION, round: 1 }
    }

    /// 按首轮同一渲染实现冻结的 main 原件 COD 身份。
    fn frozen() -> FrozenDispatchIdentity {
        let payment = ConsignmentObligation {
            id: SettlementId::from_uuid(OBLIGATION),
            contract_id: ConsignmentContractId::from_uuid(CONTRACT),
            contract_kind: ConsignmentContractKind::Sale,
            world: SettlementWorld::Main,
            kind: SettlementObligationKind::WinnerEquipmentCod,
            state: SettlementObligationState::Ready,
            state_version: 1,
            policy_version: 1,
            recipient_user_id: None,
            recipient_game_identity: String::new(),
            amount: SettlementAmount::new(100).expect("金额"),
            carrier_credit: 0,
            custody_equipment_id: Uuid::from_u128(0x4000_8000_0000_0000_0003),
            prerequisite_id: None,
        };
        let round_ref = dispatch_ref(&key());
        let d0 = Timestamp::parse_from_rfc3339("2026-12-08T00:00:00Z").expect("期限");
        let mail = render_first_dispatch(&FirstDispatchFacts {
            world: SettlementWorld::Main,
            price: ListingPrice::new(100).expect("价格"),
            round_ref: &round_ref,
            payment_deadline_at: d0,
            item_name: ITEM,
            canonical_eid: "123456",
            payment: &payment,
        })
        .expect("渲染");
        FrozenDispatchIdentity {
            rights_holder_identity: "FakeBuyer".to_owned(),
            bot_uid: "12345".to_owned(),
            world: SettlementWorld::Main,
            subject: mail.subject.clone(),
            template: mail.template.to_owned(),
            body_digest: mail.body_digest(),
            round_ref,
            d0,
            credits: 0,
            cod: 100,
            equipment_ids: mail.equipment_ids,
        }
    }

    fn adapter(
        server: &MailDouble,
        frozen: &FrozenDispatchIdentity,
    ) -> MoogleMailDispatchTransport {
        let client = MoogleMailClient::with_config(MoogleMailClientConfig {
            endpoint: server.endpoint(),
            maximum_response_bytes: 8 * 1024 * 1024,
            minimum_request_interval: Duration::from_millis(1),
            request_timeout: Duration::from_secs(5),
        })
        .expect("loopback client");
        server.set_captured_pages_complete(true);
        let transport = MoogleMailDeliveryTransport::new(
            client,
            MoogleMailCredentialSet::new("ipb_member_id=fixture; ipb_pass_hash=fixture")
                .expect("fixture cookie"),
            Arc::new(NoLoginKeys),
        );
        MoogleMailDispatchTransport {
            work: Arc::new(Requests(DispatchSendRequest {
                delivery_id: DELIVERY,
                key: key(),
                recipient: "FakeBuyer".to_owned(),
                frozen: frozen.clone(),
                item_name: ITEM.to_owned(),
                funding_uid: None,
                funding_mode: None,
            })),
            transport: Arc::new(transport),
            clock: Arc::new(FixedClock),
        }
    }

    fn work(frozen: &FrozenDispatchIdentity) -> DispatchWork {
        DispatchWork {
            obligation_id: OBLIGATION,
            claim_token: Uuid::from_u128(0xa1),
            claim_fencing: 1,
            key: key(),
            frozen: frozen.clone(),
        }
    }

    /// 冻结正文摘要与重渲染不符：不准备、不触达邮件服务，零 POST。
    #[tokio::test]
    async fn rendered_digest_mismatch_never_reaches_the_mail_service() {
        let server = MailDouble::start().await.expect("loopback server");
        let mut frozen = frozen();
        frozen.body_digest[0] ^= 1;
        let adapter = adapter(&server, &frozen);

        let prepared = adapter
            .prepare(&work(&frozen), AttemptKey { delivery_id: DELIVERY, fence: 1 }, LEASE)
            .await
            .expect("准备");

        assert!(prepared.is_none());
        assert!(server.requests().is_empty(), "摘要不符不得发出任何请求");
        assert_eq!(server.write_requests(), 0);
    }

    /// 摘要相符时才进入邮件服务的写前预检；配置 Cookie 没有持久凭据绑定，仍不起写、零 POST。
    #[tokio::test]
    async fn matching_digest_reaches_preflight_but_an_unbound_session_does_not_start() {
        let server = MailDouble::start().await.expect("loopback server");
        let frozen = frozen();
        let adapter = adapter(&server, &frozen);

        let prepared = adapter
            .prepare(&work(&frozen), AttemptKey { delivery_id: DELIVERY, fence: 1 }, LEASE)
            .await
            .expect("准备");

        assert!(prepared.is_none(), "没有持久凭据绑定不能起写");
        assert!(!server.requests().is_empty(), "摘要相符才做写前预检");
        assert_eq!(server.write_requests(), 0);
    }

    /// 冻结身份带 Credits（载体、卖款、退款）而发送请求没有成交冻结的资金账户：失败关闭，零请求、零 POST。
    #[tokio::test]
    async fn cash_round_without_frozen_funding_never_reaches_the_mail_service() {
        let server = MailDouble::start().await.expect("loopback server");
        let mut frozen = frozen();
        frozen.credits = 1;
        let adapter = adapter(&server, &frozen);

        let prepared = adapter
            .prepare(&work(&frozen), AttemptKey { delivery_id: DELIVERY, fence: 1 }, LEASE)
            .await
            .expect("准备");

        assert!(prepared.is_none());
        assert!(server.requests().is_empty(), "缺资金账户的现金轮不得发出任何请求");
        assert_eq!(server.write_requests(), 0);
    }
}
