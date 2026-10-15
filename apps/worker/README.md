# auction-worker

[文档导航](../../docs/README.md) · [任务编排](../../crates/auction-jobs/README.md) · [外部适配器](../../crates/auction-integrations/README.md) · [配置](../../ops/config/README.md)

持久化生命周期、论坛、邮件、投递和恢复循环的装配根。
`src/lib.rs` 导出 `run`、`run_from_path`，独立 `main.rs` 与[合并服务](../server/README.md)的 Worker 子进程复用装配。

仅支持 Linux x86_64 原生目标；装配入口在编译期拒绝其他 OS/架构。浏览器 WASM 不受此限制，
默认发布 ABI 与构建边界见[开发指南](../../docs/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap](src/bootstrap) | 配置、持久化任务、论坛、告警、履约与 control 装配 |
| [src/bootstrap/mail](src/bootstrap/mail) | 邮件凭据、业务处理器、transport、轮询与安全映射 |

## 职责与实现边界

Worker 不监听业务写入 HTTP，不创建第二套拍卖状态机。应用规则由 application/domain 拥有，
租约、游标和事实由 storage 持久化，循环/重试由 jobs 编排，远端传输由 integrations 实现。

## 装配与生命周期

`bootstrap/mod.rs` 选择配置/缓存/运行模式，`persistent.rs` 装配持久化 Worker；
`forum.rs`、`operations_alert.rs`、`fulfillment.rs`、`control.rs` 各有独立职责。
无数据库时 `skeleton.rs` 只报告进程状态，不执行任务或伪造数据库就绪。

SIGINT/SIGTERM 将 control 标记为 draining，停止新 claim，在途任务沿原租约和结果证明路径收口；
10 秒 drain 后停止心跳，第二次信号强制收口，最后按配置限时刷新 tracing exporter。
显式启动及数据库 feature 选择见[开发指南](../../docs/development.md)。

## 部署配置与 runtime

