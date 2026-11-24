//! 4B 读取端口：状态汇总与本人寄件指引的秘密边界。

use auction_application::consignment::{
    CustodyControlStore, DepositRead, OwnedDepositInstructions, ReservationStore,
    WithdrawDepositCommand,
};
use auction_domain::UserId;
use auction_domain::consignment::{FeeBps, ListingPrice};
use auction_storage::DepositSettings;
use uuid::Uuid;

use super::helpers::{
    USER, client_command, hash, issue_command, repository, repository_with_settings, seed_listing,
    seed_mail_ready,
};
use crate::support::TestDatabase;

fn listing_id() -> Uuid {
    Uuid::parse_str(super::helpers::LISTING).expect("固定挂单")
}

fn seller() -> UserId {
    UserId::from_uuid(Uuid::parse_str(USER).expect("固定卖家"))
}

#[tokio::test]
async fn postgres_deposit_reads_status_and_owner_instructions() {
    let database = TestDatabase::create().await.expect("克隆模板库");
    let maintenance = database.maintenance_store().await.expect("维护连接");
    let pool = maintenance.postgres_pool();
    seed_listing(pool, listing_id(), auction_domain::SettlementWorld::Main).await;
    let repository = repository(&database).await;
    let not_ready = repository.status().await.expect("读取未就绪状态");
    assert!(!not_ready.enabled);
    assert!(!not_ready.registration_ready);
    assert!(not_ready.registration_modes.is_empty());
    assert!(not_ready.registration_worlds.is_empty());
    seed_mail_ready(pool).await;

    let status = repository.status().await.expect("读取状态");
    assert!(status.enabled);
    assert!(status.registration_ready);
    assert!(status.accepts_mail);
    assert!(status.worlds.main.mail_ready);
    assert!(!status.worlds.isekai.mail_ready);
    assert_eq!(
        status.worlds.main.season_state,
        auction_domain::consignment::SeasonState::NotSeasonal
    );
    let configured = repository_with_settings(
        &database,
        DepositSettings::new(
            FeeBps::new(123).expect("费率"),
            ListingPrice::new(5).expect("下限"),
            ListingPrice::new(1_000).expect("上限"),
            24,
            7,
        ),
    )
    .await;
    let configured_status = configured.status().await.expect("配置状态");
    assert_eq!(configured_status.fee_bps, FeeBps::new(123).expect("费率"));
    assert_eq!(configured_status.min_price, ListingPrice::new(5).expect("下限"));
    assert_eq!(configured_status.max_price, ListingPrice::new(1_000).expect("上限"));

    let issued = repository
        .issue_reservation(issue_command(client_command(70), listing_id(), 2, 0xa0))
        .await
        .expect("签发预约");
    let instructions = repository
        .instructions(seller(), listing_id())
        .await
        .expect("读取本人指引")
        .expect("本人目标存在");
    let OwnedDepositInstructions::Active(active) = instructions else {
        panic!("有效预约必须是 active");
    };
    assert_eq!(active.world, auction_domain::SettlementWorld::Main);
    assert_eq!(active.subject, "consign");
    assert_eq!(active.bot, "FakeBot");
    assert_eq!(active.reservation_id, issued.reservation_id);
    assert!(!active.body.as_str().is_empty());
    assert_eq!(active.expires_at, issued.expires_at);

    let other = UserId::from_uuid(Uuid::new_v4());
    assert!(repository.instructions(other, listing_id()).await.expect("读取他人指引").is_none());
    assert!(
        repository.instructions(seller(), Uuid::new_v4()).await.expect("读取不存在目标").is_none()
    );

    repository
        .request_withdrawal(WithdrawDepositCommand {
            command_id: client_command(71),
            seller_user_id: seller(),
            listing_id: listing_id(),
            expected_version: auction_domain::consignment::ListingVersion::new(3).expect("版本"),
            request_hash: hash(0xa1),
            request_id: None,
        })
        .await
        .expect("关闭挂单并退休预约");
    let after = repository
        .instructions(seller(), listing_id())
        .await
        .expect("读取退休后指引")
        .expect("本人目标仍存在");
    assert_eq!(after, OwnedDepositInstructions::None);
}
