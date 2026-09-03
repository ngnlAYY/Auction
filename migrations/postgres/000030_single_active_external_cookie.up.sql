-- 不能由迁移静默选择保留哪条 Cookie；部署者必须先通过 Admin 命令停用重复项并留下审计。
DO $$
BEGIN
    IF EXISTS (
        SELECT credential_kind
        FROM public.credential_keys
        WHERE status = 'active'
          AND credential_kind IN ('mail_cookie', 'forum_cookie')
        GROUP BY credential_kind
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'each external cookie kind may have only one active credential'
            USING ERRCODE = '23505';
    END IF;
END;
$$;

-- 最终唯一性由数据库保护，避免并发导入或重新启用绕过应用层检查。
CREATE UNIQUE INDEX credential_keys_one_active_cookie_kind_idx
    ON public.credential_keys (credential_kind)
    WHERE status = 'active'
      AND credential_kind IN ('mail_cookie', 'forum_cookie');
