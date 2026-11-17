//! 现金 sending 围栏对邮件证据拒绝原因的分类（PostgreSQL 18，运行账号）。
//!
//! 证据 owner 把“凭据或运行时 owner 在观察之后正常推进”（凭据删除、停用、改版本，generation 被
//! 新 owner 推进）与证据结构、generation、绑定、时间异常区分为类型化原因：前者在围栏里是业务
//! 延期 Deferred，后者回滚后带安全分类返回 Faulted，Worker 据此保留错误信号与计数。第一条用例
//! 直接调用证据 owner 的三个入口核对类型化原因；第二条经 `start_sending_authorized_until` 驱动
//! 真实现金围栏，核对 Deferred 与 Faulted 都不留任何写入，再用一条合法证据进入 sending 作对照。
//!
//! 证据快照由测试按生产形状合成（各级摘要用生产 `compute_digest`），不是真实 MoogleMail 会话的
//! 观察；“未来观测”用观察时刻晚于数据库时钟的快照代替真实的应用与数据库时钟偏差，“已过期”用
//! 早于数据库时钟的时窗代替事务等待越过证据寿命。
//!
//! 仅在 `test-db` feature 下编译，入口见 `just test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

use auction_application::core::RequestHash;
use auction_application::core::diagnostics::SafeErrorKind;
use auction_application::delivery::{
    DeliveryClaimExclusions, DeliveryFenceOutcome, DeliveryPreparedExpectation,
    DeliverySendCredentialBinding, DeliverySendExpectation, DeliveryWorkClaim, DeliveryWorkStore,
};
use auction_application::settlement::{
    INTERNAL_EVIDENCE_PROTOCOL, InternalEvidencePurpose, MailAccountEvidenceBinding,
    MailRuntimeBinding, SettlementInternalEvidenceSnapshot, VerifiedMainCashObservation,
};
use auction_domain::SettlementWorld;
use auction_storage::{DatabaseStore, MailEvidenceRejection, MailEvidenceTransactionError};
use chrono::{Duration, DurationRound, Utc};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::support;

/// 合成邮件账号的认证 UID 与凭据登记的预期玩家名。
pub(crate) const BOT_UID: &str = "7100001";
pub(crate) const BOT_NAME: &str = "Fence Bot";
/// 持久 generation；“已被新 owner 推进”的证据用更小的值。
pub(crate) const GENERATION: i64 = 3;
/// active 凭据的 state_version；“观察后已改版本”的证据用更小的值。
pub(crate) const CREDENTIAL_VERSION: i64 = 2;
/// 现金 Send 的 Credits。
pub(crate) const CREDITS: i64 = 5;

/// 写入一条合成凭据（密文不可解密）；同一种 Cookie 全库最多一条 active。
pub(crate) async fn insert_credential(
    pool: &PgPool,
    kind: &str,
    status: &str,
    version: i64,
    player_name: Option<&str>,
) -> Uuid {
    let id = Uuid::new_v4();
    let scope = if kind == "mail_cookie" { "mooglemail:main" } else { "forum:shared" };
    sqlx::query(
        "INSERT INTO public.credential_keys
            (id, credential_kind, scope, label, ciphertext, nonce, encryption_key_id, fingerprint,
             status, state_version, player_name)
         VALUES ($1, $2, $3, $4, $5, $6, 'test-key', $4, $7, $8, $9)",
    )
    .bind(id)
    .bind(kind)
    .bind(scope)
    .bind(format!("evidence-{id}"))
    .bind(vec![1_u8])
    .bind(vec![0_u8; 12])
    .bind(status)
    .bind(version)
    .bind(player_name)
    .execute(pool)
    .await
    .expect("写入合成凭据");
    id
}

/// 把持久 generation 设为给定值（新 owner 取得所有权时的同一张表）。
pub(crate) async fn set_generation(pool: &PgPool, generation: i64) {
    sqlx::query(
        "INSERT INTO public.mail_runtime_generation (scope, generation, activated_at)
         VALUES ('mail_cookie', $1, now())
         ON CONFLICT (scope) DO UPDATE
         SET generation = EXCLUDED.generation, activated_at = EXCLUDED.activated_at",
    )
    .bind(generation)
    .execute(pool)
    .await
    .expect("写入邮件运行时 generation");
}

