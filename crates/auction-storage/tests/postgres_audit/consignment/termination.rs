//! 验证强制关闭、拒绝回执与登记在审计写入失败时一起回滚。

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection as Rejection, ListingCommandStore,
};
use auction_domain::consignment::ListingMode;

use super::writes_fixture::{Fixture, SELLER, gear, new_id, register, withdraw};

#[tokio::test]
async fn postgres_forced_close_and_user_rejection_roll_back_on_commit_or_audit_error() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "456"), SELLER, ListingMode::Direct).await;
    f.lot("456", 1).await;
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_failure() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION '故意使测试事务失败'; END $$;
         CREATE CONSTRAINT TRIGGER fake_consignment_commit_failure
         AFTER INSERT ON public.consignment_events DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_failure();",
    )
    .execute(f.owner.postgres_pool())
    .await
    .unwrap();
    let cmd_id = new_id();
    let closure = withdraw(cmd_id, listing_id, SELLER, 1);
    let before = f.persisted().await;
    assert_eq!(f.store.withdraw(closure.clone()).await, Err(ConsignmentError::Unavailable));
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    sqlx::raw_sql(
        "DROP TRIGGER fake_consignment_commit_failure ON public.consignment_events;
         CREATE TRIGGER fake_consignment_audit_failure BEFORE INSERT ON public.audit_records
         FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_failure();",
    )
    .execute(f.owner.postgres_pool())
    .await
    .unwrap();
    assert_eq!(f.store.withdraw(closure.clone()).await, Err(ConsignmentError::Unavailable));
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    assert_eq!(f.command_count(cmd_id).await, 0);

    let unaffected_listing = new_id();
    let unaffected_gear = gear("main", "457");
    let fresh = || {
        register(
            new_id(),
            unaffected_listing,
            &unaffected_gear,
            &f.key,
            SELLER,
            ListingMode::Direct,
        )
    };
    assert_eq!(
        f.store.register(fresh(), Some(&f.crypto), Some(&f.crypto)).await,
        Err(ConsignmentError::Unavailable)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");
    sqlx::query("DROP TRIGGER fake_consignment_audit_failure ON public.audit_records")
        .execute(f.owner.postgres_pool())
        .await
        .unwrap();
    assert_eq!(
        f.store.withdraw(closure).await.unwrap().into_result(),
        Err(Rejection::ListingNotActive.into())
    );
    assert_eq!(f.command_count(cmd_id).await, 1);
    assert_eq!(
        f.state(listing_id).await,
        ("closed".into(), 2, Some("auction_lot_created".into()), false)
    );
    assert!(f.retired(listing_id).await);
}
