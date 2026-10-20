-- 详情 GET 会把无附件 Inbox 邮件标为已读。先登记列表身份，崩溃后仍可按 MID 恢复。
-- 仅持久保存安全摘要；正文、HTML、Cookie、mmtoken 和装备 key 不进入该队列。
CREATE TABLE public.mail_poll_pending_reads (
    read_sequence bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    mailbox_key text NOT NULL REFERENCES public.mail_poll_state(mailbox_key) ON DELETE RESTRICT,
    world_id text NOT NULL CHECK (world_id IN ('main', 'isekai')),
    external_mail_id text NOT NULL
        CHECK (external_mail_id = btrim(external_mail_id)
               AND octet_length(external_mail_id) BETWEEN 1 AND 256
               AND external_mail_id !~ '[[:cntrl:]]'),
    sender text NOT NULL
        CHECK (sender = btrim(sender) AND octet_length(sender) BETWEEN 1 AND 256
               AND sender !~ '[[:cntrl:]]'),
    subject text NOT NULL
        CHECK (subject = btrim(subject) AND octet_length(subject) <= 512
               AND subject !~ '[[:cntrl:]]'),
    received_at timestamptz NOT NULL,
    staged_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    UNIQUE (mailbox_key, external_mail_id)
);

CREATE INDEX mail_poll_pending_reads_mailbox_order
    ON public.mail_poll_pending_reads (mailbox_key, read_sequence);
