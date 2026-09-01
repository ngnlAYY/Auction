# Auction-BF 与 Rust 版功能等价审计

## 文档状态

本文先冻结旧仓库 Auction-BF 与当前 Rust 工作区的差异基线，再在同一矩阵记录实现和证据状态；
审计对象包括实际源码、配置、制品和可运行行为。不以“路由数量相同”“存在同名模块”或既有迁移
说明中的完成声明作为功能等价结论，也不把静态修复直接升级为运行态等价。

- 审计日期：2026-08-26 至 2026-08-29；滚动并行复核以 2026-08-29 当前工作树为准。
- 旧版基线：`/home/ngnl/HV-Projects/Auction-BF`，提交
  `f109146e246c6bbfefd7f4749ac2bba087bcfa0b`，审计开始时工作树干净。
- 新版基线：`/home/ngnl/HV-Projects/Auction`，提交
  `f0d1b7c73fd48e6228df068a3f30fa0575f98622` 加审计开始时的未提交工作树；未提交内容同样属于
  当前状态，不能只按 `HEAD` 判断。
- 旧仓库只用于核对，不在本轮修改。
- 按当前协作约束，不新增或运行单元、集成、功能、端到端及浏览器测试；运行态证据限于用户授权的
  新旧进程、只读请求、浏览器页面检查和静态制品检查。

## 证据与结论规则

每个结论至少记录旧版和新版两侧的直接证据。证据强度从高到低如下：

1. `R1`：同一输入和环境下的新旧运行态响应、状态变化或浏览器行为。
2. `R2`：从入口到领域/存储/外部适配器的完整源码调用链，以及真实装配根。
3. `R3`：冻结的 route、DTO、事件、配置、schema 或浏览器 manifest。
4. `R4`：README、迁移计划或代码注释中的声明。

`R4` 只能帮助定位，不能单独证明完成。路由已注册但返回占位、空结果或能力错误时，不得记为等价。
数据库表、DTO 或 Dioxus 组件存在，也不能证明事务、错误和交互语义一致。

审计状态使用以下闭集：

- `等价`：两侧直接证据覆盖主要成功、失败和状态边界。
- `代码差异`：源码或装配已证明行为不同，尚未进入实现。
- `行为差异`：运行态已复现差异。
- `静态修复，待 R1`：差异已按双侧调用链实施并通过对应静态校验，但仍缺同输入运行态证据。
- `证据缺失`：代码看似对应，但缺少外部服务、真实数据、浏览器或部署证据。
- `声明边界`：SQLite Lite 或明确新增能力，不属于 PostgreSQL 旧版等价缺口。
- `待判定`：证据相互矛盾或不足，不能给出完成结论。

严重度定义：

- `P0`：可能破坏竞价/结算正确性、幂等、资金守恒、凭据安全、审计或数据恢复。
- `P1`：旧版核心用户流程、HTTP/事件契约、Worker 任务或管理命令不可用或语义错误。
- `P2`：可见页面、运维、配置或降级行为明显不同，但不直接破坏核心账务正确性。
- `P3`：文案、布局、可访问性或低风险兼容细节。

## 冻结清单

下列数量只作为覆盖索引，逐项等价结论见后续矩阵：

| 切面 | 旧版冻结基线 | 新版覆盖入口 | 当前结论 |
| --- | --- | --- | --- |
| Public 业务 HTTP | 20 个唯一路径 | `auction-http` Public routes | 路径齐全；Final、world/open gate、PostgreSQL 锁序、身份化读取认证优先级及规范 UUID 已静态修复，待同库竞价和组合输入 R1 |
| Admin 业务 HTTP | 37 个唯一路径 | `auction-http` Admin routes | 路径齐全；统一 mutation 安全顺序和论坛预览会话缺口已静态修复；正常非空结算、零税补偿账本、due 事实时间与 claim/retry/final 审计已有隔离局部 R1，其余报告/事件/资金及认证组合仍待 R1 |
| Public 浏览器路由 | 14 条 | Dioxus `Route` + SPA allowlist | 路径齐全；14 条路径已有 `en` 桌面匿名/空态巡检，分页大小、Catalog 动作、session 四态和 lot 历史列也已静态修复，待动态同数据 R1 |
| Admin 浏览器路由 | 21 条 | Dioxus `Route` + SPA allowlist | 路径齐全；未知客户端路由已收回 `AdminShell`，既有局部浏览器 R1 保留，列表页大小、绝对过期刷新、dashboard 聚合和 stale delivery dialog 已静态修复，待动态/认证 R1 |
| 界面语言 | 旧 Public 为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，旧 Admin 仅 `zh-CN` | 新版两套独立 Fluent 资源均支持五种语言 | 启用闭集、默认语言及 `hv.lang` 持久化已对应；双 surface picker、Public 五语言规则/补偿池和最终 Public populated 根页、Admin 登录页五语言初始化均有浏览器 R1，动态变量已恢复旧版无 FSI/PDI 文本；其余逐页换行仍归 EVD-002 |
| Worker/外部集成 | 按旧版持久任务和 recovery action 分类 | `auction-jobs` + `auction-integrations` | 慢速代理、论坛恢复、邮件 retryability/失败时间、告警 jitter，以及每周 bootstrap/mail gate 均已静态恢复，待运行态 R1 |
| 数据库与部署 | canonical PostgreSQL v28 + 旧部署/发布脚本 | PostgreSQL 完整档、SQLite Lite、`auctionctl`、确定性发布、systemd/Caddy | 既有隔离 R1 保留；独立 migrate 停机门禁、stop 幂等和显式非破坏性 PostgreSQL bootstrap 均已静态实现 |

五个 `gpt-5.6-luna` 子代理并行覆盖上述六个切面，主代理再按当前工作树复核高优先级结论。
子代理报告只作为定位输入；未能由双侧源码、配置或运行态复核的判断不进入“已证明差异”。

## 既有完成声明的处理

`docs/migration-implementation-plan.md` 已记录大量迁移工作，但其中“HTTP 路径无缺项”“浏览器路由
完整对应”“全部页面已接入真实 API”等表述只证明静态覆盖或作者声明。该文档同时明确保留以下证据
缺口，因此本次审计不会直接继承其完成结论：

- 真实 main/isekai 邮箱和 E-Hentai 的分页、限流、凭据失效、写入不确定结果与回执。
- 审计起点缺少 systemd/Caddy 环境中的启动顺序、readiness、失败回滚、Worker control 和凭据注入；
  当前已取得同宿主机隔离 R1，但真实域名/TLS、专用生产账号和流量切换仍不在证明范围。
- Public 动态行、认证动作、错误/冲突和移动端逐页状态，以及 Admin 实际浏览器产物的其余导航、字体、
  换行、焦点、空态和响应式布局。
- PostgreSQL 完整档、SQLite Lite 声明边界以及新旧双跑下的业务结果对照。

本次审计会把这些项目记为 `证据缺失`，而不是再次记为“已完成但待测试”。如果源码调用链已经证明
旧行为没有 owner、装配或状态分支，则直接升级为 `代码差异`，不等待运行态复现。

## 交叉复核清单

最终矩阵必须覆盖以下权威清单，并为每项给出 `等价`、差异、声明边界或证据缺失结论：

- `docs/contracts/routes.md` 的 Public 20 条、Admin 37 条业务 HTTP 路径及健康/metrics 边界。
- `docs/contracts/events.md` 的事件信封、注册表、catch-up、WebSocket 和 Worker 消费游标。
- `docs/contracts/invariants.md` 的 I-01 至 I-12，包括所有 I-05/I-09 子项。
- `docs/contracts/schema.md` 的身份、拍卖、结算/recovery、邮件/论坛、实时与告警对象 owner。
- 旧版 `public.routes.json` 14 条和 `admin.routes.json` 21 条浏览器路径，以及两套实际组件路由。
- Public/Admin 五种语言的资源闭集、默认语言、`hv.lang` 持久化和用户可见文案引用。
- 旧版 Worker 调度类别、五类 recovery action、外部传输重试/超时/游标和进程停止顺序。
- 主配置、敏感配置、Admin runtime、部署脚本、CLI 命令、数据库接管/备份/恢复和制品嵌入。

## 差异总表

本表只收录有双侧证据的差异或证据缺口。`建议实现` 描述实现方向，不代表已经授权跳过前置审计。

| ID | 切面 | 严重度 | 状态 | 旧版证据 | 新版证据 | 差异/缺失证据 | 建议实现 | 验收证据 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| BID-001 | 竞价事务 | P1 | 静态修复，待 R1 | `auction/postgres/adapter/repository.go::PlaceBid`、`eligibility.go::LockActiveForParticipation`、`proxy.go::SetProxy` | `CompetitionService::execute`、`CompetitionTransaction::participant_is_eligible_for_update`、两后端 `bidding/eligibility.rs` | 冻结基线时，新版没有在同一事务锁定、校验竞价者及活动代理 owner 的用户状态/当前黑名单资格，也不会先冻结已失去资格的代理；现已按 lot -> proxy -> 稳定 user 顺序补齐，并在主计划前原子应用冻结计划 | 已增加窄参与者资格端口和失败关闭映射；保持旧版只以用户参与状态为准，不把 `key_status` 撤销扩张为代理冻结条件 | PostgreSQL/SQLite 静态编译已通过；仍需同库并发 R1 证明被拉黑用户失败关闭，以及冻结、竞价事实、审计和事件同事务 |
| BID-003 | 普通竞价事实 | P1 | 静态修复，待 R1 | `competitionBidIDs` 与 `ordinaryBidFactCount` 在 fast 代理反击或任意代理同价胜出时为同一命令生成两条确定性 bid fact | `CompetitionCommand::trigger`、`competition_bid_ids` | 冻结基线时普通出价固定只生成一条 ID，领域规划器会在 fast 反击/代理同价分支失败；现已按锁内活动代理、金额和模式生成 1/2 条确定性事实 ID | 已沿用命令 ID + 稳定序号派生事实 ID，没有开放调用方注入随机第二 ID | 静态编译已通过；仍需同库 R1 核对压过代理为 1 条、fast 反击和同价代理胜出为 2 条，以及 fact index、状态版本、事件和重放连续 |
| BID-019 | Public 竞价日志与 Admin 代理历史的两代账本 | P1 | 静态修复，混存历史待 R1 | 旧 `auction/postgres/adapter/proxy.go` 固定写 `create_proxy_authorization`/`lower_proxy_authorization`、结果 `OK`；公开日志和管理历史按同一稳定种类读取 | 新命令 owner 已恢复旧 canonical kind/result；PostgreSQL/SQLite bid log 同时接受 canonical 和早期 Rust 的 `create_proxy`，Admin 历史只接受 `短 kind + accepted` 或 `canonical kind + OK` 两个精确世代，并关联追加式 transition/authorization 事实构造投影 | 旧矩阵曾错误宣称当前应以短 kind 为唯一事实源；这既与滚动升级账本合同矛盾，也会在改回 canonical 新写入后丢失早期 Rust 历史或错误解码两代不兼容 payload | 新写入只使用旧 canonical kind/result；只读防腐层显式接受两代精确组合，依赖权威转换事实，不猜测或改写历史 payload | 既有 fast 代理 R1 已证明短 kind 历史可进入 bid log，BID-015 已证明 canonical 新写与重放；本轮 Domain/Storage 定向静态编译通过，仍需同一混存库核对 Public 明细/total 与 Admin cursor/total |
| BID-008 | Public 正式拍品金额投影 | P0 | 已修复，1100 起拍价 R1 | 旧 `auction/postgres/adapter/public_read.go::mapPublicLiveLot` 不对存量起拍价执行竞价命令网格校验：无人领先时直接返回 `next_bid=start_price`；`mail/processing/startprice.go::parseStartAmount` 与 Admin 手工录入均允许任意正整数起拍价 | `database/lots/mapping.rs::map_live_fields` 现先按旧版校验正起拍价与价格关系；无人领先时直接投影起拍价，仅在存在领先者时才通过显式持久化金额入口执行下一口策略计算 | 冻结实现会让一个非整千但旧版合法的起拍价拖垮整个 Catalog；当前读取防腐层已修复，写命令的金额网格没有放宽 | 投影层保留任意正起拍价；HTTP 普通出价、代理上限和 Worker 命令继续使用 `BidAmount::new`，不得借兼容读取放宽真实竞价网格 | 隔离同源 JSON intake 以 1100 起拍，重放后两侧各只有一条 lot/submission；preview/open 均为 200。Public lot 详情两侧均为 `start_price=1100/current_price=1100/next_bid=1100/has_leader=false/status=live`。新版存储仍以 `current_price=NULL` 表示无人领先，由读取防腐层恢复旧公开投影；非整千写入继续按 BID-009 失败关闭 |
| BID-020 | Public 下一口最低价与普通竞价阈值 | P1 | 静态修复，待代理末口 R1 | 旧正式 Public `public_read.go::mapPublicLiveLot` 在有领先者时始终使用 `policy.NextBid(currentPrice)`；普通 `PlaceBid` 使用同一完整阶梯，等于该阈值即可受理。论坛模型才会在 `proxyLast` 时单独投影 `current + 1000` | 新 `database/lots/mapping.rs::map_live_fields` 曾把 `leader_proxy_last` 传入领域下一口计算，使 Public 显示论坛专属 `+1K`；普通竞价计划仍使用完整阶梯，因此页面会提示 1576K、提交 1576K 后又以过低拒绝 | 浏览器展示了后端不会接受的最低价，且错误文案继续引用同一旧投影，形成自相矛盾的操作提示；事务本身保持失败关闭，未接受低价 | Public 列表与详情不再读取 `leader_proxy_last`，统一以 `proxy_last=false` 计算普通下一口；论坛帖子继续由自身 owner 保留代理末口规则 | PostgreSQL/SQLite Storage 与 Public WASM 静态门禁；仍需构造“上一口为代理”的真实 lot，确认列表、详情、输入默认值和普通竞价成功金额一致 |
| BID-009 | Public 正金额网格错误外壳 | P2 | 已修复，认证 R1 | 旧 `publichttp/account.go` 只在 HTTP 层拒绝 `amount/maximum_amount <= 0`；正数随后由 `BidCommand.Validate`、`SetProxyCommand.Validate`、`LowerProxyCommand.Validate` 执行整千校验，其通用命令错误不在 `writeCommandError` 闭集中，稳定落为 503 `temporarily_unavailable` | 新普通出价、创建代理和降低代理 handler 在构造应用命令前直接调用 `BidAmount::new`，把任意非整千正数映射为 400 `invalid_amount` 或 `invalid_maximum_amount` | 同一认证用户、同一 live lot 上，零值普通出价/代理上限两侧均为 400 且 code/message 相同；1500/5500 正数非整千输入旧版均为 503，新版均为 400。该差异与 `BID-008` 的读取修复独立，也不代表旧版接受非整千写入 | HTTP 防腐层保留旧两阶段分类：非正数继续返回 400；正数但不在信用网格时返回旧依赖错误外壳，不进入数据库或命令账本。外部命令继续由 `BidAmount::new` 保持严格网格，不放宽 Worker/论坛命令；存量价格和自动代理事实只通过显式持久化防腐入口恢复正数 | 原生静态编译通过；同一认证用户在同源 live lot 上，普通出价与代理创建零值两侧均为 400，普通 1500、创建 5500、降低 55500 三种正数非整千请求两侧均为 503 `temporarily_unavailable`。整千 fast 创建及降低均为 200 且可精确重放；三个非整千 command_id 在新版账本中的计数为 0，未产生竞价、代理转换、审计或 realtime 事实 |
| BID-010 | 首笔竞价的无领先者投影 | P0 | 已修复，fresh Open/旧哨兵 R1 | 旧 v28 以 `current_price=start_price`、`leader_user_id=NULL` 表示尚无领先者；旧代理创建会把该投影映射为领域无领先状态并产生首条竞价事实。新版 `legacy_v28/bridge.sql` 也明确把该旧哨兵转换为 Rust 使用的 `current_price=NULL` | 新 PostgreSQL/SQLite Open 计划却重新写入 `current_price=start_price`；竞价快照随后直接映射为 `Some(current_price)` 与空 `leader_id`，触发 `LotBidState::validate` 的一致性拒绝 | 在新建、录入、preview、open 均已逐字段一致的同源 isekai 场次上，旧版 fast 代理首次及重放均为 200；新版同一整千上限在进入首笔事实前返回 503 `storage_unavailable`。数据库锁内快照显示 `current_price=50000` 且无领先者，精确命中该矛盾 | 两后端 Open 恢复写 `NULL`；竞价快照仅为已由旧版接管或缺陷版本写出的精确 `current_price=start_price + no leader` 哨兵做防腐映射，转为领域 `None`。无领先者但价格高于起始价、或有领先者却缺价格仍失败关闭，不修改历史事实 | PostgreSQL/SQLite 静态编译通过；同源 fresh 场次 preview/open 首次与重放均为 200，开拍后旧库为起拍价哨兵、新库为 `NULL`，Public 均投影 `current_price/next_bid=50000` 且 `has_leader=false`；旧哨兵行上的 fast 代理也已成功提交并重放 |
| BID-011 | 首笔起拍价竞价事实 | P0 | 已修复，普通/fast 局部 R1 | 旧版无领先者时以起拍价作为下一口；普通竞价或新代理的首条公开事实可以等于起拍价，写入后才建立当前价与领先者 | 新 `competition/plan/state.rs::finalize_plan` 把 `current_price.unwrap_or(start_price)` 统一当作已有上一口，并只允许“已有 current 的代理事实”同价，导致 `current_price=None` 时普通/代理首笔等于起拍价均返回 `BidTooLow` | 修复 `BID-010` 的快照矛盾后，同一 isekai lot 已从 503 前进到 409 `bid_too_low`；旧版同一用户、起拍价 50000、fast 上限 100000 已返回 200，且首条代理 bid 正是 50000。两侧均无此前 bid/proxy，排除真实最低价变化 | `finalize_plan` 只在 `state.current_price=None` 时允许首条事实等于起拍价；已有 current 时继续拒绝同价普通竞价，仅保留既有代理内部同价事实分支。低于起拍价、状态/领先者矛盾和后续同价写入仍失败关闭 | 同源对齐策略/lot 后，普通首价 50000 首次与重放两侧均为 200，均只写一条 ordinary fact、状态版本 3 和一条 realtime；fast 创建同样以起拍价写首条 proxy fact 并可重放。slow 首价、后续同价拒绝和 fast 反击双事实仍保留后续 R1 |
| BID-012 | PostgreSQL 代理转换写入 | P0 | 已修复，fast 创建/降低 R1 | 旧代理创建在同一事务写授权、created transition、首条 bid、审计、事件和 lot 投影；成功后可精确重放 | 新 PostgreSQL `bidding/proxy.rs` 的 transition INSERT 列表含 13 列，却使用 `$1..$13` 再追加 `CURRENT_TIMESTAMP` 共 14 个表达式；实际只绑定 12 个业务值。SQLite 同形语句正确使用 12 个占位符 | `BID-010/011` 修复后，同源请求已通过快照与领域规划，PostgreSQL 日志精确报告 `INSERT has more expressions than target columns`，HTTP 回到 503 且 command/bid/proxy 计数均为 0，证明事务完整回滚但所有 PostgreSQL 代理转换不可达 | 把 PostgreSQL VALUES 恢复为 `$1..$12, CURRENT_TIMESTAMP`，不改列、绑定顺序、transition 版本或时间 owner；SQLite 无此差异不修改 | PostgreSQL 静态编译通过；同源 fast 创建/重放与降低/重放两侧均为 200，created/lowered transition、授权版本、唯一首条 bid、lot、账本和 realtime 原子事实对应。slow、耗尽/final/freeze/close 分支仍待 R1 |
| ID-001 | 黑名单 | P1 | 静态修复，待 R1 | `users/.../admin_repository.go::BlacklistAction`、`blacklist_repository.go::freezeBlacklistProxies/recordBlacklistEffects`、`system_blacklist_repository.go::Process` | PostgreSQL/SQLite `users/admin/*/commands/blacklist.rs` 与 `mutation.rs` | 冻结基线时 Admin 与 recovery 黑名单只更新用户、投影、历史和审计；现已在同一事务按 lot -> proxy -> user 顺序冻结 `active/final_fast` 代理、追加确定性拍卖子命令/转换、清理慢速调度并撤销全部 Public 会话 | PostgreSQL Admin/recovery 共用 effects；SQLite Lite 的 Admin 拉黑使用 `BEGIN IMMEDIATE` 落同形证据，recovery 仍保持完整档限定 | 两后端静态编译已通过；仍需同库 R1 证明用户状态、代理冻结、会话撤销、历史和审计任一步失败均整体回滚 |
| ID-002 | Admin 手工登录 key | P1 | 已修复，隔离认证 R1 | 旧 `users/admin.go::validManualKey` 与 `admincrypto/codec.go::validKey` 接受 32–256 字节、有效 UTF-8 且无控制字符的手工 key；无一次性 key 时响应以 `omitempty` 省略 `generated_key`；隔离旧 Admin 对同一非 Base64URL key 返回 201 | `MailLoginKey::from_admin_manual` 已恢复旧入口闭集，`from_encoded` 继续只供系统签发/MoogleMail canonical key；`AdminUserMutationResponse.generated_key` 在 `None` 时省略 | 冻结新版错误把系统签发 key 的严格闭集套到了旧 Admin 手工入口，并多序列化 `generated_key:null`；现已分离两个信任边界且不改变摘要、信封和一次性返回语义 | Admin 手工入口保持 32–256 字节、UTF-8、无控制字符；生成及邮件换绑继续只接受 canonical Base64URL，明文只在请求和一次性响应中短暂存在 | 隔离 PostgreSQL 上同一非 Base64URL key 新旧均为 201 `created`、`data` 只含 `user`、精确幂等重放保持同一用户，Public 登录均为 200；31/257 字节和 32 字节含控制字符三组新旧均为 400 `invalid_user_command` |
| API-005 | Admin users 命令契约 | P1 | 已修复，隔离认证 R1 | 旧 `users/adminhttp/mutations.go::create/update/writeCommandError`、`blacklist.go::blacklist` 与 `renames.go::revertRename/writeRenameError` 先按字符串分别解析 command/user/rename UUID；创建成功使用 `created`；普通用户命令、改名回滚、版本/命令/名称冲突和依赖错误各有稳定 code/message/data，黑名单成功统一返回“用户黑名单状态已更新” | users wire DTO 已改为字符串 ID 并按 Go 零值兼容缺失/`null`，`common.rs` 分离 list/user/revert 映射，`commands.rs`/`status.rs` 恢复规范小写 UUID 分类、旧字段闭集及成功 code/message | 冻结新版曾把非法 UUID 落入 `invalid_json` 或错误 ID 分类、暴露新错误 code、合并冲突、丢失版本数据，并在 mutation 接受旧 wire 不存在的 `public_reason`；现已恢复旧 wire 和映射且不改变应用/存储端口 | 普通用户与改名回滚保持独立错误 taxonomy；版本冲突固定返回 `current_state_version`，owner 损坏/key/存储失败回到旧依赖错误外壳；`request_id` 仍按 `API-004` 作为非敏感扩展 | 隔离认证 R1 已逐项同得：malformed/大写/缺失/`null` command ID，非法 user ID/action/未知字段，not-found，state/command/user conflict，创建与幂等重放，生成 key，黑名单/解封，以及改名回滚的 stale/not-found/invalid/success/command-conflict/not-revertable；所有状态、code、message、版本数据和业务投影均一致 |
| API-006 | Admin 拍卖生命周期成功响应 | P1 | 静态修复，待 R1 | 旧 `auction/adminhttp/adapter.go::openPreview/openAuction/cancelAuction/enterFinalPhase` 分别返回动作专属 message，`data.auction` 为命令提交时的完整拍卖快照；幂等账本保存同一结果 | 生命周期事务现已构造并持久化旧字段闭集的拍卖快照，HTTP 返回 `data.auction` 和四个动作的专属 message；Web Admin 已改读嵌套结果，旧最小账本 payload 只在数据库仍与原结果完全一致时安全补全 | 冻结新版曾返回平铺最小投影和统一 message，成功 wire、可见文案及幂等重放结果均不同；现已在命令事务内闭合，避免事后读取把后续状态冒充原命令结果 | 保留完整快照随幂等账本提交；对旧最小账本失败关闭，不以漂移后的当前状态伪造历史响应 | PostgreSQL/SQLite 静态编译、Admin WASM 和全工作区 Clippy 已通过；仍需同源 draft→preview→live→final 及 cancel 分支逐字段 R1，并核对同 command_id 重放仍返回原快照 |
| API-007 | Admin 拍卖创建/录入成功契约 | P2 | 静态修复，待 R1 | 旧 `auction/adminhttp/adapter.go::createAuction/addIntakeItem` 均以 HTTP 201、envelope code `created` 返回；创建的 `data.auction` 使用命令快照字段闭集，录入 data 为平铺结果 | 两个 handler 现已统一使用 `ApiResponse::created`；创建改用命令专属拍卖 DTO，列表扩展字段只留在列表/lot 页面，Web Admin 同步读取命令响应 | 冻结新版虽返回 HTTP 201，却写出 code `ok`，创建还泄露列表专属字段；现已恢复旧客户端可校验的 envelope 和字段闭集，未改变事务与应用结果 | 保持创建、录入首次执行和精确重放使用同一旧版成功 wire；列表 DTO 不再复用于命令响应 | 原生静态编译、Admin WASM 和全工作区 Clippy 已通过；仍需同源创建/录入首次执行与精确重放核对 201/`created`/message/data 键集合 |
| API-008 | Admin 拍卖命令错误契约 | P1 | 静态修复，待 R1 | 旧创建/四类生命周期 handler 先按规范小写 UUID 字符串解析；命令字段无效统一为 `invalid_auction_command` 加动作专属 message；owner 错误稳定映射为 `not_found`、`stale_auction_state`、`invalid_auction_state`、`command_conflict` 或依赖错误 | wire 边界现已恢复字符串/Go 零值分类和规范小写 UUID 校验，创建与生命周期 handler 使用旧版动作专属 `invalid_auction_command`，并把应用/存储错误恢复到旧 taxonomy | 冻结新版曾让 malformed/大写/缺失值落入 `invalid_json` 或细分 code，领域非法状态、stale 及存储 `InvalidState` 也会被错误分类；现已恢复既有客户端可识别的错误外壳，内部继续使用带类型 ID 和版本 fence | 在 HTTP 防腐层兼容旧输入分类，不放宽领域类型、事务 fence 或严格 JSON；依赖故障保持旧版 503 外壳 | 原生静态编译、Admin WASM 和全工作区 Clippy 已通过；仍需认证 R1 覆盖 malformed/大写/缺失/`null` ID、零/陈旧版本、非法状态、not-found、command conflict 与依赖失败 |
| API-009 | Admin 拍品录入输入/错误契约 | P1 | 静态修复，待 R1 | 旧 `auction/adminhttp/adapter.go::addIntakeItem/canonical` 用字符串解析规范 UUID；来源组合与 canonical 必填字段缺失返回 `invalid_auction_command`，价格/玩家等应用校验返回 `invalid_auction_intake`；owner 还区分 `auction_intake_unavailable`、`seller_blacklisted`、`equipment_conflict`、`command_conflict`，其余依赖错误使用稳定外壳。旧 canonical 要求非空 quality、允许已提供的空 stats，并在摘要前 trim `player_id` | wire 已恢复字符串/零值和 pointer-heavy 必填分类；HTTP 在调用应用端口前校验来源组合、canonical presence/value 与规范 UUID，应用服务在校验及摘要前 trim 玩家 ID。PostgreSQL owner 现保留无草稿、黑名单卖家、装备重复和命令唯一冲突的独立原因，HTTP 恢复旧 code/message | 冻结新版曾把 malformed/大写/缺失/`null`、来源结构及 canonical 边界分到错误 taxonomy，且把真实 owner 冲突压成通用错误；现已恢复旧客户端的可行动分类，同时纠正空 quality/空 stats 的相反边界 | HTTP 防腐层兼容旧 wire，领域内仍使用带类型 ID；严格 JSON、装备 key 清零、Serializable 事务、幂等账本、审计和 realtime 原子写入均未放宽 | 工作区静态编译、PostgreSQL/SQLite feature、Admin WASM、嵌入式 Web payload、策略检查和全工作区 Clippy 已通过；仍需认证 R1 覆盖非法 ID、缺字段/`null`、URL/JSON 组合、空 quality/空 stats、玩家 trim、无草稿、黑名单、装备重复、命令冲突，以及同源成功与精确重放 |
| API-010 | Admin 拍卖/lot 列表服务端响应闭集 | P1 | 已修复，认证 R1；浏览器 decoder 见 API-038 | 旧 Admin Web `auctionKeys` 对拍卖对象使用严格 `only`：允许可选 `scheduled_ends_at/lot_count`，不允许 `scheduled_end_at/effective_end_at`；`isLotList` 只允许 `auction/lots/pagination`。旧 `mapAuctionSummary/mapAuctionWithLotCount` 分别填充汇总结束时间和 lot 数量 | 当前 HTTP 拍卖列表和 lot 列表使用旧拍卖 DTO 闭集；lot body 只含 `auction/lots/pagination`，`manual_intake` 通过 `x-auction-capability-manual-intake` 响应头提供。浏览器是否逐层拒绝坏响应独立归 API-038 | 冻结服务端曾扩张两个时间字段并把 capabilities 放进 body，旧 Web 会拒绝整个页面；当前已恢复旧 v1 JSON 输出闭集，同时保留非破坏性能力头，但不能由服务端正常样本外推浏览器 decoder 已等价 | 服务端列表 DTO 只保留旧可选字段；能力不进入 body，也不让浏览器猜数据库档位。客户端成功响应闭集和跨字段语义由 API-038 唯一拥有 | 隔离认证 R1 中两侧 auction list 均为 200，根键同为 `auctions/capabilities/pagination`，auction item 键集合逐项相同且无两个禁用时间键；lot list 均为 200，根键同为 `auction/lots/pagination`，body 均无 capabilities。新版响应头精确给出 `x-auction-capability-manual-intake=true` |
| INT-001 | Admin 手工录入持久化 | P0 | 已修复，JSON intake R1 | 旧 `postgres/adapter/admin_intake.go` 经 `InsertAdminAuctionSubmission` 把投稿 owner 稳定写为 `source_kind='admin'`，URL/JSON 仅属于请求适配器与来源引用；schema 闭集为 `mail/admin` | `database/admin_intake/write.rs` 现固定写 `source_kind='admin'`；请求来源继续保留在 `manual-json-v1:<external_id>` 来源引用、指纹和命令摘要中 | 冻结实现把 `url/json` 写入只允许 `mail/admin` 的 owner 列，导致 PostgreSQL 全部手工录入回滚；当前已消除该不可达路径 | owner 字段固定为 `admin`，不得把 HTTP 适配器种类扩散到持久化 owner 闭集 | 隔离同源 canonical JSON（空 stats、10 位 key、1100 起拍）两侧首次均为 201；数据库均得到 `source_kind=admin`、`source_reference=manual-json-v1:9900006`、一条 submission/lot，拍卖从 v1/revision0 推进到 v2/revision1。同命令重放仍为 201 且计数不增；装备冲突和异负载命令冲突仍待单独 R1 |
| LIF-001 | 开放预览的内容修订版本 | P1 | 已修复，单 lot R1 | 旧 `postgres/queries/auction.sql::OpenPreview` 只推进 `status/state_version`、开放时间和人工标记，返回并沿用录入/lot 编辑形成的既有 `preview_revision`；该字段表示预览内容修订，不表示生命周期次数 | PostgreSQL/SQLite preview plan 现只推进 `state_version`，物化项绑定现有 `preview_revision` | 冻结实现会制造没有内容变更对应物的修订号，导致预览物化项、事件和客户端 revision fence 错位；当前已恢复旧语义 | 开放预览只推进状态版本；录入、lot 编辑和 recovery relist 继续拥有内容修订号 | 隔离同源单 lot draft 在两侧均从 `draft v2/revision1` 以 200 开为 `preview v3/revision1`，`auction_preview_items.source_revision=1`；随后 open 均为 200 且拍卖为 `live v4/revision1`。失败的前置预览命令没有写事实；精确成功重放仍待补录 |
| INT-002 | Admin 手工录入幂等摘要 | P0 | 已修复，变 request_id 重放 R1 | 旧 `admin_intake.go::adminIntakeDigest` 只摘要来源、world、装备事实、价格、规范化玩家和 actor，不包含每次 HTTP 请求都会变化的 `RequestID`；同一 `command_id`、同一业务负载会精确重放原结果 | `AdminIntakeCommand::request_hash` 现明确排除 `request_id`；请求 ID 仍进入追加式审计元数据 | 冻结实现会把网络重试或反向代理重发误判为异负载冲突；当前已恢复首要幂等不变量 | 观测性 `request_id` 不进入业务摘要；actor、业务负载和装备 key 摘要仍受命令 fence 保护 | 隔离同源 intake 首次与使用不同 `x-request-id` 的同命令重放，两侧均为 201；两侧最终各只有一条命令、submission 和 lot，拍卖版本/修订不重复推进。异业务负载仍须核对 409 `command_conflict` |
| BID-002 | 代理命令 | P1 | 静态修复，待认证 R1 | `auction/publichttp/account.go:294-315` | `CreateProxyAuthorizationRequest.mode` 对缺失字段投影为空串，`proxy.rs` 只接受 `fast`/`slow` | 冻结基线时空字符串会进入策略默认模式；现已恢复缺失、空值和未知值统一 `invalid_proxy_mode` 的失败关闭语义 | v1 继续要求显式 `fast`/`slow`；默认模式只属于内部冻结策略，不作为 v1 请求缺省值 | DTO/handler 静态编译通过；仍需有用户和 live lot 的认证 R1 核对三种失败及两种成功语义 |
| WRK-001 | Auction Worker | P1 | 静态修复，待 R1 | `auction/worker/loop.go::ListDueSlowProxies/ExecuteSlowProxy`、`postgres/adapter/worker_state.go` | `ports/auction/slow_proxy.rs`、`CompetitionService::execute_slow_proxy`、`database/slow_proxy_due.rs`、`worker/auction/slow_proxy.rs` | 冻结基线时只有 `SlowDue` 领域规划和 due 索引，没有扫描/执行 owner；现已恢复数据库时钟无锁有界扫描、旧版确定性命令 ID/请求摘要、generation/due/authorization fence、数据库执行时间和 stale candidate 成功跳过 | PostgreSQL 使用 Serializable 三次重试，SQLite 使用 `BEGIN IMMEDIATE`；失格 owner 在同一事务冻结代理，成功计划写入旧版 `lot.slow_proxy_tick` Worker 审计，不引入内存游标或第二套状态机 | 两后端静态编译/Clippy 已通过；仍需同库 R1 证明重启恢复、并发重复扫描、截止/Final 竞争、失格冻结，以及命令账本/竞价/审计/事件整体回滚与重放 |
| WRK-002 | 论坛同步 | P1 | 固定双帖静态实现，Worker API 待白名单外部 R1 | 旧版只在拍卖首次进入 `live` 时建立主题任务，`CreateTopic` 每次执行先按精确标题对账；已发布主题不随 `state_version` 重新发布；旧版没有固定 #2 或远端编辑接口 | 新版保留一次建主题与未知写入栅栏，并按产品扩展固定 #1 拍卖目录、#2 编辑/错误记录、#3+ 用户命令；`000025` 持久化两个 post ID、内容版本和独立内容 lease/fencing，竞价后按 exact ID 编辑 #1/#2，既有 #2 非系统标记时失败关闭 | 双帖是明确新增能力而非旧版等价声明；CODE=03/09 表单、远端 post proof、历史主题补齐、编辑未知结果、内容 lease/fencing 和 CAS 已静态接线，但白名单 Worker 真实 transport 尚未运行，不能把浏览器手工发帖证明外推到该链路 | 只有 HTTP 401 或明确登录表单/失效文案才作废 Cookie；固定楼层写入必须校验同源 `/index.php` 表单的 board/thread/post，未知回复创建需先只读对账/人工授权；固定 post 编辑允许同目标幂等重放但不得改写其他楼层 | 2026-09-01 Chrome DevTools MCP 已只读核对真实 reply/edit 表单字段，并在测试板块验证参考主题分页和既有主题形状；新 Worker 的 #1/#2 创建、编辑、数据库 post proof、命令后价格刷新和错误日志仍待白名单 IP 机器 R1 |
| WRK-003 | 邮件轮询 | P2 | 静态修复，待外部 R1 | `mail/worker/loop.go::process/isTemporary` | `ApplicationError::mail_processing_retryability`、`worker/mail/poll/processing/business.rs::process_with_retry` | 冻结基线时所有业务错误都会重试到上限；现已冻结 `Retryable`/`Permanent` 分类，确定性输入、配置和业务拒绝立即以 `business_processing_permanent` 收口，超时及可重试依赖错误才执行有界 inline retry | 游标、claim 和持久化失败退避仍由原 owner 维护；永久错误不会通过重复执行绕过失败关闭 | 静态编译已通过；仍需真实邮箱 R1 核对错误分类的实际尝试次数，以及 cursor/lease 在失败时保持不提前推进 |
| RT-001 | Public WebSocket 与浏览器状态推进 | P1 | 核心 merge/refetch 已静态修复，待异常帧/重连 R1 | 旧浏览器 `mergeEvent` 同时维护 cursor、seen event ID、bid fact identity 和 `state_version/fact_index`；完全相同的重复事件/事实为 no-op，同一事实身份内容冲突、跳号或版本回退才进入权威恢复。`refetch_required` 清空事实缓存但保留 `after=max(old_after,next_after)`，现有连接继续运行；单拍卖 WS 还会把带 world 的事件与当前页面 world 核对；核心连接提示为 connecting/degraded/live/manual，外层 readiness 还可呈现 idle/unavailable | 新 `RealtimeCache` 已恢复有界 event/fact 去重、冲突与 state/fact fence；握手后 HTTP catch-up、WS 帧和 refetch 共用同一严格事件 decoder。单拍卖 WS 目标 key 同时包含 auction ID 与 world，带 world 的 payload 必须匹配当前页面；refetch 不再清零游标或结束 socket，而是刷新权威快照并保留高水位；真实断线按旧 3 秒间隔重连。页面已恢复核心四态，discovery 与单拍卖分别持有游标/缓存 | 核心 merge、refetch、游标、目标 world 和重连静态差异已关闭；旧 readiness 驱动的 idle/unavailable 仍未作为独立徽章状态恢复，连续/重复/冲突/跳号的真实浏览器时序也尚无 R1 | 保持单一 realtime owner 和目标切换取消语义；补齐或明确登记 idle/unavailable 外层呈现，再以同部署异常帧证明缓存、快照刷新、连接保留、world fence 和 3 秒重连 | Application/Public 原生、Public WASM 与两后端 Storage 定向 check，以及 Application/Public `-D warnings` Clippy 已通过；仍需同部署连续事件、完全重复、冲突事实、跳号、refetch、目标切换和断线重连 R1 |
| RT-002 | WebSocket v1 wire 与连接边界 | P1 | 已修复，1009/preview gate/live 握手 R1；容量/计时器待证 | 旧版 `/api/v1/ws` 在启用 allowlist 时要求且只接受一个可信 `Origin`，随后按 query→session 顺序失败关闭；query 只接受一个 `auction_id` 或 `scope=auctions`。升级后无 hello/ack，文本帧为单个六字段事件；读取上限 4 KiB，Ping/Pong/写入边界为 50 秒/60 秒/5 秒。客户端发送超过上限的消息时返回空 reason 的 1009；慢消费者、事件合同错误、客户端 Text/Binary 和服务关闭分别发送 1013/1002/1008/1001。只有 discovery 受 128 连接上限。preview 单拍卖仅在同一 world 不存在 live/final/settling 拍卖时允许建连，live/final/settling 允许，settled 拒绝 | 新版六字段事件、50/60/5 秒、四类显式 close、discovery 槽与核心快照校验已经建立；WebSocket owner 已把类型化 `MessageTooLong` 映射为空 reason 1009。PostgreSQL/SQLite `realtime_status` 都已补齐 `LEFT JOIN lots AS l`，并保留同 world preview `NOT EXISTS`、settled 拒绝和完整快照校验。新版在 Origin 之前保留独立 per-IP 连接尝试限流，并额外拒绝 `scheduled_ends_at < opens_at`，两者登记为安全/完整性强化；旧 projection 的 ForumSync 与 FinalRestriction 经调用链复核不参与 WebSocket 预检 | 4097 字节 EOF、全部单拍卖预检 503 以及同 world 已有活动拍卖时 preview gate 被掩盖的已知差异均已关闭。核心快照损坏、50/60/5 秒真实计时器、128+1 容量、慢消费者和普通底层读写失败仍不能由本次局部证据外推 | 保持 WebSocket owner 的类型化 1009、两后端 `lots AS l` 左连接、preview 同 world `NOT EXISTS`、settled→404 和完整快照校验；不得把 ForumSync/FinalRestriction 加入应用端口。per-IP 门禁与反向时间拒绝继续明确保留，剩余容量/计时器/损坏快照按独立 R1 收口 | 旧 8092/原新 8090 的受控会话证明 1/4096 字节均为 1008，4097 字节旧版为 1009、原新版为 EOF；修复候选 18090 的 discovery 为 101，masked 4097 字节 Text 返回空 reason 1009。一次性 PostgreSQL clone 中，同 world preview 握手为 404、live 为 101；修复前原 SQL 明确报 `missing FROM-clause entry for table l`。clone、会话材料和 18090 监听在零连接后精确清理，数据库/路径/监听残留均为 0。release 可执行文件 SHA-256 为 `850f77efbb45c3c08c33931280ce35f4e69f675b86c27e0ac945785abe7731c6`，已接管常驻 8090；`/proc` 映像哈希一致，本机 root/live/ready、LAN 根页和全部 HTML 引用资源均为 200 |
| RT-003 | Catch-up HTTP 严格 envelope、缺口与 cursor | P1 | 严格 decoder/缺口高水位已静态修复，待畸形/缺口 R1 | 旧单拍卖合同要求规范 `auction_id`、默认 100、顶层 `auction_id`。共享外层 envelope 要求 `code/message/data` 但允许非破坏性扩展；`data`、event 和每种 payload 使用精确字段闭集。服务端遇 sequence 缺口或坏投影会置 `refetch_required`，继续扫描本次有界原始页并把 `next_after` 推进到原始高水位；浏览器看到该标记后不合并本页事件，只重置事实缓存并保留恢复游标。旧服务端零时间是 Go `time.Time{}`，不是 Unix epoch | 新 HTTP/WS 现共用 raw JSON 防腐 decoder：page/data/event/payload 精确校验字段、类型、规范 UUID、JS safe integer、world、时间、状态和枚举，外层 envelope 保持旧扩展边界。服务端时间校验只拒绝精确 Go 零时间并允许 Unix epoch。PostgreSQL/SQLite 都会在缺口/坏投影后继续有界扫描、隐藏坏事件、标记 refetch 并以原始行推进 `next_after`；discovery 继续使用隔离的严格 page 和游标 | 已知宽松反序列化、错误零时间判断、缺口处提前停止和 refetch 游标清零的静态差异已关闭；畸形 page/event/payload、缺口后合法事件、高水位和浏览器“不合并本页事件”的组合仍缺运行态 R1 | 保持内部 `stream_key` 不出 wire，外层 envelope 的兼容扩展与严格内层分开；以 PostgreSQL/SQLite 受控缺口和畸形行核对事件数组、`next_after`、refetch、快照刷新及 socket 保留 | 临时 SQLite 既有证据已证明 discovery 200、无目标/非规范 after 400、未知拍卖 404；本轮 Application/Public/Storage 两后端定向编译与 Clippy 已通过，仍需同数据连续分页、畸形字段和缺口页 R1 |
| RT-004 | Admin realtime | P1 | 静态修复，局部 R1 | `realtime/adminhttp/adapter.go:121-149` 在 `data.pagination` 返回分页 | `AdminRealtime*ListResponse` 与 reads handler 已恢复 lag/failure 的嵌套分页，Admin UI 改读 `page.pagination` | 冻结基线时 A21/A22 同一 v1 路径把页码平铺；现已恢复旧字段位置 | failure 的 `has_more` 保持在其 pagination 内，内部查询页模型不变 | 临时 SQLite 已认证请求实际返回两种 `data.pagination`；仍需有数据翻页与 PostgreSQL R1 |
| RT-005 | Admin replay | P1 | 静态修复，待 R1 | 旧 `replayRequest` 只有 `schema_version/public_payload` | DTO 接受可选 `command_id`；缺失时 `replay.rs` 按 failure/schema/规范化 payload 以 UUID v5 稳定派生 | 冻结基线时旧 A24 请求直接 400；现有兼容层不删除核心命令 ID，新 Dioxus 仍显式发送并复用 ID | 旧 wire 与新版 wire 汇入同一 `RealtimeReplayCommand`、request hash 和命令账本 | 静态编译/Clippy 通过；仍需同库 R1 证明旧请求重试不重复事件及新 ID 异摘要冲突 |
| API-001 | Admin delivery | P1 | empty/filter/page2 HTTP R1；populated 待证 | `delivery/adminhttp/adapter.go:180-185` 使用 `data.pagination` | `AdminDeliveryListResponse`、handler 与 Dioxus 表格/指标已改回嵌套分页 | 冻结基线时 A34 同一路径平铺分页；现已恢复旧字段位置 | DTO owner 保持独立，delivery 内部页模型不变 | embedded 新旧已认证 Admin 分别请求默认页、`page=2&page_size=10` 与 `status=failed`，六次均为 200，raw JSON 逐字同形：`data={deliveries:[],pagination:{page,page_size:10,total:0}}`，顶层同为 `code/message/data`；page 2 也保留请求页码而不平铺或提前 clamp。当前两库均无 delivery，仍需 populated 行及实际多页 R1；探针未保存为脚本 |
| API-002 | JSON 写边界 | P2 | 静态修复，局部 R1 | `platform/httpx.DecodeJSON` 校验 `application/json`、body 上限、未知字段和尾随 JSON，并映射 413/415 | `shared/strict_json.rs` 统一媒体类型、路由上限和单值校验；所有请求 DTO 保持 `deny_unknown_fields` | 冻结基线时各 mutation 只依赖本地 `from_slice`；现已恢复共享 400/413/415 和每路由原上限 | 严格边界读取后放回原始 bytes，类型化 DTO owner 不迁入 middleware | 旧 8092 与临时当前 18092 的 Public session 探针均得到非 JSON 415、未知/尾随 400、超限 413；Admin 与大 body 路由仍需认证 R1 |
| API-003 | HTTP method | P2 | 静态修复，局部 R1 | 旧 8092 `HEAD /api/v1/worlds` 为 405、`Allow: GET`；session 为 `DELETE, GET, POST` | `shared/method_strict.rs` 给全部业务 MethodRouter 注册显式 HEAD/fallback；健康/metrics/静态 owner 不受影响 | 冻结基线时 Axum `get` 无意接受 HEAD；现已恢复业务方法闭集和 JSON 405 | `Allow` 由每条业务路由的实际方法显式冻结，避免把隐式 HEAD 暴露给客户端 | 临时当前 18092 已得到 worlds 405/GET 与 session 405/DELETE, GET, POST；health live/ready 仍与旧版同为 HEAD 200 |
| API-004 | 错误 envelope | P2 | 静态修复，局部 R1 | 旧错误固定包含 `{code,message,data:null}` | `ApiErrorResponse.data` 不再在 `None` 时省略，严格 JSON/方法/既有 handler 共用该 DTO | 冻结基线时无业务数据会省略 `data`；现已固定输出 `null`，保留非敏感 `request_id` 扩展 | 带稳定业务数据的错误 DTO 不变，禁止把敏感上下文塞入扩展 | 临时 18092 的 400/405/413/415 均实测包含 `data:null`；其余映射静态继承同一 DTO |
| PUI-001 | Public 规则页/i18n | P1 | 已修复，五语言同数据浏览器 R1 | 旧规则页按当前 locale 请求 `GET /api/v1/rules?lang=...`，成功态只含标题和默认 Markdown 排版 | `fetch_rules(locale)` 响应式读取同一 locale；成功态恢复 `public-rules-page` 两层结构并使用无主题块渲染 | 冻结基线时切换语言不重读正文，修复数据后又发现新版额外显示 revision/published metadata、正文宽度和共享 Markdown 主题不同；现均已移除/隔离 | 五个 Fluent locale、`hv.lang`、`<html lang>` 和规则 API locale 保持同一闭集；Dioxus 安全文本 span 与透明 table wrapper 不改变可见/语义结构 | 旧 8092 与新 18190 五语言标题、正文文本 SHA-256、全部测量矩形、计算样式、文档高度和整篇规则 PNG SHA-256 逐项相同；WASM/bundle 通过 |
| PUI-002 | Public 登录返回 | P1 | 静态修复，匿名浏览器 R1 | 旧 LoginPage 保存来源 pathname + search，默认返回 `/` | `Route::Login.return_to`、CTA 百分号编码的当前 pathname+search 与 `safe_return_path`/navigator replace | 冻结基线时依赖 `history.back()` 且无历史去 `/me`；现已恢复确定来源和直接登录默认 `/` | 只接受单 `/` 开头、拒绝 `//`、反斜杠、fragment、控制字符和 login 自循环，避免开放重定向 | `/login` 新旧 1440×1000 PNG SHA-256 同为 `ab95abe4c31f588663a0895d46d891d21916c76669d9722a4d971a412b56f190`；CTA 已保留完整来源，成功登录/恶意 return 仍需带有效会话 R1 |
| PUI-003 | Public URL 状态 | P2 | 静态修复，浏览器 R1 | 旧 bidlog 使用 `page`，world 属于页面 URL 状态 | `query.rs` 只规范化写入 `page`，在 `/bidlog` 兼容读取 `bidlog_page`；页面内 world 与 Router 前地址快照/`popstate` 重建共同保留 URL 状态 | 冻结基线时旧收藏/分享链接不能恢复同一页码和筛选；现已恢复直接打开、canonical 收敛及同路径历史导航 | canonical `page` 优先，切换 world/场次/lot 清除失效下游状态；历史 state 继续保留 Dioxus 滚动坐标 | Chromium 18190 已确认 `?bidlog_page=2&world=isekai` 收敛为 `?world=isekai&page=2`，`page=3` 与别名并存时取 3，前进/后退时页头和页面内 world 均同步 |
| PUI-004 | Public 历史展示 | P2 | 静态修复，空态浏览器 R1 | 旧拍卖历史含状态列和直接 Bid log 链接，默认页大小 10 | `/auctions` 已恢复 10 条请求、状态徽章和 Lots/Bid log 直接入口 | 冻结基线时信息密度、操作入口和分页边界不同；当前结构、请求边界及一页空态布局已恢复 | 继续使用稳定 DTO，不从 SQL 行拼 UI；其它列表的页大小按各自旧契约单独审计 | Chromium 对 settled/live 均实际请求 `page_size=10`，1440×1000 空态筛选、说明及一页 pager 与旧版视觉对应；隔离数据没有可见行，状态徽章、链接和多页总数仍需同数据 R1 |
| PUI-005 | Public 非法 lot 路由 | P2 | 静态修复，浏览器 R1 | 旧 `/lots/not-a-uuid` 在客户端进入 not-found，不发业务请求 | 路由页先按旧 UUID 闭集校验并渲染五语言本地 not-found | 冻结基线时无效路径从确定性空态变成依赖错误态；现已恢复客户端确定结果 | 非法标识不进入 lot API，合法 UUID 继续走权威读取 | Chromium 18190 显示 “Lot not found”、无 Retry，performance 记录中没有 `/api/v1/lots/...` 请求 |
| PUI-006 | Public 状态页 | P3 | 静态修复，浏览器 R1 | 旧 `/status` 是单一静态服务说明，只显示一次 “Service status” | 页面恢复同一 section/H2/说明和旧版局部排版；health/realtime 诊断继续由机器 API 与页头状态拥有 | 冻结基线时重复标题和原始诊断表改变信息层级；现已移除页面内第二套诊断 surface | 不用“增强”扩张已冻结 WebUI；运维证据仍可从既有安全接口取得 | 新旧浏览器均为单 H2、零 details/table，H2 与说明矩形完全一致；整图仅动态页头区域 36 个像素相差 1 色阶 |
| PUI-007 | Public 静态制品 | P3 | 静态修复，制品 R1 | 旧 Public/Admin title 分别为 `Auction 公开站点`/`Auction 管理站点`，HTML 引用指纹 favicon | 两个静态入口恢复旧 title，并以稳定 `/favicon.ico` 提供同一共享 SVG | 冻结基线时浏览器标签和图标组合不同；现已恢复同一可见制品契约 | 稳定 favicon 路径由后端映射到内嵌指纹资源，继续保留静态缓存与 surface 隔离 | 新旧 SVG SHA-256 均为 `f1ad9b17bf7080a0ac648ec2efb0a370eba1a66980b9dfdd3d79ec9aa9834add`；favicon 200/immutable，SPA HTML 200/no-store，未知路由 404/no-store |
| PUI-008 | Public 响应式布局 | P3 | 已修复，浏览器 R1 | 旧版 `/` 在 390×844 下刷新按钮和语言选择器相邻，1440×1000 空首页为既定基线 | 移除窄屏语言选择器额外的自动起始外边距，保留原 flex/gap 与桌面布局 | 冻结基线时移动端两控件被分散；现已恢复旧控制组关系 | 只修改 Public surface 窄屏规则，不触及共享 picker 或桌面样式 | 390×844 新旧截图 SHA-256 均为 `40fdadeaa827911f7a0d9cd17f43085987482c7fa46aa69121f8e623c375d957`；此前 1440×1000 也为相同哈希 |
| AUI-001 | Admin 配置页 | P1 | 静态修复，局部认证浏览器 R1 | 旧 `features/config/ConfigPage.tsx` 编辑业务 runtime：`equipment_filter`、`mail_templates`、`worlds` 等 | `AdminRuntimeDocument` 的三个可选业务区块、`settings/worker/runtime.rs`、Admin 配置页 JSON 编辑器及旧版 world/filter 事实卡 | 冻结基线时同一路由管理的是另一份 runtime 文档；现已逐字段恢复 main/isekai、发件人 allowlist、装备过滤和 14 个模板，Worker 启动时从同一文件投影业务快照 | transport endpoint、凭据、调度和全局 MoogleMail `enabled` 仍由 Worker 主配置拥有；旧 runtime 缺少三个区块时保留原 fingerprint/命令摘要并沿用 Worker 主配置，首次保存才显式物化旧版默认值 | 隔离 PostgreSQL 已认证页面实际显示 world `2/2`、filter 已停用，并读到 14 个模板、250 ms、900/28800；仍需保存、Admin/Worker 重启和同一 Worker 有效配置 R1 |
| AUI-002 | Admin 错误反馈 | P2 | 静态修复，局部认证浏览器 R1 | 旧页面普遍通过 `errorKey(error)` 展示稳定业务错误 | Admin API 统一解析 `ApiErrorResponse.code`，`admin_api_error_message` 映射五语言 Fluent，所有写页面保留稳定 code | 冻结基线时多处 `Err(_)` 统一显示“操作失败”；现已恢复 stale version、command conflict、权限、限流和依赖错误的稳定可操作反馈，未知 code 才使用兜底 | 页面反馈保留 code，冲突文案提示刷新权威状态后重试，不暴露服务端原始错误或浏览器网络细节 | 隔离 PostgreSQL 已认证 `config:validate` 400 实际显示“请求参数无效。请检查输入内容后重试。”；仍需 stale/conflict/权限/限流等逐类 R1 |
| AUI-003 | Admin 凭据表 | P2 | 静态修复，局部认证浏览器 R1 | 旧 `CredentialsPage.tsx` 把 `rotated_at` 显示为 last checked、last used 固定为 `—`、imported by 显示当前 Admin session actor；旧 DTO 没有三项独立后端事实 | 新凭据表用现有安全 DTO 和 session actor 恢复同样三列，并额外保留 state version | 冻结基线时新表只突出 rotated/version，旧运营列的呈现消失；现已恢复旧页面实际合成语义，没有虚构或扩展后端凭据事实 | last checked 继续来自 `rotated_at`，last used 明确为空值，imported by 继续是当前会话 actor；Cookie/密文仍不进入 DTO | 隔离 PostgreSQL 临时凭据行已显示列顺序、`—`/`—`、当前 actor 与 version 1，且密码框提交后清空；仍需同数据和窄屏布局 R1 |
| AUI-004 | Admin 可访问性 | P3 | 静态修复，浏览器 R1 | 旧 dialog 具有 `aria-modal`、Tab 循环和逐弹窗触发器引用，行级结果使用 status/描述关系 | 共享 trap 在动态打开时聚焦 `autofocus`/首控件、保存背景触发器并在关闭后恢复；Users/凭据行反馈按成功/失败使用 `status`/`alert` 和唯一 `aria-describedby` | 冻结基线时新版首次焦点停在背景、关闭落到 body，部分行反馈缺少目标关联；现已补齐 | 焦点 owner 保持在无业务偏向的 shared hook，行名称和反馈语义留在各 Admin 页面 | Chromium 18191 Users 创建框已确认标签存在、首次焦点为用户名、Tab/Shift+Tab 双向循环、body 滚动锁；取消后焦点回到“创建用户”且滚动锁清除 |
| AUI-005 | Admin 登录布局 | P3 | 已修复，浏览器 R1 | 旧版 `/login` 在 1440×1000 无纵向滚动，卡片矩形为 `[544,64,352,250]` | 静态入口移除与卡片外边距叠加的 `#main min-block-size:100vh` | 冻结基线时根最小高度加折叠外边距产生 1064px 文档；现已恢复内容高度所有权 | 不给 CSR 根强制视口高度，已认证长页面仍由内容自然撑开 | 18191 在 1440×1000 的卡片矩形与旧版完全一致、文档高 1000；390×844 文档高 844，两者均无无意义滚动 |
| I18N-001 | 语言选择器持久化 | P2 | 已修复，双 surface picker 浏览器 R1 | 旧 `createI18n.ts` 以 `hv.lang` 恢复受支持 locale，并同步页面语言 | 共享 `LanguagePicker` 逐个 option 显式绑定当前 locale 的原生 `selected` 状态 | 修复前刷新后译文、`html lang` 和存储值正确，但下拉错误显示第一项 English | 保留 `select.value` 与 `onchange`，同时显式投影 option 选中态，不改变五语言闭集或存储键 | Admin 已证明刷新日语及五语言切换；Public 补偿池又逐项证明五种语言的 `hv.lang`、`html lang`、picker、标题和空态与旧版一致，十组页面均无异常 |
| I18N-002 | 非法语言码回退 | P3 | 已修复，非法存储双 surface R1 | 旧 `setLanguage` 收到未注册 code 时切换并持久化当前 surface 默认语言；旧初始化读取到不受支持的 `hv.lang` 时按 surface 默认语言运行，但不改写原始存储值 | 共享 `set_language` 已由 picker 显式接收 Public `en`/Admin `zh-CN` 默认值；非法 code 解析失败时改用默认值并写入 `hv.lang`。初始化路径则维持旧版只做有效语言回退、不主动改写存储的行为 | 修复前非法 setter 输入直接 no-op；现已恢复旧 setter 边界，同时避免把浏览器初始化的旧行为误判成必须自愈存储 | 继续只注册 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`；非法输入不得扩张有效语言集合。初始化可保留原始非法存储值，但运行语言、picker 与 `<html lang>` 必须回退；用户下一次合法选择会覆盖它 | 在新旧 Public/Admin 四页分别向 `hv.lang` 注入 `fr-FR` 后 reload：两版均保留原始存储值，Public 的 `<html lang>`/picker 回退 `en`，Admin 回退 `zh-CN`，标题文案同步；无第六种语言进入有效集合。共享 setter 的非法输入分支及三目标静态门禁已通过，不新增测试脚本 |
| I18N-003 | Fluent 动态变量文本兼容 | P1 | 已修复，双 surface 五语言局部 R1 | 旧 i18next 的动态插值直接生成用户可见文本，不在变量外插入 Unicode FSI/PDI | `dioxus-i18n` 0.5.1 默认 Fluent bundle 会为每个动态变量包裹 U+2068/U+2069，且当前配置 API 无法关闭；Public/Admin 现统一通过 `auction-web-shared::legacy_t!`，只剥离外层成对隔离符并保留嵌套内容 | 修复前分页、金额、拍卖号、封禁原因等 DOM 文本含不可见字符，复制、辅助技术输出和严格文本对照均与旧版不同；组件逐点修补还会遗漏后续消息 | 两个 surface 的用户可见动态译文统一走共享兼容边界；不修改 FTL 资源、不删除变量实际内容，也不把任意 Unicode 控制字符作全局清洗 | 最终 embedded Public populated 根页在 `en/zh-CN/zh-TW/ja/ko` 下均正确初始化 `html lang`/picker/H1，Admin 登录页五语言 H1 与 `html lang` 也对应；两侧页面正文 FSI/PDI 计数均为 0，黑名单动态原因和分页文本另取得精确 R1。其余已认证 Admin 动态页面仍按 EVD-003 逐页核对 |
| CFG-001 | Metrics 暴露 | P1 | 已修复，双 surface loopback scrape R1 | `config/validate.go:414-425` 要求启用 metrics 时 listener 为 loopback TCP 或 Unix | `auction-config` 的 metrics/listener 联合校验同时覆盖主配置和 Admin runtime 覆盖后的有效 listener | 冻结基线时可在非本地 TCP listener 上启用 `/metrics`；现已恢复 loopback TCP 或 Unix 的失败关闭边界 | 非本地 TCP + metrics 在绑定前拒绝，错误只报告字段类别；合法 loopback/Unix 行为不变 | 临时脱敏配置已证实 `0.0.0.0` + metrics 在绑定前退出 2；Public/Admin 分别以 `127.0.0.1:8100/8101` 启用后，`GET /metrics` 均为 200、`text/plain; version=0.0.4; charset=utf-8`，并含唯一 `auction_process_up 1`、`auction_metrics_enabled 1` 与 process identity；带 query 均 404，常驻关闭实例 `8090/8091` 均 404。临时进程及配置已清理 |
| CFG-002 | Origin 防护默认值 | P1 | 已修复，Public/Admin 缺省 HTTP R1 | 旧配置默认启用 origin allowlist | `HttpSecurityConfig::default` 已恢复启用 allowlist，并从有效 TCP listener 推导同源 origin | 冻结基线时省略配置会关闭来源校验；现已恢复缺省同源闭集，显式关闭仍是可审计的部署选择 | Public/Admin 主配置与 Admin runtime 使用同一默认和规范化 origin 校验，不接受凭据、路径、查询或片段 | 在临时隔离的新版 `8100/8101` 省略整个 `http_security`，由 loopback TCP listener 推导同源；旧版 `8102/8103` 启用对应精确同源策略。Public JSON 登录与 Admin Basic 登录均证明：无 `Origin` 和精确同源请求进入 handler 并返回预期 401，跨源请求在凭据校验前返回 403 `origin_forbidden`；四个临时进程及两份无敏感配置已清理，常驻 `8090..8093` 未受影响 |
| CFG-003 | Admin 会话默认值 | P2 | 已修复，配置与滑动 touch R1；绝对过期待证 | 旧 Admin 默认绝对 8 小时、滑动 15 分钟；Public 7 天/30 分钟。已认证读取会把 idle 截止向后滑动，但不改变 absolute 截止 | Admin runtime/default projection 已恢复 `absolute_ttl_seconds=28800`、`idle_ttl_seconds=900`；Public 保持 604800/1800。Admin session touch 使用同一 idle/absolute 双截止模型 | 冻结基线时 Admin 复用 Public 默认导致会话寿命显著变长；现已恢复 surface 独立默认和已认证滑动续期语义 | 旧 runtime 缺字段时由 Admin 默认投影生效，新保存文档显式保留该值；absolute 截止不得被 touch 延长 | `auctionctl check-config admin` 已输出 900/28800，Worker/Public 安全投影保持 1800/604800。新旧已认证 Admin 连续两次读取间隔约 1.3 秒：两侧 idle 截止都同步前进约 1.3 秒且距响应时间约 900 秒，absolute 截止逐字不变；仍需时间受控的真实 8 小时绝对过期 R1 |
| CFG-004 | E-Hentai 请求节流 | P2 | 静态修复，局部配置 R1 | 旧 equipment 请求最小间隔默认 250 ms | Admin runtime 装备来源默认和示例已恢复 `minimum_request_interval_millis=250` | 冻结基线时默认 1 秒；现已恢复旧默认，论坛/MoogleMail 继续保留各自独立限速 | 省略字段时由 runtime 默认投影 250 ms，显式合法值仍可覆盖 | `auctionctl check-config admin` 已脱敏输出有效 250 ms；仍需真实装备来源节流 R1 |
| DB-001 | SQLite 历史投影 | P2 | 已修复，Lite HTTP/WebUI R1 | 旧版仅 PostgreSQL，成交历史由 settlement 事实关联生成 | 已删除只看 `settled`/`sold` 状态的 SQLite 查询；storage 固定返回 `CapabilityUnavailable`，HTTP 映射 `501 feature_not_supported`，WebUI 显示五语言永久能力缺失且不提供重试 | Lite 生命周期没有 `settled`/`sold` 完成事务，也没有 settlement 表；保留原查询会让手工状态行看似成为权威成交事实 | 成交历史只由 PostgreSQL 的 `auction_settlements` + `lot_settlements` owner 提供；SQLite 保留 URL/JSON 错误契约但不伪装数据能力 | 隔离 Lite 18290 的 P12 返回 501、稳定 code、`data:null`；五语言页面的 `hv.lang`/`html lang`/标题/说明均正确，筛选与 Retry 均为 0 |
| OPS-001 | canonical v28 接管 | P1 | 已修复，隔离 canonical R1 | 旧对照实例的 canonical PostgreSQL 18.6/v28 数据库、59 张 public 表和目录规则 | `legacy_v28` 锁内预检/bridge、普通前向迁移、规则导入、backup/rebuild/restore 及三类 receipt | 首轮真实执行发现并修复 PG18 关键字别名、连接池自会话、普通 owner 目录权限和 `pg_default` 重建权限四类阻塞；最终链路完整通过 | 接管在同一锁定物理连接复核，恢复盘点不要求 superuser，默认表空间由隐式默认语义重建；保留失败关闭和不可覆盖证据 | v28 专用 dump/manifest、adoption receipt、SQLx 1..16、规则五语言、当前备份、OID 变化、空库证明、restore receipt 及恢复前后摘要均已核对；旧副本核心拍卖事实为空，非空业务行抽样仍不在本项证明范围 |
| OPS-002 | 发布制品 | P2 | 已修复，制品/运行 R1 | 旧 Makefile 有 API-only/嵌入前端 archive、资源隔离和确定性 archive 校验 | `web-embed` 显式 feature、`ci/web-bundle.sh`、`ci/release.sh` 与 `Justfile` 的 build/verify/package 入口 | 冻结基线时构建成功不证明部署拿到正确静态制品；现已把 API-only、embedded、Web payload、依赖边界和 archive 内容提升为显式可审计合同 | 两种原生制品使用独立 target；Web 每次从清空的 release 中间目录优化构建；archive 使用成员 allowlist、固定 mode/owner/mtime、双次构造同哈希和内外两层校验和 | API-only 两个 surface 根路径均为明确 503；embedded 两端 own asset 200/cross asset 404；Worker/auctionctl 无 Web payload；dirty 默认拒绝且显式证据包记录源码指纹 |
| OPS-003 | systemd/Caddy 切换 | P2 | 隔离 systemd/Caddy R1，生产切换未批准 | 旧 `start.sh` 的严格准备、前向迁移、Worker → Admin → Public 启动、Public → Admin → Worker 停止、新启动角色逆序回滚和 Caddy metrics 屏蔽 | `auctionctl deployment`、四个独立 systemd unit、Caddy 配置、启动回执及三进程 drain | 冻结基线没有实际 system manager 证据；现已在同宿主机独立高端口、临时 PostgreSQL 和隔离目录证明严格预检、迁移、顺序、readiness、restart、回执、反代、metrics 屏蔽、SIGTERM drain 和启动失败回滚 | 真实部署仍须使用专用服务账号、真实域名/TLS 和正式 secret 注入，并经用户批准流量切换 | systemd 257/Caddy 2.11.4 下配置/单元校验通过；三角色顺序及回执匹配，Public 故障返回退出码 4 并逆序回滚，临时资产已完整清理 |
| EVD-001 | 外部服务 | P1 | E-Hentai 浏览器局部 R1；Worker/MoogleMail 待证 | 真实 main/isekai MoogleMail、E-Hentai 和通知行为 | 新 gateway/parser/Worker 调用链；E-Hentai 测试板块允许受控浏览器验证 | 已取得一次测试板块表单、预览、POST 回执和真实主题回读；本地非浏览器请求被 Cloudflare 403 拦截，尚无 Worker API 发布、持久状态回写、真实分页/限流、凭据过期、POST 结果未知及 MoogleMail 通知样本 | 浏览器证据只证明论坛表单与 BBCode 合同；下一步通过 SSH 控制白名单 IP 机器，以同一 API 客户端和隔离账号验证 Worker 读写，不得绕过未知写入栅栏或把浏览器 Cookie/用户名写入证据 | 2026-09-01 MCP 发布的测试主题为 `https://forums.e-hentai.org/index.php?showtopic=300433`；白名单机仍须证明精确标题只读对账、单次 POST、回执解析、数据库 `published`、凭据失效与未知写入恢复 |
| DATA-001 | Public 规则运行数据 | P1 | 已修复，受控导入/API R1 | 旧 Public 优先读取配置目录的五个 Markdown；受控只读查询证明旧 `rules_documents` 为 0 行 | `auctionctl import-rules-directory` 把五文件经现有 Admin rules CAS/幂等/审计端口导入 PostgreSQL，并失效同语言缓存 | canonical v28 数据库接管不会携带目录权威源；冻结基线的 503 已定位为真实迁移缺口，而非 Public reader 或 locale 配置错误 | 要求五文件齐全并先全量校验；任何现有数据库正文不同都在首次写入前阻断，相同正文保留，缺失正文使用稳定子命令 ID 发布；不硬编码 seed | 隔离 PostgreSQL 首次五项 `imported`、同批次重放五项 `preserved`、均 revision 1 且缓存失败为空；18190 五语言 API 的正文 bytes/SHA-256 与旧 8092 完全一致，并在未重启 Public 的情况下可读 |
| EVD-002 | WebUI 视觉 | P2 | Public 桌面匿名/空态 R1，仍有证据缺口 | 旧 Public/Admin 实际浏览器页面 | 新 Dioxus 制品 | Public 14 条路径已完成 `en` 桌面巡检，首页、登录、历史/竞价日志、价格历史、账户、黑名单、补偿池、状态页、规则、别名和非法 lot 的当前匿名/空态已对照；规则另有五语言逐篇像素 R1，补偿池有五语言语义 R1。动态列表行、认证表单动作、错误/冲突、移动端逐页状态及 Admin 其余页面仍未覆盖 | 延续人工对照清单，不新增浏览器测试用例；按 14/21 路由逐页记录 | desktop/mobile、五语言、空态/错误/冲突/长文本、焦点与响应式均有对照记录 |
| BID-004 | Final 竞价 | P0 | 静态修复，待同库 R1 | 旧 `auction/model.go` 明确 Final 仍可竞价；`postgres/adapter/bid_state.go::lockBidState` 接受 `live`/`final` | PostgreSQL/SQLite 竞价快照均已接受 `live`/`final`，结算继续使用独立 `settling` 快照 | 冻结基线会在 Final 阶段拒绝全部竞价；现已恢复旧状态闭集且未把 `settling` 放入公开竞价范围 | 竞价快照接受 `live`/`final`，结算快照只接受 `settling`，两条路径不得互相放宽 | storage 双后端/all-features 静态检查已通过；仍需同库核对 Final 普通/代理成功、截止后失败及原子事实 |
| BID-005 | world/open gate | P0 | 静态修复，待同库 R1；缺失结束时间另见 BID-014 | 旧 `lockBidState` 同时要求 world enabled、`scheduled_open_at <= now`、`effective_end > now`；命令时间来自 handler 的 `command.Now`，只有慢速代理执行另用数据库当前时间 | 两后端竞价快照已在同一事务使用应用 `SystemClock` 冻结的命令时间读取 world enabled、scheduled open 与 effective end | 冻结基线可能在 disabled world 或开场前接受竞价；现已把三项 gate 收回锁内并失败关闭。旧 schema 还禁止 live/sold lot 缺失 `effective_end_at`，新版 schema 的可达状态差异不能混入普通截止 gate | 非法 gate 不得留下命令结果、bid 事实、审计或 realtime 事件；SQLite 继续依赖串行写事务；缺失结束时间的 schema 与强制损坏分支由 BID-014 单独收口 | storage 双后端/all-features 静态检查已通过；仍需覆盖开场前、边界、截止后和 world 切换的同库 R1 |
| BID-006 | PostgreSQL 锁顺序 | P0 | 静态修复，待并发 R1 | 旧生成查询 `LockLotForBid` 用 MATERIALIZED CTE 按 world → auction → lot 锁定 | PostgreSQL snapshot 已用 MATERIALIZED CTE 固定 target lot → world → auction → lot 的读取/锁定依赖，SQLite 保持 `BEGIN IMMEDIATE` | 冻结基线只锁 lot，生命周期/admin 并发时可能读取失效 world/phase；现已恢复稳定跨 owner fence | 所有竞价入口复用同一锁序；不得在其它路径引入 auction → world 的逆序锁 | storage PostgreSQL/all-features 静态检查已通过；仍需并发切换 world/phase 与竞价的无死锁串行结果 R1 |
| BID-013 | Slow 代理终局窗口状态 gate | P0 | 已实施，静态门禁通过，待 R1 | 旧 `SetProxy` 只在拍卖状态已经是 `final` 时拒绝新建 slow 代理；处于 final lead 时间窗口但生命周期状态仍为 `live` 时仍按 live 规则受理 | 竞价快照现显式携带锁内读取的 auction status；slow 新建只在状态已经为 `final` 时拒绝，不再从 `effective_end_at - final_phase_lead` 推导状态。截止、world、open、资格及 fast 代理规则保持原边界 | 生命周期 Worker 延迟或竞争时，新版不再扩大禁止窗口并误拒绝旧版仍可接受的 live 命令；真正 final 仍失败关闭 | 两后端 snapshot、应用预校验与领域 trigger 使用同一显式 status；保留锁、命令账本、截止 gate 和原子事实不变量 | Luna 与主线程逐层确认双侧调用链；Domain/Application/Storage 双 feature/HTTP/Public WASM check 与 Clippy 均通过。仍需同一 live-in-final-window、真实 final 与边界竞争 R1 |
| BID-014 | live/sold lot 缺失结束时间边界 | P1 | 已修复，双后端隔离迁移 R1 | 旧 schema 的 CHECK 禁止 `status in ('live','sold')` 且 `effective_end_at IS NULL`；若绕过约束强制构造，`pgtype.Timestamptz.Valid=false` 会先在 `lockBidState` 变为 409 `auction_not_open`。真正 row scan/decode 或后续 policy/competition 损坏才走 503 | PostgreSQL `000018` 以锁内预检加命名 CHECK 恢复旧 shape；SQLite `000008` 以 insert/update trigger 和 no-op 预检恢复同义 Lite 不变量。两后端 snapshot 对强制残留 NULL 单独映射 `InvalidState`/409，row decode、损坏 policy 与其它内部 snapshot 仍安全 503 | 正常新版数据库不再进入旧 schema 不可达状态；接管前非法行会阻断迁移而不是猜测截止时间。兼容 409 分支只处理绕过 schema 的强制损坏，不掩盖真实依赖错误 | 已发布迁移未修改，只追加两个后端各自实现；约束允许 pending/unsold/withdrawn 保持 NULL 或既有非空值，不扩大状态闭集。迁移失败需先按备份/权威事实人工修复 | Luna 已追踪旧 SQL/sqlc nullable scan、`lockBidState`、HTTP mapper及双侧 schema；格式、仓库策略、PG/SQLite 独立 feature check/Clippy 均通过。临时 PostgreSQL 18 与 SQLite fresh schema 均完成合法升级，live+非空结束时间写入成功，live+NULL 写入被约束/触发器拒绝；在 `000018`/`000008` 前构造的 live+NULL 旧行均使新迁移失败关闭，临时库/文件已清理 |
| BID-007 | 失格代理前置冻结后继续竞价 | P0 | 已修复，隔离 Final R1 | 旧 `repository.go::PlaceBid` 在同一竞价事务锁定参与者资格；若残留 active proxy 的 owner 已失格，先用当前外部 `command_id` 计划 `eligibility_lost` 冻结，再基于冻结后的 lot 接受挑战者普通竞价，只写一份命令结果和一条 `lot.bid_accepted` 审计 | 新应用层同样先生成 prerequisite freeze，但修复前冻结后保留了描述“上一笔接受事实来自代理”的 `leader_proxy_last=true`；随后领域校验把该历史标记误当成“当前必须存在 active authorization”，以 `auction_not_open` 拒绝主竞价并回滚整笔事务 | 隔离 Final 场景中，双方先建立相同 fast 代理，再受控把 owner 置为 blacklisted 而故意保留 active proxy：修复前旧版挑战者 60000 普通竞价返回 200、代理冻结且 lot 前进，新版返回 409 且整笔回滚；这会阻断合法竞价，并使应当原子发生的资格冻结无法落地 | `leader_proxy_last` 继续作为最后接受事实的机制标记，不因授权终止而清除；“无 active authorization”只在仍有 slow 调度引用时视为损坏状态。命令账本、前置转换、普通事实、审计和实时事件继续在同一事务提交 | 修复后复用同一 fixture 与同一命令，两侧均返回 200、current price 60000、state version 4、挑战者领先；数据库各恰有一份命令结果、一条 `active→frozen/eligibility_lost` 转换、一条 ordinary bid、一条 `lot.bid_accepted` 审计和一条 realtime 事件。同 ID 重放仍为 200，五类事实计数均保持 1；格式、Domain/Application/Storage all-features check 与 Clippy 均通过，全程未编写或运行测试用例 |
| SET-001 | 正常结算读取 | P0 | 已修复，非空结算局部 R1 | 旧 `auction/postgres/adapter/settlement.go` 在锁住 `settling` auction 后使用结算专属 lot/competition 读取 | `settlement/transaction.rs` 已改用只接受 `settling` auction 与 live lot 的专属 competition snapshot | 冻结基线的通用 live 快照会阻断正常结算；现已拆开公开竞价与结算读取 owner | 公开竞价不得读 `settling`，结算不得借用 `live/final` 竞价谓词 | 隔离 PostgreSQL 18 中一笔到期 Isekai final/单 live lot 已由最新 Worker 原子推进为 settled/sold，并生成唯一摘要、lot 事实、四类义务、账本、审计和事件；二次启动计数不增。仍需公开竞价拒绝、并发与更多形状 R1 |
| SET-002 | 补偿池与 ledger | P0 | 已修复，零税资金局部 R1 | 旧 `settlement.go:391-406,820-872` 仅在 pool enabled 时更新 pool/version 和写 tax/comp ledger；closing 记录 amount 为 0 且保留 closing balance；比例补偿还逐 lot 执行 `min(proportional, discount)` 封顶 | 领域计划、pool CAS/version 与 ledger 写入已统一按 enabled 分支；启用时按 tax 后 compensation 顺序写入，closing amount 为 0 且保留 closing balance；补偿分配现恢复单 lot 折扣封顶；模式与开关合法组合由 SET-014 失败关闭 | 冻结基线除禁用 pool/closing 形状不同外，还会在可用资金大于总需求时让单 lot 补偿超过折扣，造成买方金额下溢或计划不守恒；三处均已按旧算法修复 | `legacy_zero_v0` 禁用时 pool、version、tax/comp ledger 均不变；`hold_curve_v1` 启用时每条补偿不超过该 lot 折扣，金额、顺序和 closing balance 必须守恒 | 隔离 R1 已证明 pool enabled、期初/期末均为 0、版本 1→2、opening/closing 各一条且 closing amount=0；探针同时发现并修复补偿账本误读 `auction_commands.auction_id`，现改从权威 `aggregate_id` 取拍卖。补偿封顶修复已通过 PostgreSQL feature 静态编译；legacy disabled、非零税/补偿及资金大于需求场景仍待 R1 |
| SET-003 | 结算实时事件 | P1 | 静态修复，待抓帧 R1 | 旧 `settlement.go:474-490` 发 `auction.settled` schema v2，payload 含 policy_version 与 obligation_count | 新结算 owner 已恢复 schema v2，并同时发送 policy_version 与 obligation_count | 冻结基线以同一事件名发送不兼容 v1；现已恢复冻结 wire，不把内部字段变化伪装成旧版本 | envelope、schema version、payload 和 sequence 继续由 realtime owner 原子发布 | contracts/application/storage 静态检查已通过；仍需同一结算事件逐字段抓帧和重放序号 R1 |
| SET-004 | 结算稳定 ID | P1 | 静态修复，待接管样本 R1 | 旧 `settlement_policy.go`、`recovery.go` 用固定 namespace、长度前缀自然键和 SHA-256 派生 obligation/recovery UUID | domain 已实现旧长度前缀 SHA-256 UUID 派生，obligation/recovery 分别使用冻结 namespace 与 auction/lot/kind 自然键 | 冻结基线以 command ID/字符串 purpose 派生，接管后会改变幂等引用；现已恢复旧自然键，command_id 只保留命令账本职责 | 相同自然键跨命令必须得到同一 UUID；不同摘要的同 command_id 继续冲突 | domain/application/storage 静态检查已通过；仍需 canonical 自然键样本和接管后引用 R1 |
| SET-005 | 结算重放校验 | P1 | 静态修复，待损坏事实 R1 | 旧 `ValidStoredSettlementResult` 校验拍卖、统计、kind 计数和结果完整性，计划拒绝重复 lot 事实 | 重放验证除 exact cardinality、重复 lot、总计/end price 与 obligations 全量校验外，还会用冻结策略、首次 opening pool 和锁内 lot 事实重算当前算法计划，并要求摘要与 payload 全量相等 | 冻结基线可能接受内部守恒但并非由权威事实和当前算法产生的持久结果；现已对结构、统计、算法或摘要任一漂移失败关闭 | 校验只读取持久结果并纯计算期望计划，不修补历史、不产生新 obligation/recovery 或外部副作用 | Domain/Application/Storage 定向静态检查已通过；仍需重复、缺失、计数不符、算法漂移和合法重放的同库 R1 |
| SET-013 | 结算提交错误有限重试 | P1 | 静态修复，待故障注入 R1 | 旧关键事务只把序列化冲突/死锁视为可安全重试，其他提交失败保留原错误边界 | `SettlementTransactionRunner` 现先映射 commit 错误，只对 `SerializationConflict` 且未耗尽次数时重试；其它错误立即返回 | 冻结实现会对连接、约束或未知提交错误重新执行整笔结算，扩大结果未知窗口并掩盖真实分类 | 仅 `40001/40P01` 进入有界完整事务重试；其它 commit 错误不得重放或统一折叠 | Storage 定向 check 已通过；仍需分别注入序列化、死锁、连接与约束错误核对次数和零重复事实 |
| SET-014 | 结算模式与补偿池配置闭集 | P0 | 静态修复，待配置/资金 R1 | 旧结算把 pool enabled 同时作为税、补偿、pool version 与 ledger 的总开关，不存在“计算税但不记账”的第三种资金语义 | 领域策略现要求 `hold_curve_v1 + compensation_pool_enabled=true` 或 `legacy_zero_v0 + false`；三个运行示例和配置文档同步说明闭集 | 冻结配置允许 HoldCurveV1 关闭 pool，计划仍可能计算税额而存储分支不写 pool/ledger，破坏资金守恒和可审计性 | 非法模式/开关组合在启动策略校验时失败关闭，不由结算写路径猜测或静默纠正 | Domain/Config/Storage 定向 check 与 config example 门禁已通过；仍需两种合法组合和两种非法组合的配置 R1，以及非零税资金核对 |
| SET-006 | 结算领取、due 世代与事实时间 | P0 | 已修复，延迟事实时间局部 R1 | 旧 `ClaimSettlement` 使用 Serializable、最多 8 次冲突重试；workflow/claim/complete 稳定 ID 均包含 `dueAt`，Worker 把任务 `DueAt` 作为结算事实时间；旧 due 扫描还会重新发现 `settling + settlement_next_attempt_at IS NULL` 的 claim 后崩溃行 | claim 已恢复 Serializable 与旧重试上限；稳定 ID 和全部结算业务事实使用冻结 `dueAt`。候选 SELECT 与租约 UPDATE 现同时允许旧 claim 的 `settlement_next_attempt_at IS NULL`，领取后再写新 lease | 冻结实现除隔离和事实时间漂移外，还要求 settling 的 next attempt 非空，使旧版 claim 后崩溃数据永久沉默，已有兼容账本校验实际不可达；当前已恢复旧扫描闭集 | 领取保持 PostgreSQL Serializable 与有界重试；稳定 ID 和业务事实时间按旧 due 世代统一；数据库当前时间只用于 due/租约/失败观测。旧 NULL claim 必须先通过稳定 workflow/claim 账本校验再续租 | 隔离 R1 在实际执行时间晚于 due 约三小时的 fixture 上证明：claim/complete 命令、started/due/settled/updated、lot、摘要、补偿账本及 claim/final 审计均精确使用同一 `due_at`，二次启动计数不增；NULL claim 扫描修复已通过 PostgreSQL feature 静态编译。旧 claim 中断恢复、并发 claim、`40001/40P01`、同/异 due 仍待 R1 |
| SET-007 | 履约资格、托管授权、依赖激活、return 与失败收口 | P0 | 静态修复，待授权/终态 R1 | 旧 pending/prepared 每次推进前都重读用户身份并要求当前 `active`，BuyerRefund 还读取与 equipment world/external ID 精确绑定且位于 claim 时间窗内的 custody 证明，再校验派发世界余额；enqueued 继续重读身份但不再要求 active。terminal 事务会原子满足当前义务、激活 blocked dependents 并写旧审计，`return` 及 pending/failed/manual/succeeded 各有独立收口 | Application 已建立 recipient/custody/balance 窄端口：pending 与 prepared 每次推进都重读当前身份和资格、BuyerRefund custody 及余额，enqueued 只复核身份；claim 现分别携带 dispatch world 与 equipment world，避免异世界退款误用 main 核验装备。Storage 已恢复缺失 execution 惰性创建、obligation 顺序、坏上下文逐项隔离，以及 terminal 精确 state/version/token/fencing/租约、Serializable 三次冲突/死锁重试、依赖激活和旧审计 | 修复前领取后失格/改名仍可能激活，BuyerRefund 无 custody 或错用 dispatch world，余额只做一次授权；缺失 execution 与单条坏装备事实还会阻塞同批任务。现有静态链已消除这些路径，但真实外部读取、两次授权窗口与提交后副作用仍未运行证明 | pending/prepared 的 unavailable、mismatch、insufficient、stale 分别进入旧稳定等待/人工态；enqueued 不因发送后失格否定既有外部事实；terminal owner 在同一 Serializable 事务提交义务、依赖、proof 和审计，坏上下文不能污染其它 obligation | Application/Storage/Jobs/Integrations 与 PostgreSQL Worker 目标静态编译已通过；隔离 R1 仍须覆盖领取后 blacklisted、身份变化、custody 缺失/错配/过期/未来、余额不可用/过期/不足、两次授权窗口、enqueued 后失格、send/return 四终态及零重复副作用 |
| SET-008 | MoogleMail fulfillment wire、稳定身份与主题闭集 | P0 | 静态修复，待真实 MoogleMail R1 | 旧 `BuildPayload` 使用严格 schema-v1 send/return JSON、Go `encoding/json` canonical 字节与 SHA-256；delivery/prepare/activate 使用三个固定 namespace 和长度前缀自然键；activation digest 绑定 delivery/obligation/prepare digest/status；所有 send 模板由 `FulfillmentSubject` 生成带 `[reference]` 的主题，return Subject 解析要求固定小写前缀与 canonical 小写非零 UUID | Application 已恢复 schema-v1 字段、排序后的 variables、空 equipment refs、Go HTML-safe JSON 字节、原始 payload digest、三个旧 namespace 稳定 UUID 和旧 activation digest；Integrations 严格解码同一 wire，发送与 Sent evidence 共用 canonical Subject，return parser 恢复大小写敏感前缀和 canonical UUID。装备 secret ref 只在发送前按 owner UUID 解析，payload 不含 key | 修复前 v2 富 payload、另一套 UUID v5 标签、RequestHash activation 外壳与宽松 Subject 解析都会改变接管后的 delivery 身份、幂等摘要和外部邮件 wire；装备 ref 若未在发送边界解析还会得到错误附件或错误失败类别 | wire、摘要和稳定 ID 必须直接复现旧字节算法；`secret_not_found` 是确定失败，reader/解密不可用进入 outcome unknown；return 复用原邮件且不生成新主题，send 与 Sent evidence 不得各自拼接 | Application/Integrations/Jobs/Worker 静态编译与目标格式检查已通过；仍需真实 main/isekai 的 compose/return、四类 Subject、附件、Credits/COD、Sent evidence、缺 secret 与未知结果 R1，外部证据前不宣称完整闭合 |
| SET-009 | 结算命令结果与重放合同 | P1 | 只读核验入口已实现；安全失败局部 R1，真实旧样本待证 | 旧 kind 为 `settlement_claim/settlement_complete`、result 为 `OK`；claim payload 是含 workflow/command/due/started/attempt/version 的 `SettlementClaimResult`，complete payload 是含 policy/pool/金额/sold/unsold/obligation kind/state/settled_at 的完整 `SettlementResult`；`CompleteSettlement` 在锁 auction 前先按 command ID/hash 读取账本，合法已完成结果直接重放 | Application 增加 `LegacySettlementVerificationPort` 和纯只读严格重放校验；PostgreSQL 使用首语句为 `SET TRANSACTION ISOLATION LEVEL SERIALIZABLE READ ONLY DEFERRABLE` 的同一快照，读取 settled workflow/claim/complete/due 身份、无锁 complete ledger 与权威结算事实。`auctionctl verify-legacy-settlement --auction-id` 是专用生产入口，只输出安全计数/身份摘要；SQLite Lite 明确报告能力不可用。正常 Worker 写路径、complete fence 和既有结算状态机均未改写 | 原内部兼容函数不可达的静态缺口已经关闭；缺 workflow/账本、新版账本、摘要/身份漂移或事实不一致均失败关闭，不会清空 fence、重新结算或追加事实。当前隔离库没有真实旧 complete 样本，因此只能证明入口、只读事务和失败边界，不能升级为完整接管 R1 | 保持 dedicated read-only verify，不把旧 complete 重放塞回 due Worker 候选；严格复用旧 kind/result/摘要与权威事实重建，任何不一致都拒绝。真实旧样本验证前后必须核对命令、结算、lot、义务、审计和 realtime 零增量 | Application/Storage/auctionctl 的 PostgreSQL/SQLite 定向 check、all-features Clippy、rustfmt 与策略门禁均通过。迁移后对 settled `33333333-3333-4333-8333-333333333333` 执行入口，因缺旧 workflow 以 operation exit 5 失败；`auction_commands/auction_settlements/lot_settlements/settlement_obligations/audit_records/realtime_events` 前后计数同为 `54/0/0/1/71/45`。未有真实旧 complete 样本前仍不得标为完整接管 R1 |
| SET-010 | Admin 结算报告事实守恒与严格 wire | P1 | 已修复，待合法/损坏报告 R1 | 旧 `RunSerializable(...,3)` 在同一快照读取报告；`SettlementReport.Validate` 校验 sold/unsold、`buyer+compensation=seller+tax`、tax/comp 互斥、汇总/明细/movement count、discount demand、生命周期及 adjustment metadata；lot adjustment metadata 只允许 `policy` | reader 已取消金额 `COALESCE` 伪值，要求每个 lot 存在 settlement 事实，并恢复 Serializable 最多三次明确 SQLSTATE 重试；HTTP mapper 已补完整守恒和严格 wire。复核又发现 writer 写入额外 `policy_version` 而 reader 会拒绝，现已把 lot metadata 恢复为仅含 `policy`，版本继续由结算摘要拥有 | 冻结实现既可能隐藏损坏事实，也会因 writer/reader 自身闭集矛盾让正常新版报告返回 503；两个额外 auction 时间键也会被旧 Web 拒绝。当前三类静态缺陷均已修复 | writer 与 reader 共用旧 metadata 闭集；报告 reader/mapper 对完整 cardinality、sold/unsold、逐 lot/汇总守恒和旧 auction JSON 键闭集失败关闭，不现场修补历史 | Contracts/Storage/HTTP 既有静态检查通过，本轮 metadata 修复另通过 rustfmt 与 PostgreSQL storage feature 编译；仍需合法报告 200 与旧 Web 解码，以及缺行、残留 summary、重复 lot、winner/price 非法、金额不守恒、movement/count 不符、未知 metadata 和序列化冲突的隔离 R1 |
| AUD-003 | 结算与履约审计事实 | P0 | 结算已修复，claim/retry/final 局部 R1 | 旧 claim/retry/final action 为 `auction.settlement_claim`、`auction.settlement_retry_scheduled`、`auction.settle`，使用 `system/auction-worker/worker` 与 NULL request_id；retry metadata 含 workflow/error/attempt/next/version，final 含 policy、pool、金额、sold/unsold、obligation 统计；fulfillment terminal 另有 obligation owner 审计 | claim 已在 final→settling 的 Serializable 事务中写旧 action/identity/NULL request 及 auction/version metadata；失败 owner 以数据库观测时间立即递增 attempt、按 5s/15s/30s/1m/2m/5m 固定退避，并在同一 Serializable 最多三次事务写旧 retry metadata；Worker 单项超时用独立两秒窗口尽力调用该收口。final 审计与结算事实同事务写 `auction.settle`、旧身份、NULL request 及完整 policy/pool/金额/count metadata；fulfillment terminal 保持已对齐实现 | 修复前 action/identity/metadata 不兼容、retry 无审计且 attempt 时机不同；现已按 owner 恢复，且没有把 delivery 审计或 fulfillment terminal 重复算作新的结算审计 | claim/retry/final 各由其状态写事务唯一追加；审计失败整体回滚。timeout 若面对已提交 settled 状态由 fence 阻止误写；send/return 仍通过 obligation execution 关联，delivery owner 的隔离重试不在本项重复实现 | 第一轮隔离探针因真实 SQL 缺陷触发 `persistence`，同事务得到 attempt=2、5 秒 next 与唯一 retry 审计；修复后全新库成功得到唯一 claim/final 审计，均为旧 action/identity、NULL request 和闭集 metadata，二次启动不增。timeout/invalid、后续退避、`40001/40P01` 及 fulfillment/delivery 仍待 R1 |
| WRK-004 | 每周场次调度 | P1 | 静态修复，待调度/邮件 R1 | 旧 `auction/schedule.go` 每轮按数据库时间确保当前周场次，使用 world/local-week 稳定 ID 推进 preview/open，并在 open 前两小时暂停、open 后恢复 MoogleMail | `worker/runtime/schedule.rs`、`scheduler/bootstrap.rs` 与 `scheduler/claim.rs` 已接入数据库时间 current-week ensure、missed 轮次、旧稳定 round/auction/create/preview/open/pause/resume ID、mail CAS gate，并在 lifecycle 后同批再次对账；本地墙上时间还按 Go `time.Date` 语义处理 DST 重复/跳过区间 | 冻结基线时空库或错过首轮不会自动 bootstrap，且没有 mail gate；实现复核又发现 PostgreSQL 创建遗漏 NOT NULL 的 preview 时间、普通事务无序列化重试及 preview/open 命令 ID 不兼容，现均已修复 | PostgreSQL bootstrap 使用 Serializable 最多三次、world 行锁和同事务拍卖/intake/命令/轮次/审计；SQLite Lite 使用 `BEGIN IMMEDIATE`；mail gate 只通过 Admin Mail owner 的状态版本与幂等命令变更，不改 cursor | PostgreSQL/SQLite storage、Jobs/Worker 静态编译已通过；仍需空库、跨周、DST 边界、重启、错过窗口、并发 ensure 和真实邮箱暂停/恢复 R1 |
| WRK-005 | 邮件失败轮询时间 | P2 | 静态修复，待 R1 | 旧 `mail/worker/loop.go:491-510` 在失败持久化时同步写 `LastPollAt = now` | 新 `mail_poll/poll.rs::fail` 已在同一条件更新中用数据库 `CURRENT_TIMESTAMP` 单调更新 `last_poll_at`，未写 `cursor_value` | 冻结基线时 dashboard/运维看到的最后轮询时间会停留在上次成功；现已补齐失败可见时间，退避和 cursor 边界不变 | 失败 finalize 同样以数据库时间更新 `last_poll_at`，但不得提前推进邮件 cursor | 目标 crate 静态检查通过；仍需连续失败 R1 核对数据库时间单调前进、退避和原 cursor |
| WRK-006 | 告警轮询 jitter | P2 | 静态修复，待 R1 | 旧 `operationsalert/worker/loop.go::Run` 使用 `RandomJitter(PollInterval, Jitter)`；Config 显式校验 jitter | 新 `OperationsAlertWorkerConfig` 已补 `jitter_seconds` 严格字段、默认 5 和不大于 poll 的校验；三个 loop 通过 `poll_delay` 等待有界 `[0,jitter]` | 冻结基线时多实例或重启后 scan/dispatch/reconcile 会同相位唤醒；现已恢复削峰等待并保留独立 loop/claim/fencing/取消边界 | 恢复有界 jitter 配置、默认值和随机等待；不能影响 claim/fencing 正确性 | 目标 crate 静态检查通过；仍需运行日志核对等待落在允许区间且取消可立即退出 |
| OPS-004 | 独立 migrate 门禁 | P1 | 静态修复，待 systemd R1 | 旧 `ops/deploy/start.sh:1455-1460` 在 Public/Admin/Worker 任一运行时拒绝独立 migrate | `auctionctl migrate` 已复用 deployment state-dir 锁并读取三个 unit 的 load/active/sub/MainPID；只有明确 loaded 且 inactive/failed、MainPID=0，或明确 not-found 才允许连接数据库 | 冻结基线允许操作员在进程运行期间改变 schema；现已把停机证明放在数据库连接和迁移之前，deactivating、unknown、查询失败及非零 PID 均失败关闭 | 部署控制器内部受控迁移继续由其既有停机顺序拥有；独立数据库维护共用同一守卫，不把 systemd 查询异常当作已停止 | auctionctl 静态编译已通过；仍需 active/inactive/failed/deactivating/not-found 与状态目录锁竞争的退出码和数据库未变化 R1 |
| OPS-005 | stop 幂等 | P2 | 静态修复，待 systemd R1 | 旧部署脚本在 pid/unit 不存在时按已停止成功收口 | systemd snapshot 已把明确 `LoadState=not-found` 归入 already stopped，loaded unit 仍按实际 active/sub 状态处理 | 冻结基线会让重复 stop 或部分卸载清理失败；现已恢复幂等收口且未吞掉权限/systemd 查询异常 | 只放行明确 not-found；权限、systemd 故障和未知 load state 继续失败关闭 | auctionctl 静态检查曾通过；仍需不存在、inactive、failed、active 四态的退出码和回执清理 R1 |
| OPS-006 | 数据库 bootstrap | P2 | 静态修复，待 R1 | 旧 init-db 会在目标 PostgreSQL 数据库不存在时创建后再迁移 | 新增 `postgres-bootstrap`：Public URL、人工目标确认、同 endpoint/TLS maintenance URL、deployment state-dir 停机锁、集群 advisory lock、CREATEDB/owner 校验、仅 CREATE DATABASE 和应用连接后置验证；普通 migrate 仍不建库 | 恢复显式建库合同，同时不把 CREATEDB 或覆盖能力带入普通 migrate | 建库 owner 为 `auctionctl` 的 PostgreSQL bootstrap 适配器；缺库后再显式 migrate，已存在只在 owner/连接一致时幂等成功 | 缺库、已存在、权限不足、名称非法、owner 不一致和连接异常均需在授权环境核对，且不触及其他数据库 |
| PUI-009 | Public 列表页大小 | P2 | 静态修复，待同数据 R1 | 旧共享 `TABLE_PAGE_SIZE = 10`，global/lot bid log 均请求 10 | global 与 lot bid log 请求均已恢复 page size 10 | 冻结基线的 50/20 会改变首屏密度、页数和翻页位置；现已恢复旧请求参数 | 其它列表继续按各自冻结合同，不用一个共享常量覆盖不同 owner | Public native/WASM 静态检查已通过；仍需同数据核对总页数、首尾行和翻页参数 |
| PUI-010 | Catalog 普通竞价入口 | P2 | 静态修复，待已认证 R1 | 旧 Catalog 仅在当前用户领先时禁用普通竞价；active proxy 只在 lot detail 阻断 | Catalog `ordinary_blocked` 已只按 leading 判定，lot detail 仍保留 active proxy 限制 | 冻结基线会让目录页活动代理用户失去旧入口；现已恢复两个页面各自的旧分支 | Catalog 与 detail 不共享会改变页面语义的阻断表达式 | Public native/WASM 静态检查已通过；仍需同一用户/lot 状态下的已认证按钮与错误 R1 |
| PUI-011 | Session 非匿名状态 | P2 | 静态修复，待四态 R1 | 旧页面仅在 session 明确返回 `null` 时显示登录 CTA，loading/error 不冒充匿名 | board/action 已传递 loading、error、anonymous、authenticated 四态，只在 Anonymous 显示登录；开放竞价看板在 session error 时于动作区前显示可见错误和原位重试 | 冻结基线把加载和网络错误折叠为匿名，后续看板又会静默隐藏动作；现已恢复等待、错误与身份事实分离及可操作恢复 | error 保留错误/重试，loading 保留等待，不根据请求失败推断会话不存在 | Public native/WASM 静态检查已通过；仍需四态 CTA、错误、请求重启和恢复后动作显示的浏览器 R1 |
| PUI-012 | Lot 历史列 | P3 | 静态修复，待动态行 R1 | 旧 lot history 为 bidder、amount、time 三列，proxy 作为 bidder 附近标记 | 历史表已移除独立 mechanism 列，并把 proxy 标记并回 bidder 单元 | 冻结基线多出一列，改变表格宽度和信息层级；现已恢复旧三列结构 | proxy 仍由稳定 bid DTO 决定，不通过可见 mechanism 列扩张旧表 | Public native/WASM 静态检查已通过；仍需动态行桌面/窄屏列数和代理标记 R1 |
| AUI-006 | Admin 列表页大小 | P2 | 静态修复，待同数据 R1 | 旧 Admin API hooks 统一使用 `TABLE_PAGE_SIZE = 10` | auctions/lots/mail/realtime/delivery/credentials/renames/users/proxy/audit 的旧列表请求已统一恢复 10；dashboard-only 聚合请求不作为可见列表 | 冻结基线的 20 至 100 会改变密度、页数和操作定位；现已恢复旧页面边界 | 可见旧列表使用 10；聚合 owner 的轻量读取需明确标为 dashboard 专用 | Admin native/WASM 静态检查已通过；仍需逐页同数据、mutation 后页位置 R1 |
| AUI-007 | Admin session 刷新 | P2 | 静态修复，待认证时钟 R1 | 旧 `admin/platform/session.tsx` 按 `min(expires_at, absolute_expires_at)` 安排刷新 | session 刷新 deadline 已取 idle/absolute 两者最小值，并保留提前量与最小一秒等待 | 冻结基线接近绝对过期仍按较晚 idle 调度；现已恢复旧 deadline 且避免忙循环 | 绝对期限不能被 touch 延长，过期边界继续由服务端权威决定 | Admin native/WASM 静态检查已通过；仍需 absolute 早于/晚于 idle 的认证时钟 R1 |
| AUI-008 | Dashboard 聚合 | P2 | 已并入 AUI-014 静态收口，待认证 R1 | 旧 dashboard 只消费各 owner 聚合后的单一完整 snapshot，不另读 auction/delivery 列表 | Dashboard 现只请求一次 `/api/v1/admin/dashboard`，不再从分页列表派生 active、attention 或 total；各卡片只消费该 snapshot 的旧版 section/water-level 事实 | 冻结基线的分页派生数字会随排序和页大小变化；移除额外列表请求后，该误报路径和多页耦合一并消失 | 没有 dashboard owner 聚合的数字不进入旧页面；内部增强指标不能原地扩张旧 v1 页面 | Admin native/WASM check 与 Clippy 已通过；仍需已认证 snapshot 下核对卡片值、请求数量和降级投影 R1 |
| AUI-009 | stale delivery dialog | P1 | 已静态修复，待真实 409 R1 | 旧 `DeliveryPage.tsx` 遇 stale 409 关闭确认框并失效/重读权威列表；TanStack 在 `onError` 回调后仍把 mutation 置为 error，故页面 alert 实际残留 | Dioxus 页面保留 `stale_delivery_state`/`stale_state_version` 的关闭、清命令和刷新分支；application 已增加稳定 `DeliveryAdminError`，storage 把 fence/state 变化归为 `Stale`，HTTP 恢复 409 `stale_delivery_state`；新版主动清 stale feedback | 修复前真实 stale 被统一折叠为 503，页面分支不可达；当前静态类型链已关闭该阻断。无残留 alert 是记录于 AUI-053 的旧缺陷修复，不再误称旧实现事实 | 保持 stale 关闭 dialog、清当前命令并刷新权威列表；普通 400/503 继续保留 dialog、输入和同一 `command_id`；stale alert 取舍见 AUI-053 | rustfmt、Application/Storage/HTTP/Admin WASM check 与 Storage/HTTP Clippy 已通过；仍需真实 stale 与非 stale 失败对照 R1 |
| PUI-013 | 竞价冲突后的权威快照刷新 | P1 | 已修复，待浏览器冲突 R1 | 旧普通竞价与代理 dialog 在命令失败后失效 `auctionRoot`，保留当前 dialog/command 上下文但重新读取价格、leader 和最低可接受金额 | 普通竞价、代理创建及代理降低失败现均先保留 error/input/command_id，再推进共享 `refresh_generation`；board、lot、bid log、history 与代理读取沿既有 generation 重读 | 修复前 `bid_too_low`、409 conflict 或并发代理变化后，用户看到错误却继续基于旧 `next_bid`、leader 和 price 重试 | mutation 失败保留同一 command_id 和用户输入，只推进现有刷新信号；成功路径和事实写入不变 | Public native/WASM check 与 Clippy 已通过；仍需同一 lot 制造旧价格提交，证明双方错误后 dialog 保留且页面价格/leader/最低金额更新、无额外 mutation 事实 |
| PUI-014 | `/me` 分页与表格投影 | P2 | 受控 populated 多页/双视口/五语言浏览器 R1 | 旧 `/me` 先读取 session，认证成功后才请求受保护列表；固定 page size 10，Network query 顺序为 `bid_page,page_size,proxy_page`。Bid 为 Lot/Amount/Status/Time 四列；Proxy 为 Lot/Secret maximum/Authorization history/Mode/Proxy bid uses/Status 六列，仍显示本人上限、历史、模式、次数和状态，但页面只增加上限保密提示。认证成功后直接进入两张表/空态，各自分页留在 section 内；页面使用独立 `public-account-table*` 样式、section 底部间距和专属 48rem 折叠 | session-first、一次 retry/10 秒 timeout、page size 10 与独立页码保持不变；GET 已恢复 `bid_page,page_size,proxy_page`，认证页移除新增说明/总数，四列/六列标题、空态及五语言词法逐项恢复。两张表和领先状态改回 `public-account-*` DOM/CSS owner，分页回到对应 section，桌面底部节奏和专属窄屏卡片规则按旧 CSS 落地 | 冻结的 query、信息闭集、表头词法、DOM owner、双视口几何和五语言差异已在同一合成只读输入下关闭。两侧页头因既有 readiness/数据库状态产生 46px 全局纵向偏移，账户 main 内部相对几何相同；该上游状态差异继续归 PUI-020，不回写账户组件 | 保持 session-first gate 和旧四/六列；富 DTO/严格 decoder 继续保留 maximum、transition、quota/capability 等安全事实，账户主表仍显示旧版已有的本人上限、授权历史、模式、次数和状态，只移除旧版没有的 capability/transition/quota 扩张说明，不把“上限保密”误实现为隐藏本人上限 | 隔离浏览器上下文向未修改旧 8092 与最新新 8090 注入同一只读 session、21 bid 和 21 proxy 快照；两侧首屏请求均精确为 `/api/v1/session` 后 `/api/v1/me/bids?bid_page=1&page_size=10&proxy_page=1`。Bid/Proxy 三页均为 10/10/1 行，首尾 lot 分别为 101-110/111-120/121 与 201-210/211-220/221，两个 pager 独立推进。1440×1000 与 390×844 下表头/卡片、section/pager 相对几何一致且无横向溢出；窄屏表头隐藏、行转卡片，316px 长授权历史完整换行。`en/zh-CN/zh-TW/ja/ko` 的 main 全文、标题、表头、首行、`data-label`、`html lang`、`hv.lang` 和请求序列逐项相同，全文哈希依次为 `b69ecee7/9858d1e5/6f38b1ab/947acc84/67f75119`。探针未保存为测试脚本，也未写数据库；真实账号/真实数据库链路仍由认证/API 项目单独取证 |
| PUI-015 | Public 查询参数与历史分页边界 | P2 | URL/请求边界局部 R1，旧双 owner 缺陷不回迁 | 旧 world 只允许 `main/isekai`；缺失或非法 query 只在内存选择 `main`，不重写地址。用户切换使用 push：Isekai 写 `world=isekai`，Main 删除 `world`；只有服务端 worlds 使当前选择失效时才 replace 并删除 auction。price history 固定 page size 10，current 始终显式携带 world。旧非法 world 首屏还因 shell/page 两个 owner 分别使用固定 main 和 worlds 插入顺序首项而同时请求 main、isekai | history page size、显式 world 请求和世界闭集已恢复；首次缺失/非法 query 不 canonical replace，用户切换恢复 push/Main 无参数，服务端失效回退仍 replace 并清理 auction。Public 共用 GET 边界改用原生 `web_sys::Request`，绕开 gloo-net 对已有 query 自动追加空 `&` 的实现缺陷；共享 world owner 对非法值只请求 main | URL、选择器、浏览器历史和 current 精确 query 的新版差异已关闭。旧非法值双请求会让页头 main、页面 isekai 状态分裂，属于旧版重复 owner 缺陷而非外部契约，因此不回迁 | 保持世界闭集和 push/replace 分工，不把未知世界作为普通筛选项；请求 URL 必须逐字保持 serializer 结果；服务端若新增世界须先修改冻结契约 | 根路径与非法 `?world=garbage&foo=1` 均保持原 URL；Main→Isekai→Main 两侧地址和 history length 增量相同。最终 embedded 网络记录中的 current 精确为 `/api/v1/auctions/current?world=main`、无尾随字符；旧非法 world 的 main+isekai 双请求已由双组件源码和 worlds JSON 插入顺序证明，新版只读 main 是有意的缺陷修复 |
| PUI-016 | Public session header 与 `/me` 请求序列 | P2 | 四态、失败及登录动态错误局部 R1 | 旧 header 在 session loading、error、`null` 三态都保留 `/me` 与 `/login` 入口；已认证退出期间文案不变，失败只供屏幕阅读器播报。`/me` 先读 session，认证成功后才发 bids/proxies，请求由共享缓存合并；会话/登录响应严格校验且账户请求具有 10 秒 timeout 和一次约 1 秒重试。直接登录地址精确为 `/login`，blacklisted reason 使用独立 span | header 三态入口、认证用户名/退出文案、隐藏失败 alert、唯一 session owner、严格 UUID/用户名/时间/状态解码、login error 分类以及账户 timeout/retry 均已恢复；退出失败保持认证快照，不刷新 current。所有动态译文经共享旧版兼容宏移除 Fluent 自动 FSI/PDI；Router 挂载后只对原始无 query marker 的裸登录移除 Dioxus 生成的空 `?` | 已知 header DOM、退出反馈、坏响应失败关闭、请求失败边界、裸登录 URL、blacklisted reason 和动态变量不可见字符差异已关闭；`/me` 额外内容/几何继续归 PUI-014 | 保持唯一 session owner；header retry 只重读 session，登录成功复用返回快照，退出失败不把用户伪装成匿名；用户原本输入 `/login?` 时保留该 marker，非空 query 不改写 | loading/error/null/authenticated、登录成功、非法 `suspended`、退出失败均取得同浏览器局部 R1；旧新登录成功各一次 GET/POST，退出失败各一次 GET/DELETE。最终 blacklisted mock 两侧均为 `/login`、一次 GET/POST、主文案“该账号已被封禁。”和独立 `.public-account-login-ban-reason`“封禁原因：公开原因”，正文 FSI/PDI 为 0；10 秒 abort/一次重试见 PUI-014 |
| PUI-017 | Catalog 代理查询与动作闭集 | P2 | 静态修复，待请求序列 R1 | 旧 Catalog 每个 lot 只依赖 session；始终提供普通“代理”入口，点击后才读取所选 lot 的 active proxy；详情页单独读取并展示代理状态 | Catalog 初始渲染现不再请求 per-lot mine；普通“代理”入口保持稳定，只有打开所选 lot 的代理弹窗时才触发一次读取；lot detail 仍立即读取摘要并保留降低能力与显式错误 | 修复前已认证目录产生 N+1 并按 active capability 改写主表动作；现已恢复旧请求时机和 Catalog/详情职责边界 | 弹窗读取失败继续显示明确重试，不把错误伪装成无授权；普通竞价阻断仍由既有 PUI-010 规则控制 | Public native/WASM 静态检查已通过；仍需多 lot 已认证页面核对首屏零 mine、点击后仅一个 mine，以及 active proxy 在 Catalog/detail 的按钮、dialog 与错误态 |
| PUI-018 | SPA 内部导航、query 与长文本 | P2 | 已静态修复，待无刷新导航 R1 | 旧目录、历史、lot 返回和拍卖链接使用 Router `Link`；board query 更新保留未知参数；长文本用可聚焦截断与 tooltip | `LotDetailBack` 与历史拍卖“上一轮”已改为 Dioxus `Link`，其余站内链接、query helper 和共享截断组件也已恢复；论坛/装备等外链继续使用原生 `<a>` | 已消除已知站内 document reload owner；实际滚动、realtime 内存状态和浏览器历史仍须运行态证明 | 保持站内 `Link`/站外 `<a>` 闭集；后续新增链接按目标 origin 选择 owner | rustfmt 与 Public WASM check 已通过；Catalog→Lot→返回及历史→上一轮仍需人工确认无 document reload、实时状态不重置且未知 query 保留，长文本继续按桌面/390px 核对 |
| PUI-019 | 论坛同步动态面板 | P2 | main 五态桌面 R1，窄屏/isekai 局部缺口 | 旧 live 页面在主体前只挂一个 `public-page-status`：loading；`pending/publishing/failed` 都显示“同步中（原状态）”；`published` 只多一个论坛外链；404/其它错误均为普通状态提示，没有 wrapper、标题、详情或重试按钮 | 主页面现把论坛状态移回 live 主体首项；loading/error/非 published/published 均只输出旧单行 `role=status`，published 额外带 `aria-live=polite` 和论坛外链。title/body/revision/time 继续保留在 API/存储/Worker，不进入页面；DTO 拒绝未知字段，浏览器恢复 auction UUID/一致性、非负 revision、时间、状态闭集及 published 三个非空字段校验 | 已知信息披露、DOM 顺序、辅助技术语义和长正文布局扩张已关闭；五语言只是承载同一旧语义，不再改变主页面结构 | 保留 API/存储/Worker 中的 title/body/revision/time 权威事实；主页面只消费状态与 published URL，不把后端增强事实塞回旧 DOM | main 的 loading/pending/publishing/failed/published/404 已按文本、role、aria-live、链接和首子节点取得桌面局部 R1；pending 桌面状态块两侧均为 x240/y97/w960/h87，390px 状态块均为 x24/w342/h87，但新版受上游 shell 影响 y271、旧版 y273。isekai 与上游 2px 几何继续保留 |
| PUI-020 | lot realtime 辅助状态 | P3 | 六态静态一致；ready+anonymous 双视口/移动逐像素 R1，其余状态待证 | 旧有 current auction 时只建立 auction-specific WebSocket，无 current 时才使用 discovery；核心 LiveRoom 只持有 connecting/degraded/live/manual。PublicApp 另一次性读取 `/health/ready`，按优先级投影外层六态：readiness checking→connecting、非 ready→unavailable、session pending→connecting、未认证或 session error→manual、已认证无 current 且 discovery live→idle、同场景 degraded→connecting，其余沿用核心状态；刷新按钮只在 degraded/manual 出现。旧两个 session observer 均 `retry:false` | Public 外壳保持唯一 session/current/realtime owner、auction/discovery 二选一、目标 world、独立 cursor/cache、3 秒重连和 refetch 保留连接；一次性 `/health/ready` 与 `PublicConnectionState` 已精确组合六态，五语言/class/圆点/刷新条件一致。新 session reader 已移除首次失败后 300 ms 的第二次读取，恢复与旧版相同的一次请求/显式重试 owner | 运行库 17/19 时的 46px 偏移已证明来自异输入；迁移到 19/19 后，旧 8092 与新 8090 均为 `ready + anonymous → manual + refresh`，双视口全部几何和可见文本一致，390px 截图逐像素相同。核心六态、CSS、五语言和匿名请求次数已经闭合；checking、错误、认证 current/discovery 与切换时序仍不能由本场景外推 | 保持 `LiveConnectionState` 四态和单 owner；页头六态只消费共享资源与核心状态，刷新按钮仍只看 degraded/manual。session 失败不自动重试，只有用户/共享 refresh owner 才重新读取；继续对 checking、错误、认证与目标切换补同输入证据 | 迁移后 1440×1000 两侧 header/main/heading top 均为 `0/104/128px`，状态与刷新按钮矩形逐项一致；390×844 两侧 header/main/account 均为 `324px/324px/342×215.5px`，无横向溢出。移动 PNG SHA-256 同为 `e0527bd8e57c4164659efd19393725c232fca9a48e2e6bc8fc2cd011eb8e1b07`；Network 各只有一次 `/api/v1/session`。新版无 console error/warn，旧版仅有字段 id/name 的既有浏览器 issue；仍需 checking、session loading/error、认证 current 有/无、discovery degraded、网络/坏 readiness 与 world/target 切换 R1 |
| PUI-021 | Public 合法 lot 可选字段解码 | P0 | 已修复，实际根页局部 R1 | 旧浏览器 lot decoder 允许合法响应省略 `stats` 与 `info_summary`，并分别按空统计/无摘要处理 | Rust DTO 曾把两字段设为必填，因而拒绝新版 HTTP 自己返回的合法 200；现为两字段补 `serde(default)`，其余严格未知字段闭集不变 | 修复前 API 正常且数据库有一条 live lot，但根页被浏览器解码失败折叠为空；这不是空态或数据库差异 | 仅恢复旧 wire 的可省略语义，不放宽字段类型、未知字段、lot/auction 一致性或业务状态校验 | 同一实际运行库中，修复前根页无行，修复后的 embedded 根页恢复一条权威 lot 表格行；Contracts/Public 原生与 WASM 静态检查通过，仍需 populated 多字段样本 |
| AUI-010 | Settlement recovery 409 生命周期 | P1 | 已静态修复，待冲突 R1 | 旧 confirm 对全部 HTTP 409——`stale_recovery_state`、`recovery_not_confirmable`、`recovery_evidence_unresolved`、`command_conflict`——都关闭 dialog、清空 reason、只失效 recovery 查询并恢复 opener 焦点；mutation error alert 仍保留。页面级 signature→command_id 映射在所有错误后保留，只在成功后删除 | 新版现按所有 409 关闭/清草稿/保留错误，mutation 只推进独立 recovery generation；signature→command_id map 已提升到页面 owner，失败保留、成功删除。400/503 继续留框复用，dialog 已补 `aria-describedby` | 已知四类冲突的 DOM、刷新 owner、错误反馈和幂等重试静态生命周期恢复；共享 focus trap 的 opener 时序仍缺运行态证据 | 保持所有 409 同一旧收口，不按内部 code 分裂；版本变化后的新 signature 自然生成新 ID，领域/事务不变 | Contracts/HTTP/Admin 原生、Admin WASM、Clippy、rustfmt、i18n 与差异空白门禁通过；仍需分别构造四种 409、400、503，核对 dialog、reason、alert、请求数、焦点与 command_id |
| AUI-011 | Intake 成功后的 command_id 生命周期 | P2 | 已修复，待连续提交 R1 | 旧 intake 成功后删除命令 ID，下一次相同表单生成新命令；可重试失败保留原 ID | 成功分支现清空 `command_id_state` 并刷新所属页面；失败分支保留 ID，且结构化错误仍就地反馈 | 修复前第二次真实录入可能被错误当作幂等重放，表面显示成功却没有新增 intake 事实 | 成功后新业务动作必须生成新 ID；同一次网络/服务失败重试继续使用原 ID | Admin native/WASM check 与 Clippy 已通过；仍需连续两次相同表单得到不同 ID/两份业务结果，并证明同次失败重试 ID 不变；无新增测试用例 |
| AUI-012 | Admin settlement 页面投影 | P2 | 11 列/卡片闭集已静态收口；数值与刷新差异另列 | 旧 page size 10、lot 表 11 列且无独立 tax 列；pool 卡片顺序为 tax→discount→compensation→closing→opening；标题下无说明，只显示 settled 时间 | 当前请求、11 列主表、卡片顺序、标题区和摘要可见文本均已恢复；`movement_count` 继续保留在 API/报告事实中但不进入旧版主页面 | 主结构扩张已消除；后续审计确认 hold 四舍五入、金额对齐、loading/error 外壳和 recovery 成功刷新仍不一致，分别登记 AUI-025/AUI-026/AUI-027/AUI-029 | 保持旧主页面闭集，新增守恒事实进入 API 或二级详情，不挤入冻结摘要；按新条目修正功能差异 | 既有 rustfmt/i18n/Admin WASM 证据只覆盖静态闭集；仍需同报告逐页核对数值、五语言、卡片/表格几何和成功刷新 |
| AUI-013 | Admin 动态列表与默认输入投影 | P2 | Mail/Users/Renames populated 与 Users 创建 390px 局部 R1；其余状态待证 | 旧创建 sequence 默认 `1`；Credentials 成功后清空 label/secret 并重置 kind/world 为 `mail_cookie/main`；Users/Renames/Mail 主表分别为 7/6/8 列。Users 显示原始 status、最新公开封禁理由及旧四动作闭集，完整历史只由聚焦/悬停 tooltip 展示；Renames 使用旧名/新名/方式/时间/回滚时间/操作，空回滚时间为空。Mail polling 默认折叠且其失败不遮蔽 intake 列表 | 创建 sequence、Credentials 成功复位和 Mail 均已恢复。Users/Renames 主表与动作已恢复旧闭集；Users 创建又恢复旧表单结构、输入归一化、手工 Key 长度约束、一次性 Key 结果和页面级成功反馈 | 已知字段、动作能力、原生表格语义、当前 populated 行高/列宽、代表确认框及创建流程差异已关闭。旧版若干 locale 泄漏简体中文或英文，且创建成功把 `{{name}}` 原样显示；新版按项目五语言合同正确本地化并插入真实用户名，不回迁这些缺陷 | 保持旧 7/6/8 列和页面级结构闭集；新增安全事实只供命令并发控制或二级详情。五语言资源分别本地化，但除旧翻译/插值缺陷外不扩张可见字段或动作 | Mail/Users/Renames populated 证据沿用；Users 创建在五语言 390px 下完成 generated/manual 201 与 manual 503→重试，字段 trim、`minLength=32`、Key 清空、同 ID 重试、busy Escape 和 opener 均已记录。仍需 empty/error、超过 10 行、409 及其他真实写动作 |
| AUI-014 | Dashboard/Status/Outbox 请求模型 | P2 | Dashboard/Status 已认证局部 R1；Outbox 主结构静态一致、细节待修 | 旧 Dashboard 单快照并按固定卡片顺序展示；Status 仅静态说明且不发业务请求；Outbox 先 failure、后整体折叠 lag，两侧独立 loading/error，failure 为 12 列、lag 为 consumer→stream 五列，resolved 不显示 replay | Dashboard/Status 已恢复；Outbox 的 failure 优先、旧 12/5 列、默认折叠、独立资源与 replay dialog 主结构存在 | Outbox 静态复核又确认 status DOM value、Replay busy 输入和 HTTP error code 不同，见 AUI-032/API-017；此前“已恢复”不能外推完整生命周期 | Dashboard 只消费旧 v1 snapshot，Status 保持零业务请求；Outbox 保持独立 owner 与旧主表闭集，幂等 ID 保留，新差异按专条处理 | Dashboard/Status 证据沿用；Outbox 全成功/单侧失败/空态/replay、五语言与两 viewport 统一归 EVD-006 |
| AUI-015 | Delivery stale 反馈闭集 | P1 | 已并入 AUI-023 静态修复，待运行态 | 旧 409 stale 会关闭 dialog、刷新列表并恢复 opener，但 mutation 最终仍进入 error，普通 alert 残留 | 页面视觉分支与服务端 409 typed mapper 现已同时具备，stale 不再进入通用 503；新版按目标语义清理 stale alert | 原先只修页面导致闭集不可达；旧 alert 残留的真实时序已由 AUI-053 单独记录为不回迁缺陷，不能继续用错误旧事实证明一致性 | 与 AUI-009/AUI-023 共用 409，保留关闭/刷新/新 command 语义；普通 400/503 保持可见错误 | 待真实 409 后确认无 alert、opener 恢复且只刷新 Delivery；普通 400/503 应保留 dialog/输入，并在证据中明确这是旧缺陷修复而非像素等价 |
| EVD-003 | Admin 动态数据 WebUI | P2 | Mail/Users/Renames populated 390px 局部 R1 | 旧 Mail、Users、Renames、Delivery、Settlement、Audit 等均有冻结的 populated/empty、分页、错误和动作状态 | 新 Dioxus 页面均已接入权威 API；Mail 单条 failed、Users 单条 blacklisted 及 Renames 单条未回滚投影已用同一脱敏响应完成窄屏对照，Settlement 摘要按 AUI-012 恢复 | 三条 populated 证据不能外推空态、多页、动作提交或其他页面；Users 非中文 locale 的正确本地化与旧中文泄漏是有意差异，不能误记成结构回归 | 继续使用只读脱敏响应逐页对照；新增事实进入详情/二级面板，不扩张旧主表；旧翻译泄漏按五语言合同修正并明确记录 | Mail/Users/Renames 已记录五语言文本、表头/单元格、action display、wrapper/table/row/pager、tooltip 和代表确认框；超过 10 行、成功/空/错误/stale、动作交互及 Delivery/Settlement/Audit/Outbox 等仍待证 |
| AUI-016 | Admin 窄屏表格投影 | P2 | Mail/Users/Renames populated 390px R1；其余页面待证 | 旧版在窄屏继续保留 table 语义，由页面 wrapper 横向滚动；列、表头和行高不变，Users/Renames 的动作 `td` 仍为 `table-cell` | 已删除新版全局 48rem 卡片化、隐藏表头和逐单元格 `data-label` 伪标签规则，恢复原生 table/thead/tbody/tr/td 与 wrapper 横向滚动；Users/Renames 的动作现位于单元格内部 wrapper，不再改变 `td` display，扩张内容也已移除 | Mail/Users/Renames 当前 populated 样本的共享及页面级破坏均关闭；其他页面的页面级 CSS、动态长文本与 dialog 占位行仍不能由三页外推 | 保持全局原生表格；动作布局只放进 `td` 内部 wrapper；只实现旧版明确存在的页面专属移动规则 | Mail table 750.563×160.406、row 高 80.203；Users zh-CN table 771.484×510.906、row 高 409.094；Renames zh-CN table 645.844×182.016、row 高 80.203，三页均保持横向 wrapper 和原生 table-row/table-cell。Delivery/Settlement/Audit/Outbox 仍需逐页记录 |
| AUI-017 | Admin dialog 生命周期与可访问性 | P2 | 公共结构已收口，Users/Renames/Users 创建五语言 390px 局部 R1；其余写入待证 | 旧版 17 个 dialog 状态分支由页面显式保存 opener，并为每个 dialog 指定初始焦点；提交型 dialog 在 busy 时禁止 Escape，关闭路径阻止传播。公共 focus trap 处理最顶层 dialog 与双向 Tab 循环；Admin 不锁 body 滚动。dialog-level `aria-describedby` 只属于 Settlement recovery；公共 overlay/panel 固定为 32rem、视口减 2rem最大高度、内部滚动和统一层级/圆角 | 新版 13 个组件/分支统一复用旧公共外壳，shared trap 按 DOM identity 处理 create→generated-key 替换、循环、清理和 opener。Users 创建已补 form/ID/help/minlength、busy Escape、201/503 生命周期与一次性 Key 替换 | 公共 Escape、几何、滚动和 identity 风险已关闭；Users 创建的结构与动作生命周期已补证。Delivery 二次确认/按钮顺序/服务端 409、Recovery 刷新范围及 Realtime busy 字段仍各有明确差异，不能由代表框外推 | 保持每种 mutation 的旧关闭策略，不统一错误生命周期；初始焦点优先显式 `autofocus`，共享 trap 只负责最顶层循环、身份替换和 opener；Admin 不新增旧版没有的通用 ARIA 属性或 body lock | 最新 embedded payload `d29e1521dacfa69282eb385730c00a03774279116c80f4f8d222d06e0e07c379` 下，原 Users/Renames 代表证据继续成立；Users 创建五语言 generated/manual、一次性 Key、busy Escape、503 同 ID 重试及成功后 opener 已记录。简中/繁中创建框几何逐项相同；其它 locale 因修复旧简中帮助泄漏而只保留有意文本/高度差。仍需 Delivery/Recovery/Realtime、409、长正文和 1440px |
| AUI-018 | Admin GET 查询 URL | P2 | 已修复，精确 URL 浏览器 R1 | 旧 Admin 浏览器使用原生 fetch，serializer 结果直接成为请求 URL；受控 `/auctions` 实际请求精确为 `?page=1&page_size=10`，邮箱值 `mail box+一` 精确编码为 `?mailbox_key=mail+box%2B%E4%B8%80` | 修复前 Admin 把已含 query 的 URL 交给 gloo-net 0.6.0 `Request::get`，实际 `/auctions` 为 `?page=1&page_size=10&`；现已在 API 根建立共用 `send_get`，21 个 GET 调用点统一直接构造 `web_sys::Request` 后交回 gloo response owner | 尾随空参数造成的代理缓存键、访问日志、签名输入和严格请求对照差异已关闭；代表请求取得逐字证据，其余调用点由同一共享边界静态覆盖 | 保持 serializer、响应解码和 mutation owner 不变；后续新增 Admin GET 必须复用 `send_get`，不能重新使用会改写 URL 的 builder | 最终 embedded 的 `/auctions` 与邮箱编码请求分别精确为 `?page=1&page_size=10`、`?mailbox_key=mail+box%2B%E4%B8%80`，与旧版逐字相同；Admin 原生/WASM check、Clippy、rustfmt、i18n 与差异空白门禁通过，未新增浏览器测试脚本 |
| EVD-004 | Admin 响应式与 dialog 可访问性 | P2 | 壳层、Mail/Users/Renames 表格及 Users 创建五语言 390px 局部 R1 | 旧固定 12rem 侧栏、横向表格及页面级焦点恢复 | 新导航已修回固定侧栏，1440px/390px 壳层与旧版一致；全局卡片化已移除，Mail/Users/Renames populated 表格取得 390px R1。Users 创建→一次性 Key 又补充五语言 201/503、焦点和 busy Escape 证据 | 当前代表页面差异关闭；正确本地化造成的有意文本高度差已单独记录。动态长文本、409、其它页面级 CSS 和 dialog 生命周期仍不能外推 | 不新增浏览器测试用例；按 AUI-016/AUI-017/AUI-019 的一次性人工矩阵记录 | Users/Renames 代表框及 Users 创建/Key 已记录；Delivery/Settlement/Audit/Outbox populated 表格、各自错误/冲突、滚动与 1440px 仍需证据 |
| AUI-019 | Admin 共用分页组件 Pager/AdminPager | P2 | 实现已收口，Users/Renames 五语言 390px 代表 R1；非法/越界与全调用点待证 | 旧 Pager 固定每页 10 条；越界页由 effect 回调校正；跳页只接受纯数字 safe integer 且非法时 Go 按钮 disabled；form 带 `noValidate`，标签可见。布局左对齐，主 gap/顶部间距均 12px，按钮/input 最小高度 44px；390px 下 controls/jump 各占满一行但内部不换行 | `AdminPager` 现保留调用方原始 page 并由 effect 回调校正越界；跳页输入只接受纯数字和有效范围，非法时 Go disabled；form 恢复 `novalidate`、可见 label、旧 input/class/按钮结构。共享 CSS 恢复左对齐、12px gap/顶部间距、44px 控件及窄屏两行 | 已知 DOM、原生校验入口、触控目标和代表窄屏几何差异关闭；14 个调用点的参数、非法输入和越界首帧仍需运行态覆盖 | 普通列表保持旧每页 10 条；组件继续使用响应中的 page_size 计算页数，不伪造服务端分页；Settlement 本地包装单独核对 | Users/Renames 五语言的 pager 文本、class、display、顶部 12px 和 390px 布局相同；zh-CN Renames pager 高 144.5。仍需 1440px、非法/溢出输入、越界回调及 14 个调用点 page/page_size/total |
| EVD-005 | Admin Pager 运行态证据 | P2 | Users/Renames 五语言 390px 代表 R1 | 旧 Pager 的 nav/form/label/input/button DOM、disabled/noValidate 及 1440/390 计算样式 | 新 AdminPager 已恢复同一结构和 CSS；Users/Renames 同响应下的五语言文本、顶部间距和窄屏两行已记录 | 两个代表页面不能替代 14 个调用点的参数、非法输入和越界页复核 | 使用一次性受控响应和 DOM 读取；不提交浏览器测试脚本 | 继续核对可见标签、非法输入 disabled、越界回调、44px 控件、1440px 左对齐及各调用点 page/page_size/total |
| AUI-020 | Admin Users 创建与一次性 Key 生命周期 | P1 | 已实现，五语言 390px 201/503 局部 R1；409/1440px 待证 | 旧 `form.admin-users-create` 包含标题、分离 label/input、`game-id-help` 和 submit，取消为独立 actions 兄弟；提交前 trim 两字段，手工 Key `minLength=32` 且提交即清空；成功关闭创建框，generated 模式替换为一次性 Key 框，关闭后恢复 opener。旧成功 notice 因错误使用 i18next 插值参数实际显示字面量 `{{name}}`；旧自定义 manifest 又把 game-id help 泄漏为全语言简中 | 新版已恢复旧 DOM/ID/class、select padding、help 描述关系、trim、32 字符约束、提交即清 Key、旧按钮/Key 框文案和页面 owner；同 payload 失败重试复用稳定 `command_id`，成功后删除。成功 notice 正确插入真实用户名，help 在五语言分别本地化，作为旧插值/翻译缺陷修复保留 | 原功能分叉、长度约束、表单结构、一次性结果和反馈丢失均已关闭；只剩旧翻译泄漏与字面占位符不回迁，因此 en/ja/ko 的帮助文本高度和旧版不同，ko 页面滚动条还会影响 Key 框可用宽度 | 保持旧交互和几何 owner，同时遵守五语言合同与幂等不变量；409 仍按真实错误 owner 单独核对，不从受控 503 外推 | payload `d29e1521dacfa69282eb385730c00a03774279116c80f4f8d222d06e0e07c379`。简中/繁中 generated 358×564.781、manual 358×655.781 与旧版相同；Key 框 en 358×378.594、zh-CN/zh-TW 358×356.094、ja 343×378.594 相同。五语言请求均 trim，`minLength=32`；manual 首次 503 时只有 submit/cancel disabled、Escape 保持打开、Key 清空，第二次同 ID 成功并恢复 opener。旧错误翻译泄漏与 `{{name}}` 字面量由新版正确五语言文本/真实用户名替代；仍需真实 409 和 1440px |
| AUI-021 | Delivery 列表字段与行投影 | P1 | 已实施，五语言 empty shell R1；populated 行待证 | 旧浏览器显示 8 列：ID、Type、To、COD、Status、Attempts、Last error、Actions；ID 优先取 `origin_id` 数字后缀，否则取 delivery UUID 前 8 位，`settlement` 类型显示为 `equipment`。旧浏览器状态闭集不含 `prepared`，尽管旧后端领域闭集允许该状态；空态先渲染空表再显示提示，页面没有说明/total H2/“无操作”占位 | 新表已恢复旧八列、表头翻译、短 ID/full tooltip、type 映射、空 failure、左对齐数字、空动作单元格和 table→empty 顺序；wrapper/table/badge/number/actions 已恢复旧 class 与普通 badge 视觉，行级 `data-label` 和着色状态扩张已移除。`fencing_version` 仍只供命令使用；`prepared` 安全显示/筛选继续保留 | 已知桌面/窄屏列数、顺序、ID 可读性、状态装饰和信息层级差异关闭；保留 `prepared` 是避免旧浏览器拒绝旧后端合法状态的缺陷修复 | 保持旧主表八列；内部 fencing 不进入投影。新增合法状态可显示但不能顺带扩张列 | embedded Admin payload `01876a44473c871335cee59b5b5fc2f361f1e3fecb0771aa4b8a22cde25063ac` 下，新旧 empty 页面均为 `section.admin-delivery-page`、同标题/筛选 ID、`admin-delivery-table-wrap > table`、message、pager 顺序且没有说明/total H2；五语言八列表头与 pager 名称逐字一致。专属 empty 及含 `prepared` 的完整本地化筛选是 AUI-092/AUI-073 登记扩展；仍缺同一 populated 响应核对短 ID、type/状态、八个 td 和 390px 行布局，探针未保存为脚本 |
| AUI-022 | Delivery manual reconcile retry 二次确认 | P1 | 已实施，待动作 R1 | 旧行内 reconcile 表单选择 `retry` 并提交后，不直接写命令；先关闭行内框，再打开 release/retry 共用的第二确认框，操作员再次 Confirm 才调用 reconcile | `DeliveryDialogKind::ReconcileRetry` 现只表示第二确认阶段；第一次表单 submit 由页面把 dialog identity 从字段框替换为共用 Retry 框并立即返回，只有第二次 Confirm 才进入命令/`command_id` 路径 | 已消除一次 submit 直接写命令的误触路径；succeeded/failed 仍从字段表单直接提交 | 二次框沿用原行按钮 opener；稳定命令签名只在最终确认时建立，Cancel 不产生 mutation | rustfmt、i18n、Admin WASM check 与目标 Clippy 已通过；待记录第一次 submit/第二框 Cancel 均零 mutation、Confirm 恰一请求、成功/503 焦点与同 ID 重试 |
| AUI-023 | Delivery typed error 与 HTTP 映射 | P1 | 已实施，待六类事务 R1 | 旧命令把 invalid→400、not found→404、stale state/fence→409 `stale_delivery_state`、同 ID 异 payload→409 `command_conflict`、依赖失败→503 `temporarily_unavailable`；页面据 HTTP status/code 选择关闭或保留 dialog | application 已定义不携带敏感正文的 `DeliveryAdminError`；storage 精确产生 Invalid/NotFound/Stale/CommandConflict/StoredResultCorrupt/Unavailable，并校验幂等结果的 command/delivery identity 与安全快照；HTTP 恢复旧 400/404/409/503 code/status，SQLite capability 501 保持显式 | 通用 `RepositoryError::Failed(String)` 不再吞掉 Delivery 业务分类，页面 stale 分支重新可达；真实数据库 race、坏账本和依赖故障仍需运行态证明 | 错误只按类型映射，不匹配正文；状态不允许、fence 改变和关联 fulfillment 漂移均归 stale，坏投影/坏账本与数据库故障失败关闭为 503 | rustfmt、Application/Storage PostgreSQL/HTTP/Admin WASM check 与 Storage/HTTP `-D warnings` Clippy 已通过；仍需真实事务分别触发六类结果，409 驱动旧 dialog 生命周期，503 保留输入和 `command_id` |
| DLY-001 | Delivery Admin 并发命令幂等 | P0 | 已实施，待并发 R1 | 旧命令在 Serializable 事务中执行并最多重试 3 次；同 `command_id` 同摘要并发会返回已保存的同一结果，异摘要或异 delivery 返回 command conflict；唯一冲突后重新加载 ledger 恢复结果 | PostgreSQL 命令现先按 command ID 获取事务级 advisory lock；delivery 行锁后、fence/status 判断前再次读取 typed ledger。`ON CONFLICT DO NOTHING` 为零时先回滚当前事务，再用新事务读取已提交账本并恢复结果或返回精确 conflict | 已消除后到同 ID 请求先撞上变化后 fence 而误报 stale，以及唯一冲突后丢失已提交结果的静态路径；Read Committed 与旧 Serializable 有限重试差异仍由 DLY-004 跟踪 | 保持命令锁、行锁后二次读取和事务外 typed 恢复；账本身份、摘要与安全快照任一不符都失败关闭，不匹配数据库错误正文 | Storage PostgreSQL check、目标 Clippy 与差异空白检查通过；仍需 PostgreSQL 隔离 R1 覆盖同 ID 同 payload、异 payload、异 delivery、过期 fence/status 和唯一冲突恢复，并核对 delivery/settlement/ledger/audit 无失败副作用；不编写测试用例 |
| DLY-002 | Delivery Admin 旧摘要兼容 | P0 | 已实施，五动作双向交叉实例 R1；特殊字符待证 | 旧 release/retry 摘要只包含 delivery ID、expected fencing version、actor ID；reconcile 再包含 action 及结果字段，request ID 始终排除；字符串按 Go `encoding/json` 的 HTML-safe 规则转义 | Delivery owner 现按旧字段顺序构造 canonical bytes，release/retry 忽略 reconcile 字段，reconcile 把缺失结果编码为空字符串，并兼容 `<>&`、U+2028/U+2029 与控制字符转义；HTTP 不再写通用 envelope 摘要 | 已消除新实例首次写入必然不被旧实例接受、旧 ledger 必然不被新实例接受的算法分叉；若此前未发布版本已经向生产账本写入 envelope 摘要，仍需部署前单独盘点 | 旧 canonical hash 作为唯一新写入摘要；不能只双读两种摘要，因为旧实例无法接受新 envelope。`request_id` 继续只进入审计，不进入摘要 | Application/Contracts/PostgreSQL Storage/HTTP/Admin-WASM 的目标 check、五目标 `-D warnings` Clippy、i18n 与 diff 已通过；同一临时 clone 上 release/retry 由旧实例首次写入后新版均 200 重放，reconcile succeeded/failed/retry 由新版首次写入后旧版均 200 重放，五个命令最终仍只有五份 ledger/五条 audit。普通字段双向闭环已证明，仍需逐字核对 `<>&`、U+2028/U+2029 与控制字符摘要 |
| DLY-003 | Delivery 已存快照完整不变量 | P1 | 已实施，坏完整账本局部 R1 | 旧 `ValidStoredDelivery` 按状态校验 lease、attempt/fencing、claimed/sending/completed 时间轴、external reference/failure 排他、origin/text 合法性；HTTP 还要求结果状态符合动作且 fencing 已前进 | live row 现读取并校验 lease token，payload/result metadata 必须为对象；单一 `valid_snapshot` 已覆盖 origin/text、计数、时间轴及七种状态的必有/必空组合。ledger replay 复用该 validator，并额外校验 command/delivery identity、动作终态和相对 expected fence 的前进 | 结构合法但动作错位、时间倒序、lease/结果字段冲突或 fence 未前进的持久化结果均失败关闭，不再作为成功重放返回 | live row 与 ledger 共享状态不变量；账本身份/摘要不匹配仍归 command conflict，正文或快照损坏统一归 typed stored-result-corrupt/安全 503，不提交新事实 | PostgreSQL Storage check、`-D warnings` Clippy 和全批静态门禁通过；隔离 clone 删除完整 payload 必填 `CommandID` 后新版重放返回 503 `temporarily_unavailable`，delivery/ledger/audit 均未追加；仍需逐类覆盖时间轴、lease、动作终态和身份损坏 |
| DLY-004 | Delivery Admin Serializable 有限重试 | P2 | 已实施，40P01 三次边界 R1；40001 待证 | 旧 `RunSerializable(..., 3)` 对 40001/40P01 最多重跑完整事务三次，等待 10ms/20ms；业务结果和无明确 SQLSTATE 的失败不重试 | 新版现以独立 typed wrapper 开启 fresh Serializable 事务，begin/SET/query/commit 原始 SQLx 错误保留到最外层；仅 40001/40P01 最多重试三次，业务 stale/conflict/坏账本不重试，唯一冲突恢复也使用新 Serializable 事务 | 已关闭 Read Committed 及明确序列化/死锁冲突直接 503 的静态差异；未知或结果不明 commit 错误仍安全 503，不以幂等假设擅自重放 | 保留 advisory lock、行锁后二次 ledger、事务回滚后恢复和 DLY-005 完整旧 payload；每次重试重新执行完整事务，不复用已失败 transaction | 隔离 clone 以外部行锁和可释放 session advisory lock 构造真实二进程锁环：一次及连续两次 `40P01` 后 HTTP 均为 200，第三次尝试只写一份 delivery/ledger/audit；连续三次 `40P01` 时 HTTP 为 503 `temporarily_unavailable`，delivery 保持 `failed/fence=1` 且 ledger/audit 均为 0。外部事务全部正常提交、`pg_stat_database.deadlocks` 分别增加 1/2/3，证明 victim 是命令事务；仍需独立 40001 R1 |
| DLY-005 | Delivery Admin 跨版本结果 payload | P0 | 已实施，四路径 R1 | 旧实例把完整 `delivery.Delivery` 以 Go 默认 PascalCase 字段名写入 `result_payload`；UUID/摘要固定数组编码为数字数组，时间编码为 RFC3339 字符串，可空字段编码为 `null`。重放会解码完整快照并执行 `ValidStoredDelivery`，缺失身份、时间轴、激活快照或状态字段即失败关闭 | 新实例原先写入 `{command_id,snapshot}` 精简 envelope，且只读取该格式；旧账本无法被新版解码，新账本在旧版会得到零值完整对象并被拒绝。现已改为完整旧格式写入、旧完整/当前精简双读，完整格式额外恢复创建与激活快照校验 | DLY-002 只统一请求摘要，原先尚不能形成真正的跨实例幂等闭环；有效旧 payload 的 23 字段 wire 已恢复。Go decoder 对大小写、标量 `null` 和长短固定数组的宽松容错不回迁，损坏 payload 依项目不变量严格失败关闭 | 完整对象只存在 storage 防腐层，转换后仍只向 HTTP 返回安全 `DeliveryAdminSnapshot`，不得泄漏正文、收件人内部材料或命令摘要；旧完整格式识别优先于过渡 envelope | 隔离 clone 已验证新写→旧读、旧写→新读、过渡 `{command_id,snapshot}`→新读均为 200，坏完整账本→新读为安全 503；新写 payload 恰有旧版 23 个键，ID/CommandID/RequestDigest 数组长度为 16/16/32、时间为字符串且无过渡键。四条命令最终仅有四份 ledger 与四条 audit；未保存测试脚本、未修改旧仓库 |
| API-019 | Delivery Admin mutation 滚动升级请求体 | P0 | 已修复，五动作双向请求 R1 | 旧 release/retry 严格请求体只允许 `command_id/expected_fencing_version`；旧 Web reconcile 固定发送 `action/external_reference/failure_code`，未使用结果字段为 `""`。Go `DisallowUnknownFields` 会拒绝 release/retry 的任何额外键，而 reconcile 校验把未使用空串视为合法零值 | 新浏览器共用 DTO 原会把三个 `Option::None` 序列化为显式 `null`，导致新版 UI 指向旧后端时 release/retry 为 400；新 storage 原又要求 reconcile 未使用字段必须为 `None`，导致旧 UI 指向新后端时三个 reconcile action 都为 400。现已拆分 simple 请求、序列化时省略缺失字段，并在 reconcile HTTP 边界把旧未使用 `""` 规范化为缺失 | digest 与完整结果 payload 一致后，HTTP 滚动升级入口也恢复双向可用；使用字段仍严格要求非空 trimmed，未知字段继续失败关闭 | release/retry 只接受旧两个字段；reconcile 保留旧五字段合法零值语义，再建立 command/hash，不放宽其它 JSON 闭集 | 同一隔离 clone 上，以新版浏览器等价的两个字段分别请求旧 release/retry 均为 200；以旧浏览器等价的五字段和未使用空串请求新版 reconcile succeeded/failed/retry 均为 200。反向实例重放也全部 200，五命令仍只有五份 ledger/五条 audit；无测试脚本 |
| API-020 | Delivery Admin 成功 message | P2 | 已修复，三类 message R1 | 旧 release、retry、reconcile 成功 message 分别为“交付已释放”“失败交付已进入重试队列”“交付对账已完成” | HTTP 已按 action 恢复三个旧 message，不再统一返回“交付状态已更新” | `code/data/message` 的 v1 可见信封已对齐，Dioxus 自己的五语言成功 notice 不受影响 | message 只由 HTTP action mapper 决定，不读取数据库正文 | 隔离 clone 的 release、retry 及 reconcile succeeded/failed/retry 五次首次写入与跨实例重放均为 200；release/retry/reconcile 分别精确返回上述三个旧 message |
| DLY-006 | Delivery Admin 事实时间语义 | P1 | 已修复，五动作隔离 R1 | 旧五类人工动作都由 PostgreSQL `clock_timestamp()` 写 `updated_at`；reconcile succeeded/failed 还各自用 `clock_timestamp()` 写 `completed_at`，因此时间取自实际语句执行点 | delivery 五类 UPDATE 已恢复数据库 `clock_timestamp()`；settlement fulfillment 仍使用自己的事实时间 owner | 长事务或 Serializable 重试后的 delivery 时间不再来自进程预取值或事务起点；completed/updated 的独立语句时钟关系也恢复 | delivery 行按旧 SQL 取数据库墙钟；settlement fulfillment 自己的事实时间继续由其 owner 管理，不借本项回退跨表不变量 | 隔离 clone 五动作均成功：release/retry/reconcile-retry 的 completed 为空且 updated 为执行点；reconcile succeeded 的 completed=updated，failed 的两个独立 `clock_timestamp()` 相差 1µs 且顺序合法；ledger created_at 均先于最终 updated，跨实例重放未改变这些时间 |
| DLY-007 | Delivery Admin 与 settlement fulfillment 联动 | P0 | 有意扩展，保留并待终态 R1 | 旧 Admin delivery 命令只更新 delivery、ledger、audit，没有与 settlement fulfillment 状态建立原子联动 | 新 settlement 数据模型把 delivery 作为履约证明载体；retry/reconcile 同事务重置或完成 `settlement_obligation_fulfillments`，若 settlement 关联存在但状态不匹配则 stale 失败关闭 | 表面删除该联动可更像旧 SQL，却会让 delivery 与履约权威事实分裂，违反当前 settlement owner、审计和原子写入不变量 | 作为旧版缺少新 settlement 模型联动的缺陷修复保留；非 settlement delivery 仍允许零关联行，settlement delivery 必须恰更新一行 | `commands.rs` 与 storage README 已明确 owner；后续需 retry/reconcile 三终态同时核对 delivery/fulfillment/ledger/audit，不能从无关联合成行外推 |
| AUI-024 | Delivery dialog 顺序、busy 与反馈文案 | P2 | 已实施，待 slow/error R1 | 旧 release/retry 二次框按钮为 Confirm→Cancel，Cancel autofocus；busy 只禁用按钮且确认文案不变。行内 reconcile 为 Cancel→Confirm，busy 时 select/input 仍可编辑；标题含短 ID，label/control 分离。成功提示按 released/retried/done 分支选择 | 两类框现各自恢复旧按钮顺序、autofocus、静态文案和标题/描述；reconcile 字段在 busy 时保持可编辑且不再添加旧版没有的 maxlength，只有按钮禁用。行按钮同样保留原文案。成功反馈按 Release/Retry/Reconcile 分类 | 已知键盘顺序、慢请求可编辑性、按钮文本和 generic success 差异关闭 | 继续通过统一安全错误组件输出失败；五语言为三个成功分支提供真实译文。旧 runtime 把 singular delivery key 泄漏为简中，且遗漏 `delivery.retried` 后回退到 reconcile done，这些翻译缺陷不回迁 | rustfmt、i18n、Admin WASM check 与目标 Clippy 已通过；待 release/retry/reconcile 三类 slow 201/400/503，核对 DOM、Escape、action-specific status 与 opener |
| AUI-025 | Settlement 加载/错误页面外壳 | P2 | 已实施，待 slow/error R1 | 旧页面始终先渲染标题、拍卖选择器和分页；auction/report/recovery 任一 pending 在其下显示共享 loading，任一 error 显示共享 alert，没有 Retry 按钮；已成功 report 可与 recovery loading/error 同时存在 | 页面现常驻 `admin-auction-page`、`settlement-title` 和旧 picker；三资源共享一个 loading/alert，读取 Retry 已移除，auction pager 位于共享状态后；report 成功时 recovery 标题即使 pending/error 仍保留 | 已知壳层卸载、双状态区、额外 Retry 和 recovery 标题消失差异已从源码移除；组合请求时序仍待运行态验证 | 保持三资源独立 owner，只共享页面状态投影；成功 report 不因 recovery 慢/错而卸载，错误不展示服务端正文 | rustfmt、i18n、Admin WASM check、native `-D warnings` Clippy 与差异空白检查通过；仍需分别记录 auctions/report/recovery slow/503、组合成功/失败、选择值和请求数 |
| AUI-026 | Settlement hold 百分比与金额对齐 | P1 | 已实施，待同数据 R1 | 旧 `holdShare` 使用 `Math.round((tax*10000)/finalPrice)/100`，即四舍五入到两位小数；金额单元格沿用 `admin-auction-num` 左对齐，零 final price 显示 `—` | helper 已把非负金额提升到 `i128`，用加半分母的整数算法恢复旧正数四舍五入；负税额/非正价格失败关闭为空值。金额 class 只保留 tabular nums，继承表格左对齐 | 先前向下截断和强制右对齐路径已消除，后端金额事实不变 | 保持整数计算以避免浮点/溢出；异常事实不伪装成 0%，零价格继续显示 `—` | rustfmt、i18n、Admin WASM check 与目标 Clippy 已通过；仍需只读报告选取可进位、不进位、整百分比和零价格四类值，核对文本与 11 列几何；不编写测试用例 |
| AUI-027 | Recovery 成功刷新范围与行 busy | P1 | 已实施，待成功/409 R1 | 旧 confirm 成功同时失效 recovery、settlement report 与 auction list；mutation pending 时所有 recovery 行动作按钮均 disabled | 成功分支现推进三个资源共用的 `reload_generation`，409 仍只推进独立 recovery generation；页面 `action_busy` 已逐层传到 table/row 并禁用所有 recovery 动作 | 已关闭成功后跨 owner 保留旧快照和提交期间替换 dialog 上下文的静态路径 | 成功刷新 recovery、settlement 和 auctions；409 只刷新 recovery，保持 AUI-010 边界。命令 ID 仍只在成功后删除 | rustfmt、i18n、Admin WASM check 与目标 Clippy 已通过；仍需成功后恰三类 GET、409 后仅 recovery GET，并核对 slow pending、focus 和 notice |
| API-015 | Recovery 浏览器响应严格闭集 | P2 | 已实施，待真实报告 R1 | 旧 `recoveryApi.ts` 对 report/recovery/action/confirmation 每层使用 `only(...)`；还校验小写规范 UUID、可解析时间、stage/command/action/error 枚举、正数版本/编号、提醒上下界、report/recovery auction 一致性、recovery/action ID 唯一及 nullable 关联字段成对出现 | 四层 DTO 现拒绝未知字段且把 nullable 字段冻结为“键必有、值可 null”；浏览器解码边界已补规范小写 v1-v5 UUID、时间、枚举、计数、唯一性、auction/关联字段和 confirmation 关系校验，并显式允许 `buyer_blacklist`。API-018 又把内部错误码收敛到旧公开闭集，未知值失败关闭 | 结构缺失、扩张、语义漂移和内部错误码泄漏的静态路径均已关闭；合法报告不再因 owner 私有 code 被浏览器整页拒绝 | 保持严格闭集与明确 action 扩展；Worker/storage→HTTP 只输出冻结 `RecoveryErrorCode`，不通过放宽浏览器枚举掩盖泄漏 | Contracts/PostgreSQL Storage/HTTP/Admin-WASM check、目标 `-D warnings` Clippy、i18n 与 diff 已通过；仍需含 nullable 组合、buyer blacklist、十一种公开错误及未知内部码的真实/受控报告 R1 |
| AUI-028 | Recovery `buyer_blacklist` action | P2 | 有意扩展，保留并补文档 | 旧后端 recovery worker 已有买家封禁副作用，但旧浏览器 `recoveryActionKinds` 遗漏 `buyer_blacklist`，严格 decoder 遇到该合法 action 会拒绝整份 report | 新 DTO/UI/五语言显式支持 `buyer_blacklist` 并展示其状态 | 与旧浏览器闭集不完全一致，但回退会重现“后端合法事实导致整页不可读”的旧缺陷，并隐藏安全相关动作 | 保留该 action，冻结为旧后端能力的前端缺陷修复；主表仍只显示安全 kind/state，不暴露封禁内部信息 | 待含 buyer_blacklist 的真实/脱敏 report 在五语言下可读，未知其它 kind 继续失败关闭 |
| AUI-029 | Delivery/Settlement 页面专属响应式样式 | P2 | 已实施，待浏览器 R1 | 旧 Delivery 筛选 select 在窄屏仍固定 8.875rem；Settlement 卡片 gap 为 `space-4`、有底部 `space-5`，卡片最小宽度和 padding 使用旧 auction card 规则；表格始终横向滚动 | 已移除 Delivery 的 48rem 全宽覆写；Settlement cards 已恢复 `space-4` gap、`space-5` 底距、旧最小宽度、横向 padding、圆角和非拉伸布局 | 已知 CSS owner 的宽度、换行和后续 recovery 起点差异关闭；动态文本仍可能改变实际矩形 | 保持页面专属旧 CSS；全局原生 table 横向滚动继续由 AUI-016 拥有，不重新引入卡片化规则 | old/new CSS 逐项静态对照完成，rustfmt、i18n、Admin WASM check 与目标 Clippy 已随本批重跑通过；仍需同一 populated Delivery/Settlement 在 390×844 与 1440×1000 核对 select、卡片矩形、table overflow 和 recovery 起点 |
| I18N-004 | Delivery/Settlement 五语言扩展边界 | P2 | 登录页五语言运行态 R1；认证页待证 | 旧仓库虽有 en/zh-CN/zh-TW/ja/ko 资源，但旧 Admin 自定义 manifest/回退使部分运行态文案只落到简中或英文 | 新版按项目合同完整注册五种语言，新增 prepared、buyer_blacklist 及错误文案都有五份 FTL | 为像素复刻旧泄漏而删语言或回退文案会违反本仓库明确 i18n 合同 | 保持 `en/zh-CN/zh-TW/ja/ko`、`hv.lang` 与各自翻译；把由旧翻译缺陷导致的文本/高度差逐项标记为有意修复 | payload `d29e1521dacfa69282eb385730c00a03774279116c80f4f8d222d06e0e07c379` 下，登录页逐一写入五个合法 `hv.lang` 后均保持原值，`<html lang>` 分别同步为 `en/zh-CN/zh-TW/ja/ko`，标题、ready、用户名、密码和登录按钮均显示对应语言；认证后 Delivery/Settlement 长文案、选择器持久化和 390px 几何仍待证 |
| AUI-030 | Audit 加载/错误外壳 | P2 | 已实施，待 slow/error R1 | 旧 Audit 始终保留标题和九个筛选器；loading/error 只在筛选器后显示，错误没有独立 Retry | 页面现始终挂载 `admin-audit-page`、标题与九筛选器，resource pending/error 只在其后输出旧 `admin-shell__status`/`admin-shell__inline-error`；独立错误 panel 和 Retry 已移除 | 已知筛选上下文卸载、额外恢复入口和 DOM 层级差异关闭；慢请求期间输入触发的取消/重读次数仍需运行态记录 | 保持标题/筛选器为页面 owner，只在结果区切换 loading/error/data；错误继续由安全本地化 mapper 输出，不展示服务端正文 | rustfmt、i18n、Admin WASM check、native `-D warnings` Clippy 与差异空白检查通过；待 slow/503 下九筛选值、请求参数及五语言 390/1440px R1 |
| AUI-031 | Audit 空态与成功表格投影 | P2 | 已实施，待 populated/empty R1 | 旧先渲染空表格再显示空态；成功页无说明/total H2，actor/entity 形如 `id(type)`，actor type/source/result 显示原始 token，result 无 badge，td 无 `data-label`，空 request ID 为字面 `—` | 页面说明、total H2、空态顺序、括号格式、普通 result 文本和无 `data-label` 的八列表格均已恢复；五语言 actor/source/result、安全 metadata fallback 和本地化空值按项目合同保留 | 已知结构、额外 badge、括号空格和响应式属性差异已关闭；有意保留的五语言/安全投影不回迁旧原始 token 与不安全 metadata | 保持旧表格 DOM 和视觉权重，同时以五语言安全枚举替代旧英文 token；未知 metadata 只显示安全 fallback | Admin-WASM/native check、`-D warnings` Clippy、i18n 与 diff 已通过；待 populated/empty、8 列、metadata 截断、长 actor/entity 和 390/1440px 行高 R1 |
| AUI-032 | Realtime 筛选 value 与 Replay busy | P2 | 已实施，待动作 R1 | 旧状态 select 的 DOM value 为 `dead_letter/completed/pending/retry`，请求层把前两项映射为 `resolved=false/true`；Replay pending 时只禁用 Confirm/Cancel，schema input 与 payload textarea仍可编辑，Confirm 文案保持静态 | select 已恢复四个旧 DOM token，API 查询边界把前两项映射为 resolved false/true；提交瞬间快照 schema/payload/signature，pending 期间输入可编辑但不改变已发请求或稳定 command ID，只禁用按钮并保持静态 Confirm | DOM value、请求映射、慢请求可编辑性和按钮宽度的静态差异已关闭 | 保持旧交互，同时用提交快照保护幂等签名；失败保留 notice、输入和 command ID，成功刷新仍由页面 owner 执行 | Admin-WASM/native check、`-D warnings` Clippy、i18n 与 diff 已通过；待四筛选的 select.value/GET query、slow replay 编辑/Escape、400/409/503 后 command ID 与输入保留 R1 |
| AUI-033 | Delivery 加载/错误页面外壳 | P2 | 已实施，待 slow/error R1 | 旧 Delivery 始终保留标题和 status 筛选器；pending/error 只在筛选器后显示普通 status/alert，错误没有独立 Retry | 页面现始终挂载 `admin-delivery-page`、标题和 `admin-delivery-filter`；pending/error 只在结果区输出旧 `admin-delivery-status`/`admin-delivery-error`，独立 panel 与 Retry 已移除，筛选器恢复旧 label/select DOM 和固定宽度 | 已知筛选上下文卸载、额外恢复入口、DOM 层级和页面 class 差异关闭；慢请求时筛选变化的取消/重读次数仍需运行态记录 | 保持标题/筛选器为页面 owner，只在结果区切换 loading/error/data；错误继续使用安全本地化 mapper，不展示服务端正文 | rustfmt、i18n、Admin WASM check、native `-D warnings` Clippy 与差异空白检查通过；待 slow/503 下筛选值、请求参数及五语言 390/1440px R1 |
| AUI-034 | Settlement 成功报告额外标题 | P2 | 已实施，待 populated R1 | 旧成功 report 直接从 lot table 开始，资金池标题是该区域第一个 H2 | 额外 `section.admin-settlement__report` 与动态 report H2 已删除，成功区直接从旧 lot table 开始 | 可见文本、标题层级和 table 起点间距的静态扩张已关闭 | 保留旧资金池和 recovery 两个 H2，不新增报告层标题 | rustfmt、i18n、Admin WASM check、native Clippy 与 diff 通过；待同报告核对 H2 数量/顺序、table 首节点和五语言高度 |
| AUI-035 | Settlement summary DOM 与时间语义 | P2 | 已实施，待 populated R1 | 旧卡片为 `admin-auction-cards > admin-auction-card > value/label`；结算时间是 `p.admin-auction-status` 中的文本前缀加 `<time datetime>`，英文无冒号 | 五张 summary 已恢复旧 class/层级；结算时间恢复旧五语言前缀、字面空格及带 `datetime` 的 `time` 元素 | CSS selector、字体 owner、机器可读时间和可见标点的静态差异已关闭 | 保持旧 card owner 与时间语义，不用 Fluent 插值吞掉 `time` 元素 | 静态门禁通过；待五卡顺序、value/label、time datetime/文本和 390/1440px 矩形 R1 |
| AUI-036 | Settlement lot table DOM | P2 | 已实施，待 populated R1 | 旧 wrapper/table 为 `admin-shell__table-wrap` 与 `admin-shell__table admin-auction-table`，th 有 `scope=col`，行/td 无扩张属性，数字 class 为 `admin-auction-num` | wrapper/table/caption、11 个 `scope=col`、普通 tr/td 和 number class 已恢复；BEM row、amount 与 `data-label` 已移除 | 旧 CSS/定位器和窄屏原生 table 语义的静态差异已关闭 | 保持原生横向表格，不重新引入卡片式 `data-label` | 静态门禁通过；待 11 个 th/td、caption、横向 overflow 和数值对齐 R1 |
| AUI-037 | Settlement recovery 标题与表头文案 | P2 | 已实施，待五语言 R1 | 旧标题/列名分别为 Settlement recovery、Recovery stage、Recovery actions、Actions，并在五语言资源中有对应译文 | recovery 标题与三个扩张表头均已恢复旧五语言文案 | 表格可见闭集和由长文案引起的静态列宽扩张已关闭 | 不删除 `buyer_blacklist` 行事实扩展，只恢复旧列标题 | i18n 与编译门禁通过；待五语言 7 列标题、390px 横向宽度和表头/行语义 R1 |
| AUI-038 | Settlement recovery stage badge | P3 | 已实施，待 populated R1 | 旧 stage 为普通本地化文本 | stage 彩色圆角 badge/class 已移除，恢复普通本地化文本 | 额外颜色语义、视觉权重和 badge 行高已从源码移除 | stage 只表达冻结文本状态，不增加装饰语义 | 静态门禁通过；待 populated 各 stage 的 DOM、颜色和行高 R1 |
| AUI-039 | Settlement recovery 行与动作布局 | P2 | 已实施，待 populated/slow R1 | 旧世界信息为 `<br>` + `span.admin-auction-status`；动作 wrapper/button 为 `admin-auction-actions`/`admin-auction-recovery-action`，compact gap，窄屏纵向且按钮有最小触控高度 | 世界、普通 row/td、action wrapper/button 及 48rem 窄屏规则已恢复；BEM row/commands、`small` 与 `data-label` 已移除，busy 全行动作禁用继续保留 | 内容换行、动作按钮高度和横向表格几何的静态差异已关闭 | 保持旧布局，同时维持 AUI-027 的跨行动作并发保护 | 静态门禁通过；待长世界/动作、slow busy、390/1440px 行高与按钮矩形 R1 |
| AUI-040 | Settlement picker 成功态 | P2 | 已实施，待 populated R1 | 旧 filter/select 有固定 class/id，label 为旧五语言 Auction/场次，option 使用原始 world/status token，placeholder 带旧省略号 | filter、id/class、语言宽度、旧 label/prompt、五语言省略号 placeholder 与原始 option token 均已恢复 | 原 panel 边框、本地化 token 扩张和 placeholder 标点差异已关闭 | auction 仍只请求 settled，不把筛选事实改成可见扩张 | 静态门禁通过；待 options populated、select.value、五语言宽度和 390/1440px picker 矩形 R1 |
| AUI-041 | Settlement recovery 成功反馈 | P2 | 已实施，待成功 R1 | 旧成功为 `admin-shell__inline-ok[role=status]`，文案 Recovery decision confirmed. 及五语言对应 | class/role 与五语言旧即时确认文案均已恢复；三资源刷新仍按 AUI-027 保留 | 可见结果措辞差异已关闭，不回退更完整的成功刷新行为 | 成功 notice 位于旧顺序并只表达已确认，不泄漏服务端正文 | 静态门禁通过；待成功 200 后文案、DOM 顺序、三 GET 与 dialog opener R1 |
| AUI-042 | Settlement recovery dialog busy/DOM | P2 | 已实施，待 slow/error R1 | 旧 form/button 使用 `admin-auction-recovery-confirm`/`admin-auction-recovery-action`，textarea 在 pending 时仍可编辑，Confirm 文案保持不变；Escape/Cancel 关闭框但不清 mutation error | 旧 form/button/ID 已恢复；busy 只禁用按钮，textarea 可编辑且 Confirm 文案静态；Escape 不再清 feedback。提交瞬间仍快照 reason 与 command signature | 慢请求可编辑性、按钮宽度、错误生命周期和 DOM 定位器的静态差异已关闭，幂等签名不受 pending 期间编辑影响 | 保持提交瞬间快照，失败重试继续复用稳定 `command_id` | 静态门禁通过；待 slow 200/400/503 下编辑、Escape、按钮文案/class、同 ID 重试与 opener R1 |
| AUI-050 | Delivery populated 默认排序 | P1 | 已验证，双页 populated R1 | 旧 Admin Delivery 查询固定按 `created_at DESC, id DESC`，最新交付优先出现在第一页 | 新 storage 已从升序恢复为 `created_at DESC, id DESC` | 同一分页数据的首尾行、动作定位和翻页结果已恢复旧顺序 | 保持旧降序及稳定 ID 次序，不改变 status 筛选和 page size 10 | 隔离 clone 写入 12 条受控记录后，新旧第一页 ID 同为 `012..003`、第二页同为 `002,001`；`011/012` 使用相同 `created_at`，两侧均由较大 ID `012` 排在前。Storage PostgreSQL/SQLite feature 静态编译通过 |
| AUI-051 | Delivery 浏览器响应语义边界 | P0 | 合法 list/五动作 R1；坏响应待证 | 旧 Delivery API 先校验 envelope，再由路由 decoder 校验分页、delivery 行和 mutation result 的必需已知字段及 UUID、时间、状态、计数、origin 类型；旧 decoder 不拒绝未知字段，也没有 lease 时间线、状态/结果组合和动作语义的跨字段校验 | 新浏览器原先把响应直接交给 serde DTO，连旧版已有的 route-specific 已知字段语义校验也没有形成；现已增加 Delivery 专属严格 DTO 与 validator，在覆盖旧校验的基础上进一步拒绝未知字段、非法时间线、状态/结果组合及动作不匹配 | 旧版已有的失败关闭能力已恢复；新增的闭集和跨字段约束属于当前不变量下的有意加固，避免坏投影被页面当作权威状态继续展示或操作 | 在 Admin 浏览器 Delivery API 边界集中校验，不把规则散到组件；保留旧合法 wire，对未知扩张和自相矛盾响应按安全失败处理 | 隔离 clone 上合法 populated list 及 release、retry、reconcile succeeded/failed/retry 均由新版页面/API 成功解码，五个结果又可被另一版本重放；原生/WASM 定向编译已通过。未知字段、坏 UUID/时间/状态/计数/动作结果的受控响应仍待逐项核对，不新增测试脚本 |
| AUI-052 | Delivery `origin_kind` 扩展 | P2 | 有意扩展，投影已收口，待五语言 R1 | 旧浏览器只认识其冻结时期的 Delivery origin 闭集并按既有类型映射展示 | 新系统增加 `settlement_recovery_action`、`operations_alert` 等由新 owner 产生的合法 origin；页面现已把两类映射到五语言安全标签，不再直接显示内部 token | 直接删除新 kind 会隐藏当前履约/告警事实；安全标签保留事实同时维持旧八列表格信息层级 | 保留新合法 origin 作为旧系统能力缺口修复；未知未来值由 AUI-051 的专属 decoder 失败关闭，不扩张列 | 五种 FTL 资源与 i18n 门禁通过；待两类真实/脱敏行核对 Type 文案、tooltip 与八列几何，未知 kind 应拒绝整份响应 |
| AUI-053 | Delivery stale 409 残留 alert | P1 | 有意修复旧缺陷，新版 stale R1 | 旧 React 在 `onError` 内关闭 dialog 并失效列表，但 TanStack 随后仍把 mutation 置为 error；页面无 stale 过滤，因此错误 alert 会残留并恢复 opener 焦点 | 新页面关闭 dialog、刷新列表并清 stale feedback，不把已由权威重读收口的 stale 再作为普通错误 alert 展示 | 先前 AUI-009/AUI-015 把“无 alert”误写为旧实现事实；源码完整时序证明那是目标语义，不是旧运行事实 | 维持新版无残留 alert，明确记录为旧异步状态生命周期缺陷修复；普通 400/503 仍留框并显示错误，不能一并吞掉 | 隔离浏览器以 dialog 读取的 fence=5 打开表单后将数据库受控推进到 fence=6，提交得到 409，随后恰一次 Delivery GET 200；dialog 关闭、无 alert、opener 恢复。普通空白字段 400 仍留框并显示错误。旧版残留 alert 由源码时序确认，不伪造运行样本 |
| AUI-054 | Delivery reconcile 空白字段提交 | P2 | 已验证，空白 400 R1 | 旧表单的 HTML `required` 接受全空格，提交层 trim 后仍发空字符串，由服务端返回稳定 400 并在原 dialog 显示错误 | 新页面已移除 trim 后静默 return，恢复提交到服务端权威校验的路径 | 操作员动作不再表现为无响应，请求数、错误生命周期和稳定 command ID 可按旧版对照 | 保留原 dialog、草稿和同一 `command_id`，由服务端拒绝空白事实 | 旧版 succeeded 全空格与新版 succeeded/failed 全空格均恰发一次 mutation 并得到 400，dialog、输入和安全错误提示保留；同一失败输入重复提交继续复用各自原 `command_id`。新版按旧兼容语义省略无关字段，旧版仍发送两个无关空字符串，二者都由服务端接受为同一动作边界 |
| AUI-055 | Delivery reconcile 草稿取消语义 | P2 | 有意修复旧缺陷，待浏览器 R1 | 旧行内 reconcile 草稿由页面级 state 保存，Cancel 后再次打开同一交付仍保留 action、external reference 或 failure code | reconcile draft 仍由 Delivery 页面 owner 并绑定 delivery identity，但 Cancel/Escape 现在同时清除 dialog、草稿和命令反馈；普通提交失败继续保留输入与稳定命令 ID，成功/stale 仍按权威刷新清除 | 旧版 Cancel 实际没有放弃输入，重开后可能误提交上一次明确取消的外部事实；新版把取消解释为显式放弃，同时不牺牲失败重试 | Cancel/Escape 必须清草稿与命令状态；普通 400/503 留框并保留输入/ID，成功与 stale 清空 | Admin native/WASM 定向静态检查已通过；仍需三种 action 的取消重开、普通失败重试、成功/stale 和 opener 焦点浏览器 R1 |
| AUI-056 | Delivery reconcile dialog 表格占位 DOM | P3 | 已验证，populated dialog R1 | 旧 reconcile dialog 从当前行内部渲染，表格中保留用于挂载的 `tr/td[colspan=8]` 层级 | reconcile dialog 现紧随匹配主行渲染为合法 `tbody > tr.admin-delivery-dialog-row > td[colspan=8]`；release/retry 二次确认仍保持页面级 | populated 表格的 DOM 子节点和 CSS owner 已恢复，未引入非法 `tr` 嵌套 | 保持 fixed backdrop、共用 focus trap、busy/Escape/opener 语义；行离开当前分页时 dialog 随旧 row owner 消失 | 新旧 populated 页均为 `tbody > tr.admin-delivery-dialog-row > td[colspan=8]`，占位行/单元格约 0.5px；Cancel 自动聚焦、Tab 到 Confirm、继续 Tab 回到 select、Shift+Tab 反向闭环。Escape 移除占位行并恢复 opener；Admin 原生/WASM 与 diff 静态检查通过 |
| AUI-057 | Delivery 表头 class 与隐藏收件人标题 | P3 | 已验证，长文本 390/1440 R1 | 旧八列表头带七个列专属 class，actions 列无 class；收件人单元包含 `h3.admin-visually-hidden` | 七个旧列 class、actions 无 class 及 recipient 隐藏标题/节点顺序均已恢复 | 静态列宽 owner、长收件人截断和辅助技术名称的源码差异已关闭 | 不新增可见列或可见文案，继续使用五语言资源 | 新旧均有八列表头、七个专属 class、actions 无 class，以及收件人单元首个 `h3.admin-visually-hidden`。1440px table/wrapper 均为 1200px；390px wrapper 均为 135px 且横向滚动，长 recipient 行高均为 69.78125px。普通 status 行因项目约定的新五语言翻译与旧 raw token 产生有意宽高差异，归 I18N-004 |
| AUI-058 | Delivery 长文本截断/tooltip DOM | P2 | 键盘/滚动/视口 R1；鼠标/卸载待证 | 旧 `TruncatedText` 的 trigger 只使用 `ui-truncated-text ui-truncated-text--{size}`，按真实裁剪或 display 缩写设置 `data-truncated/tabIndex`；旧组件没有 `title/data-full-value`，tooltip 以 `ui-truncated-text__tooltip` portal 到 `document.body`，持续监听 resize 测量，并在 tooltip 打开时以 capture scroll/resize 关闭 | 共享组件已增加隔离的 legacy DOM 模式，Delivery 的 ID/type/recipient/failure 四处均启用旧 class、旧初始测量、无额外属性和 body portal；scroll capture、永久 resize measure、`ResizeObserver`、tooltip dismiss 与卸载均有对称清理。默认 `shared-*` 模式继续服务其它已迁移页面 | 已知 class、属性、portal owner、嵌套滚动和 resize 生命周期差异已关闭；手工 portal 在快速路由切换/连续重挂载下的最终 DOM 时序仍需浏览器证据 | 兼容逻辑只由 Delivery 显式启用，不全局替换其它页面；legacy CSS 与旧 50 行组件样式逐项对应，不增加页面专属 class | 新旧四类 trigger 均只有旧 class 且无 `title/data-full-value`；长 recipient 在 1440/390px 均为 `data-truncated=true/tabindex=0`。键盘 focus 后 tooltip 以匹配 `aria-describedby` 的动态 ID portal 到 body，Escape 与 capture scroll/resize 均关闭并清 aria，resize 后继续正确测量。鼠标 hover、blur 及快速路由卸载仍待补证；Shared/Admin 原生/WASM、Clippy、rustfmt 和 diff 已通过 |
| AUI-059 | Admin 登录成功后的 layout 路由重绘 | P0 | 已修复，隔离登录 R1 | 旧 Admin 从带 `return` 的登录页成功认证后使用 replace 进入目标路由，并立即渲染已认证外壳及目标页面 | 新 Dioxus 的 `/login` 与认证页共用同一 layout，认证资源成功后虽已 GET 200，layout 却通过一次性 `window.location` 判定登录路径；Router replace 不重挂 layout，导致 DOM 永久停在空重定向占位且不发目标页面请求，手工 reload 后才恢复 | 登录接口、Cookie 和目标路由都正常时仍出现空白页，是阻断全部 Admin WebUI 的真实 P0，不可归因为隔离库 ready=503 | `AdminShell` 改为订阅 `use_route::<Route>()` 并由当前 route variant 决定认证分支；保留安全 return 解析、replace 历史语义和会话 resource owner，不用整页跳转掩盖状态错误 | 隔离同库旧版成功后直接进入 Delivery；修复前新版 POST/GET session 均 200、DOM 仅余启动占位且无 Delivery GET，reload 后才正常。最终 embedded 在全新隔离浏览器 context 中从 `/deliveries` 跳到 `/login?return=%2Fdeliveries`，一次 POST 200、一次 session GET 200 后无需 reload 即渲染 shell/Delivery，并恰发一次 Delivery GET 200；Admin 原生/WASM check、`-D warnings` Clippy、rustfmt 与 diff 均通过 |
| API-016 | Audit numeric/keyset 响应信封 | P1 | 已验证，三模式认证 R1 | 旧 numeric 固定输出 `records/page/page_size/total/has_more/next_cursor:null/next_cursor_token:""`；旧 keyset 固定输出 `records/page_size/has_more/next_cursor:null/next_cursor_token:""`，没有 `page/total`；record 的空 `request_id` 显式为 `null` | DTO 现始终序列化 `next_cursor` 与 `next_cursor_token`，record 的 `request_id` 始终出现；`page/total` 仅 numeric 输出，response `cursor` 已移除。opaque token 的内部编解码和 keyset 排序继续保留 | 键集合随 `None` 漂移、请求 cursor 回显和 null→缺失差异已从静态 wire mapper 消除 | 保持 numeric/keyset 互斥字段形状，不回退 opaque cursor 内部能力 | 新旧认证实例已逐键核对 numeric `page=1&page_size=2`、原始 `before_time/before_id` 第二页和各自 opaque `cursor` 第二页：numeric 均只含 `records/page/page_size/total/has_more/next_cursor/next_cursor_token`，两种 keyset 均只含 `records/page_size/has_more/next_cursor/next_cursor_token`；record 键集合一致且空 `request_id` 显式为 `null`。两侧 opaque token 与各自原始 pair 取得相同第二页记录；隔离数据库的 ID、总数及 `Z`/offset 时间文本差异不作为信封等价证据 |
| API-017 | Realtime Replay 错误状态/code | P1 | 已实施，invalid 局部 R1；其余五类待证 | 旧 invalid→400 `invalid_replay`，already resolved/业务冲突→409 `conflict`，replay mismatch/owner drift/order 问题→500 `internal_error`，依赖失败→503 `temporarily_unavailable` | replay 路由现使用独立 HTTP 防腐映射：invalid/path/body→400、resolved/command conflict→409、mismatch→500、capability/repository/其它依赖→503；JSON 语法错误也通过 route-specific strict policy 输出 `invalid_replay`，413/415 保持旧稳定 code。读取路由继续使用自己的新 typed mapper | v1 wire 的 status/code 差异已关闭，内部 typed error、命令账本、事务 fence、command ID 与安全分类均未回退 | 只在 replay HTTP 边界投影旧别名，不匹配错误正文且不泄漏 owner 信息；服务端仍保留细分类型供事务和日志决策 | HTTP check、`-D warnings` Clippy、i18n 与 diff 已通过；最新 8091 临时认证会话的 `schema_version=0` 实际返回 400 `invalid_replay`、`data:null/request_id:null`，Cookie 临时目录已删除。仍待 resolved、mismatch、command conflict、capability、storage 五类 envelope，页面生命周期另归 AUI-032 |
| API-018 | Recovery 内部错误码公开映射 | P1 | 已实施，待真实错误报告 R1 | 旧浏览器只接受冻结 `RecoveryErrorCode`：`invalid_recovery_fact`、recipient/delivery/evidence/outcome/relist 等带 `recovery_` 前缀的 11 个公开码 | HTTP report mapper 现通过 `PublicRecoveryErrorCode` 把已知 evidence、delivery reference/发送、recipient、activation、credential/session/blacklist、protocol 等 owner 私有码收敛到最窄的十一种公开语义；已是公开码的值原样通过，未知值使整份映射失败并返回安全 503 `storage_unavailable` | 合法内部事实不再把私有字符串冻结进 v1 或触发严格浏览器整页拒绝；无法安全分类的未来值也不会泄漏正文 | 映射只位于 storage/Worker→HTTP wire 防腐层，不修改数据库证据；未知值失败关闭，不通过扩张浏览器枚举掩盖协议漂移 | HTTP all-target check、`-D warnings` Clippy 和全批静态门禁通过；待受控报告证明每类来源只输出十一种公开码，未知值为无正文泄漏的安全 503 |
| EVD-006 | Audit/Outbox 运行态证据 | P2 | 证据缺失 | 旧 Audit/Outbox 的 populated/empty、单侧失败、Replay 及两 viewport | 新主要路由、筛选字段、分页默认值、failure/lag 列顺序和五语言 key 静态齐全 | 焦点恢复、409/503、真实 CSS 计算布局、长 ID/metadata、数据库排序/分页仍未取得同源浏览器证据。并行审计最初把 Public 的 48rem 卡片 CSS误外推到 Admin；复核 Admin bundle/CSS 后已排除该结论，当前仍按横向表格处理 | 使用一次性受控响应和已授权运行实例逐状态记录，不提交浏览器测试脚本；先协议和生命周期，再视觉 | 五语言、390/1440px、empty/populated/slow/error、Replay 400/409/503、Tab/Escape/opener、真实数据库首尾行均有证据后才关闭 |
| EVD-007 | Delivery Admin PostgreSQL 并发重放证据 | P0 | 证据缺失 | 旧 Serializable 命令 runner、三次重试和唯一冲突后的 ledger 结果恢复 | 新 typed ledger loader、行锁后二次读取与唯一冲突恢复路径 | 静态修复不能证明并发时序、失败事务回滚和审计唯一性 | 在隔离 PostgreSQL 对照同 ID 同 payload、异 payload、异 delivery、过期 fence/status；读取最终 delivery、settlement、command ledger 与 audit | 所有同摘要调用返回同一保存结果；异摘要/异 delivery 稳定 typed conflict；失败事务不留下业务、ledger 或 audit 副作用；不提交测试脚本 |

| API-011 | Public 代理转换响应闭集 | P0 | 服务端 nullable wire 已修复并认证 R1；浏览器/错误闭集见 API-021/API-025 | 旧 `publichttp/account.go::ownedProxyTransition` 的 `old_maximum_amount/old_mode/old_status/reason` 均无 `omitempty`，created 转换固定显式写为 `null`；旧 Web `proxy-bid.ts::isProxyTransition` 又通过 `only` 把 11 个转换字段全部列为必填，并要求 created 的四项恰为 `null` | 新 `PublicOwnedProxyTransitionResponse` 曾对上述四个 `Option` 使用 `skip_serializing_if`；同源 fast 代理创建虽已在数据库成功提交并可精确重放，HTTP 响应中的 created 转换却只含 7 个非空字段。该服务端 wire 已修复，但新版浏览器自身的严格 decoder 和完整业务错误闭集仍未恢复 | 命令结果本身为 200，但旧版 Web 严格解码器会把新版响应拒绝为无效协议；账户代理读取复用同一 DTO，因而刷新账户页也无法绕过。顶层 active `terminal_reason` 与无 bid 的命令响应在旧版确为可省略字段，不属于此差异 | 保留四个 nullable 字段显式 `null` 的服务端修复；浏览器响应语义与竞价/代理错误分类分别由 API-021/API-025 收口，不把局部 wire R1 外推为整条 Public 命令闭合 | Contracts/HTTP/Public WASM 与格式静态检查均通过；同源 fast 创建及降低的首次/重放均为 200，created 四项显式为 `null`，lowered 的 old 三项有值且 `reason:null`。剩余严格浏览器和错误码证据见后续条目 |
| API-012 | Public lot 投影响应闭集 | P0 | 顶层 wire 已修复并 fresh live R1；语义/浏览器边界见 API-021/API-022/API-023/API-026/PUI-022 | 旧 `/api/v1/auctions/{auction_id}/lots` 与 `/api/v1/lots/{lot_id}` 顶层只含 `auction_id/auction_status/projection_kind/presentation_mode/bidding_open/opens_at/preview_revision`、可选 `final_restriction` 及列表/详情分支；旧 Web `listKeys/detailKeys` 使用严格 `only` | 同源 fresh live lot 抓包曾显示新版两个端点额外写出 `world/number/scheduled_ends_at`，该扩张已移除；后续审计仍确认浏览器 decoder、stored projection 语义、nil UUID、preview 可选 category 和页面错误/空态未完全对齐 | 顶层键集合的局部 R1 不能证明非法持久化数据失败关闭、客户端严格解码和错误展示等完整 lot 契约 | 保留已完成的顶层旧 wire；其余责任拆到 API-021/API-022/API-023/API-026/PUI-022，逐层修复而不重新扩张 v1 | Contracts/HTTP/Public WASM、Web bundle、release build 与格式检查通过；重建后列表/详情顶层键集合相同。其余证据按后续条目收集，不再把 fresh live 单样本外推为完整闭集 |
| API-021 | Public 浏览器路由专属响应 decoder | P0 | 残差已实施，待分路由 R1 | 旧共享 client 对成功和非 2xx 都先校验 envelope，再由每条路由的 `only(...)` decoder 检查 lot、代理、竞价等字段闭集及 UUID、金额、时间、状态、projection 和分支关系；logout 还要求成功 `data === null`。旧 bid decoder 只校验响应自身 lot UUID，不绑定请求 path；账户 `fact_index` 接受 JS safe integer；装备 URL 使用 WHATWG `host` 边界；封禁 `public_reason` 要求唯一键且原字符串无首尾空格 | Public 浏览器已集中使用 list/detail/mine/create/lower/bid 与账户读取的专属 raw JSON decoder，主体字段闭集、nullable/optional、projection、quota/transition 及多数归属关系已经迁移；list/my-bids/my-proxy/session 已统一把 transport 失败和非 2xx 安全 envelope 收敛为旧稳定 code，session 的 `unauthenticated` 回到匿名状态；logout 现在同时校验非 2xx envelope 与成功 `data:null`。浏览器 bid decoder 已移除旧版没有的 path lot 绑定，服务端领域归属校验不变；公开 `fact_index` 放宽到 `i64`，raw decoder 又把 JSON `1.0`/`1e0` 这类数学安全整数规范化后再进入 serde。登录封禁原因恢复“唯一 `public_reason` 键、原串无首尾空格、UTF-8 1024 字节和无控制符”；装备 URL 的 host/port/userinfo 候选差异经原始前缀 gate 证明不可达，未作无效放宽 | 严格成功边界继续阻止非法、缺失或扩张响应伪装为正常数据；本轮又关闭了错误分类、登出成功判定、极端合法数值和封禁文案分叉。浏览器不再额外绑定 bid path lot，但服务端仍在命令与领域边界保护 lot owner，不以恢复旧 decoder 为由削弱写入不变量；运行态坏响应尚未覆盖前仍不能宣称整条路由闭合 | 逐项恢复旧成功/错误 envelope 和可达数据边界；对超出旧 decoder 但保护项目明确不变量的加强单独记录为有意差异，不用宽松 DTO 掩盖。服务端局部 wire 修复仍不能替代浏览器坏响应取证 | list 注入顶层未知键后新旧都进入 `invalid_response`，但运行中的新版仍暴露 PUI-025 错误壳差异。CodeGraph/Luna 已逐路由确认 list/detail/mine/create/lower/bid、账户和 session/logout 的上述边界；待当前源码重建后补 list 的合法/未知/缺失/坏语义 R1，并继续 mine/create/lower/bid/account/session/logout 一次性探针，不提交测试脚本 |
| API-022 | Public lot 持久化投影语义校验 | P1 | 合法 live NULL/旧哨兵 R1；坏持久化待证 | 旧 application/read adapter 在返回前校验整份 lot page/detail、projection、分页/分类计数、lot/auction 归属、装备与 seller 等业务字段；依赖或无效 owner 结果返回 503 `temporarily_unavailable`，空 seller 不会被伪造成占位值 | Application 已建立 projection/equipment/preview/live/page/detail 统一 validator，补齐非零 ID、Go 零时间、状态/展示/竞拍关系、最终阶段限制、时间/价格/leader/state、精确八分类计数及筛选 total；PostgreSQL/SQLite 读取均在同一事务快照映射并返回前校验。Public 防腐层只把新版 `current_price=NULL + no leader` 与旧版 `current_price=start_price + no leader` 两种等价哨兵投影成旧 wire 起拍价；有领先者缺价格、无领先者但价格变化、空 seller 或坏策略继续失败关闭。HTTP 再校验归属并恢复旧 503 code，装备 attributes 的未知内部形状采用比旧 backend 更严格的安全失败关闭 | 已知浅校验、seller `—` 伪造及 generic `storage_unavailable` 差异已关闭；SQL 可见性筛选仍可能把不兼容状态表现为 not found，不能把本条夸大成对任意隐藏坏行都返回 503 | 统一 validator 只约束公开投影，不把 SQL 行或原始 attributes 直接暴露给浏览器；合法 preview 可选 category 与 live 必填 category 继续分离，页面错误/空态另由 PUI-022 收口 | 当前新版数据库 live lot 实际为 `current_price=NULL/leader=NULL/start=1100`：修复前列表为 503，最终 embedded 在不改数据库后列表与详情均为 200，并与旧版起拍价哨兵响应具有相同顶层/lot/equipment 键集合及 `start/current/next=1100,has_leader=false`。新版浏览器根页也显示当前价/下一口均 `1,100c`。Application/HTTP/Storage PostgreSQL/SQLite 编译、rustfmt 与 Storage Clippy 通过；合法 preview、空 seller及非法计数/状态/时间/ID/价格关系仍待隔离反例 |
| API-023 | Public lot nil UUID 边界 | P1 | 已验证，nil/大写/非规范/合法缺失 R1 | 旧 `ParseUUID` 同时拒绝非规范文本和全零 UUID，列表返回 400 `invalid_auction_id`，详情返回 400 `invalid_lot_id` | Public lot 路由现共用规范小写、非零 UUID 解析器，在 storage 前按端点返回旧 400 code | nil/大写/非规范输入不再进入 storage 或误报依赖不可用 | 保持列表与详情的独立错误码，不放宽其它 UUID 路由；新版错误 envelope 的 `request_id:null` 继续按 API-004 作为非敏感扩展 | 新旧列表端点对 nil、全大写和无连字符 UUID 均精确返回 400 `invalid_auction_id`，详情端点三类均为 400 `invalid_lot_id`；同一规范小写但不存在的 UUID 在两端点两侧均为 404 `not_found`，证明非法文本没有落入 owner 的缺失分支。源码中 `parse_public_uuid`/`ParseUUID` 均在 reader 调用前拒绝输入；未保存探针脚本或运行测试用例 |
| API-024 | Public 降低代理 nil authorization 错误码 | P1 | 已验证，七种 path 组合 R1 | 旧 lower-proxy 对无效或 nil authorization path 返回 400 `invalid_authorization_id`，lot path 无效时优先返回 400 `invalid_lot_id` | lower 路由已分别按 lot→authorization 顺序解析；格式错误、非规范或 nil 各返回所属的旧 400 code | 调用方可以重新区分两个 path 参数错误，且失败发生在命令端口前；两个参数同时无效时 lot 保持旧优先级 | 保持 `invalid_lot_id`/`invalid_authorization_id` 分离；认证只用短时临时 session 穿过 middleware，不创建用户或业务事实 | 新旧均在有效 session 下对 nil/无连字符/全大写 lot 返回 400 `invalid_lot_id`，对相同三类 authorization 返回 400 `invalid_authorization_id`，两者同时 nil 时均由 lot 错误优先；code/message/data 对应，只有 API-004 已记录的新版 `request_id:null` 扩展。固定探针 command ID 在两库 `auction_commands` 均为 0；两条临时 session 由 trap 精确删除，结束后两库近两分钟 session 候选均为 0，未留下命令、代理、竞价或审计事实，也未保存脚本/测试 |
| API-025 | Public 竞价与代理业务错误闭集 | P1 | 残差已实施，待九原因与损坏状态 R1；NULL 结束时间见 BID-014 | 旧 HTTP 分别公开 `bidder_ineligible`(403)、`already_leading`、`active_proxy_exists`、`proxy_authorization_not_active`、`proxy_maximum_not_lower`、`proxy_maximum_below_binding`、`proxy_reentry_maximum_not_higher`、`invalid_policy` 与 `auction_not_open` 等稳定原因；代理策略/历史和 lower target 的错误优先于资格检查。损坏 persisted policy、row scan/decode 或非法内部 competition state 走依赖错误 503；create/lower 对正数非 credit-grid 金额在 lot 可见性之后失败。强制构造 `effective_end_at=NULL` 则会先被旧 `lockBidState` 映射为 409，但有效旧 schema 不允许该状态 | Domain/HTTP 已恢复多数可判别错误，应用事务也把代理策略、完整 owner history、重入与 lower target 校验移到资格检查前，PG/SQLite 验证连续 usage 与完整终态，Public Web 五语言覆盖稳定 code。HTTP 将 `InvalidBidState` 与真实依赖失败映射为 503 `temporarily_unavailable`，真正未开放状态为 409 `auction_not_open`；create/lower 在正数金额的 credit-grid 校验前先确认 lot 对 Public projection 可见，恢复“隐藏 lot + 正数非网格金额”先 404 的旧顺序。新版 schema 允许且 snapshot 把 NULL 结束时间归 503 的独立差异已移到 BID-014 | 稳定业务冲突、真正的损坏解码/策略状态和 schema 可达性现在分开记录，组合输入也恢复旧错误优先级；这避免用一个“损坏状态”标签掩盖 409/503 的真实分叉。静态调用链已对齐仍不足以证明真实 PostgreSQL 事务返回、错误 envelope 和零副作用 | 保持 typed 原因、事务回滚、锁和历史完整性；row decode、损坏 policy 与非法内部 snapshot 收敛到安全依赖错误，只让正常未开放状态输出 `auction_not_open`；NULL 结束时间按 BID-014 恢复 schema 不变量并单独决定兼容分支 | 双仓调用链已逐原因静态核对，九种正常业务失败均在写 ledger/bid/proxy/realtime/audit 前返回，PG/SQLite 错误事务回滚；Luna 又逐层纠正 NULL scan 的旧行为。尚无逐原因运行态 envelope、页面文案及无副作用 R1；下一步进入隔离 PostgreSQL 取证，不提交测试脚本 |
| API-026 | Public preview equipment category 可选 wire | P1 | 已实施，live/category 参数 R1；preview 三形状待证 | 旧 preview `equipment_category` 与详情 preview equipment `category` 可省略；旧浏览器接受“都缺失”或“都为相同非空字符串”，拒绝单侧缺失、显式 `null`、空字符串和不一致值；live equipment 的 category 始终必填非空 | preview summary/detail 使用独立 `Option<String>` DTO，HTTP mapper 把空内部值归一为缺失；浏览器 decoder 对 preview list/detail 保留同缺同值规则，对 live 强制非空。DTO 理论上的 `Some("")` 会序列化空字符串，但唯一 HTTP mapper 不会产生该形状；损坏 live 空 category 在 storage 更早安全失败 | 合法旧 preview 不再被拒绝或伪造成空字符串，preview/live 语义保持分离；更早拒绝损坏 live 行是安全加强，不改变合法 wire | 只恢复旧 `omitempty` 形状，不把可选性扩散到 live；保留 storage 对损坏 live 数据的失败关闭，并把不可达 DTO 空值作为边界约束记录 | Luna 已逐项核对旧 `api.ts`、新 `decoder.rs`、Contracts 与 HTTP mapper。当前两侧唯一 live auction 的 list/detail category 均为 `One-Handed`，`category=one` 都返回一行且 `page_size=10`；当前运行库没有 preview，故缺失/有值/null/空字符串与单侧缺失三形状仍不得标 R1 |
| API-027 | Public 普通竞价 lot UUID 规范边界 | P1 | 已实施，静态门禁通过，待 R1；外层优先级见 API-032 | 旧 `POST /api/v1/lots/{lot_id}/bids` 使用共用 `ParseUUID`，只接受规范小写、带连字符、RFC 形式且非 nil 的 UUID；其它形状在进入竞价端口后由 path 校验返回 400 `invalid_lot_id` | 新 lot 读取、代理命令与普通竞价 handler 现统一使用 `parse_public_uuid`；大写、无连字符、nil 及其它非规范 Rust 可解析形状都在 path 校验处拒绝 | 同一 v1 surface 的 path 闭集重新一致，非规范文本不会进入 lot lookup 或请求摘要；它与认证、Origin、限流、Content-Type/body 的组合优先级此前未在本项证明，现拆到 API-032 | 保持共用 `parse_public_uuid`；请求外层失败 owner、命令摘要、金额校验和领域事务分别按 API-032/BID-015/API-033 审计，不能由单一合法 session 样本外推 | Luna 与主线程 CodeGraph 均定位到旧新 UUID 分叉；HTTP check/Clippy 已通过，尚待 embedded 重建和临时 session 探针，不外推 API-023/024 的其它路径证据 |
| PUI-022 | Public lot 错误与目录空态语义 | P1 | 已修复，五语言/双视口错误与空态 R1 | 旧 lot 详情保留 HTTP `status/code`，非法 UUID 本地显示 `role=status` 的 Missing；404/400/503/坏响应使用 `role=alert` 的共享 `ui-empty-state`，详情与 Retry 默认视觉隐藏。详情 query `retry:false`，仅实时连接正常且错误或 lot 仍为 pending/live 时每 60 秒兜底刷新；目录按 active 是否为 `all` 区分两种空态 | Public 读取现只保留安全 status/code，不携带服务端正文；详情恢复旧 title/detail/Retry DOM、错误键映射及 60 秒连接态轮询条件，目录恢复全空/分类空五语言键。繁中/日/韩补齐 not-found/invalid-response 隐藏详情翻译，不复制旧英文回退 | 过滤为空不再伪装整场无 lot，永久 404 在 manual 模式不再每 5 秒重放；辅助技术可以区分本地缺失、协议错误和服务故障 | 保持旧可见壳、role、隐藏重试与 status/code 选择；旧三种语言缺翻译作为缺陷修复，目录 page size/共享 Pager 另由 PUI-024 记录 | embedded Public payload `5c171828c3152589e8c36f56d658ccfecfbbfc3b38cf61cd8187effdb7ed6c47`。非法 UUID 新旧均不请求 lot API；真实 404、注入 400/503/坏 200 在五语言与 390/1440px 下 title、role、三子节点、Retry、相对矩形逐项一致，英/简中详情相同，繁中/日/韩仅完整翻译替代旧英文。manual 模式等待超过 5 秒后新旧 404 均仍只有一次请求。真实 `?type=two` 与受控全空响应的五语言空态、active tab、无表格、相对几何和无横向溢出均对应；未提交探针脚本或测试用例 |
| PUI-023 | Public populated 当前拍卖结构与文案 | P1 | 已修复，五语言/双视口 populated R1 | 旧当前拍卖使用 `RoundHeader → aria-labelledby=auction-lots 的目录 section → Catalog`：标题按 locale 插入轮次，截止标签后依次为倒计时与绝对时间，长周期固定为 `Nd HH:MM:SS`，分类标题为 `h3`；shell 的跳转链接命中真实 `main#public-content`，导航、表头和匿名 CTA 使用各自旧键，根页登录 URL 为 `/login`。目录行另把等宽 code 类放在链接、装备/卖家使用旧 `ui-truncated-text`、倒计时由 `td > span.public-auction-countdown` 承载、avg roll 带同文案 `aria-label/title`，动作格没有移动端 `data-label`；lot 状态优先使用独立 `auction.lotStatus.*` 五语言键 | 修复前 populated 根页把 header/catalog 直接平铺，缺少 `public-auction-live`、目录 region/隐藏 `h2` 与 `main#public-content`；标题拼接、截止顺序、长周期单位、导航/表头/CTA 独立键、根页登录 URL 以及目录行 class/子节点/ARIA/动作格闭集均有差异。现已恢复旧 DOM 层级和五语言可见文案，日/韩价格历史导航、日文状态表头及简中 live 状态也使用各自旧键；code/倒计时/truncated/avg roll/actions 结构已逐项回归旧版 | 原差异会同时改变可见信息层级、标题语义、键盘跳转目标、辅助技术 region、表格语义和窄屏布局；修复后这些路径已由同数据双视口证明。旧版非默认 locale 的隐藏标题回退英文，以及两库 forum 权威状态不同，均与目录组件等价分开记录 | 保持旧 DOM 层级、标题/截止/倒计时/目录/导航/表头/CTA 的独立 Fluent 键及根页默认登录 URL；目录 live/preview 行保持旧 class/子节点/ARIA/动作格闭集和 lot status 译文。五语言继续完整本地化，旧版繁中/日文/韩文隐藏标题漏译不回迁 | embedded Public payload `6580af51f826b09b3aa59fafe618c1a25d8e6570377badda54e1ea640d437c6d`。同一 One01 数据在 zh-CN 1440×1000 下，新旧 page/live/category/catalog/row 矩形分别同为 `[240,81.5,960,443.5]`、`[240,210,960,315]`、`[240,326.5,960,122.5]`、`[240,365.5,960,83.5]`、`[241,402.5,958,45.5]`，十个单元格的矩形、class、`data-label`、首子节点、状态和 CTA 逐项一致；390×844 下五种 locale 的导航、表头、状态、CTA、倒计时 `Nd HH:MM:SS`、两个 `/login`、动作格无标签及 live 内相对几何均无差异。繁中/日文/韩文隐藏 `h2` 的新版译文替代旧英文回退，作为旧翻译缺陷修复；forum 文案来自两库不同状态，不计入本项 |
| PUI-024 | Public 共用 Pager 与当前目录分页边界 | P1 | 已修复，目录双视口 R1；其余调用点待证 | 旧当前/历史目录固定请求 `page_size=10`；共享 Pager 使用 `ui-pager*` DOM、`noValidate`、可见 label/input 类、纯 ASCII 数字跳页和原始页越界 effect，目录 aria label 的英文/简中分别为 `Auctions`/`实时竞拍` | 当前目录曾写死 20，PublicPager 使用 `public-auction-pages*`、缺 `novalidate` 和控件类，`parse::<i64>` 接受带符号输入并在首帧提前 clamp；目录 label 又误用通用 lot pagination。现已恢复旧请求、DOM、输入和越界语义，并为繁中/日/韩补齐独立目录 Pager 标签 | 多于 10 个 lot 时分页边界会改变；DOM/class 与原生校验差异会影响 CSS、键盘和辅助技术，即使一页空态的外观碰巧相同也不能视为等价 | 共用组件恢复旧结构和输入闭集；游标兼容时仍只允许跳到当前已知页。目录英/简中沿用旧 aria 值，旧繁中/日/韩英文回退不回迁 | 最终 payload `5c171828c3152589e8c36f56d658ccfecfbbfc3b38cf61cd8187effdb7ed6c47` 下，新旧真实分类空请求均为 `page=1&page_size=10&category=two`；1440px nav/form/label/input/button、44px 高度、12px gap 与 `1e2` 禁用 Go 相同，390px controls/jump 两行矩形和无溢出相同。五语言可见状态/按钮与结构一致；繁中/日/韩 aria 使用完整翻译。`/auctions`、`/history` 标签 owner 已静态改回各自页面键，其余 PublicPager 调用点仍待逐页 R1 |
| PUI-025 | Public 当前目录加载、自动重试与错误壳 | P1 | 已验证，一次自动 retry 与错误/Retry R1 | 旧 `LiveAuction` 在 query pending 或 current/list 尚未对齐时渲染 `p.public-page-status[role=status]` 与 `common.loading`；Public QueryClient 对普通 query 固定 `retry:1`，约一秒后重试一次，第二次仍失败才进入错误态。错误壳为 `div.ui-empty-state[role=alert]`，标题固定目录全空文案，detail 按错误 code 选择旧键并回退 `common.error`，Retry 使用 `ui-empty-state__retry` | 修复前新版首次坏响应就显示 `section.public-auction-empty`、通用 Auction 标题与“数据暂不可用”，且不会先自动重试。payload `f984c68e…` 已恢复 pending/alignment status、目录专属 `CatalogLoadFailure`、稳定 transport/HTTP error code 和旧 DOM/class/文案；一次性“首响应坏、第二响应合法”探针随后证明新版仍停在错误壳，而旧版在用户点击前已自动恢复。最终 payload `0ef53419…` 已把目录读取拆成两次 attempt，中间固定等待 1000ms，并完成新旧瞬时/连续失败及手工 Retry 对照 | 通用错误壳会丢失 invalid-response/network/timeout/not-found 等可行动原因并改变节点、ARIA 与 93px/167.5px 几何；缺少旧 retry 会把瞬时协议/网络故障过早暴露给用户。alignment 被表现为错误还会把暂态误报成故障 | 目录 query 使用自己的旧错误壳，不全局替换其它页面的 `RetryPlaceholder`；只展示本地安全 code 文案，不显示服务端正文。恢复恰好一次 query retry，不做无限重放；alignment 继续先自动刷新一次并以 loading 呈现 | `f984c68e…` 下，unknown-field 在 en/zh-CN 的五语言两视口中 DOM、文案、矩形逐项相同；zh-TW/ja/ko 仅以完整本地化 detail 替代旧英文回退，节点和尺寸相同，延续 PUI-022 的旧翻译缺陷修复。缺 `category_counts`、lot/auction 归属冲突、503 与 network_error 在 1440px 下新旧 title/detail/Retry 和 `960×93` 完全对应。最终 payload `0ef53419cfe62b71602712b03d1b2e340cee0d49112ede1630038fc5e6423f1d` 下，首坏后合法时新旧都在点击前恰请求 2 次、无 alert 并显示唯一 One01 行；前两次都坏时双方同为请求 2 次，错误壳标签/class/三子节点/英文文案及 `[240,318.5,960,93]` 完全一致且无横向溢出；点击 Retry 后双方都在第 3 次请求恢复同一行且移除 alert；不保存探针脚本 |
| PUI-026 | Public session 坏响应的查询生命周期 | P1 | 已验证，有界恢复浏览器 R1 | 旧 `PublicApp`、`SessionMenu`、`MyBidsPage`、live `LiveAuction`、按非空类别挂载的 `LiveTable` 及 lot `BidControls` 均以同一 `["account","session"]`、`retry:false` 订阅；全局 `refetchOnMount/refetchOnReconnect/refetchOnWindowFocus` 对该 key 返回 true。首个 malformed 401 进入 error 后，后续 observer mount 可能再次 refetch | 新版保留 `PublicShell` 的唯一 session owner，并只对 `invalid_response`、network 与 timeout 在 300ms 后重试一次；完整 `unauthenticated + data:null` 立即返回且不重试，持续失败最多两次。旧根页快速 malformed 的 2/3 次挂载副作用没有迁移 | 避免复制无稳定契约价值的多 observer 重复流量，同时恢复“首坏后合法”时用户可见的匿名菜单与竞价登录 CTA；持续坏响应仍保持 error，而不是伪装成匿名 | 不把 malformed 401 转换为匿名；恢复恰好一次有界 transport/protocol recovery，不做无限重放，页面继续只消费共享 session 状态 | embedded Public payload `9a3daca0ab234bd448e4d04533ee7c34bb6fe8f9e85f9ad3e7d662002a9df3d6`。1440px/en 根页合法匿名时新版恰请求 1 次；首个 malformed 401 缺 `data`、第二个合法时新旧均恰请求 2 次、无 alert 且恢复两个 `/login`；持续 malformed 时新版恰 2 次、旧版因多 observer 为 3 次，双方均不伪装成匿名。一次性 CDP 拦截未保存脚本，结束后页面已恢复正常响应 |
| PUI-027 | Public 当前目录分类筛选的可访问名称 | P3 | 已实施并通过五语言 populated 浏览器 R1 | 旧 `Catalog` 分类 nav 使用 `auction.filter.label`：en `Filter lots by type`、zh-CN `按类型筛选拍品`、zh-TW `依類型篩選拍品`、ja `種類で出品を絞り込む`、ko `유형별로 출품 필터링` | 新分类 nav 改用 `filter-label` 的名词短语：en `Lot categories`、zh-CN `拍品分类`、zh-TW `拍品分類`、ja `出品カテゴリ`、ko `품목 분류`；可见 tab 文案一致，但屏幕阅读器得到不同操作上下文 | 已恢复旧五语言动作型 aria label；分类按钮、计数、active 状态和页面可见布局不变 | 不改变 `aria-pressed`、category query、page reset、分类闭集和完整本地化；PUI-023 已明确保留的繁中/日/韩隐藏目录标题补译不属于本项 | embedded Public payload `004a4feb41f112efda1db03496b05c55e1abf1c2b1a147b9e86bc5c45861b277` 在新旧同一 populated 根页动态切换五语言，分类 nav aria label 五组逐字一致；每次 picker、`hv.lang` 与 `<html lang>` 同步，最后恢复 en。i18n、Public WASM check/Clippy 与 `git diff --check` 通过，探针未保存为脚本 |
| API-013 | Admin dashboard 响应闭集 | P0 | 已修复，认证 shape 局部 R1 | 旧 `/api/v1/admin/dashboard` 的非 unknown section 只含 `status/observed_at/data`，unknown 只含 `status/error_code`；process、credentials、recovery、alerting 各有严格闭集，旧 Web 对顶层及每层使用 `only` | HTTP 已恢复旧字段和 disabled 语义；Contracts/浏览器均拒绝未知字段并逐层校验 raw JSON | 旧 Web 会拒绝整份响应的破坏性扩张已关闭；内部 scope、restart、evidence age 等增强不再泄漏到旧 v1 wire | 同一路径只保留旧 section/data 闭集；扩展诊断必须使用独立新契约 | 已认证新旧请求均 200，顶层十键及 health/worker/其余 section 闭集逐项相同，known/degraded/unknown/disabled 形状均可对应；两库数值不同不作为 shape 差异。仍缺同一数据库值级 R1 |
| API-014 | Admin recovery confirm 成功响应字段 | P0 | 成功 wire 已静态修复；其余 confirm 差异拆分至 API-028/029/030、REC-001/002、AUI-060 | 旧 `POST /api/v1/admin/recoveries/{id}/confirm` 成功 envelope 的嵌套对象使用 `recovery.id`，并固定输出 `action/id/relisted_auction_id/relisted_lot_id/resolution/stage/state_version`，两个 relist 字段即使无值也显式为 `null`；旧浏览器逐层严格拒绝额外键 | Rust 内部仍保留领域名 `recovery_id`，结果 DTO 以 `serde(rename = "id")` 恢复旧 wire；嵌套 result/response 拒绝未知字段且 nullable relist 字段显式存在 | 旧 Web 在 200 后把已提交命令误判为无效协议的 P0 字段路径已关闭，但不能把该局部 shape R1 外推为 confirm 的输入优先级、摘要、并发重放、relist 副作用或依赖错误等价 | 同一路径只输出旧成功闭集，不能同时保留 `recovery_id`；其余差异按独立 owner 收口，避免继续让单一 API-014 行掩盖多个事务与 WebUI 风险 | Contracts/HTTP/Admin 原生、Admin WASM、Clippy 与格式门禁通过；embedded 边界探针确认额外请求字段为 `400 invalid_json`、合法闭集但不存在 recovery 为 `404 not_found`。当前隔离库 recovery 数为 0，成功首次/重放仍待 populated R1 |
| API-028 | Admin recovery confirm 校验与 middleware 优先级 | P0 | 已修复，十组组合输入 HTTP R1 | 旧顺序为 rate limit → origin → CSRF → session → handler 内 path/query/body；因此缺 CSRF cookie 优先 403 `csrf_invalid`，无 session 加坏 path/JSON/content-type 仍优先 401，query/path 使用 recovery 专属错误码 | confirm 路由现以 route-specific owner 恢复 rate limit → origin → CSRF → session → path/query/content-type/body；缺失 CSRF cookie/header 统一 403，无 session 不泄漏坏 path/body 分类，非法 recovery path 使用 `invalid_recovery_command`。其它 Admin 路由与通用 strict JSON middleware 未调整 | 攻击面与正常客户端重新观察旧 status/code 优先级；只移动失败 owner，不降低 origin、rate、CSRF、session、body limit 或字段闭集强度 | confirm handler 自行做精确 JSON content-type/语法/typed decode，路由仍保留 `DefaultBodyLimit`；其它路由继续使用通用 strict JSON | embedded Admin payload `cb940cf634fb51577876a503756dc49f27b09111fea88372e5604e6421497e3a`。新旧认证环境逐项得到相同 status/code：缺 CSRF 403 `csrf_invalid`、伪 CSRF 且无 session 401 `unauthenticated`、坏 path 400 `invalid_recovery_command`、query 400 `invalid_query`、错误 content-type 415、scalar null 400、未知字段 400 `invalid_json`、大小写 UUID 两类 400、规范但不存在 recovery 404；均未进入业务写入 |
| API-029 | Admin recovery confirm 输入归一化与跨版本摘要 | P1 | null/UUID 边界 R1；摘要接管待证 | 旧 Go decoder 对 scalar `null` 先得到零值再进入业务校验；非 reclaimed action 的 reason 若有首尾空白直接拒绝，并按 UTF-8 字节限制长度。幂等摘要使用 trim 后 reason 与 `{RecoveryID,Version,Action,Reason}` 的 canonical JSON，不包含 `command_id` | Contracts/HTTP 防腐层现把整包或字段级 `null` 映射为零值后交给业务校验，path/command UUID 只接受规范小写非零文本；应用按旧 `TrimSpace` 与 UTF-8 byte 边界校验 reason，并按 Go `[16]byte` JSON、字段顺序和 HTML 安全转义生成旧摘要。HTTP `request_id` 独立进入审计 metadata，不参与摘要 | 合法/非法边界、错误 code 及已接管旧 ledger 的精确重放重新统一；同一业务负载更换 request ID 不影响幂等，同 command ID 跨版本仍命中同摘要或稳定 conflict | 在 HTTP 防腐层复现旧 null/空白/UTF-8 byte/UUID 规则，在 recovery owner 使用旧 canonical 摘要；内部继续使用 typed action/version，不能靠双写新摘要制造旧实例无法读取的账本 | Luna 实施后主线程补齐整包 null、规范 UUID 与审计 request ID；Contracts/Application/HTTP/Admin Web 原生与 WASM check/Clippy、格式和仓库策略门禁通过。HTTP R1 已证明 scalar null、未知字段、大小写 path/command UUID 与旧版 status/code 一致；仍待 populated recovery 下普通/多字节/首尾空白及旧写→新读、新写→旧读摘要 R1 |
| REC-001 | Recovery confirm 并发相同命令重放 | P1 | 已实施，静态门禁通过，待并发 R1 | 旧 PostgreSQL 在同 `command_id` 唯一冲突后回滚并重新加载已提交 ledger；同摘要返回原成功结果，异摘要返回 command conflict | recovery command insert 现只把 `settlement_recovery_commands_pkey` 的 23505 识别为并发同命令候选；显式回滚 failed 事务后用新事务读取已提交 ledger，并复用完整 typed replay 校验 | 两个同时到达的相同请求不再由唯一冲突直接误报 409；异 recovery/version/action/摘要仍冲突，ledger 缺失或结果损坏失败关闭 | 保留 Serializable 最多三次与提交不确定性边界；新事务只读命令结果，不重放业务写入或审计 | Luna 实施后 PostgreSQL check/Clippy 与格式门禁通过；主线程复核 recovery/摘要/action/version/结果闭集。仍待隔离并发 R1 核对一份 ledger/业务事实/审计，不新增并发测试用例 |
| REC-002 | Recovery relist 稳定 ID 与状态副作用 | P1 | 已实施，静态门禁通过，待 R1 | 旧 relisted lot ID 使用 namespace `auction.recovery.relist-lot.v1`、recovery UUID 与 nil UUID 三段长度前缀 SHA-256 自然键；目标 auction 在同一 CAS 中同时推进 `preview_revision` 与 `state_version`。realtime event 的 `state_version` 使用新 preview revision，event ID 随机且事务重试复用；Worker relist command ID也由 recovery 自然键稳定派生 | lot 与 Worker relist command 已改回旧长度前缀 SHA-256 自然键；目标 auction 继续原子推进 preview/state 双版本。preview realtime event 改用新 revision 作 state version，随机 event ID 在进入 Serializable 重试前生成并跨 retry 复用；Admin audit 恢复 auction entity、NULL 列 request ID 与旧 metadata，并按旧规则把入口 request ID 放入 metadata | 接管后同一 recovery 重新生成旧 lot/command 引用，ledger、审计与下游关联稳定；event version 与随机身份也恢复旧 realtime 去重语义。保留 target auction `state_version`，避免破坏旧有并发 fence | 恢复旧自然键 lot/Worker command ID、preview revision event version 与随机但跨 retry 稳定的 event ID；保留 preview/state 双版本 CAS。Admin audit 字段按旧筛选语义恢复，已有混存数据只读盘点后再决定显式迁移 | Luna 与主线程完成持久化身份、事件和审计语义；Domain/Application/Storage PG/SQLite/HTTP check、格式和仓库策略门禁通过。仍待 populated recovery、Serializable retry 与接管数据 R1 |
| API-030 | Admin recovery confirm 依赖错误映射 | P2 | 已实施，静态门禁通过，待 R1 | 旧共享 `WriteDependencyError` 对 request context deadline 返回 504 `timeout`，其它依赖失败返回 503 `temporarily_unavailable` | 新 HTTP 边界本来已有全请求 timeout/cancel owner，但输出 code 为 `request_timeout`；confirm 的其它应用/存储错误又统一为 503 `storage_unavailable`。现已分别恢复旧 `timeout` 与 `temporarily_unavailable`，不尝试从已抹平的 SQL 错误文本猜测超时 | 运维和页面重新获得旧 504/503 分类，且共享 request deadline 会取消 handler future；普通依赖错误不泄漏底层正文 | timeout 由全 surface HTTP deadline 统一拥有，confirm mapper 只映射已返回的非 timeout 依赖失败；事务未知结果仍按幂等 ledger 先对账再决定响应 | 主线程对照旧 `httpx.WriteDependencyError` 与新 `HttpBoundary`/recovery mapper 后实施；HTTP 与依赖 crate check、格式和仓库策略门禁通过。仍待受控 timeout/依赖失败 R1，并核对 dialog/输入/command ID 均按 AUI-010 保留 |
| AUI-060 | Admin Web 对旧错误 envelope 的兼容 | P1 | 静态修复，待受控错误 R1 | 旧共享 `requestJSON` 要求错误正文为非数组对象且 `code/message/data` 三键存在、前两者为字符串；它允许未知顶层扩展，也不限制 `code` 的词法或长度 | Admin Web 共用错误模型现只要求旧三键及其类型，忽略全部未知顶层扩展并原样保留任意字符串 code；`message/data` 只完成协议解码，不进入页面状态。字段缺失或类型损坏仍安全回退 `http_<status>` | 滚动升级、回退或网关追加非破坏性诊断字段时，页面不再丢失旧客户端本可消费的稳定 code；400/409/503 dialog 生命周期继续由 code owner 决定 | 只要求并消费 `code/message/data`，忽略所有未知扩展；`message/data` 不进入 UI，`code` 原样作为稳定分类。字段缺失或类型损坏继续回退安全 HTTP 状态 | 双仓严格 decoder 已逐分支复核；Admin 原生/WASM check、Clippy `-D warnings`、rustfmt 与仓库策略门禁通过。仍待旧 backend 的扩展字段、非常规 code 和损坏 envelope 受控 R1 |
| AUI-061 | Admin 已认证导航当前项 class | P3 | 已修复，12 路由浏览器 R1 | 旧 React Router `NavLink` 对当前路由同时输出 `aria-current="page"` 与 `class="active"`，非当前链接的 `className` 为空；12 个导航路由共用该形状 | Dioxus `Link` 默认只输出 `aria-current="page"`，初次登录后逐页浏览器盘点确认 12 个当前链接均缺 `active`。已认证 shell 现订阅 `use_route::<Route>()`，逐路由显式恢复 active/空 className，保留框架的 aria-current | 当前 CSS 已按 aria-current 着色，因此视觉颜色碰巧一致，但 DOM/class、旧样式扩展点与严格 WebUI 对照仍不等价 | current route 只由 Router 权威状态决定，不读取一次性 window location；链接 href、文案、导航顺序与认证生命周期不变 | Admin payload `7532085d54b1628da96a61e0ce20a168c593b44da8c18f476d1d37b4562781d7`。修复前 `/` 及其余 11 路由均为旧 active、新空；修复后 12 条当前链接逐项同时具有匹配 href、`aria-current=page` 与 `class=active`，非当前链接的 className 继续为空；Admin WASM check/Clippy、格式和新 embedded 运行均通过 |
| API-031 | Admin users 列表筛选与稳定排序 | P0 | 已修复，认证数据 R1 | 旧 `CountAdminUsers/ListAdminUsers` 对 `username/game_player_id` 使用区分大小写的 `strpos`，按 `username, id` 返回；旧 Users owner 再用 Go 字符串字节序与 UUID 字节序校验严格递增 | PostgreSQL/SQLite 均把筛选和排序改成了 `lower(...)`；应用层也按小写 Rust 字符串复核。PostgreSQL 默认 locale 排序又不等于 Rust 字节序，当前运行库 7 条用户中有 5 条位次不同，列表因此被误判为损坏并向 Web 返回 503 `temporarily_unavailable` | `/users` 在合法数据上完全不可用；大小写筛选结果和分页边界也偏离旧 v1，不能归因为数据库迁移或运行数据。旧实现自身在非 C locale 下也会因 SQL/Go 排序不一致返回 503，这是本项明确修复而不回迁的旧缺陷 | 已恢复旧区分大小写筛选和原始用户名排序；PostgreSQL 显式使用 `COLLATE "C"` 固定旧 owner 实际校验的字节序，SQLite 使用默认 BINARY；应用 validator 同步恢复原字符串/UUID 顺序，避免数据库 locale 再次制造假损坏 | 发现基线：embedded Admin payload `7532085d…` 的认证 `GET /api/v1/admin/users?page=1&page_size=50` 为 503；同一非 C locale 条件下旧 endpoint 也为 503，但旧 Web 仍保留筛选壳。新版库迁移 `1..17` 全成功，表/字段完整，7 条 users 与 2 条 blacklist history 均通过值域、长度和时间校验；只读排名探针得到 `collation_rank_mismatches=5`、`non_ascii_names=0`。修复后二进制 `5957bda6e3f331e2a459ec9021f27d15c16acb417d229a2329b0cf636020c0d6` 在同一库返回 200/7 条且按原 username/id 严格递增；真实用户名精确大小写筛选为 1 条，翻转一个 ASCII 字母大小写后为 0 条，均与浏览器本地按旧规则计算一致；`/users` 页面无 alert、渲染 7 行并保持 active 导航。未保存探针脚本 |
| AUI-062 | Admin 列表加载/错误时保留页面能力 | P1 | 已修复，受控 pending/error 浏览器 R1；成功 DOM 残差拆分至 AUI-066/067/068/070 | 旧 Mail 始终保留标题、三个筛选器与独立 polling 区；Auctions 始终保留标题、world 筛选、创建入口和满足条件时的手工录入；Credentials 的导入表单也不依赖列表请求成功。loading/error 只替换各自列表区域，主要使用页面内联状态 | 修复前三页都在 resource 为 `None/Err` 时提前返回统一整页面板；现均先渲染稳定 shell 与独立操作区，只让 intake/list 槽位切换 pending/error/content。Mail 根 class 同时去除旧版没有的 `admin-panel`；Config 以外的这些旧页错误态不再附加统一 Retry | 暂时网络/协议/依赖错误不再阻断与失败 owner 无关的管理动作；标题、筛选、创建/导入、polling、焦点顺序和错误 owner 均恢复旧页面边界 | Mail 保留新版可点击详情扩展，Auctions 手工录入仍只在成功能力快照允许时显示，Credentials 删除 dialog/反馈 owner 不变；未改变 API 请求、分页、command ID 或五语言 | Luna 对 12 路由完成静态审计并实施 Mail，主线程实施 Auctions/Credentials。Admin native/WASM check 与 Clippy `-D warnings` 通过。CDP 受控挂起首个列表请求及返回 503 后，新旧 Mail 均保留 `h1`、3 个筛选器、polling form/2 个按钮；Auctions 均保留 `h1`、筛选与创建按钮；Credentials 均保留 `h1`、2 个选择框、导入 form/按钮，loading 为 status、错误为 alert。稳定 ID、文案、form/class 等剩余差异不混入本项，分别由 AUI-066/067/068/070 收口；探针未保存为脚本 |
| AUI-063 | Admin Config/Rules 加载与缺失外壳 | P2 | 已修复，受控 pending/error/404 浏览器 R1；稳定壳残差拆分至 AUI-071 | 旧 Config 与 Rules 在 loading/error 时仍保留唯一页面标题和说明；Rules 文档 404 时也只有一个页面级标题。Config 成功态 fingerprint 只在事实卡中显示一次，错误只是一条内联 alert；Rules 错误保留自己的 Retry | Config/Rules 现把 pending/error/missing 收回稳定页面 shell；Rules workspace 不再创建第二个页面标题/说明，404 的 missing 状态与编辑区共用外层页面 `h1`；Config 删除重复 fingerprint status，并恢复无 Retry 的单一内联错误 | 错误态保留页面上下文，Rules 不再产生重复页面标题/可访问名称，Config 可见事实层级与旧版一致 | 命令、草稿、签名、revision、发布、重启和错误 code 不变；Rules 继续保留旧页 Retry，Config 按旧页不提供额外 Retry | Luna 实施 `operations/config.rs`、`content/rules/page.rs`、`workspace.rs`，主线程复核旧 Config/Rules 错误分支并移除 Config 多余 Retry。Admin native/WASM check 与 Clippy `-D warnings` 通过。CDP 受控 pending/503 下，新旧 Config 均保留标题/说明且无 Retry，Rules 均保留标题/说明并在错误态保留 Retry；404 下双方均有 missing status、workspace 与 9 个按钮。双方第二个 `h1` 都来自 Markdown 预览文档标题“未命名”，不是重复页面标题。根 class、稳定 ID 与简中页面名残差由 AUI-071 记录；探针未保存为脚本 |
| AUI-064 | Admin Auctions/Credentials 空态与成功 DOM | P2 | 已实施，Auctions populated 与 Credentials empty 双视口 R1；反向数据态待证 | 旧 Auctions/Credentials 成功页均先渲染空 table，再在 table 后显示 empty 文案；Credentials 根为带 `aria-labelledby` 的 `section.admin-credentials-page`，没有总数摘要或 restart 提示 | 两页已恢复 table→empty 顺序；Credentials 根恢复旧 section/class/label 关系，删除额外 list section、total 摘要和 restart 提示。`restart_required` 仍保留在 API/内部状态，不影响运行语义 | 空态阅读顺序、辅助技术 section 语义、可见节点和垂直几何重新统一；没有为视觉一致性删除后端能力字段 | Auctions 与 Credentials 只调整 Web 投影和 DOM 顺序，不改变表格行、分页、动作或数据库；对应 CSS 标题 selector 随旧根 class 同步 | embedded payload `deba6969…` 下，Auctions 同为 9 行 populated，根/筛选/table/pager 顺序及 1440/390px owner 一致；Credentials 同为空态，390px 下根、导入 form、table→empty→pager 顺序和各矩形逐项相同，且没有 total/restart 节点。当前隔离库缺 Auctions empty 与 Credentials populated，故不外推这两个反向状态；探针未保存为脚本 |
| AUI-065 | Admin Renames/Outbox 小型 DOM 语义 | P3 | 已修复，认证浏览器 R1 | 旧 Renames 行 `td` 没有移动端 `data-label`；旧 Outbox 状态筛选的 label 通过 `htmlFor` 关联 `select#outbox-status` | Renames 六个单元格的额外属性已移除；Outbox 已恢复 `label for="outbox-status"` 与同 ID select 的显式关联 | 严格 DOM 闭集和 label 点击/辅助技术命名均恢复，不改变可见表格数据 | 筛选 value、精确 GET URL、页码、replay、busy 和表格内容均未改变 | Luna 实施 `identity/renames.rs` 与 `operations/realtime/page.rs`；Admin native/WASM check、Clippy `-D warnings` 与 diff check 通过。认证 CDP 对 populated Renames 逐行核对六个 `td`，新旧均无 `data-label`、class 为空且文本顺序一致；Outbox 新旧 label 的 `for=outbox-status`、select ID/value/options 完全一致，未保存探针脚本 |
| AUI-066 | Admin Auctions 成功页外壳、筛选与表格术语 | P2 | 已实施，五语言 populated 与双视口 R1；empty 待证 | 旧 `/auctions` 使用 `section.admin-auction-page[aria-labelledby=auction-title]`，筛选为 `admin-auction-filter > form`、`select#auction-world.admin-auction-world`，空筛选值不写 `world_id`；创建入口和表格 shell 均有稳定结构。简中创建按钮为“创建拍卖”，表头依次使用“届次/开拍时间/截止时间/操作” | 新版成功态根原为 `div.admin-auctions`，筛选使用 `all` sentinel 和不同 ID/class，创建按钮为“创建草稿”，对应表头使用“序号/开放时间/计划结束/生命周期操作”；现已恢复旧根、筛选 DOM/空值 wire、创建入口独立文案、表格 class/th class、表头与 empty class/文案 | 可见术语、表单语义、可访问名称、请求 URL 和 CSS 扩展点重新一致；加载/错误时保留能力由 AUI-062 负责，table→empty 顺序由 AUI-064 负责 | API 客户端同时容忍既有 `all` 与旧空字符串但都省略 query；创建/草稿命令继续分用两个文案键。英/简中沿用旧创建文案，旧繁中/日/韩的英文回退不回迁，改用完整本地化；能力快照、生命周期动作、分页和 command ID 不变 | embedded payload `deba6969…` 下，同源 9 行页面的根、标题、筛选 ID/class/选项、表格及五语言标题/筛选/七列表头/pager 逐项对应；繁中/日/韩创建入口使用完整翻译替代旧英文 fallback，符合冻结扩展。切换 isekai 后双方同为 2 行；切回全部时网络请求均精确为 `?page=1&page_size=10`，不含 `world_id`。390px 下表格只在 wrapper 内横向滚动且 body 无额外溢出；隔离数据无 empty 状态，探针未保存为脚本 |
| AUI-067 | Admin Credentials 导入表单结构、字段顺序与文案 | P2 | 已实施并通过五语言/双视口 empty 浏览器 R1 | 旧 `/cookies` 真实简中页面的导入区标题为“导入 Cookie”，字段顺序固定为“类型、账号名称、世界、Cookie 值”，选项可见文本为 `mail_cookie/forum_cookie/main/isekai/shared`，提交按钮为“导入”；导入表单直接属于页面稳定操作区 | 新版曾额外包裹 section 并显示旧版没有的安全提示，标题/提交均为“导入凭据”，字段顺序为“类型、范围、标签、Secret”且选项被展示名替换；现已恢复旧 form 层级、四个稳定 ID、字段顺序、原始闭集选项和五语言资源 | 表单阅读/键盘顺序、可见节点、协议值认知和文案重新统一；错误态保留导入能力已由 AUI-062 负责 | 只删除浏览器可见的额外提示，后端 secret 加密、不回显、成功清空、command ID、反馈和删除 dialog 等安全/行为不变量保持；table→empty 与根 section 由 AUI-064 负责 | embedded payload `deba6969…` 下，新旧五语言逐项读取导入标题、四组 label/ID、两组原始选项、提交按钮和语言持久化状态，全部一致；390px 下 form 均为 `[216,145.61,135,420.72]`，根、table/empty/pager 的顺序和矩形也相同。未填写或提交 secret，探针未保存为脚本 |
| AUI-068 | Admin Mail/Auctions/Credentials 稳定 ID 与表格 shell | P3 | 已实施，三页 shell 双视口 R1；Credentials populated 待证 | 旧三页标题 ID 分别为 `mail-title`、`auction-title`、`credentials-title`；Mail wrapper/table 为 `admin-shell__table-wrap` / `admin-shell__table admin-mail-table`，Auctions 为 `admin-auction-table-wrap` / `admin-auction-table`，Credentials 为 `admin-credentials-table-wrap` / `admin-credentials-table`，Mail 还保留旧表头 class | 新版对应标题 ID 带 `admin-` 前缀；三个表格 wrapper 曾复用 Public 的 `public-auction-catalog`，Auctions/Credentials table 缺旧专属 class，Mail 表头也缺旧 class。现已逐页恢复旧 ID、wrapper/table/th class，并把对应 CSS owner 改回 Admin | 当前样式曾能勉强显示，但严格 DOM、可访问关系、旧样式扩展点及后续主题覆盖不等价；恢复后 Public catalog 不再隐式拥有 Admin 表格 | 只恢复 Admin 旧稳定 ID/class 和 label 引用，不改变行内容、排序、分页、dialog 或 Mail 新版详情能力 | embedded payload `deba6969…` 下，Auctions populated、Mail empty、Credentials empty 的标题 ID、root 关联、wrapper/table/th class 在 1440/390px 逐项对应，三页均无 Public class；Mail 窄屏 table 宽 `271.53`、wrapper 宽 `150`，Credentials 窄屏 wrapper 宽 `135`，双方一致且仅局部滚动。Mail 纵向 15px 差异来自 AUI-081 已登记专属提示；当前缺 Credentials populated 行，探针未保存为脚本 |
| AUI-069 | Admin Outbox 页面根、筛选与表格 shell | P2 | 已修复，认证空态浏览器 R1 | 旧 `/outbox` 根为 `section[aria-labelledby=outbox-title]`，高级筛选 form 使用 `admin-realtime-filter`；failure/lag 表格沿用 Admin table wrapper/class | 新版根曾为 `div` 且标题 ID 为 `realtime-page-title`，筛选 form 使用 `admin-panel__actions admin-realtime-filters`，failure/lag wrapper 复用 Public catalog class；现已恢复旧 root/ID/form/table shell 和对应 CSS | 页面 landmark、可访问关系、筛选 CSS 和表格布局扩展点重新归回旧 owner；AUI-065 仍单独负责状态 label/select 的直接关联 | 保持 status/advanced 参数、精确 URL、replay、busy、分页和数据闭集；不因换回旧表格壳删除新版安全投影 | Luna 实施 `operations/realtime/page.rs`、`filters.rs`、`failure.rs`、`lag.rs`，主线程同步 CSS；格式、i18n、Admin native/WASM check 与 Clippy 均通过。认证 CDP 下新旧根均为 `section.admin-realtime-page[aria-labelledby=outbox-title]`，标题、状态筛选、高级筛选 class、failure 与 lag 的 wrapper/table class 逐项一致，空态均无 alert；专属空态文案差异按 AUI-075 保留，未保存探针脚本 |
| AUI-070 | Admin Mail polling 操作文案与控件 ID | P2 | 已实施并通过五语言浏览器 R1 | 旧 Mail polling 使用 `summary#mail-polling-title`、`label[for=mailbox-key]`、`input#mailbox-key`；简中加载按钮为“确认”，触发按钮为“手动触发收件轮询”，其余语言由旧 locale 资源冻结 | 新版控件 ID 曾为 `admin-mailbox-key`，简中按钮为“加载”和“立即轮询”，标题也额外带 `admin-` 前缀；现已恢复旧 summary/input/label ID 和五语言 `description/load/trigger/mailbox` 资源映射 | 核心 polling API 本来可用，但可见动作含义、label 关系和自动化/样式稳定锚点不同；现已收口 | 保持 mailbox key 状态、command ID、busy/反馈和 API 请求不变；旧按钮本身没有独立 ID，因此不凭空新增 | embedded payload `deba6969…` 下，新旧动态切换五语言，`summary#mail-polling-title`、`label[for=mailbox-key]`、`input#mailbox-key` 与确认/手动触发两按钮逐项一致；picker、`hv.lang`、`<html lang>` 同步。未输入邮箱或触发轮询；空提示文字差异是 AUI-081 已登记修复，探针未保存为脚本 |
| AUI-071 | Admin Config/Rules 稳定页面 ID、class 与标题 | P2 | 已实施并通过错误/成功壳浏览器 R1；success workspace 残差转 AUI-086 | 旧 Config 根只使用 `section.admin-config-page[aria-labelledby=config-title]`；Rules 使用 `section.admin-rules-page[aria-labelledby=rules-title]`，页面标题取各语言 `rulesContent.title`，简中为“规则内容” | 新 Config 根曾额外带 `admin-panel admin-runtime-config` 且标题 ID 为 `admin-runtime-title`；新 Rules 标题 ID 为 `admin-rules-page-title`，标题键为“规则策略”。现已恢复旧根 class、标题 ID 与五语言页面名 | AUI-063 已恢复 loading/error/404 的页面能力；本项进一步收口稳定可访问锚点、CSS 闭集和可见页面名 | 保留新版签名、revision、草稿、发布、重启及 Markdown 预览能力；预览文档自身的 `h1` 仍是旧版也存在的内容标题 | 主线程实施 `operations/config.rs`、`content/rules/page.rs` 与五种 FTL；格式、i18n、Admin native/WASM check 与 Clippy 均通过。embedded payload `1021a78a…` 下 Config 错误态与 Rules success/missing 态均确认根 section、`aria-labelledby`、标题 ID 和“规则内容”一致；Rules success 内层 workspace 差异另由 AUI-086 冻结，探针未保存为脚本 |
| AUI-072 | Admin Outbox replay 确认文案与对话框焦点 | P2 | 已实施，五语言与静态门禁通过，待 populated R1 | 旧 replay 对话框确认按钮使用专属 `outbox.confirmReplay`，简中为“确认重放”；关闭后恢复打开 replay 的按钮焦点 | 新确认按钮误复用 Auction 通用“确认”键。静态审计一度怀疑缺少局部焦点恢复，但 Admin shell 已全局调用 `use_dialog_focus_trap(false)`；该钩子记录点击/焦点 opener、观测 dialog 增删并在关闭后恢复焦点 | 专属动作名称已按旧五语言恢复，避免破坏性操作只显示泛化“确认”；焦点继续由 surface 级唯一 owner 覆盖，不重复添加局部状态 | replay payload、schema、稳定 command ID、busy/错误反馈和 Escape 行为不变；全局焦点 owner 同时服务 Auction、Credentials、Outbox 等对话框 | Luna 实施专属键调用，主线程从旧五份 locale 恢复 `Confirm replay/再送を確定/재생 확인/确认重放/確認重播`；格式、i18n、原生/WASM check 和 Clippy 通过。待 populated failure 复验确认/取消/Escape 三条关闭路径 |
| AUI-073 | Admin 列表响应式/详情扩展与 CSS owner | P3 | owner 泄漏已修复，扩展已记录，静态门禁通过，待双视口 R1 | 旧 Mail、Outbox、Auctions 与 Credentials 行没有新版的 `data-label`/BEM 状态 class；Mail 没有可点击详情投影，旧 Auction/Credentials 的 raw 英文状态也未完整国际化 | 新版为移动端表格增加 `data-label`、状态 class 与逐行反馈，Mail 增加安全详情面板；这些是有价值扩展。但 Outbox lag 数值单元格误用了 Public 的 `public-auction-catalog__number`，跨越 surface owner | 保留不改变 wire/动作闭集的响应式、详情和五语言修复；lag 已删除 Public class 泄漏，Admin 不再依赖 Public catalog 实现 | 不回迁旧 raw 英文状态、缺失移动端标签或缺失详情的旧缺陷；`TruncatedText` 安全截断与详情数据仍只消费 Admin API 安全投影 | 两个 Luna 切片分别审计 Mail/Outbox 与 Auctions/Credentials，主线程冻结处理；格式、i18n、原生/WASM check 和 Clippy 通过，待双视口明确区分“应一致 DOM”与本项已登记扩展 |
| AUI-074 | Admin Mail/Outbox/Credentials 分页名称及 Outbox 反馈顺序 | P3 | 三页五语言 pager R1；Outbox populated 反馈顺序待证 | 旧 Mail 分页 `aria-label` 使用邮件页标题，Outbox failure/lag 分别使用 Outbox 标题与 consumer lag 标题，Credentials 使用旧 `credentials.pages` 的 Cookie pool 名称；Outbox replay 反馈位于 empty 之后、pager 之前 | 新三处 pager 复用了新建的通用 pagination 文案；Outbox failure 成功分支又把 pager 封装进列表内容，导致 replay 反馈落在 pager 后 | Mail、failure、lag、Credentials 已分别改回自己的旧标题键；failure 内容与 pager 拆开，使反馈恢复到 table/empty 与 pager 之间 | 分页页码、上一页/下一页/跳页、查询参数和 replay 刷新范围不变；只调整可访问名称与 DOM 阅读顺序 | embedded payload `deba6969…` 下，Mail/Credentials 五语言 pager 与 1440/390px 结构对应；最终 payload `01876a44…` 下，Outbox failure/lag 两枚 pager 在五语言分别与旧页面逐字一致，failure empty 后直接接 pager。当前无 populated failure 可验证 replay notice 位于 pager 前的运行态顺序，探针未保存为脚本 |
| AUI-075 | Admin Outbox 空态专属文案 | P3 | failure 五语言预期差异 R1；lag empty 待证 | 旧 failure 与 lag 空态都复用含义模糊的全局 `noResults`，不能说明当前为空的是失败事件还是消费者 lag | 新版分别使用 failure/lag 专属空态文案，五语言均能准确表达数据 owner | 这是旧版信息架构缺陷，不为逐字一致性回迁；DOM 位置、class 与阅读顺序仍按 AUI-069/AUI-074 对齐 | 不改变空态判定或 API 数据；只保留更准确的可见文案 | embedded Admin payload `01876a44…` 下，failure 在五语言分别显示专属“当前筛选无失败记录”语义，旧版对应全局 no-results；双方空态 `p` 均无 class，位置同为 table 后、failure pager 前，故仅文字是明确预期差异。两库 lag 均有 7/8 行，尚无 lag empty 运行态样本；探针未保存为脚本 |
| AUI-076 | Admin Auction populated 动作、创建与手工录入文案 | P2 | 五语言动作/intake 与简中 create 浏览器 R1；create 完整本地化为登记扩展 | 旧 populated 行使用“管理拍品”“可立即开拍”等动作/能力文案；创建对话框使用“创建拍卖、世界标识、拍卖序号、计划预览时间、计划开拍时间、截止时间”；手工录入以“向该拍卖添加装备/添加装备”表达目标与动作 | 新版对应简中文案仍为“查看拍品”“可开放”“创建拍卖草稿、世界、序号、预览计划时间、开放时间、计划结束时间”，手工录入又使用“添加装备拍品/添加拍品”，可见语义尚未等价 | 已恢复旧版无缺陷的动作、字段和带 world 的 intake 术语；旧版繁中/日/韩创建区硬编码英文 fallback 不回迁，改为完整本地化；英文旧 create 字段 `World/Number/Opens/Opens/Ends` 的重复、含糊标签也不回迁，继续使用可区分 preview/open 的完整名称 | 保留新版 UTC 输入提示、严格 canonical equipment JSON 校验、能力快照、command ID 和错误反馈；不为文案一致性放宽领域输入 | embedded payload `deba6969…` 下，同源 populated 行动作与 isekai intake 的标题、说明、四组字段、来源选项和按钮在五语言逐项一致；create 的简中标题/五字段/两按钮逐字一致，英文更明确字段名、繁中/日/韩完整翻译及 UTC note 均为本项登记扩展。所有 dialog 只打开后关闭，未提交创建或 intake 命令；结构证据见 AUI-082/083，探针未保存为脚本 |
| AUI-077 | Admin Auction/Credentials 行标题与 dialog 稳定关联 | P3 | 已实施并记录扩展，静态门禁通过，待 populated R1 | 旧 Auction 生命周期 dialog、Credentials rename/delete dialog 和 rename 输入使用按实体/动作生成的唯一 ID；Credentials populated 行还包含隐藏 `h3` 行标题，label 通过 `for` 关联输入 | 新版部分 dialog 使用页面内固定标题 ID，rename 输入缺显式 `id/for`，Credentials 行缺隐藏标题；同时把 delete dialog 提升为页面级遮罩，并增加移动端属性与逐行反馈 | 生命周期、rename、delete 的标题和 rename 输入已恢复实体/动作唯一 ID，Credentials 行补回隐藏 `h3`；页面级遮罩、响应式属性和逐行反馈作为结构/可用性增强保留并记录 | 全局 `use_dialog_focus_trap` 已统一覆盖焦点循环与 opener 恢复，无需复制旧组件的局部 ref；删除/改名/lifecycle API、幂等与 busy 状态不变 | 两个 Luna 实施 disjoint Rust 切片，格式、i18n、原生/WASM check 和 Clippy 通过；待 populated 行、唯一 ID、label 与 accessibility tree R1，页面级 delete dialog 为预期结构差异 |
| AUI-078 | Admin Outbox lag 表头与空态节点闭集 | P3 | 已实施并通过认证浏览器 R1 | 旧五语言 lag 表头使用 `outbox.offset`，简/繁中均为“偏移量”；failure/lag 空态 `<p>` 不带 class，位置在 table 之后、pager 之前 | 新简/繁中 `realtime-offset` 被译为“游标/游標”；failure/lag 专属空态又附加 `admin-panel__note`，虽然当前视觉接近但严格文本/class 闭集不同 | 已恢复旧 offset 术语和无 class 空态节点；专属空态文字仍按 AUI-075 保留，不混淆语义修复与 DOM 对齐 | lag 数据字段、排序、分页和 API 不变；只调整两种中文资源与空态 class | 格式、i18n、原生/WASM check 与 Clippy 均通过；embedded payload `1021a78a…` 与旧版认证页面确认 lag header、两张表/root/pager 和空态 class 一致，failure 专属文字为 AUI-075 预期差异，探针未保存为脚本 |
| AUI-079 | Admin Auctions 分页、隐藏行标题与 class owner | P3 | 已实施并通过 populated 浏览器 R1 | 旧 pager 的 `aria-label` 使用 `auction.pages`（简中“拍卖分页”）；每行隐藏标题固定为 `{world} · #{sequence}`，序号单元格不依赖 Public class | 新 pager 改用了页面标题“拍卖场次”，隐藏标题被本地化改写为“第 N 届拍卖 · world”，序号单元格又带 `public-auction-catalog__number` | 已恢复旧 pager 名称与隐藏标题词序并移除 Public class owner 泄漏；行 `data-label`、本地化状态和 BEM 行 class 仍按 AUI-073 保留为移动端/i18n 修复 | 列表数据、排序、链接、生命周期能力和分页行为不变；只调整可访问文本、class owner 与五语言资源 | 格式、i18n、原生/WASM check 与 Clippy 均通过；embedded payload `1021a78a…` 与旧版同源 9 行认证数据确认 pager 为“拍卖分页”、隐藏标题为 `world · #sequence` 且页面不再含 Public class，行 BEM/data-label 为登记扩展，探针未保存为脚本 |
| AUI-080 | Admin Auction 生命周期 dialog 内容结构与取消字段 | P2 | 已实施并通过 populated 浏览器 R1 | 旧 Preview/Open 使用 `form.admin-auction-open`，Final 使用无 form 的按钮组；说明 `<p>` 无 class。Cancel warning 位于 form 前，`label[for=cancel-reason-{id}]` 关联同 ID textarea，空值初始不显示 alert，简中字段/提交为“取消理由/确认取消拍卖” | 新四种动作统一包进 `form.admin-auctions__dialog-form`，说明附 `admin-panel__note`；Cancel 缺 `for/id`、空值一打开就显示 alert，简中文案为“取消原因/取消拍卖”；Final confirm 也从 button 变成 submit | 已按动作恢复旧 DOM、字段关联、初始错误生命周期和专属文案；动态 title ID 与全局焦点恢复继续保留 | lifecycle action、expected version、command ID、busy、错误反馈和成功刷新不变；只重排 Web dialog 投影，不发送任何额外命令 | 格式、i18n、原生/WASM check 与 Clippy 均通过；embedded payload `1021a78a…` 安全打开 Preview/Cancel/Final 但未提交，逐节点确认 title ID、form/说明/字段/按钮完全一致；Escape 关闭后两边焦点均恢复到各自 opener，探针未保存为脚本 |
| AUI-081 | Admin Mail 未选择邮箱时的 polling 空态 | P3 | 新版文案修复已保留，DOM 已通过认证浏览器 R1 | 旧 Mail 尚未输入邮箱时复用无 class 的全局“暂无结果”，无法说明下一步操作 | 新版使用五语言专属提示，简中为“输入邮箱标识后加载该邮箱的轮询状态。”，但额外附加 `admin-panel__note` | 保留更可行动的专属提示，视为旧信息缺陷修复；已移除多余 class，使节点位置与 DOM 闭集恢复旧形状 | polling 查询、邮箱键、加载/触发按钮和状态卡不变；只处理未选择目标时的展示 | 格式、i18n、原生/WASM check 与 Clippy 均通过；embedded payload `1021a78a…` 认证页面确认未选择邮箱的 `<p>` 位置与空 class 已一致，专属文字为本项明确预期差异，探针未保存为脚本 |
| AUI-082 | Admin Auction 创建 dialog DOM、字段 ID 与关闭动作 | P2 | 已实施并通过认证浏览器 R1 | 旧 dialog 直接包含 `form.admin-auction-create` 与独立 `div.admin-shell__dialog-actions`；表单内 `h3#create-auction-title`，字段 ID 为 `create-world/create-sequence/create-preview/create-open/create-end`，label 与控件为相邻节点，关闭按钮位于 form 外 | 新 dialog 内又嵌 `section.admin-panel`，标题为 `h2`，form 使用新 BEM class、label 包裹控件，字段 ID 加 `admin-auctions-` 前缀，关闭按钮进入 form；另有 AUI-076 已明确保留的 UTC 提示 | 恢复旧稳定 DOM、字段 ID、标题层级、form class 与关闭动作 owner；UTC 提示作为已登记扩展保留在表单内，但不改变提交闭集 | create API、world/时间校验、自动 UUID/command ID、busy、错误反馈及成功刷新不变；不得因为 DOM 对齐放宽 schedule 不变量 | embedded payload `6adef156…` 与旧版安全打开创建 dialog、不提交，逐节点确认 dialog 直接子节点、form class、标题、五组 label/ID、submit 与 form 外 close owner 一致；新版只多已登记 UTC 提示。关闭焦点残差转 AUI-089，探针未保存为脚本 |
| AUI-083 | Admin Auction lots 页面容器、intake、表格/空态顺序与 CSS owner | P2 | 已实施并通过 populated/draft 浏览器 R1 | 旧 lots 路由以 `section.admin-auction-page[aria-labelledby=auction-title]` 为外层，包含 `h1#auction-title`、`p.admin-auction-intro` 与 `section.admin-auction-lot-section[aria-labelledby=lot-title]`；intake 使用 `admin-auction-intake`、`auction-lot-intake-title`、`admin-auction-intake-form` 和 `auction-intake-*` ID；表格 wrapper/table 为 `admin-auction-table-wrap` / `admin-auction-table admin-auction-lot-table`，空态位于 table 后且 pager 前 | 新成功页简化为单个 `section.admin-panel[aria-labelledby=admin-lots-title]`，back/intake/内层标题/table/empty 使用新 BEM class/ID；wrapper 又错误带 `public-auction-catalog`，金额单元格带两项 Public class，空态移到 table 前；loading/error 也切换成另一套通用 panel | 恢复旧无缺陷的外层、loading/error/success 稳定壳、intake 字段 ID、Admin table/amount class 和 empty 阅读顺序，移除全部 Public owner 泄漏；旧隐藏 lots 标题因 locale 参数错误显示 `{{id}}` 属历史缺陷，不逐字回迁，改用正确当前场次标题 | 保留新版 `data-label`、本地化状态/状态变体 class、具体“没有找到拍品”空态、严格 JSON 校验、Retry 与逐行反馈等扩展；列表 API、intake 条件、分页和路由不变 | embedded payload `6adef156…` 在同一 settled 三行页和 draft 空页逐节点确认外层/内层 owner、标题/back、intake class/五组 ID、table wrapper/class、金额 class、empty-after-table 与 pager 一致，Admin DOM 无任何 Public class；`data-label`、本地化状态及具体空态文案为明确扩展，探针未保存为脚本 |
| AUI-084 | Admin lot submission ID 列表投影 | P2 | 已实施；尾随数字分支浏览器 R1，fallback 为静态证据 | 旧 `submissionID` 只在完整值以数字结尾时去前导零并显示该数字，否则显示字符串最后 8 个字符；旧隔离数据的无数字结尾 UUID 分别显示 `#3d2bf3d0/#fcf495cf/#a404463d` | 修复前 helper 对无尾随数字值回退完整字符串，仅在恰有尾随数字时与旧正则相同；新版隔离数据恰好均以数字结尾，显示 `#7/#51/#7` 是该输入下的正确旧语义，不能把两侧不同 UUID 的可见后缀误判为同输入缺陷 | 精确恢复旧正则语义：仅匹配整个字符串末尾连续数字；无尾随数字时按字符安全截取最后 8 个字符，保留完整值作为 tooltip/可访问投影 | submission UUID/数据库/API 不变；只修正浏览器显示函数，不解析或重写权威标识 | embedded payload `6adef156…` 直接读取新版三条完整 submission UUID，尾随 `7/51/7` 与显示逐条符合旧算法；旧版三条独立 UUID 无尾随数字并逐条显示后八位。当前新版数据库没有 fallback 输入，故该分支由双侧源码静态确认而不伪称同输入运行态 R1；探针未保存为脚本 |
| AUI-085 | Admin lot 改价/撤回/代理 dialog 稳定结构与 owner | P2 | 代理 empty/populated 已浏览器 R1；改价/撤回为静态证据 | 旧改价/撤回 dialog 使用实体唯一 `price-title-{lot_id}` / `withdraw-title-{lot_id}`，改价 `form.admin-auction-reprice` 且 `label[for=price-{lot_id}]`；撤回无 form、确认按钮为普通 button。代理 dialog 由当前行拥有，使用 `aria-label`、旧 Admin table classes 与专属 pager 名称 | 新改价/撤回共用固定 `admin-lots-dialog-title`、统一 `form.admin-lots__dialog-form`，改价 label/input 缺旧 ID；代理 dialog 提升为页面级 owner，表格与空态 class 也改为新 BEM | 恢复实体唯一 ID、改价关联和按动作区分的 form/button 语义；页面级代理遮罩作为焦点/阅读顺序扩展保留，内部 Admin class、空态和专属 pager 名称对齐旧版 | lot command ID、expected version、busy、反馈、刷新及 proxy API/分页不变；全局焦点 owner 继续唯一负责 opener 恢复，不复制旧局部 ref | embedded payload `6adef156…` 下新旧 empty proxy 的 dialog class/aria/title、空态节点、专属 pager、close owner 与焦点恢复逐项一致；新版另以一条真实历史确认 populated wrapper/table 为旧 Admin class、pager 正确且无 Public class，旧隔离库无 populated 对照数据。当前唯一 draft 无 lot，故改价/撤回只具双侧源码静态证据且未提交任何命令；探针未保存为脚本 |
| AUI-086 | Admin Rules success workspace DOM、ID 与可访问 owner | P2 | 已实施并通过双 success 浏览器 R1 | 旧 success/missing workspace 直接位于唯一 `section.admin-rules-page[aria-labelledby=rules-title]` 下；tabs/add/default/form/editor/preview 使用 `admin-rules-tabs`、`form.admin-rules-add`、`label/select#rules-default-locale`、`form.admin-rules-form`、`rules-title-input/rules-markdown/rules-preview-title`，没有第二个 region | 新 `AdminRulesWorkspace` 又包一层 `section.admin-panel.admin-rules[aria-labelledby=admin-rules-page-title]`，但 DOM 中不存在该 ID，形成无名 region；tabs/add/default/form 与字段 ID 全换为新 BEM，默认语言由禁用 select 变成静态 code | 已移除无名嵌套 region，恢复旧稳定 class/ID、add form 和默认语言 select；保留新版候选签名、两阶段校验、revision facts、发布 command ID、busy/notice 与 Markdown 安全预览 | 语言闭集仍是 `en/zh-CN/zh-TW/ja/ko`；不因 DOM 对齐允许任意 locale 发布或取消服务端 normalize/版本 CAS | embedded payload `6adef156…` 与旧版同为 Rules success：唯一 root owner、tablist、add/default/form/editor/preview class，`rules-locale/rules-default-locale/rules-title-input/rules-markdown/rules-preview-title` 和 form owner 逐项一致；双方均无悬空 `aria-labelledby`，新版 revision facts 为明确扩展。短文案残差转 AUI-088，探针未保存为脚本 |
| AUI-087 | Admin Config/Rules 页面说明文案 | P3 | 已实施并通过 Config/Rules 五语言浏览器 R1 | 旧 Config/Rules 五语言分别为：en “Edit the restart-loaded runtime configuration. Secrets and deployment settings are not exposed here.” / “Edit each language as Markdown and publish it to the public rules page immediately.”；zh-CN “编辑重启时加载的运行时配置；密钥与部署设置不会在此处暴露。” / “分别以 Markdown 编辑各语言版本，保存后立即发布到公开规则页面。”；zh-TW “編輯重新啟動時載入的執行階段設定；密鑰與部署設定不會在此處顯示。” / “分別以 Markdown 編輯各語言版本，儲存後立即發佈到公開規則頁面。”；ja “再起動時に読み込まれるランタイム設定を編集します。シークレットとデプロイ設定は表示されません。” / “各言語を Markdown で編集し、公開ルールページへ即時反映します。”；ko “재시작 시 불러오는 런타임 설정을 편집합니다. 비밀값과 배포 설정은 표시하지 않습니다.” / “각 언어를 Markdown으로 편집하고 공개 규칙 페이지에 즉시 게시합니다.” | 新 Config 强调 JSON/共享非敏感文档和重启，Rules 强调带版本竞价规则及发布前校验；首次五语言移植还把韩文 Config 的主题助词 `은` 误写成宾格助词 `을`，其余主说明已收口 | 恢复旧无缺陷的五语言主说明；新增约束继续由字段 hint、事实区和运维文档承载，不替换页面定位文案；韩文必须以真实运行态旧资源为准修正助词 | 不改变配置签名/重启语义、规则校验/发布语义或敏感字段隔离；错误 code 的更精确映射不属于本项 | embedded Admin payload `deba69699c2a7816bc89f8a9daaf9ebdf236a354af25e58b9e18c5e437029744` 下，新旧 Config/Rules 两页动态切换全部五语言并逐项读取说明、picker、`hv.lang` 与 `<html lang>`，十组文案全部逐字一致；Config ko 已确认同为 `비밀값과 배포 설정은 표시하지 않습니다.`，最后恢复 zh-CN。探针未保存为脚本 |
| AUI-088 | Admin Rules workspace 字段与动作短文案 | P3 | 五语言已实施并通过双 success 浏览器 R1 | 旧 workspace 的 `languages/addLanguage/defaultLanguage/documentTitle/markdown/validate/save` 依次为：en `Rule languages / Add language / Fallback language / Page title / Markdown content / Validate / Save and publish`；zh-CN `规则语言 / 新增语言 / 默认回退语言 / 页面标题 / Markdown 内容 / 校验 / 保存并发布`；zh-TW `規則語言 / 新增語言 / 預設後備語言 / 頁面標題 / Markdown 內容 / 驗證 / 儲存並發佈`；ja `ルールの言語 / 言語を追加 / フォールバック言語 / ページタイトル / Markdown 内容 / 検証 / 保存して公開`；ko `규칙 언어 / 언어 추가 / 대체 언어 / 페이지 제목 / Markdown 내용 / 검증 / 저장 및 게시` | 新简中相同位置曾为“添加语言”“默认语言”“标题”“Markdown 正文”“校验候选”“发布规则”；其他语言也使用新版术语；字段/动作含义相近但破坏旧 WebUI 逐字一致 | 已逐语言恢复 workspace 定位性字段名和按钮短文案；`create/publish` 同映射旧 `save`，中文 placeholder 也恢复旧无冒号形式；新版 revision facts、冲突/校验反馈等新增事实文案作为扩展保留 | 不改变先校验后发布、dirty signature、revision CAS、command ID、busy 禁用和 locale 闭集；只调整 FTL 投影 | embedded payload `7c8bc6ae…` 在新旧 Rules success 页动态切换 `en/zh-CN/zh-TW/ja/ko`，逐项读取语言 aria、add label/placeholder/button、default/title/Markdown/preview 与三枚动作按钮，五组全部逐字一致；每次 `hv.lang`、picker 与 `<html lang>` 同步，最后恢复 zh-CN。探针未保存为脚本 |
| AUI-089 | Admin Auction 创建 dialog 关闭后的键盘焦点恢复 | P2 | 已实施并通过点击/Escape 浏览器 R1 | 旧版点击关闭或按 Escape 后，焦点恢复到 `.admin-auction-create-opener`“创建拍卖”按钮 | 新版关闭后 dialog 已卸载，但 `document.activeElement` 落到 `body`，键盘用户失去当前位置；原因是 opener 与 dialog 互斥渲染，现有唯一焦点恢复 owner 记录的旧节点已断开 | 保持 opener 节点跨 dialog 生命周期连接，让 `auction-web-shared` 已有全局 trap 统一记录并恢复；按钮关闭与 Escape 继续共用 open state，busy 时仍禁止关闭，不增加第二套 focus effect | 不改变 create form、命令 ID、输入状态和请求；不复制多个互相竞争的焦点 owner，也不在 mutation busy 期间抢焦点 | embedded payload `7c8bc6ae…` 与旧版分别通过真实 close click 和 DevTools Escape 关闭，四次 `activeElement` 均为同一 `.admin-auction-create-opener`，dialog 均已卸载；未提交创建命令，探针未保存为脚本 |
| AUI-090 | Admin Credentials 日/韩表头术语 | P3 | 已实施并通过五语言浏览器 R1 | 旧 Credentials 日语表头使用 `ステータス`、`インポート者`，韩语使用 `가져온 사람`、`동작`；其余五语言表头以及日语“操作”、韩语“상태”均与当前资源一致 | 新日语对应为 `状態`、`インポート担当者`，新韩语对应为 `가져온 관리자`、`작업`；字段 owner 和 wire 未变，但可见术语不再逐字一致 | 只把四个 Fluent 值恢复为旧版已本地化文案；表格结构、字段顺序、状态值和动作行为保持不变 | 不回迁旧版缺翻译造成的英文 fallback；本项四个值都有旧版真实本地化资源和运行态证据，因此按逐字兼容处理 | embedded Admin payload `ef125ac4fb5e2289c85a4314e0ebd82b77a6e7662cd567e93bacfcbce9321d6f` 下，新旧 `/cookies` 空态动态切换 `en/zh-CN/zh-TW/ja/ko` 并逐项读取八个表头，五组全部逐字一致；日语确认为 `ステータス/インポート者`，韩语确认为 `가져온 사람/동작`，picker、`hv.lang` 与 `<html lang>` 每次同步，最后恢复 zh-CN。探针未保存为脚本 |
| AUI-091 | Admin Outbox 筛选动作与五语言表格术语 | P3 | 已实施并通过五语言浏览器 R1 | 旧高级筛选提交为 en `Confirm`、zh-CN `确认`、zh-TW/ja `確認`、ko `확인`；繁中 Stream 使用“串流”，日语 Status 使用“ステータス”，韩语 Consumer/Actions 使用“소비자/동작”，英文 lag 最新列为 `Latest` | 新对应为 `Apply filters/应用筛选/套用篩選/フィルターを適用/필터 적용`、繁中“流”、日语“状態”、韩语“컨슈머/작업”及英文 `Latest sequence`；字段和筛选行为未变，但操作文案与旧已本地化术语不一致 | 恢复 Outbox 专属筛选/表头键；韩语共用 Actions 键同时服务 Delivery，并已与两页旧资源一起核对。AUI-075 的专属 failure/lag 空态文案继续保留，不混入本项 | 不改变筛选 query、failure/lag 字段、replay、分页或数据值；只调整 FTL 投影，数据库不同造成的 lag 行数/序号不计作文案差异 | embedded Admin payload `01876a44473c871335cee59b5b5fc2f361f1e3fecb0771aa4b8a22cde25063ac` 下，新旧 failure empty、lag populated 页面动态切换五语言，status label、三字段高级筛选、Confirm、12/5 列表头与两种 pager 全部逐字一致；唯一可见文案差异是 AUI-075 专属空态，lag 行数/数值来自不同隔离库。语言三态每次同步并恢复 zh-CN，探针未保存为脚本 |
| AUI-092 | Admin Delivery COD/Status/Actions 术语与分页名称 | P3 | 已实施并通过五语言 empty 浏览器 R1 | 旧 Delivery 的 COD 表头五语言都固定为 `COD`，日语 Status 为“ステータス”，韩语 Actions 为“동작”；pager `aria-label` 使用各语言页面标题 `Delivery queue/投递队列/投遞佇列/送信キュー/발송 대기열` | 新把 COD 翻译为中/日/韩本地词，日语使用“状態”、韩语使用“작업”，pager 使用新通用“Delivery pagination/投递分页…”；八列字段和页码行为本身未变 | 恢复旧缩写、日/韩术语与页面专属 pager 名称；新版比旧全局“暂无结果”更准确的五语言 Delivery 专属空态继续作为信息架构修复保留并显式登记 | 不改变 Delivery wire、筛选、排序、动作、分页参数或 AUI-021 已登记的 `prepared` 安全显示；专属 filter label、`prepared` 选项与状态选项本地化按 AUI-021/AUI-073 保留，空态仍保持 table→message→pager 顺序 | embedded Admin payload `01876a44473c871335cee59b5b5fc2f361f1e3fecb0771aa4b8a22cde25063ac` 下，新旧 Delivery empty 页面动态切换五语言，八列表头与 pager 名称全部逐字一致，COD 五组固定、日语“ステータス”、韩语“동작”均已确认；专属筛选/完整本地化选项和专属 empty 为登记扩展。双方均为零行，未提交 Delivery 命令，语言三态最后恢复 zh-CN；探针未保存为脚本 |
| AUD-001 | Public 竞价命令审计事实 | P0 | 外部三命令、Final 与独立 freeze 已修复并 R1；结算待收口 | 旧普通出价每个命令只追加一条 `lot.bid_accepted`；代理创建与降低分别只追加一条 `lot.proxy_authorization_created`、`lot.proxy_authorization_lowered`。旧 `repository.go::auditRecord` 冻结了各 action 的 actor/source/entity 分类，proxy metadata 记录授权、lot、mode/usage 或新上限；即使命令产生多条公开事实也仍是一个命令级审计事实 | 修复前 PostgreSQL/SQLite competition audit 按每条公开 bid 写 `place_bid`，再按每条授权转换写 `proxy_transition`；现由应用端口携带命令级审计上下文，两后端先原子写账本、前置资格转换、bid/realtime/lot，再只为主外部命令追加一条旧 action。旧 proxy action 未被 helper 归为 Public 的历史分类也按原样保留为 `admin/admin/auction`，`request_id` 固定为空 | bids、代理转换和 realtime 事实继续逐项追加；前置失格冻结不再额外重复一条 Admin 审计。Admin action/source/entity 筛选和计数恢复旧外部命令语义；final 子命令已由 AUD-002 闭合，独立失格冻结已由 BID-007 闭合，余下缺口是结算关闭 | 新增后端共用 `competition_audit.rs` 构造旧 metadata：普通竞价恢复含全部 facts 的完整结果及 UUID byte-array 形状；代理创建恢复 `authorization_id/lot_id/mode/usage_number/public_fact_count`，降低恢复 `authorization_id/lot_id/maximum_amount`。slow tick 继续走既有 Worker 专属审计，final 仍留在独立生命周期路径 | Application/Storage all-features check/Clippy、PostgreSQL 与 SQLite 独立 feature check 通过；同源 fast 反击命令首次/重放均 200，旧新各仅一条 `lot.bid_accepted`，两条 facts 的金额 `[67000,71000]`、机制 `[ordinary,proxy]`、版本 `[6,7]` 相同，metadata lot ID 均为 array。降低与新代理创建首次/重放也均 200，旧新各仅一条对应 action，actor/source/entity、NULL request_id、metadata 键和值逐项相同；失格前置冻结场景双方也只写一条 `lot.bid_accepted` 且 metadata 形状一致；仍待结算关闭独立 R1，Final 子命令见 AUD-002 |
| AUD-002 | 拍卖生命周期与 Final 代理审计 | P0 | 已修复，生命周期/Final 隔离 R1 | 旧 Preview/Open/Cancel/Final 父审计 action 分别为 `auction.preview_open`、`auction.open`、`auction.cancel`、`auction.enter_final_phase`，统一沿用 `admin/admin/auction` 分类；HTTP `request_id` 只并入 metadata，审计列固定为 NULL。metadata 分别冻结预览条目/人工标记、原始与实际开拍时间/trigger、取消前状态/返还义务数，以及 Final 的拍卖与版本。Final 对每个受影响 slow/失格代理另追加且只追加一条 `lot.proxy_final_transition`，metadata 为旧 `finalProxyCommandResult` 完整闭集 | 修复前父审计使用账本 kind `auction_preview/open/cancel/final_phase` 作为 action，把 `request_id` 写入列而 metadata 只含通用 `command_id/action/status/state_version`；Final 子计划还复用通用 competition audit，按 bid 和 proxy transition 分别追加 `place_bid`/`proxy_transition`，与旧命令级子审计 owner 不同 | 父 action、筛选分类、request 关联位置和 action 专属 metadata 都会改变 Admin 审计页及历史取证；Final 的额外审计会改变计数并丢失旧完整 final lot/facts 快照。新版内部 `final_fast` 继续作为已声明建模差异，但旧审计兼容边界必须映射为 `mode=fast/status=active` | Application 生命周期事务保留锁前上下文和动作计划，并由两后端返回取消写入摘要；共用映射生成旧父 metadata。Final 子计划只写权威 bid/proxy/realtime/lot 事实，再单独写一条旧子审计；不改变命令账本、内部 `final_fast`、Public 账户兼容映射或实时事件 | Application/Storage all-features check/Clippy、PostgreSQL 与 SQLite 独立 feature check、Admin release 构建均通过。隔离新旧 PostgreSQL 的 Preview、Open、Draft/Preview Cancel、无代理/slow/失格 Final 首次调用均为同状态码和版本；父 action、分类、NULL request_id、metadata、实时事件及取消后的 `withdrawn`/返还义务数逐项一致。无代理 Final 各仅一条父审计；slow/失格 Final 各为一父一子，slow 的单条 fact 与旧完整快照对应，失格无 fact 时两边均为 JSON `null`。slow Final 同 ID 重放均为 200 且事实数不变；旧版 Cancel 同 ID 重放实测 503，新版按项目幂等不变量返回原 200 且不重复写入，该旧缺陷不回迁；全过程未编写或运行测试用例 |
| BID-015 | Public 竞价/代理命令账本的滚动升级兼容 | P0 | 已实施，普通竞价与代理创建/降低双向共享库 R1 | 旧 `command.Hash` 分别对 `{LotID,BidderID,Amount}`、`{LotID,UserID,Mode,MaximumAmount}`、`{LotID,UserID,AuthorizationID,NewMaximum}` 使用 Go `encoding/json` canonical 字节并 SHA-256；账本 kind 固定为 `place_bid`、`create_proxy_authorization`、`lower_proxy_authorization`，摘要不含 `command_id`。旧 `SaveCommand` 固定写入大写 `result_code=OK` 和严格 `BidResult/ProxyResult`，UUID 以 `[16]byte` 数组编码；降低结果的 Go nil `Facts` slice 固定为 JSON `null`。bid fact/代理授权 ID 使用 `SHA-256(namespace || command_id原始16字节 || 十进制序号)` 截断并设置 UUID 版本/变体位 | 新 HTTP 与论坛命令共用 application 的旧 canonical helper，三种摘要/kind、`OK + BidResult/ProxyResult`、逐字 `DeterministicID` 和既有 `accepted + CompetitionPlan` 只读兼容均已恢复。代理降低在同一竞价事务读取完整转换历史；账本防腐 DTO 只在该旧 wire 上把必有字段 `facts:null` 映射为空事实，并让新版无事实结果仍输出 `null` | 滚动升级期间普通竞价、代理创建和代理降低可由任一版本首次写入、另一版本无副作用重放；完整授权、quota、转换历史、bid/realtime/audit、lot 版本和冲突语义均保持一致。成功响应的时间 offset/亚微秒精度差异仍独立归 API-035，不能据此宣称响应 bytes 等价 | 继续拒绝账本字段缺失、未知字段、非空错误事实、错误 kind/hash/result code、损坏转换历史和不匹配确定性 ID；保持 HTTP、论坛、应用与两后端只通过同一 owner，不把 `null` 兼容扩散到 HTTP DTO 或领域模型 | Application 定向 check/`-D warnings` Clippy、rustfmt、diff whitespace 与 embedded release 构建通过。BID-016 后普通竞价共享 PostgreSQL 双向首次/重放均 200、异 payload 均 409 且快照不变。两个独立克隆中的代理旧写→新重放与新写→旧重放，创建和降低各为 200/200；创建账本均为 `create_proxy_authorization/OK`、32 字节摘要、`facts` 数组、一条 bid/转换/realtime/audit，降低均为 `lower_proxy_authorization/OK`、32 字节摘要、`facts:null`、零 bid/realtime、一条转换/audit。授权从 active v1 降低到 active v2、完整历史由 1→2，lot 维持 1000/v3；四组合法异 payload 双端均 409 且所有快照不变，非时间业务响应删除 `request_id` 后一致。临时库、角色、配置和 8094-8096 均清理，常驻 8090-8093 根路径仍为 200 |
| BID-016 | 旧 v28 二进制写新版 `bids.effective_end_at` schema | P0 | 已实施，迁移/旧首笔共享库 R1 | 旧 `UpdateLotCompetitionFact` 会先把每条事实的 `EffectiveEnd` 投影到已锁定 lot，再调用 `InsertCompetitionBid`；旧 INSERT 的固定列只有 id/command/fact/lot/bidder/amount/mechanism/state version/created time，不包含后来新增的 `effective_end_at` | PostgreSQL `000019_legacy_bid_effective_end_compat.up.sql` 新增 `BEFORE INSERT` trigger；只对 `NEW.effective_end_at IS NULL` 执行，并要求 lot 的 current price、leader、state version 与新 bid 逐字段相等后复制 lot 的非空结束时间 | 旧 Public 在已经迁移的新库上不再因遗漏列 503；兼容分支同时验证旧事务确实已经投影同一事实，不能把任意缺列 INSERT 悄悄补成合法事实 | 保持追加 migration、列 NOT NULL 和新版显式写入；匹配不到同一 lot 事实或结束时间为空时以 `23514` 失败关闭，不修改 SQLite Lite | PostgreSQL 18.6 隔离克隆已证明 migration object 存在、旧列清单补值、显式新版值原样通过、无法解析时失败关闭；项目自身 `auctionctl migrate --features postgres` 写入 SQLx v19 成功记录，重复执行幂等。共享库旧 Public 首笔随后为 200，bid 结束时间与 lot 相等；所有临时数据库、配置和端口均已清理 |
| API-035 | Public 竞价命令 RFC3339 词法、精度与幂等响应 | P2 | 已实施，六向量与代理命令重放 R1 | 旧 Go `time.Time` 通过 RFC3339Nano 输出，保留进程本地 location 的 offset、裁剪尾随零并可保留纳秒；旧命令账本还会保留 payload 中的 offset/亚微秒词法，而 PostgreSQL 关系表只有微秒精度 | application 兼容边界现以 `CompetitionWireTimestamp` 同时保存原 wire 与 UTC instant；Public 数据库/新生成时间统一由 Go-compatible formatter 裁零并使用 `Z`，Admin 仍使用原固定六位微秒格式。PostgreSQL/SQLite 新写竞价 realtime payload 也复用同一 Public formatter | 旧账本的 offset/纳秒可以在成功重放中逐字返回，普通目录、lot、账户、历史、补偿、forum 和 realtime envelope 不再人为增加 `.000000` 或保留无意义尾零；WebUI `Date` instant、金额、领先者和状态均不变，也不伪造数据库已经丢失的纳秒 | wire 类型只进入 `legacy_ledger`、Public command outcome 与 HTTP serializer；相等、排序及领域一致性全部比较 UTC instant。关系模型和领域仍使用 `DateTime<Utc>`，Admin formatter 保持隔离，实时 payload writer 只改变公开时间字符串 | 最新 PostgreSQL release 在 8094 对既有 create/lower 账本重放均为 200，原有 transition 纳秒未被截断。受控把同 instant 改写为 `+02:00` 后，create 的整秒 effective end 与纳秒 transition、lower 的微秒/纳秒 transition 均逐字回放；两行随后按整行 MD5 精确恢复。当前拍卖 `opens_at` 三个关系时间向量分别输出整秒 `Z`、`.1234Z`、`.123456Z`，原值精确恢复。Public 全调用面回扫、Application/HTTP 及双后端 Storage check/`-D warnings` Clippy、embedded PostgreSQL release 构建通过；直接让新版读取未迁移旧克隆因缺 `lots.leader_proxy_last` 在账本前失败，该无效样本未计入证据。未编写或运行测试用例 |
| RT-006 | Public 竞价实时 `mechanism` 枚举与服务端防火墙 | P1 | 服务端已修复；非法类型/枚举 catch-up 与真实 ordinary/proxy WS 均 R1 | 旧持久事件、public registry 与浏览器都只接受 `lot.bid_accepted.mechanism=ordinary/proxy`，服务端 public projection 已拒绝错误类型或枚举 | 新 server/storage 正常只产生 `ordinary/proxy`，Dioxus decoder 与 application `PublicRealtimeEvent::validate` 现共同恢复同一闭集；PostgreSQL/SQLite catch-up、Hub/WebSocket 和 relay 都复用应用层 owner，不依赖某一个浏览器兜底 | 畸形或损坏持久事件会在服务端投影边界失败关闭并要求权威 refetch，不再作为公开 payload 泄漏；正常事件中的 mechanism、金额、fact/state version 与事件顺序恢复旧语义 | 应用层在精确字段集合之后校验 `mechanism` 必须是字符串 `ordinary/proxy`，浏览器保留二次防线；代理授权 mode 的 `fast/slow` 仍属于另一契约，未混入 bid mechanism。event ID、既有 stream 基线和 API-035 时间词法不在本项强行归一化 | 双仓 CodeGraph、静态门禁与两轮损坏事件克隆均已通过。本轮在全新隔离克隆中以同一 challenger 普通出价 60000 触发 active fast proxy 63000；新旧真实 WS 都只按 `ordinary/fact1/v4 → proxy/fact2/v5` 发出两帧，schema v2、auction、确定性 bid ID 及字段闭集一致。首次/同 ID 重放均 200；重放观察 10 秒无新增文本帧，数据库各保持 1 command、2 bids、2 realtime、lot 63000/v5 与 active proxy usage 1。临时进程、数据库和含登录密钥目录均已不可恢复地清理 |
| API-032 | Public 写请求的 middleware/校验优先级 | P1 | 顺序/大 body 已修复并组合 R1；query 词法转 API-036，限流/Origin 启用态待证 | 旧 Public `Protect` 顺序为 rate limit → Origin → session/authentication → handler；handler 内再做 query/path/body。未认证组合输入不会先泄漏 JSON/content-type/path 分类 | 三条 Public competition mutation 已移除 route-level 预解析 body；handler 先执行 rate limit → Origin → authentication，再依次校验 query、path、Content-Type/body。Admin 守卫顺序和其余 JSON 路由不随本项改写 | 同一组合错误重新由旧 owner 决定 status/code，未认证请求不再先观察 JSON、媒体类型或 path 分类；限流仍在最外层消费请求 | 只对三条 Public mutation 延迟读取 body，并保留 4 KiB 上限、严格未知字段、单 JSON 值、Origin 与 session 安全边界；query 继续先于 path；`invalid_query` 的自然语言逐字值由 API-036 独立收口 | 新旧一次性 HTTP 矩阵已覆盖未认证叠加坏 query/path/body、认证后的 query→path→Content-Type/JSON/大 body 顺序。探针曾发现大于 4 KiB 的首字节语法错误被新版过早报 413，而旧 `MaxBytesReader + Decoder` 先报 400；现以有界前缀恢复旧顺序并复测“大 malformed 前缀=400、大合法已知字段=413”一致。当前配置关闭 Origin allowlist，坏 Origin 只证明双方仍由认证先返回 401；启用态与 rate-limit 耗尽仍待独立 R1 |
| API-033 | Public 命令 DTO 的 Go 零值与规范 UUID 输入语义 | P1 | 已修复，35 组输入矩阵 R1 | 旧 Go JSON 对缺失或 scalar `null` 先得到字符串/整数零值，再由命令专属校验输出稳定错误；UUID 只接受规范小写带连字符且非 nil | Public 三命令防腐 DTO 现只对字符串/整数 scalar 的缺失或 `null` 恢复 Go 零值，错误类型、未知字段和第二个 JSON 值仍严格拒绝；command/path/authorization ID 统一在 route-specific mapper 使用规范小写、带连字符、非 nil 的 `parse_public_uuid` | 缺失/null 重新进入命令专属 `invalid_*` 分类，非规范 UUID 不再由宽松 Rust parser 接受，也不会进入摘要或事务 | 零值兼容只存在于旧 v1 HTTP DTO；领域金额、带类型 ID、请求摘要、事务 fence 和浏览器生成的 UUID 均未放宽 | 普通出价 16 组、代理创建 10 组、代理降低 9 组一次性 HTTP 输入矩阵已覆盖字段缺失/null/错误类型、nil/大写/无连字符/URN/braced/malformed UUID、未知字段、第二 JSON 值及负数/未对齐金额；新旧 35 组 status/code/message 全部一致，均未进入成功写事务。Contracts/HTTP/Public 定向编译与最新版 HTTP `-D warnings` Clippy 已通过 |
| API-034 | Public 竞价依赖失败、timeout 与提交后读取窗口 | P1 | 依赖失败、锁超时与恢复均 R1；提交后读取窗口已闭合 | 旧 Public mutation 的普通依赖失败统一为 503 `temporarily_unavailable`，可识别 `context deadline` 为 504 `timeout`；代理成功响应直接使用命令结果，不在提交后新增账户投影读取失败窗口 | Competition command port 返回应用层 typed outcome；旧 `OK + ProxyResult`、新写结果及历史 `accepted + CompetitionPlan` 都在同一事务内冻结授权、quota、完整转换历史和 terminal reason，HTTP 直接映射该结果。普通 session/commands/repository/serialization 失败统一为 503，外层 `HttpBoundary` 拥有 15 秒 504；storage 即使抹平内层 deadline 原因，外层仍能取消等待中的 SQLx future | 已关闭“事实提交成功但事后账户读取失败导致 HTTP 报错”的窗口；真实行锁证明外层 timeout 后 transaction drop/旧显式 rollback 都不留下命令、bid、audit、realtime、代理转换或聚合版本，依赖恢复后同一服务进程立即可用 | 历史 `accepted` 重放只在同一只读事务按授权版本截取完整 transition，并读取冻结 policy quota，不重新执行写入、不使用当前 lifecycle/time gate；typed 400/403/404/409 与 501 保持原分类。坏账本 wire 分类和安全强化独立由 API-037 收口 | disposable 双克隆以同一合法 lower 请求持有 lot 行锁，双方分别在 15.003/15.005 秒返回 `504 timeout/请求处理超时`；锁释放后新命令均为 0，账本、bid、代理转换、audit、realtime 计数及 lot/auth 哈希全部不变，残留 sleeper 为 0。把 `public_sessions` 或 `auction_commands` 临时改为应用账号不可读时，双方均为 `503 temporarily_unavailable`，恢复 owner 后 `/me/bids` 或同 ID 重放立即 200，业务状态仍为基线。首轮因探针异步启动偏差让旧锁提前释放的样本已判无效，并从权威源重新克隆后用单脚本同步复测；未编写或运行测试用例 |
| API-036 | Public 竞价 query 与代理领域错误 message 词法 | P2 | query 已通过三路 R1；三个代理领域状态已静态修复，fixture 待补 | 旧三条 mutation 的非空 query 固定为 `invalid_query/查询参数无效`；`proxy_outbid_limit`、`proxy_disabled`、`proxy_slow_final` 分别为“代理出价次数已达上限”“代理竞价已禁用”“最终阶段不允许新建慢速代理” | 新版 code/status 原本相同但 message 使用路由专属说明和近义代理文案；六处 wire message 现已恢复旧稳定值，领域判断、code/status 和 WebUI 本地化未改变 | 客户端主要按 code 本地化，业务状态不变；但 HTTP JSON message 是冻结 v1 可观察 wire，必须逐字恢复，不能用语义近似冒充一致 | 六处只改稳定中文 message，不改变 middleware 顺序、错误 code/status、领域判断、FTL 或 WebUI 本地化；query 顺序仍由 API-032 拥有。三个代理领域错误仍需各自可构造状态，不能用源码值冒充运行态覆盖 | disposable session 对 bid/create-proxy/lower-proxy 三条路由分别提交非空 `?x=1`，新版和旧版六个请求全部返回 `400 invalid_query/查询参数无效`；命令、bid、代理转换、audit、realtime、lot 与 auth 均保持基线。旧底层 sentinel 的“代理被超价次数已达上限”不是最终 HTTP 文案，矩阵仍以 `writeCommandError` 的“代理出价次数已达上限”为准。auction-http all-targets/all-features check、`-D warnings` Clippy、rustfmt 与 diff whitespace 通过；三个领域状态 fixture 待分项补证，未编写或运行测试用例 |
| API-037 | Public 损坏竞价账本的错误分类与防御强度 | P1 | 故障矩阵 R1；非成功结果码已修复，强身份校验登记为安全加固 | 旧 `LoadCommand` 在 hash 相同后把任意非 `OK` 结果码归为 409 `command_conflict`，再严格解码 payload；它不比较账本行的 `command_kind/aggregate_id`，因此这两列被篡改但 payload 仍合法时继续 200 重放 | 新版继续先用 `command_kind + aggregate_id + request_hash` 完整匹配，任一漂移即 409；匹配后未知 `result_code` 已从 503 改为旧 409，空/损坏 payload 仍同旧为 503 | 未知结果码的 wire 差异已关闭；取消 kind/aggregate 校验会让内部损坏行被 200 接受，违反 fail-closed 与命令身份不变量，因此明确保留新版 409 防线，登记为相对旧版的有意安全强化 | 只把完整请求身份已经匹配后的未知结果分类映射为 `CommandLedgerError::Conflict`；不放宽 `StoredCommand::matches_request`，不把空/损坏 payload 伪装成冲突，也不改变成功重放、事务和账本 schema | 同一 lower 命令基线重放双方 200 且零副作用。仅改 `result_code=CORRUPT` 时修复前新版 503、旧版 409，修复后新版同为 409；仅改 payload 为 `{}` 时双方 503；仅改 kind 或 aggregate 时新版均 409、旧版均 200，实证支持保留强校验。每轮都由 trap 恢复整行并逐字比对，最终结果码为 `OK`，命令/bid/代理转换/audit/realtime 计数和 lot/auth 哈希不变；Application/HTTP 定向 check、`-D warnings` Clippy、rustfmt 与 diff whitespace 通过，未编写或运行测试用例 |
| WRK-007 | Recovery action mutation 租约围栏与有限重试 | P0 | 已实施，有效释放/重领 stale fence R1；刚过期/冲突重试待补 | 旧 pending→prepared、prepared→enqueued、release 和三种 terminal UPDATE 都匹配 state/version/token/fencing 且要求 `claim_expires_at > clock_timestamp()`；repository 以 Serializable 最多三次执行；claim 增加 state/fencing 而 release 才增加 retry count | 新 PostgreSQL action owner 现共用最多三次 Serializable wrapper，只重试 `40001/40P01`；claim/prepare/activate/release/terminal 全部使用数据库时钟和精确 fence，claim/release 的 state、fencing、retry 计数语义也已恢复旧版 | 有效租约 release 与被重新领取后的旧 owner 零终态写入已经运行态确认；仍需单独在数据库时钟边界复现“刚过期但未被替换”以及强制 `40001/40P01`，才能关闭完整租约/有限重试证据 | wrapper 与 typed error 位于 `recovery_scheduler/scheduler/actions.rs`，各 mutation 在 `actions/claim.rs`、`delivery/prepare.rs`、`delivery/activation.rs`、`resolution.rs` 复用；业务/fence 错误不重试 | 隔离 PostgreSQL v19 克隆中，pending observation 从 state 1 经 claim 2 到 release 3，fencing=1、retry=1、claim 清空且 next attempt 后移。另一次真实 Worker 原 claim 为 state 2/fence 1，权威重领后为 state 3/fence 2；旧 owner 收到“终态 claim 已失效”，action/父 case/Recovery 审计均零变化。PostgreSQL/SQLite Storage、Jobs 定向 check、相关 `-D warnings` Clippy、rustfmt 与差异空白已通过；刚过期和强制冲突重试仍待 R1 |
| SET-011 | Recovery action 终态、父 case 投影与审计原子性 | P0 | 已实施，三终态/审计失败回滚 R1 | 旧 `MarkTerminal` 在同一 Serializable 最多三次事务中更新 action，随后按 satisfied/failed/manual 投影 `settlement_recoveries` 的 custody/blocking/manual 字段并追加唯一 `auction.recovery.*` 审计；任一步失败整体回滚 | 新 `resolution.rs` 已成为三种终态的单一事务 owner：精确 action fence/数据库租约后更新 action，按旧规则投影父 recovery，并以 `system/auction-recovery`、旧 action/entity/source/result 与 metadata 追加 audit；delivery observation 与 buyer blacklist 共用该收口 | action、父 case 与审计不再存在分事务成功窗口；证明内容的 canonical identity 仍由独立 `WRK-008` 跟踪，不能据此宣称完整 recovery 终态等价 | satisfied/failed/manual 的 action→case→audit 在同一 Serializable 事务提交，任一步失败整体回滚；审计 ID 跨重试稳定且不重复外部事实 | 隔离 PostgreSQL v19 克隆中，satisfied/failed/manual 三项均由 action state 1→3、fencing=1，父 case state 1→2，并各产生且只产生一条 `auction.recovery.satisfied/failed/manual_reconcile` 审计；再次轮询版本与审计数零增长。只针对第四 fixture 拒绝审计 INSERT 时，终态 action、proof/completed 与父 case 投影全部回滚，action 随后仅由独立 release 变为 enqueued state 3/retry 1，终态审计仍为零。Storage/Jobs 静态门禁通过；canonical proof 字节仍归 WRK-008 |
| WRK-008 | Recovery 外部证明 identity 与 canonical digest | P0 | 已实施，静态门禁通过；证明字节 R1 待补 | 旧 Delivery 成功引用统一为 `mooglemail:{world}:{operation}:{remote-or-source-id}`，take 证明保存后缀并按完整 delivery snapshot 生成 schema-v1 SHA-256；buyer blacklist 证明则按 command/recovery/action/buyer/reason/actor/time 的 schema-v1 result 生成摘要。旧 reminder 同时要求 send 后缀等于 payment obligation ID，但 transport 实际使用 remote mail ID（缺失时为 delivery ID），这是旧基线内部已确认的矛盾 | 新 MoogleMail send 已恢复同一前缀 wire，缺远端 ID 时以后缀 delivery UUID 回退；take/reminder 使用旧 proof kind、后缀与 schema-v1 snapshot，buyer blacklist 从已验证 Users result 生成并传递旧 schema-v1 摘要。共用 canonical helper 复现 Go `encoding/json` 的字段顺序、HTML 安全转义与 SHA-256，blacklist 时间另按 RFC3339Nano 去尾零 | take/blacklist 的静态证明 identity 与摘要字节规则已统一；旧 reminder 的 payment obligation ID 与 transport remote/delivery ID 矛盾按旧基线原样保留，因此该场景仍会进入人工 reconcile，不能误称为新版回归 | `RequestHash::from_legacy_go_json` 成为旧 Go JSON 摘要的单一 owner；notification transport、delivery observation 与 buyer blacklist 分别只提供冻结 payload。不得偷换 reminder identity 来“修好”旧缺陷，若未来修复必须登记为有意扩展 | 双仓 CodeGraph 与逐段主审已确认旧 `externalReference`、`satisfySucceeded` 和两种 digest；Application/Integrations/Jobs、PostgreSQL/SQLite Storage check，相关 `-D warnings` Clippy、rustfmt 与差异空白均通过。仍需固定 snapshot 逐字核对 reference/proof/digest，并单列 reminder 旧缺陷运行态行为 |
| WRK-009 | Recovery enqueued 未完成观察的持久错误码 | P1 | 已实施，pending 释放 R1 | 旧 delivery 为 pending/claimed/sending 时释放 action，并持久化公开闭集 `recovery_evidence_not_observed` | 新 Worker 已直接恢复持久化 `recovery_evidence_not_observed`；release 的 retry count、state version、next attempt 与租约 fence 由 WRK-007 的同一 owner 保证，HTTP 对旧有新版私有码的只读兼容映射保留 | 新产生的数据库事实、运维扫描与跨版本接管不再扩大内部错误码闭集；既有 `recovery_delivery_pending` 行仍可安全展示 | Worker observation 只选择旧公开 code，Storage release 继续负责原子计数、调度时间和清除 claim；pending 分支不得生成终态审计 | 隔离 PostgreSQL v19 克隆的 claimed delivery 被真实 Worker 观察后，action 固定持久化 `recovery_evidence_not_observed`，state 1→claim 2→release 3、fencing=1、retry=1，next attempt 后移、claim 清空；父 case 保持 state 1 且终态审计为零，持续运行期间未立即重复领取。双仓静态对应与 Application/Jobs/Storage 门禁均已通过 |
| WRK-010 | 论坛回复扫描状态闭集 | P1 | 静态修复，待隔离扫描 R1 | 旧 reply Worker 只扫描 `auction.status IN ('live','final')`，并只把 `lot.status IN ('live','sold')` 建入映射 | 新版 topic query 已收窄为 live/final，lot query 已收窄为 live/sold；parser、游标和外部通知 owner 未改动 | preview/settling/settled 场次不再进入回复解析，withdrawn/unsold 等 lot 也不再进入消息映射；真实扫描、游标推进和外部通知仍需隔离运行态证明 | 保持两个 SQL 状态闭集与旧版一致，不扩张 parser、游标、账本或外部写 owner | 双仓 CodeGraph 已逐 query 与调用链确认；PostgreSQL/SQLite Storage 及 Jobs 的 locked check、对应 `-D warnings` Clippy、rustfmt 与差异空白门禁通过。未启动 Worker 或外部通知，因此仍待隔离扫描 R1 |
| WRK-011 | 论坛通知 owner 未装配的命令账本时序 | P1 | 静态修复，待可恢复配置 R1 | 旧 Worker 在 notifier 未装配时先返回，不生成稳定命令或终态账本；配置恢复后同一回复可再次处理 | notifier 可用性检查已前移到命令 ID 生成和 `begin_command` 之前；真正已经开始的 pm/both 命令仍按原终态规则收口 | 暂时关闭或未装配 MoogleMail 不再永久消耗稳定命令 ID，配置恢复后同一回复仍可处理；配置恢复的真实重领、通知和账本形状尚未运行证明 | 保持 unavailable 早返回位于 ledger begin 前；一旦命令已开始，pm/both 的成功、拒绝和未知结果仍必须按既有持久终态收口 | 双仓调用链已确认；Jobs locked check、`-D warnings` Clippy、rustfmt 与差异空白门禁通过。未装配→恢复配置的真实 Worker R1 尚缺 |
| WRK-012 | 论坛回帖旧命令账本接管 | P1 | 静态修复，待 canonical 扫描 R1 | canonical v28 使用旧版稳定命令 ID，并以 auction/thread/post/line/lot/kind 自然键唯一；当前算法为相同命令生成不同 UUID | PostgreSQL `begin_command` 现同时捕获 ID/自然键冲突，要求唯一记录和全部业务负载一致后返回实际既有 ID；Worker 后续竞价、通知、attempt 与完成写入统一复用该 ID | 只按新 ID 插入会被旧自然键唯一约束持续拒绝，使每轮全主题扫描在历史首条命令处失败并阻断后续新回复；绕过账本又会重复执行 | 自然键与完整负载一致才兼容复用；命令 ID 与自然键命中两行、负载漂移或状态越界均失败关闭 | Application/Storage/Jobs 定向 check 已通过；仍需 canonical 旧账本下历史行跳过、新回复继续、通知补偿和双身份冲突的隔离 R1 |
| API-038 | Admin 拍卖/lot/users 浏览器成功响应 decoder | P1 | 静态修复，待坏响应 R1 | 旧 Admin Web 对 auction、capabilities、pagination、lot、users、blacklist history 和命令结果逐层使用 route-specific `only(...)`，并校验规范 UUID、JS safe integer、时间、状态、排序、归属及 nullable/optional 组合 | 三组 API 现分别使用 route-specific raw DTO 与集中 validator：业务 data 拒绝未知/缺失字段，恢复规范 UUID、浏览器 `Date.parse`、safe integer、状态/价格/版本、取消/slow/submission 联动、auction-lot 归属和黑名单历史计数/倒序；验证后才转换为页面模型。外层 envelope 继续允许旧兼容扩展 | 损坏或原地扩张的 200 响应会在浏览器边界失败关闭，不再被页面当作权威状态展示或用于下一次管理命令；API-010 继续只拥有服务端正常输出 | 为三组路由建立专用 raw DTO 与集中 validator；外层 envelope 保持旧兼容扩展，业务 data 内层恢复旧字段闭集和可达语义，最后才转换为现有页面模型 | 双仓 decoder、Contracts DTO 与调用点已逐项复核；Admin 原生/WASM check、Clippy `-D warnings`、rustfmt 与仓库策略门禁通过。仍待未知/缺失字段、非法 UUID/时间/组合及合法边界的一次性受控 R1 |
| API-039 | Admin session 浏览器协议与匿名判定 | P0 | 静态修复，待会话故障 R1 | 旧 session 成功必须 `authenticated===true`、actor/CSRF 非空且两个期限可由 `Date.parse` 解析；只有 HTTP 401 且合法错误 code 为 `unauthenticated` 才进入匿名。logout 成功还要求 `data===null` | 会话成功现严格校验认证标志、actor、CSRF 与两个时间；401 先经过旧兼容错误 envelope，只有 `unauthenticated` 返回匿名，其它 code 或损坏正文失败关闭；logout 成功必须解码为 `data:null`。CSRF 刷新也复用同一 session decoder | 协议损坏或错误分类漂移不再被伪装成匿名状态；依赖/网关故障会进入错误登录壳，损坏 logout 200 也不会误报成功 | session 成功严格校验必需事实；401 先解码错误 envelope，仅 `unauthenticated` 转匿名，其余失败关闭；logout 要求成功 `data:null`。时间在 WASM 使用浏览器 `Date.parse` 语义 | 双仓 session client/Auth shell 已逐分支复核；Admin 原生/WASM check、Clippy `-D warnings`、rustfmt 与仓库策略门禁通过。仍待 `401 unauthenticated`、其它 401、损坏 200 和 logout 非 null 的受控 R1 |
| API-040 | Admin 浏览器请求默认超时 | P1 | 静态修复，待超时 R1 | 旧 `requestJSON` 对全部 Admin GET/mutation 默认使用 10,000 ms `AbortController`；超时稳定为 `request_timeout`，普通 fetch 失败为 `network_error` | Admin API 根现由唯一 `send_request` owner 建立 10,000 ms 截止；WASM 超时主动 abort fetch 并返回 `request_timeout`，普通发送失败仍为 `network_error`。精确 GET 与全部 mutation 均已迁入，API 目录只剩 owner 内部两处 `.send()` | 临时网络、反向代理或半开连接不再让管理页面与 dialog 无界等待；所有管理请求共用同一截止与稳定错误分类 | 在 Admin API 根建立唯一 10 秒发送 owner，WASM 用 AbortController 取消 fetch；全部 GET/mutation 复用并保留现有 URL、CSRF、JSON 与响应 decoder，超时只暴露稳定 `request_timeout` | 双仓请求 owner 与直接发送点已逐项复核；Admin 原生/WASM check、Clippy `-D warnings`、rustfmt 与仓库策略门禁通过。真实 10 秒挂起、abort 与普通网络错误仍待一次性受控 R1 |
| AUI-093 | Admin session loading/error 登录外壳 | P2 | loading 已修复并部署；error 保留安全扩展；双视口待 R1 | 旧 `AdminApp` 的 pending 分支直接返回顶层 `div[role=status]`；旧版没有显式 session error 分支，协议失败可能错误落入认证 Outlet，是不可回迁的 fail-open 缺陷 | pending 已恢复顶层 `div[role=status]`；error 继续使用 `main/card[role=alert]`、重试并严格阻断 Outlet | pending 的旧 DOM 差异已关闭；error 的显式安全页是新版加固，不能因视觉对照删除 | pending 保持顶层 `div[role=status]`；保留 error 的 main/card、alert、重试和 fail-closed 边界 | 旧源码和冻结 fetch 的一次性浏览器证据已确认基线；Admin 原生/WASM、全工作区静态门禁通过。新 Admin payload `1b6885da…` 已内嵌并由 SHA-256 `54cb004a…` 的常驻制品接管 8091；根页、live/ready、未登录 session、全部嵌入资源和 LAN 访问已验证。error 双视口几何仍待 R1 |
| BID-017 | 生命周期与竞价事务锁顺序 | P0 | 静态修复，PostgreSQL 锁计划 R1；并发待证 | 旧 Preview/Open 在拍卖锁前先锁 world；旧竞价写链保持 world → auction → lot/authorization 的稳定顺序 | PostgreSQL 生命周期 context 已用 materialized CTE 固定 world → optional scheduled round → auction 的 `FOR UPDATE` 顺序；竞价快照原有 world → auction → lot 顺序未变 | 已从结构上消除 Final 与普通竞价之间的 auction→world 反向锁序；仍不能用静态计划代替真实并发争用证据 | 保持应用层 Serializable 三次完整重试；SQLite 继续依赖串行写事务，不模拟 PostgreSQL 行锁 | 双仓 SQL/调用链 R2；真实 PostgreSQL v19 数据上 `EXPLAIN (COSTS OFF)` 显示 world CTE `LockRows`、round CTE `LockRows`、外层 auction `LockRows` 的顺序，事务随后回滚。两后端 Storage、Application、全工作区 check/Clippy 均通过；并发 R1 仍待补 |
| LIF-002 | Preview/Open 的 world gate 与 scheduled round 锁内快照 | P1 | 静态修复，锁计划 R1；状态并发待证 | 旧 Preview/Open 在锁 world 后拒绝 disabled world，并在同一事务锁可选 scheduled round；Cancel 只锁 world、不检查 enabled | lifecycle context 现返回锁内 `world_enabled` 与可选 round；Preview/Open 在应用计划层恢复 enabled gate，Cancel 不错误扩张该限制 | disabled world 不再进入 Preview/Open，round 也不再来自未锁定 scalar 子查询；真实 enabled 切换和 round 并发尚待证明 | PostgreSQL 复用 BID-017 的 world→round→auction 锁链；SQLite 在明确串行写事务中 join 同一快照 | 双仓 repository/plan 逐分支 R2，PostgreSQL 锁计划 R1；Application 与 PostgreSQL/SQLite Storage check/Clippy、全工作区静态门禁通过，真实 enabled/round 并发仍待 R1 |
| BID-018 | slow proxy 执行时的 auction 终态竞态 | P1 | 静态修复，scan→Final 竞态待 R1 | 旧 slow worker 的执行 SQL 同时要求 lot live 且 auction `Live` | due scanner 后的锁内 snapshot 现要求 `auction_status == Live`，非 Live 与 generation/due/authorization 等漂移统一按 stale no-op 收口 | scan 与 execute 之间若拍卖进入 Final，已领取 tick 不再继续写入竞价事实；真实竞态仍待隔离证明 | 不改变 generation/due/authorization/资格检查，只在同一锁内快照增加旧版 Live gate | 双仓 Worker/competition 调用链 R2；Application、Jobs、PostgreSQL/SQLite Storage 及全工作区 check/Clippy、rustfmt 门禁通过，未启动 Worker，scan→Final R1 仍待补 |
| API-041 | Public 其余读取/内容响应的 route-specific decoder | P1 | 静态修复，待坏响应 R1 | 旧 Public 对 worlds/current/auctions/bid-log/price-history/rules/blacklist/compensation 的业务 data 逐路由校验闭集、类型、枚举、时间和组合 | 上述读取现统一先进入 route-specific raw validator，再反序列化为展示模型；world/current/目录/历史恢复字段闭集与查询绑定，rules/blacklist/compensation 保持旧版允许扩展字段、但严格校验必需语义 | 损坏、非法组合或不符合当前查询的 200 data 会在浏览器边界失败关闭；内容路由不会因误加字段而破坏旧兼容扩展 | 复用 Public 集中 decoder 与既有错误 envelope；按旧版每条路由分别冻结字段、safe integer、UUID、时间、枚举、排序、分页和查询归属语义，验证后才转换页面模型 | 五路审计、旧 TypeScript validator 与新调用点逐项复核 R2；Public 原生/WASM check、全特性 Clippy `-D warnings`、rustfmt 与差异空白门禁通过。未知/缺失字段、非法 UUID/时间/组合及合法扩展仍待一次性受控 R1 |
| API-042 | Public mutation 默认 10 秒请求截止 | P1 | 静态修复，真实挂起/abort 待 R1 | 旧共享 Public client 对全部请求默认使用 10,000 ms AbortController，超时稳定为 `request_timeout`、普通发送失败为 `network_error` | bid/create proxy/lower proxy 三条命令已复用 Public 唯一 `send_request`，透传 AbortSignal，并经 `transport_error_code` 区分超时与普通网络错误 | 核心竞价动作不再绕过共享截止 owner；半开连接的实际 10 秒取消和 UI 展示仍待运行态证明 | 保持 command_id、URL、JSON、成功/业务错误 decoder 和 `Date` 头语义不变，仅统一发送与 transport 分类 | 双仓请求 owner 与直接发送点 R2；Public 原生/WASM check、定向及全工作区 Clippy、rustfmt/i18n/策略门禁通过，真实挂起/abort 仍待 R1 |
| API-043 | Admin 其余成功响应的严格业务 decoder | P1 | 静态修复，待坏响应 R1 | 旧 Admin 对 intake、proxy events、settlement/recovery、credentials、renames、mail、realtime、audit、rules 与 runtime config 使用逐路由闭集和语义校验 | 上述路由现分别使用严格 raw DTO 或已有严格 contract，再校验规范 UUID、safe integer、时间、枚举、分页、查询/路径归属、版本递增和 nullable/optional 组合；intake 绑定请求价格与玩家，recovery 绑定 auction/recovery/action/version；HTTP 映射只把内部 `final_fast` 投影为旧 Admin wire 的 `active`，realtime failure 的 `has_more` 也恢复旧版完整页边界算法 | 损坏、非法组合或不属于当前请求的 200 data 会在浏览器边界失败关闭，不再被管理页面展示或用于后续命令；旧兼容允许扩展的 settlement/renames/mail/realtime/audit 外壳保持可扩展 | 按高副作用、identity、operations/content、runtime config 的 route owner 分批建立集中 validator；保留 API-038 已冻结的 envelope 策略和 API-040 超时 owner，最后回扫所有直接 `ApiResponse<Admin…>` 调用点 | 五路审计、旧 TypeScript validator、Contracts DTO 与新调用点逐项复核 R2；各批 Admin 原生/WASM check、全特性 Clippy `-D warnings`、rustfmt 与差异空白门禁通过。未知/缺失字段、非法 UUID/时间/枚举/组合及合法扩展仍待一次性受控 R1 |
| PUI-028 | Public WorldPicker 首屏 canonicalization 时序 | P1 | 静态修复，三态地址栏待 R1 | 旧 WorldSwitcher 在 worlds 查询尚无成功 data 时不规范化 URL；只在服务端选项已知后处理当前 world 失效 | fallback 仍用于可见选择器，但 URL canonicalization 现在只在 worlds 成功且非空后执行；query 在内存中仍仅显式 `isekai` 选中 isekai，其余回退 main | pending/error 不再提前改写用户 URL，成功后才按服务端选项规范化；真实三态地址栏仍待受控证明 | 保持选择器可用性与旧 `selectedWorld` 语义，避免把本地 fallback 冒充服务端真值 | 双仓组件 effect R2；Public 原生/WASM check、定向及全工作区 Clippy、rustfmt/i18n/策略门禁通过，pending/error/成功地址栏仍待一次性 R1 |
| AUI-094 | Rename revert 的最新 user state version | P1 | 静态修复，真实并发改名待 R1 | 旧 revert 点击后立即重新读取目标 user，再以该快照 state version 提交回滚 | 回滚提交前现按 `next_username` 重新读取 users，并以稳定 user ID 精确归属目标；命令使用最新 `state_version`，成功响应还必须递增一个版本 | 列表加载后的其它用户变更不再必然制造可避免的 stale 冲突；重读失败、找不到精确 user 或返回版本不一致都会失败关闭，并保留已生成的稳定 command ID | 复用现有 users 读取契约，不新增单用户旁路；显示名只负责缩小候选集，最终归属由 user ID 决定 | 双仓 action 调用链与旧 `RenamesPage.tsx` R2；Admin 原生/WASM check、全特性 Clippy `-D warnings`、rustfmt/i18n 门禁通过。真实列表加载→用户并发变化→revert 仍待受控 R1 |
| AUI-095 | Users status badge 的旧 token 与 i18n 边界 | P1 | 静态修复，五语言运行态待 R1 | 旧 Users 主表展示冻结 status token/manifest 文案，并与筛选 value 分离 | API enum 仍作为筛选和命令值；可见 badge 现把 `active`/`blacklisted` 映射到 Admin Fluent 消息，未知 token 使用现有非法响应文案失败关闭 | 非默认语言不再直接泄漏内部英文 token，筛选契约与可见文本 owner 已重新分离 | 五种受支持语言 `en/zh-CN/zh-TW/ja/ko` 均新增同一消息键集合；不扩张 locale 闭集 | 双仓表格投影 R2；五份 FTL populated key 已同步，Admin 原生/WASM check、全特性 Clippy `-D warnings`、i18n key-set、rustfmt 与差异空白门禁通过。五语言页面切换仍待受控 R1 |
| OPS-007 | retention policy 的当前 PostgreSQL migration version | P1 | 已实施，read-only inventory R1 | retention inventory 要求 policy `postgres_migration_version` 与当前应用迁移树精确相等，避免用过期恢复点进入回滚集合 | 受控 policy 已从 16 更新为当前 migration 19；历史证据中“当时 schema 16”保持不改 | 官方 retention inventory 不再被陈旧 policy 阻断，v19 恢复点可进入合法保留集合 | 后续新增 PostgreSQL migration 时必须同步受控 policy；inventory 仍保持数据库只读和显式目标目录 | 使用 Worker maintenance 配置对当前 PostgreSQL v19 执行一次性 `retention-inventory`：`relations=62`、`policy_tables=61`、`migration_current=true`，证据显示 `transaction_read_only=on` 且 policy/expected version 均为 19；临时输出目录已删除。auctionctl 与全工作区 check/Clippy、策略/格式门禁通过，未写数据库、未运行清理 |
| SET-012 | 结算明细存储/重放边界 | P0 | 静态修复，损坏事实待 R1 | 旧 `SettlementReport.Validate` 对每条成交明细要求金额守恒、tax/compensation 互斥，征税时卖方金额等于成交价，补偿时卖方金额严格低于成交价 | 新结算计划生成器已经产生相同合法形状，但 `SettlementLine::validate` 冻结基线只校验守恒和 listed seller amount，没有独立关闭税/补偿矩阵 | 正常新写路径看似一致，损坏账本或接管事实却可能通过领域/存储重放边界，随后被错误当作权威结算结果 | 在 `SettlementLine` 的单一校验 owner 恢复旧矩阵；不改变税率、补偿分配、计划算法或 SQLite Lite 能力声明 | Domain/Storage all-targets/all-features 定向静态编译已通过；仍需税补同时非零、征税卖方折扣、补偿但卖方未折扣及合法四类明细的受控损坏/重放 R1 |
| API-044 | Admin HTTP 认证与 mutation 安全顺序 | P0 | 静态修复，组合输入待 R1 | 旧 `sessions/adminhttp/adapter.go::Protect` 外层顺序为 rate limit → Origin → CSRF → Require/session → handler；全部 Admin 写路由（包括论坛预览）经 `Protect`，GET 只经 `Require` | `routes/session/guards.rs` 现提供唯一 `authenticated_admin_mutation`；全部 23 个写入口先通过该 helper 再解析 query/path/body，GET 不再消费 mutation limiter，论坛预览也恢复真实 session | 冻结新版 Admin 守卫先 Origin 后限流、GET 消耗 mutation quota，多条写路由在 session 前解析参数；论坛预览甚至只有 Origin/CSRF 而没有 session，缺 CSRF 还可能错误返回 401 | 保持统一顺序和旧 403 `csrf_invalid`；route-specific 参数、严格 JSON、命令 fence 与错误 taxonomy 留在认证后的 handler | auction-http/Admin Web 定向静态编译与 rustfmt 已通过；仍需缺/坏 CSRF、无 session+坏 query/path/body、Origin、限流耗尽、论坛预览及 GET 不消耗写额度的认证组合 R1 |
| API-045 | Public 身份化读取认证优先级与 UUID 词法 | P1 | 静态修复，组合输入待 R1 | 旧 `auction/publichttp/account.go` 把 `/me/bids` 与 `/lots/{lot_id}/proxy-authorizations/mine` 只包装在 `Sessions.Require` 下；旧读取边界使用规范小写、带连字符且非零 UUID | 两条身份化读取现先认证且不调用 mutation limiter；mine、forum、bid log 和 compensation movements 的 path/query UUID 统一进入 `parse_public_uuid` | 冻结新版会让只读账户请求消耗写额度，mine 还在认证前暴露 path 分类；若其它读取继续使用宽松 `Uuid::parse_str`，大写/URN/braced 文本可能进入查询 | 身份化 GET 只做 session 认证，不借写限流改变可用性；所有冻结 v1 UUID 文本在 HTTP 防腐层统一失败关闭，领域 ID 不放宽 | auction-http all-targets/all-features 定向静态编译与 rustfmt 已通过；仍需匿名+坏 path/query、认证大写/URN/braced/nil UUID，以及连续 GET 后 mutation quota 不变的 R1 |
| AUI-096 | Admin 客户端未知路由的认证外壳 | P2 | 静态修复，浏览器待 R1 | 旧 Admin 路由由会话外壳统一拥有，匿名深链接不会先渲染管理内容 | Dioxus `NotFound` 现与 21 条冻结路由同处 `AdminShell` layout；后端未知深链接仍保持 404，不扩大 SPA allowlist | 冻结新版把 catch-all 放在 `#[end_layout]` 之后，客户端导航到未知路径时可绕过 loading/error/login 状态直接显示管理端 404 结构 | 只把客户端 catch-all 收回现有 shell；不新增服务端 SPA 路径、不改变 21 条 manifest 或登录 return 安全边界 | Admin 原生 all-targets/all-features 定向静态编译与 rustfmt 已通过；仍需匿名/loading/error/已认证四态的客户端导航 R1 |
| OPS-008 | 已发布 PostgreSQL migration 字节、解析与 checksum 双代兼容 | P1 | 静态修复，fresh/两代历史待 R1 | 已发布 migration 必须保持字节与 SQLx checksum 不变；PG18 又会拒绝 `000005`/`000015` 中未加引号的保留字别名 | 两个历史文件已恢复 HEAD 字节；`database/migrations` 只对固定版本、描述、片段和数量生成执行视图。fresh/原始历史使用原 checksum，早期别名修正版只在既有 checksum 精确等于固定替换 SQL 的 SHA-384 时沿用；readiness 和 v28 adoption 复用同一边界 | 直接改历史 SQL 会让原 checksum 库失败；只恢复原 SQL 又会让 fresh PG18 与 v28 前向迁移解析失败；只接受原 checksum 还会阻断已经运行过早期修正版的数据库 | 不修改 `_sqlx_migrations`，不接受任意第三种 checksum；SQLx 对未知历史继续 `VersionMismatch`，执行兼容仅限两个已证明别名 | 两个历史文件对 HEAD 的 diff 为零，Storage all-targets/all-features 静态编译通过；仍需 fresh、原 checksum history、已知修正版 history、未知 checksum 及 v28 adoption 的隔离 R1 |
| LIF-003 | Preview 非空门禁与不可开放空场次 | P1 | 静态门禁通过；历史空场次已受控取消 | 旧 Preview 会接受零个 pending lot，但 Open 随后拒绝零 lot，能够留下无法自行恢复的 preview 场次 | PostgreSQL 与 SQLite Lite 均在同一生命周期事务内、状态更新和 intake target 转移前要求至少一个 pending lot；历史空场次需要受控取消 | 这是相对旧版的有意不变量强化：拒绝不可达状态早于写入，避免 Worker 对同一 scheduled round 无限重试 | 非空检查复用生命周期已经锁定的 auction/intake scope；不改变有 lot 的 Preview 物化、successor、账本、审计或 realtime 语义 | PostgreSQL/SQLite Storage check、全工作区 check/Clippy、rustfmt 与策略门禁通过；测试库唯一空 preview 通过 Admin 命令从 version 2 取消为 version 3，scheduled round 转为 missed，并追加 `auction.cancel` 成功审计。未把受控修复冒充新门禁的并发 R1 |
| PUI-029 | Public `opening` 过渡态的倒计时投影 | P1 | 静态修复并部署，真实 opening 画面待 R1 | 计划开放时间到达后，页面不能同时把同一时刻表达为“距开拍”和通用过期“落锤中” | `presentation_mode=opening` 现有独立五语言状态，只显示“开拍处理中”，不渲染过期计划时间；live/lot 的通用过期收尾语义保持不变 | 当前 generic 非 live 分支把合法 opening 投影误当 preview，产生用户报告的矛盾文案和过期时间 | 只在看板页分离 opening；不伪装成 live、不开放竞价、不改变 countdown helper 或权威生命周期状态 | 五份 Public FTL 已同步新键；i18n、Public 原生/WASM、全工作区 check/Clippy、rustfmt 与策略门禁通过。新 Public payload/进程已接管测试环境且 root/live/ready 为 200；空场次已取消，真实 opening 画面仍待下一场非空拍卖的人工 R1 |
| PUI-030 | Public 普通出价 K 输入与二次确认 | P2 | 用户批准扩展，静态实施待 R1 | 旧普通出价默认显示 `next_bid` 金额缩写，输入允许 K/M 语法，首次提交立即发送命令，没有二次确认 | 新普通出价边界只把纯数字解释为 K，默认值向上投影到可提交的整 K；第一次提交只建立稳定命令并进入确认步骤，明确确认后才发送 Credits 金额 | 这是用户明确批准的防误操作扩展，不追求旧版逐字交互等价；代理创建/降低继续保留旧金额缩写规则 | 复用同一 modal 和按钮焦点，避免嵌套 dialog；返回修改保留输入与命令 ID，服务端失败仍回到编辑状态并刷新权威快照 | 五份 Public FTL、i18n key-set、Public 原生/WASM、Clippy、rustfmt 与差异空白门禁；仍需浏览器确认默认值、键盘输入、返回修改、Escape/关闭和一次实际确认提交 |
| BID-021 | Public 代理立即反击的竞价日志压缩 | P2 | 已实施，测试环境 fast 历史 R1；slow/多页待证 | 旧版和冻结新版都会把同一普通竞价命令产生的 challenger `ordinary` 事实与随后 `proxy` 自动应价作为两行公开历史返回 | Public 竞价日志只隐藏同一 `place_bid` 命令、同一 lot、连续 fact/state version 且确由另一用户代理自动反击的 ordinary 行；页面直接显示最终代理应价，不公开代理最高上限 | 这是用户明确批准的公开投影扩展，不追求旧版逐行日志等价；内部 append-only bid、审计、命令重放、realtime 与账户“我的出价”仍保留完整事实 | PostgreSQL/SQLite 的明细与 total 使用同一强配对规则；不按 `valid=false`、时间或任意后续代理启发式过滤，slow 代理短暂领先事实不误隐藏 | 两后端 Storage check/Clippy、rustfmt、仓库策略和差异空白门禁通过；测试环境 main 日志返回 200/total 9，保留 1737K、1916K、2113K 的 proxy 应价与 1111K、2334K 的独立 ordinary，配对的 1654K、1824K、2012K ordinary 不再公开。Public 进程已接管且 live/ready 为 200；同价胜出、slow 代理和多页 cursor/total 仍待 R1 |
| API-046 | Dashboard MoogleMail `shared` 凭据继承 | P1 | 已实施，shared-only dashboard R1 | Worker 按目标 world 读取专属加 `shared` 凭据池，dashboard 健康投影必须使用同一有效 scope 语义 | 每个 main/isekai availability 现合并专属与 shared 的 active/invalid 计数，更新时间取最新值；只有 shared 时也会生成两个 world 的安全 availability | 冻结实现只精确匹配 world，导致 Worker 实际可用 shared 凭据时 Admin 错报缺失或 degraded | 不改变凭据密文、列表 DTO、重启判断或 Worker 读取；任一有效 scope 有 invalid 仍使该 world 失败关闭 | HTTP all-targets/all-features、全工作区 check/Clippy、rustfmt 与策略门禁通过；测试库只有一条 active `mooglemail:shared`，重启后的 dashboard 对配置的 main 返回 status ok、available 1、invalid 0、restart false，未回显 secret |

2026-08-27 第二轮源码复核确认了 `BID-004` 至 `BID-006`、`SET-001` 和 `SET-002` 五项 P0。
这些结论不依赖运行态猜测：Final 状态、竞价 SQL 谓词/锁对象、settling 状态与通用快照谓词，以及
pool enabled 分支和 ledger 写法均可从双侧完整调用链直接证明。五个窄切片已完成静态修复，但不能
外推为整条结算/履约链等价。

2026-08-27 第三轮由 Settlement、Public WebUI、Admin WebUI 三路独立双仓库审计补充确认了
`SET-006` 至 `SET-010`、`AUD-003`、`PUI-013` 至 `PUI-020`、`AUI-010` 至 `AUI-015`，并把
动态浏览器证据缺口收口到 `EVD-003`/`EVD-004`。随后按旧调用链完成了 `SET-006` due 事实时间、
`SET-007` 双阶段资格/custody/余额授权与终态、`SET-008` schema-v1 wire/稳定身份/主题，以及
`SET-010` 严格报告 wire 和 Serializable 重试的静态修复；复核中额外拆开 BuyerRefund 的 dispatch
world 与 equipment world，避免异世界装备 custody 被错误投影到 main。`BID-007` 已完成最小修复和
隔离 Final R1。随后重建最新 Public/Admin/Worker/auctionctl release 制品，在明确停止新版 HTTP 进程后
把隔离 PostgreSQL 从 SQLx 16/16 前向迁移到 17/17；重启后 8090/8091 的根页面、live、ready 与 LAN
根页面均为 200，8092/8093 旧版仍为 200。当前隔离库没有 `auction_settlements` 行，只有一条 main
withdrawn-return pending 事实，因此这次运行态只证明最新制品、迁移和健康边界，不提升 SET-006/007/
008/010 的证据等级。真实 MoogleMail、授权时间窗、合法/损坏报告和完整动态 WebUI 仍须按矩阵补 R1，
不得宣称完整功能或 WebUI 等价。

其后曾按静态实现把 `PUI-020`、`AUI-013`、`SET-009` 与 `AUD-003` 标为收口，并重建切换 8090/8091
embedded 制品；Public/Admin payload SHA-256 分别为
`16f80e32c1a6075253f1e06015acd1dcbbb0e1e063b8d59223e3363852fc80bc` 与
`3a27e7ea174cbd90f33e0d34fb1edfb8ef3c401d074653386bbfea4cbf8f211f`，本机及 LAN 的新版/旧版根、
live、ready 均为 200。第三路独立复核随后证明该标注过早：当时 Public 拍卖页仍可能同时建立 discovery
与 auction-specific 两条连接；Admin 创建默认值、表单复位、Users/Renames/Mail 主表及 Outbox 外壳仍有
明确差异；旧 complete 重放虽有内部实现，却没有真实运维入口。矩阵已按当前源码重新降级，不能用制品
可访问性替代动态行、认证状态或浏览器交互 R1。

## 路由与 WebUI 覆盖复核

### Public HTTP P01-P20

| 路由 ID | 复核结论 | 关联差异 |
| --- | --- | --- |
| P01 | 路径和读取 owner 已对应；桌面与 390×844 空态已逐像素一致，动态 worlds 数据仍缺同库 R1 | PUI-008（已关闭）、EVD-002 |
| P02 | UI 已按当前 locale 响应式重读；五语言目录规则已受控导入，成功态几何/样式/文本和逐篇截图均与旧版一致 | PUI-001、DATA-001（已关闭），EVD-002 |
| P03 | 旧 per-auction 契约、严格内层 decoder、缺口有界扫描与高水位游标已静态恢复；discovery 保持显式 scope，待畸形/缺口同数据 R1 | RT-003 |
| P04 | client/server 已统一为旧单事件帧，并恢复有界 merge cache、refetch 保留连接/游标和 3 秒重连；待异常帧、状态提示和握手/close R1 | RT-001、RT-002、PUI-020 |
| P05、P07-P17 | 路径、认证分类和读取 owner 静态齐全；URL 历史、非法 lot 及当前桌面匿名/空态已有浏览器 R1；bid log 页大小、Catalog 动作、session 四态和 lot 历史列均已静态恢复，待动态同数据 R1 | PUI-003/PUI-005（已关闭）、PUI-004、PUI-009 至 PUI-012、EVD-002 |
| P06 | POST/GET/DELETE、Public 7 天/30 分钟会话默认值与登录来源返回已静态对应 | PUI-002 |
| P18-P20 | 写路由、资格锁、显式 proxy mode、严格 JSON、Final/world/open gate 和 PostgreSQL 锁顺序均已静态恢复；待同库及并发竞价 R1 | BID-001、BID-002、BID-004 至 BID-006、API-002 |

### Admin HTTP A01-A37

| 路由 ID | 复核结论 | 关联差异 |
| --- | --- | --- |
| A01 | 三种 method 与 Admin 15 分钟/8 小时会话默认值已静态恢复；仍待认证时钟 R1 | CFG-003（已静态修复） |
| A02 | dashboard owner 已接入并恢复旧 v1 严格响应闭集；已认证新旧响应顶层/section 键、请求数和桌面/390px 壳层几何取得局部 R1；不同数据库的值级内容及最终导航制品仍待复核 | API-013、AUI-008、AUI-014、EVD-002、EVD-004 |
| A03-A15 | 路径齐全；竞价与 settlement/recovery 的第二、三轮 P0/P1 已静态修复，仍需同库、抓帧、旧账本重放和外部服务 R1 | BID-004 至 BID-007、SET-001 至 SET-010、AUD-003、WRK-001、EVD-001 |
| A16-A20 | 邮件读取/控制、远端 retry 分类和失败 `last_poll_at` 已静态恢复；每周 current-week/mail gate 正在实施，真实服务证据仍未等价 | WRK-003（已静态修复）、WRK-004、WRK-005、EVD-001 |
| A21-A23 | 读取路径及旧嵌套分页已静态恢复；待已认证 R1 | RT-004 |
| A24 | 旧请求可稳定派生命令 ID，新请求仍显式携带；待同库重放 R1 | RT-005 |
| A25 | PostgreSQL 规则读写/CAS 已接入；旧目录到权威表的显式导入已取得幂等/API R1，Lite 只读仍是声明边界 | DATA-001（已关闭）、EVD-002 |
| A26-A27 | 旧版 worlds/equipment_filter/mail_templates 已映射到同一 Admin runtime，并由 Worker 重启投影；待认证保存和重启 R1 | AUI-001（已静态修复） |
| A28-A30 | 用户/改名路径、核心命令账本、稳定错误反馈、列表页大小和 session 绝对过期刷新均已静态恢复；待同数据/认证时钟 R1 | AUI-002（已静态修复）、AUI-006、AUI-007 |
| A31 | 路径存在；黑名单事务副作用已静态修复，待同库 R1 | ID-001 |
| A32 | 凭据命令/密文边界和旧版三列合成语义已恢复，并保留新版 version；待认证浏览器 R1 | AUI-003（已静态修复） |
| A33 | 审计读取路径和安全投影存在；真实追加记录与 Audit/Outbox WebUI 仍缺同库 R1 | EVD-002、EVD-006 |
| A34 | Delivery 路径、旧嵌套分页、五动作滚动升级 wire/message/数据库时钟、typed 400/404/409/503、默认排序及 populated WebUI 已取得多组隔离 R1；`40001`、坏响应全集、新 origin 五语言和 settlement 联动终态仍待补 | API-001、API-019、API-020、DLY-002 至 DLY-007、AUI-021 至 AUI-024、AUI-050 至 AUI-059 |
| A35-A37 | 命令路径、fencing、账本和稳定错误反馈已静态恢复；外部/认证 R1 仍保留 | AUI-002（已静态修复）、EVD-001 |

### 浏览器路由与语言

- Public 的 14 条旧路径 `/`、`/lots/:id`、`/bidlog`、`/auctions`、`/history`、`/login`、
  `/me`、`/rules`、`/blacklist`、`/compensation-pool`、`/auction`、`/account`、
  `/compensation`、`/status` 均有 Dioxus route 和 SPA allowlist 对应。
- Admin 的 21 条旧路径 `/`、`/login`、`/mails`、`/auctions`、`/auctions/:id/lots`、
  `/cookies`、`/users`、`/renames`、`/deliveries`、`/outbox`、`/settlement`、`/config`、
  `/rules-content`、`/audit`、`/status`、`/dashboard`、`/auction`、`/mail`、`/delivery`、
  `/credentials`、`/rules` 均有 Dioxus route 和 SPA allowlist 对应。
- 路径存在只证明可导航；旧 alias 是否展示同一 owner、同一查询状态和同一错误反馈，按 PUI/AUI
  差异分别处理。
- 旧版 Public 实际启用 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，旧版 Admin manifest 只有
  `zh-CN`；新版按项目 i18n 契约把两套 surface 都扩展到这五种语言。Fluent/`dioxus-i18n`
  可以承载更多 BCP 47 语言，但不会自动生成翻译，不能把框架能力算作已支持语言。
- Public 默认 `en`，Admin 默认 `zh-CN`，偏好键均为 `hv.lang`；未知值回退 surface 默认语言。
  共享 picker 已显式同步原生 option 选中态，避免刷新后译文 locale 与下拉显示分离。
- 新版 Public 每种资源各有 467 个消息 ID，Admin 每种资源各有 933 个消息 ID；同 surface 五个文件
  数量和键集合一致。语义翻译质量与逐页换行仍属于 EVD-002，不能由键数证明。
- Fluent 默认会在动态变量外加入 FSI/PDI；旧 i18next 没有该文本变化。两个 surface 现统一通过共享
  `legacy_t!` 兼容边界剥离外层成对隔离符。最终 populated Public 根页和 Admin 登录页均已逐个 locale
  证明 `html lang`、标题/选择器与译文对应，页面正文 FSI/PDI 计数为 0；更深的 Admin 认证动态页面仍
  归 EVD-003，不能由登录页初始化外推。
- Public 规则 resource 已把当前 locale 纳入 key并取得五语言同数据浏览器 R1；补偿池也已逐项取得五语言
  picker/空态对照。登录来源已显式保存 pathname + search，带有效会话的成功返回仍保留在 PUI-002。
- Public/Admin 全部 feature API 现使用方法严格 MethodRouter 和共用 JSON 写边界；健康、metrics 与
  静态 GET/HEAD 仍由独立 owner 维护，不把业务 HEAD 规则外推到运维或资源路径。

## 已证明等价项

本节只保留达到 `等价` 标准的项目；既有迁移文档中的“已接入”不会自动搬入。

| ID | 能力 | 旧版证据 | 新版证据 | 证明范围 | 未覆盖范围 |
| --- | --- | --- | --- | --- | --- |
| EQ-001 | 拍卖状态机 | `auction/service.go`、`bidding_state.go` | `auction-domain/src/auction/mod.rs` | 创建、预览、开放、Final、取消、结算入口和版本递增的 R2 对应 | 真实并发和旧数据接管 |
| EQ-002 | 竞价计划 | `auction/postgres/adapter/repository.go`、`proxy.go` | `auction-domain/.../competition/plan/{ordinary,proxy}.rs`、PostgreSQL competition transaction | 纯领域普通/fast/slow 计划、重入、降低上限、绑定价、次数、Final 转换的 R2 对应；v1 mode 已失败关闭 | BID-001/BID-003/WRK-001 同库 R1、BID-002 认证 R1 |
| EQ-003 | PostgreSQL 会话 | `sessions/service.go` 和 PostgreSQL repository | `auction-application/src/services/sessions`、`auction-storage/src/database/sessions` | Public 滑动/绝对过期、数量限制，Admin CSRF/轮换及两 surface 独立默认值 | CFG-003 已静态修复；ID-001 同库 R1 和真实时钟行为 |
| EQ-004 | PostgreSQL 规则发布 | `rules/service.go`、rules PostgreSQL repository | `services/admin/rules.rs`、`database/rules/transaction.rs` | locale revision CAS、command replay/conflict、审计；Public locale 请求已静态恢复 | 输入长度/locale 校验收窄、Lite 只读、五语言同数据 R1 |
| EQ-005 | PostgreSQL 成交历史 | `auction/postgres/adapter/public_history.go` | `database/price_history/postgres.rs` | 时间、世界、价格、身份、名称过滤，稳定 cursor 和统计方式 | 同数据 R1 与 DB-001 |
| EQ-006 | PostgreSQL 结算/recovery | `settlement.go`、`recovery_service.go` | `services/settlement`、`database/settlement`、`database/recovery_scheduler` | 税/折扣/补偿守恒、义务、case/action、lease/fencing 的 R2 对应 | 真实邮件/论坛/人工回执 EVD-001 |
| EQ-007 | 幂等、审计和持久事件 | 旧 command ledger、audit、realtime/outbox 写入 | 新各事务 runner、`audit_records`、`realtime_events`、delivery/commands | 相同 ID/摘要重放和异摘要冲突；关键 PostgreSQL 写入同事务追加事实；Batch B wire 已静态恢复 | RT-001-RT-005/API-001 的同部署与同库 R1 |
| EQ-008 | 浏览器路径闭集 | 两个 `*.routes.json` | 两个 Dioxus `Route` enum 与静态 allowlist | Public 14、Admin 21 条路径均存在 | 页面功能/视觉见 PUI/AUI/EVD-002 |
| EQ-009 | 界面语言注册 | 旧 Public manifest、Admin locale merge 和两个 picker | `auction-web-shared::SUPPORTED_LOCALES`、`legacy_t!` 与两套 `I18nConfig` | 五语言、默认值、`hv.lang`、`html lang` 与 Public 规则 locale 对应；双 surface picker、Public 规则/补偿池、最终 Public populated 根页与 Admin 登录页已有 R1，动态变量无 FSI/PDI | 其余逐页长文案换行、认证动态页和移动端语言布局 |
| EQ-010 | canonical v28 接管与灾备链 | 旧对照 PostgreSQL 18.6/v28 副本和目录规则 | `postgres-legacy-v28-backup`、`postgres-adopt-legacy-v28`、`import-rules-directory`、`postgres-backup`、`postgres-rebuild`、`postgres-restore` | 专用恢复点离线校验、锁内接管到 SQLx 16、五语言导入、当前格式备份、同名空库重建和单事务恢复的隔离 R1 | 旧副本没有 auction/lot/bid/user/realtime 业务行；非空业务抽样、外部服务和真实生产切换仍未覆盖 |
| EQ-011 | 发布制品边界 | 旧 `build-release`、`verify-artifacts` 与 `release-check` | 显式 `web-embed`、双 target、Web payload manifest、ELF 内容扫描和确定性 package | API-only/embedded 四个 ELF、Public/Admin 自有资源隔离、缺资源 503、来源/依赖/成员/权限/时间戳/哈希证据均取得本机 Linux R1 | 隔离 systemd/Caddy 已消费 embedded 制品；真实生产账号、域名/TLS、正式 secret 和流量切换仍未覆盖 |
| EQ-012 | systemd/Caddy 生命周期 | 旧 `ops/deploy/start.sh`、Caddy 配置和进程顺序 | `auctionctl deployment`、`ops/systemd`、Caddy 模板及三进程信号边界 | 同宿主机隔离安装已证明严格配置/权限预检、SQLx 16/16 前向迁移、Worker → Admin → Public 就绪、Public → Admin → Worker drain、InvocationID 回执、restart、Caddy 反代/metrics 屏蔽及 Public 启动失败后的逆序回滚 | 未使用真实域名/TLS、专用生产账号、正式 secret、外部服务或真实流量；不构成生产切换批准 |

“R2 对应”表示当前源码从入口到事务 owner 的主要分支一致，不表示已经在同一 PostgreSQL 数据和
真实外部服务上完成 R1。表中任何“未覆盖范围”未关闭前，不得把该能力提升为完整行为等价。

## 声明能力边界

以下内容需要与缺口分开记录：

- SQLite 是 Lite 档，不要求实现旧版 PostgreSQL 完整部署的全部 Worker、结算、recovery、邮件、论坛
  或管理写入能力；但已声明支持的写入仍必须满足幂等、事务、审计和失败关闭不变量。
- Redis 缓存、OTLP/Prometheus 增强、Unix listener 和额外运维诊断若不改变旧版契约，属于新增能力；
  不能用这些增强替代旧版功能，也不能反向记为旧版缺口。
- 外部服务真实回执若没有目标环境证据，统一记录为 `证据缺失`，不能以静态代码存在标记为完成；
  systemd/Caddy 当前只取得同宿主机隔离 R1，canonical v28 也只证明隔离对照副本的运维链，两者都不
  外推为真实域名/TLS、正式 secret、专用生产账号或流量切换批准。
- 旧版 JSON + 环境变量/CLI 配置迁移为主 TOML + 敏感 TOML 是明确架构决策；旧格式必须清楚失败，
  不要求继续作为第二套运行配置。需要补的是逐字段迁移映射和运行证据，而不是恢复散落的环境变量。
- 装备 key、用户登录 key、外部凭据使用三套数据密钥是新版安全加固，不应为“字段数量与旧版一致”
  合并回单一密钥。
- SQLite 未实现 Admin 规则发布、settlement、recovery、compensation、mail/delivery/fulfillment 和
  recovery buyer blacklist，属于已声明 Lite 子集；所有缺失能力必须稳定返回
  `CapabilityUnavailable`/501，不能返回空成功。
- Redis 缓存、发现派生流、额外 health 路径、Unix listener 和诊断字段可以保留为增强；同一路径的
  旧 v1 wire 仍必须兼容，增强不能成为原地改写旧契约的理由。

## 不变量交叉复核

| 不变量 | 当前结论 | 主要差异/证据 |
| --- | --- | --- |
| I-01/I-02 幂等与确定性事实 | PostgreSQL 主要命令链、slow tick 和旧 replay 兼容 ID 已静态接入 | RT-005 仍需同库证明派生 ID 重放；WRK-001 仍需同库重放 R1 |
| I-03/I-04 事务、锁和后端差异 | 核心 transaction runner、参与资格锁与黑名单副作用锁序已静态接入 | BID-001、ID-001 仍缺同库并发 R1；SQLite 使用串行写且仅承诺 Lite |
| I-05 竞价原子集合 | bid/audit/realtime 写入链、失格代理前置冻结及 slow tick Worker 审计已接入 | BID-001、WRK-001 静态修复待同库 R1 |
| I-05a 装备录入 | PostgreSQL 密文信封、命令账本和 intake owner 已接入 | 真实外部来源与非空旧装备数据仍归 EVD-001；OPS-001 只关闭 schema/灾备链 |
| I-05b 论坛预览 | A05 本地解析 owner 已接入 | 实际长文本/恶意 HTML 页面呈现 EVD-002 |
| I-05c 心跳 | 三进程持久心跳和安全投影已接入；隔离 systemd 已证明三角色顺序启动/restart、SIGTERM drain、停止回执与 Worker socket 清理 | 真实 heartbeat TTL、故障聚合和 exporter 状态仍需生产等价环境观察，不由 OPS-003 的进程生命周期 R1 外推 |
| I-06/I-07 状态与金额 | 领域状态机及 PPM/网格计划 R2 基本对应 | 部署实际默认软截止仍需有效配置证据；BID-002 已静态恢复显式 mode 失败关闭，仍待认证 R1 |
| I-08 外部写提交后执行 | outbox/Worker 边界和论坛未知写入的只读对账/人工再授权已静态接入 | WRK-002 仍缺真实论坛回执 R1；EVD-001 无完整外部证据 |
| I-09/I-09a-f Worker 持久进度 | slow due generation、论坛恢复授权账本、邮件 retryability 与大多数 scheduler/lease/fencing 已实现 | WRK-001 待并发/重启 R1；WRK-002/WRK-003 已静态修复但仍缺外部与停止时序证据；WRK-010/WRK-011 已静态修复，仍待隔离扫描与可恢复配置 R1 |
| I-10 缓存非权威 | 可选 cache 只位于读模型，竞价不依赖缓存 | Redis 故障回源尚缺运行态证据 |
| I-11 追加审计/事件 | migration、transaction owner 和旧 v1 单事件 wire 已建立 | 真实回放、断线补齐和外部消费见 RT-001-RT-005/EVD-001 的 R1 |
| I-12 敏感材料 | SecretString/密文信封/安全 DTO 边界静态存在；OPS-001 私有证据与 OPS-002 发布 archive/manifest 均不携带连接串或凭据；OPS-003 严格权限、unit 配置绑定、0600 root 回执及三角色 journald 敏感模式扫描已取得隔离 R1 | 正式 secret 管理器/注入方式和专用生产服务账号仍需真实部署批准 |

## 数据库对象复核

- `docs/contracts/schema.md` 的 61 个旧 PostgreSQL 对象已按逻辑 owner 映射到当前 PostgreSQL
  migration、兼容视图或明确替代模型；表名不同本身不构成差异。
- `bid_proxies` 由只读兼容视图替代，权威写入进入 `bid_proxy_authorizations` 及追加 transition，
  该防腐层符合迁移边界。
- PostgreSQL 的 settlement/recovery/delivery/forum/realtime/alert 对象有对应存储 owner；canonical
  v28 形状已完成接管和灾备链 R1，但旧副本没有非空 auction/lot/bid/realtime 行，不能据此证明这些
  owner 对真实业务历史的逐行语义。
- SQLite 只复核标为“两者”或明确 Lite 的对象。DB-001 已把成交历史收紧为稳定能力缺失；不得用
  SQLite 编译成功、空集合或直接写入的状态行代替 PostgreSQL 结算事实、行锁和隔离证据。
- schema 清单中关于“当前仍在迁移”的陈述需要在实施收口时更新为本审计结果，避免文档阶段状态
  与当前代码互相矛盾。

## WebUI 人工对照清单

后续运行态对照不编写浏览器测试用例，使用同一脱敏数据库快照、同一语言和 viewport 人工记录：

1. Public 14 条、Admin 21 条路径逐一打开，核对标题、导航、主要表格列、主操作和 alias 行为。
2. 每条数据页覆盖 loading、空态、有数据、404、400、401/403、409、429 和 503 中实际可达状态。
3. 核心竞价覆盖未登录跳转、登录返回、普通出价、fast/slow 代理、降低上限、已领先、过低价、
   stale state 和命令重放；结果以 HTTP 权威快照为准。
4. 五种语言逐页切换，核对 `html lang`、`hv.lang`、长文案换行、时间格式和规则正文 locale。
5. desktop 与 mobile 至少覆盖导航、表格横向处理、dialog、焦点返回、键盘操作和状态播报。
6. 对每个确认差异保存旧/新 URL、viewport、语言、响应摘要和截图位置；不同数据库导致的数据差异
   只记为证据缺失，不误判成组件回归。

### 本轮运行态证据

2026-08-26 先使用当时已运行、尚未包含本轮修复的新版 Public/Admin（8090/8091）和旧版 Public/Admin
（8092/8093）进行只读人工检查。浏览器为系统 Chromium headless，截图均使用相同浏览器参数；
这是一轮运行态取证，不是新增浏览器测试用例。两套服务当前数据库不同，因此只对与数据无关的布局、
错误边界和线协议给出行为结论，动态内容继续保留为证据缺失。该表记录差异发现时的基线；当前源码
修复后的隔离端口复核见后文批次 E 证据。

| 页面/请求 | 条件 | 旧版结果 | 新版结果 | 结论 |
| --- | --- | --- | --- | --- |
| Public `/` | 1440×1000、默认 `en`、当前空态 | 截图 SHA-256 `81eaa9a6872071e159d6559feda91510492993d25ca83055da6117c24f241c48` | 相同 SHA-256 | 当前桌面空首页逐像素一致 |
| Public `/` | 390×844、默认 `en`、当前空态 | `40fdadeaa827911f7a0d9cd17f43085987482c7fa46aa69121f8e623c375d957`，刷新与语言选择相邻 | 修复前 `d331b78e1a269cc7a3a5502db174e596898e0e06c1d86ec9ce55e6b127119c99`，两控件分散 | PUI-008 的发现基线；后续已关闭 |
| Public `/status` | 1440×1000、默认 `en` | `12292cb56f81c4628be31ba899a84702cb330e2b01b483d1647adc7ca5ea6925`，单一静态说明 | 修复前 `25e67a7ceff06a6d74d8d2f6d990b3b394e47df7e9198233f355c43566e5cab9`，重复标题及诊断表 | PUI-006 的发现基线；后续已关闭 |
| Public `/lots/not-a-uuid` | 默认 `en` | 本地 “Lot not found” | 修复前为 “Auction data is temporarily unavailable.” 与 Retry | PUI-005 的发现基线；后续已关闭 |
| Public `/rules` 与规则 API | 默认 `en` | API 200、4599 bytes，页面有正文 | API 503 `rules_unavailable`，页面为重试错误态 | DATA-001；因数据库不同暂不归因为代码差异 |
| Admin `/login` | 1440×1000、默认 `zh-CN` | `0878915d2488f92d1d39927274f8df3b3f624e51091171837184e582f56dadf8`，无页面滚动条 | 修复前 `a5d470b68924720b910410bc976739fb5538db531620ab71ce200793e66b48eb`，主体近似一致但有纵向滚动条 | AUI-005 的发现基线；后续已关闭 |
| `HEAD /api/v1/worlds` | 相同请求头 | 旧 8092 为 405、`Allow: GET` | 最终源码 embedded 制品 8090 为 405、`Allow: GET` | API-003 已取得当前长驻对照环境 R1 |
| Public `/auctions`、`/bidlog` | 最终源码、1440×1000、`en`、当前空态 | 一页仍显示 Previous/Next、页数和直接跳页；使用旧筛选说明与间距 | 同一分页文案、禁用态、筛选说明和空态几何；两页均无浏览器异常 | 当前空态桌面 R1；真实行、状态徽章、链接和多页数据仍待同库 R1 |
| Public `/history` | 最终源码、1440×1000、`en`、当前空态 | 不显示零统计；Submit 为 94.171875×44、字重 600、内边距 16px/4px；顶部世界选项为 Isekai/Main 且默认 Main | 结构、文案和上述计算样式逐项相同，页面截图视觉一致且无异常 | 当前 PostgreSQL 空态桌面 R1；真实成交行和高级筛选结果仍待同数据 R1 |
| Public `/login` | 最终源码、1440×1000、`en`、匿名 | 默认段落间距和窄登录表单 | 页面 PNG 与旧版 SHA-256 同为 `ab95abe4c31f588663a0895d46d891d21916c76669d9722a4d971a412b56f190` | 匿名视觉 R1；有效登录、恶意 return 和会话时钟仍待认证 R1 |
| Public `/me` | 最终源码、1440×1000、`en`、匿名 | My bids 标题、登录说明和 CTA 使用自然高度 | 同一文字、间距和 CTA 几何，无异常 | 匿名空态 R1；真实竞价/代理行和动作仍待认证 R1 |
| Public `/blacklist` | 最终源码、1440×1000、`en`、当前空态 | 页面说明、一页 pager 与直接跳页始终可见 | 同一文案、禁用态、输入和按钮布局，无异常 | 当前空态 R1；真实公开行、截断和多页仍待同数据 R1 |
| Public `/compensation-pool` | 最终源码、1440×1000、五语言、当前空态 | 余额 0K、自然段空态及一页 pager；五语言标题/空态按 `hv.lang` 切换 | 五语言的存储值、`html lang`、picker、标题、余额和空态逐项相同，十组页面异常均为 0 | 空态五语言 R1；真实轮次、展开流水和多页仍待同数据 R1 |
| Public 三个 alias 与非法 lot | 最终源码、`en` | `/auction`、`/account`、`/compensation` 分别收敛到 `/`、`/me`、`/compensation-pool`；非法 lot 本地失败 | canonical 地址逐项相同，非法 lot 不进入业务 API；四条路径无异常 | alias/非法输入 R1 |

截图只保存在本机临时目录，不作为仓库制品提交；表中的完整请求条件和结论才是审计记录。后续修复
对应差异时重新人工对照，并继续禁止把不同数据库造成的内容差异当成 WebUI 回归。

### 本轮实施证据

2026-08-26 已完成批次 A 中 `BID-001`、`BID-003`、`ID-001`、`WRK-001` 的静态修复：竞价写事务增加参与者
资格锁及失格代理前置冻结；普通出价按代理响应分支派生一条或两条确定性事实 ID；Admin/recovery
黑名单在同一事务冻结代理、追加子命令与转换证据、清理慢速调度并撤销公开会话。旧版
`LockActiveForParticipation` 只以用户参与状态为准，因此本轮没有把撤销登录 key 扩张成代理冻结条件。
Auction Worker 还恢复数据库时钟驱动的 slow due 扫描，并在竞价事务内重读 generation、截止时间、
活动授权和 owner 资格；命令 ID、请求摘要、stale 分类和 `lot.slow_proxy_tick` 审计沿用旧版格式。

以下验证均通过，且没有编写或运行任何测试用例：

- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo check --locked -p auction-application -p auction-jobs`
- `cargo check --locked -p auction-storage --no-default-features --features postgres`
- `cargo check --locked -p auction-storage --no-default-features --features sqlite`
- `cargo check --locked -p auction-worker --features postgres`
- `cargo check --locked -p auction-worker --features sqlite`
- 上述 Application/Jobs、Storage 两后端和 Worker 两 feature 的对应 `cargo clippy -- -D warnings`

这些结果只证明格式与两后端静态编译/分析，不替代 PostgreSQL Serializable 并发、命令重放、事务
回滚、Worker 重启和同库 Web/API 行为的 R1 证据；四个条目因此保留“待 R1”，不能提升为完整等价。

同日完成批次 B 中 `RT-001` 至 `RT-005`、`API-001` 的静态修复：Public 单拍卖 Catch-up 恢复
必填规范 UUID、默认 100、顶层 `auction_id` 和隐藏 `auction.created` 但保留 cursor fence；发现流改为
显式 `scope=auctions` 响应。WebSocket 恢复只接受一个目标键和逐帧单事件，Dioxus 客户端等待握手后
HTTP 补齐并在跳号时再次补齐。Admin realtime lag/failure 与 delivery 恢复 `data.pagination`；旧版
replay 请求缺少 `command_id` 时按 failure/schema/payload 稳定派生，新客户端仍显式复用 ID。

批次 B 已通过以下静态验证，且没有编写或运行任何测试用例：

- `cargo check --locked -p auction-contracts -p auction-http -p auction-web-public -p auction-web-admin`
- `cargo check --locked -p auction-web-public -p auction-web-admin --target wasm32-unknown-unknown`
- 上述四个原生 crate 的 `cargo clippy -- -D warnings`

WASM 目标的额外 Clippy 会命中本批未触及的 `clock.rs`、query、proxy success 与 Admin auction helper
既有 lint；项目冻结门禁要求的是两个 Web crate 的 WASM `cargo check`，本批没有借机改动无关文件。
上述结果仍不替代真实 WebSocket 帧、已认证 Admin JSON、同库 cursor/replay 和浏览器重连 R1。

随后在已删除的独立临时 SQLite 数据库和 18090/18091 端口运行当前 Public/Admin 二进制做只读 shape
探针：`GET /api/v1/events?scope=auctions&after=0&page_size=100` 返回带 `data.scope=auctions` 的 200；
省略目标或使用 `after=01` 返回 400，未知规范 auction 返回 404。临时 Admin 登录后，lag 返回
`data.pagination={page,page_size,total}`，failure 返回
`data.pagination={page,page_size,has_more,total}`。SQLite 按声明对 delivery Admin 返回
`feature_not_supported`，因此 API-001 未提升为运行态证明。探针没有写功能测试用例，临时服务、Cookie、
配置和数据库均已停止并删除，既有 8090-8093 对照服务未受影响。

同日完成批次 C 中 `BID-002`、`API-002` 至 `API-004`、`PUI-001`、`PUI-002` 的静态修复。代理创建
省略或传入空 `mode` 均恢复旧版 `invalid_proxy_mode`；全部业务写路由共用严格 JSON 媒体类型、逐路由
字节上限、单 JSON 值和 DTO 未知字段边界，错误外壳固定保留 `data: null`。业务 MethodRouter 显式拒绝
隐式 HEAD 并返回稳定 JSON 405/排序后的 `Allow`，健康、metrics 和静态资源继续保留各自方法 owner。
Public 规则读取把当前 Fluent locale 放入 `lang` 查询并以语言作为 reactive resource 依赖；认证 CTA
把完整 pathname + search 编码进 `return_to`，登录页只接受安全同源路径并在成功后 replace 返回。

旧版 8092 与当前临时 18092 Public 的只读 HTTP 对照确认：`HEAD /api/v1/worlds` 均为 405 且
`Allow: GET`，`HEAD /api/v1/session` 均为 405 且 `Allow: DELETE, GET, POST`，live/ready 的 HEAD 均为
200；缺失/错误 Content-Type 为 415，未知字段和尾随第二个 JSON 值为 400，超过 4 KiB 的 session body
为 413。当前错误响应均包含 `data: null`，`request_id` 是 Rust 的非敏感扩展。当前 WASM 的本地 Chromium
探针还确认 `/me?bid_page=2&proxy_page=3` 的认证 CTA 生成
`/login?return_to=%2Fme%3Fbid_page%3D2%26proxy_page%3D3`，编码后的登录路由可正常渲染；直接页头入口
保持精确 `/login`。这些探针没有写成或运行浏览器/功能测试用例。

批次 C 的最终静态验证范围为 `auction-contracts`、`auction-http`、`auction-web-public` 的 locked 原生
check/Clippy、Public WASM check、`dx bundle`、rustfmt、仓库策略和差异空白检查。仍需有真实用户与 live lot
的代理命令 R1、五种语言规则同数据切换、有效登录后的最终返回，以及 Admin 大 body 路由对照；因此本批
条目只按各自行状态记为“静态修复”或“局部 R1”，不能提升为完整等价。

同日完成批次 D 中 `WRK-002`、`WRK-003`、`AUI-001` 至 `AUI-003`、`CFG-001` 至 `CFG-004` 的静态修复。
论坛主题写入结果未知时先进入精确标题只读对账，命中只补本地证明，未命中必须由
`authorize-forum-write-retry` 的持久化幂等命令和逐字确认显式打开下一次 POST；PostgreSQL
`000016_forum_sync_write_authorization` 保存授权命令与追加审计。MoogleMail 单封业务处理只对
`Retryable` 错误和超时执行有界 inline retry，确定性 `Permanent` 错误立即收口且不推进 cursor。

Admin runtime 已恢复旧版 `worlds`、`equipment_filter` 和 14 个 `mail_templates` 的闭集模型、校验、页面
事实卡和 JSON 编辑；Worker 在启动时从同一文件投影这些业务字段，同时继续由主配置拥有 transport、
凭据、调度和全局启用开关。旧 runtime 缺少三组字段时保持原 fingerprint/命令摘要与 Worker 主配置
行为，首次保存才物化旧版默认值。Admin 页面统一解析稳定错误 code 并映射五种 Fluent 文案；凭据表
恢复旧版实际合成的 last checked/last used/imported by 三列，未扩展敏感 DTO。配置层同时恢复 metrics
仅允许 loopback/Unix、origin allowlist 缺省开启、Admin 15 分钟/8 小时会话及装备来源 250 ms 默认值。

本批已通过目标 crate 原生/WASM 编译、格式与 config/i18n 门禁，且没有编写或运行任何测试用例。
论坛/MoogleMail 真实外部回执、Admin 其余错误分类、配置保存和 Admin/Worker 重启仍需 R1，因此这些
条目只按各自已取得的局部证据提升，不能整体宣称完整等价。

配置只读诊断进一步确认：Worker 示例从共享 runtime 得到 `main`/`isekai`、14 个模板、空发件人
allowlist 和关闭的装备过滤，同时全局 MoogleMail 仍保持关闭；Admin 示例输出 900/28800 会话默认值
及装备来源 250 ms。临时非敏感 Public 配置在 `0.0.0.0` 启用 metrics 时以退出码 2 失败，改为
`127.0.0.1` 后通过；临时文件随后删除。该证据把 CFG-001/003/004 提升为局部配置 R1，但不替代
真实 scrape、认证会话时钟或外部装备请求节流。

随后在互不占用长期对照端口的隔离环境完成 Admin 浏览器 R1：SQLite `127.0.0.1:18091` 证明
能力缺失时配置页和 Cookie 池均显示稳定的“当前部署档位不支持此操作。”；PostgreSQL 18
`127.0.0.1:18092` 使用全新临时库、当前迁移和独立浏览器 profile，已认证读取配置页两张旧版事实卡，
并从实际 JSON 文档确认 main/isekai、14 个模板、250 ms 与 900/28800。无副作用的非法候选校验返回
HTTP 400，页面显示本地化稳定错误；隔离库中的临时凭据行显示旧版 last checked/last used/imported by
语义和新版 version 列，secret 未出现在列表且提交后密码框清空。该过程只写入随后清理的临时库，未写
生产或旧版仓库。

同一浏览器对照还发现并修复共享语言选择器的初始选中态：修复前 `hv.lang=ja` 刷新后译文及
`<html lang>` 为 `ja`，但下拉错误显示 `en`；显式绑定 option `selected` 后三者一致，并已依次切换
`en`、`zh-CN`、`zh-TW`、`ja`、`ko`，各自标题和主标题均使用对应译文。该证据关闭 Admin picker 的
局部运行态缺口；后续 Public 规则正文和补偿池也取得五语言 R1，其余逐页长文案、移动端语言布局仍按
EVD-002 保留。

同日继续完成批次 E 的可见一致性修复，并把当前源码重新 bundle/内嵌到隔离 Public 18190 与 Admin
18191；旧版 8092/8093 保持只读。Public `/bidlog?bidlog_page=2&world=isekai` 首次完整加载后规范化为
`?world=isekai&page=2`，同时传入 `page=3` 时 canonical 值优先；页面内切换 main/isekai 后执行浏览器
back/forward，地址、页头 world 与页面 world 均同步。Router 前快照解决首次挂载丢 query，带 key 的
Outlet 只在 `popstate` 重建当前路由内容；自建 history 条目继续使用 Dioxus 的两项滚动坐标 state。
`/auctions` 对 settled/live 均实际请求 `page_size=10`，但隔离数据没有对应行，因此状态徽章、Lots/
Bid log 入口和总页数只取得静态证据，PUI-004 仍保留同数据行级 R1。

Public 非法 lot 路径已显示本地 “Lot not found”、无 Retry，浏览器 performance 中没有业务 lot API。
`/status` 新旧均只有一个 H2 和同一说明，无 details/table；复位共享基础样式覆盖后，H2 和说明矩形与
旧版逐项相同。整图比较只在动态页头控制区发现 36 个像素相差 1 色阶，页面主体无像素差异。390×844
首页新旧截图则取得完全相同 SHA-256
`40fdadeaa827911f7a0d9cd17f43085987482c7fa46aa69121f8e623c375d957`，刷新按钮、语言选择器的矩形、
顺序和间距一致；1440×1000 桌面基线继续不变。Public/Admin title 已恢复旧值，当前 `/favicon.ico`
与旧指纹 SVG 字节哈希相同，并确认 favicon 200/immutable、SPA HTML 200/no-store、未知路由
404/no-store。

Admin 18191 登录页在 1440×1000 的卡片矩形为 `[544,64,352,250]`，文档高度恰为 1000；390×844
文档高度恰为 844，均无无意义滚动。授权登录后的 Users 创建框具备有效 `aria-labelledby`，打开即把
焦点放到 `admin-users-username`，Tab/Shift+Tab 双向循环且背景 body 锁滚动；点击取消后弹窗关闭、
滚动锁清除并把焦点恢复到“创建用户”。Users/凭据行的 `status`/`alert` 与唯一描述关系完成静态核对。

同日继续处理 DATA-001。旧版 `DirectoryReader` 源码证明配置目录存在时它优先读取
`{locale}.md`，只在文件不存在时回退数据库；对旧运行库的受控只读查询得到 `rules_documents=0`，
因此目录是当前唯一权威源，canonical v28 接管本身会遗失规则。新增
`auctionctl import-rules-directory` 后，在隔离 PostgreSQL 使用批次 UUID
`af7d92a1-8921-43be-9a3f-6c10b5418d27` 首次得到五项 `imported`/revision 1，再以同一命令重放得到
五项 `preserved`/revision 1，缓存失败均为空。Public 18190 无需重启即返回五个 200；正文 bytes 与
SHA-256 分别为 `en 4363/80763d86e1f94333f3fc9f632bac3043bacfb521ac2383c87504cc9436ececc8`、
`zh-CN 3737/1cda3c27c9bea018f0b6d1c6fcccb43a86f02058c67fef1f75c7be54dd6373b2`、
`zh-TW 3744/07ca52e54ee6813200136480234f51b6349b9a08c4d713957dd334328e891abe`、
`ja 4849/10b19f4fe3219ea8f23c8fb8bf028bcbd1c9ab08fd6b4dd5fc5d1b17bda4410b`、
`ko 4205/de97bc98bd33175efbb28f63671cee46d531f42c7bf63014b68e935e222a22d7`，逐项与旧 8092 相同。
旧目录响应的 revision/published_at 来自文件 mtime；导入后由新 Admin 发布边界从 revision 1 和导入
时刻开始。这是权威源切换的显式映射：正文/标题保持逐字一致，revision 继续作为不透明 CAS 版本单调
递增，Public 页面不展示这两项元数据。

同数据浏览器取证随后发现并修复规则页额外 metadata、正文宽度和共享 Markdown 主题差异。在
1440×1000 下，旧 8092 与新 18190 的五种语言均表现为同一 `article > h1 + div`，metadata 和嵌套
Markdown article 数量均为零；每种语言的标题、`hv.lang`、`<html lang>`、正文文本 SHA-256、全部
抽样矩形、计算样式及页面高度逐项相同。整篇规则裁剪 PNG 的新旧配对 SHA-256 分别为
`en 9703b033968e5c5b6b62dab1ae2df36857bff93f11a36a449ac7424d42427945`、
`zh-CN 0362048113864b55c34ed3291884cb67a5c5a9958413213d6ab42ceb84a0f50a`、
`zh-TW 3e6a5ac03e302116533be4e1e7082da76c711547db413d8f03feab6cff184d8c`、
`ja 080d26f6a73cc8a826573ef11cf55463f263728f6df2165e35050c924337527e`、
`ko 87ae6c116a863553c8eada33854426686a8d46dbd5678a602fb22747ba628229`。内部 HTML 哈希因 Dioxus
安全文本 span 和透明 table wrapper 不同，未把实现字节相同误报为验收条件。

DB-001 随后在独立临时 SQLite 文件和 18290 监听器上完成运行态收口。迁移成功后，
`GET /api/v1/price-history?page=1&page_size=50` 返回 `501 Not Implemented`，响应 code 固定为
`feature_not_supported`、`data` 为 `null`；这与 PostgreSQL 完整档继续从
`auction_settlements`/`lot_settlements` 读取成交事实的边界一致。Chromium 逐项切换 `en`、
`zh-CN`、`zh-TW`、`ja`、`ko` 后，`hv.lang`、`<html lang>`、页面标题和本地化能力说明均对应，
`.public-history-filters` 与能力状态内 Retry 按钮数量均为零，状态区域保持 `role=status`。临时进程、
SQLite 文件和构建目录已删除，8090/8091、8092/8093 及三条长期任务未触碰。

OPS-001 随后从旧对照实例 `auction_bf_compare_20260825` 只读取得 PostgreSQL 18.6 canonical v28
快照：`schema_migrations=28/dirty=false`、59 张 public 表，专用预检的 source shape、policy、lot
projection 和 bid event 问题计数均为 0。原始只读 dump SHA-256 为
`a9e2edd6fc1d4adb83c5fcf852d52b0a3e009f0ef84618cdb2e5fe5374e2bb81`；实际
`postgres-legacy-v28-backup` 产物 SHA-256 为
`5a889b22ab9f89cfc2b3036b2c961534c9e234981664e184bcca526952157634`，manifest SHA-256 为
`e17ca0acfaf6aeb340fa2bb39b6b3701dcfcfbeb3074b0b91e3edca61c281a5a`，预检指纹为
`a553ddf4bff7b7d1b389be98b7a2657bf8f555bf87279253303d787763ccebc2`。archive 已离线 verify，
又实际恢复到独立临时库并再次以同一 manifest 通过 dry-run，证明恢复点可用而非只验证文件存在。

首次真实执行依次暴露并修复四类原有阻塞：PG18 不接受 source check/bridge 中的关键字别名；接管先
通过连接池预检再取锁会把自身空闲连接误计为外部 session；recovery 盘点使用的 catalog 关键字别名和
`pg_user_mapping` 基表分别导致语法错误与普通 owner 权限错误；具有 `CREATEDB` 的目标 owner 可隐式
使用默认表空间，却不能显式写出 `TABLESPACE=pg_default`。最终实现改为在同一持锁物理连接内做完整
只读复核，使用语义别名和公开 `pg_user_mappings` 目录视图，并仅对非默认表空间写出 TABLESPACE。
实际同角色创建探针和后续 rebuild 均证明该权限模型成立。

从再次恢复的 v28 起点执行接管后，receipt 报告 legacy 28、SQLx 最终版本 16 和审计 ID
`7cd852b1-748e-425e-bd40-1331ebcfd8ff`；receipt 为 0600，SHA-256 为
`3044a9f873b3d623b84a783de01cabd4133b32186c60d4cd0ee2518bd75b21c8`。普通 `auctionctl migrate`
随后成功，`_sqlx_migrations` 精确包含 `1..16`、全部 success 且 checksum 非空，schema identity 为
`auction/1`，`bids_append_only` 与 `bids_contiguous_fact_index` 均保持启用。旧副本的
auction/lot/bid/user/realtime 行数均为 0，audit 为 2044；接管只追加一条 adoption 审计。五语言目录
规则随后首次全部 `imported`、同批次重放全部 `preserved`，正文 SHA-256 与 DATA-001 的旧目录证据一致；
规则发布后 audit 为 2050、`rules_documents=5`、`rules_admin_commands=5`。

当前 Rust schema 备份 archive/manifest SHA-256 分别为
`96e97747c6b1f3f621f964ee56a0d8f3f6f79215999248326f850eb21b150a8f` 和
`4d6f6e040884319d370485297df72afd24e44dab0e51c60edad29ee246eed913`，离线 verify 通过。
`postgres-rebuild --mode empty-for-restore` 把目标 OID 从 `4674510` 变为 `4676862`，空库预检得到
`user_objects=0/passed=true`；`postgres-restore` 再以单事务恢复 schema 16。恢复前后均为
`audit=2050`、`rules_documents=5`、`rules_admin_commands=5`、迁移 `16/16`，规则聚合 MD5 均为
`931ec83d360779aeb37d5524f2b7b3eb`；rebuild/restore receipt 均为 0600，SHA-256 分别为
`dbb18668df7c8f8ad6524cd467539e104807d2737f7dbf5e929153eca506a82a` 和
`34d13269e17b8153f628d5a4d95f2036085df1cbeee8a418cfb53db78aa008b3`。这关闭 OPS-001 的隔离运维链
证据，但由于旧副本没有非空核心拍卖事实，不把它外推成非空生产数据逐行等价或部署切换批准。
取证结束后已精确删除临时数据库、专用角色、恢复用 archive 副本和 0700 私有目录；数据库 catalog
确认目标均不存在，后台只保留既有旧版 8092/8093、新版 8090/8091 和 Chromium 三条长期任务。

OPS-002 随后按旧版 Makefile 和 `release-check` 的实际合同补齐。Public/Admin 装配根使用显式
`web-embed` feature 区分 API-only 与 embedded；无 feature 的二进制固定指向不存在的隔离目录，不能因
工作区残留 Web 文件而偶然嵌入。`ci/web-bundle.sh` 每次同时清空 Dioxus 实际 release 中间目录和稳定
嵌入目录，再使用 `wasm-release`、关闭 debug symbols 构建，生成 payload 明细和版本化 manifest，并把
mtime 规范化到源码提交时间。首次实现由此发现只清理名义 profile 目录会让 Dioxus 复用
`release/web` 中的旧指纹 JS/WASM；修复后每个 surface 都精确为六个 payload 文件和一组 JS/WASM。
Public/Admin 的 payload SHA-256 分别稳定为
`2994df6e772adaeaa36db061008a8fa6992232cf3eb869a48bbaafb97a50cc00` 与
`08adecb548fe13edf0998aaac18312dfd16cdc97822709e69cb4166f5bc56d6c`，WASM 分别约 1.61/2.23 MB。

`ci/release.sh` 在独立 Cargo target 中构建四个 API-only 和四个 embedded ELF，记录 commit、完整工作树
指纹、`Cargo.lock` 哈希和 feature 闭集；核验会扫描真实二进制中的 payload 摘要/标题，证明 API-only、
Worker 和 `auctionctl` 无 Web 数据，Public/Admin 只含自己的 surface，并检查 Worker/Public/Admin 依赖
边界。隔离 PostgreSQL 与 18390/18391 端口的人工 R1 进一步证明：API-only 两端 ready 为 200、根路径
均为 503 和同一明确构建提示；embedded 两端根路径为 200、旧版 title 正确，自有 JS 为 200、跨 surface
JS 为 404。发布包使用成员 allowlist、固定 0644/0755、root:root、提交时间、GNU ustar 与 `gzip -n`
双次构造并比较哈希，再离线复核成员、权限、owner、mtime、内部 SHA256SUMS、源码/依赖清单和外层发布
校验和；默认拒绝 dirty 工作树，显式 `--allow-dirty` 时文件名和 manifest 均记录 dirty 与源码指纹。

OPS-003 随后先从旧版 `ops/deploy/start.sh` 冻结实际合同：严格配置/地址准备和前向迁移后按
Worker → Admin → Public 逐角色等待 readiness，失败只逆序回滚本次启动的角色；停止顺序为
Public → Admin → Worker，systemd 发送 SIGTERM 并由进程执行有界 drain；Caddy 不向公网代理
`/metrics`。新版 `auctionctl deployment` 的控制器、unit 绑定、InvocationID 回执和 Caddy 模板静态
调用链与该合同对应。

运行态使用 systemd 257、Caddy 2.11.4、独立的 `auction-ops003-r1-*` unit、18590/18591 应用端口、
18680/18681 代理端口、临时 PostgreSQL 角色/数据库和隔离 `/opt`、`/etc`、`/var/lib`、`/run` 目录。
`deployment-check --strict-permissions`、`systemd-analyze verify` 和 `caddy validate` 全部通过，启动时
SQLx 迁移为 16/16。首次启动的 monotonic 顺序为 Worker `86446558499`、Admin `86446795741`、
Public `86447035481`；启用本地 metrics 后由控制器 restart，顺序仍为先反序停止再正序启动，新的
monotonic 值为 `86551263381`、`86551519187`、`86551823167`，三个 InvocationID 均变化且与 0600、
root:root、单硬链接的启动回执匹配。直接 `/health/live`、`/health/ready` 和本地 `/metrics` 为 200，
Caddy 代理的两个 readiness 为 200、两个 `/metrics` 固定为 404；Public/Admin 根路径均返回带旧版
title 的 embedded 页面。

正常停止日志逐角色记录 `drain started` 和 `stopped`，三者退出码均为 0；回执和 Worker control socket
随停止移除。故障演练把隔离 Public unit 的 `ExecStart` 暂时替换为 `/usr/bin/false`：Worker、Admin
先就绪，Public 五次失败后控制器以退出码 4 和稳定标记 `start-failure-v1=public-readiness` 收口，随后
按 Public → Admin → Worker 回滚，回执不存在、18590/18591 无监听，而数据库迁移仍保持 16/16。恢复
原 unit 后三角色再次成功就绪，最终回执 SHA-256 为
`af077deee35086ebdb70a2b9562918b61dc63e234f9429ec323d05a0f57c0cc2`；再次受控停止成功。三角色
journald 对连接串、Cookie 和密钥字段的敏感模式扫描均为零命中。

演练结束后已精确删除四个 unit、五个隔离目录、临时数据库和角色，四个高端口均无监听，数据库 catalog
为 0/0；后台只保留既有旧版 8092/8093、新版 8090/8091 和 Chromium 三条长期任务。这关闭 OPS-003
的同宿主机隔离生命周期证据，但没有使用真实域名/TLS、专用生产账号、正式 secret、外部服务或真实
流量，因此不构成生产切换批准。

EVD-002 随后继续使用旧版 8092 和新版 8090 的实际 embedded 制品完成 Public 对照。先逐页截图并读取
DOM/计算样式，发现并修复一页时隐藏或文案不同的共用 pager、拍卖/竞价/价格历史空态、登录和账户 CTA
间距、黑名单与补偿池布局。补偿池此前把需要 `$balance` 参数的 Fluent 消息误用于 `aria-label`，会在
成功 API 响应后触发 WASM `unreachable` 并永久停在 Loading；现已拆成无参数标签和独立金额，最终页面
显示 0K 与稳定空态。价格历史还修复了 `font` shorthand 覆盖 Submit 字重/内边距的问题。

世界接口的新旧 JSON 完全相同；旧版顶部下拉实际按 Isekai、Main 排列但默认 Main，页面筛选则按
Main、Isekai 排列。新版现将两种顺序分开，并像语言 picker 一样逐项声明原生 `selected`，最终顶部计算
状态为 options `[isekai, main]`、value/selected `main`。`/history` Submit 的 94.171875×44、字重 600、
16px/4px 内边距也与旧版逐项相同；`/login` 新旧 PNG SHA-256 完全相同。最终 14 条 Public 路径在
1440×1000、`en` 下均无残留 Loading 或浏览器异常，三个 alias 收敛到同一 canonical 地址。

随后分别在新旧补偿池逐项切换 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`；十组结果的 `hv.lang`、
`<html lang>`、picker 值、标题、0K 余额和空态文案逐项一致，异常均为零。截图只保存在本机 `/tmp`，
没有提交仓库；这轮仍未覆盖动态列表行、真实竞价/代理、有效登录、错误/冲突、移动端逐页状态和 Admin
其余页面。上述接管、发布、部署和浏览器操作都是一次性人工对照，没有写入或运行浏览器/功能测试用例。

## 2026-08-27 第二轮并行审计结论

第二轮不是重新按文件名计数，而是从旧版可执行入口反向追到状态、SQL、持久事实和 WebUI 状态，再从
新版真实装配根向内交叉验证。五个并行任务分别覆盖 Public、Admin、Worker/外部集成、数据库事务和
部署/制品，主代理复核所有 P0/P1 以及会进入实现的 P2。结论按以下边界冻结：

- 竞价切面确认冻结基线错误排除 `final`、缺少 world enabled/scheduled open gate，且 PostgreSQL 未按
  world → auction → lot 锁定；现已恢复对应状态闭集、数据库时间 gate 和锁序，运行态并发证据仍待补齐。
- 结算切面确认冻结基线的 `settling` 主路径误用了只允许 `live` 的通用快照，pool disabled/closing ledger、
  `auction.settled` wire、稳定 ID 与重放校验也不同；这些项目现已静态修复，资金事实和重放 R1 仍保留。
- Worker 切面确认新版能处理已有 scheduled round，也能由 preview 创建 successor；补齐范围准确限定为
  空库/current-week bootstrap、错过窗口恢复及旧 MoogleMail pause/resume gate。实现复核同时修正了
  PostgreSQL 必填计划列、Serializable 三次重试和旧 preview/open 命令 ID，运行态调度/邮箱证据仍待补齐。
- WebUI 切面以旧组件实际请求参数和状态分支为准。动态列表 page size、session 四态、stale conflict 和
  有数据表格结构差异已静态修复，但路径齐全、空态截图或编译成功均不能替代动态同数据页面 R1。
- 运维切面保留 OPS-001 至 OPS-003 的隔离 R1；独立 `migrate` 停机门禁、重复 `stop` 与显式非破坏性
  PostgreSQL bootstrap 均已实现，三者不能由一次完整 deployment 成功互相替代。

以下差异已审计但本轮不按旧实现机械回退：

- 新版内部持久化 `final_fast`，并在 legacy view/account 映射为旧 `active`，属于已声明的内部建模差异；
  只有发现兼容边界泄漏到旧 HTTP/事件时才升级为缺口。
- 论坛 POST 未知结果改为先对账、再持久授权重试，是防止重复外部写的安全增强，继续保留 WRK-002 方案。
- 结算时预建 fulfillment 行相较旧版有额外安全意图，继续作为设计复核项；业务事实时间已按 SET-006
  统一为冻结 `due_at`，数据库当前时间只保留租约和失败观测职责。
- SQLite Lite 没有旧版 settlement 完整能力；SET-001 至 SET-010 与 AUD-003 的实现和验收以 PostgreSQL 完整档为主，
  只把两后端共有的竞价 gate 修入 SQLite。
- 语言支持闭集仍是 Public/Admin 各 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`。旧 Admin manifest 默认只启用
  `zh-CN`，新版 Admin 扩展到五种语言是当前项目契约，不是待删除功能；逐页翻译质量仍归 EVD-002。
- 旧版非法 `world` 首屏的 main/isekai 双请求来自 shell 与页面重复拥有 world 解析，且页面错误使用
  worlds JSON 首个插入项回退，能够造成页头和主体世界不一致。新版保留地址栏中的非法值，但由共享
  owner 一致回退 main；该旧运行缺陷不作为请求契约回迁。

## 实施顺序门禁

第二轮矩阵已先于新增实现冻结，当前按以下状态推进：

1. 批次 A0（竞价/结算阻断）已完成静态实现：BID-004 至 BID-006、SET-001、SET-002 使用独立竞价/
   结算快照、数据库权威 gate、稳定锁序和统一 pool/ledger 分支；待同库与并发 R1。
2. 批次 A1（结算兼容）已完成实现：SET-003 至 SET-006、SET-009 与 AUD-003 恢复 wire、旧自然键
   ID、due 事实时间、旧账本只读接管和 claim/retry/final 审计；非空延迟结算及三类审计已取得隔离局部
   R1。fulfillment 建行时点继续作为设计复核边界，并发、非零资金和真实旧接管样本仍保留 R1。
3. 批次 B（Worker）已完成 WRK-004 至 WRK-006 的静态实现；current-week bootstrap、旧稳定阶段命令、
   missed 轮次及 mail gate 均由持久 Worker/config/数据库 owner 承担，不引入内存权威状态，待调度 R1。
4. 批次 C（WebUI）已推进到 PUI-013 至 PUI-021、AUI-010 至 AUI-019 与 I18N-003；AUI-014 的
   Dashboard/Status、PUI-016 的会话四态/失败/登录动态错误、PUI-019 的 main 五态、PUI-020 的匿名状态
   切换及 Public/Admin 五语言初始化已取得局部 R1，PUI-015 的 URL/精确 GET 请求边界和 PUI-021 的合法
   lot 可选字段已收口；AUI-018 的 Admin GET 精确 URL、AUI-016 的 Mail 390px populated 表格也已取得浏览器
   R1。PUI-014、PUI-020 认证路径、重新打开的 AUI-013 Users/Renames、AUI-016 其余页面、AUI-017
   与 AUI-019 继续按冻结差异实施；顺序保持
   wire→冲突生命周期→页面投影→响应式/dialog，不新增浏览器测试脚本。
5. 批次 D（运维）中 OPS-004 至 OPS-006 已静态实现；普通 `migrate` 不隐式取得 CREATEDB，bootstrap
   只创建明确缺失的 PostgreSQL 数据库，已有库必须 owner/应用连接身份一致且不会被覆盖或删除。
6. 批次 E（证据闭环）：保留既有静态修复和 OPS-001 至 OPS-003、DATA-001、DB-001 的 R1；对本轮新增
   修复执行格式、目标 crate 编译/Clippy，再在用户授权环境补充同库/同页面人工 R1。

第二轮实现收口后，`cargo fmt --all -- --check`、完整仓库策略、`git diff --check`、全工作区/all-targets
编译与 all-features Clippy、PostgreSQL/SQLite 两种 Worker/Storage 档及 Public/Admin WASM 编译均通过。
Public/Admin 最新 Dioxus 产物也已重新 bundle 并内嵌。随后用彼此隔离的 PostgreSQL 数据库同时启动
新 Public/Admin 与旧 Public/Admin：四个根路径以及各自 live/ready 在本机均返回 200，四个根路径通过
同一局域网地址也返回 200；新版数据库已受控导入旧版五语言规则正文。该证据证明当前制品可访问和
基础装配闭合，但两侧数据库内容不同，不能替代动态同数据、认证动作、并发事务或外部服务 R1。本轮仍
未新增或运行单元、集成、功能、端到端或浏览器测试用例。

同日继续收口 `AUD-001`：双仓库源码与隔离 PostgreSQL 证明旧外部 Public 竞价审计以命令为 owner，
而不是以 bid/proxy transition 为 owner。新版现由 Application 在同一 Serializable 事务中把唯一审计
上下文交给 Storage；资格冻结仍先写权威 transition，但不额外写 Admin 审计。重建 8090 后，同源
fast 双事实普通竞价、代理降低和新代理创建的首次/重放均为 200，旧新审计条数、action、分类、NULL
request_id 和 metadata 闭集逐项对应；两条普通 facts 的金额、机制与版本也完全相同。该人工 R1 没有
编写或运行测试用例；Final/失格生命周期组合由下述 AUD-002 收口，普通竞价独立冻结和结算关闭仍保留
证据缺口。

随后完成 `AUD-002`。生命周期端口把锁内旧上下文、动作计划和取消写入摘要交给 PostgreSQL/SQLite
record owner，父审计恢复旧 action、分类、NULL `request_id` 列及逐动作 metadata；Final 复用权威
competition 写入但抑制通用审计，再追加唯一 `lot.proxy_final_transition`。隔离 PostgreSQL 对照覆盖
Preview、Open、Draft/Preview Cancel、无代理 Final、slow Final 和资格丢失 Final：首次响应、版本、审计、
realtime、lot 状态与返还义务数均对应；slow Final 产生一条 53,000 proxy fact，失格分支的 `facts` 均为
JSON `null`。slow Final 重放两版均不重复追加；Cancel 重放暴露旧版 503，而新版依照 I-01 返回原结果且
同样不重复追加，因此没有把旧运行时缺陷复制回来。修复后的 rustfmt、差异空白、四项仓库策略、全工作区
all-targets 编译、all-features Clippy、PostgreSQL/SQLite/Redis feature 及 Public/Admin WASM 编译均通过；
本轮仍未编写或运行任何测试用例。

随后完成 `SET-009` 与 `AUD-003` 的静态实现。Application 结算用例把账本查找前移到拍卖锁之前，并在
局部兼容层逐字复现旧 Go complete 请求摘要；旧结果经过严格闭集解析后，还要与 settled auction、
`auction_settlements`、`lot_settlements` 和 `settlement_obligations` 聚合逐项一致，才返回只读重放计划。
Storage claim 接管同样只在已有 settling/settled workflow 识别旧 claim 账本；新写入仍保留当前格式，
没有迁移或改写历史行。结算所有业务事实改用冻结 `due_at`，Isekai carrier COD 的义务来源 world 保持
`isekai` 而 fulfillment dispatch 固定为 `main`。

claim、retry、final 三类审计已恢复旧 action、`system/auction-worker/worker`、NULL request 和各自 metadata；
retry 在 Serializable 最多三次事务内按数据库观测时间原子递增 attempt、计算固定退避并写审计，Worker
单项超时在独立两秒窗口尽力执行同一收口。目标 Application/Storage/Jobs 的 all-targets check 与
all-features Clippy 已通过；后续仍须用真实接管账本、非空结算 fixture、故障注入和并发冲突补 R1，不能
把静态门禁提升为完整等价。本轮仍未编写或运行任何测试用例。

随后使用当前运行库的只读副本，在当前用户独立启动的 PostgreSQL 18 临时实例中执行非空结算 R1；
论坛、MoogleMail、operations alert、Worker control 与每周 bootstrap 全部关闭，旧仓库和当前 8090/8091
数据库均未写入。第一轮真实事务在 `compensation_ledger` INSERT 处暴露
`auction_commands.auction_id` 不存在；领取事务已按冻结 due 写入，结算事务整体回滚，失败 owner 同事务
写入 attempt=2、5 秒退避及唯一 `auction.settlement_retry_scheduled`。实现随即改为读取命令账本权威列
`aggregate_id`，并从全新副本重建 PostgreSQL Worker 制品后重跑。

修复后的同一到期 Isekai 单 lot 从 final/live 原子推进为 settled/sold：状态版本 5→7、attempt=1，
结算摘要、lot 事实、opening/closing 补偿账本、四类义务及 fulfillment、claim/final 审计和 realtime
事实均唯一提交；Isekai carrier COD 的义务 world 为 `isekai`，dispatch world 为 `main`。实际执行晚于
due 约三小时，但命令 created_at、started/due/settled/updated、lot、摘要、账本和两条成功审计均精确
等于同一冻结 `due_at`。第二次启动后状态版本、attempt、摘要、义务及审计计数不增。该证据把
SET-001、SET-002、SET-006 与 AUD-003 提升为各自行标注的局部 R1；pool disabled、非零资金、并发冲突、
真实旧 claim/complete 账本和外部履约仍保留，不能把 SET-009 或整条结算链提升为完整等价。全过程没有
编写或运行任何测试用例。

随后以同一隔离双库中的共享 main draft 执行 Admin intake→preview→open→Public read 联合 R1。canonical
JSON 使用旧协议闭集 `One-Handed/Axe/Legendary`、空 stats、10 位 key 和 1100 起拍价；首次与更换
`x-request-id` 的同 command 重放两侧均为 201，最终各只有一条 command/submission/lot，投稿 owner 均为
`admin`。在把不属于本链验证范围的 final 前驱受控标为取消后，preview/open 两侧均为 200：状态版本
`2→3→4`，`preview_revision` 始终为 1，预览项 `source_revision=1`。Public lot 详情两侧均返回
`start_price/current_price/next_bid=1100`、`has_leader=false` 和 `status=live`。该证据提升 BID-008、INT-001、
INT-002、LIF-001；夹具直接调时和前驱条件只用于建立隔离前提，不作为调度或取消行为证据。

同轮 Settlement 独立复核又发现三处静态缺口并完成最小修复：lot adjustment writer 移除 reader 会拒绝的
`policy_version`；补偿比例恢复逐 lot 不超过折扣需求的封顶；settling claim 候选和续租 UPDATE 同时接受旧版
`settlement_next_attempt_at IS NULL`。rustfmt 与 PostgreSQL storage feature 编译通过；合法/损坏报告、非零
补偿封顶和旧 claim 中断恢复仍缺运行态 R1。旧 complete 只读重放的真实入口是否进入 `auctionctl` 仍须按
SET-009 冻结，不能以内部函数存在宣称接管完成。本轮仍未编写或运行任何测试用例。

随后按 `PUI-015`、`PUI-018`、`AUI-012`、`AUI-013` 落地第一组确定性 WebUI 差异：Public world 查询与
服务端目录投影收紧到 `main/isekai`；后续运行对照纠正了最初对旧 URL 的误读——缺失/非法值不改写地址，
用户切换用 push 且 Main 删除 world，只有服务端选项使当前 world 失效时才 replace。lot 返回和
历史上一轮改用 Router `Link`。Admin 创建 sequence 恢复默认 `1`，凭据成功后复位四个输入，Settlement
移除旧版没有的标题说明和 movement count 可见文本，报告事实仍保留该计数。rustfmt、五语言 i18n 门禁及
Public/Admin WASM check 均通过；无刷新导航、非法 URL、成功表单和同报告动态页面仍保留人工浏览器 R1，
Users/Renames/Mail 主表与 polling 外壳继续归 `AUI-013`，没有编写或运行任何测试用例。

第二组按 `PUI-014`、`PUI-016`、`PUI-020` 恢复 Public 共享请求 owner。外壳现在唯一读取 session 和按世界
current 快照，SessionMenu、AuctionData、lot detail 与 `/me` 只消费同一资源；`/me` 在认证成功前不挂载
受保护列表，lot session error 恢复独立 alert。实时 owner 依据共享快照在 auction-specific/discovery 中
二选一，移除 `AuctionData` 的第二条连接，目标 key 变化会取消旧 future 并为新目标建立独立 cursor。
Public 原生 Clippy、WASM check、rustfmt 和差异空白检查通过；随后 header 的 loading/error/null/authenticated
入口、退出失败、坏响应、10 秒 timeout 与一次约 1 秒 retry 已按 PUI-016 取得局部 R1；认证 WebSocket 连接数、
取消/重连/catch-up 与移动端账户表几何仍需人工浏览器 R1。

第三组按 `AUI-013` 恢复 Admin 动态列表闭集。Users 移除主表 key/version 两列，Renames 移除记录/用户
内部 ID 两列并把确认行跨度恢复为 6；Mail 恢复旧 8 列、From/Subject 占位、Error 投影和筛选顺序，安全
详情通过外部 ID 单元格进入而不新增动作列。邮箱状态查询不再改变 intake 查询，polling 改回默认折叠并
独立处理 loading/error，单侧失败不会遮蔽收件列表。rustfmt、五语言 i18n、Admin WASM check、Clippy
和差异空白检查均通过；同一非空/空/多页数据、单侧失败、默认折叠和响应式几何仍需人工浏览器 R1，未
编写或运行任何测试用例。

第四组按 `AUI-014` 恢复 Outbox 外壳。failure 回到旧 12 列与占位投影，resolved 行不再显示禁用 Replay；
lag 回到 consumer→stream 五列并连同 loading/error、表格和分页整体置于默认折叠区。failure 与 lag 使用
独立 refresh owner，一侧 pending/error 不再遮蔽另一侧成功数据；failure 筛选保持独立折叠，replay 对话框
恢复旧标题、事件警告和 JSON/schema 本地校验，稳定 command_id 继续保留。Admin WASM check、Clippy、
rustfmt、五语言 i18n 和差异空白检查通过；动态单侧失败、折叠、12 列内容及 replay 结果仍需人工浏览器
R1，本轮未编写或运行任何测试用例。

第五组按 `API-013` 与 `AUI-014` 收口 Dashboard/Status。HTTP Dashboard 恢复旧 section/data 键闭集、
known/unknown/degraded 形状和 disabled alerting 语义；Contracts 与浏览器端都拒绝未知字段，扩展的 credential
scope、restart、recovery evidence age 不再泄漏到旧 v1 wire。Dashboard 只保留一次 snapshot 请求并恢复旧卡片
顺序、轮询/暂停操作和快捷入口；Admin `/status` 恢复为零业务请求的静态说明页。已认证新旧请求均为 200，
Dashboard 顶层十键和 section 闭集逐项对应，每页各只有一次 owner 请求；Status 两边都只发生 readiness/session
外壳请求。桌面与 390px 的 header、12rem 固定侧栏、content 起点和宽度也逐项相同。对照中继续修复了退出
文案、移动侧栏、Cookies/Renames 导航名和不可点击 Logo。重建后的 embedded 制品已在同一浏览器上下文复核：
1440px 与 390px 的导航项/顺序/链接、Logo、退出文案和壳层几何均与旧版一致；Dashboard 两边各只有一次
dashboard owner 请求，旧 session 资源记录为两次、新版为一次；Status 两边均只有一次 session 且正文、H1、
几何完全相同。Contracts/HTTP/Admin 原生、Admin WASM、i18n 与格式门禁通过，本轮未编写或运行任何测试用例。

第六组并行复核冻结了 `API-014`、`AUI-010`、`AUI-016`、`AUI-017`、`PUI-016` 与 `PUI-019`。其中阻断项是
recovery confirm 成功结果把旧 `recovery.id` 改成 `recovery_id`，以及旧版四类合法 409 的 dialog/alert/reload/
command_id 生命周期被新版缩窄为单一 stale code。其余条目分别记录 Public header 四态、论坛主页面信息披露、
Admin 窄屏表格卡片化与 dialog identity/busy/描述/清理差异。该矩阵先于第六组实现落盘；实施只保留后端权威
事实和新版安全不变量，不用扩展事实原地改写旧 v1 wire 或主页面 DOM。

矩阵冻结后已先完成 `API-014`/`AUI-010`：确认结果 wire 恢复 `recovery.id` 且严格拒绝未知字段；页面级
command map 只在成功后删除，四类 409 均关闭 dialog、清空 reason、保留 alert/command 并只刷新 recovery，
400/503 继续保留输入。recovery dialog 同时恢复描述关系。目标三 crate 原生 check/Clippy、Admin WASM、
rustfmt、i18n 和差异空白检查均通过。最新版 embedded 的受控边界探针确认未知请求字段严格返回
`400 invalid_json`，合法请求闭集指向不存在 recovery 时返回 `404 not_found`，两者均为 JSON/no-store；隔离库
当前没有 recovery case，故未伪造写入数据，真实成功/重放与四类冲突仍按矩阵保留 populated 库人工 R1。

第七组按 `PUI-019` 收回论坛同步主页面扩张：状态行恢复到 live 拍卖主体首项，loading、错误和未发布状态均
使用普通 `role=status`，published 只增加 `aria-live=polite` 与论坛外链；标题、正文、revision 和更新时间仍
保留在 API/存储/Worker 权威事实中，但不再进入旧主页面。Public forum DTO 与浏览器边界同时恢复旧字段闭集、
auction UUID/一致性、非负 revision、时间、状态闭集及 published URL/title/body 非空校验。五种语言同步替换
同一组状态消息，Public 每种资源现为 425 个键。Contracts/Public 原生 check、Public WASM、Clippy、rustfmt、
i18n 与差异空白门禁均通过；未编写或运行任何测试用例。main 五状态已补桌面局部 R1，390px 状态块自身尺寸
相同但新版受上游 shell 影响仍有 2px 纵向差异，isekai 继续保留人工 R1。

第八组继续收口 `PUI-014` 至 `PUI-021` 与 `I18N-003` 的最终制品边界。Public/Admin embedded payload
SHA-256 分别为 `b1a6e3be4c83d794dbe8bf7f62ffb740626f5375434c7bca50de814d68c89ac8` 与
`edfa5229edbb0ec01bad8f57cf08c9b9f35909bc362244a31f6d08b04b4b08be`。Public 共用 GET owner 改用原生
`web_sys::Request`，实际 Network 记录中的 current URL 精确为 `?world=main`、不再被 gloo-net 追加空
`&`；缺失/非法 world 保留原地址，用户 Main↔Isekai 的 push/Main 删除 query 与旧版一致。旧非法值会由
shell 请求 main、页面按 worlds JSON 首项再请求 isekai，双侧源码和插入顺序已证明这是旧重复 owner 缺陷，
新版共享 owner 的一致 main fallback 不回迁该缺陷。裸 `/login` 也已移除 Router 生成的空 `?`，而用户原始
`/login?` 与非空 query 保持不变。

共享 `legacy_t!` 同时恢复旧 i18next 动态文本：它只剥离 Fluent 自动加入的外层成对 FSI/PDI，Public 与
Admin 组件均已切换到该边界。最终 Public populated 根页逐项切换 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`，
五组 `hv.lang`、`html lang`、picker、H1 和权威 lot 行均正常；Admin 登录页五种语言的 `html lang` 与 H1
同样对应，两 surface 正文 FSI/PDI 计数均为 0。blacklisted 登录受控响应在最终 Public 上得到精确裸
`/login`、一次 GET/POST、主文案“该账号已被封禁。”和独立原因“封禁原因：公开原因”；合法 lot 省略
`stats/info_summary` 后根页仍显示权威行，匿名 realtime 徽章也从 checking 收口到 anonymous。论坛 main
五态证据继续有效。

同一问题随后按 `AUI-018` 在 Admin 侧收口：API 根的共用 `send_get` 覆盖 21 个 GET 调用点。修复前最终
浏览器对 `/auctions` 发出 `?page=1&page_size=10&`，旧版为无尾随字符的精确 URL；重建后新旧均为
`?page=1&page_size=10`。受控邮箱值 `mail box+一` 在两版也均逐字请求
`?mailbox_key=mail+box%2B%E4%B8%80`，证明空 QueryParams 不再污染已有 query，serializer 的空格、加号和
UTF-8 编码保持旧行为。取证后 Admin 恢复 `/login`、`hv.lang=zh-CN`，Public 恢复 `/`、`hv.lang=en`；
8090/8091 readiness 均继续可用。

本组通过 `cargo fmt --all -- --check`、i18n 策略、`git diff --check`、Public/Admin 原生 check、两套 WASM/
embedded release 构建及两个 Web crate 的 `-D warnings` Clippy；没有编写或运行单元、集成、功能、端到端或
浏览器测试用例。Admin 认证动态数据的值级投影、窄屏 populated 表格及 dialog 生命周期仍按
EVD-003、AUI-016/AUI-017 继续审计，不能由精确请求 URL、登录页和 Public 证据外推。

第九组先以受控只读响应继续冻结 `AUI-013`、`AUI-016`、`AUI-017` 与新增 `AUI-019`。Mail 的单条
failed 数据在 390px 下已把八列表头、八个单元格、三个筛选器、原始 `main/equipment/failed` token、
wrapper/table/row 几何及五种语言逐项对齐，最终 Admin payload SHA-256 为
`7f96e35e84a4271d8ae676eb43e52b780fda338265878acd4e9381c5e5dc59ee`。同样的探针随后证明此前把
Users/Renames 标成“已恢复”过早：Users 仍有本地化 status、主行展开完整历史、额外改名动作和 flex `td`；
Renames 仍有扩张说明/总数标题、表头、空值和 flex `td`。矩阵已先降级，实施不能沿用先前静态结论。

共享 Pager 的双仓库审计确认 9 个页面共 14 个调用点都受 DOM、invalid jump、`noValidate`、可见标签、
44px 控件及 1440/390 布局差异影响，因此独立登记为 `AUI-019/EVD-005`，不能只在 Mail 页局部覆写。
dialog 逐项复核则确认 recovery 的唯一旧 `aria-describedby` 和四类 409 已关闭；旧版其他 dialog 并没有
可泛化的 dialog-level 描述关系。仍待实施的是 opener/identity、Escape 传播、proxy busy 边界、shared
cleanup 核实和公共 overlay/panel 几何。本段只冻结差异和实施顺序，尚不把后续代码修改计为完成。

第十组按 `AUI-013/AUI-016/AUI-017/AUI-019` 实施 Users、Renames、共享 Pager 与 Admin dialog 公共边界。
Users 已恢复旧七列、原始 status、最新公开封禁理由、固定定位历史 tooltip、四动作及顺序；Renames 已恢复
旧六列、空回滚值、筛选器、动作及页级确认框；共享 Pager 已恢复旧 DOM、纯数字/范围校验、`noValidate`、
可见标签、44px 控件和窄屏两行布局。旧版部分非中文资源错误泄漏简体中文封禁表头/tooltip，新版按项目五语言
合同正确本地化该处，明确作为缺陷修复保留，不能为了像素相同回迁旧翻译错误。

13 个 Admin dialog 入口统一恢复旧公共 overlay/panel/actions 结构、标题/段落排版、Escape busy 边界和旧 ARIA
闭集；shared trap 按 DOM identity 处理替换、循环、清理和 opener。最终 payload SHA-256 为
`e317320d04bf2ec15dee24dc9e318b52db038830ac462dfcabe9e3179aa54e5a`。一次性受控浏览器探针在旧 8093/
新 8091、390×844 及全部五种语言下确认 Users/Renames 的弹窗尺寸、初始焦点、正反 Tab 循环、Escape、按钮
文案/顺序及关闭后焦点归位逐项相同；探针只作为本轮运行态证据保存在 `/tmp`，没有提交浏览器测试脚本。
格式、shared/admin/public WASM check、Admin/shared Clippy、i18n/policy、embedded release 构建均通过；没有
编写或运行单元、集成、功能、端到端或浏览器测试用例。在该批证据完成时，Users 创建→密钥替换、
成功/409/503 和其他 Admin 动态页面仍待后续矩阵，不能由本组代表框外推。

第十一组随后按 `AUI-013/AUI-017/AUI-020/EVD-004` 单独实施 Users 创建→一次性 Key。创建框恢复旧
`form.admin-users-create`、标题/label/input/help/actions 层级、字段 ID、`aria-describedby`、手工 Key
`minLength=32`、select 几何及按钮顺序；提交前 trim username/game player ID，手工 Key 在请求发出时立即
从浏览器状态清除。失败重试保持同一 `command_id` 且不保留明文 Key，成功后删除命令 ID；generated 模式
以同一 dialog identity 替换为一次性 Key 结果，关闭后恢复创建按钮焦点。实际旧 Users 接口使用
`temporarily_unavailable`，新版错误防腐层也已把该码归入本地化暂时不可用提示，不再落入通用错误。

一次性浏览器探针在旧 8093/新 8091、390×844 和 `en/zh-CN/zh-TW/ja/ko` 下分别覆盖 generated/manual
201 以及 manual 503→同 ID 重试成功：请求字段逐项 trim，manual Key 首次提交即清空，busy 期间只有两个
动作按钮 disabled，Escape 保持原框，重试成功后恢复 opener。简中/繁中的创建框几何逐项相同；其余语言
只因旧自定义 manifest 错把简中帮助泄漏到所有 locale 而出现文本高度差。旧成功提示还把 `{{name}}` 当字面量
显示；新版遵守五语言合同并插入真实用户名，这两项均作为旧前端缺陷修复保留。探针只保存在 `/tmp`，没有
提交测试脚本或测试用例；真实 409 与 1440px 仍留在 `AUI-020`。

第十二组由两个独立只读分支并行复核 Delivery/Settlement 与 Audit/Outbox，并回到双仓库完整调用链逐条
交叉确认。Delivery 新增的第九列、完整内部 ID、缺失 retry 二次确认、dialog busy/顺序，以及 storage→HTTP
把 invalid/not-found/stale/command conflict 全压成 503 的问题，分别冻结为 `AUI-021` 至 `AUI-024`；其中
typed error 是 `AUI-009/AUI-015` 真实 409 不可达的前置阻断。Settlement 另确认 loading/error 外壳、hold
百分比向下截断、金额右对齐、Recovery 成功刷新范围和全行 busy 差异，冻结为 `AUI-025` 至 `AUI-029` 与
`API-015`；`buyer_blacklist` 和完整五语言则作为旧浏览器闭集/翻译缺陷修复保留。

Audit/Outbox 复核把常驻筛选器、空表顺序、额外说明/total、Realtime select DOM value、Replay busy 字段及
Replay HTTP code 差异冻结为 `AUI-030` 至 `AUI-032`、`API-016/API-017`，运行态缺口统一归 `EVD-006`。
并行审计曾把 Public 的 48rem 卡片 CSS 误推到 Admin；主代理复核实际 Admin bundle 只加载 `admin.css`，且
该表格规则为 wrapper 横向滚动，因此明确撤销这个错误结论。第十二组只完成差异文档和实施顺序，不把尚未
修改的 Delivery/Settlement/Audit/Outbox 条目误记为完成。

第十三组先按 `AUI-021/AUI-022/AUI-024/AUI-026/AUI-027/AUI-029` 实施六个不依赖协议决策的
Delivery/Settlement/Recovery 差异。
Delivery 主表恢复旧八列、短 ID/full tooltip、`settlement→equipment`、空 failure/动作、数字左对齐和 table→empty
顺序；多余说明/total H2 已删除，合法 `prepared` 状态仍作为旧前端闭集缺陷修复保留。hold 比例不再先做
整数除法向下截断，而是在 `i128` 上等价恢复旧 `Math.round` 的正数四舍五入；负税额和非正 final price
失败关闭，金额单元格恢复继承旧表格左对齐。Recovery 成功改为推进 auction list、settlement report 与
recovery 共用 generation，409 仍只推进 recovery；全局 action busy 也已从页面传到每一行命令按钮，慢请求
期间不能替换正在提交的 dialog。Delivery retry 人工对账已恢复第一次 submit 零 mutation 的第二确认阶段；
两类框的按钮顺序、Cancel autofocus、busy 可编辑字段/静态按钮文案和动作分类反馈也按旧调用链恢复，同时
正确翻译旧 runtime 泄漏简中/遗漏 retried key 的文案。Delivery 窄屏筛选器与 Settlement 卡片已按旧 CSS 的固定宽度、gap、底距、
最小宽度、padding 和非拉伸布局恢复。格式、i18n、Admin WASM check 和目标 Clippy 通过；真实 populated 报告的
四类百分比、成功三类 GET、409 单类 GET 与焦点仍待一次性运行态证据，未编写或运行测试用例。

第十四组按 `AUI-009/AUI-015/AUI-023` 恢复 Delivery Admin 的跨层错误闭集。application 新增不携带
驱动正文的 `DeliveryAdminError`；PostgreSQL adapter 不再把参数无效、不存在、状态/fence 漂移、幂等摘要冲突、
坏投影/坏账本和数据库不可用统一压成字符串仓储错误，并额外校验重放结果的 command/delivery identity 与安全
快照。HTTP 防腐层现按旧版输出 400 `invalid_delivery_command`、404 `not_found`、409
`stale_delivery_state`/`command_conflict` 和 503 `temporarily_unavailable`，SQLite capability 501 不变；
非 Delivery route 若意外收到该 owner 错误仍失败关闭为各自通用不可用响应。rustfmt、Application、Storage
PostgreSQL、HTTP、Admin WASM check 及 Storage/HTTP Clippy 已通过；六类真实事务和 409/503 dialog 生命周期
仍待一次性运行态证据，没有新增或运行测试用例。

同一最终制品随后以内嵌 payload `d29e1521dacfa69282eb385730c00a03774279116c80f4f8d222d06e0e07c379`
重启在 `0.0.0.0:8091`；根路径、`/health` 与 `/health/ready` 均返回 200。一次性 DevTools 手工切换
`hv.lang` 后，登录页五种语言都保持合法存储值、同步 `<html lang>` 并显示各自译文；探针未保存为脚本或
测试用例。该证据只覆盖登录外壳，不外推认证后的 Delivery/Settlement 页面。

第十五组按 `AUI-030/AUI-031` 先实施 Audit 页面无需协议决策的主结构差异。标题和九个筛选器改为在
pending/error/success 三态常驻，loading/error 仅占筛选器后的结果区；独立错误 panel 与 Retry 已删除。成功
结构移除旧版没有的说明和 total H2，恢复 `admin-shell__table-wrap/admin-shell__table`，空结果仍先渲染八列表格
再显示 empty 提示。五语言枚举、result badge、安全 metadata fallback、格式空格和 `data-label` 暂不盲目回退，
继续留给 populated 五语言/两 viewport R1 决策。rustfmt、i18n、Admin WASM check、native Clippy 和差异空白
检查通过；WASM-target Clippy 另暴露四处既有 cfg-only `needless_return`，与本批无关且未越界修改。没有新增或
运行测试用例。

第十六组按 `DLY-001` 至 `DLY-003`、`API-015` 至 `API-018`、`AUI-031/AUI-032` 完成跨实例交付账本、
Audit/Realtime wire 与 Recovery 错误防腐层。Delivery Admin 现以 command advisory lock、行锁后二次账本读取和
冲突后新事务恢复保证同 ID 并发入口；新写摘要逐字采用旧 Go 字段顺序与 HTML-safe JSON 字符串规则。live row
和 ledger replay 共用完整状态/lease/时间轴 validator，重放还要求动作终态一致且 fencing version 相对请求已
前进。以上只收紧已存结果的可信边界，不改交付状态机或 settlement 原子联动。

Audit numeric/keyset 信封恢复旧 nullable/省略闭集，成功行移除扩张 badge、括号空格和 `data-label`；Realtime
恢复四个筛选 DOM token、请求映射、busy 可编辑字段与提交瞬间签名快照。Replay HTTP 独立映射旧
400/409/500/503 code，内部 typed error、命令账本和事务 fence 保留。Recovery 四层严格 decoder 与 HTTP
内部错误码映射现共同保证只输出冻结十一种公开语义，未知 owner code 安全失败为 503，不泄漏正文。

本组合并后通过 i18n 策略、Application/Contracts/PostgreSQL Storage/HTTP/Admin-WASM 的 locked check、上述
五个目标的 `-D warnings` Clippy、rustfmt 与 `git diff --check`。第一次总校验只暴露 realtime 模块漏导出三个
新 mapper，补齐装配后同一完整命令重跑为零错误。全过程没有编写或运行单元、集成、功能、端到端或浏览器
测试用例；交叉实例摘要、PostgreSQL 并发/坏账本、六类 Replay envelope、真实 Recovery 报告及认证页面动作
仍保留 R1，不能由静态门禁提升为完整等价。

随后重建 Admin WASM 与 PostgreSQL `web-embed` release，payload SHA-256 为
`02ae8c1827d3c3c785572c93c5247cc11d5ca470ad32fc0104bcfbb5f3806521`，并只重启 8091 Admin；8090 与
旧版 8092/8093 未触碰。重启后本机根/live/ready 和 `192.168.31.210:8091` 根页均为 200，服务端 manifest
逐字返回同一 payload。Chromium 强制忽略缓存后又依次写入五个合法 `hv.lang`，`en/zh-CN/zh-TW/ja/ko`
均保持存储值和 `<html lang>`，登录 H1 分别为对应语言；最后恢复 `zh-CN`。该一次性探针未保存为脚本，只证明
最新制品可访问及登录语言初始化，不外推认证后的 Audit/Outbox/Settlement 动态状态。

第十七组按 `DLY-004/DLY-005` 继续收口 Delivery Admin 跨版本事务与账本协议。命令事务现由独立 typed
wrapper 开启 fresh Serializable transaction，只对 SQLSTATE `40001/40P01` 最多执行三次完整尝试，并按
10ms/20ms 等待；业务 stale/conflict/坏账本和未知 commit 结果均不重试。新写 `result_payload` 不再保存精简
HTTP 投影，而是由 storage 防腐层生成旧 Go `delivery.Delivery` 的完整 23 字段 JSON；读取端先识别旧完整
对象，再兼容本轮前未发布版本可能留下的 `{command_id,snapshot}` envelope。创建命令、摘要、激活快照、
lease、状态字段和时间轴在转成安全 `DeliveryAdminSnapshot` 前统一失败关闭。

运行态对照使用从旧版运行库 dump/restore 的一次性 PostgreSQL clone `auction_dly_parity_20260828`，让未修改的
旧 Admin 监听 8094、最新新 Admin 监听 8095，并只插入四条合成交付。修正探针读取 `data.csrf_token` 后，
新写→旧重放和旧写→新重放均返回 200、`pending/fence=2`；第三份账本替换为过渡 envelope 后新版仍返回
200；第四份完整账本删除必填 `CommandID` 后新版返回 503 `temporarily_unavailable`。新版首次写出的 payload
恰有 23 个旧字段，`ID/CommandID/RequestDigest` 数组长度分别为 16/16/32，`CreatedAt` 为字符串且不存在
`command_id/snapshot` 过渡键。最终四条交付均为 `pending/fence=2`，账本与 audit 各四条、每个命令各一条，
证明上述重放没有追加第二份状态事实或审计。

该运行态 R1 完成了 `DLY-005` 四条目标路径，并为 `DLY-002` retry 双向互操作及 `DLY-003` 缺失身份字段的
失败关闭提供局部证据；它没有制造 `40001/40P01`，因此 `DLY-004` 仍需冲突 R1，也不外推 release/reconcile
摘要和其余坏快照组合。实现后已通过 rustfmt、repository/layout policy、PostgreSQL Storage locked check、
目标 `-D warnings` Clippy 与 `git diff --check`；全程没有编写或运行单元、集成、功能、端到端或浏览器测试
用例，也没有修改旧仓库。

临时双版本进程停止、零连接确认后，clone、dump 和专用配置均已按精确名称删除。随后仅把常驻 8091 Admin
重启到本组最新 release，二进制 SHA-256 为
`f04c629998d958740b9d3d1be690184fc1e31304d8298b1fda29337752f7282c`；进程监听 `0.0.0.0:8091`，本机
root/live/ready 与 `192.168.31.210:8091` 根页均为 200。8090 Public 和旧版 8092/8093 没有重启或改写。

第十八组先补齐 `DLY-004` 的真实 deadlock 边界，再由三路 `gpt-5.6-luna` 子代理分别审计 Delivery
命令协议、Admin Delivery WebUI 和 Public lot/代理表面；每路都先使用两仓库 CodeGraph，再逐调用链复核，
且没有修改旧仓库或新增/运行测试。第二个隔离 clone `auction_dly_actions_parity_20260828` 上，以外部行锁和
可释放 session advisory lock 构造命令事务必为 victim 的二进程锁环：连续两次 `40P01` 后第三次 HTTP 为
200，最终 delivery 只前进一次且 ledger/audit 各一份；连续三次 `40P01` 时 HTTP 为 503，delivery 保持原
`failed/fence=1` 且 ledger/audit 均为零。外部事务均正常提交，`pg_stat_database.deadlocks` 分别增加 2/3，
因此该证据覆盖三次尝试上限与失败回滚，但尚不能替代独立 `40001`。

本轮静态审计把此前未覆盖的 Delivery 请求体滚动互通、成功 message、数据库时钟、默认排序、严格浏览器
decoder、origin 扩展、stale alert 真实时序、空白字段、草稿/DOM/长文本差异冻结为 `API-019/API-020`、
`DLY-006/DLY-007` 与 `AUI-050` 至 `AUI-058`。其中 settlement fulfillment 原子联动和 stale 无残留 alert
分别作为当前数据模型不变量及旧异步 UI 缺陷修复保留，不能伪称旧实现完全相同；其余条目按优先级实施。
Public 审计又确认 API-011/API-012 只完成局部服务端 wire，浏览器严格 decoder、lot 持久化语义、nil UUID、
代理 authorization 错误码、业务错误闭集、preview category 和页面错误/空态分别登记为 `API-021` 至
`API-026` 与 `PUI-022`。这些新增行先于代码实施写入本矩阵，避免以已有正常样本反向缩小目标范围。

第十九组依照上述矩阵实施并主审收口 Delivery 与 Public 两条切片。Delivery 侧完成 `API-019/API-020`、
`DLY-006`、`AUI-050` 至 `AUI-059` 的对应修复：旧 release/retry 两字段请求、reconcile 空串兼容、三类成功
message、数据库事实时钟、降序分页、专属响应 decoder、登录后路由重绘，以及 populated 表格、二次确认、
stale/空白/草稿、占位行、列 class、长文本 portal 等 WebUI owner 均已落地。隔离 clone 上取得五动作双向
请求/重放、时间、排序、合法 decoder、409/400 生命周期、草稿局部生命周期、390/1440px 表格与键盘焦点
R1；`AUI-052` 新 origin 五语言、`AUI-055` 草稿全分支、坏响应全集、`DLY-007` settlement 联动终态及
独立 `40001` 仍按各自行保留，未由正常样本外推关闭。Admin 登录 P0 `AUI-059` 已由全新隔离 context 证明
成功后无需手工刷新即可进入 Delivery 并只发一次列表 GET。

Public 侧先完成 `API-021/API-022/API-025` 的严格浏览器 decoder、公开投影 validator 与 typed 业务错误链，
并同步修复 `API-023/API-024/API-026` 已登记的 nil path、authorization code 和 preview category 边界；其中
当前 live 的 NULL/旧起拍价哨兵、nil UUID 已取得局部 R1，其余坏响应、逐业务原因和 preview 三形状仍明确
待证。随后按 `PUI-023` 恢复 populated 根页旧 DOM、五语言独立键、truncated/倒计时/状态/动作格语义。
最终 embedded Public payload 为
`6580af51f826b09b3aa59fafe618c1a25d8e6570377badda54e1ea640d437c6d`：同一 One01 在 zh-CN
1440×1000 下 page/live/category/catalog/row 及十个单元格矩形、class、`data-label`、首子节点和可见文本均
逐项相同，行高同为 45.5px；390×844 下 `en/zh-CN/zh-TW/ja/ko` 的导航、表头、状态、CTA、倒计时形状、
`/login` 和 live 内相对几何全部无差异。新版对繁中/日文/韩文隐藏标题的完整翻译替代旧英文回退，作为旧
i18n 缺陷修复保留；两库 forum 状态事实不同造成的文案或整体纵移没有误归到目录组件。

实现后已通过 rustfmt、repository/layout/i18n/config policy、差异空白、Public/Admin 原生与 WASM 编译、
相关 PostgreSQL/SQLite feature 编译及统一 `-D warnings` Clippy；embedded Public/Admin payload 均重建，
只把常驻 8090 Public 切换到上述最新制品，旧版 8092/8093 与常驻 8091 Admin 未受本次 Public 重启影响。
临时 8094/8095 进程和三个隔离浏览器页已关闭，确认零连接后精确删除
`auction_dly_actions_parity_20260828`、对应 dump 与专用配置目录，端口和临时路径均已复核为空。全程未
新增或运行单元、集成、功能、端到端或浏览器测试用例，也未修改旧仓库。

第二十组按 `PUI-022` 收口 lot 错误与目录双空态，并在运行态反例中补登、实施 `PUI-024`。lot 查询现保留
安全 `status/code`，恢复旧 `ui-empty-state`、隐藏详情/Retry 和连接态 60 秒兜底条件；非法 UUID、真实 404、
注入 400/503/坏 200、全空和分类空均在五语言与 390/1440px 下逐项对照。繁中/日/韩的隐藏错误详情和目录
Pager aria 不再回退英文，作为五语言合同修复明确保留。对照抓包还发现当前目录仍请求 20 条且共用 Pager
结构没有真正沿用旧组件；最终改回 `page_size=10`、`ui-pager*`、`novalidate`、纯数字跳页与页面专属 label。
最终 embedded Public payload 为 `5c171828c3152589e8c36f56d658ccfecfbbfc3b38cf61cd8187effdb7ed6c47`，
常驻 8090 已切换该制品，旧 8092 与两个 Admin 进程保持原样。格式、i18n policy、Public 原生/WASM 编译、
`-D warnings` Clippy 与差异空白门禁均已通过；所有注入只存在于单次 DevTools 新文档，未写入仓库或保存为
脚本，也未新增或运行任何测试用例。

第二十一组先按 `BID-015/RT-006/WRK-007/SET-011` 收口竞价命令摘要与账本 kind、Public 实时 mechanism、
Recovery action Serializable 租约围栏，以及 action/父 case/审计的原子终态；随后在逐字复核旧 delivery 与
buyer blacklist 证明时新增并实施 `WRK-008/WRK-009`。旧 Go JSON 兼容入口现同时覆盖字段顺序、HTML 安全
转义和 SHA-256，blacklist 时间按 RFC3339Nano 去尾零；MoogleMail send 引用、take/reminder proof kind、
delivery snapshot digest、blacklist result digest 和 pending 释放错误码均恢复旧 wire。旧 reminder 一端要求
payment obligation ID、transport 另一端实际持久化 remote mail ID 或 delivery ID 的内部矛盾明确保留并登记，
没有借兼容实施静默改变旧行为。

随后逐字回溯确认旧账本固定使用大写 `OK` 和严格 `BidResult/ProxyResult` JSON，新版此前使用
`accepted + CompetitionPlan`；旧 `DeterministicID` 又是 SHA-256 拼接截断，不是通用 OID UUIDv5。应用层现以
独立防腐模块双读两种历史格式，新命令写旧 wire，并对普通/代理事实、授权、配额、转换闭集和确定性 ID 做
严格校验；代理完整转换历史由两后端在同一竞价事务读取。既有新版 `accepted` 行仍可只读重放。

共享 PostgreSQL 首轮探针确认旧固定 `InsertCompetitionBid` 不含新版 NOT NULL
`bids.effective_end_at`，而旧命令已经先把同一事实的结束时间投影到 lot。追加迁移 `000019` 因而只在
INSERT 遗漏值、且价格/领先者/状态版本逐项匹配已锁 lot 时复制该时间；否则以 `23514` 失败关闭。该对象已通过
PostgreSQL 18.6 直接隔离验证和项目自身 `auctionctl migrate`/重复执行，旧首笔竞价随后成功，登记为
`BID-016` 的迁移与共享库 R1。

普通竞价之后完成旧写→新重放、新写→旧重放、合法冲突和零重复副作用。严格响应对照同时发现 Go
RFC3339Nano 的 offset/去尾零及代理 transition 纳秒会被新版 `Micros + Z` 投影改变，作为 `API-035`
独立保留。代理双向探针又暴露旧降低账本的 nil `Facts` 固定为 JSON `null`，新版严格 `Vec` 最初返回 503；
防腐层现只对该必有字段接受并输出旧 `null`，仍严格拒绝缺字段、未知字段和损坏事实。修复后的两个独立克隆
均完整通过代理创建/降低首次、跨版本重放、合法冲突、账本 JSON 形状及 bid/转换/realtime/audit 计数，
`BID-015` 因而取得双向共享库 R1。

本组已通过 Application/Integrations/Jobs、PostgreSQL 与 SQLite Storage 定向编译，相关 `-D warnings` Clippy、
Public WASM 编译、rustfmt、差异空白门禁及 embedded release 构建；真实竞价帧、租约冲突、三终态事务、
固定 proof snapshot 与 pending 释放仍按各自矩阵项保持待证。旧仓库未修改，也未新增或运行任何单元、集成、
功能、端到端或浏览器测试用例；所有共享库探针均为未落盘的一次性命令，临时数据库、角色、配置和监听已清理。

2026-08-28 又以三路 `gpt-5.6-luna` 子审计分别复核竞价锁/结算入口、Public mutation 错误边界和实时协议。
竞价 `BID-004/005/006/013` 未发现新的静态语义差异，仍只缺对应运行态证据；Public 侧确认并扩充
`API-032/033/034`，尤其把 session/commands/repository/ledger 错误分类、deadline 丢失和代理提交后再次读取
投影的失败窗口纳入同一防腐边界；实时侧确认正常六字段 bid 帧基本等价，但 `RT-001/003` 的 merge/refetch、
gap/cursor 与严格 decoder 仍不同，`RT-006` 的服务端 mechanism 闭集也尚未恢复。上述差异均先写入矩阵，
尚未用正常 happy path 的一致性掩盖异常路径缺口。

随后在一次性 PostgreSQL v19 克隆中以真实 Worker 取得 Recovery 增量 R1：satisfied、failed、
manual_reconcile 三种 action 都与父 case 投影和唯一终态审计同事务提交，重复轮询零增量；受控拒绝审计 INSERT
时，终态 action、proof/completed 和父 case 全部回滚，只留下 Worker 后续独立 release/backoff。pending delivery
固定释放为 `recovery_evidence_not_observed`，并精确表现为 state 1→2→3、fencing 1、retry 1、claim 清空和
next attempt 后移。另一路在真实 owner 持有原 claim 时把权威记录重领为更高 state/fencing/token，释放阻塞后旧
owner 只能得到“终态 claim 已失效”，没有改写 action、父 case 或 Recovery 审计。由此关闭 `SET-011` 与
`WRK-009` 的对应 R1，并把 `WRK-007` 收窄为仅缺“刚过期未替换”和强制 `40001/40P01` 重试；证明摘要字节仍
按 `WRK-008` 独立保持待证。该批仍未新增或运行任何测试用例。

同日继续按 `API-032/033/034` 实施 Public mutation 防腐边界。组合 HTTP 探针首先证明认证、query、path、
Content-Type 与 JSON 分类已经恢复旧顺序，同时暴露新版把所有超过 4 KiB 的 body 统一预判为 413，而旧 Go
`MaxBytesReader + json.Decoder` 会让限额前已经确定的语法错误先返回 400。严格 JSON owner 现只保留不超过
上限的前缀并额外观察一个字节，对首个值的语法、完整 DTO、第二个值和尾随内容按旧可观察顺序分类；复测大
malformed 前缀为双方 400、大合法已知字段为双方 413。Origin allowlist 在当前配置中关闭，启用态和真正耗尽
rate limit 因而继续留在矩阵，未用关闭态的共同 401 冒充完整证据。

随后三条命令共完成 35 组缺失/null、错误 scalar、规范与非规范 UUID、未知字段、双 JSON 值及金额边界输入，
新旧 status/code/message 全部一致。成功路径先确认当前成对 live lot 没有 bid 或代理授权，再以逐字段前置条件
把两侧对应起始价投影从 1100 规范到 50000；代理创建 100000、降低 90000 的首次与同 ID 重放均为 200，授权、
quota、完整转换历史和可选 bid 响应逐字段一致。数据库最终均为两条 `OK` 命令账本、一条代理 bid、一条 active
授权和两条转换，重放没有重复副作用；这同时为 typed application outcome 和移除提交后账户 projection 读取
窗口提供了运行态证据。受控 session/storage/坏账本及 deadline/timeout 仍按 `API-034` 独立待证。

最新版变更已通过 rustfmt、仓库/布局/配置/i18n 策略、workspace/all-targets、PostgreSQL/SQLite/Redis 独立
feature、全 workspace `-D warnings` Clippy、Public/Admin WASM 编译和差异空白门禁。本组未编写或运行任何
单元、集成、功能、端到端或浏览器测试用例；HTTP/SQL 对照均为未落盘的一次性探针。
最新版 embedded Public 已重新构建并在 8090 运行；`/health/live` 与根路径正常，`/health/ready` 的 503 来自
保留用于对照的 SQLx v17 数据库低于当前迁移树，不代表进程或 WebUI 无法访问。

同日继续实施 `RT-006`。旧 `bidAcceptedCodec` 在 public projection 前对 `mechanism` 同时执行字符串类型和
`ordinary/proxy` 闭集校验；新版应用边界此前只校验字段存在。最初修复位于
`PublicRealtimeEvent::validate`；后续 `RT-001/003` 收口又把同一防火墙扩展到全部已注册公开 payload 的
精确字段、类型、范围、UUID、world、时间、状态和枚举校验。因此 PostgreSQL/SQLite 回放、HTTP、
Hub/WebSocket 与 relay 共用服务端第一道防线，Dioxus 严格 decoder 是第二道防线。正式成对 live stream 的 proxy 事件在双方 catch-up 中均为
200，金额、状态版本和事实索引一致；不同的 stream sequence 来自两侧既有事件历史，未归入本项强行抹平。

为验证损坏持久事件，分别从当前新旧数据库建立临时克隆，只在克隆中把同一 proxy 事件的机制先改为未知
字符串、再改为 JSON 数字。两版两轮都返回 `refetch_required=true`、`next_after=3`，公开事件数组均未出现
畸形机制；这证明新服务端已恢复旧失败关闭，而不是仅靠浏览器拒绝。该轮 8094/8095 临时进程、两份数据库
克隆和含敏感连接配置的临时目录随后精确删除且不可恢复；正式数据库没有写入畸形事实。

随后为真实 WebSocket 另建一对 disposable PostgreSQL 克隆和同条件 challenger session，在双方同一拍卖的
对应 live lot 上提交同一个普通竞价命令 60000。已有 active fast proxy 在两边都立即反击到 63000；两条服务端
文本帧严格按 `ordinary/fact 1/state v4`、`proxy/fact 2/state v5` 到达，schema v2、auction、确定性 bid ID、
金额和 mechanism 一致。两侧 HTTP 首次与同 ID 重放均为 200；重放期间新建连接观察 10 秒均无文本帧，数据库
各保持 1 条 command、2 条 bids、2 条 realtime，lot 为 63000/v5，active proxy 仍为 90000/usage 1。event ID、
已有 stream 基线及 offset/`Z` 时间词法按各自矩阵项保留，不在 RT-006 中伪归一化。8094-8097 四个临时进程、
两份克隆和含生成登录 key 的目录均已精确删除且不可恢复，常驻 8090-8093 未动。

并行只读审计同时完成 `API-035` 的全 Public 时间表面盘点，随后按冻结方案完成实现。兼容边界现同时保存
legacy wire 与 UTC instant：旧命令账本的 offset/纳秒只用于逐字重放，领域校验、相等和排序仍比较 instant；
数据库与新生成值统一按 Go-compatible RFC3339Nano 裁零输出，不把该表示扩散到领域或伪造 PostgreSQL 已丢失的
纳秒。普通 bid、代理 transition、账户、目录、lot、历史、补偿、forum、realtime envelope 和新写 payload 已回到
同一 Public formatter，Admin 固定微秒 formatter 保持不变。

运行态先确认最新版 8094 对自身 create/lower 账本保留既有纳秒。随后在可恢复的同库探针中，把 create 的整秒
effective end、纳秒 transition 以及 lower 的微秒/纳秒 transition 改写为等价 `+02:00` wire；四项均 200 且响应
逐字保留，账本两行按修改前 MD5 精确恢复。当前拍卖关系时间另覆盖整秒、需裁尾微秒和完整微秒三向量，输出依次为
`...44Z`、`...44.1234Z`、`...44.123456Z`，原始 `scheduled_open_at` 精确恢复。尝试让最新版直接读取未迁移旧克隆时，
因旧库缺 `lots.leader_proxy_last` 在 lot 投影阶段先 503，未触达账本，故明确判为无效样本且未计入 R1。全过程没有
编写或运行测试用例。

`API-034` 随后以同步 lot 行锁证明双方标准边界均在约 15 秒返回 504，依赖表/账本 owner 暂时失效均返回 503，
恢复权限后同进程立即恢复 200，全部业务事实保持基线。`API-037` 的未知非 `OK` 结果已从新版 503 恢复为旧 409；
空 payload 仍同旧为 503，kind/aggregate 漂移继续由新版 409 失败关闭并登记为安全强化。`API-036` 三条 mutation 的
非空 query 已完成六请求 R1，三个代理领域状态 fixture 仍按矩阵保留，不能用源码词法冒充运行态覆盖。

本批清理前再次确认新旧均为 2 条目标命令、0 条 `f300*` 探针残留、lot 为 50000/v3、授权为 active
90000/v2 且两条转换。随后停止 8094/8095，精确删除两份 `auction_api037_*` 克隆、API-034/035 敏感配置与诊断
临时文件，均不可恢复；8094-8096 已释放，常驻 8090-8093 根路径仍全部为 200，9222 浏览器进程未动。本批没有
编写或运行任何测试用例。

第二十二组按 `RT-001/RT-003/PUI-020` 收口 Public 实时异常路径的静态差异。双仓 CodeGraph 与独立
`gpt-5.6-luna` 只读复核纠正了两个关键细节：完全相同的重复事件/事实应当 no-op，冲突事实才进入恢复；
服务端缺口页可继续携带后续合法事件并推进原始高水位，但浏览器在 `refetch_required` 时不合并该页事件。
新版据此增加严格 raw JSON page/event/payload decoder、有界 event/fact cache、state/fact fence、握手后
catch-up、3 秒断线重连，以及 refetch 后保留 `max(old_after,next_after)` 和现有 socket 的快照恢复。discovery
与单拍卖 owner 继续隔离，核心提示恢复 Connecting/Degraded/Live/Manual；旧 readiness 外层的 idle/unavailable
仍明确保留为 WebUI 残差，未用四态编译成功冒充六态完整等价。

最终逐段复核又纠正两个边界：服务端零时间只对应 Go `time.Time{}` 的
`0001-01-01T00:00:00Z`，合法 Unix epoch 不得被拒绝；单拍卖 WebSocket target key 同时包含 auction ID
与 world，并对带 world 的文本帧核对当前页面 world，HTTP catch-up 继续保持旧版只冻结 auction ID 的页合同。

服务端同时让 PostgreSQL/SQLite 在缺口或坏投影后继续扫描本次有界原始页，以原始行推进
`next_after`、隐藏坏事件并置 `refetch_required`；应用公开边界则按注册表逐 kind/version 校验精确 payload，
不再只保护 bid mechanism。Public 五种 FTL 各增加 degraded/live/manual 三项译文，当时每种资源均为
461 个消息 ID 且键集合一致。定向 Application/Public/Storage 检查通过后，又完成 rustfmt、仓库/布局/
配置/i18n 策略、workspace/all-targets、PostgreSQL/SQLite/Redis 独立 feature、全 workspace
`-D warnings` Clippy、Public/Admin WASM 编译和差异空白门禁；该证据只达到静态修复，畸形页、缺口页、
冲突帧、状态徽章、目标切换和断线重连仍按矩阵补 R1。全过程没有编写或运行任何测试用例，旧仓库与
常驻 8090-8093/9222 进程均未修改或停止。

第二十三组在实施前再次以三路 `gpt-5.6-luna` 并行审计 `RT-002/PUI-014/PUI-020`，并由主线程交叉
复核双仓调用链。`RT-002` 已从笼统“连接策略不同”细化为启用 allowlist 时恰好一个可信 Origin、
origin→query→auth 失败顺序、4 KiB frame/message 上限、50/60 秒 Ping/Pong、5 秒 write timeout、四类
close code/reason、底层读写失败直断、discovery 专属 128 槽位及 429 `discovery_busy`、settled 新连拒绝/
既有连接可收终态事件，以及独立 realtime limiter；严格 query 与连接尝试限流作为安全强化保留，不再与
普通 mutation 配额耦合。`PUI-014` 又确认
精确 query 顺序、账户表头/说明、专属 CSS 和双视口间距残差。`PUI-020` 冻结为“核心四态 + 外层六态”
边界，并逐分支恢复 readiness、idle/unavailable 和 session error→manual 的旧语义。本段只记录已完成的
差异冻结，尚未把后续代码实施或静态门禁计为完成；三路审计均未修改旧仓库，也未新增或运行测试。

随后按 `PUI-020` 实施冻结方案：Public 新增一次性 readiness JSON 读取，以独立外层六态按旧优先级组合
readiness、共享 session/current 和核心四态；session error 不再误报 degraded，无 current 的 discovery
live/degraded 分别投影为 idle/connecting，503、网络错误或坏 readiness 响应投影为 unavailable。核心实时
枚举、lot 状态播报、单连接 owner、3 秒重连和 refetch 游标均未扩张；五种 FTL 各补 idle/unavailable，
当前每种 Public 资源为 463 个消息 ID，并恢复 unavailable danger 圆点。rustfmt、四项策略、
workspace/all-targets、PostgreSQL/SQLite/Redis feature、全 workspace `-D warnings` Clippy、Public/Admin
WASM 与差异空白门禁全部通过；本项仍只达到静态修复，未新增或运行任何测试用例，浏览器状态时序 R1
继续按矩阵保留。

随后按 `RT-002` 实施服务端 v1 连接边界：`AuctionReader` 与 PostgreSQL/SQLite 增加只返回启用世界公开
状态闭集的窄预检，HTTP 不再用事件页猜测 settled；Public state 把原全局信号量收窄为 discovery 专属
128 槽，并持有可广播 drain 的 watch。WebSocket 连接尝试使用与 mutation 相同配置数值但独立账本，
通过后恢复可信 Origin→严格 query→session 顺序；Axum frame/message 固定 4 KiB，50/60/5 秒
Ping/Pong/write、四类 code/reason、底层失败直断和进程 1001 均按旧版落地。rustfmt、四项策略、
workspace/all-targets、PostgreSQL/SQLite/Redis feature、全 workspace `-D warnings` Clippy 与 Public/Admin
WASM 均通过；未新增或运行测试，也未重启常驻 8090，因此真实帧和容量 R1 仍保留。

随后按 `PUI-014` 实施认证账户页差异：Public 账户 GET 恢复旧版
`bid_page,page_size,proxy_page` 参数顺序，认证成功页删除新增说明和总数，Bid/Proxy 恢复四列/六列、旧版
标题/空态/保密提示与各自 section 内分页。账户表不再借用 catalog owner，`public-account-table*`、领先行、
状态徽章、section 底部间距及 48rem 卡片折叠均按旧 CSS 独立落地；代理本人上限、授权历史、模式、次数和
状态继续显示，富 DTO/严格 decoder 不变。五种 FTL 各为 466 个消息 ID，rustfmt、四项策略、
workspace/all-targets、PostgreSQL/SQLite/Redis feature、全 workspace `-D warnings` Clippy 与 Public/Admin
WASM 均通过；未新增或运行测试，也未重启常驻 Public，因此 populated 多页与双视口 R1 仍按矩阵保留。

2026-08-28 在不触碰旧仓库和数据库的前提下，为 `PUI-014/RT-002` 完成最新 Public 制品切换与局部运行态
收口。Public WASM embedded payload SHA-256 为
`f708cdd6a821bf0855270a50ece6eb5f87dd4a362b07317280846f113fd6c673`，release 可执行文件 SHA-256 为
`ede24625c414cca6f82ef0b3dadb233abe240c64421e0b028acca08500207557`。候选先在 18090 金丝雀确认根页、live、
全部 HTML 引用资源和 WebSocket 失败边界，再安全接管 8090；接管后 `/proc` 映像哈希与候选一致，根页/live/
静态资源均为 200。ready 继续为既有数据库迁移状态导致的 503，未据此误报代码回归；上一运行映像已保留为
可恢复回退件，金丝雀随后关闭。

同一隔离 Chromium 上，旧 8092 与新 8090 使用完全相同的内存只读 session、21 条 bid 和 21 条 proxy
快照，完成三页独立分页、1440×1000/390×844、长授权历史、无横向溢出及五语言逐项对照。初次比对发现
简中/繁中/日文/韩文的代理模式、状态、转换或额度词法仍偏离旧 manifest；四份 FTL 按旧运行态资源修正并
重建后，五语言 main 全文和结构化字段全部相同。该探针只存在于隔离浏览器文档内，没有保存为测试脚本，
没有使用真实凭据或写入数据库。`RT-002` 仅新增旧新 400 `invalid_query`、401 `unauthenticated` 的同序义
握手失败证据；已认证 101、事件/Ping/Pong/close、4 KiB、128+1 和 settled preflight 仍明确保留。

同日下一轮由三路 `gpt-5.6-luna` 分别审计 realtime、`PUI-020` 与非 UI P0/P1 残差，主线程再以
CodeGraph、SQL 和当前进程/数据库状态交叉复核。结论重新打开两项此前过早视为“只缺 R1”的静态差异：
`SET-009` 的旧 complete 核验代码没有任何可达生产入口；`RT-002` 的额外连接尝试限流改变失败优先级，
单拍卖预检也只查 status 而未复用旧完整 Public projection。`PUI-020` 六态、DOM 顺序、圆点、刷新条件和
五语言静态一致；390px 的 46px 偏移已证明来自旧 ready/new not-ready 的不同运行输入，不是 `/me` CSS。
新版 session 首次失败后额外 300 ms 恢复一次的请求时序残差同时登记；该审计时点尚未据此修改代码。

新版 Public/Admin 共用的隔离 PostgreSQL 数据库已确认是 `auction_rust_runtime_20260827`，当前
`_sqlx_migrations` 为成功的 17/17，而工作区二进制要求 19/19；`000018` 的 live/sold lot
`effective_end_at` 预检得到非法行数 0，`000019` 的旧 BF bid 兼容 trigger/function 均尚不存在。因此两端
`/health/ready` 的 503 已定位为运行库迁移滞后，而不是监听失败或 WebUI 代码回归。迁移前已生成 owner-only
PostgreSQL custom-format 备份
`/tmp/auction-runtime-pre-v19.UCCuYW/auction_rust_runtime_20260827-pre-v19.dump`，`pg_restore --list`
校验通过，SHA-256 为
`ba9cfeb769f4d161ad2695ec83693fb610bc11f46ba5a34eb0cdf8067f4d06f2`。该预检时点尚未执行迁移；后续只在
停止新版 8090/8091、确认无新版 Worker 且旧 8092/8093 与 Chromium 保持运行后，使用独立 maintenance 配置
执行 forward-only 17→19，再重启并验证 ready 与同输入 WebUI。全过程未编写或运行测试用例。

随后按上述冻结差异完成三处最小实现。Public session reader 删除首次失败后的 300 ms 隐式第二次请求；
PostgreSQL/SQLite realtime 预检从 status-only 改为读取并校验完整核心 `CurrentAuctionSnapshot`，覆盖
ID/world/sequence、时间、revision、展示模式、竞价开关和状态组合，settled 仍拒绝新连。旧 projection 对
preview 还要求同一 world 不存在 live/final/settling 拍卖；该 SQL 条件在本轮后续审计才被发现。额外 per-IP
连接尝试限流明确作为失败优先级不同的安全强化保留。结算侧新增 `LegacySettlementVerificationPort` 与
`auctionctl verify-legacy-settlement --auction-id`；PostgreSQL 在首语句为
`SERIALIZABLE READ ONLY DEFERRABLE` 的单一快照中读取旧 complete ledger、冻结 workflow/due/claim 身份和
权威结算事实，只输出安全摘要，SQLite Lite 明确拒绝该能力。Application、Storage、auctionctl 的 PostgreSQL/
SQLite 定向 check、all-features Clippy、Public WASM、Public API embedded check、rustfmt、仓库策略和差异空白
均通过。

运行态收口先重建两端最新 Web 与 release 制品：Public payload/进程 SHA-256 分别为
`5f4214d659b7292b15498f26b3bec0b81f7d0420d33d0947c8834ee0db83c743` /
`e93163e86a9e536066bdc83e4f3b19d70126c18d8ab5696ece9ca5bb5344652c`，Admin 分别为
`01876a44473c871335cee59b5b5fc2f361f1e3fecb0771aa4b8a22cde25063ac` /
`422d35ce7b3cd255bacffb897cfbf8fd12e4e4b4b86347528f12242f1ae42fa5`，当前 auctionctl 为
`bb4204bdb7c8b8bb0860fe40525dea191eb0fd1b41d03c914c408e31bb0f07fc`。Public 18090 金丝雀先确认根页、
全部引用资源和 live 为 200，ready 仍按迁移前输入返回 503，随后关闭。停机窗口只停止新版 8090/8091；
确认两端口释放、目标库额外连接 0、Worker 不存在且旧 8092/8093 仍为 200 后，由 owner-only
`/tmp/auction-migrate-v19.ySPoUp` 控制目录执行官方 forward-only migrate。结果为成功 19/19、失败迁移 0，
`lots_effective_end_shape_check` 已验证，旧 BF bid trigger 已启用，函数保持 `SECURITY INVOKER` 和
`search_path=pg_catalog`。重启后 8090 至 8093 的本机 root/live/ready 全部为 200，8090/8091 LAN 根页也为
200；`/proc` 运行映像哈希与上述候选逐字一致，旧版与 Chromium 全程未停止。

迁移后的旧 8092/新 8090 `/me` 使用相同 English、ready、anonymous 输入，均呈现 manual 和刷新按钮。
1440×1000 的 header/main/heading top 同为 `0/104/128px`；390×844 的 header/main top 同为 324px、账户块
同为 `342×215.5px` 且无横向溢出。移动截图逐字节相同，SHA-256 均为
`e0527bd8e57c4164659efd19393725c232fca9a48e2e6bc8fc2cd011eb8e1b07`；两侧 Network 各只有一次
`/api/v1/session`。新版匿名 CTA 继续用已登记的 `return_to` query 映射旧 Router state，额外
`aria-labelledby` 不改变画面且避免复制旧版已有的字段 id/name 浏览器 issue。最后对一条没有旧 complete
账本的 settled 行执行新核验入口，按 operation exit 5 因 workflow 缺失失败关闭；命令、结算、lot、义务、
审计和 realtime 六类计数前后同为 `54/0/0/1/71/45`。真实旧 complete 样本仍缺，不能据此提升 SET-009
完整接管等级。全过程没有新增或运行任何单元、集成、功能、端到端或浏览器测试用例。

同日继续对 `RT-002` 做已认证 WebSocket 受控对照。两侧均为同一原始 32 字节 URL-safe Base64
token、解码字节 SHA-256 摘要和 `auction_public_session` Cookie；探针只在两套运行库各插入一条对应
`parity-contract-user` 的短时会话，并以摘要和精确 user ID 删除，结束后两库匹配残留均为 0。旧 8092/
新 8090 的 discovery 握手均为 101，客户端 Ping 均收到相同 payload 的 Pong，1 字节和恰好 4096 字节
Text 均以 1008 关闭。该对照首次暴露 `RT-002` 的可达 wire 差异：4097 字节 Text 在旧版发送 1009
close frame，新版 Axum/Tungstenite 上限当前只让读取返回错误并直接结束 TCP，客户端观察不到 1009。
因此本项重新标为待修复，先在 WebSocket owner 显式恢复读超限 1009，再重建候选并复测；服务端 50 秒
Ping、128+1 discovery 容量、同一 world preview gate 和损坏快照证据仍单独保留。该一次性探针未保存
为测试脚本，也没有运行仓库测试用例。

同轮三路 Luna 与主线程 CodeGraph 交叉复核又冻结四项实现目标：RT-002 的 `MessageTooLong`→1009 和
同一 world preview gate；WRK-010 的论坛回复扫描状态闭集；WRK-011 的通知 owner 未装配时账本时序。
旧 projection 的 ForumSync/FinalRestriction 经完整调用链确认不参与 WebSocket 预检，已从缺口清单撤销。
本矩阵先于代码更新，随后仅在对应 owner 内实施最小修复，不扩写或运行测试用例。

候选 Public 随后已用原运行库短时会话证明 4097 字节 Text 返回空 reason 1009。为验证 preview gate，又从
原运行库建立一次性 PostgreSQL clone，只在 clone 中构造同 world 的 live 与 preview；相同 Cookie 的 session
读取为 200，但两个单拍卖握手都返回 503。直接执行 `realtime_status` SQL 确认两后端查询都引用
`l.effective_end_at` 而没有连接 `lots AS l`；PostgreSQL 原查询明确报缺失 FROM 条目，手工补左连接后 preview
返回 0 行。该缺陷会阻断全部单拍卖 WebSocket，矩阵因此先登记第三项 RT-002 修复目标，再修改两套查询。

两后端补齐 `LEFT JOIN lots AS l` 并重建后，同一 clone 上 preview 握手为 404、live 为 101；discovery 为
101，masked 4097 字节 Text 返回空 reason 1009。一次性 clone、会话材料、复制的敏感配置和 18090 监听均在
零连接后精确清理，数据库/路径/监听残留为 0。release 可执行文件 SHA-256 为
`850f77efbb45c3c08c33931280ce35f4e69f675b86c27e0ac945785abe7731c6`，已接管常驻 8090；`/proc` 映像哈希
逐字一致，本机 root/live/ready、LAN 根页和全部 HTML 引用资源均为 200，8091 至 8093 全程保持在线。

同批还完成 `WRK-010/WRK-011` 的最小静态修复：论坛 topic/lot 查询恢复旧状态闭集，notifier 未装配检查移到
命令 ID 与 ledger 之前。PostgreSQL/SQLite Storage、Jobs 的定向 check/Clippy，以及 rustfmt、仓库策略和差异
空白门禁均通过；未启动 Worker 或外部通知，因此两项仍保持运行态 R1 缺口。本批没有编写或运行任何测试用例。

随后按矩阵先行的 `AUI-060/API-038/API-039/API-040/AUI-093` 收口 Admin 浏览器边界。错误 envelope 恢复
旧版“必需三键、允许扩展、任意字符串 code”；拍卖、lot 与 users 的成功 data 改由专用 raw DTO 和集中
validator 逐层验证闭集、规范 UUID、JS safe integer、浏览器时间及状态/归属/nullable 组合。session 只把
合法 `401 unauthenticated` 视为匿名，成功与 logout 分别验证完整认证事实和 `data:null`。后续逐分支和浏览器
冻结取证纠正了 loading DOM 的早期误读：旧 pending 是顶层 `div[role=status]`，不是 main/card；error 仍保留
新版显式 main/card/alert、重试和协议错误绝不进入认证 Outlet 的 fail-closed 加固。

Admin GET 与全部 mutation 现只经唯一 10 秒请求 owner 发送；WASM 超时 abort fetch 并返回
`request_timeout`，普通发送失败仍为 `network_error`。API 目录回扫只剩 owner 内部两处 `.send()`；原生与
`wasm32-unknown-unknown` locked check、目标 crate Clippy `-D warnings`、rustfmt、仓库策略和差异空白门禁
均通过。坏响应、401/logout、真实 10 秒挂起及 error/登录壳双视口仍待一次性受控 R1；pending 的结构差异已
由冻结 fetch 的一次性新旧 DOM 对照确认。本批没有编写或运行任何单元、集成、功能、端到端或浏览器测试用例。

最新 Admin WASM payload `2060ab83596b418c018dd31cb5233f3895e65a2ced1d49c4968fc80c592cc072` 已重建并
内嵌到 PostgreSQL release 制品；二进制 SHA-256 为
`eacb9d4c1277b948111720e7ae4e907339c1819f1514f87ce25050b842e41506`。常驻 8091 在保存旧映像回滚副本后
完成有界排空和接管，`/proc` 映像哈希与候选一致；8090 至 8093 根路径、8091 live/ready、8090/8091 LAN 根页及
HTML 引用的 favicon/CSS/JS 均返回 200。该访问证据只证明新制品已生效，不替代上述坏响应、超时或双视口 R1。

2026-08-29 继续使用五路 `gpt-5.6-luna` 只读并行审计 Public、Admin、核心竞价/生命周期、Worker/Settlement
和 Config/Ops，主线程再用双仓 CodeGraph、SQL/组件源码与当前运行态逐项交叉复核。20 条 Public HTTP、37 条
Admin HTTP 入口已重新盘点；本轮新增并冻结 `BID-017/LIF-002/BID-018/API-041/API-042/API-043/PUI-028/
AUI-094/AUI-095/OPS-007` 十项真实缺口。最高优先级是生命周期与竞价的反向锁序；随后是 slow tick 在 Final
竞态中继续写入、两套浏览器剩余 decoder/timeout、WorldPicker 首屏 URL 时序、Rename 过期版本、Users 可见
状态词法和 v19 retention policy。语言闭集仍精确为 `en/zh-CN/zh-TW/ja/ko`，框架可承载更多 locale 不等于
已经支持更多翻译。

主线程同时撤销三项子审计误报：旧 Public 的 zh-TW/ja/ko 资源本身就存在英文 fallback；Recovery 通用 proof
validator 已校验 kind/UUID；MoogleMail 成功但 `proven=false` 会立即进入 manual reconcile，不是静默完成。
Admin pending DOM 的旧证据也已按旧源码和一次性冻结 fetch 浏览器取证纠正。第一实施批选定
`BID-017/LIF-002/BID-018/API-042/PUI-028/AUI-093/OPS-007`；`API-041/API-043/AUI-094/AUI-095` 保留为后续
独立窄批，避免在一个改动里混合大面积 decoder 与状态命令。审计和取证均未修改旧仓库，也未新增或运行任何
单元、集成、功能、端到端或浏览器测试用例。

第一实施批随后按矩阵完成 `BID-017/LIF-002/BID-018/API-042/PUI-028/AUI-093/OPS-007`。PostgreSQL
生命周期 context 以 materialized CTE 固定 world → optional scheduled round → auction 锁序并返回锁内
`world_enabled`；Preview/Open 恢复旧 enabled gate，Cancel 保持旧语义。slow proxy 在执行锁内快照新增 auction
`Live` gate。Public 三条竞价 mutation 统一进入已有 10 秒发送 owner，WorldPicker 只在 worlds 成功后规范化 URL；
Admin pending 恢复旧顶层 `div[role=status]`，同时保留新版 session error 的 fail-closed 安全外壳。retention policy
同步到 PostgreSQL migration 19。

静态证据包括 Application、Jobs、Public/Admin 原生和 WASM、Storage PostgreSQL/SQLite、auctionctl 的定向
check/Clippy，以及全工作区 `cargo check --locked --workspace --all-targets` 和 all-features `-D warnings` Clippy；
rustfmt、diff whitespace、repository/layout/config/i18n 门禁也全部通过。真实 PostgreSQL v19 数据上的只读
`EXPLAIN (COSTS OFF)` 显示 world、round、auction 三层 `LockRows` 顺序；一次性只读 retention inventory 返回
`relations=62`、`policy_tables=61`、`migration_current=true`，policy/expected version 均为 19，临时目录随后删除。
这些证据不替代锁竞争、slow scan→Final、请求超时、WorldPicker 三态和 WebUI 双视口的后续 R1。本批仍未编写
或运行任何单元、集成、功能、端到端或浏览器测试用例。

候选制品随后用独立 18090/18091 监听启动；两端 root/live/ready 均为 200，Public `/api/v1/worlds` 为 200，
Admin 匿名 session 为 401，Public/Admin 共 12 个 index/JS/WASM/CSS/SVG 响应逐个与 payload 清单哈希一致。
旧运行映像先由 `/proc/<pid>/exe` 与磁盘逐字校验并保存回滚副本，再经 SIGTERM 有界排空；候选 Public
`8691dc5050b8249bfb6237e5dae859fa78b32037b3143b13e93ff9362abe0798` 与 Admin
`54cb004af5c981a1e5511229c8a451c00ae95e308d792b7be39c096b7b714026` 随后接管 8090/8091。接管后 localhost
与 `192.168.31.210` 的 root/live/ready 全部为 200，运行中 `/proc` 哈希与候选完全一致；临时 canary 进程和配置
已经清理，回滚副本保留在忽略的 `target/` 运行目录。该证据只证明新制品交付与访问正常，不外推为待补竞态、
超时或完整 WebUI 等价。

第二实施批随后按矩阵完成 `API-041/API-043/AUI-094/AUI-095`。Public 的 worlds/current/auctions、bid log、
price history、rules、blacklist 和 compensation 读取已进入逐路由 decoder。Admin 的 intake、proxy events、
settlement/recovery、credentials、renames、mail、realtime、audit、rules 与 runtime config 已按旧 validator 的
字段闭集或兼容扩展边界恢复 UUID、safe integer、时间、枚举、分页、路由归属和版本语义；直接 serde 回扫只剩
使用严格 contract 并继续执行语义 validator 的 recovery/session。交叉复核还发现新版 realtime failure HTTP
曾按“offset + 实际条数”计算 `has_more`，现已恢复旧版“page × page_size”的 wire 算法；`final_fast` 仅在
Admin wire 映射为旧 `active`，内部状态不变。Rename revert 会重读目标 user 并使用最新 state version；Users
状态 badge 已通过 Fluent 覆盖精确的 `en/zh-CN/zh-TW/ja/ko` 五语言闭集。

第二批完成后，rustfmt、diff whitespace、repository/layout/config/i18n 策略门禁、全工作区 all-targets check、
PostgreSQL/SQLite Storage、Redis feature check、Public/Admin WASM check 与 all-features `-D warnings` Clippy 均
通过。没有新增或运行单元、集成、功能、端到端或浏览器测试用例；运行态只执行用户授权的一次性候选和访问取证，
不能替代矩阵中仍待的损坏响应与完整逐页 WebUI R1。

第二批优化 payload 的 Public SHA-256 为
`4c2c296d5c67cfd27dfb9bb87674f1209ed542f1022f3945046793394c1e310e`，Admin 为
`4bce743e4acf8e61560ffbface9500a5f7593befb24c10c7b0fa3fe4888be9a3`；内嵌二进制 Public 为
`aafd2aff1d4cbf2ff0edbad393e390e95d5d482208bf1b583a32018603c23914`，Admin 为
`d0c8183bd715b321a027f574a46b3f850d2e6b4ee4f0275506389268f753ed8c`。候选在独立 18090/18091 上的
root/live/ready、Public worlds、Admin 匿名 session 和两端共 12 个清单资源均符合预期；旧 8090/8091 映像
经 `/proc` 核对后保存到 `target/runtime-r1-parity-decoders-20260829-rollback/bin` 并有界排空。新制品接管后，
localhost 与 `192.168.31.210` 的 root/live/ready 全部为 200，运行中 `/proc` 哈希与候选一致；临时 canary 和
配置已经清理。Public/Admin 当前入口分别为 `http://192.168.31.210:8090` 与
`http://192.168.31.210:8091`。

2026-08-29 第三个静态收口批次完成 `BID-019/SET-012/API-044/API-045/AUI-096/OPS-008`。代理命令
新写入继续使用旧 canonical kind/result，Public bid log 与 Admin proxy history 只读兼容早期 Rust 短 kind；
结算明细在领域/存储重放边界恢复税/补偿矩阵。Admin 23 个 mutation 入口统一为 rate limit → Origin →
CSRF → session，GET 不再消费写限流，论坛预览恢复缺失的会话认证；Public 两条身份化读取先认证且不占用
写额度，其余读取 UUID 统一使用规范 v1 解析。Admin 客户端 catch-all 已收回 `AdminShell`。

同批还恢复 PostgreSQL `000005/000015` 的历史文件字节，并以固定、计数受限的执行视图处理 PG18 保留字；
fresh/原始历史保留原 checksum，早期别名修正版只在既有值等于固定替换 SQL 的 SHA-384 时沿用，readiness
与 v28 adoption 共用该边界且不改写 `_sqlx_migrations`。复核确认 Worker lifecycle 前后两次 weekly bootstrap
分别承担预处理和后置对账，不是重复 owner，因而没有删除；Public page-size 解码与登录 DOM ID 也没有足够
冻结证据支持改动。本批 rustfmt、repository/layout/config/i18n、全工作区 all-targets、PostgreSQL/SQLite
Storage、Redis feature、Public/Admin WASM 和全特性 `-D warnings` Clippy 均通过；没有新增或运行任何测试，
矩阵中的认证组合、混存账本、损坏结算和迁移两代历史仍明确保留 R1。

2026-08-30 按运行环境反例完成 `LIF-003/PUI-029/API-046`。两后端 Preview writer 都在状态写入和
intake target 转移前检查 pending lot 非空；Public 把 `opening` 从通用非 live 倒计时中分离为五语言
“开拍处理中”；Admin dashboard 的 MoogleMail availability 与 Worker 一致合并 world 专属和 `shared`
scope。测试库唯一空 preview 通过既有 Admin 命令从 version 2 取消为 version 3，关联 scheduled round
转为 missed、auction_id 清空并追加 `auction.cancel` 成功审计；下一周 draft/active round 保持 version 1、
attempt 0，没有被本次修复改写。

本批通过 rustfmt、repository/layout/config/i18n 策略、workspace/all-targets、PostgreSQL/SQLite Storage、
Redis feature、Public/Admin WASM 和全 workspace `-D warnings` Clippy，未新增或运行任何测试用例。
embedded Public/Admin payload SHA-256 分别为
`35691efa6e7c1407a0b3062dc57e1894142e8c2aad75e600fdadc6eb34603497` 与
`030aae03309771a0c72c99447e3830156eea2beff7be3cc6bf3a29c8ef1a1a4c`；测试环境三进程已用新制品
有界重启，运行映像与 artifact 哈希逐项一致，Public/Admin root/live/ready 均为 200。Worker control socket
为 owner/group `ngnl`、0660，`/v1/health/live`、`/v1/health/ready`、`/v1/status` 均为 200，状态为
serving/ready 且 `mail_pollers=0`。MoogleMail 继续显式关闭，没有触发真实外部邮件请求；Admin dashboard
已从 shared-only 数据正确投影 main availability。由于当前没有合法非空 opening 场次，PUI-029 的真实过渡
画面仍保留人工 R1，不能由制品可访问性外推关闭。

2026-08-31 根据测试环境真实反例补齐 `BID-008` 的竞价事务读取防腐层：PostgreSQL/SQLite
锁内快照通过显式持久化防腐入口原样恢复 Admin/MoogleMail 合法保存的任意正起拍价和真实当前价；
数据库、领域状态、账本和 Public 投影不再把它们静默取整。旧版
`current_price=start_price + 无领先者` 哨兵继续按两个原始值精确比较，其他当前价/领先者矛盾仍
失败关闭。固定增量和 proxy-last 保留既有价格余数，PPM 分支按旧版先向上折算当前价网格单位、
再把整千增量加回原始当前价，因此同样保留余数；
首次、slow/final 自动代理事实及旧账本重放因此可以保留正的非整千金额。HTTP 普通出价、代理上限、
论坛/Worker 命令和规则默认值仍通过严格构造保持整千网格。起拍价快照防腐部署后，同一 `1111`
起拍 lot 的认证普通出价 `1,111,000` 已于 2026-08-30 22:43:45 UTC 返回 200，并只写入一条
ordinary fact；此前同一 lot 的请求稳定返回 503。最终按旧生产公式修正并重新部署后，本轮只读核对
Public 投影为 `start_price=1111/current_price=1111000/next_bid=1167000/has_leader=true`，没有为了
验证再次提交会改变用户竞价状态的命令。

2026-08-31 第一批审计缺陷实施引用 `WRK-012`、`SET-002`、`SET-005`、`SET-013`、
`SET-014`、`PUI-011` 和 `AUI-055`：论坛命令账本接管复用 canonical 旧 ID；当前结算重放从
权威事实重算，commit 只重试序列化冲突/死锁，HoldCurve/LegacyZero 与补偿池开关形成闭集；
Public session 失败提供可见重试，Delivery 取消明确丢弃草稿。修改覆盖 Application/Domain/
Storage/Jobs/Public/Admin、配置示例与模块契约。`just verify`、repository/layout/config/i18n
策略门禁和差异空白检查均通过；本批未新增或运行任何测试用例，canonical 扫描、故障注入、资金和
浏览器行为仍按各差异行保留 R1。

每一批实现前必须从本表引用差异 ID，完成后补充修改路径和静态/运行态证据；不能用一次狭窄的
编译成功证明整个切面的等价性。

在本轮约束下不新增或运行单元、集成、功能、端到端或浏览器测试。实现后的最低验证是 rustfmt、
仓库策略、目标 crate 编译/Clippy，以及用户已授权环境中的只读或受控新旧对照；涉及外部写入、
canonical 数据接管和部署切换时必须先满足对应运行手册的安全前置条件。
