//! 已提交投稿的回执事实：只读拍卖场次、lot 代码与起拍价，不读取装备 key 或可变远端页面。

use auction_application::mail::{SubmissionReceiptFacts, SubmissionReceiptLot};
use auction_application::public::lot_code;
use sqlx::Row;
use uuid::Uuid;

use self::parent::*;
use super as parent;

pub(super) async fn load_submission_receipt(
    transaction: &mut PostgresCompetitionTransaction,
    submission_id: Uuid,
) -> Result<Option<SubmissionReceiptFacts>, CompetitionTransactionError> {
    if submission_id.is_nil() {
        return Err(failed("MoogleMail 回执投稿身份无效"));
    }
    let transaction = transaction.transaction_mut()?;
    let rows = sqlx::query(
        "SELECT auction.sequence, auction.status, equipment.name,
                lot.category_key, lot.category_number, lot.start_price
         FROM public.auction_submissions AS submission
         INNER JOIN public.auctions AS auction ON auction.id = submission.auction_id
         INNER JOIN public.lots AS lot ON lot.submission_id = submission.id
         INNER JOIN public.equipment AS equipment ON equipment.id = lot.equipment_id
         WHERE submission.id = $1
         ORDER BY lot.lot_number",
    )
    .bind(submission_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sql_error)?;
    let mut facts: Option<SubmissionReceiptFacts> = None;
    for row in rows {
        let session: i64 = row.try_get("sequence").map_err(map_sql_error)?;
        let status: String = row.try_get("status").map_err(map_sql_error)?;
        let item_name: String = row.try_get("name").map_err(map_sql_error)?;
        let category: String = row.try_get("category_key").map_err(map_sql_error)?;
        let category_number: i32 = row.try_get("category_number").map_err(map_sql_error)?;
        let start_price: i64 = row.try_get("start_price").map_err(map_sql_error)?;
        let category_number = u32::try_from(category_number).map_err(|error| {
            let _ = error;
            failed("MoogleMail 回执 lot 编号无效")
        })?;
        let lot = SubmissionReceiptLot {
            lot_code: lot_code(&category, category_number),
            item_name,
            start_price,
        };
        match &mut facts {
            Some(facts) => {
                if facts.session != session || facts.auction_status != status {
                    return Err(failed("MoogleMail 回执事实跨场次"));
                }
                facts.lots.push(lot);
            }
            None => {
                facts = Some(SubmissionReceiptFacts {
                    session,
                    auction_status: status,
                    lots: vec![lot],
                });
            }
        }
    }
    Ok(facts)
}
