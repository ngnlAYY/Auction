//! 本人改价、取消与 Admin 下架的回执、归属及竞争裁定。

use auction_application::consignment::{
    ConsignmentError, ListingBusinessRejection as Rejection, ListingCommandOutcome,
    ListingCommandStore, ListingPriceDisposition, StoredListingDecision,
};
use auction_domain::consignment::ListingMode;

use super::fixture::{
    Fixture, OTHER, SELLER, delist, gear, new_id, price, register, reprice, withdraw,
};

#[tokio::test]
async fn postgres_reprice_same_price_no_event_and_parallel_versions() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "431"), SELLER, ListingMode::Direct).await;
    let deadline = f.deadline(listing_id).await;
    let same = reprice(new_id(), listing_id, SELLER, 1, 101);
    assert_eq!(
        f.store.reprice(same).await.unwrap().into_result().unwrap().disposition,
        ListingPriceDisposition::Unchanged
    );
    assert_eq!(f.state(listing_id).await.1, 1);
    assert_eq!(f.deadline(listing_id).await, deadline);
    let a = reprice(new_id(), listing_id, SELLER, 1, 200);
    let b = reprice(new_id(), listing_id, SELLER, 1, 300);
    let blocker = f.block_control().await;
    let (a_result, b_result, ()) = tokio::join!(
        f.store.reprice(a.clone()),
        f.store.reprice(b.clone()),
        f.release_when_waiting(blocker, 2),
    );
    let a_result = a_result.unwrap().into_result();
    let b_result = b_result.unwrap().into_result();
    let (winner, receipt, loser) = match (a_result, b_result) {
        (Ok(receipt), Err(ConsignmentError::Business(Rejection::StaleVersion))) => (a, receipt, b),
        (Err(ConsignmentError::Business(Rejection::StaleVersion)), Ok(receipt)) => (b, receipt, a),
        other => panic!("需要一条改价与一个旧版本拒绝：{other:?}"),
    };
    assert_eq!(receipt.price, winner.price);
    assert_eq!(receipt.version.value(), 2);
    assert_eq!(f.deadline(listing_id).await, deadline);
    let mut replay = winner.clone();
    replay.enabled = false;
    assert_eq!(
        f.store.reprice(replay).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt))
    );
    assert_eq!(
        f.store.reprice(loser).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(Rejection::StaleVersion))
    );
    let different = reprice(winner.target.command_id.as_uuid(), listing_id, SELLER, 1, 500);
    assert_eq!(
        f.store.reprice(different).await.unwrap().into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );
    let event_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM public.consignment_events WHERE listing_id=$1")
            .bind(listing_id)
            .fetch_one(f.pool())
            .await
            .unwrap();
    assert_eq!(event_count, 2);
    let persisted_price: i64 =
        sqlx::query_scalar("SELECT price FROM public.consignment_listings WHERE id=$1")
            .bind(listing_id)
            .fetch_one(f.pool())
            .await
            .unwrap();
    assert_eq!(price(persisted_price), receipt.price);
}

#[tokio::test]
async fn postgres_reprice_same_command_across_two_targets_actors_and_operations() {
    let f = Fixture::new().await;
    let a_id = new_id();
    let b_id = new_id();
    f.registered(a_id, &gear("main", "432"), SELLER, ListingMode::Direct).await;
    f.registered(b_id, &gear("main", "433"), OTHER, ListingMode::Direct).await;
    let duplicate_id = new_id();
    let blocker = f.block_control().await;
    let (a, b, ()) = tokio::join!(
        f.store.reprice(reprice(duplicate_id, a_id, SELLER, 1, 201)),
        f.store.reprice(reprice(duplicate_id, b_id, OTHER, 1, 202)),
        f.release_when_waiting(blocker, 2),
    );
    let (a, b) = (a.unwrap().into_result(), b.unwrap().into_result());
    assert!(matches!(
        (&a, &b),
        (Ok(_), Err(ConsignmentError::CommandIdConflict))
            | (Err(ConsignmentError::CommandIdConflict), Ok(_))
    ));
    assert_eq!(f.state(a_id).await.1 + f.state(b_id).await.1, 3);
    assert_eq!(f.command_count(duplicate_id).await, 1);
    let accepted = a.or(b).unwrap();
    assert_eq!(
        f.store
            .withdraw(withdraw(duplicate_id, accepted.listing_id(), SELLER, 1))
            .await
            .unwrap()
            .into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );
}

