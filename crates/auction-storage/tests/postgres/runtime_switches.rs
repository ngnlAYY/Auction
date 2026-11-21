//! 验收：运行账号以只追加权限收口开关到期，并拒绝非法的过期目标。

use std::sync::Arc;

use auction_application::operations::AuditFailureMode;
use auction_application::runtime_switches::{
    RuntimeSwitchEvent, RuntimeSwitchName, RuntimeSwitchOperation, RuntimeSwitchRefreshService,
    RuntimeSwitchRole, RuntimeSwitchStore, RuntimeSwitches,
};
use auction_storage::DatabaseStore;
use auction_time::Timestamp;
use serde_json::json;
use uuid::Uuid;

use crate::support;

fn expired_pause() -> RuntimeSwitchEvent {
    RuntimeSwitchEvent {
        command_id: Uuid::new_v4(),
        name: RuntimeSwitchName::ExternalAccessPaused,
        revision: 1,
        operation: RuntimeSwitchOperation::Set,
        value: Some(json!(true)),
        expires_at: Some(Timestamp::from_ymd_hms(2025, 1, 1, 1, 0, 0).unwrap()),
        expired_command_id: None,
        actor_id: "switch-fixture".to_owned(),
        entrypoint: "admin".to_owned(),
        reason: "expiry regression fixture".to_owned(),
        recorded_at: Timestamp::from_ymd_hms(2025, 1, 1, 0, 0, 0).unwrap(),
    }
}

// 直接录入已到期的历史夹具，无需等待墙钟；反例经同一路径直接验证数据库触发器。
async fn insert_event(
    store: &DatabaseStore,
    event: &RuntimeSwitchEvent,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "WITH audit AS (
            INSERT INTO public.audit_records
                (id,occurred_at,actor_type,actor_id,action,entity_type,entity_id,result,source,metadata)
            VALUES (gen_random_uuid(),$8,$10,$9,'runtime_switch.' || $4::text,'runtime_switch',$2,
                    'success',$10,jsonb_build_object('command_id',$1::uuid))
            RETURNING id
         )
         INSERT INTO public.runtime_switch_events
            (command_id,digest_version,request_digest,switch_name,revision,expected_revision,
             operation,value,expires_at,expired_command_id,actor_id,entrypoint,reason,audit_id,
             result_payload,recorded_at)
         SELECT $1,1,decode(repeat('00',32),'hex'),$2,$3,$3::bigint - 1,$4,$5,$6,$7,
                $9,$10,$11,audit.id,$12,$8
         FROM audit",
    )
    .bind(event.command_id)
    .bind(event.name.label())
    .bind(event.revision)
    .bind(event.operation.label())
    .bind(&event.value)
    .bind(event.expires_at)
    .bind(event.expired_command_id)
    .bind(event.recorded_at)
    .bind(&event.actor_id)
    .bind(&event.entrypoint)
    .bind(&event.reason)
    .bind(serde_json::to_value(event).unwrap())
    .execute(store.postgres_pool())
    .await?;
    Ok(())
}

#[tokio::test]
async fn postgres_runtime_switch_worker_expiry_appends_one_fact_and_audit() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let can_update: bool = sqlx::query_scalar(
        "SELECT has_any_column_privilege(current_user,'public.runtime_switch_events','UPDATE')",
    )
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取开关账本权限");
    assert!(!can_update, "运行账号不得拥有开关账本的列更新权限");

    let pause = expired_pause();
    insert_event(&store, &pause).await.expect("录入已到期设置");
    let switches = RuntimeSwitches::new(RuntimeSwitchRole::Worker, AuditFailureMode::Shadow, true);
    let service = RuntimeSwitchRefreshService::new(Arc::new(store.clone()), switches);
    let (first, second) = tokio::join!(service.expire(), service.expire());
    first.expect("收口到期设置");
    second.expect("并行收口完成");

    let read = store.read().await.expect("读取最新开关事实");
    let [expired] = read.events.as_slice() else {
        panic!("应只读到一个开关的最新事实")
    };
    assert_eq!(expired.name, pause.name);
    assert_eq!(expired.revision, 2);
    assert_eq!(expired.operation, RuntimeSwitchOperation::Expire);
    assert_eq!(expired.expired_command_id, Some(pause.command_id));
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM public.runtime_switch_events WHERE switch_name = $1),
                (SELECT count(*) FROM public.audit_records
                 WHERE action = 'runtime_switch.expire' AND entity_type = 'runtime_switch'
                   AND entity_id = $1 AND source = 'system' AND metadata->>'command_id' = $2)",
    )
    .bind(pause.name.label())
    .bind(expired.command_id.to_string())
    .fetch_one(store.postgres_pool())
    .await
    .expect("读取追加事实与审计计数");
    assert_eq!(counts, (2, 1));
    store.postgres_pool().close().await;
}

#[tokio::test]
async fn postgres_runtime_switch_expiry_rejects_invalid_targets_as_runtime() {
    let database = support::TestDatabase::create().await.expect("克隆模板库");
    let store = database.store().await.expect("以运行账号连接");
    let pause = expired_pause();
    insert_event(&store, &pause).await.expect("录入已到期设置");
    let expire = RuntimeSwitchEvent {
        command_id: Uuid::new_v4(),
        revision: 2,
        operation: RuntimeSwitchOperation::Expire,
        value: None,
        expires_at: None,
        expired_command_id: Some(pause.command_id),
        entrypoint: "system".to_owned(),
        recorded_at: pause.expires_at.unwrap(),
        ..pause.clone()
    };
    for (case, invalid) in [
        (
            "目标不存在",
            RuntimeSwitchEvent { expired_command_id: Some(Uuid::new_v4()), ..expire.clone() },
        ),
        (
            "跨开关",
            RuntimeSwitchEvent { name: RuntimeSwitchName::ReadinessCacheEnabled, ..expire.clone() },
        ),
        ("目标版本不匹配", RuntimeSwitchEvent { revision: 3, ..expire.clone() }),
        ("期限未到", RuntimeSwitchEvent { recorded_at: pause.recorded_at, ..expire.clone() }),
    ] {
        let error = insert_event(&store, &invalid).await.expect_err(case);
        assert_eq!(
            error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
            Some("23514"),
            "{case} {error}",
        );
    }

    let clear = RuntimeSwitchEvent {
        command_id: Uuid::new_v4(),
        operation: RuntimeSwitchOperation::Clear,
        expired_command_id: None,
        entrypoint: "admin".to_owned(),
        ..expire.clone()
    };
    insert_event(&store, &clear).await.expect("追加同名清除事实");
    let error = insert_event(&store, &expire).await.expect_err("拒绝收口已经过时的设置");
    assert_eq!(
        error.as_database_error().and_then(sqlx::error::DatabaseError::code).as_deref(),
        Some("23514"),
        "{error}",
    );
    store.postgres_pool().close().await;
}
