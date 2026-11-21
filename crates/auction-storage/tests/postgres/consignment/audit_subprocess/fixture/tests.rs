//! 用独立连接验证两个子测试只复用已经完成共享初始化的父夹具。

use std::env;
use std::time::Duration;

use sqlx::{Connection, PgConnection};

use super::{
    ADMIN_URL, PREFIX, ParentFixture, TEMPLATE, child_url, control_connection, execute,
    lock_prefix, new_prefix, open_child_database, quote_ident,
};
use crate::consignment::audit_subprocess::test_path;

#[tokio::test]
#[ignore = "由共用服务器并发夹具的父测试启动"]
async fn postgres_consignment_shared_init_probe_child() {
    let (runtime, maintenance) = open_child_database().await.expect("克隆已初始化的父夹具");
    let is_runtime_member: bool =
        sqlx::query_scalar("SELECT pg_has_role(current_user, 'auction_runtime', 'MEMBER')")
            .fetch_one(runtime.postgres_pool())
            .await
            .expect("检查运行账号的角色来源");
    assert!(is_runtime_member);
    let migrated: bool =
        sqlx::query_scalar("SELECT to_regclass('public.consignment_commands') IS NOT NULL")
            .fetch_one(maintenance.postgres_pool())
            .await
            .expect("检查克隆库已迁移");
    assert!(migrated);

    let prefix = env::var(PREFIX).expect("父夹具前缀");
    let template = env::var(TEMPLATE).expect("父夹具克隆源");
    let mut control = control_connection(&child_url(ADMIN_URL).expect("父维护地址"), &prefix)
        .await
        .expect("同步连接");
    // 在同一个维护库以共享锁会合，父测试能确认两个子进程同时到场。
    sqlx::query("SELECT pg_advisory_lock_shared(hashtextextended($1, 0))")
        .bind(template)
        .execute(&mut control)
        .await
        .expect("等待两个子进程同步");
}

#[tokio::test]
async fn postgres_consignment_children_share_completed_role_initialization() {
    let fixture = ParentFixture::prepare().await.expect("完成父夹具初始化");
    let child = test_path(module_path!(), "postgres_consignment_shared_init_probe_child");
    let prefixes = [new_prefix(), new_prefix()];
    let mut blocker = control_connection(&fixture.admin_url, "auction_audit_role_blocker")
        .await
        .expect("打开角色初始化探针连接");
    let mut gate = control_connection(&fixture.admin_url, "auction_audit_child_gate")
        .await
        .expect("打开子进程同步连接");
    lock_prefix(&mut gate, &fixture.template).await.expect("锁住子进程同步点");

    // 重现首次创建角色的重叠窗口，使用独立、唯一的角色名以保护共享运行角色。
    let role = quote_ident(&format!("{}_race", fixture.template));
    let mut transaction = blocker.begin().await.expect("开启角色初始化事务");
    execute(&mut transaction, format!("CREATE ROLE {role}"), "创建并发探针角色失败")
        .await
        .expect("留下尚未提交的角色目录项");

    let mut observer = control_connection(&fixture.admin_url, "auction_audit_child_observer")
        .await
        .expect("打开锁观察连接");
    let rendezvous = async {
        let waits = tokio::time::timeout(
            Duration::from_secs(15),
            wait_for_children(&mut observer, &prefixes),
        )
        .await;
        let waits_for_role = matches!(waits, Ok(Ok(true)));
        if waits_for_role {
            // 注入旧的检查后创建时，提交首个创建者，让等待中的创建者明确收到唯一冲突。
            transaction.commit().await.expect("提交探针角色");
        } else {
            transaction.rollback().await.expect("回滚未使用的探针角色");
        }
        drop(gate);
        waits.expect("两个子进程应同时到达同步点").expect("读取子进程等待状态")
    };
    let (first, second, waits_for_role) = tokio::join!(
        fixture.run_with_prefix(&child, &prefixes[0]),
        fixture.run_with_prefix(&child, &prefixes[1]),
        rendezvous,
    );
    execute(&mut blocker, format!("DROP ROLE IF EXISTS {role}"), "清理探针角色失败")
        .await
        .expect("清理探针角色");
    assert!(
        !waits_for_role && first.is_ok() && second.is_ok(),
        "两个子进程必须复用父进程已初始化的角色。等待角色创建={waits_for_role}，结果={first:?} / {second:?}"
    );
    let remaining: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname::text = ANY($1))
             OR EXISTS (SELECT 1 FROM pg_database WHERE datname::text = ANY($2))",
    )
    .bind(prefixes.iter().map(|prefix| format!("{prefix}runtime")).collect::<Vec<_>>())
    .bind(prefixes.iter().map(|prefix| format!("{prefix}db")).collect::<Vec<_>>())
    .fetch_one(&mut observer)
    .await
    .expect("查询子测试角色和临时库");
    assert!(!remaining, "子测试的临时库和角色必须由父进程回收");
}

async fn wait_for_children(
    observer: &mut PgConnection,
    prefixes: &[String],
) -> Result<bool, sqlx::Error> {
    loop {
        let waits: Vec<String> = sqlx::query_scalar(
            "SELECT wait_event FROM pg_stat_activity
             WHERE application_name = ANY($1) AND wait_event IN ('transactionid', 'advisory')",
        )
        .bind(prefixes)
        .fetch_all(&mut *observer)
        .await?;
        if waits.iter().any(|event| event == "transactionid") {
            return Ok(true);
        }
        if waits.len() == prefixes.len() {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
