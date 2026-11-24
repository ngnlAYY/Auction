//! v2 多挂单命令的审计失败必须回滚业务事实、事件与回执。

use auction_application::consignment::{ConsignmentError, EnvelopeStore, ReceiptDisposition};
use auction_domain::CommandId;
use auction_storage::{
    test_deposit_audit_count, test_deposit_command_exists, test_deposit_event_count,
    test_deposit_listing_state,
};
use sqlx::AssertSqlSafe;
use uuid::Uuid;

use super::deposit_fixture::{accepted_command, repository, seed};
use crate::support::TestDatabase;

const FIRST: Uuid = Uuid::from_u128(0x7a00_0000_0000_0000_0000_0000_0000_0001);
const SECOND: Uuid = Uuid::from_u128(0x7a00_0000_0000_0000_0000_0000_0000_0002);
const COMMAND: Uuid = Uuid::from_u128(0x7b00_0000_0000_5000_8000_0000_0000_0001);

fn seed_sql() -> String {
    format!(
        "INSERT INTO public.users (id, username, public_key_hash) VALUES
            ('10000000-0000-0000-0000-000000000001', 'FakeSellerOne',
             decode(repeat('11', 32), 'hex')),
            ('10000000-0000-0000-0000-000000000002', 'FakeSellerTwo',
             decode(repeat('12', 32), 'hex'));
         INSERT INTO public.consignment_replay_keys
            (id, write_current, wrapped_key, nonce, encryption_key_id, created_at, updated_at)
         VALUES ('90000000-0000-0000-0000-000000000001', true,
                 decode(repeat('13', 48), 'hex'), decode(repeat('14', 12), 'hex'),
                 'fake-data-key', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
         {first}
         {second}",
        first = listing_sql(
            FIRST,
            Uuid::from_u128(0x7a01),
            "10000000-0000-0000-0000-000000000001",
            "42",
            "7b010000-0000-4000-8000-000000000001",
            "7b010000-0000-5000-8000-000000000002",
        ),
        second = listing_sql(
            SECOND,
            Uuid::from_u128(0x7a02),
            "10000000-0000-0000-0000-000000000002",
            "43",
            "7b020000-0000-4000-8000-000000000003",
            "7b020000-0000-5000-8000-000000000004",
        ),
    )
}

fn listing_sql(
    listing: Uuid,
    snapshot: Uuid,
    seller: &str,
    eid: &str,
    register: &str,
    publish: &str,
) -> String {
    format!(
        "INSERT INTO public.consignment_listings
            (id, seller_user_id, world_id, external_equipment_id, mode, state, price,
             pricing_version, fee_bps, state_version, service_epoch, blacklist_count_at_register,
             blacklist_baseline_ids, created_at, updated_at, expires_at)
         VALUES ('{listing}', '{seller}', 'main', '{eid}', 'consigned', 'resolving',
                 101, 1, 500, 1, 1, 0, ARRAY[]::uuid[],
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z');
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{register}', 'public_user', '{seller}', 'register', 'new',
                 decode(repeat('18', 32), 'hex'), '90000000-0000-0000-0000-000000000001',
                 '{listing}', 202, 'accepted',
                 '{{\"command_id\":\"{register}\",\"operation\":\"register\",
                    \"listing_id\":\"{listing}\",\"listing_version\":1,
                    \"state\":\"resolving\",\"disposition\":\"registered\",\"price\":101}}',
                 '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing}', 1, '{register}', 'public_user', 'registered', NULL,
                 'resolving', NULL, 101, '2026-01-01T00:00:00Z');
         INSERT INTO public.consignment_snapshots
            (id, listing_id, generation, adapter, observed_at, digest, world_id,
             external_equipment_id, name, category, alt_name, level_kind, level_number,
             tradeable, owner_name, stat_groups)
         VALUES ('{snapshot}', '{listing}', 1, 'hvdata-equip-v1', '2026-01-01T01:00:00Z',
                 decode(repeat('15', 32), 'hex'), 'main', '{eid}', 'Fake Gear {eid}', 'Weapon',
                 NULL, 'assigned', 100, true, 'FakeSeller', '[]'::jsonb);
         INSERT INTO public.consignment_commands
            (command_id, actor_kind, actor_ref, operation, request_target, request_digest,
             replay_key_id, listing_id, result_status, result_code, result_body, created_at)
         VALUES ('{publish}', 'worker', 'fake-worker', 'apply_lookup', '{listing}',
                 decode(repeat('16', 32), 'hex'), NULL, '{listing}', 200, 'ok',
                 '{{\"command_id\":\"{publish}\",\"operation\":\"apply_lookup\",
                    \"listing_id\":\"{listing}\",\"listing_version\":2,
                    \"state\":\"awaiting_deposit\",\"disposition\":\"published\",\"price\":101,
                    \"expires_at\":\"2026-01-02T00:00:00Z\",\"reason_code\":null}}',
                 '2026-01-01T01:00:00Z');
         INSERT INTO public.consignment_events
            (listing_id, state_version, command_id, actor_kind, event_kind, from_state,
             to_state, reason_code, price, occurred_at)
         VALUES ('{listing}', 2, '{publish}', 'worker', 'published', 'resolving',
                 'awaiting_deposit', NULL, 101, '2026-01-01T01:00:00Z');
         UPDATE public.consignment_listings
         SET state='awaiting_deposit', state_version=2, current_snapshot_id='{snapshot}',
             updated_at='2026-01-01T01:00:00Z'
         WHERE id='{listing}';"
    )
}

#[tokio::test]
async fn postgres_deposit_audit_failure_rolls_back_multi_listing_command() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("连接维护账号");
    let store = database.store().await.expect("连接运行账号");
    sqlx::raw_sql(AssertSqlSafe(seed_sql())).execute(owner.postgres_pool()).await.expect("夹具");
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_deposit_audit_reject() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION '测试审计失败' USING ERRCODE = '23514'; END;
         $$;
         CREATE TRIGGER fake_deposit_audit_reject BEFORE INSERT ON public.audit_records
             FOR EACH ROW WHEN (NEW.action LIKE 'consignment.%')
             EXECUTE FUNCTION public.fake_deposit_audit_reject();",
    )
    .execute(owner.postgres_pool())
    .await
    .expect("安装审计失败触发器");

    store
        .test_deposit_advance_two_listings(COMMAND, FIRST, SECOND, 2, 2)
        .await
        .expect_err("审计失败必须整笔回滚");
    assert_eq!(
        test_deposit_listing_state(&store, FIRST).await.expect("第一件"),
        ("awaiting_deposit".into(), 2)
    );
    assert_eq!(
        test_deposit_listing_state(&store, SECOND).await.expect("第二件"),
        ("awaiting_deposit".into(), 2)
    );
    assert_eq!(test_deposit_event_count(&store, COMMAND).await.expect("事件数"), 0);
    assert!(!test_deposit_command_exists(&store, COMMAND).await);

    sqlx::query("DROP TRIGGER fake_deposit_audit_reject ON public.audit_records")
        .execute(owner.postgres_pool())
        .await
        .expect("移除审计失败触发器");
    store
        .test_deposit_advance_two_listings(COMMAND, FIRST, SECOND, 2, 2)
        .await
        .expect("恢复后同一命令应提交");
    assert_eq!(
        test_deposit_listing_state(&store, FIRST).await.expect("第一件"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(
        test_deposit_listing_state(&store, SECOND).await.expect("第二件"),
        ("deposit_accepted".into(), 3)
    );
    assert_eq!(test_deposit_event_count(&store, COMMAND).await.expect("事件数"), 2);
    assert!(test_deposit_command_exists(&store, COMMAND).await);
}

async fn fact_counts(pool: &sqlx::PgPool, command: Uuid) -> (i64, i64, i64, i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
            (SELECT count(*) FROM public.consignment_commands WHERE command_id=$1),
            (SELECT count(*) FROM public.consignment_events WHERE command_id=$1),
            (SELECT count(*) FROM public.mail_initial_decisions WHERE command_id=$1),
            (SELECT count(*) FROM public.mail_final_dispositions WHERE command_id=$1),
            (SELECT count(*) FROM public.consignment_deposit_items),
            (SELECT count(*) FROM public.consignment_custody_positions),
            (SELECT count(*) FROM public.mail_disposition_work)",
    )
    .bind(command)
    .fetch_one(pool)
    .await
    .expect("读取整封事实数")
}

