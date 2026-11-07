# W8.2 代码结构收敛评估

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement/w08-consolidation.md](w08-consolidation.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/architecture.md](../../architecture.md) · [ci/checks/README.md](../../../ci/checks/README.md)

> 2026-11-06 后续结构提交已实施 D-W8.2-a/b/c/e/f 与剩余大文件拆分：当前 `all:#[path]` 为 0、`use_super::*` 为 186，迁移 `000097` 已删除四类无人读写对象，Worker `realtime_consumer` 运行路径已删除。以下 §3–§9 保留当时的评估记录，不再代表当前状态；现行门禁以 `ci/checks/source-ratchet.baseline` 为准。

## 1. 结论摘要

| 类别 | 本轮处理 | 说明 |
| --- | --- | --- |
| 超大模块拆分 | 已实施 1 处 | `crates/auction-storage/src/database/mailbox_sweep`：969 行 `mod.rs` 拆为 `mod.rs`（450）+ `candidates.rs`（230）+ `transition.rs`（272）+ `error.rs`（56），纯代码搬移 |
| `use super::*` 收敛 | 已实施 1 个 crate | `auction-config` 13 处改为显式 `use super::{…}`/`use std::io`，`all:use_super::*` 326 → 313 |
| `#[path]` 与目录对齐 | 未实施 | 除 1 处目录定位必需外，其余都是“扁平命名空间 + 目录分组”的装置，改动会重排模块身份，须先定决策 D-W8.2-a |
| `auction-application` 平铺导出 | 未实施 | 152 处 `#[path]` + 838 个平铺符号，按上下文分组属 API 重排，须先定决策 D-W8.2-b |
| 旧兼容层 | 未删除 | 四处兼容层仍被冻结历史/重放路径使用，删除条件见 §6 |
| 无人读写的 schema 对象 | 未删除 | 无迁移、未改数据库契约；删除条件见 §7 |
| 不可达运行路径 | 未删除 | Worker `realtime_consumer` 在装配根恒为 `None`，删除属能力决策 D-W8.2-e |

计数门禁（[ci/checks/source-ratchet.baseline](../../../ci/checks/source-ratchet.baseline)，14 项）本轮只有一项下降，其余保持不变：

| 项 | 本轮前 | 本轮后 | 说明 |
| --- | --- | --- | --- |
| `all:use_super::*` | 326 | 313 | `auction-config` 全部收敛 |
| `all:#[path]` | 403 | 403 | 未变；§3 说明为何不宜机械删除 |
| `storage:map_err(\|_\|)` | 313 | 313 | 罕见错误的分类改造，非结构收敛 |
| 其余 11 项 | — | — | 未变 |

## 2. 盘点方法

- `#[path]`、`use super::*` 计数口径见 [ci/checks/README.md](../../../ci/checks/README.md)：注释与字面量不计入，所以 `rg` 原始命中（413/313）与门禁计数（403/313）不同；差额来自文档注释里的示例。
- 大文件按 `wc -l` 统计 `apps/*/src`、`crates/*/src` 的非 `tests/` 源文件。
- “无人读写”按 Rust 源码（含动态 SQL 字符串）逐名检索，并区分迁移、保留策略与文档中的登记。

## 3. `#[path]` 清单（403 处）

| crate | 处数 | 主要形态 |
| --- | --- | --- |
| `auction-application` | 152 | `ports/mod.rs`（77）、`services/mod.rs`（27）等：把 `admin/`、`auction/`、`settlement/`、`mail/` 等目录下的文件拉平成单一命名空间 |
| `auction-web-admin` | 97 | `src/api/mod.rs`（22）、`app/surface/mod.rs`（20）等：把 `api/`、`app/` 各目录的模块重新挂到 surface 树 |
| `auction-storage` | 63 | 目录子模块与测试挂载（如 `forum/sync/snapshot.rs` 的 `#[path = "settlement.rs"]`） |
| `auction-http` | 37 | `http/routes/mod.rs`（35）：路由文件平铺 |
| `auction-web-public` | 31 | `app/surface/mod.rs`（13）、`api/mod.rs`（12）：跨目录重新挂载 |
| `auction-jobs` | 30 | `worker/runtime/mod.rs`（11）等 |
| `auction-integrations` | 3 | 单点 |