/// 待发送的手工 Send：Main、没有 COD 与装备；Credits 非零时必须经现金授权。
pub(crate) async fn insert_manual_send(pool: &PgPool, recipient: &str, credits: i64) -> Uuid {
    let delivery_id = Uuid::new_v4();
    let payload = json!({
        "schema_version": 1,
        "operation": "send",
        "world": "main",
        "external_mail_id": delivery_id.to_string(),
        "template": "manual_notice",
        "result_code": "test",
        "user_key_user_id": null,
        "credits": credits,
        "cod": 0,
        "attachment_ids": [],
        "body": "Frozen body",
    });
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload, status)
         VALUES ($1, $2, decode(repeat('35', 32), 'hex'), 'manual', $3, $4, $5, 'pending')",
    )
    .bind(delivery_id)
    .bind(Uuid::new_v4())
    .bind(format!("cash-fence:{delivery_id}"))
    .bind(recipient)
    .bind(&payload)
    .execute(pool)
    .await
    .expect("写入手工 Send");
    delivery_id
}

/// 合成证据的可变事实；[`EvidenceShape::current`] 与 active 凭据、持久 generation 完全一致。
#[derive(Clone, Copy)]
pub(crate) struct EvidenceShape {
    pub(crate) credential: Uuid,
    pub(crate) credential_version: i64,
    pub(crate) generation: i64,
    pub(crate) player_name: &'static str,
    pub(crate) world: SettlementWorld,
    /// 观察时刻相对现在的秒数：正值是未来观测，负值越过 60 秒证据时窗即已过期。
    pub(crate) observed_offset_seconds: i64,
    /// 是否附带 Main 现金观察；未开户的现金 Send 只凭它进入 sending。
    pub(crate) main_cash: bool,
}

impl EvidenceShape {
    pub(crate) fn current(credential: Uuid) -> Self {
        Self {
            credential,
            credential_version: CREDENTIAL_VERSION,
            generation: GENERATION,
            player_name: BOT_NAME,
            world: SettlementWorld::Main,
            observed_offset_seconds: 0,
            main_cash: false,
        }
    }

    /// 适配器在同一次准备里随证据产出的冻结 Send 凭据绑定。
    pub(crate) fn binding(&self) -> DeliverySendCredentialBinding {
        DeliverySendCredentialBinding {
            credential_id: self.credential,
            credential_version: self.credential_version,
            runtime_generation: self.generation,
        }
    }
}

/// 绑定 claim 令牌、fencing 与请求摘要的证据用途。
pub(crate) fn claim_purpose(claim: &DeliveryWorkClaim) -> InternalEvidencePurpose {
    InternalEvidencePurpose::DeliveryClaim {
        delivery_id: claim.delivery_id,
        claim_token: claim.lease_token,
        claim_fencing: claim.fencing_version,
        request_hash: claim.request_hash,
    }
}

/// 按生产形状合成同 session 证据；各级摘要都用生产 `compute_digest` 计算，时窗 60 秒。
pub(crate) fn evidence(
    operation_id: Uuid,
    purpose: InternalEvidencePurpose,
    shape: &EvidenceShape,
) -> SettlementInternalEvidenceSnapshot {
    let observed = (Utc::now() + Duration::seconds(shape.observed_offset_seconds))
        .duration_trunc(Duration::microseconds(1))
        .expect("截断到微秒");
    let mut identity = MailAccountEvidenceBinding {
        credential_id: shape.credential,
        credential_version: shape.credential_version,
        runtime: MailRuntimeBinding { generation: shape.generation },
        authenticated_uid: BOT_UID.to_owned(),
        player_name: shape.player_name.to_owned(),
        world: shape.world,
        observed_at: observed,
        protocol_version: INTERNAL_EVIDENCE_PROTOCOL,
        digest: [0; 32],
    };
    identity.digest = identity.compute_digest();
    let main_cash = shape.main_cash.then(|| {
        let mut cash = VerifiedMainCashObservation {
            binding: identity.clone(),
            credits: 1_000,
            observed_at: observed,
            digest: [0; 32],
        };
        cash.digest = cash.compute_digest();
        cash
    });
    let mut snapshot = SettlementInternalEvidenceSnapshot {
        operation_id,
        purpose,
        identity,
        equipment: None,
        main_cash,
        sent_collection: None,
        observed_from: observed,
        expires_at: observed + Duration::seconds(60),
        digest: [0; 32],
    };
    snapshot.digest = snapshot.compute_digest();
    snapshot
}

/// 与传输边界同形的冻结 Send 身份；数据库凭据的会话带凭据绑定。
pub(crate) fn send_expectation(
    claim: &DeliveryWorkClaim,
    binding: Option<DeliverySendCredentialBinding>,
) -> Arc<DeliverySendExpectation> {
    Arc::new(DeliverySendExpectation {
        world_id: "main".to_owned(),
        recipient: claim.recipient.clone(),
        subject: "Cash fence notice".to_owned(),
        body_digest: [0x35; 32],
        credits: claim.payload["credits"].as_i64().expect("Credits"),
        cod: 0,
        equipment_ids: Vec::new(),
        credential: binding,
    })
}

