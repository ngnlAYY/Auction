//! Admin 投递列表与玩家支持投递分区的缺省投影与扩展投影（`include=settlement_returns`）的
//! PostgreSQL 18 用例。
//!
//! 缺省响应要能被升级前已发布的 Admin 页面按闭集解码：结算退件 Take（来源
//! `settlement_return_action`）不在缺省投影内。排除必须在 SQL 里与计数、行数上限同一条件，不能在
//! 页内事后过滤，否则总数、分页与截断对不上。旧页面的闭集是独立抄录的成员断言，不是旧 decoder 的
//! 贯通执行。
//!
//! 仅在 `test-db` feature 下编译，入口见 `mise run test-db`。

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::assert_matches;

use auction_application::core::ApplicationError;
use auction_application::delivery::{
    DeliveryAcknowledgementCommand, DeliveryAdminAction, DeliveryAdminCommand, DeliveryAdminError,
    DeliveryAdminGuardBlocker, DeliveryAdminListRequest, DeliveryAdminPort,
    DeliveryDispositionPort, DeliveryFenceOutcome, DeliveryTransportError, DeliveryWorkStore,
    MAX_PLAYER_SUPPORT_ROWS, PlayerSupportBundle, PlayerSupportRequest, PlayerSupportSelector,
};
use auction_domain::DeliveryDispositionActorSource;
use auction_storage::DatabaseStore;
use auction_time::{Duration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use super::evidence_rejections::insert_manual_send;
use super::fence::{claim_due, claim_for, fence_outcome};
use crate::support;

/// 升级前已发布 Admin 页面（v0.1.5）解码投递时认可的来源闭集（`valid_origin_kind`）；独立抄录，
/// 代表仍在浏览器里运行的旧 decoder。
const RELEASED_DECODER_ORIGINS: [&str; 9] = [
    "settlement",
    "settlement_recovery_action",
    "user_key",
    "manual",
    "mail_intake",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
];

/// 数据库当前允许的全部来源；结算退件 Take 最后写入，按创建时间倒序排在第一页最前面。
const STORED_ORIGINS: [&str; 10] = [
    "settlement",
    "settlement_recovery_action",
    "user_key",
    "manual",
    "mail_intake",
    "operations_alert",
    "lot_watch",
    "seller_summary",
    "season_reminder",
    "settlement_return_action",
];

/// 结算退件 Take 的收件人固定为系统退件发件人。
const SYSTEM_SENDER: &str = "MoogleMail";

/// 写入一条 pending 投递；结算退件 Take 收件人是系统退件发件人，其余发给 `Sample`。
async fn insert_pending(pool: &PgPool, origin_kind: &str, created_at: Timestamp) -> Uuid {
    let recipient =
        if origin_kind == "settlement_return_action" { SYSTEM_SENDER } else { "Sample" };
    insert_pending_to(pool, origin_kind, recipient, created_at).await
}

/// 写入一条发给指定收件人的 pending 投递；结算退件 Take 用 Take 载荷，其余为普通 Send。
async fn insert_pending_to(
    pool: &PgPool,
    origin_kind: &str,
    recipient: &str,
    created_at: Timestamp,
) -> Uuid {
    let id = Uuid::new_v4();
    let payload = if origin_kind == "settlement_return_action" {
        r#"{"schema_version":1,"operation":"take","world":"main"}"#
    } else {
        r#"{"schema_version":1,"operation":"send","template":"register_key","credits":0,"cod":0,"attachment_ids":[]}"#
    };
    sqlx::query(
        "INSERT INTO public.deliveries
            (id, command_id, request_digest, origin_kind, origin_id, recipient, payload,
             status, created_at, updated_at)
         VALUES ($1, $2, decode(repeat('44', 32), 'hex'), $3, $4, $5, $6::jsonb,
                 'pending', $7, $7)",
    )
    .bind(id)
    .bind(Uuid::new_v4())
    .bind(origin_kind)
    .bind(format!("projection-{id}"))
    .bind(recipient)
    .bind(payload)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("写入投递");
    id
}

#[tokio::test]
async fn postgres_admin_reconcile_projects_missing_sending_fence_before_transition() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let pool = store.postgres_pool();
    for (failure_code, metadata, blocker) in [
        (
            "external_recovery_unresolved_send",
            serde_json::json!({}),
            Some(DeliveryAdminGuardBlocker::MissingSendingFence),
        ),
        (
            "mail_disposition_conflict",
            serde_json::json!({
                "reason_code": "mail_disposition_conflict", "preflight_blocked": true, "outcome_unknown": true,
            }),
            None,
        ),
    ] {
        let delivery_id = insert_pending(pool, "mail_intake", Timestamp::now()).await;
        sqlx::query(
            "UPDATE public.deliveries SET status='manual_reconcile', failure_code=$2,
             result_metadata=$3, fencing_version=1, completed_at=clock_timestamp(),
             updated_at=clock_timestamp() WHERE id=$1",
        )
        .bind(delivery_id)
        .bind(failure_code)
        .bind(metadata)
        .execute(pool)
        .await
        .unwrap();
        let (before, guard, _) = store.detail_with_guard(delivery_id).await.unwrap();
        let action = DeliveryAdminAction::ReconcileFailed;
        assert_eq!(guard.projected_blocker(action), blocker);
        let result = store
            .execute(DeliveryAdminCommand {
                command_id: Uuid::new_v4(),
                delivery_id,
                expected_fencing_version: 1,
                action,
                actor_id: "admin".to_owned(),
                failure_code: Some("verified_not_sent".to_owned()),
                external_reference: None,
                request_id: None,
                request_hash: action.request_hash(
                    delivery_id,
                    1,
                    None,
                    Some("verified_not_sent"),
                    "admin",
                ),
            })
            .await;
        if let Some(blocker) = blocker {
            assert_matches!(result, Err(ApplicationError::DeliveryAdmin(error)) if error == blocker.error()
            );
            assert_eq!(store.detail(delivery_id).await.unwrap(), before);
        } else {
            assert_eq!(result.unwrap().snapshot.status, "failed");
        }
    }
}

