-- 检查 auction_runtime 的继承权限、对象 ownership、PUBLIC 授权与所有对象创建者默认 ACL。
-- owner: auction-storage
-- Breaking：收紧运行角色权限并替换 SECURITY DEFINER 授权入口；升级前须停止旧角色。
LOCK TABLE
    public.auction_commands,
    public.auction_settlements,
    public.auctions,
    public.bid_proxy_authorization_transitions,
    public.bid_proxy_authorizations,
    public.compensation_pool_reversals,
    public.delivery_external_quarantines,
    public.delivery_send_attempts,
    public.delivery_sent_bindings,
    public.deliveries,
    public.inbound_mail,
    public.lot_settlements,
    public.lots,
    public.mailbox_sweep_cursors,
    public.realtime_events,
    public.runtime_switch_events,
    public.scheduled_rounds,
    public.settlement_funding_entries,
    public.settlement_funding_sources,
    public.settlement_legacy_bot_seller_claims,
    public.settlement_obligation_fulfillments,
    public.settlement_obligations,
    public.settlement_recoveries,
    public.settlement_recovery_actions,
    public.settlement_recovery_commands,
    public.settlement_recovery_return_consumptions,
    public.settlement_return_actions,
    public.settlement_return_cases,
    public.settlement_return_evidence,
    public.settlement_return_events,
    public.settlement_return_takes,
    public.settlement_return_verifications,
    public.user_mail_commands
IN SHARE ROW EXCLUSIVE MODE;


-- 000081/000088/000096 的直接 ACL 重建不能清除继承组权限、其它 creator 的默认 ACL 或函数旧授权。
ALTER FUNCTION public.apply_auction_runtime_privileges()
    RENAME TO apply_auction_runtime_privileges_000117;

DO $block$
DECLARE
    owner_oid oid;
    grantee record;
BEGIN
    SELECT procedure.proowner INTO owner_oid
    FROM pg_catalog.pg_proc AS procedure
    WHERE procedure.oid =
        'public.apply_auction_runtime_privileges_000117()'::pg_catalog.regprocedure;
    IF owner_oid IS NULL OR owner_oid <> current_user::regrole::oid THEN
        RAISE EXCEPTION '运行时权限函数必须由当前迁移 owner 拥有'
            USING ERRCODE = '42501';
    END IF;
    EXECUTE 'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000117() FROM PUBLIC';
    FOR grantee IN
        SELECT role.rolname
        FROM pg_catalog.pg_roles AS role
        WHERE role.oid <> owner_oid
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000117() FROM %I',
            grantee.rolname
        );
    END LOOP;
END
$block$;

CREATE FUNCTION public.apply_auction_runtime_privileges()
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $function$
DECLARE
    runtime_oid oid;
    function_owner oid;
    database_oid oid;
    public_schema oid := 'public'::pg_catalog.regnamespace;
    creator record;
    grantee record;