判定：

1. **不机械删除。** `#[path]` 在这里不是遗漏，而是“文件按 owner 分目录、模块路径保持扁平”的显式装置。直接改成自然层级会改变每个模块的 `super::` 解析结果与 `crate::…` 可见路径，波及同目录内所有 `use super::…`；删除必须与 §5 的调用面一起重排。
2. **`forum/sync/snapshot.rs` 的 `#[path = "settlement.rs"]` 不可删。** `snapshot.rs` 自身是 `forum/sync.rs` 的子模块，其子模块目录是 `forum/sync/snapshot/`；该属性是让它指向同级 `forum/sync/settlement.rs` 的唯一办法。本轮曾误判为冗余，编译失败后已恢复。
3. **可安全删除的冗余 `#[path]`：0 处。** 逐个比对“属性值 vs 默认解析路径”（`name.rs`、`name/mod.rs`）后无命中。

删除/对齐的迁移路径（需先定 D-W8.2-a）：先选一个内聚子目录（如 `auction-application/src/ports/settlement`）改成自然层级，把该目录所有文件的 `super::` 改到新深度，`ports/mod.rs` 用 `pub use settlement::*` 保持对外符号不变，再用门禁与全量编译验证。

## 4. `use super::*` 清单（313 处）

| crate | 本轮前 | 本轮后 |
| --- | --- | --- |
| `auction-storage` | 108 | 108 |
| `auction-integrations` | 54 | 54 |
| `auction-web-public` | 32 | 32 |
| `auction-contracts` | 30 | 30 |
| `auction-jobs` | 28 | 28 |
| `auction-application` | 23 | 23 |
| `auction-http` | 22 | 22 |
| `auction-web-admin` | 8 | 8 |
| `apps/auction-worker` | 7 | 7 |
| `auction-config` | 13 | **0** |
| `auction-domain` | 1 | 1 |

已实施：`auction-config` 的 13 处全部替换为显式导入（`use super::{ConfigError, WorkerConfig, …}`、`use super::super::{…}`、`use super::{Error, PathBuf}` + `use std::io`）。`worker_validation/*.rs` 需要 `super::super::RecurringSchedule` 才能让 `next_after` 这个 trait 方法可见，说明 glob 实际在传递 trait。

未实施的判定：其余 313 处中 306 处在生产代码、7 处在 `#[cfg(test)] mod tests`（测试模块的 glob 是 Rust 惯例，建议保留）。生产代码部分是“父模块声明公共导入、子模块 glob 承接”的既有约定，逐文件显式化是纯机械改动的宽面 diff（≈300 文件），应按 crate 分批推进，每批都要有编译器与门禁证据。建议顺序：`apps/auction-worker`(7) → `auction-web-admin`(8) → `auction-http`(22) → `auction-application`(23)，其余大 crate 视收益再定（D-W8.2-c）。`auction-domain` 唯一一处位于测试模块，建议保留。

## 5. 平铺导出与 `include!`

`crates/auction-application/src/ports/mod.rs`（657 行）同时使用两种装置：77 处 `#[path]` 与 10 处 `include!`（`include!` 用于把一个文件整体塞进额外的包装模块，如 `mod delivery_admin_guard { include!("delivery/admin_guard.rs"); }`）。全仓 `include!` 共 20 处，分布在 9 个文件：`auction-application` 的 `ports/mod.rs`（10）、`ports/operations/alert.rs`（2）、`services/mod.rs`、`services/mail/processing.rs`、`ports/settlement/funding.rs`、`ports/admin/runtime_config.rs`，以及 `auction-http/src/http/routes/mod.rs`（2）、`auction-jobs/src/worker/runtime/mod.rs`、`auction-web-admin/src/app/operations/config.rs`。

