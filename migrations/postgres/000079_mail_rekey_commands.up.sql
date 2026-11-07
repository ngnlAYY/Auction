-- users owner：登录 key 自助轮换（`rekey` 邮件主题）需要新的命令种类与结果码，
-- 并允许入站邮件以 `rekey` 种类记录处理决定。
-- 不变量：轮换命令、结果与入站种类只追加记录；旧程序不认识 rekey 结果，必须停机迁移。
-- 放宽 CHECK 后新程序会写入旧程序拒绝的状态，因此按 breaking 登记。
LOCK TABLE public.user_mail_commands, public.inbound_mail
IN SHARE ROW EXCLUSIVE MODE;

-- 000004 的三个 CHECK 未命名，名称由 PostgreSQL 按建表顺序生成；按约束定义定位，
-- 不把自动命名当作迁移契约。基线被改写时迁移失败关闭。
DO $$
DECLARE
    constraint_record record;
    dropped integer := 0;
BEGIN
    FOR constraint_record IN
        SELECT catalog_constraint.conname
        FROM pg_catalog.pg_constraint AS catalog_constraint
        WHERE catalog_constraint.conrelid = 'public.user_mail_commands'::regclass
          AND catalog_constraint.contype = 'c'
          AND (
              pg_get_constraintdef(catalog_constraint.oid) LIKE '%command_kind%'
              OR pg_get_constraintdef(catalog_constraint.oid) LIKE '%result_code%'
              OR pg_get_constraintdef(catalog_constraint.oid) LIKE '%user_id IS NOT NULL%'
          )
    LOOP
        EXECUTE format('ALTER TABLE public.user_mail_commands DROP CONSTRAINT %I', constraint_record.conname);
        dropped := dropped + 1;
    END LOOP;
    IF dropped < 3 THEN
        RAISE EXCEPTION '未找到 000004 的 user_mail_commands 约束';
    END IF;
END
$$;

ALTER TABLE public.user_mail_commands
    ADD CONSTRAINT user_mail_commands_command_kind_check
    CHECK (command_kind IN ('register', 'rebind', 'rekey'));

ALTER TABLE public.user_mail_commands
    ADD CONSTRAINT user_mail_commands_result_code_check
    CHECK (
        (command_kind = 'register' AND result_code IN (
            'registered', 'recovered', 'blacklisted', 'identity_conflict'
        ))
        OR (command_kind = 'rebind' AND result_code IN (
            'rebound', 'blacklisted', 'identity_conflict', 'unchanged', 'invalid_key'
        ))
        OR (command_kind = 'rekey' AND result_code IN (
            'rekeyed', 'blacklisted', 'identity_conflict', 'not_registered'
        ))
    );

ALTER TABLE public.user_mail_commands
    ADD CONSTRAINT user_mail_commands_user_id_check
    CHECK (
        user_id IS NOT NULL
        OR (command_kind = 'rebind' AND result_code = 'invalid_key')
        OR (command_kind = 'register' AND result_code = 'identity_conflict')
        OR (command_kind = 'rekey' AND result_code IN ('identity_conflict', 'not_registered'))
    );

-- 入站邮件种类与业务决定一致：rekey 是独立的账号命令，不能伪装成 registration 或 rebind。
DO $$
DECLARE
    constraint_record record;
    dropped integer := 0;
BEGIN
    FOR constraint_record IN
        SELECT catalog_constraint.conname
        FROM pg_catalog.pg_constraint AS catalog_constraint
        WHERE catalog_constraint.conrelid = 'public.inbound_mail'::regclass
          AND catalog_constraint.contype = 'c'
          AND pg_get_constraintdef(catalog_constraint.oid) LIKE '%mail_kind%'
    LOOP
        EXECUTE format('ALTER TABLE public.inbound_mail DROP CONSTRAINT %I', constraint_record.conname);
        dropped := dropped + 1;
    END LOOP;
    IF dropped < 1 THEN
        RAISE EXCEPTION '未找到 000002 的 inbound_mail mail_kind 约束';
    END IF;
END
$$;

ALTER TABLE public.inbound_mail
    ADD CONSTRAINT inbound_mail_mail_kind_check
    CHECK (
        mail_kind IN ('equipment', 'bid', 'delivery', 'registration', 'rebind', 'rekey', 'unknown')
    );

-- 轮换回信属于账号类发送，与注册/换绑共用 account 发送节奏；否则会落入 intake 分类。
CREATE OR REPLACE FUNCTION public.delivery_send_category(origin text, payload jsonb)
RETURNS text LANGUAGE sql IMMUTABLE PARALLEL SAFE AS $$
    SELECT CASE
        WHEN payload ->> 'operation' IS DISTINCT FROM 'send' THEN NULL
        WHEN origin = 'lot_watch' THEN 'outbid'
        WHEN origin = 'operations_alert' THEN 'operations'
        WHEN origin = 'mail_intake' AND payload ->> 'template'
            IN (
                'register_key', 'register_result', 'rekey_key', 'rekey_result',
                'rebind_result', 'user_assets_rejected'
            )
            THEN 'account'
        WHEN jsonb_typeof(payload -> 'equipment_secret_refs') = 'array'
             AND payload -> 'equipment_secret_refs' <> '[]'::jsonb THEN 'equipment'
        WHEN jsonb_typeof(payload -> 'attachment_ids') = 'array'
             AND payload -> 'attachment_ids' <> '[]'::jsonb THEN 'equipment'
        WHEN origin IN ('settlement', 'settlement_recovery_action') THEN 'settlement'
        WHEN origin = 'mail_intake' THEN 'intake'
        ELSE 'manual'
    END
$$;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (79, 'breaking');
