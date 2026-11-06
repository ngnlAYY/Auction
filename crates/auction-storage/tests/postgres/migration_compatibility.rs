//! W7.8 迁移兼容声明验证：沿迁移链证明 `compatible` 登记属实。
//!
//! `compatible` 的承诺是“上一版程序可以继续在只多出该迁移的 schema 上运行”。本夹具沿
//! 真实迁移链逐级前进，在 V-1 捕获旧程序依赖的数据库契约，执行 V 之后重新捕获：
//!
//! - 契约快照：关系/列/默认值/约束/触发器/函数/域/枚举/权限，只比较 V-1 已存在的对象。
//! - 写入探针：对每个既有表执行 `INSERT ... DEFAULT VALUES`；旧写入被拒绝方式的任何变化
//!   都说明新增了旧代码无法满足的约束。
//! - 旧版语句：`LEGACY_WORKLOAD` 固定重放旧程序实际发出的读写语句（基线版本即可执行的
//!   列集与闭集取值），要求结果逐字一致。
//!
//! V 登记为 `compatible` 时任何收缩都判失败并指名迁移；登记为 `breaking` 时只重建基线，
//! 因此破坏性迁移不会被误报。第二个测试用人为破坏证明探针本身会失败。
//!
//! 只使用既有 W6.2 隔离夹具（临时集群或 `AUCTION_TEST_DATABASE_URL` 的独立库）与回滚
//! 事务，不写生产数据。入口：`just test-db-compat`（同属 `just test-db`，CI 测试作业逐项运行）。

#![cfg(feature = "test-db")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[allow(dead_code)]
use crate::support;

use std::collections::{BTreeMap, BTreeSet};

use auction_storage::{postgres_expected_migration_version, postgres_migrate_to};
use sqlx::{AssertSqlSafe, PgPool};

/// 只由迁移/运维入口调用的函数：compatible 迁移会反复 `CREATE OR REPLACE` 它，
/// 旧程序的运行路径不依赖其函数体，因此只比较存在性与返回类型。
const MIGRATION_TOOLING_FUNCTIONS: &[&str] = &["apply_auction_runtime_privileges()"];

