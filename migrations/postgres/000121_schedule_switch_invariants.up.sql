-- 只接受真实 Gregorian 排期日期、数值型冻结恢复策略与可证明的开关过期目标。
-- owner: auction-storage
-- Breaking：拒绝非法策略快照与过期事件，升级前须停止 Worker。
LOCK TABLE
    public.auctions,
    public.runtime_switch_events,
    public.scheduled_rounds
IN SHARE ROW EXCLUSIVE MODE;

-- 000076 的 FK 只证明 expired_command_id 存在；过期必须引用同一 switch 的到期 set。
DO $block$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM public.runtime_switch_events AS expiry
        LEFT JOIN public.runtime_switch_events AS target
          ON target.command_id = expiry.expired_command_id
        WHERE expiry.operation = 'expire'
          AND (target.command_id IS NULL
               OR target.switch_name IS DISTINCT FROM expiry.switch_name
               OR target.operation IS DISTINCT FROM 'set'
               OR target.expires_at IS NULL
               OR target.expires_at > expiry.recorded_at
               OR target.revision IS DISTINCT FROM expiry.expected_revision
               OR EXISTS (
                   SELECT 1 FROM public.runtime_switch_events AS later
                   WHERE later.switch_name = expiry.switch_name
                     AND later.revision > target.revision
                     AND later.revision < expiry.revision
               ))
    ) THEN
        RAISE EXCEPTION '既有 runtime switch expire 事件不能唯一归属到已到期 set'
            USING ERRCODE = '23514';
    END IF;
END
$block$;

CREATE FUNCTION public.validate_runtime_switch_expiry_target_000121()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $function$
DECLARE
    target public.runtime_switch_events%ROWTYPE;
    latest_command uuid;
    latest_revision bigint;
BEGIN
    -- 同名追加由事务锁串行化；账本事实不可变，校验读取无需 FOR SHARE 所要求的 UPDATE 权限。
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended('runtime_switch:' || NEW.switch_name, 0)
    );
    IF NEW.operation <> 'expire' THEN
        RETURN NEW;
    END IF;
    SELECT * INTO target
    FROM public.runtime_switch_events AS event
    WHERE event.command_id = NEW.expired_command_id;
    IF NOT FOUND
       OR target.switch_name IS DISTINCT FROM NEW.switch_name
       OR target.operation IS DISTINCT FROM 'set'
       OR target.expires_at IS NULL
       OR target.expires_at > NEW.recorded_at
       OR target.revision IS DISTINCT FROM NEW.expected_revision THEN
        RAISE EXCEPTION 'runtime switch expire 必须引用同名且已到期的 set 事件'
            USING ERRCODE = '23514';
    END IF;
    SELECT event.command_id, event.revision
    INTO latest_command, latest_revision
    FROM public.runtime_switch_events AS event
    WHERE event.switch_name = NEW.switch_name
    ORDER BY event.revision DESC
    LIMIT 1;
    IF latest_command IS DISTINCT FROM target.command_id
       OR latest_revision IS DISTINCT FROM NEW.expected_revision THEN
        RAISE EXCEPTION 'runtime switch expire 不能越过更新的 switch 事实'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$function$;
CREATE TRIGGER runtime_switch_expiry_target_000121
BEFORE INSERT ON public.runtime_switch_events
FOR EACH ROW EXECUTE FUNCTION public.validate_runtime_switch_expiry_target_000121();
-- 000065/000066 的正则只证明 early-bird 日期形状；以 Gregorian 月长与闰年规则校验日历身份。
ALTER TABLE public.scheduled_rounds
    ADD CONSTRAINT scheduled_rounds_slot_calendar_date_000121 CHECK (
        CASE
            WHEN slot_key IS NULL THEN true
            WHEN slot_key ~ '^early-bird:[0-9]{4}-[0-9]{2}-[0-9]{2}:(0|[1-9][0-9]?)$' THEN
                CASE
                    WHEN substring(slot_key from 12 for 4)::integer > 0
                     AND substring(slot_key from 17 for 2)::integer BETWEEN 1 AND 12
                    THEN substring(slot_key from 20 for 2)::integer BETWEEN 1 AND
                        CASE substring(slot_key from 17 for 2)::integer
                            WHEN 2 THEN 28 + CASE
                                WHEN substring(slot_key from 12 for 4)::integer % 4 = 0
                                 AND (substring(slot_key from 12 for 4)::integer % 100 <> 0
                                      OR substring(slot_key from 12 for 4)::integer % 400 = 0)
                                THEN 1 ELSE 0 END
                            WHEN 4 THEN 30
                            WHEN 6 THEN 30
                            WHEN 9 THEN 30
                            WHEN 11 THEN 30
                            ELSE 31
                        END
                    ELSE false
                END
            ELSE false
        END
    ) NOT VALID;
ALTER TABLE public.scheduled_rounds VALIDATE CONSTRAINT scheduled_rounds_slot_calendar_date_000121;

-- 000067 比较 JSON text，未确认 version 与 timeout 的 JSON 类型为 number。
ALTER TABLE public.auctions
    ADD CONSTRAINT auctions_recovery_policy_numeric_types_000121 CHECK (
        CASE WHEN jsonb_typeof(policy_snapshot -> 'recovery') = 'object' THEN
            jsonb_typeof(policy_snapshot #> '{recovery,version}') = 'number'
            AND (
                policy_snapshot #>> '{recovery,version}' <> '2'
                OR jsonb_typeof(policy_snapshot #> '{recovery,payment_timeout_seconds}') = 'number'
            )
        ELSE false END
    ) NOT VALID;
ALTER TABLE public.auctions VALIDATE CONSTRAINT auctions_recovery_policy_numeric_types_000121;

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (121, 'breaking');
