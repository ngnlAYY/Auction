-- owner: auction-storage（寄售历史证据、碰撞与有效活动额度读取）。
-- 不变量 6：按完整规范身份与 lot 基线检查，不因历史规模截断证据。
-- 兼容等级 compatible：两个索引均非唯一，别名函数对既有文本和旧程序写入可用，读写语义与权限不变。

CREATE INDEX equipment_consignment_eid_alias_idx
    ON public.equipment (world_id, public.consignment_eid_alias(external_equipment_id));

CREATE INDEX lots_equipment_id_idx
    ON public.lots (equipment_id, id);

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (118, 'compatible');