#[tokio::test]
async fn postgres_delivery_action_preview_checks_command_snapshot_shape() {
    let database = support::TestDatabase::create().await.unwrap();
    let store = database.store().await.unwrap();
    let pool = store.postgres_pool();
    let delivery_id = insert_manual_send(pool, "Sample", 0).await;
    let command = |action: DeliveryAdminAction, fence| DeliveryAdminCommand {
        command_id: Uuid::new_v4(),
        delivery_id,
        expected_fencing_version: fence,
        action,
        actor_id: "admin".to_owned(),
        failure_code: None,
        external_reference: None,
        request_id: None,
        request_hash: action.request_hash(delivery_id, fence, None, None, "admin"),
    };
    let claim = claim_for(&claim_due(&store).await, delivery_id);
    let release = command(DeliveryAdminAction::Release, claim.fencing_version);
    let release_id = release.command_id;
    store.execute(release).await.expect("先记录一次人工释放");

    let claim = claim_for(&claim_due(&store).await, delivery_id);
    let fence = claim.fencing_version;
    assert_matches!(fence_outcome(&store, &claim).await, DeliveryFenceOutcome::Sending);
    let completed_at = Timestamp::now();
    store.fail(claim, DeliveryTransportError {
        failure_code: "verified_not_sent".to_owned(),
        safe_detail: "传输替身确认未寄出".to_owned(),
        outcome_unknown: false,
        external_reference: None,
        result_metadata: serde_json::json!({"mailbox_sweep": {"mid": "9090", "observed_at": completed_at.to_rfc3339()}}),
    }, completed_at).await.unwrap();

    let (before, guard, acknowledge) = store.detail_with_guard(delivery_id).await.unwrap();
    assert_eq!(before.delivery.status, "failed");
    assert_eq!(before.attempts.len(), 1);
    assert_eq!(before.attempts[0].fencing_version, fence);
    assert_eq!(before.evidence[0].reference, "mailbox_sweep:9090");
    assert!(before.dispositions.iter().any(|item| item.command_id == release_id));
    assert_eq!(guard.projected_blocker(DeliveryAdminAction::RetryFailed), None);
    assert_eq!(acknowledge, None);

    sqlx::query(
        "UPDATE public.deliveries SET request_digest=decode(repeat('00',32),'hex') WHERE id=$1",
    )
    .bind(delivery_id)
    .execute(pool)
    .await
    .unwrap();
    let (detail, guard, acknowledge) =
        store.detail_with_guard(delivery_id).await.expect("仍能读取原有证据");
    assert_eq!(detail, before);
    assert_eq!(acknowledge, None);
    for action in [
        DeliveryAdminAction::Release,
        DeliveryAdminAction::RetryFailed,
        DeliveryAdminAction::ReconcileRetry,
        DeliveryAdminAction::ReconcileSucceeded,
        DeliveryAdminAction::ReconcileFailed,
    ] {
        assert_eq!(
            guard.projected_blocker(action),
            Some(DeliveryAdminGuardBlocker::CommandSnapshotUnreadable)
        );
    }
    let blocker = guard.projected_blocker(DeliveryAdminAction::RetryFailed).unwrap();
    assert_eq!(blocker.error(), DeliveryAdminError::CommandSnapshotUnreadable);
    assert_matches!(
        store.execute(command(DeliveryAdminAction::RetryFailed, fence)).await,
        Err(ApplicationError::DeliveryAdmin(error)) if error == blocker.error()
    );
    assert_eq!(store.detail(delivery_id).await.unwrap(), before);

    let reason = "核对未寄出证据，确认已处置";
    let acknowledged = store
        .acknowledge(DeliveryAcknowledgementCommand {
            command_id: Uuid::new_v4(),
            delivery_id,
            expected_fencing_version: fence,
            actor_source: DeliveryDispositionActorSource::Admin,
            actor_id: "admin".to_owned(),
            reason: reason.to_owned(),
            operator_identity: None,
            request_id: None,
            request_hash: DeliveryAcknowledgementCommand::request_hash(
                delivery_id,
                fence,
                "admin",
                reason,
            ),
        })
        .await
        .expect("完整命令快照不可读不影响确认处置");
    assert_eq!(acknowledged.kind, "operator_acknowledged");
    let (after, _, acknowledge) = store.detail_with_guard(delivery_id).await.unwrap();
    assert_eq!(after.delivery, before.delivery);
    assert_eq!(after.attempts, before.attempts);
    assert_eq!(after.evidence, before.evidence);
    assert_eq!(acknowledge, Some(DeliveryAdminGuardBlocker::AlreadyDisposed));
}

