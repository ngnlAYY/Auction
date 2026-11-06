//! PostgreSQL 管理端 lot 行到稳定摘要的映射。

use crate::database::competition::admin::map_sql_error;
use auction_application::{AdminLotSummary, CompetitionTransactionError};
use auction_domain::{AuctionId, LotId, ProxyAuthorizationId, UserId};
use serde_json::Value;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

pub(super) fn map_admin_lot_row(
    row: PgRow,
) -> Result<AdminLotSummary, CompetitionTransactionError> {
    let attributes: sqlx::types::Json<Value> =
        row.try_get("equipment_attributes").map_err(map_sql_error)?;
    Ok(AdminLotSummary {
        id: LotId::from_uuid(row.try_get("id").map_err(map_sql_error)?),
        auction_id: AuctionId::from_uuid(row.try_get("auction_id").map_err(map_sql_error)?),
        equipment_id: auction_domain::EquipmentId::from_uuid(
            row.try_get("equipment_id").map_err(map_sql_error)?,
        ),
        submission_id: row
            .try_get::<Option<Uuid>, _>("submission_id")
            .map_err(map_sql_error)?
            .map(auction_domain::SubmissionId::from_uuid),
        lot_number: row.try_get("lot_number").map_err(map_sql_error)?,
        status: row.try_get("status").map_err(map_sql_error)?,
        start_price: row.try_get("start_price").map_err(map_sql_error)?,
        current_price: row.try_get("current_price").map_err(map_sql_error)?,
        leader_user_id: row
            .try_get::<Option<Uuid>, _>("leader_user_id")
            .map_err(map_sql_error)?
            .map(UserId::from_uuid),
        effective_end_at: row.try_get("effective_end_at").map_err(map_sql_error)?,
        slow_due_at: row.try_get("slow_due_at").map_err(map_sql_error)?,
        slow_generation: row.try_get("slow_generation").map_err(map_sql_error)?,
        slow_authorization_id: row
            .try_get::<Option<Uuid>, _>("slow_authorization_id")
            .map_err(map_sql_error)?
            .map(ProxyAuthorizationId::from_uuid),
        state_version: row.try_get("state_version").map_err(map_sql_error)?,
        created_at: row.try_get("created_at").map_err(map_sql_error)?,
        updated_at: row.try_get("updated_at").map_err(map_sql_error)?,
        external_equipment_id: row.try_get("external_equipment_id").map_err(map_sql_error)?,
        equipment_name: row.try_get("equipment_name").map_err(map_sql_error)?,
        equipment_category: row.try_get("equipment_category").map_err(map_sql_error)?,
        equipment_attributes: attributes.0,
        equipment_collected_at: row.try_get("equipment_collected_at").map_err(map_sql_error)?,
        seller: row.try_get("seller").map_err(map_sql_error)?,
        submission_source_kind: row.try_get("submission_source_kind").map_err(map_sql_error)?,
        submission_source_reference: row
            .try_get("submission_source_reference")
            .map_err(map_sql_error)?,
        mailbox_key: row.try_get("mailbox_key").map_err(map_sql_error)?,
        external_mail_id: row.try_get("external_mail_id").map_err(map_sql_error)?,
        submission_received_at: row.try_get("submission_received_at").map_err(map_sql_error)?,
    })
}