`crates/auction-application/src/services/mod.rs`（116 行，27 处 `#[path]`）与 `crates/auction-http/src/http/routes/mod.rs`（202 行，35 处 `#[path]`）形态相同。`crates/auction-web-admin/src/api/mod.rs`（948 行，22 处 `#[path]`）同时是超大文件与平铺导出。

按上下文分组（`ports::settlement::*` 等）会让外部调用点从 `ports::Foo` 变为 `ports::settlement::Foo`；是否接受这一层重新导出成本需要用户决定（D-W8.2-b）。`include!` 的包装模块可以顺带消除——它存在只是为了给被包含文件一个模块边界。

## 6. 旧兼容层清单与移除条件

| 兼容层 | 位置 | 现状 | 移除条件 |
| --- | --- | --- | --- |
| 旧竞价账本编解码 | [crates/auction-application/src/services/auction/competition/legacy_ledger/](../../../crates/auction-application/src/services/auction/competition/legacy_ledger/)（1524 行） | 在用：`competition.rs` 的确定性 ID、`encode`/`replay_accepted` 与 `LEGACY_RESULT_CODE` 都走它 | 只有当库里不再存在需要重放的旧命令结果（`auction_commands` 历史行）且不再需要旧 Public 的 `result_payload` 编码时才可移除；属于数据寿命决策，不是代码清理 |
| 旧结算重放 | [crates/auction-application/src/services/settlement/commands/legacy.rs](../../../crates/auction-application/src/services/settlement/commands/legacy.rs)（329 行） | 在用：`classify_stored_settlement`、`replay_legacy_settlement`、`legacy_worker_id`、`legacy_complete_hash` | 需要“历史结算命令不再可重放”的判定与审计迁移方案，且必须保留对旧 `request_hash` 的校验能力；不得在仍有冻结回归样本时删除 |
| 旧 v1 竞价/结算策略 | `crates/auction-domain/src/bidding/policy.rs`、`crates/auction-domain/src/settlement/{plan,policy,facts}.rs` | 在用：冻结场次的快照按 `policy_version` 选公式，v1 与 v2 并存是领域不变量 10 的实现 | 不可移除。只能新增版本；删除 v1 需要“所有 v1 场次及其回放样本均已出清”的证明，并同步结算契约 |
| 旧自寄（self-send）核对 | `crates/auction-domain/src/delivery/{mod}.rs`、`ports/settlement/internal/mod.rs`、`ports/delivery/admin_guard.rs` | 在用：`settlement_legacy_self_reconciliations` 修复命令与“永久退役”退役规则仍在校验 | 属于资金事实修复路径，只能追加新状态，不能删除既有分支 |
| 迁移执行视图 | `migrations/postgres/` 中的兼容视图与函数（如 `bid_proxies`） | 见 §7 | 见 §7 |

结论：兼容层均仍被“历史事实可重放、可核对”的要求支撑，删除会与不变量 4/8/10 冲突。本轮不删除任何兼容层。

## 7. 不可达代码与无人读写的 schema 对象

2026-11-05 复核：对 Rust（含动态 SQL 字符串）、迁移、`ops/`、`ci/`、契约与计划文档逐名检索（排除 `target/` 构建与证据产物），并核对迁移历史与保留策略。