/// 缺省列表的每个来源都在旧页面闭集内，结算退件 Take 不计入总数、不占分页；扩展投影列出它，
/// 按参考编号定位也遵守同一投影。
#[tokio::test]
async fn postgres_admin_delivery_list_default_projection_omits_settlement_return_takes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let base = Timestamp::from_ymd_hms(2026, 11, 16, 0, 0, 0).expect("时间");
    let mut take_id = Uuid::nil();
    for (offset, origin) in (0_i64..).zip(STORED_ORIGINS) {
        let id = insert_pending(pool, origin, base + Duration::minutes(offset)).await;
        if origin == "settlement_return_action" {
            take_id = id;
        }
    }
    assert!(!take_id.is_nil());
    // 时间窗只框住本用例写入的行，不受模板库里其他投递影响。
    let request = |include: bool, reference: Option<Uuid>, limit: i64, offset: i64| {
        DeliveryAdminListRequest {
            reference,
            created_from: Some(base),
            created_to: Some(base + Duration::hours(1)),
            include_settlement_return_actions: include,
            limit,
            offset,
            ..DeliveryAdminListRequest::default()
        }
    };

    let default =
        DeliveryAdminPort::list(&store, request(false, None, 100, 0)).await.expect("缺省列表");
    assert_eq!(default.total, 9);
    assert_eq!(default.entries.len(), 9);
    assert!(
        default
            .entries
            .iter()
            .all(|entry| RELEASED_DECODER_ORIGINS.contains(&entry.origin_kind.as_str())),
        "{:?}",
        default.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>()
    );

    // 最新的结算退件 Take 不占缺省第一页：页内条数与总数同一口径。
    let first_page =
        DeliveryAdminPort::list(&store, request(false, None, 4, 0)).await.expect("缺省第一页");
    assert_eq!(first_page.total, 9);
    assert_eq!(
        first_page.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>(),
        ["season_reminder", "seller_summary", "lot_watch", "operations_alert"]
    );
    let last_page =
        DeliveryAdminPort::list(&store, request(false, None, 4, 8)).await.expect("缺省末页");
    assert_eq!(last_page.total, 9);
    assert_eq!(
        last_page.entries.iter().map(|entry| entry.origin_kind.as_str()).collect::<Vec<_>>(),
        ["settlement"]
    );

    let extended =
        DeliveryAdminPort::list(&store, request(true, None, 100, 0)).await.expect("扩展列表");
    assert_eq!(extended.total, 10);
    assert_eq!(extended.entries.first().map(|entry| entry.id), Some(take_id));
    assert_eq!(extended.entries[0].origin_kind, "settlement_return_action");

    let by_reference = DeliveryAdminPort::list(&store, request(false, Some(take_id), 50, 0))
        .await
        .expect("缺省投影按参考编号定位");
    assert_eq!(by_reference.total, 0);
    assert!(by_reference.entries.is_empty());
    let by_reference = DeliveryAdminPort::list(&store, request(true, Some(take_id), 50, 0))
        .await
        .expect("扩展投影按参考编号定位");
    assert_eq!(by_reference.total, 1);
    assert_eq!(by_reference.entries.first().map(|entry| entry.id), Some(take_id));

    store.postgres_pool().close().await;
}

