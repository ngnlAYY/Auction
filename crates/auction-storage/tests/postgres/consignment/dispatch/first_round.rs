//! 首轮：随购买同事务写入，键为 (合同, 义务, 1)；金额不符整笔回滚；授权确认与新轮前置。

use auction_application::consignment::ConsignmentError;
use auction_domain::consignment::dispatch::{RoundOrigin, dispatch_round_id};
use sqlx::PgPool;
use uuid::Uuid;

use super::fixture::{PRICE, Seeded, frozen, round_key, uuid};
use crate::consignment::contracts::fixture::{CONTRACT, OBLIGATION_1};

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM public.{table}")))
        .fetch_one(pool)
        .await
        .expect("读取行数")
}

#[tokio::test]
async fn postgres_consignment_dispatch_first_round_is_created_with_the_purchase() {
    let seeded = Seeded::purchased().await;
    let pool = seeded.pool();
    // 轮键是 (合同, 付款义务, 1)，轮 ID 由键确定性派生，不复制派生算法而是与领域函数比对。
    let (contract, obligation, round, round_id, delivery): (Uuid, Uuid, i32, Uuid, Uuid) =
        sqlx::query_as(
            "SELECT contract_id, obligation_id, round, round_id, delivery_id
             FROM public.consignment_dispatch_rounds",
        )
        .fetch_one(pool)
        .await
        .expect("恰一个首轮");
    assert_eq!((contract, obligation, round), (uuid(CONTRACT), uuid(OBLIGATION_1), 1));
    assert_eq!(round_id, dispatch_round_id(&round_key()));
    assert_eq!(delivery, seeded.delivery_id);
    // 投递停在 prepared，履约行指向它；起写之前没有任何尝试、冻结身份或在途 position。
    let status: String = sqlx::query_scalar("SELECT status FROM public.deliveries WHERE id = $1")
        .bind(delivery)
        .fetch_one(pool)
        .await
        .expect("投递状态");
    assert_eq!(status, "prepared");
    // 载荷形状（键集合，含身份摘要）与首轮冻结时一致，不随载荷构造函数的重构而变。
    let keys: Vec<String> = sqlx::query_scalar(
        "SELECT jsonb_object_keys(payload) FROM public.deliveries WHERE id = $1 ORDER BY 1",
    )
    .bind(delivery)
    .fetch_all(pool)
    .await
    .expect("载荷键");
    assert_eq!(
        keys,
        [
            "cod",
            "credits",
            "equipment_ids",
            "identity_digest",
            "operation",
            "round_ref",
            "subject",
            "template",
            "world"
        ]
    );
    // 履约行保持初始 pending 且不指向投递（合同图约束要求提交时仍是 pending）。
    let fulfillment: (String, Option<Uuid>) = sqlx::query_as(
        "SELECT execution_state, delivery_id FROM public.settlement_obligation_fulfillments
         WHERE obligation_id = $1",
    )
    .bind(uuid(OBLIGATION_1))
    .fetch_one(pool)
    .await
    .expect("履约行");
    assert_eq!(fulfillment, ("pending".to_owned(), None));
    assert_eq!(count(pool, "delivery_send_attempts").await, 0);
    assert_eq!(count(pool, "consignment_dispatch_attempts").await, 0);
}

#[tokio::test]
async fn postgres_consignment_dispatch_first_round_with_a_wrong_cod_rolls_back_the_whole_purchase()
{
    let seeded = Seeded::available().await;
    // 反例：冻结的 COD 与付款义务金额不符：失败关闭，合同、义务、轮、投递一行不留。
    let error = seeded.purchase(&frozen(PRICE + 1)).await.expect_err("金额不符必须失败");
    assert_eq!(error, ConsignmentError::InvalidDepositRequest);
    for table in ["consignment_contracts", "settlement_obligations", "consignment_dispatch_rounds"]
    {
        assert_eq!(count(seeded.pool(), table).await, 0, "{table} 必须整笔回滚");
    }
    // 夹具里只有取得用的 Take 投递，购买的 settlement 投递不得留下。
    let settlement_deliveries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM public.deliveries WHERE origin_kind = 'settlement'",
    )
    .fetch_one(seeded.pool())
    .await
    .expect("投递数");
    assert_eq!(settlement_deliveries, 0);
}

#[tokio::test]
async fn postgres_consignment_dispatch_authorize_confirms_the_purchase_round_and_gates_new_rounds()
{
    let seeded = Seeded::purchased().await;
    let service = seeded.service();
    // 正例：首轮授权确认购买事务建立的轮，不另建；同一命令重放仍是同一结果。
    let receipt = service
        .authorize(round_key(), RoundOrigin::First, seeded.delivery_id, frozen(PRICE))
        .await
        .expect("确认首轮");
    assert!(receipt.replayed);
    assert_eq!(receipt.delivery_id, seeded.delivery_id);
    let again = service
        .authorize(round_key(), RoundOrigin::First, seeded.delivery_id, frozen(PRICE))
        .await
        .expect("重放授权");
    assert_eq!(again.round_id, receipt.round_id);
    // 反例：同一轮键、被改动的冻结身份（同命令 ID、异摘要）冲突，且不改写已存的轮。
    let mut changed = frozen(PRICE);
    changed.subject = "another subject".to_owned();
    let conflict = service
        .authorize(round_key(), RoundOrigin::First, seeded.delivery_id, changed)
        .await
        .expect_err("异请求冲突");
    assert_eq!(conflict, ConsignmentError::CommandIdConflict);
    // 反例：新轮没有封闭上一轮的退件接管与消费，不得无依据重发。
    let mut second = round_key();
    second.round = 2;
    let mut next = frozen(PRICE);
    next.round_ref = auction_domain::consignment::dispatch::dispatch_ref(&second);
    let denied = service
        .authorize(
            second,
            RoundOrigin::Redispatch { closed_by_return_intake: Uuid::new_v4() },
            Uuid::new_v4(),
            next,
        )
        .await
        .expect_err("无依据重发必须拒绝");
    assert!(matches!(denied, ConsignmentError::Deposit(_)), "{denied:?}");
    assert_eq!(count(seeded.pool(), "consignment_dispatch_rounds").await, 1);
}
