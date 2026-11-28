-- PM 提醒的发送围栏只允许单向推进：结果未知/已完成/已退役的行不能重新开始发送。
-- owner: auction-storage
--
-- 不变量：
--   1. `sending_at` 一经写入不可改、sender 不可换（000042 已强制）。
--   2. 000042 的守卫只拒绝离开 `sending`/`succeeded`/`manual_reconcile` 的状态集合，
--      `succeeded → sending` 与 `manual_reconcile → sending` 仍可写入：回到 `sending` 会重新挂上
--      发送租约并再次外写已结束的论坛私信。
--   3. 已产生外部结果的 `sending`/`manual_reconcile`/`succeeded`/`suppressed` 行不得重新进入
--      `sending`；`succeeded`/`suppressed` 为终态，不得再迁移。`pending`/`claimed` 仍未外写，
--      存储层只会从 `claimed` 开始发送（state.rs），库层不因夹具直接置位而拒绝。
--   4. 只替换守卫函数，不改写任何行、索引或既有约束。
--
-- 等级：breaking。新增会被拒绝的状态回退写入，须与同版程序在停机门禁下切换。

CREATE OR REPLACE FUNCTION public.guard_lot_watch_pm_sending()
RETURNS trigger
LANGUAGE plpgsql
AS $function$
BEGIN
    IF NEW.status = 'sending'
       AND OLD.status IN ('sending', 'manual_reconcile', 'succeeded', 'suppressed') THEN
        RAISE EXCEPTION '已结束或结果未知的 PM 不能重新开始发送' USING ERRCODE = '23514';
    END IF;
    IF OLD.status IN ('succeeded', 'suppressed')
       AND NEW.status IS DISTINCT FROM OLD.status THEN
        RAISE EXCEPTION 'PM 终态不可回退' USING ERRCODE = '23514';
    END IF;
    IF OLD.sending_at IS NOT NULL AND
       (NEW.sending_at IS DISTINCT FROM OLD.sending_at OR
        NEW.sender_forum_account_id IS DISTINCT FROM OLD.sender_forum_account_id OR
        NEW.status NOT IN ('sending','succeeded','manual_reconcile')) THEN
        RAISE EXCEPTION 'PM sending fence is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$function$;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (141, 'breaking');
