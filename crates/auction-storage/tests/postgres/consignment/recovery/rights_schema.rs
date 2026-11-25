//! 受限权利：定位符摘要全局唯一、权利必须指向合同或 position；权利 ID 由 UUIDv5 确定性派生；
//! 136 到 137 升级时既有寄售与拍卖行逐字节不变，并按 v5(contract, role) 回填买卖双方的权利（R5-C6）。

use auction_storage::postgres_migrate_to;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::contracts::fixture::CONTRACT;
use super::super::contracts::upgrade::seed_legacy_settlement;
use super::super::deposit::support::{USER_1, USER_2, fails, succeeds};
use super::{RIGHT, T0, command_ids, digest, right_sql, seed};
use crate::support::TestDatabase;

/// 独立实现：UUIDv5(NAMESPACE_URL, 域前缀 ‖ 合同 ID 原字节 ‖ 角色)，用 uuid crate 而不是复制库内 SQL。
fn expected_right_id(contract: &str, role: &str) -> String {
    let contract = Uuid::parse_str(contract).expect("合同 ID");
    let mut name = b"auction.consignment.right.v1\0".to_vec();
    name.extend_from_slice(contract.as_bytes());
    name.extend_from_slice(role.as_bytes());
    Uuid::new_v5(&Uuid::NAMESPACE_URL, &name).to_string()
}

#[tokio::test]
async fn postgres_consignment_right_locator_digest_is_unique_and_target_is_required() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    seed(pool).await;
    let command = command_ids(pool, 1).await.remove(0);

    succeeds(pool, &right_sql(RIGHT, "c1", "continue_order", &command)).await;
    // 反例：同一定位符摘要不能指向第二个权利（换了种类以排除身份键的干扰）。
    fails(
        pool,
        &right_sql("7c100000-0000-4000-8000-000000000002", "c1", "claim_payment", &command),
        "23505",
        Some("c7_right_locator_uq"),
    )
    .await;
    // 正例：不同摘要的第二个权利可登记。
    succeeds(
        pool,
        &right_sql("7c100000-0000-4000-8000-000000000003", "c2", "claim_payment", &command),
    )
    .await;
    // 反例：既无合同又无 position 的权利被拒绝。
    fails(
        pool,
        &format!(
            "INSERT INTO public.consignment_rights
                (right_id, locator_digest, beneficiary_user_id, frozen_identity, world_id, kind,
                 contract_id, position_id, created_command_id, created_at)
             VALUES ('7c100000-0000-4000-8000-000000000004', {d}, '{USER_2}', 'FakeSellerTwo',
                     'main', 'loss_case', NULL, NULL, '{command}', '{T0}');",
            d = digest("c3")
        ),
        "23514",
        Some("c7_right_target_ck"),
    )
    .await;
}

#[tokio::test]
async fn postgres_consignment_sha1_matches_reference_vectors_across_block_boundaries() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    // 向量来自 Python hashlib：覆盖空串、填充边界（55/56 字节）、整块（64）与多块（200）。
    let vectors = [
        (0, "da39a3ee5e6b4b0d3255bfef95601890afd80709"),
        (55, "c1c8bbdc22796e28c0e15163d20899b65621d65a"),
        (56, "c2db330f6083854c99d4b5bfb6e8f29f201be699"),
        (63, "03f09f5b158a7a8cdad920bddc29b81c18a551f5"),
        (64, "0098ba824b5c16427bd7a1122a5a442a25ec644d"),
        (200, "e61cfffe0d9195a525fc6cf06ca2d77119c24a40"),
    ];
    for (length, expected) in vectors {
        let actual: String = sqlx::query_scalar(
            "SELECT encode(public.c7_sha1(convert_to(repeat('a', $1), 'UTF8')), 'hex')",
        )
        .bind(length)
        .fetch_one(pool)
        .await
        .expect("计算 SHA-1");
        assert_eq!(actual, expected, "长度 {length}");
    }
    // 权利 ID 与 uuid crate 的独立实现一致；未知角色失败关闭。
    for role in ["buyer", "seller"] {
        let actual: String = sqlx::query_scalar("SELECT public.c7_right_id($1::uuid, $2)::text")
            .bind(CONTRACT)
            .bind(role)
            .fetch_one(pool)
            .await
            .expect("派生权利 ID");
        assert_eq!(actual, expected_right_id(CONTRACT, role), "{role}");
    }
    let unknown =
        sqlx::query_scalar::<_, String>("SELECT public.c7_right_id($1::uuid, 'admin')::text")
            .bind(CONTRACT)
            .fetch_one(pool)
            .await;
    assert!(unknown.is_err(), "未知角色必须失败关闭");
}

