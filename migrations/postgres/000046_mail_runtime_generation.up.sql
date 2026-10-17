-- 全局 mail_cookie owner 的跨进程事务 fence；不按凭据或世界拆分。
-- 不预填成功 generation；每个取得 advisory ownership 的新版进程必须先推进并提交。
CREATE TABLE public.mail_runtime_generation (
    scope text PRIMARY KEY CHECK (scope = 'mail_cookie'),
    generation bigint NOT NULL CHECK (generation > 0),
    activated_at timestamptz NOT NULL
);
COMMENT ON TABLE public.mail_runtime_generation IS '邮件运行时持久 generation；内部事务锁行至提交，新 owner 推进后才可外呼';
