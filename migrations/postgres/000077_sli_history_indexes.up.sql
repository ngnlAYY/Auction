-- storage owner：为 W0.13 历史 SLI 增加全局时间窗口候选路径。
-- 仅新增普通索引，不改写业务事实、状态、证明或历史记录；旧程序读写语义不变。
-- 索引只提供候选入口，派生查询、证据判定与完整候选上限另行实现。
-- compatible：纯新增索引；旧程序不读取、不依赖，既有约束与写入结果不变。

-- 全局入站时间窗口；现有 inbound_mail_poll_idx 以 mailbox_key 为首列。
CREATE INDEX inbound_mail_sli_received_at_idx
    ON public.inbound_mail (received_at ASC, id ASC);

-- 所有成功投递的完成时间窗口；领取/lease 部分索引不覆盖历史完成记录。
CREATE INDEX deliveries_sli_succeeded_completed_at_idx
    ON public.deliveries (completed_at ASC, id ASC)
    WHERE status = 'succeeded';

-- 已成功发送的 PM 完成窗口，包括后来失效的通知。
CREATE INDEX lot_watch_pm_sli_succeeded_completed_at_idx
    ON public.lot_watch_pm_deliveries (completed_at ASC, notification_id ASC)
    WHERE status = 'succeeded';

-- 全局场次结算窗口；付款邮件终点仍需从 deliveries 反查场次。
CREATE INDEX auction_settlements_sli_settled_at_idx
    ON public.auction_settlements (settled_at ASC, auction_id ASC);

-- 固定 action 的全部历史 tick；不依赖当前 live lot 调度状态。
CREATE INDEX audit_records_sli_slow_proxy_tick_idx
    ON public.audit_records (occurred_at ASC, id ASC)
    WHERE action = 'lot.slow_proxy_tick';

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (77, 'compatible');
