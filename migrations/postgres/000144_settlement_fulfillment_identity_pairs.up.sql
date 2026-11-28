-- 结算履约的准备/激活身份必须成对出现，避免落库后无法领取的履约。
-- owner: auction-storage
--
-- 不变量：
--   1. `claim_validation` 要求 `prepare_command_id` 与 `prepare_request_digest` 同时存在，
--      `activate_command_id` 与 `activate_request_digest` 同时存在，且激活身份晚于准备身份。
--   2. 000002 的 CHECK 只校验摘要长度，命令 ID 与摘要可以各自单独出现：这样的行按领取
--      校验永远不可领取（`IncompletePrepareIdentity`/`IncompleteActivationIdentity`）。
--   3. 迁移只补成对性约束；不改状态机、不改写历史行，也不要求激活身份已经存在
--      （`prepared` 阶段只有准备身份）。`delivery_id` 与准备身份的关系不在本迁移收紧：
--      既有读取夹具会写入“有 delivery、无身份”的历史形状，需先审计存量。
--   4. 约束以 `NOT VALID` 加入：历史行（含旧程序写入）保持原样，新写入立即受约束。
--
-- 等级：breaking。新增会拒绝不完整身份的新写入，须与同版程序在停机门禁下切换。

ALTER TABLE public.settlement_obligation_fulfillments
    ADD CONSTRAINT settlement_fulfillment_prepare_identity_000144 CHECK (
        (prepare_command_id IS NULL) = (prepare_request_digest IS NULL)
    ) NOT VALID,
    ADD CONSTRAINT settlement_fulfillment_activate_identity_000144 CHECK (
        (activate_command_id IS NULL) = (activate_request_digest IS NULL)
        AND (activate_command_id IS NULL OR prepare_command_id IS NOT NULL)
    ) NOT VALID;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (144, 'breaking');