| 对象 | 当前代码引用 | 迁移/策略登记 | 判定 |
| --- | --- | --- | --- |
| `public.bid_proxy_attempts` | 无 | 表定义 `000003`；权限重建 `000081/083/088/089/092/096`；保留策略 v2 归入 `auction_history`（`retain_forever`、`candidate_class=none`、`mutation_authorized=false`） | 无人读写；本仓历史没有任何 `INSERT`/`UPDATE`/`DELETE` 该表的代码。历史唯一 Rust 引用是 `legacy_v28/source_checks.rs` 的 `to_regclass` 存在性检查，已随 `ce1cedbc` 删除。删除条件见 §7.2 |
| `public.bid_proxy_outbid_uses` | 无 | 同上；另被 `000005` 的 `bid_proxies` 视图 `LEFT JOIN` | 同上；删除须先删除视图 |
| `public.bid_proxies`（视图） | 无 | 仅 `000005` 定义；不在保留策略内（盘点只收集表，不收集视图） | 视图不持有数据；无显式授权，运行账号仅经 `GRANT SELECT ON ALL TABLES` 可读。删除只要迁移与契约更正 |
| `public.auction_forum_reply_cursors` | 无 | 表定义 `000005` + `main`/`isekai` 种子 2 行 + 多条权限重建；保留策略 v2 归入 `ephemeral_current_projections`（`retain_forever`、`candidate_class=owner_lifecycle_only`、`mutation_authorized=false`） | 旧世界级扫描游标；`8d25ea76` 起不再有写入方，W1.11 已改用 `auction_forum_bid_commands` 的逐主题 `post_number` 游标，`b9c5f867` 删除最后的游标模块。表内只余种子/运行状态，不是历史事实 |
| `public.settlement_pools` | 无 | 按契约只作冻结证据（合并到全局池后由触发器冻结） | **不可删除**：属于历史资金证据 |

不可达运行路径：

| 路径 | 证据 | 判定 |
| --- | --- | --- |
| Worker `realtime_consumer` 消费循环 | `apps/auction-worker/src/bootstrap/persistent.rs` 恒为 `None`（“外部广播协议尚未冻结；没有具体 sink 时不启动实时消费”）；`crates/auction-jobs/src/worker/realtime/consumer.rs`（255 行）在当前树只有定义、重导出、`PersistentWorker` 字段与 `runner.rs` 的 `is_some()` 守卫，测试目录零引用；`git log --all -S 'PersistentRealtimeConsumer::new'` 与 `-S 'realtime_consumer: Some'` 零命中 | 生产不可达且本仓从未装配，但属于待启用能力（外部广播 sink 协议未冻结）。删除属能力决策 D-W8.2-e，且要同步 `ops/README.md` 的日志目标表与指标词表；保留 |

### 7.1 复核证据

- 全仓逐名检索（`*.rs`/`*.sql`/`*.sh`/`*.json`/`*.yml`/`*.toml`/`*.md`，排除 `target/`）四对象在生产代码中零命中；唯一命中为迁移定义、权限重建、保留策略、数据库契约与本页。
- `git log -S 'INSERT INTO public.bid_proxy_attempts'`、`... outbid_uses` 与 `... auction_forum_reply_cursors` 均无应用写入提交；`auction_forum_reply_cursors` 的最后写入方（`mark_scanned_postgres`）在 `8d25ea76` 移除，`b9c5f867` 再删除遗留模块。
- 2026-11-02 的生产只读盘点（`target/operations-evidence/sent-precision-deploy-20261102/remote-evidence/inventory.json`，schema 74）显示 `bid_proxy_attempts` 与 `bid_proxy_outbid_uses` 的 `estimated_rows=0`，`auction_forum_reply_cursors` 为 2 行（`000005` 的 `main`/`isekai` 种子）。这是历史证据，不替代当前库核对。
- 外键只有两表指向 `bid_proxy_authorizations` 与 `auction_commands`；没有其它表引用这三张表。`bid_proxies` 依赖 `bid_proxy_outbid_uses`，删除顺序必须先视图后表。
- `ops/README.md` 的日志目标表仍登记 `realtime_consumer`；`WorkerLoop::Realtime` 同时出现在 runner 排空分支、`auction-observability` 的循环标签与业务事件映射中，删除不是单文件删除。

### 7.2 删除条件