#[tokio::test]
async fn postgres_deposit_audit_fixture_commits_without_trigger() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("连接维护账号");
    let reservation_expires_at = seed(owner.postgres_pool()).await;
    let runtime = database.store().await.expect("连接运行账号");
    let store = repository(runtime.clone());
    let command_id =
        CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"audit-deposit-control"));

    let receipt = store
        .decide_envelope(accepted_command(command_id, reservation_expires_at))
        .await
        .expect("无触发器时同一夹具第一次就必须提交");
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    assert_eq!(
        fact_counts(owner.postgres_pool(), command_id.as_uuid()).await,
        (1, 1, 1, 1, 1, 1, 1)
    );
    assert_eq!(test_deposit_audit_count(&runtime, command_id.as_uuid()).await.expect("审计数"), 1);
}

#[tokio::test]
async fn postgres_deposit_audit_failure_rolls_back_all_facts() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("连接维护账号");
    let reservation_expires_at = seed(owner.postgres_pool()).await;
    let runtime = database.store().await.expect("连接运行账号");
    let store = repository(runtime.clone());
    let command_id =
        CommandId::from_uuid(Uuid::new_v5(&Uuid::NAMESPACE_URL, b"audit-deposit-decide"));
    let observation_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_envelope_observations")
            .fetch_one(owner.postgres_pool())
            .await
            .expect("读取初始观察数");
    sqlx::raw_sql(AssertSqlSafe(
        "CREATE FUNCTION public.fake_deposit_envelope_audit_reject() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION '测试审计失败' USING ERRCODE = '23514'; END;
         $$;
         CREATE TRIGGER fake_deposit_envelope_audit_reject BEFORE INSERT ON public.audit_records
             FOR EACH ROW WHEN (NEW.action LIKE 'consignment.%')
             EXECUTE FUNCTION public.fake_deposit_envelope_audit_reject();"
            .to_owned(),
    ))
    .execute(owner.postgres_pool())
    .await
    .expect("安装审计失败触发器");

    let error = store
        .decide_envelope(accepted_command(command_id, reservation_expires_at))
        .await
        .expect_err("审计失败必须整封回滚");
    // 审计插入的 23514 在 WriteError 中归为 Invariant，端口只暴露 Unavailable；同一夹具的
    // 无触发器对照用例已证明可首次提交，因此这里的不可用来自审计失败而非夹具前提。
    assert_eq!(error, ConsignmentError::Unavailable, "存储层把审计写入失败映射为不可用");
    assert_eq!(
        fact_counts(owner.postgres_pool(), command_id.as_uuid()).await,
        (0, 0, 0, 0, 0, 0, 0),
        "命令、事件、观察、决定、工作均不得留下"
    );
    assert_eq!(test_deposit_audit_count(&runtime, command_id.as_uuid()).await.expect("审计数"), 0);
    let delivery: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind='consignment_disposition'",
    )
    .fetch_one(owner.postgres_pool())
    .await
    .expect("读取投递数");
    assert_eq!(delivery, 0);
    let observation_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_envelope_observations")
            .fetch_one(owner.postgres_pool())
            .await
            .expect("读取失败后观察数");
    assert_eq!(observation_after, observation_before, "决定失败不得追加观察");

    sqlx::query("DROP TRIGGER fake_deposit_envelope_audit_reject ON public.audit_records")
        .execute(owner.postgres_pool())
        .await
        .expect("移除审计失败触发器");
    let receipt = store
        .decide_envelope(accepted_command(command_id, reservation_expires_at))
        .await
        .expect("审计恢复后同一命令应成功");
    assert_eq!(receipt.disposition, ReceiptDisposition::Accepted);
    let committed = fact_counts(owner.postgres_pool(), command_id.as_uuid()).await;
    assert_eq!(committed, (1, 1, 1, 1, 1, 1, 1));
    assert_eq!(test_deposit_audit_count(&runtime, command_id.as_uuid()).await.expect("审计数"), 1);
    let replay = store
        .decide_envelope(accepted_command(command_id, reservation_expires_at))
        .await
        .expect("同命令重放");
    assert_eq!(replay, receipt);
    assert_eq!(
        fact_counts(owner.postgres_pool(), command_id.as_uuid()).await,
        committed,
        "重放不得追加第二份事实"
    );
    let observations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.mail_envelope_observations")
            .fetch_one(owner.postgres_pool())
            .await
            .expect("读取成功后观察数");
    assert_eq!(observations, observation_before);
}