Worker 没有业务 `listen_address`。Admin/Worker 可共用 `runtime_config_file`；Worker 只消费
world、发件人、装备过滤、14 个邮件模板以及接管后的 `operations`，不继承 Admin listener/security/session。
transport、凭据、mailbox identity 和全局启用仍由 Worker 部署配置拥有。保存后须重启对应进程，
不改既有拍卖快照、due、游标、账本或租约；分组责任见[配置说明](../../ops/config/README.md#runtime-共享与重启)。

## 拍卖邮件起拍价

未写 `start` 的装备默认 50,000 Credits，可由 `[worker.mooglemail].start_price` 覆盖，独立于公共经济模板。
一条 `start amount` 应用于全部装备，逐件形式为同一行的 `<equipment-url> start amount`，旧跨行三元组仍兼容。
普通说明、空行、Attachment 块、URL 包裹/句末标点和空白后的 `#` 注释不改变未指定附件的默认价。

金额支持正整数、规范千分位及精确整数 Credits 的 `k`/`m` 小数（如 `1.5k`）。
缺金额、误写 `start=10k`/`start:10k`/`start10k`、零负值、溢出、重复定价、未知 ID 或 key 不匹配均拒绝，
不能静默退回默认价；`starting`、`starter`、`start with...` 等普通说明不是定价指令。

## 撤拍装备寄回

PostgreSQL 撤拍事务创建 `withdrawn_return` 与履约记录，Worker 在原 world 把原装备单独 Send 给冻结卖家，
Credits/COD 为零。同封其他 lot 不受影响，已 Take 的邮件不能整体 Return。
旧未知发送结果不自动重寄，旧冻结 Return 不能直接改为 Send；核对方法见[历史资产处置](../../docs/operations/mail-diagnostics.md#历史资产处置核对)。

## Cargo feature

默认 feature 为空；`postgres` 启用持久存储/任务，`cache-redis` 编译可选缓存。

## 部署约束

生命周期、慢速代理、结算，
以及 fulfillment/recovery 等 owner 各有预算，不因单个积压阻塞其他循环。
默认值与租约约束见[Worker 循环预算](../../ops/config/README.md#worker-循环预算)。
自动周排期默认关闭；新轮次 world 取配置 allowlist、runtime 启用集合与数据库 `worlds.enabled` 的交集。
数据库 world 关闭阻止新轮次，但不阻止已持久化结算/交付/恢复安全排空。
与 Admin 一样，Worker 向生命周期命令注入有效经济模板，仅在建立后继时冻结；旧场次与已接受命令不重算。

## 邮件调度

每个 world 有独立 cron、抖动和手动唤醒器；批量读取、业务处理、重试及续租均受预算约束。
重启只把过晚的 due 向当前调度收敛，不恢复暂停邮箱、越过活动 claim 或压缩失败退避。
批次只提交完成前缀；确定性错误立即收口，只有 Retryable/超时进入有界重试。
参数、时区和默认值见[论坛与邮件配置](../../ops/config/README.md#论坛与邮件)。

## 持久化与外部结果证明

任务 due、租约、尝试次数、完成 fence、命令账本和游标均持久化；内存计时器只负责唤醒。
claim token/owner/版本或续租 fence 失效后取消陈旧处理，只能从权威 cursor 重放。
外部写入在本地事务提交后执行，未知结果进入对账，不能抹掉已接受事实或自动重发。

所有主动 ComposeSend 都以完整有界 Sent baseline 和发送后独立 scan 证明新增唯一 MID，
同时精确匹配 world、recipient、subject/body digest、Credits/COD 和装备；零资产通知也需要证明。
Return/Take 使用独立 action proof。证明、加密异常留证、历史展示回填和资产核对统一见
[邮件诊断手册](../../docs/operations/mail-diagnostics.md)。诊断证据不参与命令摘要、completion 或 cursor。

凭据由 PostgreSQL 的独立数据密钥解密，论坛和邮件每种 Cookie 全局最多一条 active，
生命周期 advisory lock 保证单 Worker owner。失效后不切换 Cookie，导入或轮换后须重启交接。
未配置凭据数据密钥时保留受限 `.keyenv`/敏感文件兼容入口；`.keyenv` 仅 `hv-ck-key1` 供 Worker 使用。
`hv-ck-key2` 只供显式隔离验证。告警通过同一幂等 outbox 发送，自身 delivery 排除于告警统计，避免循环。

## Recovery 推进

PostgreSQL recovery due 推进独立于邮件 transport；付款阶段、超时、提醒、退件 Take、买家黑名单、
guarded relist 与退款完成由各 owner 的稳定命令、资产摘要和证明推进。只有启用邮件 transport 后
相关 action 才关联 delivery outbox；未知结果保持人工复核。状态迁移与审计使用 Serializable 事务。
详细编排见[auction-jobs](../../crates/auction-jobs/README.md)。

## 论坛同步

仅 PostgreSQL 且启用论坛时装配主题发布和回帖导入。每轮按“发布 → 导入”执行，一阶段失败不跳过另一阶段。
只为实际 `live` 拍卖创建主题；未知 POST 先精确标题 GET 对账，未命中须通过
[`authorize-forum-write-retry`](../auctionctl/README.md#论坛与结算核验)显式授权。

固定 #1/#2 的 exact post ID 和内容 lease 持久化后才处理 #3+；既有 #2 非系统标记时不覆盖。
论坛数字账号 ID 是稳定身份，用户名用于显示和 MM 收件；新作者可建立 revoked-key 的 `forum_only` 主体，
不自动获 active Key。只导入普通 `bid`，`proxy`/`proxy lower` 静默忽略，`start/cancel` 和 PM-only `notify`
仅作 warning；论坛读取、身份与渲染协议见[外部适配器](../../crates/auction-integrations/README.md)。

## 心跳与论坛启用状态

数据库 Worker 发布启动、就绪、周期及停止心跳，含 runtime 投影后的 `details.forum_sync_enabled`；
Public 据此显示实际启用状态，不从历史帖子推断。开关变更须重启 Worker。
`[worker.control]` 提供受信目录、实例锁、固定 group/0660 和 UID/GID peer allowlist 保护的 live/ready/status，
部署要求见[systemd 手册](../../ops/deploy/systemd/README.md)。

## 维护与验证

`cargo check --locked -p auction-worker` 只做默认 feature 静态编译；完整门禁见[开发指南](../../docs/development.md)。
历史运行结果与剩余缺口见[验证状态](../../docs/status.md)，本 README 不授予测试或远端写入授权。