/// 001–074 不回填登记，缺行即 breaking；075 建立登记表并登记自身为 breaking，
/// 因此兼容链的固定起点是 075（再往前的 schema 没有登记可验证）。
const COMPATIBILITY_BASELINE_VERSION: i64 = 75;

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelationShape {
    kind: String,
    row_security: bool,
    force_row_security: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ColumnShape {
    data_type: String,
    not_null: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ConstraintShape {
    kind: String,
    definition: String,
    columns: Vec<String>,
}

/// 旧程序依赖的 public schema 契约；比较时只按基线对象过滤，新增对象不参与。
#[derive(Clone, Debug, Default)]
struct SchemaContract {
    relations: BTreeMap<String, RelationShape>,
    columns: BTreeMap<String, BTreeMap<String, ColumnShape>>,
    defaults: BTreeMap<String, BTreeMap<String, String>>,
    constraints: BTreeMap<String, BTreeMap<String, ConstraintShape>>,
    triggers: BTreeSet<(String, String)>,
    functions: BTreeMap<String, String>,
    domains: BTreeMap<String, String>,
    enums: BTreeMap<String, BTreeSet<String>>,
    relation_privileges: BTreeSet<(String, String, String)>,
    column_privileges: BTreeSet<(String, String, String, String)>,
    function_privileges: BTreeSet<(String, String, String)>,
}

/// 旧程序实际发出的语句；`query` 为真时语句必须返回单行文本摘要。
struct LegacyStep {
    label: &'static str,
    sql: &'static str,
    query: bool,
}

/// 旧版 Worker 的运维告警读写路径（列集与闭集取值在基线 075 即合法）。
/// 迁移 000089 给该表加了新列，因此它同时是“老代码忽略新列”的行为证据。
const LEGACY_WORKLOAD: &[LegacyStep] = &[
    LegacyStep {
        label: "operations-alert-signal-insert",
        sql: "INSERT INTO public.operations_alert_signals
                  (fingerprint, kind, scope, source, active, first_seen_at, last_seen_at,
                   next_notify_at, count_value, age_seconds, state_version)
              VALUES (
                  decode(repeat('00', 31) || '01', 'hex'),
                  'delivery_pending', 'delivery', 'delivery', true,
                  '2026-11-05T00:00:00Z'::timestamptz, '2026-11-05T00:00:30Z'::timestamptz,
                  '2026-11-05T00:02:00Z'::timestamptz, 1, 30, 1)",
        query: false,
    },
    LegacyStep {
        label: "operations-alert-signal-read",
        sql: "SELECT jsonb_build_object(
                      'kind', kind, 'scope', scope, 'source', source, 'active', active,
                      'count', count_value, 'age', age_seconds, 'version', state_version,
                      'seen', to_char(last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS')
                  )::text
              FROM public.operations_alert_signals
              WHERE fingerprint = decode(repeat('00', 31) || '01', 'hex')",
        query: true,
    },
    LegacyStep {
        label: "operations-alert-signal-update",
        sql: "UPDATE public.operations_alert_signals
              SET count_value = 2,
                  last_seen_at = '2026-11-05T00:01:00Z'::timestamptz,
                  state_version = 2
              WHERE fingerprint = decode(repeat('00', 31) || '01', 'hex')",
        query: false,
    },
    LegacyStep {
        label: "operations-alert-signal-reread",
        sql: "SELECT jsonb_build_object(
                      'kind', kind, 'scope', scope, 'source', source, 'active', active,
                      'count', count_value, 'age', age_seconds, 'version', state_version,
                      'seen', to_char(last_seen_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS')
                  )::text
              FROM public.operations_alert_signals
              WHERE fingerprint = decode(repeat('00', 31) || '01', 'hex')",
        query: true,
    },
    LegacyStep {
        label: "operations-alert-signal-delete",
        sql: "DELETE FROM public.operations_alert_signals
              WHERE fingerprint = decode(repeat('00', 31) || '01', 'hex')",
        query: false,
    },
    LegacyStep {
        label: "operations-alert-signal-empty-read",
        sql: "SELECT coalesce(
                      jsonb_agg(fingerprint ORDER BY fingerprint)::text, '[]')
              FROM public.operations_alert_signals
              WHERE kind = 'delivery_pending'",
        query: true,
    },
];

#[tokio::test]
async fn postgres_compatible_declarations_preserve_previous_version_contract() {
    let database = support::TestDatabase::create_empty().await.expect("创建空测试库");
    let store = database.store().await.expect("连接测试库");
    let pool = store.postgres_pool();

    let max_version = postgres_expected_migration_version();
    assert!(
        max_version > COMPATIBILITY_BASELINE_VERSION,
        "内嵌迁移最高版本 {max_version} 不高于兼容链起点 {COMPATIBILITY_BASELINE_VERSION}"
    );

    postgres_migrate_to(pool, COMPATIBILITY_BASELINE_VERSION).await.expect("迁移到兼容链起点");

    // 001–074 不回填；075 建立登记表并登记自身为 breaking。
    let legacy_registrations: i64 =
        sqlx::query_scalar("SELECT count(*)::bigint FROM public.schema_migration_compatibility")
            .fetch_one(pool)
            .await
            .expect("读取起点登记");
    assert_eq!(legacy_registrations, 1, "075 之前不得出现兼容登记");
    assert_eq!(
        registration_level(pool, COMPATIBILITY_BASELINE_VERSION).await.as_deref(),
        Some("breaking"),
        "075 建立登记表，必须登记为 breaking"
    );

    let mut baseline_contract = capture_contract(pool).await.expect("捕获基线契约");
    let mut baseline_probes =
        probe_legacy_writes(pool, &baseline_contract).await.expect("执行基线写入探针");
    let mut baseline_workload = run_legacy_workload(pool).await.expect("执行基线旧版语句");

    let mut verified_compatible = Vec::new();
    let mut verified_breaking = Vec::new();

    for version in (COMPATIBILITY_BASELINE_VERSION + 1)..=max_version {
        postgres_migrate_to(pool, version)
            .await
            .unwrap_or_else(|error| panic!("迁移到 {version} 失败：{error}"));
        let level = registration_level(pool, version).await.unwrap_or_else(|| {
            panic!("迁移 {version} 缺少兼容登记（缺行即 breaking，但不能缺登记）")
        });
        assert!(
            matches!(level.as_str(), "compatible" | "breaking"),
            "迁移 {version} 登记等级 {level} 不在闭集内"
        );

        let contract = capture_contract(pool).await.expect("捕获迁移后契约");
        let probes =
            probe_legacy_writes(pool, &baseline_contract).await.expect("执行迁移后写入探针");
        let workload = run_legacy_workload(pool).await.expect("执行迁移后旧版语句");

        if level == "compatible" {
            let mut problems = contract_violations(&baseline_contract, &contract);
            for (table, signature) in &baseline_probes {
                match probes.get(table) {
                    Some(after) if after == signature => {}
                    Some(after) => problems.push(format!(
                        "表 public.{table} 的旧版默认写入结果改变：{signature} → {after}"
                    )),
                    None => problems.push(format!("表 public.{table} 的旧版默认写入无法执行")),
                }
            }
            for (label, digest) in &baseline_workload {
                match workload.get(label) {
                    Some(after) if after == digest => {}
                    Some(after) => {
                        problems.push(format!("旧版语句 {label} 结果改变：{digest} → {after}"));
                    }
                    None => problems.push(format!("旧版语句 {label} 缺失结果")),
                }
            }
            assert!(
                problems.is_empty(),
                "迁移 {version} 登记为 compatible，但上一版程序依赖的契约被破坏：\n{}",
                problems.join("\n")
            );
            verified_compatible.push(version);
        } else {
            // breaking 允许任意变化：不比较，只重建基线。
            verified_breaking.push(version);
        }
        // 下一级一律相对执行后的状态比较，因此 compatible 的收缩会逐级累积。
        baseline_contract = contract;
        baseline_probes = probes;
        baseline_workload = workload;
    }

    let applied: i64 = sqlx::query_scalar("SELECT max(version) FROM public._sqlx_migrations")
        .fetch_one(pool)
        .await
        .expect("读取最终迁移版本");
    assert_eq!(applied, max_version, "逐级迁移必须完成全部内嵌迁移");

    // 登记覆盖与逐级验证集合必须与数据库事实完全一致，防止夹具悄悄跳过某一级。
    let registered: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM public.schema_migration_compatibility ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .expect("读取最终兼容登记");
    assert_eq!(
        registered,
        (COMPATIBILITY_BASELINE_VERSION..=max_version).collect::<Vec<_>>(),
        "075 起每个版本必须恰好登记一次"
    );
    let compatible_registered: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM public.schema_migration_compatibility
         WHERE level = 'compatible' ORDER BY version",
    )
    .fetch_all(pool)
    .await
    .expect("读取 compatible 登记");
    assert_eq!(
        compatible_registered, verified_compatible,
        "每个 compatible 登记都必须经过本次比较，不能有未验证的声明"
    );
    assert!(
        !verified_compatible.is_empty(),
        "兼容链中没有 compatible 迁移，夹具等于空转：{verified_breaking:?}"
    );
    // 至少覆盖当代唯一会改动既有表的 compatible 迁移，避免声明被悄悄改回 breaking 后失守。
    assert!(
        verified_compatible.contains(&89),
        "000089 必须仍登记为 compatible 并由本夹具验证：{verified_compatible:?}"
    );
}

