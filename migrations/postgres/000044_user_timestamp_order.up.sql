-- Users owner：只修复可由 MM 注册账本与论坛身份历史共同证明的快照时间倒退。
-- 受控停机后执行；不猜测远端时区、不改创建时间、凭据、邮件原始时间或历史命令/审计。
LOCK TABLE public.users IN SHARE ROW EXCLUSIVE MODE;

WITH candidates AS MATERIALIZED (
    SELECT account.id, account.created_at, account.updated_at, account.state_version
    FROM public.users AS account
    WHERE account.updated_at < account.created_at
      AND EXISTS (
          SELECT 1 FROM public.user_mail_commands AS command
          WHERE command.user_id = account.id
            AND command.command_kind = 'register'
            AND command.result_code = 'registered'
            AND command.created_at = account.created_at
      )
      AND EXISTS (
          SELECT 1 FROM public.forum_identity_history AS history
          WHERE history.user_id = account.id
            AND history.created_at = account.updated_at
      )
), repaired AS (
    UPDATE public.users AS account
    SET updated_at = GREATEST(candidates.created_at, clock_timestamp()),
        state_version = candidates.state_version + 1
    FROM candidates
    WHERE account.id = candidates.id
    RETURNING account.id, candidates.created_at,
              candidates.updated_at AS previous_updated_at,
              candidates.state_version AS previous_state_version,
              account.updated_at, account.state_version
)
INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
SELECT gen_random_uuid(), 'system', 'migration-000044', 'user.timestamp_order_repaired',
       'user', id::text, 'success', 'system',
       jsonb_build_object(
           'migration_version', 44,
           'reason', 'mail_registration_forum_update_clock_order',
           'created_at', created_at,
           'previous_updated_at', previous_updated_at,
           'updated_at', updated_at,
           'previous_state_version', previous_state_version,
           'state_version', state_version
       )
FROM repaired;

-- 未能证明来源的倒退行使整个迁移回滚；不能跳过坏行或放宽 Admin 的严格投影校验。
ALTER TABLE public.users
    ADD CONSTRAINT users_timestamp_order CHECK (updated_at >= created_at);
