//! 本人 v2 命令回执读口（D9 §6.1）：按原玩家 actor 读回购买的首次决定，只读、不写账本。
//!
//! 保护不变量 1（回执与原写入同一账本，只读重放）与 7（非本人、v1 命令与不存在同为空）。

use auction_application::consignment::{
    OwnedCommandReadV2, OwnedCommandV2, PurchaseOutcome, PurchaseStore,
};
use auction_domain::CommandId;
use uuid::Uuid;

use super::super::deposit::support::{REGISTER, USER_1};
use super::fixture::{BUYER, Fixture, USER_3, command_id, purchase, user};

/// 成交与确定拒绝都按原 actor 读回首次决定；他人、卖家、v1 登记与不存在的命令为空，读取零写入。
#[tokio::test]
async fn postgres_consignment_owned_command_v2_reads_the_actors_first_decision_only() {
    let fixture = Fixture::available().await;
    let won = fixture.repository.purchase(purchase(BUYER, 1, 0x11)).await.expect("购买");
    let PurchaseOutcome::Purchased(won) = won else { panic!("夹具购买必须成交：{won:?}") };
    let lost = fixture.repository.purchase(purchase(USER_3, 2, 0x22)).await.expect("确定拒绝");
    let PurchaseOutcome::Rejected(rejection) = lost else {
        panic!("已成交件应确定拒绝：{lost:?}")
    };
    let commands = fixture.count("consignment_commands").await;

    let read = |actor: &'static str, command: CommandId| {
        fixture.runtime.owned_command_v2(user(actor), command)
    };
    let Some(OwnedCommandV2::Purchase(PurchaseOutcome::Purchased(stored))) =
        read(BUYER, command_id(1)).await.expect("读成交")
    else {
        panic!("买家应读回成交回执");
    };
    assert_eq!(
        (stored.contract_id, stored.listing_version, stored.price, stored.equipment_id),
        (won.contract_id, won.listing_version, won.price, won.equipment_id)
    );
    assert_eq!(
        read(USER_3, command_id(2)).await.expect("读拒绝"),
        Some(OwnedCommandV2::Purchase(PurchaseOutcome::Rejected(rejection)))
    );
    let register = CommandId::from_uuid(Uuid::parse_str(REGISTER).expect("夹具命令"));
    for (actor, command) in [
        (USER_3, command_id(1)),
        (USER_1, command_id(1)),
        (BUYER, command_id(2)),
        (USER_1, register),
        (BUYER, command_id(99)),
    ] {
        assert_eq!(read(actor, command).await.expect("读"), None, "{actor} {command:?}");
    }
    assert_eq!(fixture.count("consignment_commands").await, commands, "读取不写账本");
}