| 对象 | 删除前置 |
| --- | --- |
| Worker `realtime_consumer` | 用户批准 D-W8.2-e（能力决策，非纯清理）；删除 `worker/realtime/consumer.rs`、`PersistentWorker.realtime_consumer` 字段、`runner.rs` 的 spawn/select/drain 分支、`WorkerError::RealtimeTask*`、`worker/mod.rs`/`lib.rs` 重导出与装配根的 `None`；同步 `WorkerLoop::Realtime`/`BusinessEvent::Realtime` 指标标签、`ops/README.md` 与相关文档。重新引入需要冻结外部广播协议并实现真实 sink |
| `bid_proxies` | 用户批准新增迁移；迁移中先 `DROP VIEW public.bid_proxies`，再更正 `docs/contracts/database.md`；视图不在保留策略内，无需策略变更 |
| `bid_proxy_outbid_uses` | 先删除视图；确认零行；用户批准删除历史表并把两表从 `auction_history` 家族移除；更新 `ops/deploy/retention-policy.v2.json` 的 `tables`、`database_schema_version`（须等于 `sqlx::migrate!` 最大版本，当前 96）与 `resolved_operator_decisions`；同步契约与权限重建 |
| `bid_proxy_attempts` | 同 `bid_proxy_outbid_uses` |
| `auction_forum_reply_cursors` | 用户批准新增迁移；确认仅种子/运行状态行；从 `ephemeral_current_projections` 家族移除并更新策略版本与 `database_schema_version`；更正 `docs/contracts/database.md` 的两处描述 |

保留策略 JSON 由 `crates/auction-storage/src/database/retention_inventory/mod.rs` 的 `include_str!` 编译期内嵌，`retention_inventory/mod.rs` 会拒绝 `database_schema_version` 与当前迁移版本不一致的策略；因此任何删表迁移都必须同步更新该文件，不能只写迁移。本轮未新增迁移、未删除 schema。

## 8. 大文件清单（非测试源码，>600 行）

| 文件 | 行数 | 归属 | 建议 |
| --- | --- | --- | --- |
| `crates/auction-application/src/services/auction/competition/legacy_ledger/` | 1524 | 旧兼容层 | 见 §6，不拆（随兼容层退场一起消失） |
| `crates/auction-integrations/src/clients/mooglemail/gateway/reader.rs` | 1523 → 42 + 子模块 | 邮件读取解析 | **本轮已拆**（D-W8.2-d）：入口只留 gateway 状态与子模块声明；`reader/gateway.rs`（构造与观测注入）、`reader/window.rs`（窗口纯函数与测试）、`reader/scan.rs`（增量计划/检查点/前缀选择）、`reader/inbox.rs`（收件窗口与详情）、`reader/sent.rs`（Sent 证据）、`reader/recovery_scan.rs`（恢复枚举）、`reader/snapshot.rs`（余额/库存）、`reader/error.rs`（错误映射） |
| `crates/auction-integrations/src/clients/mooglemail/sender.rs` | 1008 | 邮件发送 | 可按 transport/证据拆分；本轮未动 |
| `crates/auction-application/src/services/mail/maintenance.rs` | 999 | 邮件维护用例 | 可按判定/执行拆分；本轮未动 |
| `crates/auction-storage/src/database/mailbox_sweep/mod.rs` | 969 → 450 | 邮件维护持久化 | **本轮已拆**：`candidates.rs`（只读投影）、`transition.rs`（迁移事务）、`error.rs`（重试分类） |
| `crates/auction-jobs/src/worker/runtime/delivery.rs` | 955 | 投递运行时 | 本轮未动 |
| `crates/auction-web-admin/src/api/mod.rs` | 948 | Admin API 平铺导出 | 与 §5 一起处理 |
| `crates/auction-web-admin/src/app/fulfillment/deliveries/sent_proof.rs` | 880 | Admin 页面 | 本轮未动 |
| `crates/auction-web-admin/src/app/operations/dashboard/content.rs` | 854 | Admin 页面 | 本轮未动 |
| `crates/auction-storage/src/database/delivery_admin/mapping.rs` | 843 | 投递映射 | 本轮未动 |
| `crates/auction-application/src/ports/external_metrics/model.rs` | 838 | 指标模型 | 本轮未动 |
| `crates/auction-web-public/src/api/auction/realtime.rs` | 834 | Public 实时流 | 本轮未动 |
| `crates/auction-integrations/src/clients/ehentai/client/mod.rs` | 831 | 论坛客户端 | 本轮未动 |

