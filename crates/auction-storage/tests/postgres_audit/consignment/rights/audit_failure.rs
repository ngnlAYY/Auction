//! 受限权利写口的审计失败：发码与核验的回执、挑战、通知与会话随审计失败整笔回滚；撤掉注入后同命令提交，
//! 审计只含 right_id、command_id 与结果码，不含挑战码。
//!
//! 保护不变量 3（回执与追加审计同事务）、7（挑战码等秘密不进审计）。

use auction_application::consignment::RightsStore;
use auction_domain::consignment::rights::format_challenge_code;
use serde_json::Value;
use uuid::Uuid;

use super::fixture::{Rig, challenge_command, verify_command};
use crate::consignment::audit_fault::{accept_audits, audit_rows, facts, reject_audits};

const IP: &str = "198.51.100.7";

#[tokio::test]
async fn postgres_consignment_rights_audit_failure_rolls_back_challenge_and_verify() {
    let rig = Rig::new().await;
    let right = rig.seed_right(1, "claim_payment", &["claim_payment"]).await;
    let pool = &rig.pool;

    // 发码：审计失败时零挑战、零通知、零回执。
    let challenge = challenge_command(Uuid::new_v4(), 7, IP, &right);
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;
    assert!(rig.store.challenge(challenge.clone()).await.is_err(), "审计失败时发码必须失败");
    assert_eq!(facts(pool).await, before, "挑战、通知与回执都不得留下");
    assert_eq!(audit_rows(pool).await, audits);
    accept_audits(pool).await;
    let issued = rig.store.challenge(challenge).await.expect("撤掉注入后同命令发码");
    assert_eq!(facts(pool).await.commands, before.commands + 1);
    assert_eq!(audit_rows(pool).await, audits + 1, "发码与审计同事务恰一条");

    // 核验：审计失败时零会话、零回执；撤掉注入后同命令核验成功。
    let code = rig.open_code(issued.challenge_id).await;
    let verify = verify_command(Uuid::new_v4(), 7, issued.challenge_id, code.clone());
    let before = facts(pool).await;
    let audits = audit_rows(pool).await;
    reject_audits(pool).await;
    assert!(rig.store.verify(verify.clone()).await.is_err(), "审计失败时核验必须失败");
    assert_eq!(facts(pool).await, before, "会话、尝试计数与回执都不得留下");
    assert_eq!(audit_rows(pool).await, audits);
    accept_audits(pool).await;
    let verified = rig.store.verify(verify).await.expect("撤掉注入后同命令核验");
    assert_eq!(verified.right_id, right.right_id);
    assert_eq!(audit_rows(pool).await, audits + 1);

    // 审计只含 right_id、command_id、结果码；挑战码不出现在任何审计字段里。
    let rows: Vec<(String, String, Value)> = sqlx::query_as(
        "SELECT action, actor_id, metadata FROM public.audit_records
         WHERE action LIKE 'consignment.right.%' ORDER BY occurred_at, action",
    )
    .fetch_all(pool)
    .await
    .expect("读取权利审计");
    assert_eq!(
        rows.iter().map(|(action, _, _)| action.as_str()).collect::<Vec<_>>(),
        ["consignment.right.challenge", "consignment.right.verify"]
    );
    for (action, actor_id, metadata) in &rows {
        let mut keys: Vec<&str> =
            metadata.as_object().expect("元数据是对象").keys().map(String::as_str).collect();
        keys.sort_unstable();
        // 发码按定位符寻址，回执不含 right_id，审计也不推断；核验后才知道权利。
        let expected: &[&str] = if action == "consignment.right.challenge" {
            &["command_id", "result_code"]
        } else {
            &["command_id", "result_code", "right_id"]
        };
        assert_eq!(keys, expected, "{action} 的元数据只含 right_id、command_id 与结果码");
        assert_eq!(actor_id, "browser", "浏览器绑定摘要不出账本");
        let text = format!("{actor_id}{metadata}");
        let typed = format_challenge_code(&code);
        let raw: String = code.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect();
        for secret in [typed.as_str().to_owned(), typed.display(), raw] {
            assert!(!text.contains(&secret), "{action} 的审计不得含挑战码（任何形式）");
        }
    }
}
