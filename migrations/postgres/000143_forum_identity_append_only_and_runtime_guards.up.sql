-- 论坛身份历史只追加；授权入口补充 schema CREATE 与序列 UPDATE 的失败关闭检查。
-- owner: auction-storage
--
-- 不变量：
--   1. `forum_identity_history` 是论坛账号与玩家名的只追加历史（000029 建表注释）：行级拒绝
--      `UPDATE`/`DELETE`，语句级拒绝 `TRUNCATE`；运行账号只保留 `SELECT`、`INSERT`（000081 起
--      该表被放在可变表整表写授权里，与只追加语义矛盾）。
--   2. 授权入口每次执行（含 `pg_restore --no-privileges` 后重放）都重申该写入面，恢复重放不能
--      把历史重新变成可改。
--   3. `auction_runtime` 对 `public` 不得有直接 `CREATE`：000122 只撤销了 `PUBLIC` 的 `CREATE`
--      并拒绝继承角色的 `CREATE`，由其它授权者直接授予的运行账号权限逃过守卫；入口先尝试撤销，
--      仍存在时失败关闭。继承的 schema `CREATE` 已由 `apply_auction_runtime_privileges_000122()`
--      拒绝，这里覆盖的是运行账号自身的直接授权。
--   4. 序列写权限只影响 `setval`：000122 的继承角色检查只覆盖表，入口补一条对 `public` 序列
--      `UPDATE` 的失败关闭检查；入口无法撤销其它授权者的授予。
--
-- 等级：breaking。收紧既有写入面与授权入口，须与同版程序在停机门禁下切换。

LOCK TABLE public.forum_identity_history IN SHARE ROW EXCLUSIVE MODE;

CREATE FUNCTION public.reject_forum_identity_history_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $function$
BEGIN
    RAISE EXCEPTION 'forum identity history is append-only'
        USING ERRCODE = '55000';
END;
$function$;

CREATE TRIGGER forum_identity_history_append_only
BEFORE UPDATE OR DELETE ON public.forum_identity_history
FOR EACH ROW EXECUTE FUNCTION public.reject_forum_identity_history_mutation();
CREATE TRIGGER forum_identity_history_no_truncate
BEFORE TRUNCATE ON public.forum_identity_history
FOR EACH STATEMENT EXECUTE FUNCTION public.reject_forum_identity_history_mutation();

-- 沿用 000137 的改名链：新入口只追加本期收口，不复制既有授权矩阵。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000137;

CREATE FUNCTION public.apply_auction_runtime_privileges()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    function_owner oid;
    grantee record;
BEGIN
    SELECT procedure.proowner INTO function_owner
    FROM pg_catalog.pg_proc AS procedure
    WHERE procedure.oid =
        'public.apply_auction_runtime_privileges()'::pg_catalog.regprocedure;
    IF function_owner IS NULL OR function_owner <> current_user::regrole::oid THEN
        RAISE EXCEPTION '运行时权限入口必须由当前迁移 owner 拥有'
            USING ERRCODE = '42501';
    END IF;

    -- `pg_restore --no-privileges` 会把函数 ACL 还原为默认 PUBLIC EXECUTE；每次入口执行都重新收口。
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000137() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname FROM pg_catalog.pg_roles AS role WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000137() FROM %I',
            grantee.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000137();

    -- 只追加历史：入口重放不得把整表写权限重新授予运行账号。
    REVOKE INSERT, UPDATE, DELETE ON TABLE public.forum_identity_history FROM auction_runtime;
    GRANT SELECT, INSERT ON TABLE public.forum_identity_history TO auction_runtime;

    -- schema CREATE：先按 owner 能力撤销，仍不可撤销的直接授权失败关闭。
    IF pg_catalog.has_schema_privilege('auction_runtime', 'public', 'CREATE') THEN
        REVOKE CREATE ON SCHEMA public FROM auction_runtime;
    END IF;
    IF pg_catalog.has_schema_privilege('auction_runtime', 'public', 'CREATE') THEN
        RAISE EXCEPTION 'auction_runtime 仍可直接在 public 建对象；请由授权者撤销 CREATE'
            USING ERRCODE = '42501';
    END IF;

    -- 序列 UPDATE 只用于 setval；继承或其它授权者的授予无法由入口撤销，失败关闭。
    -- `CASE` 保证 `has_sequence_privilege` 只对序列求值：WHERE 不保证先过滤 relkind，
    -- 否则 TOAST 表等非序列 OID 会让查询本身报错（42809）。
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS relation
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        WHERE namespace.nspname = 'public'
          AND relation.relkind = 'S'
          AND CASE
                  WHEN relation.relkind = 'S'
                  THEN pg_catalog.has_sequence_privilege('auction_runtime', relation.oid, 'UPDATE')
                  ELSE false
              END
    ) THEN
        RAISE EXCEPTION 'auction_runtime 拥有 public 序列 UPDATE 权限；请先收紧授权'
            USING ERRCODE = '42501';
    END IF;
END
$function$;

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC, auction_runtime;
REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000137() FROM PUBLIC, auction_runtime;
SELECT public.apply_auction_runtime_privileges();

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (143, 'breaking');
