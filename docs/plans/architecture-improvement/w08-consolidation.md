# W8 规模收敛评估

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md) · [docs/plans/architecture-improvement/w08-code-structure.md](w08-code-structure.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 规模收敛评估（只出决策记录，放在最后） |
| 优先级 | P2 |
| 里程碑 | M6 最后 |
| 依赖 | 其余工作流完成后 |
| 迁移 | 否 |
| 估算 | 2–3 天 |
| 关联缺陷 | —（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 10、19、22、35（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

输出决策记录，不直接实施；放在全部工作流之后评估（决策 19）。

| 项 | 内容 |
| --- | --- |
| W8.1 | 部署形态：是否只保留合并部署（须先解决合并部署任一子进程退出即全停且 `Restart=no` 的故障域问题）、是否移除可选 Redis、首次设置服务与停用世界（isekai）代码路径的去留 |
| W8.2 | 代码结构：`#[path]` 与目录对齐、`use super::*` 收敛、`auction-application` 平铺导出改为按上下文分组、大文件（如 `crates/auction-storage/src/database/mailbox_sweep/mod.rs`）拆分、按限界上下文拆 crate 的编译收益；旧兼容层清单（`crates/auction-application/src/services/auction/competition/legacy_ledger.rs`、legacy 自寄、v1 竞价策略、迁移执行视图）及各自的移除条件；不可达代码与无人读写的 schema 对象清单（Worker `realtime_consumer` 运行路径、`bid_proxy_attempts`、`bid_proxy_outbid_uses`、`bid_proxies` 视图，以及 W1.11 未复用时的 `auction_forum_reply_cursors`）、删除条件与数据库契约更正；是否把 W9.9 的共用运行账号再拆为按角色账号；实施时以 W13.1 的计数门禁锁定进度 |

验收：决策记录写入 [docs/plans/architecture-improvement/decisions.md](decisions.md)，并同步 `docs/architecture.md` 与运维手册的对应描述。回退：只改文档，无需回退。
