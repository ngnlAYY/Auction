//! 匿名非寄售登记（000138）经登记写入者落库：命令账本、事件门、审计与约束映射。

use auction_application::consignment::{ConsignmentError, ListingBusinessRejection};
use auction_domain::consignment::ListingMode;
use uuid::Uuid;

use super::fixture::{client_command_id, equipment, register_as, seed};
use crate::database::DatabaseStore;
use crate::database::consignment::commands::errors::WriteError;
use crate::database::consignment::listings::projection::ListingLockIdentity;

/// 正例：匿名非寄售登记被受理，账本、挂单与审计按匿名身份落库。
/// 反例：同名（大小写不同）同件被唯一索引挡为 AlreadyOpen；匿名寄售被卖家约束拒绝。
pub(super) async fn anonymous_registration(
    store: &DatabaseStore,
    maintenance: &DatabaseStore,
) -> Result<(), WriteError> {
    seed(maintenance).await?;
    let listing_id = Uuid::from_u128(701);
    let anonymous = ListingLockIdentity::anonymous("Anon Seller", equipment("42"));
    let receipt =
        register_as(store, client_command_id(701), listing_id, &anonymous, ListingMode::Direct)
            .await?;
    assert_eq!(receipt.status, 202);
    let pool = maintenance.postgres_pool();
    let stored: (Option<Uuid>, Option<String>, String, String) = sqlx::query_as(
        "SELECT l.seller_user_id, l.seller_game_name, c.actor_kind, c.actor_ref
         FROM public.consignment_listings l
         JOIN public.consignment_commands c ON c.listing_id = l.id AND c.operation = 'register'
         WHERE l.id = $1",
    )
    .bind(listing_id)
    .fetch_one(pool)
    .await?;
    assert_eq!(
        stored,
        (None, Some("Anon Seller".into()), "anonymous".into(), "Anon Seller".into())
    );
    let audit: (String, String, String) = sqlx::query_as(
        "SELECT actor_type, actor_id, source FROM public.audit_records
         WHERE action LIKE 'consignment.%' ORDER BY occurred_at DESC LIMIT 1",
    )
    .fetch_one(pool)
    .await?;
    assert_eq!(audit, ("public_user".into(), "anonymous:Anon Seller".into(), "public".into()));

    let same_name = ListingLockIdentity::anonymous("ANON SELLER", equipment("42"));
    let duplicate = register_as(
        store,
        client_command_id(702),
        Uuid::from_u128(702),
        &same_name,
        ListingMode::Direct,
    )
    .await;
    assert!(matches!(
        duplicate,
        Err(WriteError::Application(ConsignmentError::Business(
            ListingBusinessRejection::AlreadyOpen
        )))
    ));
    let consigned = register_as(
        store,
        client_command_id(703),
        Uuid::from_u128(703),
        &ListingLockIdentity::anonymous("Anon Seller", equipment("43")),
        ListingMode::Consigned,
    )
    .await;
    assert!(matches!(
        consigned,
        Err(WriteError::Invariant { ref constraint, .. }) if constraint == "consignment_listing_seller"
    ));
    Ok(())
}