## 9. 待决策项

| 编号 | 事项 | 选项 |
| --- | --- | --- |
| D-W8.2-a | `#[path]` 与目录对齐 | (1) 保持现状，只在新增代码处要求自然层级；(2) 按 crate 分批改为自然层级并保持 `pub use` 对外符号不变 |
| D-W8.2-b | `auction-application` 平铺导出 | (1) 保持 `ports::Foo` 平铺；(2) 改为 `ports::<context>::Foo` 并接受调用点改动 |
| D-W8.2-c | `use super::*` 收敛范围 | (1) 只新增代码禁止；(2) 逐 crate 显式化到零 |
| D-W8.2-d | 大文件拆分顺序 | `mooglemail/gateway/reader.rs` 已按此顺序完成拆分；下一步 `mail/maintenance.rs` → `worker/runtime/delivery.rs`，每次都要有行为证据 |
| D-W8.2-e | `realtime_consumer` 运行路径 | (1) 保留待启用；(2) 删除并记录重新引入成本 |
| D-W8.2-f | 无人读写 schema 对象 | (1) 保留；(2) 新增迁移删除 `bid_proxies` 视图与 `bid_proxy_attempts`/`bid_proxy_outbid_uses`/`auction_forum_reply_cursors`，同步保留策略与数据库契约 |

## 10. 验证边界

本轮验证（隔离在本仓，未部署、未提交、未跑全量门禁）：`cargo fmt -p auction-storage -p auction-config --check`、`cargo check`/`cargo clippy`（`auction-storage --features postgres --all-targets`、`auction-storage --all-targets` 默认 feature、`auction-config --all-targets`）、`cargo test -p auction-config --all-targets`（31 通过）、`cargo test -p auction-storage --all-targets`（默认 feature 通过）、`bash ci/checks/check.sh ratchet`。`mailbox_sweep` 拆分为纯代码搬移：拆分前后去注释词元对比只差 8 个上移为模块级 `#[cfg(feature = "postgres")]` 的属性与 3 处 `super::recovery_commands` → `crate::database::recovery_commands` 的路径改写，未新增或修改行为与测试；PostgreSQL 集成测试（`just test-db`）属于数据库语义验证，本轮未运行。

`mooglemail/gateway/reader.rs` 拆分为纯代码搬移：1523 行 → 入口 42 行 + `reader/` 下 8 个子模块（共 1632 行，含新增模块文档与搬移后的 6 个既有测试）。去注释词元对比只差模块声明、`use` 引入与 `pub(in super::super)`/`pub(super)` 可见性标注；业务语句、错误映射、预算、超时、日志字段与页面解析路径逐字未改。验证：`cargo fmt -p auction-integrations --check`、`cargo check -p auction-integrations --all-targets`、`cargo clippy -p auction-integrations --all-targets`（无警告）、`cargo test -p auction-integrations --lib`（96 通过，含搬移的 `reader::window::tests` 6 项）、`bash ci/checks/check.sh layout`。未运行全量 `just verify`、数据库或真实外部服务验证。

2026-11-05 §7 复核为只读审计：全仓逐名检索、`git log -S` 历史核对、保留策略与迁移版本耦合检查，以及 `bash ci/checks/check.sh markdown-links`。未构建、未连接数据库、未部署；未新增迁移、未删除代码或 schema。