#[tokio::test]
async fn postgres_withdraw_and_admin_delist_preserve_first_terminal_fact_and_replay() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "434"), SELLER, ListingMode::Direct).await;
    let cancel = withdraw(new_id(), listing_id, SELLER, 1);
    let admin = delist(new_id(), listing_id, 1, "测试运营说明");
    let blocker = f.block_control().await;
    let (seller, operator, ()) = tokio::join!(
        f.store.withdraw(cancel.clone()),
        f.store.admin_delist(admin.clone()),
        f.release_when_waiting(blocker, 2),
    );
    let (seller, operator) = (seller.unwrap().into_result(), operator.unwrap().into_result());
    let reason = match (seller, operator) {
        (Ok(receipt), Err(ConsignmentError::Business(Rejection::StaleVersion))) => {
            assert_eq!(
                f.store.withdraw(cancel.clone()).await.unwrap(),
                ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt))
            );
            "seller_cancelled"
        }
        (Err(ConsignmentError::Business(Rejection::StaleVersion)), Ok(receipt)) => {
            assert_eq!(
                f.store.admin_delist(admin.clone()).await.unwrap(),
                ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(receipt))
            );
            "admin_delisted"
        }
        other => panic!("应只有一个终结动作成功：{other:?}"),
    };
    assert_eq!(f.state(listing_id).await, ("closed".into(), 2, Some(reason.into()), false));
    assert!(f.retired(listing_id).await);
    let work_state: String =
        sqlx::query_scalar("SELECT state FROM public.consignment_lookup_work WHERE listing_id=$1")
            .bind(listing_id)
            .fetch_one(f.pool())
            .await
            .unwrap();
    assert_eq!(work_state, "cancelled");

    assert_eq!(
        f.store
            .withdraw(withdraw(cancel.target.command_id.as_uuid(), listing_id, SELLER, 2))
            .await
            .unwrap()
            .into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );
    let changed = delist(admin.command_id.as_uuid(), listing_id, 1, "不同运营说明");
    assert_eq!(
        f.store.admin_delist(changed).await.unwrap().into_result(),
        Err(ConsignmentError::CommandIdConflict)
    );
    assert_eq!(
        f.store
            .admin_delist(delist(new_id(), listing_id, 2, "终态不应再下架"))
            .await
            .unwrap()
            .into_result(),
        Err(Rejection::ListingNotActive.into())
    );
    let hidden = withdraw(new_id(), listing_id, OTHER, 1);
    assert_eq!(
        f.store.withdraw(hidden.clone()).await.unwrap().into_result(),
        Err(Rejection::ListingNotFound.into())
    );
    assert_eq!(
        f.store.withdraw(hidden.clone()).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Rejected(
            Rejection::ListingNotFound
        ))
    );
    let no_target: bool = sqlx::query_scalar(
        "SELECT listing_id IS NULL AND result_body->'listing_id' = 'null'::jsonb
         FROM public.consignment_commands WHERE command_id=$1",
    )
    .bind(hidden.target.command_id.as_uuid())
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert!(no_target);

    // 无竞争用不同挂单验证两条正常关闭和私有审计，避免竞争胜者影响覆盖。
    let owned = new_id();
    f.registered(owned, &gear("main", "435"), SELLER, ListingMode::Direct).await;
    let withdrawal = withdraw(new_id(), owned, SELLER, 1);
    let closed = f.store.withdraw(withdrawal.clone()).await.unwrap().into_result().unwrap();
    assert_eq!(
        f.store.withdraw(withdrawal).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(closed))
    );
    let managed = new_id();
    f.registered(managed, &gear("main", "436"), SELLER, ListingMode::Direct).await;
    let admin = delist(new_id(), managed, 1, "通过私有审计确认");
    let closed = f.store.admin_delist(admin.clone()).await.unwrap().into_result().unwrap();
    assert_eq!(
        f.store.admin_delist(admin.clone()).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(closed))
    );
    let (reason, leaked): (String, bool) = sqlx::query_as(
        "SELECT a.metadata->>'admin_delist_reason', c.result_body::text LIKE '%通过私有审计确认%'
         FROM public.audit_records a JOIN public.consignment_commands c
           ON a.metadata->>'command_id'=c.command_id::text WHERE c.command_id=$1",
    )
    .bind(admin.command_id.as_uuid())
    .fetch_one(f.pool())
    .await
    .unwrap();
    assert_eq!(reason, "通过私有审计确认");
    assert!(!leaked);
    assert_eq!(
        f.store
            .admin_delist(delist(new_id(), new_id(), 1, "不存在也不暴露其他记录"))
            .await
            .unwrap()
            .into_result(),
        Err(Rejection::ListingNotFound.into())
    );
}

