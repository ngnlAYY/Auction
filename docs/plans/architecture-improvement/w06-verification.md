# W6 验证体系（W6.1–W6.4、W6.6 已批准）

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 验证体系（W6.1–W6.4、W6.6 已批准） |
| 优先级 | P1 |
| 里程碑 | M2a，W6.3、W6.6 先行 |
| 依赖 | W6.1 中各算法测试随对应工作项交付；W6.6 依赖 W6.3 |
| 迁移 | 否 |
| 估算 | 16–22 天 |
| 关联缺陷 | D2（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 2、20、27、28、44、46（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W6.1 | 纯逻辑测试：结算守恒、持有曲线插值、比例分配、冲正；竞价增量 v1/v2、代理竞争与上限出价、软截止；排期日历（夏令时、早鸟期、赛季前场次）；MoogleMail 与论坛解析器（合成样例覆盖收件人 15 字符截断、标题 7 位 hex、分钟精度时间、战斗页、登录页）；前端纯逻辑（实时事件合并与缺口检测、URL 查询状态）；随对应工作项交付的算法：W1.1 熔断状态机、W1.3 调度算法（tokio 暂停时间）、W1.4 水位与 W1.11 游标、W2 处置闭集、W7.5 时段门禁判定。另含：结算守恒、价格单调、金额网格与代理竞争不变量的属性测试；不可信输入解析器（论坛回帖语法、邮件 `start` 指令、HV 页面）以属性测试生成任意输入，保证不 panic 且失败关闭（不引入 nightly fuzz 工具）；`ops/config/examples` 与首次设置生成结果能被 loader 加载；DTO 序列化快照，发现破坏兼容的契约变化（配合 W7.6）；通知渲染快照（每个模板与占位符，含旧载荷）与提醒周期（领先周期、重新领先失效、截止后退役、PM 缺收件人、慢速代理待定提醒） |
| W6.2 | PostgreSQL 18 集成测试骨架：测试夹具用本机 PG18 二进制建临时集群，每个测试从模板库克隆独立库；以独立 feature 控制，`just verify` 只编译不运行；覆盖空库执行全部迁移、竞价（含并发序列化冲突）、结算、人工 Sent 证明、恢复推进、仪表盘计数、非 UTC 会话下的证明；W9.9 落地后以运行账号执行。覆盖分两步：M2a 先覆盖资金、投递、生命周期与竞价 owner 的存储端口；每个存储端口方法至少执行一次（157 个端口 trait、856 条运行时 SQL 没有编译期校验）作为 M6 前的目标 |
| W6.3（已批准，M2a） | 脱敏的真实页面回放语料（Inbox/Read Mail/Sent/详情/战斗页/截断列表/论坛页）：去除 Cookie、登录 key 与表单 token，替换玩家名称，只保留解析所需的页面结构（决策 28）；先于 W1.1、W1.11、W1.4 交付，供其解析与替身复用 |
| W6.4（已批准） | CI 增加独立测试作业，运行 `just test` 与 `just test-db`（CI 内安装 PostgreSQL 18），不并入默认 `just verify` |
| W6.5 | 测试入口与规范：新增 `just test`（纯逻辑）与 `just test-db`（PG18 集成）；同步更新 `AGENTS.md` 验证范围、`CONTRIBUTING.md` 与[docs/development.md#测试](../../development.md#测试)，写明已批准的测试类别与维护要求；[docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范)第 5 步改为要求运行受影响范围的测试 |
| W6.6（已批准，M2a） | 外部服务替身（决策 44）：以 W6.3 语料驱动的本地 MoogleMail（Inbox/Read Mail/Sent 列表与详情、compose、战斗页、登录页）与论坛（主题分页、回帖、私信）模拟服务，可注入账号忙、Cookie 失效、超时、页面结构变化、写入结果未知、删帖、邮资不足与收件人不存在；只在测试入口启动，绝不连接真实账号；W1 与 W5 的模拟类验收先在替身上完成，再到线上观察 |

验收：W6.1 覆盖上述每类至少一组正反例并全部通过；W6.2 在本机可一条命令运行并清理临时集群，存储端口方法覆盖率可统计；W6.6 能复现 D2、账号忙与 Cookie 失效三类场景；规范文档与 Justfile 入口一致。回退：测试只在独立入口运行，不影响默认门禁与运行时。
