-- 资金账户以认证 UID/Main/Credits 稳定命名；Cookie 换代不清占用。
CREATE TABLE public.settlement_funding_scope (
    scope text PRIMARY KEY CHECK (scope = 'mail_cookie')
);
INSERT INTO public.settlement_funding_scope(scope) VALUES ('mail_cookie');

CREATE TABLE public.settlement_funding_accounts (
    uid text PRIMARY KEY CHECK (uid ~ '^[1-9][0-9]*$' AND length(uid) <= 19),
    cash_world text NOT NULL DEFAULT 'main' CHECK (cash_world = 'main'),
    currency text NOT NULL DEFAULT 'credits' CHECK (currency = 'credits'),
    status text NOT NULL DEFAULT 'unreconciled' CHECK (status IN ('unreconciled','ready','frozen')),
    state_version bigint NOT NULL DEFAULT 0 CHECK (state_version >= 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

-- source 是实际资产来源，不是经济 settlement_pools 的余额。
CREATE TABLE public.settlement_funding_sources (
    id uuid PRIMARY KEY,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    kind text NOT NULL CHECK (kind IN ('reconciliation','collected_payment','returned_delivery')),
    source_key uuid NOT NULL,
    amount bigint NOT NULL CHECK (amount >= 0),
    proof_digest bytea NOT NULL CHECK (octet_length(proof_digest) = 32),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE(kind,source_key)
);

-- 当前session精确收款绑定，邮件与payment均全局一次；旧collected不回填。
CREATE TABLE public.settlement_funding_receipts (
    source_id uuid PRIMARY KEY REFERENCES public.settlement_funding_sources(id),
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    payment_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id),
    delivery_id uuid NOT NULL UNIQUE REFERENCES public.deliveries(id),
    external_mail_id text NOT NULL CHECK (external_mail_id ~ '^[1-9][0-9]*$'),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'),
    UNIQUE(uid,external_mail_id)
);

CREATE TABLE public.settlement_funding_commands (
    command_id uuid PRIMARY KEY,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    action text NOT NULL,
    target_id uuid NOT NULL,
    request_digest bytea NOT NULL CHECK (octet_length(request_digest) = 32),
    result jsonb NOT NULL CHECK (jsonb_typeof(result) = 'object'),
    authorization jsonb NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(authorization) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

-- allocation 为用途余额，entry 保存所有来源及用途转移；零金额行不创造资产。
CREATE TABLE public.settlement_funding_allocations (
    id uuid PRIMARY KEY,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    purpose text NOT NULL CHECK (purpose IN ('proprietary','compensation','quarantine','seller','tax','retained','refund','carrier')),
    lot_id uuid REFERENCES public.lots(id),
    obligation_id uuid REFERENCES public.settlement_obligations(id),
    amount bigint NOT NULL CHECK (amount >= 0),
    state text NOT NULL DEFAULT 'available' CHECK (state IN ('available','reserved','in_flight','unknown','spent')),
    delivery_id uuid UNIQUE REFERENCES public.deliveries(id),
    state_version bigint NOT NULL DEFAULT 1 CHECK (state_version > 0),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    UNIQUE NULLS NOT DISTINCT(uid,purpose,lot_id),
    CHECK ((state = 'available') OR delivery_id IS NOT NULL)
);
CREATE UNIQUE INDEX settlement_funding_obligation_once ON public.settlement_funding_allocations(obligation_id)
    WHERE obligation_id IS NOT NULL;

CREATE TABLE public.settlement_funding_lots (
    lot_id uuid PRIMARY KEY REFERENCES public.lots(id),
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    payment_obligation_id uuid NOT NULL UNIQUE REFERENCES public.settlement_obligations(id),
    source_kind text NOT NULL CHECK (source_kind IN ('internal','collected')),
    disposition text NOT NULL DEFAULT 'seller' CHECK (disposition IN ('seller','retained','refund')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

CREATE TABLE public.settlement_funding_returns (
    delivery_id uuid PRIMARY KEY REFERENCES public.deliveries(id),
    command_id uuid NOT NULL UNIQUE,
    proof_digest bytea NOT NULL CHECK (octet_length(proof_digest) = 32),
    evidence jsonb NOT NULL CHECK (jsonb_typeof(evidence) = 'object'),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

-- 追加分录；外部源/外部支出分别用 NULL 借方/贷方，其余严格等额转移。
CREATE TABLE public.settlement_funding_entries (
    id uuid PRIMARY KEY,
    command_id uuid NOT NULL,
    uid text NOT NULL REFERENCES public.settlement_funding_accounts(uid),
    debit_id uuid REFERENCES public.settlement_funding_allocations(id),
    credit_id uuid REFERENCES public.settlement_funding_allocations(id),
    source_id uuid REFERENCES public.settlement_funding_sources(id),
    amount bigint NOT NULL CHECK (amount > 0),
    kind text NOT NULL CHECK (kind IN ('issue','transfer','reserve','in_flight','unknown','spent','return')),
    proof_digest bytea NOT NULL CHECK (octet_length(proof_digest) = 32),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    CHECK (debit_id IS NOT NULL OR credit_id IS NOT NULL),
    CHECK (kind <> 'issue' OR (source_id IS NOT NULL AND debit_id IS NULL))
);
CREATE INDEX settlement_funding_entries_account ON public.settlement_funding_entries(uid,created_at,id);