BEGIN
    SELECT role.oid INTO runtime_oid
    FROM pg_catalog.pg_roles AS role
    WHERE role.rolname = 'auction_runtime';
    SELECT procedure.proowner INTO function_owner
    FROM pg_catalog.pg_proc AS procedure
    WHERE procedure.oid = 'public.apply_auction_runtime_privileges()'::pg_catalog.regprocedure;
    SELECT database.oid INTO database_oid
    FROM pg_catalog.pg_database AS database
    WHERE database.datname = current_database();
    IF runtime_oid IS NULL OR function_owner IS NULL OR database_oid IS NULL THEN
        RAISE EXCEPTION '运行时角色或迁移 owner 缺失' USING ERRCODE = '42501';
    END IF;
    IF runtime_oid = function_owner THEN
        RAISE EXCEPTION '迁移 owner 与 auction_runtime 必须是不同角色' USING ERRCODE = '42501';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_catalog.pg_roles AS role
        WHERE role.oid = runtime_oid
          AND (role.rolsuper OR role.rolcreatedb OR role.rolcreaterole
               OR role.rolbypassrls OR role.rolreplication OR role.rolcanlogin IS FALSE)
    ) THEN
        RAISE EXCEPTION 'auction_runtime 带有禁止的角色属性' USING ERRCODE = '42501';
    END IF;

    -- auction_runtime 自身不能拥有数据库对象，也不能经 PUBLIC 获得数据库 CREATE。
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_database AS database
        WHERE database.datname = current_database()
          AND (database.datdba = runtime_oid
               OR pg_catalog.has_database_privilege(runtime_oid, database.oid, 'CREATE'))
    ) OR EXISTS (
        SELECT 1
        FROM pg_catalog.pg_shdepend AS dependency
        WHERE dependency.refclassid = 'pg_catalog.pg_authid'::regclass
          AND dependency.refobjid = runtime_oid
          AND dependency.deptype = 'o'
          AND (dependency.dbid = 0::oid OR dependency.dbid = database_oid)
    ) THEN
        RAISE EXCEPTION 'auction_runtime 拥有数据库对象或拥有数据库 CREATE 权限'
            USING ERRCODE = '42501';
    END IF;
    REVOKE CREATE ON SCHEMA public FROM PUBLIC;
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS relation
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = relation.relnamespace
        CROSS JOIN LATERAL pg_catalog.aclexplode(COALESCE(
            relation.relacl,
            pg_catalog.acldefault(
                CASE WHEN relation.relkind = 'S' THEN 's'::"char" ELSE 'r'::"char" END,
                relation.relowner
            )
        )) AS acl
        WHERE namespace.nspname = 'public'
          AND acl.grantee = 0
          AND acl.privilege_type IN ('INSERT', 'UPDATE', 'DELETE', 'TRUNCATE', 'REFERENCES', 'TRIGGER')
    ) THEN
        RAISE EXCEPTION 'PUBLIC 对 public 对象具有写权限；请先人工收紧授权'
            USING ERRCODE = '42501';
    END IF;

    -- 运行账号可继承的角色不能带 DDL/降权、对象 ownership、schema CREATE 或表写入权限。
    IF EXISTS (
        SELECT 1
        FROM pg_catalog.pg_roles AS granted_role
        WHERE granted_role.oid <> runtime_oid
          AND pg_catalog.pg_has_role(runtime_oid, granted_role.oid, 'MEMBER')
          AND (
              granted_role.rolsuper OR granted_role.rolcreatedb OR granted_role.rolcreaterole
              OR granted_role.rolbypassrls OR granted_role.rolreplication
              OR granted_role.oid = (SELECT database.datdba FROM pg_catalog.pg_database AS database
                                     WHERE database.datname = current_database())
              OR EXISTS (
                  SELECT 1
                  FROM pg_catalog.pg_shdepend AS dependency
                  WHERE dependency.refclassid = 'pg_catalog.pg_authid'::regclass
                    AND dependency.refobjid = granted_role.oid
                    AND dependency.deptype = 'o'
                    AND (dependency.dbid = 0::oid OR dependency.dbid = database_oid)
              )
              OR pg_catalog.has_schema_privilege(granted_role.oid, public_schema, 'CREATE')
              OR pg_catalog.has_database_privilege(
                     granted_role.oid, current_database(), 'CREATE'
                 )
              OR EXISTS (
                  SELECT 1
                  FROM pg_catalog.pg_class AS relation
                  JOIN pg_catalog.pg_namespace AS namespace
                    ON namespace.oid = relation.relnamespace
                  WHERE namespace.nspname = 'public'
                    AND relation.relkind IN ('r', 'p', 'v', 'm', 'f')
                    AND (
                        pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'INSERT')
                        OR pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'UPDATE')
                        OR pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'DELETE')
                        OR pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'TRUNCATE')
                        OR pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'REFERENCES')
                        OR pg_catalog.has_table_privilege(granted_role.oid, relation.oid, 'TRIGGER')
                        OR pg_catalog.has_any_column_privilege(granted_role.oid, relation.oid, 'INSERT')
                        OR pg_catalog.has_any_column_privilege(granted_role.oid, relation.oid, 'UPDATE')
                        OR pg_catalog.has_any_column_privilege(granted_role.oid, relation.oid, 'REFERENCES')
                    )
              )
          )
    ) THEN
        RAISE EXCEPTION
            'auction_runtime 继承了禁止的对象 ownership、DDL 或写权限；请修复角色 membership 后重试'
            USING ERRCODE = '42501';
    END IF;

    -- 默认 ACL 逐一覆盖可在 public 建对象的非 superuser creator；owner 无法管理的 creator 失败关闭。
    FOR creator IN
        SELECT role.oid, role.rolname
        FROM pg_catalog.pg_roles AS role
        WHERE NOT role.rolsuper
          AND role.rolname <> 'pg_database_owner'
          AND pg_catalog.has_schema_privilege(role.oid, public_schema, 'CREATE')
        ORDER BY role.oid
    LOOP
        IF creator.oid <> function_owner
           AND NOT pg_catalog.pg_has_role(function_owner, creator.oid, 'MEMBER') THEN
            RAISE EXCEPTION '迁移 owner 不能为 public object creator % 设置默认 ACL', creator.rolname
                USING ERRCODE = '42501';
        END IF;
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE ALL ON TABLES FROM auction_runtime',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE ALL ON SEQUENCES FROM auction_runtime',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON TABLES FROM auction_runtime',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON SEQUENCES FROM auction_runtime',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE ALL ON TABLES FROM PUBLIC',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I REVOKE ALL ON SEQUENCES FROM PUBLIC',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public GRANT SELECT ON TABLES TO auction_runtime',
            creator.rolname
        );
        EXECUTE format(
            'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO auction_runtime',
            creator.rolname
        );
    END LOOP;

    PERFORM public.apply_auction_runtime_privileges_000117();
    -- 旧授权函数仍维护兼容写入矩阵；收紧不能删除的账本权限并保留所需更新面。
    REVOKE UPDATE, DELETE ON TABLE public.settlement_funding_entries FROM auction_runtime;
    GRANT INSERT ON TABLE public.settlement_funding_entries TO auction_runtime;
    REVOKE DELETE ON TABLE public.operations_alert_webhook_deliveries FROM auction_runtime;
    GRANT INSERT, UPDATE ON TABLE public.operations_alert_webhook_deliveries TO auction_runtime;

    -- CREATE OR REPLACE 保留旧 ACL；只 owner 保留执行权限，含直接授予的角色一律收回。
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
    FOR grantee IN
        SELECT role.rolname
        FROM pg_catalog.pg_roles AS role
        WHERE role.oid <> function_owner
    LOOP
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM %I',
            grantee.rolname
        );
        EXECUTE format(
            'REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000117() FROM %I',
            grantee.rolname
        );
    END LOOP;
    REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges_000117() FROM PUBLIC;
END
$function$;

REVOKE ALL ON FUNCTION public.apply_auction_runtime_privileges() FROM PUBLIC;
SELECT public.apply_auction_runtime_privileges();
INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (122, 'breaking');