/// 领取指定投递，租约两分钟。
async fn claim(store: &DatabaseStore, delivery_id: Uuid) -> DeliveryWorkClaim {
    let now = Utc::now();
    let grace = StdDuration::from_secs(60);
    store.begin_claim_cycle(now, grace).await.expect("领取前清理");
    store
        .claim_due(
            "evidence-fence-worker".to_owned(),
            now,
            now + Duration::minutes(2),
            grace,
            DeliveryClaimExclusions::default(),
            10,
        )
        .await
        .expect("领取投递")
        .into_iter()
        .find(|claim| claim.delivery_id == delivery_id)
        .expect("目标投递必须被领取")
}

/// 以现金证据调用发送围栏；Err（提交结果未知）视为用例失败。
async fn fence(
    store: &DatabaseStore,
    claim: &DeliveryWorkClaim,
    snapshot: &SettlementInternalEvidenceSnapshot,
    binding: DeliverySendCredentialBinding,
) -> DeliveryFenceOutcome {
    let prepared = DeliveryPreparedExpectation::Send(send_expectation(claim, Some(binding)));
    store
        .start_sending_authorized_until(
            claim.clone(),
            Utc::now(),
            snapshot,
            Instant::now() + StdDuration::from_secs(30),
            Some(&prepared),
        )
        .await
        .expect("现金围栏必须确定结束")
}

/// 证据 owner 的拒绝原因；通过时为 None，数据库错误视为用例失败。
fn rejection(result: Result<(), MailEvidenceTransactionError>) -> Option<MailEvidenceRejection> {
    match result {
        Ok(()) => None,
        Err(MailEvidenceTransactionError::Rejected(reason)) => Some(reason),
        Err(error) => panic!("证据 owner 不应返回数据库错误：{error}"),
    }
}

async fn generation_rejection(
    connection: &mut PgConnection,
    generation: i64,
) -> Option<MailEvidenceRejection> {
    rejection(
        auction_storage::lock_mail_runtime_generation(
            connection,
            &MailRuntimeBinding { generation },
        )
        .await,
    )
}

/// 投递状态、冻结的 Send 尝试数与 `delivery.start_sending` 审计数。
pub(crate) async fn fence_footprint(pool: &PgPool, delivery_id: Uuid) -> (String, i64, i64) {
    sqlx::query_as(
        "SELECT delivery.status,
                (SELECT count(*) FROM public.delivery_send_attempts AS attempt
                 WHERE attempt.delivery_id = delivery.id),
                (SELECT count(*) FROM public.audit_records AS audit
                 WHERE audit.entity_type = 'delivery' AND audit.entity_id = delivery.id::text
                   AND audit.action = 'delivery.start_sending')
         FROM public.deliveries AS delivery WHERE delivery.id = $1",
    )
    .bind(delivery_id)
    .fetch_one(pool)
    .await
    .expect("读取围栏痕迹")
}

