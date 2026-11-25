//! 寄售处罚的账号拉黑：存储按命令自带的 actor 落黑名单、历史与审计，并按命令账本幂等。

use auction_application::admin::{
    RECOVERY_BUYER_BLACKLIST_ACTOR, RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR,
    RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON, RecoveryBuyerBlacklistCommand,
    RecoveryBuyerBlacklister,
};
use auction_application::consignment::{
    ConsignmentSanctionAccountWork, consignment_sanction_blacklist_command,
};
use auction_domain::SettlementId;
use auction_domain::consignment::refusal::SanctionRule;
use auction_time::Timestamp;
use uuid::Uuid;

use super::fixture::{Fixture, SELLER, digest, new_id};

async fn actors(f: &Fixture, command_id: Uuid) -> (Vec<String>, Vec<String>, Vec<String>) {
    let pool = f.owner.postgres_pool();
    let current = sqlx::query_scalar("SELECT actor FROM public.user_blacklist WHERE user_id=$1")
        .bind(SELLER)
        .fetch_all(pool)
        .await
        .unwrap();
    let history = sqlx::query_scalar("SELECT actor FROM public.user_blacklist_history WHERE id=$1")
        .bind(command_id)
        .fetch_all(pool)
        .await
        .unwrap();
    let audit = sqlx::query_scalar(
        "SELECT actor_id FROM public.audit_records
         WHERE action='user.blacklist' AND metadata->>'command_id'=$1",
    )
    .bind(command_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    (current, history, audit)
}

#[tokio::test]
async fn postgres_consignment_sanction_blacklist_is_attributed_to_its_own_actor() {
    let f = Fixture::new().await;
    let occurred_at: Timestamp =
        sqlx::query_scalar("SELECT updated_at FROM public.users WHERE id=$1")
            .bind(SELLER)
            .fetch_one(f.owner.postgres_pool())
            .await
            .unwrap();
    let sanction_id = new_id();
    let work = ConsignmentSanctionAccountWork {
        sanction_id,
        work_id: new_id(),
        buyer_user_id: SELLER,
        rule: SanctionRule::MainCodReturns,
        occurred_at,
    };
    let command = consignment_sanction_blacklist_command(&work).unwrap();
    let command_id = command.command_id;
    let expected = RECOVERY_BUYER_BLACKLIST_CONSIGNMENT_ACTOR.to_owned();

    let result = f.store.ensure_buyer_blacklisted(command.clone()).await.unwrap();
    assert!(result.validate());
    assert_eq!(result.actor_id, expected);
    let (current, history, audit) = actors(&f, command_id).await;
    assert_eq!(
        (current, history, audit),
        (vec![expected.clone()], vec![expected.clone()], vec![expected.clone()])
    );

    // 同命令原样重放返回原结果，不再追加历史或审计行。
    let replay = f.store.ensure_buyer_blacklisted(command).await.unwrap();
    assert_eq!(replay, result);
    let (_, history, audit) = actors(&f, command_id).await;
    assert_eq!((history.len(), audit.len()), (1, 1));

    // 同 command_id 换理由（同一处罚的另一条规则）：冲突，不写第二行。
    let other_rule =
        ConsignmentSanctionAccountWork { rule: SanctionRule::IsekaiUnpaidCarrierReturns, ..work };
    let changed_reason = consignment_sanction_blacklist_command(&other_rule).unwrap();
    assert_eq!(changed_reason.command_id, command_id);
    assert!(f.store.ensure_buyer_blacklisted(changed_reason).await.is_err());

    // 同 command_id 换 actor（配拍卖理由，身份成对合法）：冲突，不写第二行。
    let changed_actor = RecoveryBuyerBlacklistCommand {
        command_id,
        request_hash: digest(&[b"auction-actor"]),
        recovery_id: SettlementId::from_uuid(sanction_id),
        action_id: SettlementId::from_uuid(new_id()),
        buyer_user_id: SELLER,
        public_reason: RECOVERY_BUYER_BLACKLIST_PUBLIC_REASON.into(),
        actor_id: RECOVERY_BUYER_BLACKLIST_ACTOR.into(),
        occurred_at,
    };
    assert!(f.store.ensure_buyer_blacklisted(changed_actor).await.is_err());
    let (current, history, audit) = actors(&f, command_id).await;
    assert_eq!((current, history.len(), audit.len()), (vec![expected.clone()], 1, 1));
    assert_eq!(history, vec![expected.clone()]);
    assert_eq!(audit, vec![expected]);
}