/// 与系统退件发件人同名的用户。Admin 建用户时接受这个规范文本，并把用户名写进 `game_player_id`，
/// 于是收件人固定为该名的结算退件 Take 按收件人关联到这名用户。
async fn seed_system_sender_user(pool: &PgPool) -> Uuid {
    let user_id = Uuid::new_v4();
    support::fixtures::seed_user(pool, user_id, SYSTEM_SENDER).await;
    sqlx::query("UPDATE public.users SET game_player_id = username WHERE id = $1")
        .bind(user_id)
        .execute(pool)
        .await
        .expect("写入游戏身份");
    user_id
}

async fn player_support(
    store: &DatabaseStore,
    selector: PlayerSupportSelector,
    include_settlement_return_actions: bool,
) -> PlayerSupportBundle {
    DeliveryAdminPort::player_support(
        store,
        PlayerSupportRequest { selector, include_settlement_return_actions },
    )
    .await
    .expect("读取玩家支持汇总")
}

/// 缺省汇总的投递分区能被已发布页面按闭集解码，且不超过分区上限。
fn assert_released_decodable(bundle: &PlayerSupportBundle, case: &str) {
    let origins: Vec<&str> =
        bundle.deliveries.iter().map(|delivery| delivery.origin_kind.as_str()).collect();
    assert!(
        origins.iter().all(|origin| RELEASED_DECODER_ORIGINS.contains(origin)),
        "{case}: {origins:?}"
    );
    assert!(i64::try_from(bundle.deliveries.len()).unwrap() <= MAX_PLAYER_SUPPORT_ROWS, "{case}");
}

/// 参考编号的种类与 ID。
fn references(bundle: &PlayerSupportBundle) -> Vec<(&str, Uuid)> {
    bundle.references.iter().map(|item| (item.kind.as_str(), item.id)).collect()
}

