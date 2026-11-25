//! 灭失确认测试共用的夹具：管理员命令、证据追加、买方侧事实快照与占用读数。
//!
//! 自 `mod.rs` 析出，使 `postgres_audit` 目标能以符号链接复用而不带入本目录的测试模块。

use auction_application::consignment::{
    ConfirmPositionLossCommand, ConsignmentError, DepositBusinessRejection, ListingDelistReason,
    LossLastKnownRef, PositionLossStore, RecordLossProofCommand,
};
use auction_application::core::RequestHash;
use auction_domain::CommandId;
use uuid::Uuid;

use super::super::super::deposit::support::POSITION;
use super::super::sale_recovery::{World, id};

/// 管理员命令（UUIDv4）。
pub(in crate::consignment::recovery) fn admin_command_id(seed: u128) -> CommandId {
    CommandId::from_uuid(Uuid::from_u128(0x7c4b_0000_0000_4000_8000_0000_0000_0000 + seed))
}

/// 录入证据的管理员命令（UUIDv4，与确认命令错开种子段）。
fn proof_command_id(seed: u128) -> CommandId {
    admin_command_id(0x5000 + seed)
}

pub(in crate::consignment::recovery) fn confirm(
    seed: u128,
    expected_version: u64,
    proof_id: Uuid,
    hash: u8,
) -> ConfirmPositionLossCommand {
    ConfirmPositionLossCommand {
        admin_actor: "fake-admin".to_owned(),
        command_id: admin_command_id(seed),
        position_id: id(POSITION),
        expected_version,
        proof_id,
        reason: ListingDelistReason::new("fake loss reason".to_owned()).expect("夹具说明"),
        request_hash: RequestHash::new([hash; 32]),
        request_id: None,
    }
}

/// 经真实端口追加一条完整的灭失证据，返回证据 ID。
pub(in crate::consignment::recovery) async fn record_proof(world: &World, seed: u128) -> Uuid {
    world
        .deposit
        .record_loss_proof(RecordLossProofCommand {
            admin_actor: "fake-admin".to_owned(),
            command_id: proof_command_id(seed),
            position_id: id(POSITION),
            source_kind: "fake-source".to_owned(),
            observation_ref: format!("fake-observation-{seed}"),
            last_known_ref: LossLastKnownRef::new("fake-last-known".to_owned()).expect("夹具位置"),
            private_note: ListingDelistReason::new("fake proof note".to_owned()).expect("夹具说明"),
            request_hash: RequestHash::new([0x51; 32]),
            request_id: None,
        })
        .await
        .expect("追加灭失证据")
        .proof_id
}

pub(in crate::consignment::recovery) fn rejection(
    error: ConsignmentError,
) -> DepositBusinessRejection {
    match error {
        ConsignmentError::Deposit(rejection) => rejection,
        other => panic!("期望确定业务拒绝，实际 {other:?}"),
    }
}

/// 买方一侧与资金的不可变事实表：行数与全表内容摘要。灭失确认不得改动其中任何一行。
const BUYER_SIDE_TABLES: [&str; 13] = [
    "consignment_contracts",
    "consignment_sales",
    "consignment_sale_closures",
    "consignment_funded_sales",
    "consignment_cod_receipts",
    "consignment_funding_returns",
    "consignment_fee_facts",
    "consignment_principal_facts",
    "consignment_dispatch_rounds",
    "consignment_return_consumptions",
    "settlement_obligations",
    "deliveries",
    "consignment_disposition_generations",
];

pub(in crate::consignment::recovery) async fn buyer_side_snapshot(
    world: &World,
) -> Vec<(String, i64, String)> {
    let mut snapshot = Vec::new();
    for table in BUYER_SIDE_TABLES {
        let (rows, digest): (i64, String) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT count(*), coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '')
             FROM public.{table} AS t"
        )))
        .fetch_one(&world.pool)
        .await
        .expect("读取买方侧快照");
        snapshot.push((table.to_owned(), rows, digest));
    }
    snapshot
}

/// 已有命令回执的内容摘要（确认之前就存在的命令行）。
pub(in crate::consignment::recovery) async fn prior_receipts(
    world: &World,
    command_ids: &[Uuid],
) -> String {
    sqlx::query_scalar(
        "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t.command_id)), '')
         FROM public.consignment_commands AS t WHERE t.command_id = ANY($1)",
    )
    .bind(command_ids)
    .fetch_one(&world.pool)
    .await
    .expect("读取旧回执")
}

pub(in crate::consignment::recovery) async fn all_command_ids(world: &World) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT command_id FROM public.consignment_commands")
        .fetch_all(&world.pool)
        .await
        .expect("读取命令账本")
}

/// 占用该件的实物强占用是否已释放。
pub(in crate::consignment::recovery) async fn claim_released(world: &World) -> bool {
    let claimed: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT reservation_id, position_id FROM public.consignment_equipment_claims",
    )
    .fetch_all(&world.pool)
    .await
    .expect("读取占用");
    claimed.iter().all(|(reservation, position)| reservation.is_none() && position.is_none())
}

pub(in crate::consignment::recovery) async fn loss_rows(world: &World) -> [i64; 5] {
    let mut counts = [0; 5];
    for (slot, table) in counts.iter_mut().zip([
        "consignment_loss_facts",
        "consignment_loss_cases",
        "consignment_loss_case_outcomes",
        "consignment_loss_receipts",
        "consignment_right_notice_outbox",
    ]) {
        *slot = world.count(table).await;
    }
    counts
}
