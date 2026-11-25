//! committed -> in_custody 回库边：Sale 已封闭、原件从未离库（首派从未起写）时放行；没有封闭事实，或原件已发出
//! 而未被精确收回时，数据库守卫拒绝。
//!
//! 保护的不变量：2、4、6（原件可能在途时不放行，失败关闭）。

use super::super::super::contracts::fixture::CONTRACT;
use super::super::super::deposit::support::{LISTING, POSITION, fails, succeeds};
use super::super::super::ledger136::fixture::original_dispatch_sql;
use super::{Closure, HOLD, World, closure_sql, database_time};

/// 回库的两条语句：事件先于 position 更新，版本 4；`at` 不得早于真实写端留下的 updated_at。
fn recall_sql(at: &str) -> String {
    format!(
        "INSERT INTO public.consignment_custody_events
            (position_id, version, command_id, event_kind, from_state, to_state, proof_id,
             listing_id, listing_version, occurred_at)
         VALUES ('{POSITION}', 4, '{HOLD}', 'uncommitted', 'committed', 'in_custody', '{CONTRACT}',
                 '{LISTING}', 6, '{at}');
         UPDATE public.consignment_custody_positions
         SET state = 'in_custody', version = 4, updated_at = '{at}'
         WHERE id = '{POSITION}';"
    )
}

#[tokio::test]
async fn postgres_recall_edge_needs_a_sale_closure_and_an_original_in_custody() {
    // 反例：没有封闭事实。
    let open = World::sold(Closure::None).await;
    let at = database_time(&open.owner).await;
    fails(&open.owner, &recall_sql(&at), "23514", Some("c7_position_recall_fact")).await;
    assert_eq!(open.position().await.0, "committed");

    // 反例：封闭事实在，但原件腿已有 Send 尝试且没有精确 Take 消费。
    let sent = World::sold(Closure::None).await;
    let at = database_time(&sent.owner).await;
    succeeds(&sent.owner, &original_dispatch_sql()).await;
    succeeds(&sent.owner, &closure_sql("cancelled_unpaid")).await;
    fails(&sent.owner, &recall_sql(&at), "23514", Some("c7_position_recall_fact")).await;
    assert_eq!(sent.position().await.0, "committed");

    // 正例：封闭事实在，首派从未起写。
    let closed = World::sold(Closure::CancelledUnpaid).await;
    let at = database_time(&closed.owner).await;
    succeeds(&closed.owner, &recall_sql(&at)).await;
    assert_eq!(closed.position().await, ("in_custody".to_owned(), Some(1), 4));
    // 反例：已在库的 position 不能再次回库。
    fails(&closed.owner, &recall_sql(&at), "23505", None).await;
}
