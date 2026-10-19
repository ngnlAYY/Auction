# 实现与验证状态
[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [默认验证范围](docs/guide/development.md#默认检查)

本页登记源码能力、历史验证与待补证据。隔离验证声明、定向构建及限定部署不足以证明完整业务或生产验收；文档修订不改变历史结果。

## 本页导航
- [能力范围](#能力范围) - [证据口径](#证据口径) - [本轮静态检查](#本轮静态检查)
- [源码核对发现的实现缺口](#源码核对发现的实现缺口) - [仍需运行证据的事项](#仍需运行证据的事项) - [未登记为现有能力](#未登记为现有能力)

## 能力范围
| 范围 | 源码中的实现边界 | 不能据此推导 |
| --- | --- | --- |
| HTTP 与页面 | Public 26、Admin 46 条 feature API；15/21 条浏览器入口 | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| 迁移 | PostgreSQL 48 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
| 发布 | API-only/embedded 隔离、payload 清单、确定性归档门禁及默认分支/手动 Actions 制品任务 | 新工作流已经完成制品上传、部署或运行验收 |
| 无配置首次启动 | 默认 loopback 设置页、五字段数据库输入；合并入口支持显式 HTTPS 代理及确认后空库初始化/启动 | 本轮已在真实主机完成设置、迁移、登录或浏览器验收 |
| 可选合并制品 | `auction-server` 自管三角色子进程，`auctionctl` 独立；默认分体包不变 | 已完成信号、父进程强杀、代理、迁移失败和升级运行验收 |
| 灾备 | 独立 backup/verify/rebuild/restore 与 canonical v28 接管流程 | 备份可用于未经验证的生产恢复或任意历史 schema |

注册数由[路由](docs/contracts/routes.md)、[静态入口](crates/auction-http/src/assets/routes.rs)及[迁移目录](migrations/postgres/README.md)核对；能力缺失应按契约返回明确错误，不能用空数据或示例数据替代。

## 证据口径
| 表述 | 必须有的依据 |
| --- | --- |
| 已实现 | 可定位的 owner、调用路径、契约与后端能力 |
| 静态检查通过 | 本次执行命令、退出结果、工具链及检查范围 |
| 隔离环境验证 | 明确环境、输入、目标版本、执行结果及保留的原始证据 |
| 生产验收完成 | 指定制品、真实配置/服务、生产数据边界、部署切换及恢复证据 |

HTTP、竞价、结算、浏览器及 canonical PostgreSQL v28 接管/灾备曾登记“局部 R1”；原始证据索引不足，保留为历史隔离验证声明，不能替代具体执行记录或视为当前运行确认。

## 本轮静态检查
各记录只覆盖注明的工作树与验证范围，不代表运行验收。下文 `verify全绿` 指完整 `just verify` 通过：文档份数（未注明为 45 份）、格式/仓库/布局/配置/i18n、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）及双端 WASM。检查入口见[开发指南](docs/guide/development.md#默认检查)。

2026-10-14 四轮依赖调整的公告审计共用 cargo-audit 0.22.2、`cargo audit --deny yanked --deny unsound --deny unmaintained`、1310 条公告及公告库提交 `6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`；“审计修复”的不同策略单列。

### 2026-10-19 Public 展示调整补发部署（已上线）

此前的限定热修复只更新 Admin 与 Worker，本次单独更新 `auction-public-api`，将 `auction-web-public` 的
补偿流水与历史成交价调整上线。期初/期末占位流水隐藏，整期结算等式固定在每页跨八列的表尾；
历史成交价空查询只展示居中搜索区，提交筛选后读取装备列表。复核补齐“清空”按钮：复用重置回调，
同步清除已应用 URL 条件、旧结果与草稿，返回居中搜索态。配套 Public HTTP 可选预期身份头与会话绑定同版构建，
沿用已记录的 v1 兼容契约，无新增迁移。

完整工作树 `just verify` 退出 0；Public WASM 与原生内嵌制品重新构建。六个 payload 文件通过清单哈希校验，
WASM/CSS 含补偿表尾及居中搜索标记，原生制品含同一 payload 指纹与服务端身份头校验，未混入 Admin 页面。
Public SHA-256 为 `5faee02917534f207cb0a8ca89c48fc9fc6cee84a183da84dbc32b7fdc6332cc`，
payload 为 `f926107ea41dcd5b1d04c3820f30faefffb16675c6325048f3169d2f8feb4056`。

15:28:16 UTC 单独停止并原子替换 Public，15:28:19 确认新 PID 71120 active/running、自动重启计数为零，
运行 ELF 与新制品哈希一致；Admin PID 70668、Worker PID 70666 的 InvocationID、运行哈希及 active 状态均保持。
旧 Public 制品、哈希及切换回执保留在部署目录 `.bf/public-ui-20261019/`，本地
`target/dist/public-ui-20261019/` 保存 source patch、构建清单、payload 清单与制品。未新增或运行行为测试、
发送业务 HTTP 请求、迁移或修改配置；本次证据来自静态分析、编译、制品检查、进程状态与运行映像。

### 2026-10-19 云端结算报告与 MM 所有权恢复热修复（已部署，自动轮询恢复）

只读云端证据确认：Admin 正在运行的报告 SQL 与旧源码逐字一致，14:19:58 UTC PostgreSQL 记录
`syntax error at or near "FROM"`，对应报告返回 503；主世界第 1 场结算及非撤回 lot 事实齐全。
MM 自 10:20:28 UTC 持续 `credential_unavailable`；凭据记录仍 active，邮件 advisory lock 消失，
运行 generation 仍为启动时的 25，自动调度与手动触发均继续到达凭据选择层。

`auction-storage` 保留本轮报告 SQL 修复，并为凭据 owner 首次永久撤销记录闭集安全原因；
`auction-http` 报告读取与投影校验失败记录拍卖标识和闭集失败码。Worker 观察邮件 owner 的已知失效状态，
沿用十秒有界排空，成功排空也非零退出，由进程管理器重建并重新取得 advisory lock、推进 generation。
保留原有失效后停止外呼、sending 未知结果隔离和 generation 行锁，不在旧进程内重新抢锁。

完整工作树 `just verify` 退出 0，定向独立复审未发现阻断问题；未新增或运行行为测试。
发布从基线 `b27320f9c1c2ce47b54cf45a88a0c8396008b84f` 隔离九个代码/清单文件，仅更新 Admin 与 Worker，
不夹带其余待发布源码调整。Admin Web 在隔离树重新构建，八个 payload 文件通过哈希清单核验；无新增迁移。
两条报告 SELECT 在目标 PostgreSQL 的只读事务内完成 `PREPARE`/`DEALLOCATE`，语法、表与列引用检查通过，
未执行报告业务查询或修改 schema。

14:49:47 UTC 按 Public → Admin → Worker 停止，原子替换两个制品后按 Worker → Admin → Public 启动；
14:49:52 三角色 active/running，自动重启计数均为零，运行 ELF 与上传制品哈希一致，Public 仍使用原制品。
Admin SHA-256 为 `630c51b6dd00a3ab4f8b72b7834f5b04d16505d74082a539f3e295252745f410`，
Worker 为 `ff1d1aa322862d8e18479b74719c3f56738244829a1a2ad60a87cf2bea36e498`；
Admin 内嵌 payload 为 `ebf3ef2fd8346e3795547fc71ab7b5f8594f1e00367a92ec40e31ffc2e4dc05e`。
制品内固定查询逐字匹配修复 SQL，旧错误查询已排除；管理端读取错误同时具有闭集安全诊断事件。

新邮件 owner 于 14:49:47 UTC 激活 generation 26，邮件与论坛 advisory lock 均存在。
14:50:22 UTC 日常自动任务完成三页维护及收件批次，`mail_poll_state` 的连续失败次数为零、失败码/时间为空，
下一次计划为 14:55 UTC；本轮未手动触发 MM、登录或发送业务 HTTP 请求。
旧制品与哈希及切换回执保留在部署目录 `.bf/hotfix-20261019-report-mm/`；
本地 `target/dist/hotfix-20261019-report-mm/` 保存 source patch、构建清单、payload 清单和双制品。
未执行迁移、修改配置、重写结算或未知发送事实；Admin 启动依照既有契约撤销旧管理会话，需要重新登录。

### 2026-10-19 全仓审计修复与 Public 展示调整（静态检查通过）

修复六项源码问题：`auction-storage` 更正结算报告累计领先时长 SQL，排除不参与结算的撤回标的，保留其余缺事实错误；
Web payload 指纹加入 `auction-contracts`；`auction-config` 将 bot 名单独立投影给 Worker；
`auction-observability` 原子领取 tracing 指标增量，防止并行 scrape 重复累计。
Public 的竞价、代理、标记、警告渠道和注销绑定确认宿主身份，HTTP 在当前 Cookie 认证后核对可选预期用户头；
未提供头的旧 v1 客户端保留原语义。跨标签页认证通知、聚焦和恢复可见均重读会话，同账号重登保留未知命令 ID。

补偿流水隐藏期初/期末占位行，将整期权威结算等式放在每页跨全列的固定表尾；历史成交价默认只显示居中搜索区，
提交筛选或恢复带筛选 URL 后才读取装备列表，重置回搜索态。

最终工作树 `just verify` 退出 0：策略/格式、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）
及双端 WASM 检查通过；受影响发布脚本通过 `bash -n` 与 ShellCheck。只读 PostgreSQL 语法解析覆盖 50 份迁移执行视图
及 907 条完整 Storage SQL，四处动态模板片段按调用方人工核对；`git diff --check` 通过。
本轮保留原有未提交改动，未新增或运行行为测试、连接业务数据库、构建发布制品或部署。

### 2026-10-19 补偿池修复制品重建（完成，用户手动更新）
用户明确授权后执行 `just build-release`，退出 0；Public WASM 重建，Admin payload 复用，分体四程序输出到
`target/dist/artifacts/embedded/bin/`。`auction-public-api` SHA-256 为
`81546ebb38e11bb49740d53cf6948aa82d6bc8aa281e32b0c12036863026a52b`；Public payload 为
`7734af9afcd91f485367ef014d04b69a4951cadc24df5fe58e3f93bafdc7f424`。双端 payload 文件及清单哈希一致，
Public 二进制包含新 Public 标记且无 Admin 标记；`BUILD-INFO` 标记为含本次未提交修复的 dirty 工作树。
本轮仅构建与静态制品检查，未运行行为测试、上传制品或操作云端服务；手动更新后的页面恢复仍待核验。

### 2026-10-19 补偿池前端链接误判（源码已修复，未构建部署）
用户要求结合云端日志续查：Public 在 03:47 UTC 的批次/流水请求均为 200（流水 request_id
`f7d56ce5e41645a3982ce9e32a4c9207`）；停机前留存的 200 响应含 23 条流水、21 个完整装备链接，
旧 Web 校验却只允许 ID，导致合法 ID/key 链接被判为 `invalid_response`，前 10 条中的 9 个链接同样受影响。
`auction-web-public::api::decoder` 改为复用目录完整链接校验，仍拒绝空串、keyless 与非法 URL；
`auction-http::public::compensation` 将含 key 的流水响应改为 `no-store`，分页、JSON 字段及结算事实不变。

03:50:43 UTC Public 收到 SIGTERM 后正常停止；03:57 UTC 只读复核三角色均 inactive，页面及两项 API 为 502，
Caddy 记录 `dial unix /run/hv-auction/public.sock: connect: no such file or directory`。本轮只读云端日志/状态和公开请求，
未启动角色、迁移或改写线上数据。格式、两处变更路径主动 LSP、repository/layout/config/i18n 与 diff 检查通过；
未新增测试、执行编译或部署，Public WASM 与内嵌 Public 二进制待重建，页面恢复待另行核验。

### 2026-10-17 资金迁移保留字与本地云端副本迁移
`566f5e7` 后由正式总控继续：44 修复两条时间倒退并追加 2 条审计，45/46 提交；47 因 `settlement_funding_commands.authorization` 未加引号被 PostgreSQL
保留字拒绝回滚；只读复核 46、时间倒退 0、未知投递 88、三角色停止，storage 兼容层按 47 固定描述加双引号，语义与已发布 checksum 不变。

本地副本：原 schema40 库已备份，仅回环恢复库经 `migrate-managed` 完成 39→48（48 成功、44 用户、2 拍卖、101 投递含 88 未知、时间倒退 0、修复审计 2、
资金账户 0），未复制生产密钥，应用停机待重建同版程序；无行为测试或外部服务验收，完整静态检查与第二版制品待完成。

### 2026-10-17 云端升级的用户时间迁移阻断（执行兼容静态检查通过）
获准窗口内核验 `482e0e8`、schema39 库与备份后由正式总控顺序提交 `000040`—`000043`；`000044` 的 ALTER 因 `user_forum_identity_account_id_check`
延迟未执行，被 PostgreSQL 18 以 `cannot ALTER TABLE "users" because it has pending trigger events` 拒绝回滚，只读复核仍 43、时间倒退 2、88 条未知
投递快照未变，三角色停止。

用户批准最小执行器修复：`auction-storage::migrations` 受限视图只在 44 ALTER 前执行该约束，失败随原事务回滚，已发布 SQL 与 checksum 不变；
changed-path LSP 无诊断，`verify全绿` 通过，尚待制品重建与迁移重试。另经正式 restore 将 schema39 备份导入仅回环本地库（44 用户、2 拍卖、101 投递
含 88 未知），未复制生产密钥，未接入页面。

### 2026-10-17 论坛无装备通知分类遗漏（本地修复与定向复审完成）
前轮复审 funding 三项与 evidence 期限 `Merge verdict: OK`、execution `BLOCK`：真实 `forum_auction_notice` 复用 mail_intake 与 `forum:…` origin，分类器
遗漏其 origin/模板，使合法零资产通知阻断内部履约；阻断保留，已通过源码不再改审。以冻结 tree
`25cb78b517c79dd2a008cf525ec254d878708fbb` 为输入仅改 storage conflicts 分类器：按同 command_id/world、command_kind=notify、零金额账本重建完整
`forum:<auction_uuid>:<thread_id>:<post_id>:<line>:<lot_code>` origin 并核对模板/结果码、零 Credits/COD、空附件与无正文；只排除已证明无装备通知，
未知 schema 与真实资产仍阻断。

定向 rustfmt、`cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres` 与 `verify全绿` 通过；execution 定向复审
在冻结 tree `f79459b1ffeacda6ca7c437edbbf65eb72b34790` 得 `Merge verdict: OK`，P1 闭合、无新增 P0/P1/P2，五项发现全部闭合。未运行行为测试、LSP、
SQL、迁移、服务、外呼或发信，未操作旧 32 条未知投递，未 commit/push。

### 2026-10-17 Bot 内部履约五项复审发现（本地修复，待定向复审）
初次三路复审后父采纳资金三项、装备冲突一项与现金 fence 期限一项，不改写此前记录或阻断结论。三组件以冻结 tree
`19e03eb65eaf56d93327420134e059532cbcb0c5` 输出 2/1/5 文件增量：funding 用 snapshot 阻断未闭合外发 COD opening 并在满足/发行前检查同一现金；
execution 按真实 origin/payload schema 排除无装备通知，未知载荷仍阻断；evidence 将 prepared 单调 deadline 经 `start_sending_authorized_until` 传到现金
fence，PostgreSQL 18 transaction_timeout 覆盖整事务与 COMMIT。

显式 `cargo check --locked --workspace --all-targets --all-features` 与 `verify全绿` 一次通过，8 个源码文件逐字节等于组件冻结输出；下一门禁为原三位
reviewer 定向复审。未运行行为测试、LSP、迁移、服务、资金 CLI、外呼或读取秘密，未 commit/push；PostgreSQL 时序/并发、真实 Main/Isekai 协议、
运行对账与大库扫描待授权。

### 2026-10-17 Bot 内部履约与现金保护（静态整合，待独立复审）
在 `ad33eb7` 基线整合证据/资金 foundation、内部履约与现金发送门三补丁：credentials/integrations 拥有 HV认证UID+论坛资料名、正向 Armory 库存与 mail
generation，funding 拥有真实对账/来源/未知占用，fulfillment 拥有内部证明与原子依赖推进，delivery 拥有全部现金发送门；Worker 注入同一 gateway/凭据/
owner，forum 关闭或缺证据时延期。内部结果不伪造 delivery/Sent，不改 B+C=S+T 与 payment→handoff→seller；Isekai 单独证据，退款仍需原恢复授权；
retained/税不自动再投资；有 allocation 的新 Admin delivery 动作拒绝，旧无 allocation 重试按两阶段预锁接入。

新增 `000046`—`000048`；[bot-funds](apps/auctionctl/README.md#bot资金对账) preview 零写、apply 才复核证据；[手动升级](ops/README.md#bot-内部履约与资金保护升级)
要求可验证备份、全 writer 停机与同版程序，禁止滚动混用旧 Worker。显式 `cargo check`（workspace/all-targets/all-features）与 `verify全绿` 已通过，初次
检查的纯编译接缝以最小修正收口，未改旧测试或关闭 lint。独立复审为下一门禁；本轮未运行行为测试、LSP、迁移、服务、资金 CLI、外呼或发信，未
commit/push；真实 Main/Isekai 协议、资金对账、PostgreSQL 并发待授权。以下旧阶段结论按原时间保留。

### 2026-10-17 截止、发送与 Admin 修复阶段交付
三路复审及定向复查均收口为 `Merge verdict: OK`，影响区未发现当前明确的 P0/P1/P2：integrations/jobs/storage 负责真实发送前检查、同凭据互斥、
claimed/sending 续租与发送门禁；web-admin 负责分页未决命令、凭据导入累计未知、首次提交与重试共享的一次性 Key 门禁；domain/config/application/
storage/auctionctl 负责 900/900 秒新默认与合格草稿修正，现有显式 60 与已结算场次不变。

既有修复与 Admin 分页保留在最终工作树与 `verify全绿` 范围，复审后仅追加本节。**bot 自动免寄/内部结清仍未实现**：缺可信自身名称、库存与资金证明，
`000044`、`000045` 尚未执行，普通重启不自动迁移，后续遵循[手动升级说明](ops/README.md#截止规则与邮件发送隔离升级)；旧未知投递与草稿未改，未部署
或执行行为测试。

### 2026-10-17 Delivery 慢续租饥饿修复（待第三轮定向复查）
原 A 确认 prepare 裸等租约已修复，但续租 4 秒/间隔 3 秒时 renew_at 已到期、biased 续租分支抢占准备与发送 future，形成新增 P2；Admin C 复查 OK、截止
B 保持 OK。本次仅改 jobs delivery 调度：准备首次续租独立确认，循环按请求超时 → 租约过期 → 双截止保护的操作 poll → 到期续租排序，成功续租后仍在
预算内先获一次 poll；续租时刻/TTL、fence 确认与 SQL 有效期门禁未改。

源码推演 lease10/renew3/finalize5 未编写或运行测试；`verify全绿`（主线 target、offline、`--locked`）通过，Sleep 借用冲突经冻结 request_deadline
修正后复跑通过，下一门禁仅为原 A 第三轮复查。本轮未运行行为测试、LSP、外呼、服务、迁移、秘密读取或提交，bot 内部履约仍待真实证据闭合。

### 2026-10-17 独立复审三项发现修复（第二轮静态记录）
三项复审中截止规则/草稿修正为 `Merge verdict: OK`；父采纳 prepare 租约 P2、Admin 导入 unknown P1 与一次性 Key 覆盖 P2，由隔离组件修复整合：
delivery 在 prepare 中为 claimed 续租并在首次外部 poll 前检查单调截止，storage 先锁行再校验租约与 fencing，提交未知不退回 pending；凭据导入累计
unknown 并锁定原字段与 UUID，仅明确成功才复位；Users 首次提交与原命令重试共用同步页级 gate，Key 待确认关闭后才释放。

整合后 `verify全绿` 通过（rustc/cargo 1.99.0、主线 target、离线、`--locked`），仅同步全局 owner/契约；下一门禁为原 A/C 审查者定向复查。未运行行为
测试、LSP、迁移、服务、外呼、秘密读取或提交；可信身份/库存与 bot 自动内部履约仍未完成。

### 2026-10-17 截止规则与邮件发送隔离整合（初次静态记录）
在 97 文件 WIP 基线上整合三项增量：credentials 贯通 nullable 预期玩家名称与命令/DTO 兼容（新增 `000045`，旧凭据不回填）；delivery 的只读 compose
preflight 位于 sending fence 前并持有同一 Cookie guard，不安全时释放 claim、撤销计数、追加审计并暂排对应 world 的 Send；新场次默认软截止 900/900
秒，草稿修正 CLI 默认只读、execute 具备版本/确认/Serializable/重放/审计门禁，已有显式 60 与旧草稿仍需
[用户手动升级](ops/README.md#截止规则与邮件发送隔离升级)。

**自动 bot 内部免寄/付款未完成**：无可信自身名称来源，无绑定返回 None、有绑定返回 `identity_unproven`，无内部 satisfied/资金写路径或 `000046`，
旧 32 条未知投递与草稿保持人工核对。整合静态执行 `verify全绿` 通过（`--locked`、离线），首次编译的 application 根导出缺失已补齐；独立复审为下一
门禁，未运行行为测试、LSP、迁移、服务、外呼或部署。

### 2026-10-17 用户列表时间异常与界面修复（进行中）
经授权以现有 Admin 会话只读请求用户列表，确认 HTTP `503 temporarily_unavailable`；云端只读事务确认迁移停留 `000039`、43 个用户中 2 个
`updated_at < created_at`，创建时间匹配 MM `registered` 账本、旧更新时间匹配论坛身份同步历史，其余标识/状态/版本/文本/时间范围未见异常，未读取
Key、Cookie 或密码。源码分离 Users 本地时钟与装备入站时间门禁，storage 快照更新时间不倒退，`000044` 仅更正有双重证据的倒退行并追加审计；Shared
Dialog 点背景保持打开、关闭/Esc 与 busy 保护保留。

上述修复已通过 `verify全绿`，但主动 LSP 探测的 3 个路径未能确认；Admin 分页仍在隔离实现；后续浏览器抽查因会话失效返回 `401 unauthenticated`，
未取得页面证据。本轮未迁移、部署、重启、发信或真实竞价，源码改动不代表线上恢复，UI 尚无运行验收。

### 2026-10-17 最近五次提交审计修复
只读审计 `367e342` 至 `2842c19` 确认两项 P2（均由 `450cd52` 引入，分属 web-public 与 storage）：装备标记累计保存原命令未知状态、重试遇 429 不降级，
忙碌/未知时禁止迟到刷新清除命令；MM 内部入队保留 SQLx 错误，由竞价边界识别 `40001`/`40P01` 恢复有界整事务重试，独立邮件入口仍脱敏。无新增路由、
DTO、配置、迁移或依赖。

修复后 rustc/cargo 1.99.0 下 `verify全绿` 通过（`--locked`），原两位审计者复查确认闭合、影响区无新增 P0/P1/P2，结论均为 `Merge verdict: OK`，源码
冻结；本轮未运行行为测试、LSP、迁移、服务、浏览器或外部操作。

### 2026-10-17 PM 与论坛结拍复审修复
初次复审阻断三项：终态遗留 pending 且关闭正文校验时反复 POST 未变化固定帖；偏好首次结果未知后重试遇 429 丢失未知；论坛帮助仍只称 MM 反超。现已区分
部分镜像与最终全页确认、pending 不清除且未知写入门禁不放宽，偏好累计保存未知并在重试/刷新共用，完整原命令保留到成功或明确 409，论坛帮助明确
MM/PM 选择、MM 默认、PM 仅反超与 `notify mm`/`notify pm` 语义；owner 为 jobs、web-public 与 integrations。修复后 rustc/cargo 1.99.0 下 `verify全绿`
通过，原两位复审者确认三项闭合、无新增 P0/P1/P2，结论均为 `Merge verdict: OK`。

两项初次整合记录中的“未运行行为测试/未使用扩展”仅描述 writer：父会话运行器日志另记录三项目标被调度（一项 stale、两项因预算延期），不属于授权
验证；经用户确认以任务专用禁用配置重启父进程后才恢复本次修复，本次续接只做显式静态检查，不调用 LSP/浏览器/外部服务，不执行迁移、部署、发帖发信
或提交推送，数据库并发与真实论坛协议仍待授权。

### 论坛结拍奖惩展示（本轮静态整合，独立复审待完成）
forum owner 新增 `000043`，历史 settled 不纳入、完成版本不伪造，升级时未结算及新场次按原冻结分页刷新；同一 Repeatable Read 快照核对持久结算、落锤价、
赢家、来源与连续领先起点（小时一位小数、精确 PPM，不重算经济规则）；终态停止新命令导入，live 结算走最终全固定页证明并在锁内复核 settled 版本与
content lease/fencing/revision，缺事实、正文超限、部分成功均不伪报完成。

本轮未运行行为测试，未迁移、启动服务、读密钥或发帖/发信，数据库并发与真实多页论坛协议未验证；rustc/cargo 1.99.0 下 `cargo fmt --all` 与
`verify全绿` 通过（`--locked`，all-targets 仅编译既有测试）；独立复审仍待完成，不以静态通过代替生产验收。

### 装备账号 MM/PM 竞价警告（本轮静态整合，独立复审待完成）
本轮追加 `000042`：账号渠道 GET/PUT 独立正 i64 CAS 与命令账本，旧 watch.channel 保持 mm 兼容占位；逐条接受竞价快照冻结渠道，统一来源跨渠道按领先周期
唯一，PM 缺论坛 ID 明确 suppressed 不回退 MM；PM 独立持久队列先 lot 后任务锁进入 sending，原 sender 不可换、未知/过期发送只读不重寄；Worker 独立
PM runtime 共享论坛网关与凭据 owner，CLI list/reconcile 只读。三个组件 lane 仅执行定向 rustfmt 与差异检查，已在基线 `450cd52` 整合，不改测试、依赖
或锁图；整合修复 PM 编辑器默认控件兼容、表单编码、SQLx 静态 SQL 与 CLI 标识转换，准备失败任务按领取时间与稳定 UUID 轮转。

主线 rustc/cargo 1.99.0 下 `cargo fmt --all` 与 `verify全绿` 已通过（含 `cargo check --locked --workspace --all-targets`），编译已有测试目标不等于执行
测试；独立复审尚待父级安排。未运行行为测试，未迁移、启动、发信、浏览器验证或访问真实凭据；并发取消/切换/竞价、崩溃/超时/租约、同 sender Sent
核验、论坛限流/拒收/满箱与 UID→姓名改名风险均未验证；Sent 排序视图可能消耗分页预算、详情标题节点可能带前缀，真实 HTML 兼容仍缺证据。受控部署与
原 sender 限制见[升级门禁](ops/README.md#装备标记与超价提醒升级)。

### 2026-10-17 装备标记与超价提醒（静态整合）
本轮按登录账号保存装备标记（服务器权威、本地隔离缓存，新建代理默认标记），MM 只按真实领先→被超越周期去重，取消或重新领先抑制未发送的旧提醒，PM 仅
预留；涉及追加 `000041`、Public P23/P24 与既有 delivery 发送链，契约见 [HTTP](docs/contracts/routes.md#p23p24-装备标记与超价提醒) 与
[不变量](docs/contracts/invariants.md#i-09g-装备标记与超价提醒按领先周期去重)。新增三个 watch owner 表，逐条竞价决定领先周期、同值新命令推进版本；
冻结来源 proof 保护领取与 sending，发送先锁 lot 再锁 delivery，失效任务显式收口不覆盖未知发送；按额外授权补齐 Admin delivery 闭集兼容（lot_watch
标签、严格失效 preflight、禁止旧周期人工重排），未新增页面/列/渠道/动作。

`cargo fmt --all -- --check` 与 `verify全绿` 已通过（含 `cargo check --locked --workspace --all-targets`）；本安全 writer 未运行测试、LSP/自动诊断、
迁移、服务、浏览器或外部邮件。初次复审 P2（共享 lot 锁抹去 SQLSTATE，使 Admin 三次 Serializable 重试无法识别 `40001`/`40P01`）已改为保留 SQLx 错误
交由 Admin 边界分类，锁序、来源 proof、发送状态与重试上限不变，定向复审确认闭合。并发数据库、升级回填、真实 MM 与 UI 行为仍无授权证据。

本轮只授权实现与静态验证；早期子代理的自动诊断扩展越界启动 Cargo 测试，日志中的测试记录不属于获准验证或验收证据，旧实例已停止，源码由不加载该扩展
的子代理续接，不改全局工具偏好。

### 2026-10-16 论坛首帖计时说明精简
按用户要求，`auction-integrations/forum/render/countdown.rs` 不再在双图片下重复附加整段计时说明；图片内 UTC 日期、开拍/截止列表、规则与页尾权威提示
保留，图片时间来源、P22 HTTP、分页与写入门禁不变。`verify全绿`、该 Rust 文件主动 LSP 与差异检查通过；本地独立 18092 预览与 BBCode 下载同步移除该段，
只读 HTTP 核对确认两图与 UTC 时间仍在（记录于示例目录 `removed-timer-note-checks.json`），并用独立临时 profile 的 headless Chromium 渲染
`forum-preview-without-timer-note.png`。本次未替换三角色 release 制品，未发帖、运行迁移或部署云端。

### 2026-10-16 论坛自托管动态 SVG
首帖计时器不再依赖第三方域名：application 新增公开时间窄投影，storage 单条 SQL 读取公开边界、开拍/计划截止与数据库时钟，HTTP P22 生成本站 SVG/CSS
数字转盘，integrations 只生成本站链接与 UTC 文字；无新配置、依赖、数据库对象或迁移，仅展示计划时间，不影响竞价或 soft-close。`verify全绿`、变更
Rust/CSS 主动 LSP、差异检查与原生 release 构建通过；独立复审发现 HEAD 被旧严格方法 builder 覆盖与亚毫秒截断两项问题，已新增图片专用 GET/HEAD
builder、正差向上取毫秒，复跑门禁并通过定向复审。

用户批准本地行为核对：临时进程注入标注的内存时间样例并复用真实 Public router，不访问业务库或外部服务；15 项 HTTP 核对覆盖 SVG 类型、32 KiB 上界、
CSP/CORP、双 CDN/浏览器 no-store、HEAD、405/Allow、无效参数、缺场次、未排期、过期与亚毫秒/最大跨度，Chrome 确认 `<img>` 加载、两次截图差异、
分/时/日进位、归零隐藏与 reduced-motion 降级；跨源图片先被样例父页 CSP 阻止，仅为临时样例 HTML 放开两个回环 origin。样例、JSON 与截图留在隔离
环境 `state/countdown-preview/`，不是仓库测试或权威数据。

新 `auction-server`/`auctionctl` 替换隔离实例，复用未改 payload，仍监听 `0.0.0.0:18090/18091`、PostgreSQL 回环；两端就绪 200，空库新图片 GET/HEAD
返回预期 404 且 no-store（`state/readiness-self-hosted-countdown.json`）。未插入业务样例、迁移、发帖、寄信或部署云端；真实非空库投影、论坛图片过滤/
代理、远端正文回读、Cloudflare 控制面与其他浏览器仍待验收。

### 2026-10-16 Public 自定义数量解除上限
按用户要求取消指定 Public 列表 200 条上限：共享输入、URL/偏好、HTTP、应用请求与结果页校验贯通正 `i64`，Admin/账户/实时事件上限不变，offset、超时与
cursor 互斥继续生效；四处存储游标只对探测行用饱和加法，共享页数改用无溢出上取整；无写入规则、迁移或 DTO 字段变化，五语言错误提示删除最大值文案并
左对齐。独立审查发现旧响应解码仍限 JS 安全整数，已仅为 `page_size` 放宽到完整正 `i64`，定向复审无剩余问题。

`verify全绿`、变更 Rust 主动 LSP 与差异检查通过，两端 Web 与原生程序重编并替换隔离实例（`0.0.0.0:18090/18091`，就绪 200）。授权只读核对覆盖空
列表：四个列表接受 201、1000、超 JS 安全整数与 `i64::MAX`，三条 cursor 路径接受最大值，无效数量返回 400，共 24 次 API 检查；浏览器确认 1000 可保存、
最大值跨 URL 与偏好恢复不丢精度、0/溢出不可提交。证据在 `state/http-unbounded-page-size.json` 等三份与构建日志；未造业务数据、出价、发帖或寄信，
无大数据容量验证，云端未更新。

### 2026-10-16 Public 数量偏好、论坛计时器与本地隔离启动
Public 指定列表使用统一数字分页，保留 10/20/50/100 预设并支持 1–200 正整数；`surface/pagination/` 拥有 URL 优先级与本地偏好，共享
`components/pager/size.rs` 只拥有输入、校验与可访问交互，Admin 分页不变；非法输入不请求不保存，首页入口导航时读最新偏好。独立静态审查发现并修复出价
筛选失焦与末页超出 offset 上限。论坛 `render/countdown.rs` 仅为首页添加 List/Auction Timer 并保留 UTC 兜底；契约见
[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)。`verify全绿`（45 份文档）、改动 Rust 主动 LSP、差异检查、两端
`just web-build` 通过；`auction-server` 与 `auctionctl` 以 `--locked --release --features postgres,web-embed` 编译，首页链接修复后重建 Public payload
并清理 release 缓存重编内嵌 server。

经用户要求在仓库外 `Auction-Test/local-test/` 建立 PostgreSQL 18.6 新库与三角色合并服务，仅回环 55432/18090/18091，缓存、论坛、邮件、告警、排期与
OTLP 关闭；角色配置与严格预检通过，空库 schema 门禁退出并建立 owner 锁后显式执行 `migrate-managed`（40 项成功、最新 40），Public/Admin readiness
200；不代表云端已迁移，旧库兼容迁移未获证据。

按用户允许范围核对隔离空列表：0/负数/小数/科学计数法/加号/空白/空值/字母/201 均禁用提交，1/200 可提交；37 保存后重新进入与往期恢复，URL=10 优先
且不覆盖偏好；首页旧链接冻结数量问题在本次发现并修复，重编后只含世界参数。Admin 登录页可渲染；未登录、出价、造装备数据、发帖或寄信，未新增自动
测试。同次快照仍现首屏“加载较慢”提示，其生命周期不在本任务范围。证据在本地 `logs/` 与 `state/`；论坛第三方图片展示、BBCode 回读与带业务数据的
全列表翻页仍未验证。

随后按要求将本地 Public/Admin 监听改为 `0.0.0.0:18090/18091`，精确 Origin 增加本机局域网地址，数据库仅回环、Worker control 仍为私有 Unix socket；
预检通过，本机经回环与局域网 readiness/页面均 200，未从其他设备验证路由与防火墙。仍为无 TLS 测试环境，只能受信网络访问，证据为
`state/readiness-all-interfaces.json`。

### 2026-10-16 Cloudflare 与 Caddy Unix 上游
`auction-http` 为正式 Public/Admin 响应默认发送两种 CDN 禁止缓存头，仅静态 handler 私有标记可放行；API 缓存契约保持，HTML/入口禁缓存，favicon 每次
再验证；Unix listener 不再伪造 TCP peer，Unix 信任开关不放宽真实 TCP；无新增依赖、配置或 HTTP DTO，TLS/地址清洗归 Caddy。集成后 `verify全绿`（45
份文档）通过，4 个 HTTP Rust 文件主动 LSP 无诊断；仓库 Caddy 示例与部署副本均通过 `caddy adapt`（仅解析不启动），副本仅改 listener/security、
Caddy 与两个 unit，另增 socket 目录定义与权限说明，备份 0700/0600，未改非目标 TOML。

后续按要求移除模板与本地 Caddy 的 Cloudflare 来源 IP 拦截（CIDR 仅用于代理地址头信任），通过两份 `caddy adapt`、JSON 静态核对、文档与差异检查；再将
副本的应用/HTTP socket 统一为 `ngnl:ngnl` 并移除额外 socket 组，五份非敏感 TOML、三份 unit、drop-in 与 tmpfiles 静态核对通过。权限审计发现三个敏感
TOML 为 0644、部署 state 目录 0755（不满足严格预检/总控状态要求），只报告未自动修改，新模板已设 0644；未读敏感正文，数据库目标与数据密钥一致性
未验证，Worker control UID 1000 仅与本机账号核对，云端 UID、Redis feature 与控制面未获证据。

本节不代表已安装或上线：未创建用户组、安装 unit、重启服务、签发证书或运行行为测试；云端权限、证书续期、真实客户端 IP、缓存命中、WAF、登录与
WebSocket 恢复仍缺证据。

### 2026-10-16 论坛目录分页与更新楼诊断
新主题目录每帖最多 150 件、从 #1 连续分页，系统更新楼位于末页之后；application 定义冻结布局与续页端口，integrations 负责分页与完整解析、jobs 逐帖
对账与命令导入，storage `forum/catalog.rs` 与追加迁移 `000040` 持久化续页身份/正文，证明与主记录同事务、同一 revision CAS 收口；旧已尝试/已发布/
未知写入主题保持单页，仅未尝试且无远端身份队列项与新主题启用分页。更新楼错误提示改为安全白名单（待重试/警告/忽略，最多 100 条且受字节预算），HTML
目录保留数字作者门禁，HTTP DTO 不增字段。

论坛变更通过 `verify全绿`（45 份文档）、29 个 Rust 文件主动 LSP 与差异检查；独立静态审查发现旧帖兼容与纯文本预览问题，修复后定向复审无剩余问题。
未运行行为测试，未迁移、发帖、启动服务或部署，尚无真实多楼发布/抢楼/未知写入与 PostgreSQL 运行证据。

### 2026-10-15 Actions 门禁修复与下载制品
只读核对首次 [Actions 运行](https://github.com/ngnlAYY/Auction/actions/runs/38757211415)：`fc28700` 的 docs、security-audit、msrv 通过，verify 因
runner 缺 `rg` 在布局检查退出 127，未进入 Rust 编译。工作流改为显式准备门禁工具、先跑廉价检查再并行编译/审计/MSRV，保留同提交去重与旧任务取消，
增加隔离编译缓存与固定哈希 dx 缓存；默认分支推送或手动运行全绿后调用原发布脚本上传通用四程序包。Owner 为 `.github/workflows/ci.yml` 与 `ci/`，未改
业务 HTTP/事件、配置、数据库或产物白名单。

本地 `verify全绿`（45 份文档）、actionlint、ShellCheck、Bash 语法与差异检查通过，变更文件主动 LSP 无诊断；静态核对 dx 归档 SHA-256、布局与 ELF 依赖，
未运行产品、行为测试或部署。本节登记时尚无新版全流程、缓存命中、完整 Release 重编与下载证据。

### 2026-10-15 论坛拒绝原因安全展示
新增 jobs `worker/forum/reply/rejection.rs`，把不可重试应用错误分类为具体拒绝码并将固定 #2 提示映射为安全英文，覆盖已领先、最低出价、金额、截止、资格、
状态与既有代理/通知结果；未知原因保留通用提示，不公开本地诊断。application 的 `ForumCommandOutcome` 增加既有错误码投影，storage 只读已有列，无迁移、
HTTP/配置或竞价规则变化；旧 `command_rejected`/空码仅经完整白名单映射（含 Hea04 已领先），不改历史账本、不重放出价，pending 顺序与未知写入门禁不变。

`verify全绿`、6 个 Rust 文件主动 LSP 与差异检查通过，模块 README、架构、schema、不变量与玩家说明同步。未运行行为测试，未重编 Release 包、部署或重启
云端，尚无真实论坛更新验证。

### 2026-10-15 论坛摘要与正文展示分离
Public `/forum` 保留同步摘要、真实处理楼层、更新时间与简短延迟提示，主题链接位于已同步提示行右侧；正文、源/内容版本、同步与错误详情移到 Admin
`/auctions`，默认折叠为转义 BBCode 文本。Owner 仅为两端 Web 展示、样式与五语言，沿用 P15/A46 DTO，不变更 HTTP 字段、权限、数据库或 Worker 同步逻辑。

`just verify`、双端 WASM 与完整 Ryzen 候选构建通过，两处 Rust 组件主动 LSP 无诊断；CPU/feature、资源隔离、ELF/glibc、Worker 依赖、样式归属、34 项
文件哈希与归档一致性核验通过（[构建记录](ops/build/README.md#2026-10-15-论坛摘要与正文展示分离候选)）。未运行行为测试或浏览器，未部署/重启云端；前
一项真实论坛恢复不构成本轮界面验收。

### 2026-10-15 用户授权后的云端论坛恢复
用户手动更新后，运行中 Worker SHA-256 为 `98135fdcc182a67b4853c4b8bf40c20f06a0f322b88157cdf77efaf93a63f30c`，与最新 Ryzen 候选一致，三角色 11:06
UTC 启动；Worker 省 `verify_post_body`，按新版默认关闭。原失败记录仍是 09:35 UTC 的 `write_authorization_required`、attempt_count=5，更新不会自动
解除门禁。

经用户再次授权，先只读核对板块标题唯一命中主题 `302009`、两种拍卖标记与固定帖身份，再于 11:12:54 UTC 对拍卖
`6673453c-99c2-4b2f-834b-65d1c0daea1e` 提交一次官方恢复（command_id=`81db12a1-70a8-45f3-ae71-de6dcdd5bd95`，
actor=`operator-assisted-recovery`），命令账本与 `forum.authorize_write_retry` 审计各一条。11:16:44 UTC 状态转 `published`、attempt_count=6、
thread=`302009`、#1=`6984876`、#2=`6984877`；11:16:47 UTC 内容版本 2、真实处理楼层 8、错误码皆空，第 3–8 楼 11 条竞价账本均为 `applied`（含第 3
楼 `Hea03 100k`）。远端只读 GET 确认原 #2 同步到 #8，Public 论坛投影返回 200、`published`、楼层 8。

本次仅覆盖用户授权的单场真实恢复与只读核对，不是完整生产或浏览器验收；未新增/运行自动化测试，未改配置、部署或直接改数据库状态。

### 2026-10-15 论坛校验开关与管理端状态刷新
`worker.forum.verify_post_body` 默认关闭，主题同步与回帖刷新统一配置，正常 POST 仍检查精确回执与固定帖身份；创建端口区分新建与旧帖，关闭校验时未知
主题发布等待人工授权、回帖内容未知保留门禁；现有 CLI 授权只适用主题发布失败。固定 #2 摘要改英文，公开页移除四处状态重试按钮，Admin `/auctions` 承接
只读刷新并新增 A45/A46 认证 GET，复用安全 DTO，不新增恢复写入口或迁移；Public/Admin 五语言与契约同步。

`verify全绿` 与差异检查通过，关键 Worker/HTTP/Admin 源码主动 LSP 无诊断，批量检查仍保留 Admin CSS 九项既有 `!important` 提示。已完成独立双端
wasm-release 与 Ryzen 四程序 Release 构建，CPU/feature、资源隔离、ELF/glibc、Worker 依赖、33 项文件哈希与归档一致性核验通过
（[构建记录](ops/build/README.md#2026-10-15-论坛开关与管理端刷新候选)）。未运行行为测试，未部署、重启或再提交恢复授权，线上恢复无本轮证据。

### 2026-10-15 论坛正文回读与限定恢复
应用端口统一只忽略末尾 CR/LF；集成层按真实编辑表单的列表边界换行生成 BBCode，其他正文逐字校验。固定 #1 增加配置 origin 下的当期
`Auction address` 与论坛 `About system` 链接，领先来源用英文 `(proxy)`/`(#N)`，解析器兼容旧中文/全角标记；Worker 对 `reconcile_` 未知写入使用只读
短退避，人工授权门禁不自动解除。Rust 1.99.0 下 `just verify`（45 份文档）通过，六个改动路径主动 LSP 无诊断，`auction-worker` 的
`--locked --release --no-default-features --features postgres,cache-redis` 编译通过；未运行自动化行为测试。

经用户单独授权对主题 `302009` 执行官方恢复（命令 `212c2d0b-f614-4a97-82a6-ec2e6682233c`），2026-10-15T08:31:21Z 记录
`reconcile_write_unknown`、attempt_count=2；只读 GET/浏览器确认原 #1 出现可点击当期链接、#2 仍停在楼层 2，编辑表单显示论坛删除了列表边界与末尾换行，
旧严格比较会失败，POST 回执未留原始证据，不将页面更新当作发布成功。未直接改表、重发未知 POST、创建新主题或运行新候选；用户最终选择手动更新。

#### 后续长网址回读差异
用户手动更新后只读哈希确认为上述候选；2026-10-15T09:20:24Z 首轮只读恢复转 `write_authorization_required`、attempt_count=3。再次授权执行
`b7f1cf64-0a49-428b-a125-48912f31f796`，2026-10-15T09:30:35Z 记录 `reconcile_write_unknown`、attempt_count=4；只读编辑表单确认新标签、说明链接与
英文代理已写入 #1，但论坛把纯网址显示文字截成带 `...` 的字符串（完整 URL 未变），足以阻止逐字证明；#2 仍停在楼层 2，不宣称发布或导入成功。

集成层恢复链接文字内的 `[b]` 包装（此前带格式完整网址经只读回读确认不被截短）。不放宽正文比较、未知写入或授权门禁，不新增迁移/HTTP/事件/配置
字段；没有再次授权、改表或重启服务，本次模板的部署后发布确认仍待补证据。

### 2026-10-15 PostgreSQL 收口与项目精简
storage 删除退役后端驱动、事务/查询适配、文件锁与不可达分支，config 仅接受 PostgreSQL 连接，apps/CLI/Worker 按单后端装配，既有事务、锁、幂等、审计与
outbox 规则保留；启用 tracing 时拒绝 SDK 两个环境导出头。CI 静态入口合并为 `ci/checks/check.sh`，敏感模板统一为
`ops/config/examples/secrets.toml`，Justfile 运行/迁移配方显式选 postgres；删除 48 个失效/重复文件，未删改已发布 SQL 或既有测试。

Rust 1.99.0 `just verify`（48 份文档）、MSRV 1.94.1 全目标/全特性 check、Bash 语法与 ShellCheck 通过；`cargo audit --deny warnings` 扫描 536 条锁记录/
1329 条公告无漏洞或警告，cargo-machete 无未用直接依赖；全工作区活动图只含 PostgreSQL 驱动。`just web-build` 完成双端 payload，五程序以
`--locked --release -j 1` 及 `postgres,cache-redis` 完成 API-only 构建，readelf 确认 x86-64 ELF64/GNU glibc、Fat LTO/单 CGU/unwind；构建后
`just verify` 再次通过（日志 `target/dependency-review-20261015/cleanup-*.log`）。未运行行为/性能测试、服务、迁移或部署，未执行正式 `release-check`。

### 2026-10-15 依赖审计与编译优化
覆盖 20 个工作区包、50 项直接依赖与完整锁图（决策与公告见[技术栈](docs/design/tech-stack.md#2026-10-15-全依赖审计)）：提高 19 项直接版本、更新 83 个
锁定版本，活跃节点 486 → 485、锁图 535 → 536（外部包 515 → 516）。工作区统一版本/profile/CI、摘要 API、严格 cron 与退役后端的编译面调整见决策表；
五个 apps 增加 Linux x86_64 编译期守卫，正式发布 GNU/glibc、WASM 保留；后端 release 与两份 AMD 配置采用
[生产 profile](docs/design/tech-stack.md#编译优化边界)。HTTP/JSON/事件、迁移、运行 TOML、会话算法与事务/幂等不变量不变。

Rust 1.99.0 全目标/全特性 check、MSRV 1.94.1 同范围 check 与 `just verify`（49 份文档）通过；两处原子 API 弃用仅作最小 MSRV 兼容例外。Web 发布构建与 release 链接证据待收口。RustSec 基线 rustls 四条
公告升级至 0.23.46 后归零；`just dependency-audit dependency-unused` 通过；cargo-audit 0.22.2 扫描 536 条锁记录/1329 条公告（库提交
`edb58f5974b83eb8302332821cb6d389777aa351`），原始锁图、公告 JSON 与日志留在忽略的 `target/dependency-review-20261015/`。

无行为/性能测试、服务、迁移或部署。

### 2026-10-14 Tokio LTS 与 OTel 配套升级
基于 `3511a41` 工作树升级 Tokio 1.51.6 → 1.53.3（声明 `~1.53.3`，仅收 LTS 补丁，上游支持期至 2027 年 9 月）、OpenTelemetry API/SDK/HTTP/OTLP/proto
到 0.33.1、tracing-opentelemetry 到 0.34.0；仅七个包变化，活跃节点仍 486、锁图仍 535（外部包 515）。observability 唯一适配为
`RetryPolicy::disabled()`，拒绝新增默认三次重试，保留每批一次导出与失败计数；有界队列、flush/shutdown、远端 context 与 Prometheus 路径不变，保留 64
MiB 请求体保护（[兼容边界](crates/auction-observability/README.md#实现布局与-tracing)）。

Rust 1.98.0 下 observability 独立 `--locked --all-targets` check 与 Clippy（`-D warnings`）通过，`just verify`（49 份文档）与 MSRV 1.94.1 同范围 check
通过，修改路径主动 LSP 无诊断；按 10-14 口径扫描 535 条锁记录退出 0、漏洞 0、警告 0。未修复 SDK 隐式 headers 环境覆盖，不能确认 TOML-only 边界；
未运行行为测试、真实 OTLP 导出、scrape、故障验证、数据库操作、部署或 payload 重建。

### 2026-10-14 框架补丁与指标依赖精简
在既有工作树上定向升级 Tokio 1.51.4 → 1.51.6、reqwest 0.13.4 → 0.13.5、Clap 4.6.6 → 4.6.7（clap_builder/clap_derive 随之 4.6.7），保留原 HTTP/TLS、
运行时与 CLI feature，未切 Tokio 支持分支、未升级 OTel 或引入新框架，锁图经 `cargo metadata --locked --all-features` 校验。Prometheus 0.14.0 关闭默认
`protobuf`、保留 `process` 并改用 plain_model，[指标与追踪边界](docs/design/tech-stack.md#指标与追踪)不变；`protobuf`/`protobuf-support` 3.7.2 退出
活跃图与锁图，reqwest 新增间接 `base64 0.23.1`，活跃节点 487 → 486、锁图 536 → 535（外部包 516 → 515），未测量体积或性能收益。

Rust 1.98.0 下 observability 独立 check/Clippy 通过，最终锁图下 `just verify`（49 份文档）与 MSRV 1.94.1 同范围 check 通过，535 条锁记录审计退出 0、
漏洞 0、警告 0。未运行行为测试、实际 scrape、逐字节输出比较、数据库或外部服务访问、payload 重建与部署。

### 2026-10-14 低风险依赖精简
承接下节升级，在 `3511a41` 工作树实施[五项精简](docs/design/tech-stack.md#2026-10-14-精简审计候选)（scraper、Axum、observability 直接 serde、Admin
futures-util 与 Gloo feature），HTTP/事件字段、配置、迁移、轮询与卸载逻辑不变。受控刷新锁图后活跃图去重 491 → 487，退出 `getopts`、
`unicode-width`、`axum-macros`、`serde_urlencoded`（前三者同时移出锁图，锁图 539 → 536，外部包 519 → 516）；`serde_urlencoded` 仍被未启用的 Dioxus
fullstack 等路径锁定，不能手删。

Rust 1.98.0 下 observability/integrations/http 各自 `--locked --all-targets` check、Shared/Admin/Public 各自 wasm32 `--locked` check 通过，反向 feature
图确认无合并构建补项；`just verify`（49 份文档）与 MSRV 1.94.1 同范围 check 通过，536 条锁记录审计退出 0、漏洞 0、警告 0。未运行行为测试、未重建
payload、未访问业务数据库或外部服务、未部署。

### 2026-10-14 依赖升级与精简审计
基线 `3511a41`：scraper 0.24.0 → 0.27.0（selectors 0.38.0 改用 rustc-hash）、gloo-net 0.6.0 → 0.7.0、gloo-timers 0.3.0 → 0.4.0，未改 HTTP/事件字段、
配置、迁移或业务规则，共享 GET 保留精确 URL、10 秒预算与取消守卫；锁图 543 → 539 包（外部包 523 → 519），fxhash 移除，未启用路径仍锁旧 Gloo。

Rust 1.98.0 下 `just verify`（49 份文档）、全目标/全特性 check 与 MSRV 1.94.1 同范围 check 通过，7 个改动路径 LSP 无诊断；按 10-14 口径扫描 539 条锁
记录退出 0、漏洞 0、警告 0。另完成两项只读审计与补审形成[六项候选](docs/design/tech-stack.md#2026-10-14-精简审计候选)，仅登记未当删除授权；未运行
行为测试、未调用真实服务或数据库、未重建 payload、未部署。

### 2026-10-14 审计修复
基线 `c9a2d1e`，按静态审计修复 A01–A18，仅将 `quinn-proto` 0.11.17 → 0.11.18，保留既有玩家指南与文档改动，未改历史迁移、业务表结构、HTTP/事件字段
或运行配置。Rust 1.98.0 下 `just verify`（49 份文档）、全目标/全特性 check、MSRV 1.94.1 同范围 check 与 `git diff --check` 通过；
`cargo audit --deny yanked --deny unsound`（0.22.2）扫描 543 项依赖/1310 条公告退出 0，无漏洞/撤回/unsound，保留 `fxhash` 停维警告。

未运行行为测试、未访问业务数据库或外部服务、未迁移或部署；已有异常 Draft、旧配置、失败 recovery 与邮件事实不自动回填。

### 文档整理记录（本轮修复前）
仅更新 48 份正式文档：Rust 1.98.0 下 `just docs-check`（48 份文档）、`just fmt repository-policy-check layout-check config-example-check i18n-check` 与
`git diff --check` 通过，并静态比对 27 组表格列宽与源码调用一致、各组总和 100。未改实现、配置或迁移，未重编、运行行为测试或操作服务，没有重跑完整
`just verify`；以下日期保留原有范围与结果，不表示本次重跑。

### 2026-10-13
| 既有工作 | 当时结果与限制 |
| --- | --- |
| 文档维护机制调整 | 移除独立笔记目录和模板，保留通用文档门禁。`just docs-check` 覆盖 48 份文档；格式、策略、布局、配置、i18n、差异检查通过；文档工具以 `rustc --edition=2024 -D warnings` 编译，30 个路径 LSP 无诊断。提交前完整 `just verify` 通过，仅编译既有测试目标 |
| 邮件装备取时修复 | application 全部目标 check/Clippy、Worker `postgres,cache-redis` check、Worker 全特性 Clippy、格式/差异/策略/布局/i18n 通过；改动源码和六份文档 LSP 无诊断。 Worker 单 PostgreSQL Clippy 被未改动 `admin_mail/backfill.rs:37` 的 `infallible_destructuring_match` 阻塞；当时 docs-check 因历史链接失效失败 |
| 线上规则只读排查 | Public 规则 GET 的 `zh-CN/en` 均为 `503 / rules_unavailable`，所选及默认英文正文尚未发布；只读日志和 GET，未登录、发布、改配置或操作数据库 |

取时修复用每件装备读取后的注入时钟校验 `collected_at`，不放宽未来时间门禁，不改命令时间/摘要或旧邮件事实。当时未重建部署、重启服务或重放邮件，规则正文仍需操作者确认后发布；该日期的文档/源码修改均未执行行为测试。

### 2026-10-12
以下源码静态记录使用 Rust/Cargo `1.98.0`；三轮的 docs-check 当时均因历史链接失效失败，未恢复用户删除。

| 既有工作 | 当时静态结果与限制 |
| --- | --- |
| Admin 拒收说明 | 五语言显示持久诊断与原始码；Admin 原生/WASM check 与 Clippy、格式/差异/i18n/布局/策略通过，4 个 Rust 文件 LSP 干净；CSS 保留 9 项既有 `!important` 警告。仅改善说明，取时缺陷到 10-13 才修复 |
| A44 Cookie 只读校验 | 工作区全部目标、Admin PostgreSQL + Web、独立两后端 storage、双 WASM、全特性 Clippy 和格式/差异/策略/布局/配置/i18n 通过。 29 个 Rust 文件 LSP 未报诊断，其中 18 个确认干净、11 个未确认；未用真实 Cookie 调用接口 |
| 两份 AMD Cargo 候选 | CPU 白名单经 `rustc --print cfg` 核对，EPYC 不含 `rdseed`；两配置分别通过四程序的 `--locked --release` check，显式 PostgreSQL/Redis/Web。 TOML LSP、格式/差异/策略/布局/配置/i18n 通过；这一阶段尚未链接服务程序 |

独立授权的后续构建、测量和部署证据：

| 范围 | 限定结论 | 证据入口 |
| --- | --- | --- |
| Intel KVM 领域微基准 | 6 组参数 × 10 类操作 × 9 轮；v3/Fat LTO/1 CGU 等权几何平均吞吐为基线 1.158 倍。 AMD 微基准仅编译，不能外推目标收益 | [测量记录](ops/build/README.md#已有测量和待补证据) |
| Ryzen 7950X 四程序与双 Web | Release 链接、CPU/LTO/CGU、内嵌隔离、ELF、源码/页面身份及压缩包 SHA-256 静态核对通过；是 dirty 工作树候选，非正式归档。该构建阶段未运行服务或部署，未生成 EPYC 服务制品 | [重编记录](ops/build/README.md#2026-10-12-ryzen-7950x-重编记录) |
| Ryzen 目标空库初始化与启动 | 路径/权限修正，严格停机、快照校验与预检后在 PostgreSQL 18.6 应用 39 条迁移，最大版本 39、失败 0、64 张 public 表。三角色 ready、重启 0，最近 160 条本次日志无 WARN/ERROR，receipt 和程序哈希匹配 | [目标初始化](ops/build/README.md#2026-10-12-ryzen-目标初始化与启动) |
| 后续 Cookie 候选替换 | 保存旧程序与已校验 custom-format 备份后替换；schema 保持 39，无迁移/恢复/配置修改。三角色就绪、哈希/回执和两端各 6 个静态资源匹配，重启 0，外部 readiness 200 | [候选更新](ops/build/README.md#后续-cookie-校验候选更新) |

这些部署记录不证明网页设置、竞价/并发、外部论坛/邮件、完整浏览器或恢复演练已通过；Cookie 校验接口仍未用真实凭据验证。PostgreSQL p99/吞吐、AMD 现场性能和代表性 PGO 尚无证据。

### 2026-10-11
两轮使用 Rust/Cargo `1.98.0`，格式/差异、策略/布局/配置/i18n、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）及双 WASM 均通过；docs-check 当时失败，未恢复被删除文档或改门禁。

#### 复审修复
另通过工作区 `--all-targets --all-features` check，10 个 Rust 文件 LSP 无诊断；三个 SQL 文件的 8 条查询/绑定链数量静态匹配，未执行 SQL。docs-check 首个错误为 `ci/README.md` 历史链接失效。修改领域、应用和两后端存储，不新增依赖、锁图或迁移；旧错误时间和损坏账本不自动修复。

#### 首轮领域优化
六个 Rust 文件 LSP 无诊断；docs-check 因历史目录缺失及残留链接失败，完整 `just verify` 未通过。仅修改领域及文档，不新增依赖、锁图或迁移。两轮均未运行行为测试、服务、数据库、部署或发布重编；首轮亦未做性能测量，静态结果不证明历史兼容性。

### 2026-10-09
在 `4a04214` 基线修复 5 项问题。Rust/Cargo `1.98.0` 的格式/差异、策略/布局/配置/i18n、工作区和独立 feature、Clippy、双 WASM、`--all-targets --all-features` check，以及 MSRV `1.94.1` 同范围 check 均通过，18 个 Rust 文件 LSP 无诊断。完整 `just verify` 退出 1，停在 `ci/README.md` 历史链接失效；用户确认保留 7 份历史说明的删除，其他门禁独立通过不能改写为完整 verify 通过。未执行依赖公告审计，锁图仅增加 HTTP 到既有 thiserror 2.0.20 的依赖边，未升级版本或改历史 SQL。未运行行为测试、产品、数据库、迁移或浏览器流程。

### 2026-10-03 历史记录
Rust/Cargo `1.98.0` 下 `just verify`（当时 52 份文档）、工作区全部目标/特性 check、MSRV `1.94.1` 同范围 check 及差异检查通过。依赖审计 cargo-audit `0.22.2` 使用 `--deny yanked --deny unsound`，检查 543 个条目，无漏洞/撤回/unsound 命中，保留 `fxhash` 停维提示；公告库提交 `f8dee89e1b2f2f1eaf548312df7655fe5202a302`；另发现的 chacha20 0.10.1 撤回版本升至 0.10.2。

SQL 静态扫描覆盖 39 份迁移、737 个 Rust SQL 候选；两份受限执行视图解析通过，686 条可识别直接绑定链未见数量不匹配。历史 SQL 未改，保留字问题由兼容视图处理；8 份 PL/pgSQL 的解析器输出存在限制，不能视为全部函数体/schema/事务验证。LSP 因工具链缺少 `rust-analyzer` 未完成，不以 Cargo 结果替代。未执行行为测试、发布构建、浏览器、服务、数据库或迁移。

### 2026-09-22 历史记录
Rust/Cargo `1.98.0`、just `1.58.0` 下 `just verify`（当时 51 份文档）与差异检查退出 0。修复门禁发现的借用、错误映射和 feature 编译问题后重跑，未执行行为测试、发布、浏览器、服务、数据库或迁移。29 项报告问题、论坛 DTO 附项及三份追加迁移属于该轮；历史流拍 Return、资产冲突、错误归档 Credits 和旧排期来源仍须独立核对，见[邮件资产核对](ops/mail/diagnostics.md#历史资产处置核对)。

## 源码核对发现的实现缺口
以下为历史源码修复及未验收边界。2026-10-14 全仓审计的 18 项（11 项 P1、7 项 P2）已在源码范围修复，不等于行为验收：

| 审计项 | 修复 owner 与范围 |
| --- | --- |
| A01/A02 · 自动 Draft 与后继经济模板 | [生命周期与存储](crates/auction-storage/README.md#持久调度与慢速代理)：两后端不提前写运行期结束时间，新后继冻结有效模板 |
| A03/A04 · 退款确认与 Recovery 人工重试 | [Recovery 契约](docs/contracts/routes.md#recovery-与实时运维)：对齐 ready 义务，匹配的父子投影与 delivery 重试原子恢复 |
| A05/A09/A10 · runtime 与首次凭据 | [配置](ops/config/README.md#runtime-共享与重启)：恢复错误阻断启动，复用会话校验，Unix runtime 校验有效 UID |
| A06/A07 · 论坛越序与邮件成功前缀 | [Worker 编排](crates/auction-jobs/README.md#生命周期慢速代理与结算)：持久 pending 先挡后续楼层，失败后缀不丢完成前缀 |
| A08 · Lite 时间文本比较 | [存储](crates/auction-storage/README.md)：领取/开拍使用时间值，兼容存量 RFC3339 与空格格式 |
| A11/A14 · 未知结果重试与时间精度 | [Admin 交互](crates/auction-web-admin/DESIGN.md#拍卖录入与结算)：冻结完整请求，先恢复精度再预检 |
| A12/A13 · 录入世界与隐藏来源字段 | [Admin 命令](crates/auction-web-admin/README.md#管理数据与命令)：world 隔离表单，JSON 来源不携带旧 URL |
| A15 · 灾备文件替换 | [灾备安全边界](ops/deploy/RECOVERY.md#安全边界)：校验属主与完整受信目录链 |
| A16/A17/A18 · 空库与恢复后验 | [存储迁移](crates/auction-storage/README.md#迁移)：共用完整对象盘点，subscription 限当前库，旧迁移也核对完整前缀与 checksum |

旧版本提前物化的 Draft、缺失的邮件完成证明、未闭合 recovery 和不一致 runtime 仍须独立确认，本轮未清理、重放或改写历史现场；玩家提示与 systemd 停止预算缺口保留如下。

[中文玩家指南](docs/guide/player-guide.md)核对发现两项提示偏差：[Public 中文登录占位文案](crates/auction-web-public/i18n/zh-CN.ftl)仍称“32 位 Key”，而[邮件 Key 协议](crates/auction-application/src/ports/mail/user/security.rs)实际使用 43 字符 canonical Base64URL；[论坛主题正文](crates/auction-integrations/src/forum/render.rs)把 notify 描述为反超提醒，但[通知 owner](crates/auction-jobs/src/worker/forum/reply/notification.rs)仅入队单次普通 MoogleMail 通知、论坛 PM 未接入。仅在指南说明并登记，未改译文、渲染或通知逻辑，未取得真实邮件或浏览器证据。

[分体总控 unit](ops/deploy/systemd/auction-stack.service.example)的 `TimeoutStopSec=120` 小于三角色依次各用满 45 秒的 135 秒，顺序停止可能外层先超时；部署前须重新核算，见[systemd 手册](ops/README.md#systemd-服务与顺序控制)。只登记缺口，未改模板或取得停止流程证据。

2026-10-12 装备拒收排查发现邮件开始时的 `now` 早于读取后生成的 `collected_at`，被记为 `snapshot_invalid / collected_at_future`；2026-10-13 的[拍卖入站修复](crates/auction-application/src/services/mail/processing/processor/auction.rs)改为每件装备读取后重新取时钟，规则与幂等字段不变。仅完成源码与静态验证，尚未部署或取得接收证据；关联退回/通知任务须独立核对，不盲目重放、不改历史诊断。

2026-10-11 代理变更报错后的半修改状态及竞价事实、代理转换链与最终快照的内部不一致校验缺口：三条自动反超路径已合并为同一计算，未改拍卖机制或持久化布局，版本溢出、非法终止原因、历史局部重放与同价边界仍缺运行证据；同日复审的 4 项 P2 与 1 项 P3 已在源码范围修复：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| P2 · 合法慢速代理被账户投影判为损坏 | [账户能力](crates/auction-storage/src/database/account/mapping/proxy.rs) 区分授权合法性与可降额下限，不足完整下一口返回 `can_lower=false` | 账户列表/详情与代理反超仍缺行为验证 |
| P2 · 代理历史与命令结果使用两套时间 | 两后端 `bidding/proxy.rs` 显式绑定同一锁后 `occurred_at`，覆盖 Public、slow due 与 PostgreSQL Final 计划 | 并发/精度需数据库验证；旧记录不回填、不重写 |
| P2 · 已截止候选占满慢速扫描批次 | [到期扫描](crates/auction-storage/src/database/slow_proxy_due.rs) 在 LIMIT 前排除已截止 lot，执行端仍锁后复核 | 未验证 Final 失败及跨截止竞态；不保证任意坏数据下的公平性 |
| P2 · `accepted` 重放缺少请求绑定 | [Public 防腐层](crates/auction-application/src/services/auction/competition/legacy_ledger.rs) 核对请求/首条事实；[慢速重放](crates/auction-application/src/services/auction/competition/slow_replay.rs) 核对命令/lot/授权/generation 与结果形状 | 合法旧载荷及损坏载荷仍需运行证据，不重算历史金额 |
| P3 · 拍卖聚合溢出后部分修改 | [拍卖聚合](crates/auction-domain/src/auction/mod.rs) 校验版本后才发布状态、时间和元信息 | 版本极值仍缺行为验证 |

2026-10-09 审计的 1 项 P1 与 4 项 P2 已在源码范围修复；不宣称全项目不存在其他缺陷：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| P1 · 普通实时连接无活跃上限 | [Hub 配额](crates/auction-http/src/http/shared/realtime_hub/connections.rs) 每进程 1024、每用户 8，RAII 释放；发现流独立 128 槽保留 | 多实例不共享配额，容量和释放路径仍缺运行证据 |
| P2 · 既有 WebSocket 不消费会话撤销 | [只读检查](crates/auction-http/src/http/public/realtime/session.rs) 首次推送前与周期内查两后端权威会话，不续期，失效/读取失败关闭 | 非同步踢线；注销、封禁、换钥、到期和依赖失败仍须运行验证 |
| P2 · 健康空闲连接误断线 | [浏览器订阅](crates/auction-web-public/src/api/auction/realtime.rs) 只限握手 10 秒，移除业务帧空闲看门狗 | 仍需静默连接、网络黑洞和重连恢复证据 |
| P2 · 公开拍卖空页总数为 0 | [拍卖读取](crates/auction-storage/src/database/auctions/) 同一读取快照独立统计总数，cursor 不统计 | 并发翻页和越界页未作行为验证 |
| P2 · Lite 开拍展示按时间文本比较 | 拍卖当前/实时预检和 lot 列表/详情四处 SQL 改用 `julianday` | 历史两种格式、偏移与边界时刻未作数据库验证 |

此前 2026-10-03 的 7 项问题保留以下历史源码修复记录；其中两项初始化缺口沿用更早登记，不重算为首次发现：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| F1 · Admin 凭据轮换后旧会话仍有效 | [session 服务](crates/auction-application/src/services/sessions/mod.rs) 异步启动先永久撤销旧会话，HMAC 绑定 actor/secret，读取复核 actor；两后端共用既有表 | 重启需重新登录；凭据切换、恢复旧配置、多实例维护仍缺运行证据 |
| F2 · 首次初始化绕过迁移兼容层 | [initialization](crates/auction-storage/src/database/initialization/mod.rs) 在既有迁移锁内复用受限执行视图，原 SQL/checksum 不变 | 仍须独立验证空库迁移、失败现场与三角色启动 |
| F3/F6 · rustls/h2 漏洞公告 | [锁图](Cargo.lock) 为 rustls 0.23.45、h2 0.4.19；rustls 下限同步，CI 新增依赖审计 | 版本范围修复不等于生产可达性或未知漏洞证明 |
| F4 · 设置慢请求与无界退出 | [access](crates/auction-setup/src/server/access.rs) 限正文/异步处理 10 秒，令牌先于正文；[lifecycle](crates/auction-setup/src/server/lifecycle.rs) 复用 5 秒请求头与 10 秒排空；终止不继续初始化 | 未执行慢请求、断连、信号或阻塞文件系统运行验证 |
| F5/F7 · 设置防嵌入及拒绝响应安全头 | 正式表面与设置复用 [headers](crates/auction-http/src/http/security/headers.rs)，包括拒绝、框架错误及超时应用响应 | 未执行浏览器/代理验证 |

`fxhash 0.2.1` 的 RUSTSEC-2025-0057 维护债已由 scraper 升级移除；历史命中不是已确认生产漏洞，CI 不隐藏公告（见[技术栈](docs/design/tech-stack.md#版本与升级)）。初始化失败应保留配置与数据库现场，按[合并部署维护](apps/auction-server/README.md#生命周期与维护)显式处理，不得删除配置、改历史迁移或自动重试绕过。

## 仍需运行证据的事项
- PostgreSQL 并发竞价、资格变化、序列化冲突、故障重启、损坏事实与恢复边界。
- 真实 MoogleMail/E-Hentai 的认证、未知写入结果、Sent/Balance 证明与人工恢复。
- Bot 联合身份/正向库存（含 Isekai）、真实资金对账、generation 换主/提交未知、全现金保护与大库冲突锁成本。
- Public/Admin 完整页面、五语言、认证组合、Admin 重启/凭据轮换及旧配置恢复、严格 decoder、实时帧和断线恢复。
- Public 连接总量/用户配额、槽位释放、会话只读复核与撤销延迟、静默连接及分页快照。
- 非空生产数据、目标主机权限与服务编排、制品切换、备份恢复和回执保管。
- 首次设置的本机/HTTPS 代理门禁、未知保存结果与空库初始化；合并部署的 readiness、信号、父进程强杀、迁移失败和升级。

后续获得授权开展行为验证时，应登记制品/源码身份、后端与版本、脱敏环境、时间、输入、预期与实际结果、证据保存位置和限制。敏感证据保存在受控目录，项目文档只记录安全引用和结论。静态验证、页面样式完成、空表或路由存在均不足以关闭上述事项。

## 未登记为现有能力
Public 当前实时通道是 WebSocket 加 HTTP Catch-up，未注册 SSE endpoint；外部 realtime 广播 sink、retention 数据删除/归档 mutation、自动 OpenAPI 生成也不属于当前交付能力。要新增这些能力，需要独立契约、owner、授权边界及明确的设计与运维说明。
