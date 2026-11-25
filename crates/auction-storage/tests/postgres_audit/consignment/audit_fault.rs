//! 第 5–7 期写口的审计失败注入夹具：在 `audit_records` 上装拒绝触发器，并对外写、命令、处分权、负债、
//! 证据与全库内容拍快照，供各写口断言“审计失败整笔回滚”（不变量 3：业务事实与追加审计同事务）。
//!
//! 触发器拒绝**所有**审计插入，不按 action 前缀过滤：写口若改用别的 action，同样会被拦下，
//! 不会因为前缀不匹配而让注入静默失效；每例都以撤掉注入后同一命令可提交作对照。

use sqlx::{AssertSqlSafe, PgPool};

/// 一次写口调用前后要求不变的持久事实。
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Facts {
    /// 起写许可与 Send 尝试行：每一行都意味着一次可能发生的远端 POST。
    pub(crate) posts: i64,
    /// 已提交的寄售命令（回执）行数。
    pub(crate) commands: i64,
    /// 处分权：position 与处分代的内容摘要。
    pub(crate) custody: String,
    /// 负债：资金分录与分配、资金回补、费用、Sale 封闭与现金退件阻断的内容摘要。
    pub(crate) liabilities: String,
    /// 追加证据：取得 / 退回证明、损失证据、交接证明与代管事件的行数。
    pub(crate) evidence: i64,
    /// 除 `audit_records` 外全部 public 表的逐表内容摘要：上面各项之外的任何残留也会让快照不等。
    pub(crate) tables: Vec<(String, String)>,
}

/// 安装拒绝触发器：此后任何 `audit_records` 插入都以 23514 失败。须以维护账号连接。
pub(crate) async fn reject_audits(owner: &PgPool) {
    sqlx::raw_sql(
        "CREATE FUNCTION public.fake_consignment_audit_reject() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN RAISE EXCEPTION '测试审计失败' USING ERRCODE = '23514'; END;
         $$;
         CREATE TRIGGER fake_consignment_audit_reject BEFORE INSERT ON public.audit_records
             FOR EACH ROW EXECUTE FUNCTION public.fake_consignment_audit_reject();",
    )
    .execute(owner)
    .await
    .expect("安装审计失败触发器");
}

/// 撤掉注入，恢复正常审计写入。
pub(crate) async fn accept_audits(owner: &PgPool) {
    sqlx::raw_sql(
        "DROP TRIGGER fake_consignment_audit_reject ON public.audit_records;
         DROP FUNCTION public.fake_consignment_audit_reject();",
    )
    .execute(owner)
    .await
    .expect("移除审计失败触发器");
}

/// 审计记录总数。
pub(crate) async fn audit_rows(owner: &PgPool) -> i64 {
    scalar(owner, "SELECT count(*) FROM public.audit_records").await
}

/// 拍下当前快照。
pub(crate) async fn facts(owner: &PgPool) -> Facts {
    Facts {
        posts: scalar(
            owner,
            "SELECT (SELECT count(*) FROM public.mail_disposition_write_starts)
                  + (SELECT count(*) FROM public.delivery_send_attempts)
                  + (SELECT count(*) FROM public.consignment_dispatch_attempts)",
        )
        .await,
        commands: scalar(owner, "SELECT count(*) FROM public.consignment_commands").await,
        custody: digests(
            owner,
            &["consignment_custody_positions", "consignment_disposition_generations"],
        )
        .await,
        liabilities: digests(
            owner,
            &[
                "settlement_funding_entries",
                "settlement_funding_allocations",
                "consignment_funding_returns",
                "consignment_fee_facts",
                "consignment_sale_closures",
                "consignment_cash_return_blocks",
            ],
        )
        .await,
        evidence: scalar(
            owner,
            "SELECT (SELECT count(*) FROM public.mail_disposition_proofs)
                  + (SELECT count(*) FROM public.consignment_loss_proofs)
                  + (SELECT count(*) FROM public.consignment_handoff_proofs)
                  + (SELECT count(*) FROM public.consignment_custody_events)",
        )
        .await,
        tables: all_tables(owner).await,
    }
}

async fn scalar(owner: &PgPool, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(owner).await.expect("读取计数")
}

async fn digest(owner: &PgPool, table: &str) -> String {
    sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '') FROM public.\"{table}\" AS t"
    )))
    .fetch_one(owner)
    .await
    .expect("读取内容摘要")
}

async fn digests(owner: &PgPool, tables: &[&str]) -> String {
    let mut joined = String::new();
    for table in tables {
        joined.push_str(&digest(owner, table).await);
        joined.push('|');
    }
    joined
}

async fn all_tables(owner: &PgPool) -> Vec<(String, String)> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_catalog.pg_tables
         WHERE schemaname = 'public' AND tablename <> 'audit_records' ORDER BY tablename",
    )
    .fetch_all(owner)
    .await
    .expect("读取表名");
    let mut tables = Vec::with_capacity(names.len());
    for name in names {
        let content = digest(owner, &name).await;
        tables.push((name, content));
    }
    tables
}