/// 人为破坏必须被同一套比较逻辑拒绝；否则“通过”不构成证据。
#[tokio::test]
async fn postgres_compatibility_harness_rejects_breaking_changes() {
    let database = support::TestDatabase::create_empty().await.expect("创建空测试库");
    let store = database.store().await.expect("连接测试库");
    let pool = store.postgres_pool();

    postgres_migrate_to(pool, COMPATIBILITY_BASELINE_VERSION).await.expect("迁移到兼容链起点");

    // 造一张“旧程序用得到”的表，并给运行账号一个基线权限，供破坏性变更对照。
    sqlx::raw_sql(AssertSqlSafe(
        "CREATE TABLE public.compatibility_probe_legacy (
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
             payload text NOT NULL DEFAULT 'legacy'
         );
         GRANT SELECT ON public.lots TO auction_runtime;"
            .to_owned(),
    ))
    .execute(pool)
    .await
    .expect("准备对照对象");

    let baseline = capture_contract(pool).await.expect("捕获对照基线");
    let baseline_probes = probe_legacy_writes(pool, &baseline).await.expect("执行对照基线探针");
    let baseline_workload = run_legacy_workload(pool).await.expect("执行对照基线旧版语句");
    assert_eq!(
        baseline_probes.get("compatibility_probe_legacy").map(String::as_str),
        Some("ok"),
        "对照表的默认写入在基线必须成功"
    );

    sqlx::raw_sql(AssertSqlSafe(
        "ALTER TABLE public.compatibility_probe_legacy ADD COLUMN tightened text NOT NULL;
         ALTER TABLE public.users RENAME COLUMN game_player_id TO game_player_id_renamed;
         CREATE FUNCTION public.compatibility_probe_guard() RETURNS trigger
             LANGUAGE plpgsql AS $probe$ BEGIN RETURN NEW; END $probe$;
         CREATE TRIGGER compatibility_probe_guard BEFORE INSERT ON public.lots
             FOR EACH ROW EXECUTE FUNCTION public.compatibility_probe_guard();
         ALTER TABLE public.users ENABLE ROW LEVEL SECURITY;
         REVOKE SELECT ON public.lots FROM auction_runtime;
         CREATE FUNCTION public.compatibility_probe_rewrite() RETURNS trigger
             LANGUAGE plpgsql AS $probe$ BEGIN NEW.age_seconds := 0; RETURN NEW; END $probe$;
         CREATE TRIGGER compatibility_probe_rewrite
             BEFORE INSERT ON public.operations_alert_signals
             FOR EACH ROW EXECUTE FUNCTION public.compatibility_probe_rewrite();"
            .to_owned(),
    ))
    .execute(pool)
    .await
    .expect("构造破坏性变更");

    let current = capture_contract(pool).await.expect("捕获破坏后契约");
    let problems = contract_violations(&baseline, &current);
    let current_probes = probe_legacy_writes(pool, &baseline).await.expect("执行破坏后探针");
    let current_workload = run_legacy_workload(pool).await.expect("执行破坏后旧版语句");

    for expected in
        ["新增 NOT NULL", "被删除或重命名", "新增触发器", "新增行级安全", "SELECT 权限被收回"]
    {
        assert!(
            problems.iter().any(|problem| problem.contains(expected)),
            "契约比较必须检出「{expected}」，实际：{problems:?}"
        );
    }
    assert_ne!(
        current_probes.get("compatibility_probe_legacy"),
        baseline_probes.get("compatibility_probe_legacy"),
        "写入探针必须检出旧版 INSERT 被新增 NOT NULL 拒绝"
    );
    assert_ne!(
        current_workload.get("operations-alert-signal-read"),
        baseline_workload.get("operations-alert-signal-read"),
        "旧版语句重放必须检出新增触发器改写旧读结果"
    );
}