/// 玩家支持的投递分区与列表同一投影：用户查询的缺省分区不含结算退件 Take，行数上限与截断按排除后
/// 的行计算；参考编号指向 Take 时，缺省投影下它不作为参考种类、不推导关联用户、也不补入；扩展投影
/// 照常包含，补入上限之外的 Take 时分区仍不超过上限并置截断。
#[tokio::test]
async fn postgres_player_support_default_projection_omits_settlement_return_takes() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let user_id = seed_system_sender_user(pool).await;
    let base = Timestamp::from_ymd_hms(2026, 11, 16, 0, 0, 0).expect("时间");
    // 最旧的 Take 落在上限之外，上限条普通投递居中，最新的 Take 按创建时间倒序排在第一行。
    let old_take = insert_pending(pool, "settlement_return_action", base).await;
    for minute in 1..=MAX_PLAYER_SUPPORT_ROWS {
        insert_pending_to(pool, "manual", SYSTEM_SENDER, base + Duration::minutes(minute)).await;
    }
    let new_take = insert_pending(
        pool,
        "settlement_return_action",
        base + Duration::minutes(MAX_PLAYER_SUPPORT_ROWS + 1),
    )
    .await;
    let rows = usize::try_from(MAX_PLAYER_SUPPORT_ROWS).unwrap();
    let user = || PlayerSupportSelector::User(SYSTEM_SENDER.to_owned());

    // 用户查询：缺省分区恰好是上限条普通投递，既不被 Take 占位，也不误报截断。
    let default = player_support(&store, user(), false).await;
    assert_eq!(default.user.as_ref().map(|user| user.id), Some(user_id));
    assert_eq!(default.deliveries.len(), rows);
    assert!(!default.truncated, "排除 Take 之后恰好不超过上限");
    assert_released_decodable(&default, "用户查询");
    let extended = player_support(&store, user(), true).await;
    assert_eq!(extended.deliveries.len(), rows);
    assert_eq!(extended.deliveries.first().map(|delivery| delivery.id), Some(new_take));
    assert!(extended.truncated, "扩展投影可见上限 + 2 条");

    // 参考编号：缺省投影下 Take 对玩家支持不可见，旧页面拿到的是一份空汇总。
    for take in [new_take, old_take] {
        let default = player_support(&store, PlayerSupportSelector::Reference(take), false).await;
        assert!(default.user.is_none());
        assert!(default.references.is_empty());
        assert!(default.deliveries.is_empty());
        assert_released_decodable(&default, "参考编号");
    }
    // 扩展投影：最新的 Take 已在分区第一行；上限之外的旧 Take 补到第一行，分区仍不超过上限。
    for take in [new_take, old_take] {
        let extended = player_support(&store, PlayerSupportSelector::Reference(take), true).await;
        assert_eq!(extended.user.as_ref().map(|user| user.id), Some(user_id));
        assert_eq!(references(&extended), [("delivery", take)]);
        assert_eq!(extended.deliveries.len(), rows);
        assert_eq!(extended.deliveries.first().map(|delivery| delivery.id), Some(take));
        assert_eq!(extended.deliveries.iter().filter(|delivery| delivery.id == take).count(), 1);
        assert!(extended.truncated);
    }

    store.postgres_pool().close().await;
}

/// 没有关联用户的 Take：缺省投影下参考编号查不到任何记录；扩展投影只回答参考种类，投递分区与其他
/// 无关联用户的投递一样为空。两种响应都能被已发布页面解码。
#[tokio::test]
async fn postgres_player_support_take_without_associated_user_stays_out_of_default_projection() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pool = store.postgres_pool();
    let base = Timestamp::from_ymd_hms(2026, 11, 16, 0, 0, 0).expect("时间");
    let take = insert_pending(pool, "settlement_return_action", base).await;

    let default = player_support(&store, PlayerSupportSelector::Reference(take), false).await;
    assert!(default.user.is_none());
    assert!(default.references.is_empty());
    assert!(default.deliveries.is_empty());
    assert_released_decodable(&default, "无关联用户");
    let extended = player_support(&store, PlayerSupportSelector::Reference(take), true).await;
    assert!(extended.user.is_none());
    assert_eq!(references(&extended), [("delivery", take)]);
    assert!(extended.deliveries.is_empty());

    store.postgres_pool().close().await;
}