#[tokio::test]
async fn postgres_reprice_local_disable_arbitrates_then_checks_owner_without_writes() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    f.registered(listing_id, &gear("main", "437"), SELLER, ListingMode::Direct).await;
    sqlx::query("UPDATE public.consignment_lookup_control SET admitting=false, enabled_epoch=2")
        .execute(f.pool())
        .await
        .unwrap();

    let mut blocked = reprice(new_id(), listing_id, SELLER, 1, 801);
    blocked.enabled = false;
    let before = f.persisted().await;
    assert_eq!(
        f.store.reprice(blocked).await.unwrap().into_result(),
        Err(ConsignmentError::Disabled)
    );
    assert!(f.persisted().await == before, "相关持久事实须保持一致");

    let mut hidden = reprice(new_id(), listing_id, OTHER, 1, 802);
    hidden.enabled = false;
    assert_eq!(
        f.store.reprice(hidden).await.unwrap().into_result(),
        Err(Rejection::ListingNotFound.into())
    );
    assert_eq!(f.state(listing_id).await.1, 1);
    assert_eq!(
        f.store.withdraw(withdraw(new_id(), listing_id, SELLER, 1)).await.unwrap().into_result(),
        Err(Rejection::ListingNotActive.into())
    );
    assert_eq!(f.state(listing_id).await.2.as_deref(), Some("service_disabled"));
}

#[tokio::test]
async fn postgres_write_request_id_is_audited_once_and_does_not_change_replay() {
    let f = Fixture::new().await;
    let listing_id = new_id();
    let equipment = gear("main", "438");
    let registration_id = new_id();
    let registration = |request_id: &str| {
        let mut cmd =
            register(registration_id, listing_id, &equipment, &f.key, SELLER, ListingMode::Direct);
        cmd.request_id = Some(request_id.into());
        cmd
    };
    let registered = f
        .store
        .register(registration("register-first"), Some(&f.crypto), Some(&f.crypto))
        .await
        .unwrap()
        .into_result()
        .unwrap();
    assert_eq!(
        f.store.register(registration("register-retry"), None, Some(&f.crypto)).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(registered))
    );

    let reprice_id = new_id();
    let mut price_command = reprice(reprice_id, listing_id, SELLER, 1, 173);
    price_command.request_id = Some("reprice-first".into());
    let repriced = f.store.reprice(price_command.clone()).await.unwrap().into_result().unwrap();
    assert_eq!(repriced.disposition, ListingPriceDisposition::Repriced);
    price_command.request_id = Some("reprice-retry".into());
    assert_eq!(
        f.store.reprice(price_command).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(repriced))
    );

    let withdraw_id = new_id();
    let mut withdraw_command = withdraw(withdraw_id, listing_id, SELLER, 2);
    withdraw_command.request_id = Some("withdraw-first".into());
    let withdrawn =
        f.store.withdraw(withdraw_command.clone()).await.unwrap().into_result().unwrap();
    withdraw_command.request_id = Some("withdraw-retry".into());
    assert_eq!(
        f.store.withdraw(withdraw_command).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(withdrawn))
    );

    let delist_listing_id = new_id();
    f.registered(delist_listing_id, &gear("main", "439"), SELLER, ListingMode::Consigned).await;
    let delist_id = new_id();
    let mut delist_command = delist(delist_id, delist_listing_id, 1, "测试请求关联");
    delist_command.request_id = Some("delist-first".into());
    let delisted =
        f.store.admin_delist(delist_command.clone()).await.unwrap().into_result().unwrap();
    delist_command.request_id = Some("delist-retry".into());
    assert_eq!(
        f.store.admin_delist(delist_command).await.unwrap(),
        ListingCommandOutcome::Replayed(StoredListingDecision::Accepted(delisted))
    );

    // 经维护池的独立连接核对提交结果，包括改价成功的动作、对象及审计处置。
    let mut reader = f.owner.postgres_pool().acquire().await.unwrap();
    type AuditRow = (String, String, String, String, String, Option<String>, String);
    for (command_id, action, actor, actor_id, audited_listing_id, request_id, disposition) in [
        (
            registration_id,
            "consignment.register",
            "public_user",
            SELLER.to_string(),
            listing_id,
            "register-first",
            "registered",
        ),
        (
            reprice_id,
            "consignment.reprice",
            "public_user",
            SELLER.to_string(),
            listing_id,
            "reprice-first",
            "repriced",
        ),
        (
            withdraw_id,
            "consignment.withdraw",
            "public_user",
            SELLER.to_string(),
            listing_id,
            "withdraw-first",
            "closed",
        ),
        (
            delist_id,
            "consignment.admin_delist",
            "admin",
            "fake-admin".into(),
            delist_listing_id,
            "delist-first",
            "closed",
        ),
    ] {
        let persisted: Vec<AuditRow> = sqlx::query_as(
            "SELECT action,actor_type,actor_id,entity_id,result,request_id,metadata->>'disposition'
             FROM public.audit_records WHERE metadata->>'command_id'=$1 AND entity_type='consignment_listing'"
        ).bind(command_id.to_string()).fetch_all(&mut *reader).await.unwrap();
        assert_eq!(
            persisted,
            [(
                action.into(),
                actor.into(),
                actor_id,
                audited_listing_id.to_string(),
                "success".into(),
                Some(request_id.into()),
                disposition.into()
            )]
        );
    }
}
