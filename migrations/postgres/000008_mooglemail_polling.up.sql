-- MoogleMail Worker 的租约、失败退避和 Admin 立即触发边界。
--
-- 轮询状态仍由 mail_poll_state 单表拥有；claim 字段只用于跨进程 fencing，不能进入 Admin
-- 或浏览器投影。入站事实、cursor 和租约释放由 Worker 的同一事务提交。
ALTER TABLE public.mail_poll_state
    ADD COLUMN claim_token uuid,
    ADD COLUMN claim_owner text
        CHECK (claim_owner IS NULL OR (
            claim_owner = btrim(claim_owner)
            AND claim_owner <> ''
            AND char_length(claim_owner) <= 256
        )),
    ADD COLUMN claim_expires_at timestamptz,
    ADD COLUMN last_failure_code text
        CHECK (last_failure_code IS NULL OR (
            last_failure_code = btrim(last_failure_code)
            AND last_failure_code <> ''
            AND char_length(last_failure_code) <= 128
        )),
    ADD COLUMN last_failure_detail text
        CHECK (last_failure_detail IS NULL OR (
            last_failure_detail = btrim(last_failure_detail)
            AND last_failure_detail <> ''
            AND char_length(last_failure_detail) <= 512
        )),
    ADD COLUMN last_failure_at timestamptz,
    ADD CONSTRAINT mail_poll_state_claim_fields_check CHECK (
        (claim_token IS NULL AND claim_owner IS NULL AND claim_expires_at IS NULL)
        OR (claim_token IS NOT NULL AND claim_owner IS NOT NULL AND claim_expires_at IS NOT NULL)
    );

ALTER TABLE public.mail_admin_commands
    DROP CONSTRAINT IF EXISTS mail_admin_commands_command_kind_check;

ALTER TABLE public.mail_admin_commands
    ADD CONSTRAINT mail_admin_commands_command_kind_check
        CHECK (command_kind IN ('set_polling_paused', 'trigger_poll'));
