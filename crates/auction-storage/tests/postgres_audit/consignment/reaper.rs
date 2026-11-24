//! 验证回收事务的审计失败回滚和失败候选之后的推进。

use auction_application::consignment::{ListingMaintenance, LookupWorkStore};

use super::work_store_fixture::{
    Fixture, LISTING, assert_requeued_after_failure, exhausted_command,
};

#[tokio::test]
async fn postgres_consignment_work_reap_audit_failure_rolls_back() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    let fence = fixture.seed_lease(6, true).await;
    reject_reaper_audit(&fixture).await;
    let before = fixture.work_snapshot().await;
    assert_eq!(fixture.work().reap_expired().await, Ok(0));
    assert!(before == fixture.work_snapshot().await, "审计失败后所有工作行和许可都应保持原样");
    let untouched: bool = sqlx::query_scalar(
        "SELECT l.state_version = 1 AND l.state = 'resolving' AND s.retired_at IS NULL
             AND NOT EXISTS (SELECT 1 FROM public.consignment_commands WHERE command_id = $2)
             AND NOT EXISTS (SELECT 1 FROM public.consignment_events WHERE listing_id = l.id AND state_version > 1)
         FROM public.consignment_listings l JOIN public.consignment_registration_secrets s ON s.listing_id = l.id
         WHERE l.id = $1",
    ).bind(LISTING).bind(exhausted_command(fence).0.as_uuid()).fetch_one(fixture.pool()).await.unwrap();
    assert!(untouched);
    sqlx::query("DROP TRIGGER fake_reaper_audit_reject ON public.audit_records")
        .execute(fixture.owner.postgres_pool())
        .await
        .unwrap();
    assert_eq!(fixture.work().reap_expired().await.unwrap(), 1);
}

#[tokio::test]
async fn postgres_consignment_work_reap_continues_after_first_candidate_fails() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.seed_lease(6, true).await;
    fixture.expire_other_lease().await;
    reject_reaper_audit(&fixture).await;

    let work = fixture.work();
    assert_eq!(work.reap_expired().await, Ok(1), "失败的第一条不计数，第二条仍须回收");
    assert_requeued_after_failure(&fixture).await;
    assert_eq!(work.reap_expired().await, Ok(0), "下一轮首条仍会失败");
    assert_requeued_after_failure(&fixture).await;
}

#[tokio::test]
async fn postgres_consignment_work_reap_advances_past_full_page_of_failures() {
    let fixture = Fixture::new().await;
    fixture.db.sync_admission(true).await.unwrap();
    fixture.expire_other_lease().await;
    fixture.seed_exhausted_backlog(32).await;
    reject_reaper_audit(&fixture).await;

    let work = fixture.work();
    assert_eq!(work.reap_expired().await, Ok(0), "首轮预算内的 32 条耗尽任务均失败");
    assert_eq!(work.reap_expired().await, Ok(1), "下一轮从原游标继续，应回收后续任务");
    assert_requeued_after_failure(&fixture).await;
}

async fn reject_reaper_audit(fixture: &Fixture) {
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_reaper_audit_reject() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION '测试审计失败' USING ERRCODE = '23514'; END;
         $$;
         CREATE TRIGGER fake_reaper_audit_reject BEFORE INSERT ON public.audit_records
             FOR EACH ROW WHEN (NEW.action = 'consignment.apply_lookup')
             EXECUTE FUNCTION public.fake_reaper_audit_reject()",
    )
    .execute(fixture.owner.postgres_pool())
    .await
    .unwrap();
}