/// 回填权利的可比较形状：ID、种类、受益人、冻结身份、是否沿用合同来源命令、状态版本与动作范围。
#[derive(Debug, PartialEq, sqlx::FromRow)]
struct BackfilledRight {
    right_id: String,
    kind: String,
    beneficiary: String,
    frozen_identity: String,
    from_contract_command: bool,
    version: i64,
    allowed_actions: Vec<String>,
}

/// 既有寄售与拍卖行的整行文本（升级前后逐字节比较）：合同、Sale、position、处分代、代管事件、挂单事件、交接边、
/// 命令、挂单、义务、履约与投递（共享替换段改写了处分代、事件与交接边的约束，既有行须原样保留）。
async fn existing_rows(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT concat_ws('|',
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY contract_id) FROM public.consignment_contracts t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY contract_id) FROM public.consignment_sales t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM public.consignment_custody_positions t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY position_id, generation)
             FROM public.consignment_disposition_generations t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY position_id, version)
             FROM public.consignment_custody_events t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY listing_id, state_version)
             FROM public.consignment_events t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM public.consignment_equipment_handoffs t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY command_id) FROM public.consignment_commands t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM public.consignment_listings t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM public.settlement_obligations t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY obligation_id)
             FROM public.settlement_obligation_fulfillments t)::text,
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM public.deliveries t)::text)",
    )
    .fetch_one(pool)
    .await
    .expect("读取既有行")
}

#[tokio::test]
async fn postgres_consignment_recovery_upgrade_from_136_keeps_rows_and_backfills_rights() {
    let database = TestDatabase::create_empty().await.expect("创建空库");
    let owner = database.maintenance_store().await.expect("以维护账号连接");
    let pool = owner.postgres_pool();
    postgres_migrate_to(pool, 136).await.expect("迁移到 136");
    seed_legacy_settlement(pool).await;
    seed(pool).await;
    let before = existing_rows(pool).await;

    postgres_migrate_to(pool, 137).await.expect("迁移到 137");

    assert_eq!(existing_rows(pool).await, before, "既有寄售与拍卖行必须逐字节不变");
    // 回填：Sale 的买家与卖家各一个权利，ID 为 v5(contract, role)，版本 1 active 且动作范围为空（失败关闭）。
    let rights: Vec<BackfilledRight> = sqlx::query_as(
        "SELECT r.right_id::text AS right_id, r.kind, r.beneficiary_user_id::text AS beneficiary,
                r.frozen_identity, r.created_command_id = c.origin_command_id AS from_contract_command,
                s.version, s.allowed_actions
         FROM public.consignment_rights AS r
         JOIN public.consignment_contracts AS c ON c.contract_id = r.contract_id
         JOIN public.consignment_right_states AS s ON s.right_id = r.right_id AND s.state = 'active'
         ORDER BY r.kind",
    )
    .fetch_all(pool)
    .await
    .expect("读取回填权利");
    let expected = |role: &str, kind: &str, beneficiary: &str, identity: &str| BackfilledRight {
        right_id: expected_right_id(CONTRACT, role),
        kind: kind.to_owned(),
        beneficiary: beneficiary.to_owned(),
        frozen_identity: identity.to_owned(),
        from_contract_command: true,
        version: 1,
        allowed_actions: vec![],
    };
    assert_eq!(
        rights,
        vec![
            expected("seller", "claim_payment", USER_1, "FakeSellerOne"),
            expected("buyer", "continue_order", USER_2, "FakeSellerTwo"),
        ]
    );
    let version: i64 = sqlx::query_scalar("SELECT max(version) FROM public._sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("读取迁移版本");
    assert_eq!(version, 137);
}