async fn registration_level(pool: &PgPool, version: i64) -> Option<String> {
    sqlx::query_scalar("SELECT level FROM public.schema_migration_compatibility WHERE version = $1")
        .bind(version)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|error| panic!("读取迁移 {version} 的兼容登记失败：{error}"))
}

/// 捕获 public schema 契约；所有查询只读，不修改数据库。
async fn capture_contract(pool: &PgPool) -> Result<SchemaContract, String> {
    let mut contract = SchemaContract::default();

    for (relation, kind, row_security, force_row_security) in
        sqlx::query_as::<_, (String, String, bool, bool)>(
            "SELECT rel.relname, rel.relkind::text, rel.relrowsecurity, rel.relforcerowsecurity
         FROM pg_class AS rel
         JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
         WHERE ns.nspname = 'public'
           AND rel.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取关系契约失败：{error}"))?
    {
        contract
            .relations
            .insert(relation, RelationShape { kind, row_security, force_row_security });
    }

    for (relation, column, data_type, not_null) in
        sqlx::query_as::<_, (String, String, String, bool)>(
            "SELECT rel.relname, att.attname, format_type(att.atttypid, att.atttypmod), att.attnotnull
             FROM pg_attribute AS att
             JOIN pg_class AS rel ON rel.oid = att.attrelid
             JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
             WHERE ns.nspname = 'public'
               AND rel.relkind IN ('r', 'p', 'v', 'm', 'f')
               AND att.attnum > 0
               AND NOT att.attisdropped",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取列契约失败：{error}"))?
    {
        contract
            .columns
            .entry(relation)
            .or_default()
            .insert(column, ColumnShape { data_type, not_null });
    }

    for (relation, column, expression) in sqlx::query_as::<_, (String, String, String)>(
        "SELECT rel.relname, att.attname, pg_get_expr(def.adbin, def.adrelid)
         FROM pg_attrdef AS def
         JOIN pg_attribute AS att
           ON att.attrelid = def.adrelid AND att.attnum = def.adnum
         JOIN pg_class AS rel ON rel.oid = def.adrelid
         JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
         WHERE ns.nspname = 'public' AND def.adbin IS NOT NULL",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取默认值契约失败：{error}"))?
    {
        contract.defaults.entry(relation).or_default().insert(column, expression);
    }

    for (relation, name, kind, definition, columns) in
        sqlx::query_as::<_, (String, String, String, String, String)>(
            "SELECT rel.relname, con.conname, con.contype::text,
                    pg_get_constraintdef(con.oid),
                    coalesce((
                        SELECT string_agg(att.attname, ',' ORDER BY att.attname)
                        FROM pg_attribute AS att
                        WHERE att.attrelid = con.conrelid
                          AND att.attnum = ANY (con.conkey)
                    ), '')
             FROM pg_constraint AS con
             JOIN pg_class AS rel ON rel.oid = con.conrelid
             JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
             WHERE ns.nspname = 'public'
               AND con.conrelid <> 0
               AND con.contype IN ('c', 'u', 'p', 'f', 'x', 'n')",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取约束契约失败：{error}"))?
    {
        let columns =
            columns.split(',').filter(|column| !column.is_empty()).map(str::to_owned).collect();
        contract
            .constraints
            .entry(relation)
            .or_default()
            .insert(name, ConstraintShape { kind, definition, columns });
    }

    for (relation, trigger) in sqlx::query_as::<_, (String, String)>(
        "SELECT rel.relname, trg.tgname
         FROM pg_trigger AS trg
         JOIN pg_class AS rel ON rel.oid = trg.tgrelid
         JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
         WHERE ns.nspname = 'public' AND NOT trg.tgisinternal",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取触发器契约失败：{error}"))?
    {
        contract.triggers.insert((relation, trigger));
    }

    for (identity, result) in sqlx::query_as::<_, (String, String)>(
        "SELECT proc.proname || '(' || pg_get_function_identity_arguments(proc.oid) || ')',
                pg_get_function_result(proc.oid) || '|' || md5(proc.prosrc)
         FROM pg_proc AS proc
         JOIN pg_namespace AS ns ON ns.oid = proc.pronamespace
         WHERE ns.nspname = 'public'",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取函数契约失败：{error}"))?
    {
        contract.functions.insert(identity, result);
    }

    for (name, base_type) in sqlx::query_as::<_, (String, String)>(
        "SELECT typ.typname, format_type(typ.typbasetype, typ.typtypmod)
         FROM pg_type AS typ
         JOIN pg_namespace AS ns ON ns.oid = typ.typnamespace
         WHERE ns.nspname = 'public' AND typ.typtype = 'd'",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取域契约失败：{error}"))?
    {
        contract.domains.insert(name, base_type);
    }

    for (name, label) in sqlx::query_as::<_, (String, String)>(
        "SELECT typ.typname, enm.enumlabel
         FROM pg_type AS typ
         JOIN pg_enum AS enm ON enm.enumtypid = typ.oid
         JOIN pg_namespace AS ns ON ns.oid = typ.typnamespace
         WHERE ns.nspname = 'public'",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取枚举契约失败：{error}"))?
    {
        contract.enums.entry(name).or_default().insert(label);
    }

    for (relation, grantee, privilege) in sqlx::query_as::<_, (String, String, String)>(
        "SELECT rel.relname, coalesce(role.rolname, 'PUBLIC'), acl.privilege_type
         FROM pg_class AS rel
         JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
         CROSS JOIN LATERAL aclexplode(coalesce(
             rel.relacl,
             acldefault(CASE WHEN rel.relkind = 'S' THEN 's'::\"char\" ELSE 'r'::\"char\" END,
                        rel.relowner)
         )) AS acl
         LEFT JOIN pg_roles AS role ON role.oid = acl.grantee
         WHERE ns.nspname = 'public'
           AND rel.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取关系权限契约失败：{error}"))?
    {
        contract.relation_privileges.insert((relation, grantee, privilege));
    }

    for (relation, column, grantee, privilege) in
        sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT rel.relname, att.attname, coalesce(role.rolname, 'PUBLIC'), acl.privilege_type
             FROM pg_attribute AS att
             JOIN pg_class AS rel ON rel.oid = att.attrelid
             JOIN pg_namespace AS ns ON ns.oid = rel.relnamespace
             CROSS JOIN LATERAL aclexplode(att.attacl) AS acl
             LEFT JOIN pg_roles AS role ON role.oid = acl.grantee
             WHERE ns.nspname = 'public'
               AND att.attnum > 0
               AND NOT att.attisdropped
               AND att.attacl IS NOT NULL",
        )
        .fetch_all(pool)
        .await
        .map_err(|error| format!("读取列权限契约失败：{error}"))?
    {
        contract.column_privileges.insert((relation, column, grantee, privilege));
    }

    for (identity, grantee, privilege) in sqlx::query_as::<_, (String, String, String)>(
        "SELECT proc.proname || '(' || pg_get_function_identity_arguments(proc.oid) || ')',
                coalesce(role.rolname, 'PUBLIC'), acl.privilege_type
         FROM pg_proc AS proc
         JOIN pg_namespace AS ns ON ns.oid = proc.pronamespace
         CROSS JOIN LATERAL aclexplode(coalesce(
             proc.proacl, acldefault('f'::\"char\", proc.proowner)
         )) AS acl
         LEFT JOIN pg_roles AS role ON role.oid = acl.grantee
         WHERE ns.nspname = 'public'",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| format!("读取函数权限契约失败：{error}"))?
    {
        contract.function_privileges.insert((identity, grantee, privilege));
    }

    Ok(contract)
}

/// 比较基线对象在迁移后是否被收缩；新增对象不参与。
fn contract_violations(baseline: &SchemaContract, current: &SchemaContract) -> Vec<String> {
    let mut problems = Vec::new();

    for (name, shape) in &baseline.relations {
        let Some(after) = current.relations.get(name) else {
            problems.push(format!("关系 public.{name} 被删除"));
            continue;
        };
        if after.kind != shape.kind {
            problems.push(format!("关系 public.{name} 类型 {} → {}", shape.kind, after.kind));
        }
        if !shape.row_security && after.row_security {
            problems.push(format!("关系 public.{name} 新增行级安全"));
        }
        if !shape.force_row_security && after.force_row_security {
            problems.push(format!("关系 public.{name} 新增强制行级安全"));
        }
    }

    for (relation, columns) in &baseline.columns {
        let Some(after) = current.columns.get(relation) else { continue };
        for (column, shape) in columns {
            let Some(updated) = after.get(column) else {
                problems.push(format!("列 public.{relation}.{column} 被删除或重命名"));
                continue;
            };
            if updated.data_type != shape.data_type {
                problems.push(format!(
                    "列 public.{relation}.{column} 类型 {} → {}",
                    shape.data_type, updated.data_type
                ));
            }
            if !shape.not_null && updated.not_null {
                problems.push(format!(
                    "列 public.{relation}.{column} 新增 NOT NULL，旧写入省略该列时会被拒绝"
                ));
            }
        }
    }

    // 既有表上的新列：旧代码插入时不会提供值，因此新增 NOT NULL 且无默认值即为破坏性。
    for (relation, baseline_columns) in &baseline.columns {
        let Some(after) = current.columns.get(relation) else { continue };
        for (column, shape) in after {
            if baseline_columns.contains_key(column) || !shape.not_null {
                continue;
            }
            let has_default = current
                .defaults
                .get(relation)
                .is_some_and(|defaults| defaults.contains_key(column));
            if !has_default {
                problems.push(format!(
                    "表 public.{relation} 新增 NOT NULL 列 {column} 且无默认值，旧写入会被拒绝"
                ));
            }
        }
    }

    for (relation, defaults) in &baseline.defaults {
        for (column, expression) in defaults {
            match current.defaults.get(relation).and_then(|after| after.get(column)) {
                Some(after) if after == expression => {}
                Some(after) => problems
                    .push(format!("列 public.{relation}.{column} 默认值 {expression} → {after}")),
                None => problems
                    .push(format!("列 public.{relation}.{column} 默认值 {expression} 被移除")),
            }
        }
    }

    for (relation, constraints) in &current.constraints {
        let before = baseline.constraints.get(relation);
        for (name, shape) in constraints {
            match before.and_then(|baseline| baseline.get(name)) {
                Some(baseline) if baseline == shape => {}
                Some(baseline) => problems.push(format!(
                    "约束 public.{relation}.{name} 定义改变：{} → {}",
                    baseline.definition, shape.definition
                )),
                None => {
                    // 只对既有表上的新增约束报警；新列上的约束（例如 089 的新列 CHECK）
                    // 不影响旧写入，因为旧代码省略新列时取 NULL。
                    let Some(baseline_columns) = baseline.columns.get(relation) else {
                        continue;
                    };
                    if !matches!(shape.kind.as_str(), "c" | "u" | "p" | "f" | "x") {
                        continue;
                    }
                    let touches_legacy_column = shape.columns.is_empty()
                        || shape.columns.iter().any(|column| baseline_columns.contains_key(column));
                    if touches_legacy_column {
                        problems.push(format!(
                            "关系 public.{relation} 新增约束 {name}（{}）：{}",
                            shape.kind, shape.definition
                        ));
                    }
                }
            }
        }
    }

    for (relation, trigger) in &current.triggers {
        if baseline.relations.contains_key(relation)
            && !baseline.triggers.contains(&(relation.clone(), trigger.clone()))
        {
            problems.push(format!("关系 public.{relation} 新增触发器 {trigger}，旧写入可能被拒绝"));
        }
    }

    for (identity, result) in &baseline.functions {
        match current.functions.get(identity) {
            None => problems.push(format!("函数 {identity} 被删除")),
            Some(after) if after != result => {
                // 迁移/授权专用 helper 由后续迁移反复 CREATE OR REPLACE，旧程序运行时不调用；
                // 只要求它仍然存在且返回类型不变，函数体属于迁移工具面。
                if !MIGRATION_TOOLING_FUNCTIONS.contains(&identity.as_str()) {
                    problems.push(format!(
                        "函数 {identity} 的返回类型或函数体被改写：{result} → {after}"
                    ));
                }
            }
            Some(_) => {}
        }
    }

    for (name, base_type) in &baseline.domains {
        match current.domains.get(name) {
            None => problems.push(format!("域 public.{name} 被删除")),
            Some(after) if after != base_type => {
                problems.push(format!("域 public.{name} 基类型 {base_type} → {after}"));
            }
            Some(_) => {}
        }
    }

    for (name, labels) in &baseline.enums {
        let Some(after) = current.enums.get(name) else {
            problems.push(format!("枚举 public.{name} 被删除"));
            continue;
        };
        for label in labels {
            if !after.contains(label) {
                problems.push(format!("枚举 public.{name} 取值 {label} 被移除"));
            }
        }
    }

    for (object, grantee, privilege) in &baseline.relation_privileges {
        if !current.relation_privileges.contains(&(
            object.clone(),
            grantee.clone(),
            privilege.clone(),
        )) {
            problems.push(format!("关系 public.{object} 对 {grantee} 的 {privilege} 权限被收回"));
        }
    }

    for (relation, column, grantee, privilege) in &baseline.column_privileges {
        if !current.column_privileges.contains(&(
            relation.clone(),
            column.clone(),
            grantee.clone(),
            privilege.clone(),
        )) {
            problems.push(format!(
                "列 public.{relation}.{column} 对 {grantee} 的 {privilege} 权限被收回"
            ));
        }
    }

    for (identity, grantee, privilege) in &baseline.function_privileges {
        if !current.function_privileges.contains(&(
            identity.clone(),
            grantee.clone(),
            privilege.clone(),
        )) {
            problems.push(format!("函数 {identity} 对 {grantee} 的 {privilege} 权限被收回"));
        }
    }

    problems
}

/// 对基线的每个表执行旧版“只给必需列以外全部取默认值”的写入，记录结果签名。
/// 在事务内执行并回滚，不留下测试数据。
async fn probe_legacy_writes(
    pool: &PgPool,
    baseline: &SchemaContract,
) -> Result<BTreeMap<String, String>, String> {
    let mut signatures = BTreeMap::new();
    for (relation, shape) in &baseline.relations {
        if !matches!(shape.kind.as_str(), "r" | "p") {
            continue;
        }
        let signature = probe_default_insert(pool, relation).await?;
        signatures.insert(relation.clone(), signature);
    }
    Ok(signatures)
}

async fn probe_default_insert(pool: &PgPool, relation: &str) -> Result<String, String> {
    let statement = format!("INSERT INTO public.{} DEFAULT VALUES", quote_identifier(relation));
    let mut transaction =
        pool.begin().await.map_err(|error| format!("开启写入探针事务失败：{error}"))?;
    let outcome = sqlx::raw_sql(AssertSqlSafe(statement)).execute(&mut *transaction).await;
    let signature = match outcome {
        Ok(_) => "ok".to_owned(),
        Err(error) => match error.as_database_error() {
            Some(database) => format!(
                "{}|{}|{}",
                database.code().unwrap_or_default(),
                database.constraint().unwrap_or("-"),
                database
                    .try_downcast_ref::<sqlx::postgres::PgDatabaseError>()
                    .and_then(sqlx::postgres::PgDatabaseError::column)
                    .unwrap_or("-")
            ),
            None => "driver-error".to_owned(),
        },
    };
    // 语句失败会让事务进入 aborted 状态；回滚才能让下一个探针继续。
    transaction.rollback().await.map_err(|error| format!("回滚写入探针事务失败：{error}"))?;
    Ok(signature)
}

async fn run_legacy_workload(pool: &PgPool) -> Result<BTreeMap<String, String>, String> {
    let mut transaction =
        pool.begin().await.map_err(|error| format!("开启旧版语句事务失败：{error}"))?;
    let mut digests = BTreeMap::new();
    for step in LEGACY_WORKLOAD {
        let digest = if step.query {
            let value: Option<String> = sqlx::query_scalar(step.sql)
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| format!("旧版语句 {} 查询失败：{error}", step.label))?;
            value.unwrap_or_else(|| "<null>".to_owned())
        } else {
            let result = sqlx::query(step.sql)
                .execute(&mut *transaction)
                .await
                .map_err(|error| format!("旧版语句 {} 写入失败：{error}", step.label))?;
            format!("rows:{}", result.rows_affected())
        };
        digests.insert(step.label.to_owned(), digest);
    }
    // 固定重放不落库：下一级 schema 继续使用同一测试库。
    transaction.rollback().await.map_err(|error| format!("回滚旧版语句事务失败：{error}"))?;
    Ok(digests)
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