/// 证据 owner 的三个入口逐一给出类型化原因：只有凭据删除、停用、改版本与 generation 被推进是
/// 正常失效（Stale）；generation 非正/超前/缺行、结构或摘要错误、种类/预期玩家名/作用域不符、
/// 持久版本低于证据，以及按数据库时钟过期或未来观测都是异常。种类等不变事实的不符优先于停用。
#[tokio::test]
async fn postgres_mail_evidence_owner_types_normal_invalidation_apart_from_faults() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();

    // 模板库还没有任何 owner 推进过 generation：持久行缺失时任何绑定都不可能有效。
    let mut transaction = pool.begin().await.expect("开启事务");
    assert_eq!(
        generation_rejection(&mut transaction, GENERATION).await,
        Some(MailEvidenceRejection::GenerationInvalid)
    );
    transaction.rollback().await.expect("回滚");
    set_generation(pool, GENERATION).await;
    let mut transaction = pool.begin().await.expect("开启事务");
    for (generation, expected) in [
        (GENERATION, None),
        (GENERATION - 1, Some(MailEvidenceRejection::Stale)),
        (GENERATION + 1, Some(MailEvidenceRejection::GenerationInvalid)),
        (0, Some(MailEvidenceRejection::GenerationInvalid)),
    ] {
        assert_eq!(
            generation_rejection(&mut transaction, generation).await,
            expected,
            "generation {generation}"
        );
    }
    transaction.rollback().await.expect("回滚");

    let active =
        insert_credential(pool, "mail_cookie", "active", CREDENTIAL_VERSION, Some(BOT_NAME)).await;
    let disabled = insert_credential(pool, "mail_cookie", "disabled", 1, Some(BOT_NAME)).await;
    let forum = insert_credential(pool, "forum_cookie", "active", 1, None).await;
    let current = EvidenceShape::current(active);
    let purpose = InternalEvidencePurpose::DeliveryClaim {
        delivery_id: Uuid::new_v4(),
        claim_token: Uuid::new_v4(),
        claim_fencing: 1,
        request_hash: RequestHash::new([0x35; 32]),
    };
    let snapshot = |shape: EvidenceShape| evidence(Uuid::new_v4(), purpose.clone(), &shape);
    let tampered = {
        let mut snapshot = snapshot(current);
        snapshot.digest = [0x5a; 32];
        snapshot
    };
    let cases = [
        ("合法证据", snapshot(current), None),
        ("摘要被改", tampered, Some(MailEvidenceRejection::StructureInvalid)),
        (
            "凭据已删除",
            snapshot(EvidenceShape { credential: Uuid::new_v4(), ..current }),
            Some(MailEvidenceRejection::Stale),
        ),
        (
            "凭据已停用",
            snapshot(EvidenceShape { credential: disabled, credential_version: 1, ..current }),
            Some(MailEvidenceRejection::Stale),
        ),
        (
            "凭据在观察后改版本",
            snapshot(EvidenceShape { credential_version: CREDENTIAL_VERSION - 1, ..current }),
            Some(MailEvidenceRejection::Stale),
        ),
        (
            "持久版本低于证据",
            snapshot(EvidenceShape { credential_version: CREDENTIAL_VERSION + 1, ..current }),
            Some(MailEvidenceRejection::BindingMismatch),
        ),
        (
            "认证玩家名不是预期名",
            snapshot(EvidenceShape { player_name: "Impostor Bot", ..current }),
            Some(MailEvidenceRejection::BindingMismatch),
        ),
        (
            "停用凭据的玩家名也不符：不变事实优先",
            snapshot(EvidenceShape {
                credential: disabled,
                credential_version: 1,
                player_name: "Impostor Bot",
                ..current
            }),
            Some(MailEvidenceRejection::BindingMismatch),
        ),
        (
            "凭据种类不是邮件 Cookie",
            snapshot(EvidenceShape { credential: forum, credential_version: 1, ..current }),
            Some(MailEvidenceRejection::BindingMismatch),
        ),
        (
            "凭据作用域不含证据世界",
            snapshot(EvidenceShape { world: SettlementWorld::Isekai, ..current }),
            Some(MailEvidenceRejection::BindingMismatch),
        ),
        (
            "未来观测",
            snapshot(EvidenceShape { observed_offset_seconds: 30, ..current }),
            Some(MailEvidenceRejection::TimeInvalid),
        ),
        (
            "按数据库时钟已过期",
            snapshot(EvidenceShape { observed_offset_seconds: -80, ..current }),
            Some(MailEvidenceRejection::TimeInvalid),
        ),
    ];
    let mut transaction = pool.begin().await.expect("开启事务");
    for (case, snapshot, expected) in &cases {
        assert_eq!(
            rejection(
                auction_storage::lock_mail_evidence_credentials(&mut transaction, snapshot).await
            ),
            *expected,
            "{case}"
        );
    }
    // 单独的时效入口同样先判结构，结构错误不冒充时间异常。
    for (case, expected) in [
        ("合法证据", None),
        ("摘要被改", Some(MailEvidenceRejection::StructureInvalid)),
        ("未来观测", Some(MailEvidenceRejection::TimeInvalid)),
    ] {
        let (_, snapshot, _) =
            cases.iter().find(|(name, _, _)| *name == case).expect("复用上面的证据");
        assert_eq!(
            rejection(
                auction_storage::validate_mail_evidence_time(&mut transaction, snapshot).await
            ),
            expected,
            "时效入口：{case}"
        );
    }
    transaction.rollback().await.expect("回滚");

    // 原因的安全分类：只有 Stale 没有分类，由围栏按业务延期收口。
    assert_eq!(MailEvidenceRejection::Stale.fault_kind(), None);
    assert_eq!(
        MailEvidenceRejection::GenerationInvalid.fault_kind(),
        Some(SafeErrorKind::EVIDENCE_REQUEST_INVALID)
    );
    assert_eq!(
        MailEvidenceRejection::StructureInvalid.fault_kind(),
        Some(SafeErrorKind::EVIDENCE_REQUEST_INVALID)
    );
    assert_eq!(
        MailEvidenceRejection::BindingMismatch.fault_kind(),
        Some(SafeErrorKind::DEPENDENCY_MISMATCH)
    );
    assert_eq!(
        MailEvidenceRejection::TimeInvalid.fault_kind(),
        Some(SafeErrorKind::EVIDENCE_EXPIRED)
    );

    store.postgres_pool().close().await;
}

