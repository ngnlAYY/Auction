//! 按持久 UUID 游标补扫活动挂单，返回可在独立事务中重新核实的证据。

use auction_application::consignment::{
    AutoCloseCandidate, AutoCloseEvidence, ConsignmentError, LISTING_MAINTENANCE_SCAN_LIMIT,
    ListingMaintenancePage,
};
use auction_domain::LotId;
use auction_time::Timestamp;
use sqlx::{FromRow, QueryBuilder};
use uuid::Uuid;

use crate::database::DatabaseStore;
use crate::database::backend::set_read_only_limits;
use crate::database::consignment::eligibility::predicates::listing_sql;
use crate::database::consignment::lookup::control::lock_control;
use crate::database::consignment::reads::storage_error;

#[derive(FromRow)]
struct ScannedListing {
    listing_id: Uuid,
    reason: Option<String>,
    expires_at: Timestamp,
    service_epoch: i64,
    evidence_id: Option<Uuid>,
}

impl DatabaseStore {
    /// `ListingMaintenance::scan_candidates` 的存储入口，与启动同步共用许可单例。
    pub async fn scan_consignment_candidates(
        &self,
    ) -> Result<ListingMaintenancePage, ConsignmentError> {
        let mut tx = self.pool().begin().await.map_err(storage_error)?;
        set_read_only_limits(&mut tx).await.map_err(|(_, source)| storage_error(source))?;
        // 不变量 2、5：锁单例序列化游标推进，持锁期间只快照读挂单。
        // 不变量 6、8：候选不改变业务状态，调用方须逐单进入清退命令重新取证。
        let control = lock_control(&mut tx).await.map_err(storage_error)?;
        if !control.admitting && control.enabled_epoch == 1 && control.scan_after.is_none() {
            // 代次 1 尚未发生停用。正常登记必须在准入后写入；检查存量走活动部分索引。
            let has_stock: bool = sqlx::query_scalar(concat!(
                "SELECT EXISTS (SELECT 1 FROM public.consignment_listings l WHERE ",
                listing_sql!(stored_active),
                ")",
            ))
            .fetch_one(&mut *tx)
            .await
            .map_err(storage_error)?;
            if !has_stock {
                tx.commit().await.map_err(storage_error)?;
                return Ok(ListingMaintenancePage {
                    scanned: 0,
                    candidates: Vec::new(),
                    scan_after: None,
                });
            }
        }

        // 与 auto_close 同样按登记时冻结的 ID 集合取差集。业务时刻只用于稳定排序。
        let mut sql = QueryBuilder::new(concat!(
            "WITH db_clock AS MATERIALIZED (SELECT clock_timestamp() AS at),
             page AS MATERIALIZED (
                 SELECT l.id, l.world_id, l.external_equipment_id, l.seller_user_id,
                        l.seller_game_name, l.state, l.expires_at, l.service_epoch, l.blacklist_count_at_register,
                        l.blacklist_baseline_ids
                 FROM public.consignment_listings l WHERE ",
            listing_sql!(stored_active),
        ));
        if let Some(cursor) = control.scan_after {
            sql.push(" AND l.id > ").push_bind(cursor);
        }
        sql.push(" ORDER BY l.id LIMIT ")
            .push_bind(i64::from(LISTING_MAINTENANCE_SCAN_LIMIT))
            .push(concat!(
                "), facts AS MATERIALIZED (
                     SELECT l.*, ",
                listing_sql!(forced_reason, "db_clock.at"),
                " AS reason
                     FROM page l CROSS JOIN db_clock ",
                listing_sql!(eligibility_joins),
                "
                 )
                 SELECT l.id AS listing_id, l.reason, l.expires_at, l.service_epoch,
                        CASE
                         WHEN l.reason = 'auction_lot_created' THEN (SELECT a.id ",
                listing_sql!(unbaselined_lots),
                " ORDER BY a.id LIMIT 1)
                         WHEN l.reason = 'seller_ineligible' THEN (
                             SELECT h.id FROM public.user_blacklist_history h
                             WHERE h.user_id = COALESCE(l.seller_user_id,
                                 (SELECT u.id FROM public.users u
                                  WHERE lower(u.username) = lower(l.seller_game_name)))
                               AND NOT (h.id = ANY(l.blacklist_baseline_ids))
                             ORDER BY h.blacklisted_at, h.id LIMIT 1
                         )
                        END AS evidence_id
                 FROM facts l ORDER BY l.id",
            ));
        let rows: Vec<ScannedListing> =
            sql.build_query_as().fetch_all(&mut *tx).await.map_err(storage_error)?;
        let scanned = rows.len() as u32;
        let scan_after = if scanned == LISTING_MAINTENANCE_SCAN_LIMIT {
            rows.last().map(|row| row.listing_id)
        } else {
            None
        };
        let mut candidates = Vec::new();
        for ScannedListing { listing_id, reason, expires_at, service_epoch, evidence_id } in rows {
            let evidence = match (reason.as_deref(), evidence_id) {
                (None, _) => continue,
                (Some("auction_lot_created"), Some(lot_id)) => {
                    AutoCloseEvidence::AuctionLotCreated(LotId::from_uuid(lot_id))
                }
                (Some("seller_ineligible"), Some(history_id)) if !history_id.is_nil() => {
                    AutoCloseEvidence::SellerIneligible(history_id)
                }
                (Some("service_disabled"), _) if control.enabled_epoch > service_epoch => {
                    AutoCloseEvidence::ServiceDisabled(control.enabled_epoch.cast_unsigned())
                }
                (Some("lookup_deadline"), _) => AutoCloseEvidence::LookupDeadline(expires_at),
                (Some("display_expired"), _) => AutoCloseEvidence::DisplayExpired(expires_at),
                (Some("deposit_expired"), _) => AutoCloseEvidence::DepositExpired(expires_at),
                (Some("seller_ineligible" | "service_disabled"), _) => {
                    // 账号停用等事实没有端口要求的拉黑流水 ID。缺证据时保留有效关闭并推进游标。
                    // 异常旧数据若已拉黑却没有登记后的拉黑流水，也不生成候选，可由 Admin 下架收口。
                    tracing::warn!("寄售补扫发现无法生成证据的强制关闭");
                    continue;
                }
                _ => return Err(ConsignmentError::Unavailable),
            };
            candidates.push(AutoCloseCandidate { listing_id, evidence });
        }
        if scan_after != control.scan_after {
            sqlx::query(
                "UPDATE public.consignment_lookup_control SET scan_after = $1 WHERE id = 1",
            )
            .bind(scan_after)
            .execute(&mut *tx)
            .await
            .map_err(storage_error)?;
        }
        tx.commit().await.map_err(storage_error)?;
        Ok(ListingMaintenancePage { scanned, candidates, scan_after })
    }
}