/// 真实现金围栏：凭据或运行时 owner 在观察之后正常推进是 Deferred；证据与 claim、冻结凭据绑定
/// 不符，generation 超前，摘要错误，预期玩家名不符与未来观测都回滚为带分类的 Faulted，不能像
/// 修复前那样一律冒充业务延期。每次都不进入 sending、不冻结尝试、不写审计；最后一条合法证据
/// 照常进入 sending，证明前面的拒绝都来自证据本身。
#[tokio::test]
async fn postgres_cash_fence_defers_only_normal_evidence_invalidation() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let credential =
        insert_credential(pool, "mail_cookie", "active", CREDENTIAL_VERSION, Some(BOT_NAME)).await;
    set_generation(pool, GENERATION).await;
    let delivery_id = insert_manual_send(pool, "Cash Fence", CREDITS).await;
    let claim = claim(&store, delivery_id).await;
    let current = EvidenceShape::current(credential);
    let claimed = |shape: &EvidenceShape| evidence(claim.command_id, claim_purpose(&claim), shape);

    let foreign_operation = evidence(Uuid::new_v4(), claim_purpose(&claim), &current);
    let tampered = {
        let mut snapshot = claimed(&current);
        snapshot.digest = [0x5a; 32];
        snapshot
    };
    let ahead = EvidenceShape { generation: GENERATION + 1, ..current };
    let impostor = EvidenceShape { player_name: "Impostor Bot", ..current };
    let future = EvidenceShape { observed_offset_seconds: 20, ..current };
    let superseded = EvidenceShape { generation: GENERATION - 1, ..current };
    let older_version = EvidenceShape { credential_version: CREDENTIAL_VERSION - 1, ..current };
    let faulted = DeliveryFenceOutcome::Faulted;
    let cases = [
        (
            "证据不是为这次领取的稳定命令准备的",
            foreign_operation,
            current.binding(),
            faulted(SafeErrorKind::DEPENDENCY_MISMATCH),
        ),
        (
            "冻结 Send 的凭据绑定与证据身份不符",
            claimed(&current),
            DeliverySendCredentialBinding {
                credential_version: CREDENTIAL_VERSION - 1,
                ..current.binding()
            },
            faulted(SafeErrorKind::DEPENDENCY_MISMATCH),
        ),
        (
            "generation 高于持久值",
            claimed(&ahead),
            ahead.binding(),
            faulted(SafeErrorKind::EVIDENCE_REQUEST_INVALID),
        ),
        (
            "证据摘要被改",
            tampered,
            current.binding(),
            faulted(SafeErrorKind::EVIDENCE_REQUEST_INVALID),
        ),
        (
            "认证玩家名不是凭据预期名",
            claimed(&impostor),
            impostor.binding(),
            faulted(SafeErrorKind::DEPENDENCY_MISMATCH),
        ),
        (
            "按数据库时钟是未来观测",
            claimed(&future),
            future.binding(),
            faulted(SafeErrorKind::EVIDENCE_EXPIRED),
        ),
        (
            "generation 已被新 owner 推进",
            claimed(&superseded),
            superseded.binding(),
            DeliveryFenceOutcome::Deferred,
        ),
        (
            "凭据在观察后改版本",
            claimed(&older_version),
            older_version.binding(),
            DeliveryFenceOutcome::Deferred,
        ),
    ];
    for (case, snapshot, binding, expected) in &cases {
        assert_eq!(fence(&store, &claim, snapshot, binding.clone()).await, *expected, "{case}");
        assert_eq!(
            fence_footprint(pool, delivery_id).await,
            ("claimed".to_owned(), 0, 0),
            "{case}：确定回滚不得进入 sending、冻结尝试或写审计"
        );
    }

    // 对照：同一领取带合法证据与现金观察照常进入 sending。
    let valid = EvidenceShape { main_cash: true, ..current };
    assert_eq!(
        fence(&store, &claim, &claimed(&valid), valid.binding()).await,
        DeliveryFenceOutcome::Sending
    );
    assert_eq!(fence_footprint(pool, delivery_id).await, ("sending".to_owned(), 1, 1));

    store.postgres_pool().close().await;
}
