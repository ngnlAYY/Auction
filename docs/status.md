# 实现与验证状态
[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [默认验证范围](docs/guide/development.md#默认检查)

本页登记源码能力、历史验证与待补证据。隔离验证声明、定向构建及限定部署不足以证明完整业务或生产验收；文档修订不改变历史结果。

## 本页导航
- [能力范围](#能力范围) - [证据口径](#证据口径) - [本轮静态检查](#本轮静态检查)
- [源码核对发现的实现缺口](#源码核对发现的实现缺口) - [仍需运行证据的事项](#仍需运行证据的事项) - [未登记为现有能力](#未登记为现有能力)

## 能力范围
| 范围 | 源码中的实现边界 | 不能据此推导 |
| --- | --- | --- |
| HTTP 与页面 | Public/Admin feature API 及浏览器入口见[路由口径](docs/contracts/routes.md#基线与统计口径) | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| 迁移 | PostgreSQL 65 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
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
各记录只覆盖注明的工作树与验证范围，不代表运行验收。下文 `verify全绿` 沿用记录当时的完整 `just verify` 结果；历史文档份数不是当前自动检查项，也不证明链接或语义正确。当前静态门禁及独立检查范围见[开发指南](docs/guide/development.md#默认检查)，历史命令和失败结果保持原样。

2026-10-14 四轮依赖调整的公告审计共用 cargo-audit 0.22.2、`cargo audit --deny yanked --deny unsound --deny unmaintained`、1310 条公告及公告库提交 `6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`；“审计修复”的不同策略单列。

### 2026-10-28 发布脚本精简与本机编译加速

`build-identity.sh` 收拢 Web payload 完整门禁、surface 元数据与 `sha256_file`，`web-bundle.sh` 的新建/复用与
`release.sh` 的制品核对共用同一判定；`release.sh` 并入原 `release-single.sh`，标记断言与装包清单改为表驱动，
子命令参数统一校验，命令行接口不变。发布与 Web 入口只接受环境变量启用的受控 sccache，用户全局
`build.rustc-wrapper` 由空 `RUSTC_WRAPPER` 覆盖。本机另按用户要求写入 `~/.cargo/config.toml`（sccache + clang
`--ld-path` mold），只改变编译速度，不改变 profile 或 CPU 基线。

`bash -n`、ShellCheck 与 repository/layout/config/i18n 门禁通过。`release-check --allow-dirty` 端到端完成 API-only、
双端 Web、嵌入四程序、资源隔离与确定性装包；首次运行因构建期间修改了 `ci/tools/README.md` 被输入复核拒绝，
未改输入重跑后通过。二次运行 Web 与两档原生制品均命中复用，新包文件清单及权限与 R34 正式包一致，发布二进制由
mold 链接、未经系统 sccache。验证包保留在 `target/dist/package.refactor-verify-20261028/`，`target/dist/package/`
已还原为 R34 包（SHA-256 `c71fb549…b944`）。未运行行为测试、服务或部署。

### 2026-10-28 赛季早鸟期排期与收件品质

按用户要求为异世界赛季更新新增可配置早鸟期，默认关闭；启用后默认每年 `05-01`、`11-01` UTC 09:00 起 7 天，窗口第
1/3/5/7 天 16:00 预览、18:00 开拍、竞价 24 小时。与覆盖期重叠的常规周场由早鸟场次替代，结束后按周计划恢复；窗口内收件
时间的投稿改用独立品质列表（默认不限品质）；首场早鸟草稿建立后到赛季开始前暂停该 world 收件轮询，赛季开始后恢复。
owner 为 application `ports/scheduling/{early_bird,calendar}.rs`、生命周期后继与邮件筛选，storage scheduler/lifecycle 及追加迁移
`000065_early_bird_schedule_slots`，jobs bootstrap 与邮箱门禁，config/runtime 投影和 Admin 运行参数页五语字段；
语义见[早鸟期配置](ops/config/README.md#赛季早鸟期)，升级门禁见[早鸟期运维](ops/README.md#赛季早鸟期)。

`cargo fmt --all` 后 `bash ci/checks/check.sh all fmt check feature lint wasm` 与
`bash ci/checks/check.sh repository layout config i18n` 均退出 0。未新增或运行行为测试，未在任何数据库执行 `000065`，
未启动服务、验证页面、真实邮箱或赛季时间线。迁移按 PostgreSQL 默认名删除 `000003` 的
`scheduled_rounds_world_id_local_week_key`，执行前须在隔离库核对约束名；同 world 单场活动仍由既有
`auctions_one_active_world_idx` 保证，前一场未结算时下一场预览按退避重试。

### 2026-10-28 Blaze 本地开发试验候选

按用户要求复用本机已安装的 `blaze 0.20261027.0 (49ca152)` 与 `blaze-2026-10-27`，新增
`ci/tools/blaze.sh` / `just blaze` 显式入口。build/test 使用 `--locked`，daemon、函数缓存和编译输出隔离于
`target/blaze/`；普通 Cargo 未全局接管，默认 verify、固定 Rust/MSRV、Web 与正式发布依赖保持不变。
工具 owner 为 `ci/tools`，使用边界见[开发指南](docs/guide/development.md#blaze-本地试验)。

实际核对结果：

| 范围 | 结果 |
| --- | --- |
| `auction-domain` 既有两个领域测试 | 编译阶段退出 101，测试未运行；libtest 引用的 `__RUST_STD_INTERNAL_VAL` 缺少 rustc 符号 |
| `auctionctl` 默认 feature 原生编译 | 11 个可达函数因 `__RUST_STD_INTERNAL_VAL` / `MIN` 可写静态量缺少符号而被拒绝，退出 101，无新 CLI 产物 |
| 独立临时程序调用真实 `BidAmount` | 首次编译 15.32 秒，正数、信用网格、历史非整千金额、上限及价格封顶断言通过 |
| 修改临时程序函数后重新构建 | Blaze 报告 0.15 秒；worker 编译 1 个函数、复用 35 个，实际输出从 30000 变为 470000，断言继续通过 |
| 固定 Rust 1.99.0 对照 | 最终临时程序构建报告 0.14 秒，输出同为 470000，断言通过 |

上述小程序只证明受支持路径的增量替换和结果；没有证明 Auction 完整后端兼容或实际加速收益。第一次临时源码放在
`target/` 下，Blaze 报告增量成功但运行旧结果；按该版本 `WatchedFile::immutable` 源码确认，路径包含 `target`
的普通源码被排除监视。该次耗时作废，移到独立临时目录后才取得上表有效证据。

`bash -n`、ShellCheck 及 repository/layout/config/i18n/equipment-contract/fmt 静态门禁通过；
`just verify` dry-run 仍解析到原检查命令，release 参数在实验入口被拒绝并返回 2。
本轮未修改业务 Rust、依赖锁、运行配置或数据库，没有启动 Public/Admin/Worker、访问外部业务服务或验证 Dioxus 页面。
未以 Blaze 替代完整 `just verify`，也未重新运行全工作区编译或既有行为测试；两项既有领域测试仅尝试构建且被阻断。
试验 daemon 已停止，临时源码和诊断程序已删除；编译器身份、原始 worker 日志、最终诊断源码文本及回执保留在
`target/blaze/evidence-20261028.json`。当前候选受上述编译器兼容问题阻断，不宣称可用于完整后端日常快速编译。

### 2026-10-28 云端 R34 发布与受控迁移

在下方邮件统一起拍价与 Public 控件修复完成后，按用户授权将干净提交
`5c99692225ed0ffa0dcdcf6054a2f68574a08d6f` 发布到云端 R34。`auction-application` 的定价解析、
`auction-web-public` 页面及共享分页样式随同版 Public、Admin、Worker、auctionctl 四程序上线。
`CARGO_BUILD_JOBS=2 just release-check` 通过，包含 API-only/embedded 隔离、资源边界及正式封包门禁；
发布包 SHA-256 为 `c71fb549319bb5940dbf624b7104ca7306e7efe99bc32a5bd979ff8c7245b944`。

切换在 12:24 UTC 完成。先保留旧程序、配置和 systemd 定义，确认 Worker 正常 drain，
再由旧 CLI 导出数据库恢复点、新 CLI 离线验证。三角色停止时实际执行新 `auctionctl migrate`，
返回成功；目标库从 schema 64 保持为 64，待执行及新增迁移均为 0，64 份已发布迁移 SHA384 全部一致。
停机窗口前后 88 张表的完整清单、行数及逐表摘要一致，配置内容与权限、unit、tmpfiles 和 runtime 锁身份保持。
备份 archive/manifest 已另复制到本机私有目录并核对摘要，文件权限 0600、目录 0700。

启动按 Worker → Admin → Public 完成，三项 readiness 均为 200，四程序磁盘及运行进程摘要匹配发布计划。
Public/Admin Unix 源站各 10 项、公网 Public 9 项非 HTML 资源全部返回 200 且 SHA-256 匹配封存包。
首页字节在源站核对；公网 HTML 沿用已记录的 Cloudflare 改写边界，不以其字节差异判为旧制品。
部署后 20 分钟复核保持同一 PID/InvocationID、零重启，启动以来完整日志计数为零 error、零 warning。

运行后新增 3 条服务心跳；其余变化已对照停机备份作只读字段核对，并将 `timestamptz` 统一到 UTC 后比较：
Admin 按既有启动契约撤销 1 条旧会话；邮件运行 generation 从 81 增至 82；轮询时间/版本、告警与恢复健康时间刷新；
1 条原已重试的履约任务推进 fencing、版本、重试计数和下次尝试时间。上述表无增删行，执行状态、投递绑定、
错误码及余额未变。原 3 条人工核对投递与 3 场拍卖策略摘要保持，不把运行元数据推进描述为数据库全表静止。

本次运行核验覆盖发布、恢复点、迁移、readiness、HTTP 资源及受保护事实；没有额外执行业务或浏览器用例，
没有人为重放旧拒收邮件或重发通知。Admin 重启后需重新登录，Public 已打开页面需刷新以加载新资源。
本次未更新本地运行服务；下方源码阶段尚未部署的历史记录保留。

本机汇总：`target/dist/mail-pricing-ui-20261028-ops-r34/DEPLOYMENT-SUMMARY.json`；
其 `operations/` 保存发布计划、上传/切换/后验/稳定性回执、规范化字段核对及备份复制回执。
云端操作归档为 `backups/mail-pricing-ui-20261028-r34/`，数据库恢复点另置 root 私有目录。

### 2026-10-28 邮件统一起拍价与 Public 控件密度修复

云端只读日志与入站投影确认，单件装备的 `start 30k`、`start 470k`、`start 2M`、`start 2m`
被随信附加的 Attachment 说明扩展为 13/14 个字段，触发 `start_price_invalid / too_many_fields`。
两封最新原始加密留证已离线认证并与入站内容摘要一致；云端 R33 与修复前源码使用相同的定价分支。

- `auction-application` 的统一定价分支只提取独立 start/金额指令，仍扫描全部明确指令，拒绝重复、混合、
  畸形关键字及非法金额。逐件 URL 的附件 ID/key 校验保持；原正文、内容摘要、已完成邮件决定、退件和通知任务不改写。
- `auction-web-public` 的补偿表复用历史表容器与表头，展开按钮复用 `public-history-group-toggle`；
  展开列由 8% 改为 4%、两侧各 2px，释放的宽度分给金额列。桌面按钮 24px、行高由 61px 降为 41px，手机保持 44px 触控区域。
  内嵌结算计算式及对应样式、五语键移除，真实流水和独立分页保持；余额卡按内容收紧，实测约 229×111px → 202×89px。
- Public 等待拍卖等连接状态保持单行省略，状态圆点、完整无障碍文本与悬停提示保留；历史成交价搜索、筛选和显示字段
  使用 28px 桌面高度及 13px 字号，手机字段继续为 44px/16px。共享 `ui-pager__select` 桌面实测 98×28px → 90×24px，
  左右内边距 8px → 4px；手机仍为 32px/16px，Public/Admin 共用该规则。

最终 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖策略、格式、工作区已有目标、独立 feature、Clippy 与双端 WASM。
双端正式 Web 构建通过，Public 最终 payload SHA-256 为
`cf397e081281c47dfe1b2fcaea27881df1d0cf47b41739d01417e53e942035cd`，Admin 为
`fd37028cc1bf39a992999c94c43fd2be39aa969aa72dbf4dde99ce540454ddf6`。

临时原生诊断直接包含生产定价模块并使用真实领域/应用类型，36 项均通过：四份合法云端正文得到预期价格，
冒号写法仍拒绝，并覆盖多附件、默认价、CR/LF、Unicode 空白、注释、跨行、URL 身份、重复/混合、零负值、溢出及非整数 Credits。
诊断源码和可执行文件已移除，没有新增永久测试。Chromium 加载本轮实际 WASM/CSS，并读取现有本机 API；
核对 1440/800/375px 补偿布局、展开/收起与流水翻页，320px 五语状态文字均为单行且无横向溢出。
历史成交价的真实只读搜索、桌面/手机字段及分页尺寸也已核对，未替换 API 响应或注入示例业务行。

本轮只修改源码/文档及构建制品，未更新本地或云端运行程序，未重放邮件、发送通知、修改业务数据或执行迁移。
云端旧拒收与通知未知结果仍须保留原事实；源码修复不等于旧邮件已经重新接收。证据与截图位于
`target/operations-evidence/start-price-compensation-fix-20261028/`，原始诊断索引位于
`target/operations-evidence/start-price-diagnosis-20261028/evidence.json`。

### 2026-10-27 结算借用、分页纠偏与工具依赖裁剪

基线为干净的 `7599e9d4`。按八个 owner 切片并行审查全部 20 个工作区包，清单覆盖 1,431 份 Rust 源码、
49 份 Markdown、64 份追加迁移及构建/部署配置；全范围风险扫描后深读关键路径，并由主审复核接口、摘要、
异步交接与依赖图。没有把未确认的候选强行改成新抽象或新业务策略。

| owner | 修改及保护的边界 |
| --- | --- |
| application / storage | 结算写入借用已校验计划，移除完整明细/义务的深复制；履约摘要删除未用参数、纯转发层和不可达空 Sent 分支，原序列化字段顺序、空值省略、重复装备及 Go 转义保持 |
| storage | 最多 1000 项附件由前缀重复扫描改为借用集合判重，预期 O(N²) → O(N)，增加有界 O(N) 引用集合；Take 证明直接比较 JSON 数组，预锁查询不收集丢弃的行，原 SQL、锁序、快照及金额保持 |
| integrations | 修复身份脚本前缀含多字节字符时的非法 UTF-8 切片 panic；结算 payload 用有界 writer 计算紧凑 JSON 字节数，省去整份序列化缓冲，64 KiB 上限及协议拒绝语义不变 |
| jobs | 周计划 bootstrap 复用既有单项总预算，生命周期数据库时间、领取和执行共用绝对截止；回执交接及邮箱版本重试复核同一截止，迟到结果不授权新写入，可能已提交结果仍按冻结身份恢复；固定帖对账按需取得正文所有权 |
| HTTP / setup | Admin 查询字符串直接转移到应用请求，保留返回投影复核；Unix socket 权限操作使用原始路径字节，避免有损 UTF-8 转换指向错误路径；设置策略共享只读 Arc，逐请求来源/令牌门禁保持 |
| config | 无 runtime 覆盖时复用已校验 HTTP 安全配置，避免第二次 Origin/CIDR 解析；备用主配置和实际 runtime 投影仍分别校验，原错误优先级保持 |
| Public Web | 当期标记完整结果以 Rc 共享，只借用当前十行；总数收缩含归零时保留纠偏宿主并传原请求页码，避免数据恢复后跳回旧页 |
| Admin Web | runtime GET 结果绑定刷新世代，只有当前 Ready 快照可装配草稿/编辑器；六类分页组件只接收页码、数量、总数，不复制整页记录，草稿、未决命令与页面身份保持 |
| CI / dependencies | sccache 的 object 关闭未调用的 compression，显式保留 read/std；固定哈希补丁进入配方身份，独立锁移除 ruzstd 与 twox-hash，GNU 活跃图 298 → 296，应用锁及缓存格式不变 |

相关模块 README/DESIGN、架构、配置预算与示例、技术栈和运维说明同步；运维中重复的逐轮精简说明收敛到当前升级流程，
历史验证记录保留。12 个已有测试覆盖不同边界，8 个测试模块与基线逐字一致，保留且未执行。64 份历史迁移、
应用 Cargo.lock 和 21 份应用 Cargo 清单保持；50 项直接外部依赖均有用途，未为减少数量重写协议或强并上游版本。

Rust 1.99.0 下 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖仓库/布局/配置/i18n/装备契约、格式、工作区全部已有目标、
独立 PostgreSQL/Redis feature、Clippy `-D warnings` 及两端 WASM；Rust 1.94.1 下
`CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过。
全部 CI shell 的语法与 ShellCheck warning 门禁、Actions actionlint、`cargo machete` 和 `git diff --check` 通过；
49 份 Markdown 的 1,468 处本地引用已核对路径与标题锚点，修改后的语义另与源码和调用方对照。
五个原生程序及双端 WASM 的实际 normal/build 图无第二数据库驱动、native-tls/OpenSSL、Dioxus fullstack/server，原生图无 Web crate。

四份锁使用独立干净 RustSec 快照 `99670cfc0bc574433034b75876a4b791ccaa2efb`（1,380 条公告），
以受控 cargo-audit 0.22.2 执行 `--deny warnings`。应用及 cargo-audit 零漏洞/警告；最终 sccache 完整锁
由 5 项漏洞匹配降至 4 项，仍有 9 项警告；dx 仍为 13 项漏洞匹配、11 项警告。两份工具锁审计仍失败，
未设置忽略项；受限调用路径、补丁及最终 audit/release 工具构建与回执证据见
[工具 owner](ci/tools/README.md#object-压缩依赖裁剪)。

本轮仅源码审计、静态验证和构建工具重建，不运行行为测试、数据库、浏览器、真实外部服务或性能基准，
不构建应用发布制品、不迁移或部署。复杂度、复制和分配变化来自源码推导，实际吞吐、延迟和内存收益尚未测量；
SQL 行锁/取消、Worker 迟到交接及浏览器草稿/分页的运行证据仍需按[默认范围](docs/guide/development.md#默认检查)另行开展。
同版升级边界见[运维说明](ops/README.md#读取快照与实时分发优化升级)，未将上述修复描述为生产验收。

### 2026-10-27 批量写入、读取快照与依赖收敛

基线为干净的 `49e0370c`。按 owner 并行盘点全部 20 个工作区包、1,431 份 Rust 源码、49 份 Markdown、
64 份追加迁移及构建配置，结合风险模式扫描、CodeGraph 调用链和主审交叉复核落实以下修改。

| owner | 修改与保护的边界 |
| --- | --- |
| domain / application | 正金额加法直接使用 checked_add；删除闭集枚举恒假检查、合并相同返还分支。结算义务生成和校验共用逐条构造流程，省去第二份完整列表，保持顺序、自然键、全字段比对与生成错误优先级 |
| storage | 结算明细 INSERT 调用数由 N 降至 ⌈N/256⌉；告警批次条目由最多 64 次降至 1 次。仍共用原事务、校验、锁和追加事实顺序；活动代理部分唯一索引允许移除冗余 DISTINCT |
| storage | Admin 拍卖、lot、审计、改名、代理历史、结算目录及 recovery 报告复用 Repeatable Read 只读快照，计数、页和关联投影同源；关键写入保留 Serializable |
| HTTP / Worker | 查询键借用路由允许列表；限流满表清理和最久未访问项选择合并为一次扫描，原解码、判重与确定性淘汰规则保持。Worker 用条件导入替代两份纯转发异步包装，精简资源说明注释 |
| integrations / jobs | 邮件 JSON 文本共用借用解码，保留整页脚本及协议校验；PM 复用 drain 等待门禁，原抖动精度和范围保持。UDS control 显式关闭环境代理及传输自动重试 |
| Web / contracts | Public 三状态目录最多并发三路读取，标记索引借用 ID，账号续期不再取消同身份标记读取；Admin 配置判重降为预期 O(N) 或 O(N log N)，删除四个等价邮件 DTO 镜像，保留原字段及值域校验 |
| CI / dependencies | 发布缓存增加固定文件白名单的 SHA-256 完整性校验；仓库策略纳入 Cargo.lock。cargo-audit 关闭未使用的 binary-scanning，GNU 活跃图 302 → 293；sccache 通过固定哈希补丁及独立锁升级 Zstandard 三项，缓存恢复只保留普通 rwx 权限位；补丁纳入原有构建配方身份 |

主审复核撤回了会跳过原解析校验或改变 DOM 移除观察语义的候选。配置、首次设置、进程原语、可观测性及缓存也已
纳入盘点和重点审读，未确认需要修改的实现保留。12 个已有测试覆盖不同边界，8 个测试模块与基线逐字一致；本轮保留且不执行。
应用的 50 项直接外部依赖均有用途，应用 Cargo.lock 与全部历史迁移不变；技术栈将重复的近期依赖说明收敛为当前能力摘要。

四份完整锁以独立、干净的 RustSec 快照 `99670cfc0bc574433034b75876a4b791ccaa2efb`（1,380 条公告）执行
`--deny warnings`，不加忽略项。应用与 cargo-audit 零漏洞、零警告；sccache 完整锁由 7 项漏洞匹配/17 项警告降至
5/9，dx 保持 13/11，两份完整工具锁审计仍失败。活跃图、受限 API、剩余上游约束及受控工具构建证据归
[工具 owner](ci/tools/README.md#同日并行审计锁文件扫描与-sccache-zstandard-修复)。

Rust 1.99.0 下 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖仓库/布局/配置/i18n/装备契约、格式、工作区全部已有目标、
独立 PostgreSQL/Redis feature、Clippy `-D warnings` 和双端 WASM；Rust 1.94.1 下
`CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过。
全部 CI shell 的语法/ShellCheck warning 检查、Actions actionlint、`cargo machete` 和 `git diff --check` 通过；
49 份 Markdown 的 1,466 处本地引用已核对路径与标题锚点，修改后的契约另作源码对照。
五个原生程序和双端 WASM 的实际依赖图已核对隔离边界；四份审计锁的摘要与当前文件一致，RustSec 工作树干净且提交匹配。
audit/release 两档工具已从最终脚本、补丁和锁图重建，主审独立核验两档安装回执通过；上游编译警告见工具说明。

本轮只做静态审计、编译与文档核对，不运行行为测试、服务、数据库、浏览器或性能基准，不构建应用发布制品或部署。
SQL 调用数、复杂度与分配变化来自源码分析，实际吞吐和延迟尚未测量；部署继续沿用
[同版升级边界](ops/README.md#读取快照与实时分发优化升级)。

后续候选仍包括结算逐 lot 的锁内快照装配、大历史资金投影聚合、Redis 单值读取预算，以及需要服务端接口配合的
历史场次/标记目录扫描；这些项涉及锁序、历史数据或读取契约，保留现有行为及相应边界。

### 2026-10-27 批量持久化与边界复核

基线为干净的 `155e158d`。按 owner 并行审查 20 个工作区包，清单包含 1,431 份 Rust 文件、49 份 Markdown、
64 份追加迁移；主审交叉复核关键调用链、依赖图、构建脚本及文档契约。以下改动保持现有业务金额、幂等摘要及历史迁移。

| owner | 发现与修改 |
| --- | --- |
| application | 结算事实索引同时记录明细是否消费，省去第二个判重集合；到期校验用最大结束时间省去重复扫描；黑名单中文次数理由识别不再构造临时字符串 |
| storage | 待读邮件按输入顺序批量登记，写入 SQL 往返从 N 次降至 1 次；保留原 claim 锁、身份冲突整批回滚与实时租约复核。纯加锁查询不收集无用行，展示回填借用 JSON |
| integrations / jobs | 邮件可选/必需唯一节点共用最多两个匹配的校验，减少临时节点和字符串集合；Delivery 仅在收件人缺失时构造排除项；领取或任务排队跨过 drain 后不再开始新命令 |
| HTTP / setup | 严格 UUID 和 JSON 媒体类型校验收敛，生命周期响应转移已有字符串；设置服务拒绝重复/不可解码的安全头，非法 Origin 不再按缺失放行，沿用既有错误码 |
| Web | 普通出价响应绑定请求 lot，v1/v2 拍卖列表绑定请求页码和数量；属性只校验一次。词典借用排序判重由最坏 `O(N²·L)` 降至 `O(N log N·L)`；焦点循环的 Rust 额外句柄存储由 `O(N)` 降至 `O(1)` |
| config / apps | loader 提前拒绝初始化解析器不接受的日志过滤表达式，错误脱敏；Admin 空状态、Worker 排空共用装配/错误映射，邮件留证日志少持有一份元数据副本 |
| CI | 发布指纹补齐 Cargo 编译器/包装器别名和增量覆盖；sccache 别名绑定受控工具，工具构建清除这些应用环境覆盖，防止错误复用或污染独立配方 |

未确认需要修改的 domain、contracts、cache、observability 及历史迁移保留原实现；process 仅更正过期模块注释。
12 个已有测试覆盖不同契约，未发现可直接删除的完全重复项，保留且不运行。50 项直接外部依赖均有用途，
`cargo machete` 未发现未使用项；本轮不新增程序库，四份锁图及依赖版本保持不变。

应用及三个工具锁共用独立、干净的 RustSec 快照 `99670cfc0bc574433034b75876a4b791ccaa2efb`（1,380 条公告），
以受控 cargo-audit 0.22.2 执行 `--deny warnings`：应用 535 个条目与 cargo-audit 锁均零漏洞、零警告；
dx 为 13 项漏洞匹配、11 项警告，sccache 为 7 项、17 项，完整工具锁审计仍失败，未设置忽略项。
工具配方修改后已重建两档工具并通过安装回执校验，版本及受控调用范围见[工具说明](ci/tools/README.md#上游公告与实际使用范围)。

Rust 1.99.0 下最终 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖策略/布局/配置/i18n/装备契约、格式、工作区全部已有目标、
独立 PostgreSQL/Redis feature、Clippy `-D warnings` 和双端 WASM；Rust 1.94.1 下
`CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过。
全部 CI shell 的语法/ShellCheck warning 检查、Actions actionlint、`cargo machete` 与 `git diff --check` 通过。
五个原生程序和双端 WASM 的实际 normal/build 依赖图无第二数据库驱动、native-tls/OpenSSL、Dioxus fullstack/server，
原生图无 Web crate。49 份 Markdown 的 1,464 处本地引用已核对路径与标题锚点，相关契约另作源码对照。

本轮不运行行为测试、服务、数据库/浏览器验证或性能基准，不构建应用发布制品或部署。复杂度、SQL 次数和分配变化
来自源码分析；批量取号、冲突/取消与 drain 交接的运行验证仍按[默认范围](docs/guide/development.md#默认检查)另行授权。
同版升级前提见[运维说明](ops/README.md#读取快照与实时分发优化升级)。

评估后保留的候选：结算快照仍逐 lot 加载竞价状态和锁定有效代理，批量化须守住代理锁序与异常数据的错误顺序；
HTTP 总连接配额与 HTTP/2 头部预算需要独立容量策略；预览目录逐行分类绑定需要明确历史可选字段兼容。
这些候选及上一轮排序索引建议不通过任意默认值、修改历史数据或放宽校验直接实现。

### 2026-10-27 全仓精简与资源生命周期复核

基线为干净的 `c1781d68`。经用户授权按 owner 并行审查全部 20 个工作区包，建立 1,430 份 Rust 源码、49 份 Markdown、
64 份追加迁移及构建/配置清单，结合全仓静态扫描和 CodeGraph 对重点调用链复核。各模块的修改、契约及检查结果如下。

| owner | 本轮修改与保护的边界 |
| --- | --- |
| domain / application | 结算纳秒时窗全场计算一次；内部证据摘要直接消费借用字段，Sent 不再复制装备 ID 列表；金额公式、事实校验顺序、摘要字段顺序及字节编码保持 |
| storage | Recovery 重试只捕获身份、版本、fence 与固定摘要；退件证据借用 JSON 解码，八处摘要绑定不再分配临时 Vec；Serializable、锁序、幂等比较及原子写入集合保持 |
| storage | Admin realtime 元数据的无副作用 CTE 使用 `NOT MATERIALIZED`，避免双引用强制物化全部匹配行；页与计数仍在同一语句快照，空页 total 和安全投影保持 |
| integrations / jobs | 邮件唯一节点校验最多读取两个匹配，行列索引保留协议上限及超限哨兵；分页 query 使用四个字段槽位，delivery 借用 JSON 解码；外部身份、资产、协议闭集及未知外写保护保持 |
| shared / Admin Web | 消除可见性监听器的内部引用环并增加 RAII 卸载；邮件关联校验由预期 `O(N×M)` 降至 `O(N+M)`，结算目录每行省去四次字符串复制；Markdown 使用借用迭代和固定采样缓冲，删除两层纯转发函数 |
| config / apps | 转移已校验 runtime 的名单、过滤与模板，Admin dashboard 使用借用投影；部署锁通过已打开文件描述符收紧权限，避免再次按路径定位 |
| auctionctl | systemctl 双管道改为非阻塞交替读取，查询最多 5 秒、启停最多 60 秒，异常再用最多 5 秒确认直接子进程回收；readiness 查询共用角色剩余预算，停止失败仍阻断迁移 |
| observability | 初始化时缓存固定业务指标句柄，逐笔记录省去标签哈希、读锁及 Arc 克隆；OTLP 健康包装直接借用自身；指标名称、零值、label、bucket 及导出结果分类保持 |

HTTP、Public、Worker、首次设置、进程原语、契约和迁移均纳入审查；没有确认需要修改的模块保留原实现。根 README、
架构、技术栈、运维与相关模块文档同步，部署仍沿用[同版升级流程](ops/README.md#读取快照与实时分发优化升级)。
12 个已有测试未发现重复或失效，保留且不执行。直接依赖均有实际用途，仅删除 cache 重复声明的 Tokio `sync/time`
feature；生效图和 Cargo.lock 保持不变，不强行合并上游跨版本依赖。

受控 cargo-audit 0.22.2 回执校验通过，独立获取干净 RustSec 快照，仍为提交 `99670cfc0bc574433034b75876a4b791ccaa2efb`
（1,380 条公告）；四份锁在同一快照执行 `--deny warnings`。应用 535 个条目与 cargo-audit 独立锁均为零漏洞、零警告；
dx 仍有 13 项漏洞匹配、11 项警告，sccache 仍有 7 项漏洞匹配、17 项警告，完整锁审计失败，受控调用范围见
[工具公告记录](ci/tools/README.md#上游公告与实际使用范围)。工具锁、公告忽略项及安装回执均未修改。

Rust 1.99.0 下 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖仓库/布局/配置/i18n/装备契约、格式、工作区全部已有目标、
独立 PostgreSQL/Redis feature、Clippy `-D warnings` 及 Public/Admin WASM。`cargo machete`、全部 CI shell 的 ShellCheck
warning 门禁、Actions 的 actionlint 和 `git diff --check` 通过；三角色、auctionctl、合并服务与双端 WASM 的实际依赖图
分别核对，原生图无 Web crate，全部图均未引入第二数据库驱动、native-tls/OpenSSL 或 Dioxus fullstack/server。
Rust 1.94.1 下 `CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过。
49 份 Markdown 的 1,454 处仓库内引用已静态核对，无缺失路径或标题锚点；修改后的契约与 owner 说明另作人工复核。

本轮未运行行为测试、数据库/浏览器验证、性能基准或部署。算法复杂度及分配减少由源码推导，实际吞吐、延迟与内存
收益留待代表性负载量测。[Admin 审计日志](crates/auction-storage/src/database/admin_audit/postgres.rs)按
`(occurred_at DESC, id DESC)` 全局分页、[realtime failures](crates/auction-storage/src/database/realtime_admin/failures.rs)
的 `(failed_at DESC, id DESC)`、[无邮箱筛选入站列表](crates/auction-storage/src/database/admin_mail/reads.rs)
及[全状态 delivery 列表](crates/auction-storage/src/database/delivery_admin/reads.rs)尚缺对应全局时间排序索引。
先以代表性数据对首屏、时间范围、深 OFFSET 与 keyset 取得执行计划，再决定追加迁移及其写入成本；完整计数仍需
独立评估，本轮不改已发布迁移。

### 2026-10-27 最新源码、注释、文档与依赖复审

基线为干净的 `5274cb78`。按 20 个工作区包建立清单，对 1,430 份 Rust 源码进行重复实现、panic、集合扫描和异步任务
站点检查，并沿 CodeGraph 调用关系复核候选；另核对清单/锁图、构建脚本、迁移契约和 49 份 Markdown 的引用。
这是全仓静态扫描与重点调用链审读，运行证据按下文单独记录。

| owner | 本轮改动及边界 |
| --- | --- |
| application | 邮件过滤与标色配置列表改用集合判重，期望复杂度由 `O(N²)` 降为 `O(N)`；规范文本复用领域校验，ASCII 大小写、Unicode 原文及错误边界不变，删除过期空策略注释 |
| domain | 六处规划器触发器不匹配的 `unreachable!` 改为 `InvalidBidState`，正常分派与竞价计算不变 |
| integrations | 六个装备 HTML 固定选择器改为进程内惰性编译并借用；每次仍完整解析页面和校验属性，不缓存正文 |
| storage | 恢复预检从一批完整迁移历史生成摘要，相关 SQL 由三次减为一次，保留完整成功前缀/checksum 门禁与只读快照；竞价日志逐页归一化筛选词，代理降额审计免临时向量 |
| shared / Admin Web | 去标题处理移除中间行向量及正文副本；Admin 提交编号截取直接借用末尾八个 Unicode 字符，原展示和换行处理不变 |
| observability | 关闭 tracing-opentelemetry 未用的默认 metrics 桥接，显式保留 tracing-log；锁图只删除一条 smallvec 依赖边，版本及包数不变 |

HTTP/JSON、配置字段、迁移、金额和状态机契约保持；相关模块 README 与技术栈同步。12 个已有测试分别覆盖领域迁移、
后端 URL、论坛解析/回执/分页/表单及诊断，未发现可直接删除的重复断言，本轮保留且不执行。
直接依赖未发现可安全删除的未使用项；不为去重强制替换 SQLx、Dioxus 等上游的跨版本依赖。OTLP http-proto 仍会传递启用
OTel metrics，不能将局部 feature 精简写成整个 metrics 依赖消失。

Rust 1.99.0 下 `CARGO_BUILD_JOBS=2 just verify` 通过：仓库/布局/配置/i18n/装备契约门禁、格式、工作区全部已有目标、
独立 PostgreSQL/Redis feature、Clippy `-D warnings` 与 Public/Admin WASM。Rust 1.94.1 的
`CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过。
`cargo machete`、全部 CI shell 的 ShellCheck warning 检查及 `git diff --check` 通过；49 份 Markdown 的仓库内引用
已核对。三角色与合并服务的实际发布 feature 图均仅启用 tracing-opentelemetry 的 tracing-log，未引入 Web 代码、
第二数据库驱动或 native-tls/OpenSSL 运行库。

受控 cargo-audit 0.22.2 回执校验通过，刷新至 RustSec 提交 `99670cfc0bc574433034b75876a4b791ccaa2efb`
（1,380 条公告），最终应用锁 535 个包为零漏洞、零警告；同快照下 cargo-audit 独立锁通过，dx 完整锁仍有
13 项漏洞匹配/11 项警告，sccache 仍有 7 项/17 项。工具锁未修改，未设置忽略项；现有受限调用路径及待升级事项见
[工具审计](ci/tools/README.md#上游公告与实际使用范围)。

本轮未新增或运行行为测试、数据库/浏览器验证、性能基准或部署，未重建发布制品。复杂度、SQL 次数和分配变化来自源码
分析；吞吐、延迟与内存收益待代表性负载量测。

### 2026-10-27 全仓性能、稳定性与依赖复核

基线为干净的 `b82a7a6b`，本次修改尚未提交。经用户授权按 20 个工作区包的 owner 并行审查源码、注释、已有测试、
依赖与模块文档，主审另核对构建脚本、正式文档、完整依赖锁和实际发布 feature 图，并交叉复核摘要、SQL 和浏览器更新边界。

| owner | 本轮修复与保留的边界 |
| --- | --- |
| application | Go JSON 转义统一按借用片段输出；履约证明排序借用的装备 ID，摘要复用 `RequestHash`；保留旧字段顺序、重复 ID、空值省略、转义及摘要字节 |
| storage | 代理转换原因从逐状态反向扫描改为一次建表，平均复杂度由 `O(S×T)` 降为 `O(S+T)`，最后原因为空仍覆盖旧值；告警在原完整来源锁集内批量恢复，每行仍推进一次版本 |
| storage | 无资产通知先用 SQL 筛必填的 Send/零金额/空附件条件，再执行原闭集解码与历史来源校验；PM 载荷借用 JSON 解码，完整身份、重编码及摘要核验保留 |
| integrations | 论坛和 MoogleMail 按有界响应长度预留缓冲，逐块实际字节限制、超时、敏感正文清理及未知外写分类保持原义 |
| Public Web | 目录、页头和详情共用倒计时 hook，截止文本变化时才解析，秒针仍读取最新校时结果并生成当前语言文案；Public decoder 与应用实时事件以栈缓冲核对规范 UUID |
| config / apps | 世界与邮箱窄投影借用迭代，与完整邮件配置共用稳定分组，五处查询不再复制发件人名单；配置字段与世界顺序不变 |
| observability | 连接池 counter 使用原子最大值替代 CAS 循环与转发包装；OTLP 后续批次预留有界容量，直接初始化拒绝零批次、导出和关闭时限 |
| CI / release | 依赖隔离检查覆盖实际 PostgreSQL、Redis 和 Web 发布图；归档哈希批量生成，保持清单排序与格式 |

领域、DTO、缓存、HTTP/Admin/shared Web、Worker 调度、首次设置和进程生命周期均纳入审查，未因精简删除安全校验或改变
外部接口。源码与 README 的过期注释同步修正；状态页当前缺口不再重复列入已撤销的论坛镜像加密要求，历史更正保留。
直接依赖和现有测试均有实际用途，本轮不增删依赖或测试，未改变 Cargo 清单、锁文件和已发布迁移。
原生程序及 Public WASM 的生效方式见[同版升级说明](ops/README.md#读取快照与实时分发优化升级)。

Rust 1.99.0 下最终 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖策略、格式、工作区全部已有目标、独立 PostgreSQL/Redis
feature、Clippy `-D warnings` 和双端 WASM；`cargo machete`、全部 CI shell 的 ShellCheck warning 门禁、修改脚本的
`bash -n`、实际发布依赖边界及 `git diff --check` 通过。
Rust 1.94.1 的 `CARGO_BUILD_JOBS=2 cargo +1.94.1 check --locked --workspace --all-targets --all-features` 通过；
49 份 Markdown 的仓库内链接目标与标题锚点已核对，未发现失效引用。
本次刷新 RustSec 1,380 条公告，应用与 cargo-audit 锁图均为零漏洞、零警告；dx/sccache 完整锁仍有公告命中，分别见
[工具复核](ci/tools/README.md#2026-10-27-公告复核)，未设置忽略项。

未重建 Web payload 或发布制品，未执行行为测试、数据库/浏览器/外部服务验证、性能基准或部署；算法复杂度与分配减少来自
源码分析，吞吐、延迟和内存峰值仍待实际负载量测。

### 2026-10-27 全仓冗余代码、死代码与依赖复审

基线为干净的 `ab1402f1`，本轮改动尚未提交。按逐函数体去重扫描、无引用公共 API 扫描、panic 站点扫描与热路径审读
处理；Rust 源码净减约 1,700 行，行为测试未新增或执行。

- 单一 owner 收敛：14 个 MoogleMail 通知模板的字段、默认英文正文与占位符校验原在应用、配置与集成各一份，现由
  应用层 `runtime_config/mail_templates.rs` 唯一拥有，Worker TOML 与启动快照直接复用；闭集枚举（拍卖状态、竞价机制、
  代理模式/状态/终态原因/转换、结算模式/义务种类/状态、世界）约 40 处手写名称映射收敛为领域 `stable_name` 宏表；
  35 份相同的规范文本校验、10 份正十进制 ID 校验、12 份小写十六进制编码、约 23 处 `40001`/`40P01` 判断、
  两份 `user_login_key_envelopes` 写入 SQL 以及 Public/Admin 两份页面可见性等待各收敛为一处。名称、错误分类、
  SQL 与 wire 字节保持不变；HTTP 对外映射（如代理 `FinalFast` 投影为 `active`）仍独立维护。
- 删除确认无调用的公共项：`AuctionRules::validate_bid`/`BidDecision`（与竞价计划重复）、`Auction::schedule_end`/
  `apply_effective_end`、`SettlementPolicy::legacy_zero`/`interpolate_adjustment`、两个未用 DTO、两个配置访问器、
  `build_http_client`、`render_auction_thread`、`parse_html_document`、`scan_sent_summaries_fresh`、两个凭据集合构造器、
  `run_persistent_until_shutdown`、`mark_degraded`、装备词典的两个字符串分类包装、`use_media_query` 与 `set_silently`。
  经用户批准，同样无调用的开拍后取消截止判断（`cancel_with_rules*`、`AuctionRules::can_cancel` 与
  `DomainError::CancellationCutoff`）、论坛账号获取的 `for_loopback` 隔离入口，以及资金返还原语 `return_funded_delivery`
  与 `FundingReturnProof`/`FundingReturnKind` 一并删除；`cancel_cutoff` 仍随策略冻结，`settlement_funding_returns`
  表与迁移不变，契约说明见[不变量](docs/contracts/invariants.md)与[schema](docs/contracts/schema.md)。
- 竞价热路径：事务端口 `apply_plan`/`apply_slow_proxy_plan` 改为借用计划，每笔已接受竞价不再复制整份计划。
- 稳定性：存储重试循环兜底、Delivery Admin 重试、MoogleMail Return/Take 动作及论坛系统日志中的 6 处
  `unreachable!` 改为失败关闭的错误或显式分支，不改变正常路径结果。
- 两组论坛协议回归测试抽取重复的 HTML/客户端装配，断言与覆盖不变；依赖结论见
  [技术栈](docs/design/tech-stack.md#当前依赖裁剪与复核)。

Rust 1.99.0 下 `just verify` 通过，覆盖仓库策略、格式、工作区已有目标、独立 feature、Clippy `-D warnings` 与双端 WASM
检查；Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features` 通过，`cargo machete` 未发现未使用直接依赖。
未重建 Web payload，未执行数据库、浏览器、并发负载或外部服务行为验证，也未部署。

### 2026-10-27 Worker 到期唤醒、事务入口收敛与规则阅读行宽

在上一条记录的同一未提交工作树上，按用户授权继续处理三项遗留：

- Auction 主循环以 `poll + jitter` 为最长等待，scheduler、slow proxy 与 settlement 各自的 storage owner 新增只读提示，
  返回严格未来的最早到期／重试时刻，主循环在该时刻加 50 ms 余量（至少 200 ms）提前唤醒；领取仍由原锁内谓词决定，
  已过期未领取事项与提示失败沿用原周期，避免持续失败时空转。开拍、最终阶段、慢速代理与结算的理论延迟由最多约
  一个轮询周期降到亚秒级；[不变量契约](docs/contracts/invariants.md)与配置说明同步。
- 余下 33 个事务入口（含 3 个已删除的 `set_serializable` 辅助函数的 9 个调用方、4 个运维预检与 3 个仅只读模式入口）
  全部改为在 `BEGIN` 中声明，仓库不再发送独立的 `SET TRANSACTION`；锁序、重试分类、预检语句/锁等待上限与访问模式不变。
- Public 规则正文与服务说明的段落、列表、引用和标题限制为 70ch 阅读行宽，页面容器与表格仍占满可用宽度。

Rust 1.99.0 下 `just verify` 通过（43 秒），Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features`
通过。`CARGO_BUILD_JOBS=2 just web-build` 重建 Public payload，SHA-256 为
`bef90038ab4008ae426cfb5bd5b66f5851c5b40eae1114eb1e402499decaa28f`；Admin 输入未变，复用
`54ca7ebb7071c7353b6a8a4802add7a6ad223b92b0d1b7986c17ab57c152885c`，各 9 个文件。本次同样未执行数据库、浏览器或
外部服务行为验证，未部署；唤醒提示 SQL 与实际节奏仍需隔离 PostgreSQL 环境证明。

### 2026-10-26 读取快照与实时分发复审

基线为干净的 `5bfe53fb`，本轮改动尚未提交。按竞价热路径、Public 读取与会话、实时 relay/Hub、存储事务入口、
Worker 循环、依赖图与双端样式审读调用链和正式文档；源码审读不是逐行形式证明，也不等于生产验收。

- Public relay 原先每 250 ms 遍历全部历史 stream，并为每条 stream 在持锁连接上执行一次读取事务；现在目录查询只列出
  `next_sequence - 1` 超过本 consumer 游标的 stream，未使用且可绕过持锁 session 的连接池目录实现已删除。
  进程内 Hub 改为每事件只校验、编码一次并共享引用计数文本帧，连接任务不再逐条复校验、深拷贝或序列化。
- 竞价等十一处事件追加改用单条持锁 upsert 分配 stream 序号（原为三次往返），同一竞价计划只查询一次拍卖归属；
  四个事务 runner 与 47 处 BEGIN 后紧跟 `SET TRANSACTION` 的入口改为在 `BEGIN` 中声明隔离级别与访问模式。
- Public 目录、详情、竞价日志、成交历史、账户及补偿池共 9 处读取由 Serializable 改为 Repeatable Read 只读快照，
  不再持有 SSI 谓词锁；会话认证保留与 inspect 相同的谓词，`last_activity_at` 改为最多每 60 秒写入一次。
- 限流满表时先整批移除已过期窗口；`Date` 头交由 Hyper 统一生成；36 份相同的 `storage_error` 收敛为一处，
  并删除无人装配且会使用空标色规则的 `PublicLotReader for DatabaseStore`；Admin 各表格调用处重复的“仅当前页排序”
  与“错误可页内重试”注释共 20 行删除，约束仍由共享 `app/sort.rs` 与组件文档说明。
- `tracing-subscriber` 关闭未使用的 `ansi`，`nu-ansi-term` 退出锁图；其余依赖核对结论见[技术栈](docs/design/tech-stack.md#当前依赖裁剪与复核)。
- 共享样式新增对三层底色均不低于 3:1 的控件边界色，Public 输入/选择器、分页与语言选择改用该色；
  错误文字在 raised 底上提升至不低于 4.5:1，Admin 错误横幅改用深红底，并增加 `prefers-contrast: more` 适配；
  注册提示与数据库名说明改用本地化引号，少量物理 CSS 属性改为逻辑属性。已有行为测试未删除或新增。

Rust 1.99.0 下最终 `just verify` 通过，覆盖仓库策略、格式、工作区已有目标、独立 feature、Clippy `-D warnings`
与双端 WASM 检查；Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features` 也通过，
`cargo machete` 未发现未使用直接依赖。`CARGO_BUILD_JOBS=2 just web-build` 从未提交工作树构建双端
正式资源成功：Public payload SHA-256 为 `d5323e07803e0c36cd79b6944383b0b371d72a1def95de09e677bd8c573ed37a`，
Admin 为 `54ca7ebb7071c7353b6a8a4802add7a6ad223b92b0d1b7986c17ab57c152885c`，各 9 个文件。
本次未执行数据库、浏览器、并发负载或外部服务行为验证，也未部署；查询改写与隔离调整的运行效果仍需隔离环境证明。

### 2026-10-26 全仓审计后的审查复核

结算 Worker 将锁内身份冲突与命令账本摘要冲突统一记录为 `settlement_mismatch`，沿用原有持久退避；
规则编辑器的读取结果同时绑定语言与重读世代，发布成功后的权威快照不会清除成功提示。结算契约按非负
税费 `T` 修正 v1/v2 买方公式；玩家指南改用已发布并同步完成的论坛主题描述。
论坛镜像加密仍属尚未实施的设计，不代表现有数据库或历史副本已完成保护。

更正：用户明确游戏装备直链及其中的装备 key 一直公开，不属于隐私数据；上述镜像加密要求撤销。

复核后 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖仓库策略、格式、工作区已有目标、独立 feature、
Clippy 和双端 WASM 检查；Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features` 通过。
`CARGO_BUILD_JOBS=2 bash ci/release/web-bundle.sh admin` 重新构建 Admin 正式资源，
payload SHA-256 为 `076cf7cf353663db4a46c773ceecc776c3f391514d32ae08ae2c8665221a1413`，共 9 个文件。
本次未执行数据库、浏览器或外部服务行为验证，也未部署。

### 2026-10-26 全仓源码、依赖与双端界面审计

基线为干净的 `2b220ebe`。本轮按领域与应用、PostgreSQL、外部集成、Worker、HTTP、配置与运维、Public/Admin/shared Web、
依赖与 CI 的 owner 审读关键调用链、注释、已有测试和正式文档。结构与源码审读不是逐行形式证明，也不等于生产验收。

- 结算首次执行及新版账本重放复核锁内 workflow、claim、due 世代、稳定命令身份与请求摘要；旧版账本仍按历史路径核验。
  Public 详情转移唯一装备属性值，避免重复复制，领域和查询路径移除部分临时集合与重复转换。
- Worker 在领取返回后复核停机状态；未开始处理的 delivery 使用原 claim/token/fence 释放，邮件轮询未处理 claim
  等待租约恢复。外部邮件发送保留未知结果与原回执、Sent 证明边界。
- HTTP 对论坛镜像 P15 加 `no-store`，修复限流并发时钟倒退、Unix 锁文件校验顺序和实时事件公平性；
  管理仪表盘校验凭据快照。用户选择保留论坛游戏直链；论坛正文的本地明文持久化仍未修复，见[源码缺口](docs/status.md#源码核对发现的实现缺口)。
  更正：用户明确游戏装备直链及装备 key 一直公开，论坛镜像明文不属于隐私缺口。
- 已发布主题的内容 claim 过期重领时持久标记 `write_unknown`；远端 POST 的进度直到本地正文证明提交后才清除。
  未知回帖写入不受日常正文校验开关影响，先只读逐帖核对当前目标，不一致继续阻断重发。
- MoogleMail 装备脚本按遮罩后的括号栈定位目标字段所在对象，避免把外层 JavaScript 函数体误当作 JSON；
  严格字段、重复值和正文／脚本一致性校验保留。
- 总控持有迁移独占锁，合并服务预检 Unix socket；分体启动在 readiness 和回执前核对进程身份。
- Public 恢复历史场次世界与状态筛选，保留原拍品、出价记录及论坛入口；Admin 规则草稿与未决发布命令保存在认证会话内，
  重新进入时比较权威语言、版本和内容，冲突时保留草稿并禁止校验／发布；整页离开使用原生提示。
  双端和共享组件修正表格嵌套交互、手机导航、弹窗焦点和文本提示，五语言及对应设计说明同步。
- 依赖调整集中在 feature 边界：SDK 不启用未使用的实验性批处理，Worker 显式启用 reqwest JSON，
  observability 显式启用其独立 HTTPS 依赖；受控 dx 的 Zstandard 依赖锁升级至修复版本。
  应用和工具锁的公告范围及剩余匹配见[工具审计](ci/tools/README.md#2026-10-26-zstandard-公告)和
  [技术栈](docs/design/tech-stack.md)。已有行为测试未删除。

附件及论坛恢复修正后，Rust 1.99.0 下最终 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖仓库策略、格式、工作区已有目标编译、
独立 feature、Clippy `-D warnings` 及双端 WASM 检查；Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features`
也通过。首轮门禁发现的 Public 字符串所有权和 Admin WASM trait 导入错误已修复。
`cargo machete --with-metadata` 未发现未使用直接依赖；受控发布和审计工具回执验证通过。
`bash -n` 与 `shellcheck -x` 覆盖本轮触及的检查、Web 构建及工具脚本。一次性核对 45 份 Markdown 中可识别的
仓库内路径与标题锚点，未发现缺失；复杂 Markdown 语法不在该脚本覆盖内。
主动 LSP 诊断未完成：当前 `rust-analyzer` 启动失败；Cargo 结果不替代 PostgreSQL 或浏览器运行验证。

论坛加密方案审阅及后续附件解析修正之前，`CARGO_BUILD_JOBS=2 just web-build` 使用新 dx 构建双端正式资源成功；
Public payload SHA-256 为 `553a22e14e08a651fa213bccb9dafbae3f3ee43d1c4e42979743b3d2e5d53b7d`，
Admin 为 `0ee5b6c76146f9217c6b0c7ae2840d56829491e672f98a47ce3146ecc4b1ca58`，各 9 个文件。
该次构建不证明浏览器交互、数据库并发、真实论坛或邮件行为；未执行行为测试、服务启动或部署。

### 2026-10-26 统一分页右对齐

owner 为 `auction-web-shared::style`，Public 与 Admin 共用。`base.css` 将分页容器及折行数字组右对齐；
`mobile.css` 在既有手机和触控横屏断点中按内容宽度排列三列导航并靠右，保留前后按钮的 44px 触控列。
共享 README 与两端设计已同步；仅调整 CSS，DOM／键盘顺序、页码钳制、数量、查询与命令状态不变。

最终 `just verify && just web-build` 完整成功，覆盖格式、静态策略、工作区已有目标编译、独立 feature、
Clippy `-D warnings`、双端 WASM 检查及正式 `wasm-release` 打包。两端均生成新的 9 文件 payload，已重读 manifest：

- Public：`ffd28f8d50ed7f736a79454269ac0c1c6a884a69685cb057d356912e93cc2211`。
- Admin：`b2a9415921d00eb310c750513d9b69a0deeef2a16a0ce880087c6c00d196d261`。

按既定范围只做静态检查与构建，没有新增或运行行为测试、浏览器及手工冒烟；实际页面的对齐、折行与触控布局仍未运行验收。
未部署或执行运行库迁移；此前各阶段产物与验证记录保留原范围。

### 2026-10-26 恢复拍卖历史原有查看入口

owner 为 `auction-web-public`。按用户澄清，拍卖历史“查看”列保留原有“拍品”和“出价记录”，仅追加可用的
“论坛地址”；未发布或未对账时只省略论坛外链，不影响两个原有入口。

同时恢复 `/bidlog` 的 `live`／`final`／`settled` 目录及已结算状态文案，使历史链接继续选中原场次，
不再因结算跳到进行中场次或空态。保留目录代际、查询身份、分页与旧响应隔离；五语复用已有消息。
Public README、设计、表格契约与 P30 契约已同步；HTTP／DTO、数据库及迁移没有新增变化。

最终 `cargo fmt --all && just verify && just web-build` 完整成功。静态门禁包含格式、仓库策略、工作区已有目标编译、
独立 feature、Clippy `-D warnings` 与双端 WASM 检查；正式入口重新编译 Public，Admin 输入未变并通过校验后复用。
两端各 9 个 payload 文件，已重读 manifest；本次 Public `wasm-release` payload SHA-256 为
`eca357719d9fd778b8145a73a40f9ae65f7249c6bb54ba55902fa89b8c41d354`，Admin 摘要保持此前记录。

按既定范围仅执行静态检查与构建，没有新增或运行行为测试、浏览器或手工冒烟；历史链接点击、翻页和返回仍未作运行验收。
未部署或执行运行库迁移。本节更正下方记录中的历史入口收敛，不改写此前的检查结果。

### 2026-10-26 统一分页与拍卖历史论坛链接

本节关于移除历史出价记录入口的做法已由[恢复原有入口](docs/status.md#2026-10-26-恢复拍卖历史原有查看入口)更正；
以下保留当时的实现与验证记录。

Public、Admin 与 shared 三个 Web owner 统一列表分页布局，调整手机数字页码与顶栏、缩小并居中 Key 登录输入框，
适配桌面与手机补偿展开控件。Public 活动竞价记录排除已结算场次，历史操作列移除对应出价记录入口，
场次标题与“按拍品筛选”同排；历史页“查看”列增加真实论坛地址。相关模块说明、设计与表格契约已同步。

新增 P30 `GET /api/v2/auctions`：contracts 拥有独立 v2 摘要，application 拥有不可绕过构造校验的 `PublicForumLink`，
storage 在原分页 SELECT 与同一只读快照内投影地址及内容证明，HTTP 映射已校验链接。历史页每页一次读取，
不逐行调用 P15，也不加载镜像正文；P07 v1 的 DTO 字段闭集保留。application 复用工作区已有 `url` 依赖，未升级版本。

`LEFT JOIN` 资格包含 `published`、正内容版本及非空同步时间；`000025` 保留的未对账历史场次正常返回，仅省略链接。
声明有证明的记录仍须满足有效非零同步时间、有界且无空白／控制字符的无用户信息 HTTPS 地址；损坏时返回
`503 storage_unavailable`，不能降级成无链接。SQL 先限制地址投影为最多 2048 字节，应用层与浏览器分别校验安全边界。
版本、遗漏与损坏的区分及部署顺序见[P30 契约](docs/contracts/routes.md#p30-v2-拍卖历史与论坛地址)。无新增迁移。

本轮 `cargo fmt --all && just verify && just web-build` 完整成功，覆盖格式、仓库策略、工作区已有目标编译、
独立 feature、Clippy `-D warnings`、双端 WASM 检查及正式 `wasm-release` 构建。首次检查发现新类型缺少 crate 根导出，
补齐后重跑通过。最后一次源码调整后，经正式入口与两端 manifest 复核的 payload SHA-256：

- Public：`28ec92a4c3c8b72ad007f654ff79f9853a1e497fb3ace6a1f0f126480385648c`。
- Admin：`41108f6e18248a82dffed2ee13f895870a4b4cbf2c43464d12ec5584f28ec8bd`。

随后将 P15 地址校验显式放入 `published` 分支，其他状态继续校验可选地址，所有分支均复用 `valid_forum_url`，
每个地址只解析一次。该调整后完整执行 `cargo fmt --all && just verify` 通过，并再次运行 `just web-build` 成功。
正式入口确认两端输入指纹未变，分别复用通过校验的 9 文件 payload；重新读取两个 `auction-web-manifest.v1`，
确认上列 SHA-256 与前次一致。本次执行没有触发新的 WASM 编译，不将复用结果表述为重新编译。
同时核对 `cargo tree --locked --target wasm32-unknown-unknown --workspace --invert auction-application`：
两端 Web 均不在 `auction-application` 的反向依赖图中，与正式入口判定浏览器输入未变一致。

本轮按用户限定只做静态检查与构建，没有新增或执行行为测试、浏览器验证、手工冒烟或数据库场景。
分页交互、视觉效果与 P30 的实际 PostgreSQL／HTTP 行为未作运行验收；LSP 查询因服务异常退出不可用，未计为通过。
未部署或执行运行库迁移；本条保留此前静态检查和浏览器验收记录各自的范围。

### 2026-10-26 全前端交互语义与可访问性修订

owner 为 `auction-web-public`、`auction-web-admin` 与 `auction-web-shared`。Public 补齐登录、竞价／代理和查询表单的
输入语义、错误描述关联与登录失败焦点，成交历史高级条件支持 Escape 关闭并将焦点返回触发按钮。Admin 补齐登录、
拍卖、用户、凭据、规则、邮箱、审计、Realtime、Runtime 与首次设置字段语义，用户名称筛选增加显式搜索按钮并同步五语。
共享层统一标题折行、原生控件表面、可见焦点与触控反馈，自定义分页数量非法提交会显示错误并聚焦输入，Markdown
代码禁止浏览器自动翻译；双端入口同步主题色。沿用现有布局、纯 Rust CSR 和五语资源，HTTP／JSON、数据库、配置、
命令身份及会话写门禁契约不变，无新增依赖或迁移。相关设计与模块 README 已同步。

完整 `just verify` 退出 0，工具链为 Rust 1.99.0；覆盖仓库／布局／配置／i18n 等静态策略、rustfmt、工作区已有目标编译、
独立 feature、Clippy `-D warnings` 及双端 WASM 检查。首次编译发现新增筛选错误 ID 的两处 E0382，改为借用后重跑通过。

`just web-build` 退出 0，Dioxus CLI 0.7.10 分别生成 Public／Admin 的正式 `wasm-release` payload；
每端清单含 9 个文件，并通过构建入口的文件结构、资源清单及输入指纹检查。payload SHA-256：

- Public：`fb74e1795e6b35f24532ae688f2645204e8cf936d73c03cd634a959e9fa975e3`。
- Admin：`c20300cdb7869ce867bab3dec30aa0dde150043f80a753c98f6373929965a3b8`。

**未做实际页面验证**。本轮按用户明确限定，只执行静态检查与构建；没有创建样本服务器、启动 Chromium、执行浏览器测试
或手工冒烟，也未新增或运行行为测试。视觉效果、键盘／触控交互及读屏效果仍待实际页面验证；静态通过不代表运行验收。
未连接数据库或真实外部服务，未部署或执行运行库迁移；本条不改写此前各轮浏览器验证记录。

### 2026-10-26 Public“我的”页面信息密度

owner 为 `auction-web-public`：`identity/account.rs` 增加稳定的偏好与列表布局容器，统一 AccountPager 在空／单页时
用隐藏容器收起分页控件，仍挂载 Pager 执行页码及 URL 纠偏。`public.css` 收紧段落与分区留白，`desktop.css`
并排展示设置和出价／代理分区，`mobile.css` 收紧账户卡片字段间距。表格列权重、账号隔离、草稿、命令宿主及外部
HTTP／JSON／数据库契约不变，没有新增依赖或迁移。

最终完整 `just verify` 和 `bash ci/release/web-bundle.sh public` 均退出 0；正式 Public payload SHA-256 为
`9bc2db48e81c7e300b09bfe5ceaeb3216f7dfca39b21bc98bf1e35a3562dd87a`。
Chromium 加载实际 WASM，本机内存 API 提供布局样本。同一中文样本包含每表一条记录、永久登录和长装备名，
视口高度 900px；`#public-content` 主区高度测量如下，仅代表该样本的布局结果。

| 视口宽度 | 调整前主区高度 | 调整后主区高度 |
| --- | --- | --- |
| 375px | 2076.4px | 1460px |
| 1440px | 1341.9px | 831px |

320px 五语及 375／430px 无横向溢出，受检手机表单和分页按钮至少 44px 高、输入字号至少 16px；1024／1025px
设置分区与 1279／1280px 交易列表分区边界正确，1440px 保持并排表格。标记 11 条、出价 25 条、代理 13 条分别
翻到末页后展示 1／5／3 条且页码互不覆盖；总量缩至 1 时两个 URL 页码自动回到第 1 页，分页控件隐藏。
0 条及恰好 10 条的边界也已核对；未保存的 PM 渠道、2.5 天登录时长在手机／桌面切换时保持原 DOM 和草稿。
读取 503 的反馈仍可见，不隐藏失败或伪造空态。

本机证据与截图位于 `target/audits/account-density-20261026/evidence.json` 及同目录。分页与草稿场景先在同轮
payload `89fd7ca0adc36bb872ea7c3af848abfc199052d0089f5bf8044e844d799af751` 完成，随后仅修正分区叠加边距，
重新完成静态检查、正式构建及五语／断点测量。未连接数据库或真实外部服务，未提交业务命令、迁移或部署；
未新增或执行永久测试。临时预览服务和浏览器页在核对后关闭。

### 2026-10-26 Public 手机顶栏与分页分组

`auction-web-public/assets/mobile.css` 将手机顶栏按 DOM 顺序分为两列：世界与连接状态、刷新与语言分别成组，
会话及错误独占整行，长用户名允许折行。Public 分页的总数／数量、自定义数量、数字页码与跳页分行，数字页码
自然换行，输入与提交按钮按 2:1 分配宽度。桌面沿用原布局；共享 Pager、HTTP、JSON、会话及数据库契约没有变化。

本轮完整 `just verify` 及 `bash ci/release/web-bundle.sh public` 均退出 0；Public 正式 `wasm-release` payload
SHA-256 为 `81a901afc9d6c87243911ecd268d71c0d429985017f42441016869232301c312`。
Chromium 加载该实际 WASM，由本机内存 API 提供布局样本；320px 五语、375／430px、159 字符用户名、1000 页数字分页、
匿名及会话 503 状态均无横向溢出，受检手机顶栏／分页触控目标至少 44px 高，输入字号至少 16px。
768／769px 普通指针与 1024／1025px 触控边界分别切换手机／桌面布局；1920px 保持桌面控件。
目录跳至第 8 页、选择 20 条后回到第 1 页、自定义 13 条及拒绝 0 的提示均在实际页面核对。
往期拍卖、出价记录、黑名单、补偿池及展开流水、装备详情、账户和历史成交页的分页也已核对。
补偿流水在 320px／1920px 往返切换时保留原顶栏、分页节点及展开状态。

本机证据与截图位于 `target/audits/public-controls-20261025/evidence.json` 及同目录；目录沿用本轮开始时的日期。
这些是隔离布局与分页交互证据，不代表生产验收；没有连接数据库或外部服务，没有验证已连接 WebSocket 的分支。
未新增或执行永久测试，未提交业务命令、迁移或部署。临时预览服务与浏览器页在核对后关闭。

### 2026-10-25 Public 手机卡片与统一页边距

按用户要求，`auction-web-public` 的手机业务表将同一行 DOM 重排为卡片：字段分两列，名称与操作占满整行，
长名称、时间与金额可折行；目录出价／代理并排，标记另起一行。装备详情事实也使用字段卡片，补偿批次及嵌套流水
补齐既有五语字段标签。表头仅视觉隐藏，原生表格语义、列显隐、分页、读取及命令宿主继续保留。
全部公开页使用完整可用宽度与左右各 12px 的边距，遇安全区取较大值；规则、论坛、登录取消独立页面宽度上限。
桌面、打印、Admin 和 Markdown 保持表格。HTTP、JSON、竞价、会话、数据库与部署契约没有变化；没有新增依赖或迁移。

本轮完整 `just verify` 退出 0，覆盖策略、格式、工作区 all-targets、独立 feature、Clippy `-D warnings` 和双端 WASM check。
`bash ci/release/web-bundle.sh public` 生成正式 `wasm-release` payload，Rust 1.99.0、Dioxus CLI 0.7.10；
payload SHA-256 为 `1a9307645e60eb87ef335aa11d084a28eb42fe39e60aaff140b336eecadf7893`。

Chromium 加载该实际 WASM，API 由本机内存布局样本提供，没有连接数据库或外部服务。核对了实时／预览／已结算目录、
往期拍卖、竞价日志、装备详情及历史、账户三组列表、历史成交、黑名单、补偿展开、规则、论坛、登录与状态页。
320px、375px 的定向页面无横向溢出；320px 与 1920px 的目录、补偿、成交历史、规则、论坛、登录页左右边距均为 12px。
768／769px 普通指针与 1024／1025px 触控边界分别切换卡片／表格，同一指针模式跨断点保留原记录节点。
五语目录字段标签与表头一致，隐藏字段仍为 `display:none`；实时目录三个操作按钮实测均为 44px 高。
可访问树保留 caption、columnheader、row 与 cell；打印恢复 table-row、静态表头与 colgroup，Markdown 仍为表格。

本机证据与截图位于 `target/audits/public-cards-20261025/evidence.json` 及同目录；这些是隔离布局证据，
不代表真实竞价、账户写入或生产验收。样本服务未提供 WebSocket，因此没有验证实时传输；未提交业务命令、迁移或部署，
未新增或运行永久测试。临时预览服务和浏览器页在核对后关闭。

### libc 0.2.191 依赖 PR 合并与同步

按用户选择，仅合并 [PR #3](https://github.com/ngnlAYY/Auction/pull/3)，将应用锁中的 libc 从 0.2.190 更新到 0.2.191；
[PR #2](https://github.com/ngnlAYY/Auction/pull/2) 的 tungstenite 0.30 升级暂缓，不关闭、不设置忽略规则。
当前 HTTP 活跃链仍为 Axum 0.8.9、tokio-tungstenite 0.29.0 和直接 tungstenite 0.29.0，避免跨版本 downcast 破坏 1009 关闭契约。

PR #3 的 docs、verify、security-audit、MSRV 检查成功；重复工作流按原策略跳过。GitHub squash 提交为
`97698f743574805be755ee348dd70c858c212769`，远端 `mergedAt` 回执为 `2026-10-26T02:48:22Z`。
本地以普通 merge 保留原未推送提交 `31de07af64cac9fd35b3ca1799aa46dfe8893ff1`，生成
`fb04e0324ce93d86ddb205a34cb4ca64a899b64e`；仅合入 Cargo.lock，不重写原提交，不提交本轮审计工作。
同步前后的未提交 tracked diff 摘要及 11 个未跟踪源码文件摘要完全一致，既有审计改动未被覆盖。

合入后重新执行完整 `CARGO_BUILD_JOBS=2 just verify` 和 Rust 1.94.1 的工作区 all-targets/all-features check，均退出 0；
应用锁的 `cargo audit --deny warnings` 为零漏洞／零警告，公告库仍为 1352 条、提交
`21d909e4c8bd4e14c7aea58bf980c881a0053ab1`。没有运行行为测试、迁移或部署，也未推送本地合并提交与审计改动。
本阶段没有重建发布程序或 Web payload；下节双端资源构建记录对应依赖 PR 合入前的锁图。
回执及合并后检查日志保存在 `target/audits/project-mobile-20261025/dependency-pr-sync.json` 和同目录日志。

### 2026-10-25 全仓复审与桌面移动展示分离

基于干净提交 `31de07af64cac9fd35b3ca1799aa46dfe8893ff1`，按 20 个工作区包的 owner 做结构、调用与热点审计，
并核对注释、既有测试、依赖图及正式文档；不把结构扫描描述为逐文件行为验收。

- application/storage/apps 删除三个命令服务及适配器中未消费的 Clock 泛型、字段和构造参数；时间仍取锁后事务。
  旧竞价重放借用历史投影；contracts 将 11 份同义解码函数集中为两个内部实现，保留 missing/null/type-error 区别。
  邮箱维护仅在批次包含可重建 Send 时读取完整 Sent；空批次、仅 Take 或仅不可重建旧行不做该扫描，
  也不增加无关 Sent 失败统计。有 Send 却缺完整列表时，仍保留首个 Send 及其后缀的轮转位置。
- storage 的邮箱维护先限定环形候选批次再关联发送／领取证明，页外歧义检查保持；调度只对需要截止的阶段聚合 lot。
  结算失败先锁拍卖行再生成退避起点；删除临时 roll/lot ID 数组、摘要 Vec 与已消费字符串副本，不改锁序、Serializable、CAS 或审计。
- integrations 将论坛 Cookie 克隆改为共享且不可逆撤销，防止已准备会话在失效后继续发新请求；
  Send 后的明确登录失效保留原证据，仍属于未知写入。补齐 session/owner 检查的绝对预算和完成后截止检查。
  pending 邮件恢复由逐 MID 重扫列表改为本批借用索引，预期复杂度由 `O(P×(I+R))` 降为 `O(P+I+R)`；未测量运行时吞吐。
- HTTP 有界正文复用单帧 `Bytes`，多帧才合并；装备链接投影转移所有权，删除必然被覆盖的缓存头扫描。
  Worker 在派生任务前检查初始化期间已到达的停止请求，原在途排空与未知外写策略不变。
- Public/Admin 分别增加桌面／移动 Rust 纯展示模块和 CSS；请求、表单、身份、表格与未决命令保留单份。
  手机目录采用同源计数的原生类型选择器，账户提供折叠章节导航；Admin 修正把 flex 直接施加到 `td` 的操作列，
  调整触控表单、导航、首次设置及未决命令布局。共享层按数值缓存 VisualViewport，减少重复事件的字符串分配和 CSS 往返。
  全部样式声明 `foundation, surface, desktop, mobile` 层顺序，不依赖 Dioxus 未保证的 head 顺序。
- 删除四个未注册的 Admin Dashboard 源文件及其 CSS、每种语言 53 条孤立消息；清理旧空态别名、无行为的表格类及无人使用的 sheet 变体。
  12 个现有测试分别保护迁移、协议和安全边界，没有发现可删除的重复测试，全部保留；未新增、扩展或运行测试。
- root README 更正“v2 奖池不足暂缓结算”的过期说明；当前是一份基础优惠同比缩减，零预算仍结算。
  领域公式和历史冻结快照没有修改，旧状态记录保留原时点结论。

最终 `CARGO_BUILD_JOBS=2 just verify` 退出 0，覆盖仓库策略、格式、工作区 all-targets、独立 PostgreSQL/Redis feature、
all-features Clippy `-D warnings` 与双端 WASM check；编译器为 Rust 1.99.0。独立目录的
`cargo +1.94.1 check --locked --workspace --all-targets --all-features` 同样通过。
首次门禁发现 Public 选项 Vec 在 RSX 中提前移动、Admin 对 bool 调用 clone，两处均已修正，未放宽 lint。
`CARGO_BUILD_JOBS=2 just web-build` 生成双端正式 `wasm-release` payload；每端五个 CSS 引用均指向实际打包资源，
压缩后的层首次声明顺序仍为 `foundation, surface, desktop, mobile`，无悬空相对导入。
8 份 CSS 的独立静态解析无诊断；48 份正式／协作文档的 1,359 个本地路径与标题锚点检查无错误。
设备视图互斥、菜单开合、原表单内的移动提交按钮仅完成源码／CSS 级核对，不是浏览器或真实 DOM 执行证据。

依赖核对：`cargo machete --with-metadata` 未发现未使用的直接依赖。`chrono clock` 收敛到唯一使用本地时区的 config owner；
独立 Public/Admin WASM feature 图保留 `now`，不再启用 `clock/iana-time-zone`；config 原生图仍保留。
应用 536 条锁记录在 1352 条 RustSec 公告、提交 `21d909e4c8bd4e14c7aea58bf980c881a0053ab1` 下为零漏洞／零警告；
cargo-audit 独立锁同样为零。dx 仍有 13 个漏洞匹配／11 个警告，sccache 为 5／9，未忽略公告或强行跨不兼容版本替换；
受限用途和许可边界见[构建工具说明](ci/tools/README.md#上游公告与实际使用范围)。未增加应用依赖或修改锁版本。

本轮验证限定为静态检查与资源构建；未执行单元／集成／浏览器测试、手工冒烟、数据库查询场景或真实外部操作。
一个临时本机静态预览辅助服务曾短暂启动，在访问前停止并删除脚本；没有启动 Chromium 或产品服务。
移动设备视觉、缩放／软键盘、改动路径的并发和外部协议行为均未验收，不宣称延迟、RSS 或吞吐提升已量化。
Rust LSP 启动后异常退出，未记为通过；CodeGraph 的代表性改动文件 affected 查询未找到独立测试文件，不代表内联测试覆盖。
未执行迁移、提交、推送或部署。升级与历史未知结果边界见[运维说明](ops/README.md#全仓复审与设备展示拆分升级)；
静态回执与依赖图保存在 `target/audits/project-mobile-20261025/`。

### 2026-10-25 R33 装备状态准备与注册账号提示发布

按用户授权，冻结干净提交 `9831675e5354ea1c05f08982942a22459f78c1e5`，以通用 Linux x86_64 GNU、
`CARGO_BUILD_JOBS=2` 重新执行完整 `just verify` 与 `just release-check`，均退出 0。API-only 与 embedded
四程序分别重新编译约 6 分 12 秒、6 分 17 秒，双端 WASM 经本轮 bundle 生成并通过资源隔离、确定性归档门禁。
源码指纹为 `ddcc83a65052f82c313c81e2ac7f74800f2168a5c16fe8b27c3658c4cb1888da`，
归档 SHA-256 为 `e0ee2bb92a06d442837335b4012b14ef293dfbea0b7e25bdf565e142d299eb8f`。

- `auction-integrations` 的装备状态准备与 contracts/HTTP/Public Web 的 P29 注册账号提示同版发布。
  既有 runtime 配置含 1 个机器人显示名；未改配置或手工调用新业务接口。外部契约见
  [附件装备状态准备](ops/mail/diagnostics.md#附件装备状态准备)与[P29 注册账号提示](docs/contracts/routes.md#p29-注册账号提示)。
- 使用新 R33 私有目录、冻结计划与 launch-once 入口，上传并核验 30 个包文件；旧 Worker 正常 drain。
  原 CLI 创建 schema64 数据库恢复点，新 CLI 完成离线验证，未恢复数据库；schema64 → schema64，无待执行的新迁移，已发布迁移未重放。
  停稳后的 88 张业务表共 8,818 行，更新前后逐表行数与完整行摘要一致。
- 三角色配置、runtime、managed、发送预算、unit、tmpfiles 和原 runtime 锁 inode/0600 权限均保持。
  既有三个拍卖策略快照仍为 v1，包括 Main 第003期草稿；原三条人工核对投递完整行摘要保持，未重排、补发或强制结清。
- `2026-10-25 12:26:38 UTC` 部署完成，`deploy.exit=0`，按 Worker → Admin → Public 启动，三角色 readiness 200，
  两端与公网共 16 项静态资源匹配。`12:29:44 UTC` 仍为同一组进程、`NRestarts=0`，日志 WARN/ERROR 为零且未截断，
  runtime 未决命令为零，人工核对投递仍为三条。
- 本地隔离角色在本轮开始前已经全部停止，更新后保持停止；仅构建刷新开发 Web 工作目录，未启动本地 Worker 或刷新数据库。
  本轮未新增或执行业务、浏览器、真实解锁／发送或注册流程测试；下文先前隔离验证记录保留原时点与范围。
  Rust LSP 对 26 个变化文件无有效回答，未记为通过。

回执在 `target/dist/equipment-unlock-20261025-ops-r33/`：本轮构建门禁、`plan.json`、`upload.json`、`launch.json`、
`postflight.json`、`stability.json`、`DEPLOYMENT-SUMMARY.json` 及 12 份 `cloud-evidence/` 闭集脱敏回执。
敏感配置、原始日志及数据库归档留在云端私有目录；部署就绪不扩展为真实游戏协议验收。

### 2026-10-25 登录页显示配置机器人账号

config 将既有 runtime `bot_players` 投影给 Public，装配根只向 HTTP 注入非敏感名单；
contracts/HTTP 拥有新增 P29 `GET /api/v1/registration-info`，Public Web 将账号作为五语纯文本插入注册提示。
多个账号保持配置顺序，空名单或读取失败保留原提示；不新增 TOML 字段、数据库对象或迁移，不改变 Worker 注册／履约规则。
字段、方法和重启边界见[P29 契约](docs/contracts/routes.md#p29-注册账号提示)及[配置手册](ops/config/README.md#runtime-共享与重启)。

- 通过正式 `ci/release/web-bundle.sh public` 构建 WASM，并用 `web-embed` 构建本机 Public 程序。
  三个临时回环实例加载单账号、多账号、空名单 TOML，不含数据库、真实 Cookie 或密钥；P29 均返回配置中的精确名单。
  native HTTP 确认 `no-store` 且不写 Cookie，GET 非空 query 为 400，HEAD/POST 为 405 且 `Allow: GET`。
- 实际 Chromium 登录页确认五语均展示配置账号；多账号名称中的 `<b>` 只显示为文字，不生成 HTML 元素。
  375px 页面无横向溢出，空名单及人为中断 P29 请求均保留原注册说明，不伪造账号。
  临时实例未配置数据库，页头原有会话／readiness 不可用提示保留，未把页面展示验证当成真实 Key 登录验收。
- 首次临时 runtime 目录／文件权限不合规则被正式 loader 拒绝；按既有 0750/0640 边界修正后启动，
  未放宽产品权限校验。三个临时实例均经 SIGTERM 正常退出 0，浏览器及临时配置均已清理。
- 完整 `CARGO_BUILD_JOBS=2 just verify` 退出 0，覆盖仓库策略、格式、工作区／独立 feature 编译、
  all-features Clippy `-D warnings` 及双端 WASM。CodeGraph affected 未发现关联测试文件，
  本轮未单独运行 MSRV 门禁。
- 证据与桌面／手机截图位于 `target/audits/login-registration-20261025/`。未新增或执行永久行为测试，
  未访问真实游戏／论坛、执行 PostgreSQL 场景、迁移、提交、推送或部署；Rust LSP 异常退出，未记为通过。

### 2026-10-25 附件装备自动出库与解除保护

owner 为 `auction-integrations` 的 Armory 协议、库存解析及 MoogleMail sender。按用户提供的 HVUT 4.2.4
协议参考实现 Rust 侧发送准备，不依赖该脚本运行：七类完整库存唯一定位，按需出库和解除保护／锁定，
每次修改后重新读取并核验目标状态，再执行原附件与 Sent 证明。
状态提取绑定精确 EID 及元数据中的显示名，不把自定义名称中的图标解释为状态；原内部正向库存证明保持不变。
完整协议和失败边界见[附件装备状态准备](ops/mail/diagnostics.md#附件装备状态准备)。

- 临时 Rust 入口实际调用公开 `MoogleMailSender::send`，连接本机隔离 HTTP 协议响应；20 个场景均通过，
  并核对请求字段、顺序、token 轮换、目标状态和无关装备不变。覆盖已就绪、仓库、保护、锁定、组合批次、
  Isekai 多分类、名称含图标，以及写入未生效、写后断连、回读失败、目标缺失／跨分类重复、穿戴、
  重复 token、EID 不符、跨世界页面、无装备通知和脏草稿。
- 成功路径取得精确 Sent MID 和空 compose；异常准备路径未进入附件／Credits／COD 暂存或最终发送。
  所有写后故障保留不可自动重试的 `write_unknown`，没有自动回滚或第二次 Armory POST。
- 首轮临时 MM 附件 fixture 错用了 Armory 的 hover hook，被既有附件证明拒绝；修正为既有
  `equips.set(EID,...)` 协议后通过，没有放宽产品附件解析。临时 Rust 入口、运行配置和回环服务均已清理，
  脱敏证据保留于 `target/audits/equipment-preparation-20261025/evidence.json`。
- 完整 `CARGO_BUILD_JOBS=2 just verify` 退出 0，覆盖仓库策略、`cargo fmt --all -- --check`、
  工作区／独立 feature 编译、all-features Clippy `-D warnings` 和双端 WASM；Rust/Cargo 为 1.99.0。
  本轮未单独执行 MSRV 门禁，不能把当前工具链通过扩展为 MSRV 证据。
- 未新增、删除或运行永久行为测试；未访问真实游戏／论坛、启动产品服务、执行 PostgreSQL 场景或迁移，
  未部署或修改线上装备。没有原生 Armory HTML 样本或真实游戏协议验收，隔离成功不扩展为生产验收。
  Rust LSP 启动后异常退出，未记为通过；CodeGraph affected 未发现关联测试文件。

### 2026-10-25 R32 基础优惠 v2 发布与 schema64

按用户授权，冻结 `476b5f978b0a18847e18868ad7ce8c40a3ab8bd7`，以通用 Linux x86_64 GNU、
`CARGO_BUILD_JOBS=2` 重新执行完整 `just verify` 与 `just release-check`，均退出 0。API-only 与 embedded
四程序分别重新编译约 8 分 43 秒、8 分 23 秒；双端 WASM 本轮亦重新构建，发布 payload 字节摘要与 R31 相同。
源码指纹为 `eec5ed4029ef88cdbfeda845c723afc42add2608066ad94195bf8eb7103da411`，
归档 SHA-256 为 `ebd48ea4803dae552fdf389ec367b35da736f752fd316efafec35e67a1262e98`。

- 使用独立 R32 目录、冻结计划与 launch-once 入口，上传并核验 30 个包文件。按原停止契约关闭 Public/Admin，
  在无在途 Sending/preparing 的安全窗口正常 drain Worker；保留原发送/资金证据期限，不主动打断外写。
- 旧 CLI 创建 schema63 恢复点，新 CLI 离线验证 archive、manifest、数据库身份和完整迁移前缀；未执行恢复。
  同版 CLI 只追加 `000064_seller_full_settlement`，四个新 CHECK 全部 validated；不新增表。
  停稳后的 88 张既有业务表共 8,706 行，迁移前后逐表行数与完整行摘要一致。
- settlement owner 的新默认现已部署：common 的结算 version/mode 明确改为 `2/hold_curve_v2`，并更新两条默认值注释。
  曲线、其他配置、managed、预算、unit、tmpfiles 及原 runtime 锁 inode/0600 权限不变；新建场次仅享一份奖池基础优惠，
  卖方全额收成交价，奖池不足同比缩减、零预算照常结算。规则见[结算不变量](docs/contracts/invariants.md#i-07a-结算策略与命令重放必须可从权威事实重建)。
- 既有三个拍卖策略快照摘要保持不变，均为 v1，包括 Main 第003期草稿；没有自动切换、追溯改账或补差。
  原三条人工核对投递完整行摘要保持不变，没有重试、补造成功或强制结清。
- `2026-10-25 07:07:07 UTC` 部署完成、`deploy.exit=0`，Worker → Admin → Public 启动，三角色 readiness 200，
  两端及公网共 16 项静态资源匹配。本地隔离 Public/Admin 的运行 PID/哈希不变；构建刷新开发 Web 工作目录，Worker 仍未启动。
- `07:10:20 UTC` 稳定性复核：三角色同部署进程、`NRestarts=0`，日志 WARN/ERROR 均为零且未截断，runtime 未决命令为零，
  人工核对投递仍为三条。没有新增或执行业务、浏览器或真实结算测试，没有主动发信、论坛命令或本地数据库刷新。

回执在 `target/dist/seller-base-20261025-ops-r32/`：本轮构建门禁、`plan.json`、`upload.json`、`launch.json`、
`postflight.json`、`stability.json`、`DEPLOYMENT-SUMMARY.json` 及 14 份 `cloud-evidence/` 闭集脱敏回执。
敏感配置、原始日志与数据库归档留在云端私有目录；本地只归档受控摘要，不把发布就绪扩展为 v2 真实业务验收。

### 2026-10-25 v2 仅保留基础优惠（源码修正，尚未部署）

用户进一步明确：不再保留旧 v1 的额外奖池优惠。`auction-domain` 的新 v2 仅按一份基础优惠 D 分配，
`C=min(D,floor(A×D/ΣD))`；A 仍为同一锁内世界奖池期初余额加本场税费，零需求时 C 为零。
卖方始终收 P，买方付 `P+T-C`；不额外发 C 红包、不重复减价。原默认曲线保持不变，基础优惠 2% 时
足额最多优惠 2%，成交 100K 的买方付 98K、卖方收 100K；优惠预算只覆盖一半时买方付 99K。

- 保留同比缩减、逐件向下取整和尾差留池，零预算无优惠仍结算。旧 v0/v1 冻结快照、命令重放及三/八变量邮件不变。
- 领域计划、明细和 v2 九变量账单均将实分金额封顶为一份 D；integrations 的 v2 正文不再描述额外优惠。
  HTTP 报告恢复总支出不超过基础优惠需求的闭集，沿用尚未发布的 `000064` 并同步收紧其 CHECK；没有修改已发布迁移。
- 新模板保留既有折扣严格小于 50% 的保守上限，不借本次移除额外优惠扩大可配置范围；实际默认最高优惠仍为 2%。
- 本次完整 `CARGO_BUILD_JOBS=2 just verify` 通过，覆盖策略门禁、格式、工作区/独立 feature、all-features
  Clippy `-D warnings` 与双端 WASM check。最终日志为 `target/seller-base-only-verify-final.log`，对应 `.exit` 为 0。
  Rust LSP 对 20 个改动文件仍无有效回答，未记为通过；pi-lens 另有既有金额格式化函数的 `chars().enumerate()`
  AST 提示 1 条，该函数实现本轮未改。未新增或执行行为测试，未发布、执行迁移、切换已有草稿、补差或改写历史账款。

规则及载荷边界见[结算不变量](docs/contracts/invariants.md#i-07a-结算策略与命令重放必须可从权威事实重建)
与[结算交付契约](docs/contracts/schema.md#结算恢复与交付)。

### 2026-10-25 v2 奖池不足同比缩放（含双份优惠的未发布中间方案，已取代）

以下保留当时的双份优惠方案和静态检查记录；当前 v2 仅保留基础优惠。

用户修正预算不足规则：`auction-domain` 以全场名义优惠为权重，统一缩小基础及额外优惠；
`C=min(2D,floor(A×D/ΣD))`，A 为同一锁内世界奖池期初余额加本场税费。每件向下取整、尾差留池；
奖池为零则无优惠仍正常结算，卖方始终收成交价。取消此前未发布中间方案的奖池不足错误与 Worker 等待分支。

- 沿用尚未发布的 `hold_curve_v2/version=2` 草案与 `000064`：CHECK 允许实分优惠为零至两份名义折扣。
  逐件同比分配仍由领域计划和 application 的首次冻结奖池/锁内事实重建校验；不改旧迁移或历史经济行。
- application/storage 从同源明细读取冻结模式，v2 买家邮件使用显式 `pricing_mode=hold_curve_v2` 的精确九变量，
  抵扣前价为成交价加税费，`pool_credit` 为实际总优惠。integrations 单独渲染 v2 的名义曲线和实分优惠，
  旧三/八变量的载荷、摘要和正文保持原形状；完整 Sent、命令、用途授权、generation 与 sending fence 不放宽。
- 静态检查：`cargo check --locked --workspace --all-targets --all-features` 及完整
  `CARGO_BUILD_JOBS=2 just verify` 通过（格式、策略门禁、工作区/独立 feature、all-features Clippy `-D warnings`、双端 WASM）。
  最终日志/退出码为 `target/seller-full-proportional-check-final.{log,exit}` 与
  `target/seller-full-proportional-verify-final.{log,exit}`，均退出 0。Rust LSP 对 20 个改动文件未返回有效回答，未记为通过。
- storage 的资产冲突识别同步精确九变量闭集，仍只定位出站资产，不授予资金或发送权限。
  本轮未新增或执行行为测试，未执行迁移、启动服务、发布制品、切换草稿或改写历史账款。
  未改变此前生产 R31/schema63，也未补差或发送邮件。

金额公式见[结算不变量](docs/contracts/invariants.md#i-07a-结算策略与命令重放必须可从权威事实重建)，
载荷闭集见[结算交付契约](docs/contracts/schema.md#结算恢复与交付)。

### 2026-10-25 卖方全额结算 v2（未发布中间方案，已被同比缩放方案取代）

以下保留当时工作区及静态检查记录，不代表当前预算不足处理逻辑。

按用户选择，仅修正后续新场次，保留买方原有优惠，不补发历史差额。domain 的 `hold_curve_v2/version=2`
保证卖方应收等于成交价；负调整时基础折扣及额外优惠全部由奖池承担，总支出为两份基础折扣。
奖池不足则整笔结算返回 `settlement_pool_insufficient`，application 不写成功命令/义务，jobs 复用原持久 workflow
保持 settling 并退避重试。已有 v0/v1 快照继续按冻结公式结算及重放，新模板拒绝 v1。

- `compensation_amount` 继续表示奖池总支出；领域 `buyer_pool_credit` 分离额外买方优惠，application 邮件及
  storage 收款证明重建共用该结果，不把卖方补足额重复扣减 CoD；原命令摘要、Sending fence、现金用途授权不放宽。
- storage 追加 `000064_seller_full_settlement.up.sql`，只扩展三张既有拍卖/结算表的版本/金额 CHECK，
  不修改旧快照、价格、义务、余额、投递或账本；本轮未执行该迁移，未启动服务、改线上配置或补发邮件。
- HTTP/DTO 路径与字段不变；A07 允许原 policy 字段标识 v2，报告按领域模式矩阵与逐标的折扣/税费/补偿总数核对。
  新模式奖池总支出可高于基础折扣需求，旧 v1 仍保留原上限；买家邮件八变量保持原形状和额外优惠含义。
- 当时工作区完整 `CARGO_BUILD_JOBS=2 just verify` 通过：仓库策略、格式、工作区/独立 feature 编译、
  all-features Clippy `-D warnings` 与双端 WASM check；最终日志及退出码为
  `target/seller-full-verify-final.log`、`target/seller-full-verify-final.exit=0`。
  Rust LSP 对 15 个改动文件无有效回答，未记为通过；未新增或执行行为测试、数据库迁移验收或真实结算。
  线上仍为此前 R31/schema63；已有 Main 第003期草稿快照仍为 v1，未经单目标确认不自动切换。

owner、配置与部署边界分别见[结算不变量](docs/contracts/invariants.md#i-07a-结算策略与命令重放必须可从权威事实重建)、
[对象契约](docs/contracts/schema.md#结算恢复与交付)与[升级说明](ops/README.md#卖方全额结算升级)。

### 2026-10-25 R31 已知 Sent MID 履约核对修复（授权发布与既有队列观察）

读取修复 owner 为 `auction-integrations` 的 `client/reads` 与 `gateway/reader`；结清、资金授权、claim/fencing
及原子审计仍分别属于应用与存储。已知 MID 原路径会在命中后继续扫描全部历史，反复耗尽履约外层 30 秒预算。
现逐页定位精确 MID，命中合法摘要即核对详情；只有完整到尾仍缺席才返回未观察到，页数耗尽仍为不完整扫描。
普通履约使用独立新鲜观察，防止共享缓存时间早于本次 claim；不失效其他读取者。资金组合保留原独占 session、
新鲜屏障与最长 30 秒期限；新发送基线、未知候选及写后新增唯一证明继续要求完整扫描。

- 冻结干净源码 `fe3214e2135d807031a9e69ee496e9bb753ed28a`，source fingerprint 为
  `032d0d408c98bf619f81570ab0496f265a29e3da17182804f66ef59d18de2846`。
  当前源码完整 `just verify`、`just release-check` 通过；API-only 与 embedded 四程序均重新构建，
  双端 WASM 因输入未变经当前门禁核验后复用。通用 Linux x86_64 GNU、`CARGO_BUILD_JOBS=2`，无 native CPU 参数。
  Rust LSP 未取得有效响应，未记为通过。R30 候选因补充新鲜观察修复被废弃，其构建输入身份门禁拒绝旧输入，未上传或切换线上。
- 归档 `auction-rust-0.1.0-fe3214e2135d.tar.gz`，SHA256
  `797552d65a35a467324c64e35a3231bfeea54481dd3be99568c8816da4311363`；稳定计划 SHA256
  `c80f2a5decd50ca04447238642a6cbb0cada36ea4e42f479be4d750b3badfa15`。
- 独立 R31 目录验证 30 个包文件及历史 63 项迁移 checksum；三角色正常停机，等待只读阶段窗口并确认无在途发送/准备，
  Worker 正常排空、退出码 0。旧 CLI 创建 schema63 完整恢复点，新 CLI 验证归档及 manifest；未执行恢复。
  备份归档 SHA256 `bb30be84c91c0add101590d784b331b4cb87fcfab6c2669565f6b720f1718b34`。
  停稳后的 88 张既有业务表在替换程序前后行数与摘要完全一致；无追加迁移，schema 保持 63。
- `02:10:39 UTC` 按 Worker → Admin → Public 恢复，四程序来自同版源码；部署退出码 0，readiness 200，
  16 项静态资源匹配。至 `02:30:55 UTC` 三角色维持同一 PID/invocation、`NRestarts=0`，日志无 WARN/ERROR，
  runtime 未决命令 0。配置、邮件预算、managed、unit/tmpfiles 及原 runtime 锁 inode/权限均未变。
- Main 原战斗页已由账号持有人结束战斗后恢复；`02:15:03 UTC` 收件轮询成功、连续失败 0。
  `01:57:18 UTC` 快照中 15 条履约为 `evidence_unavailable`；`02:17:26 UTC` 已降为 0。
  当前 114 条 `satisfied`、17 条 `enqueued/evidence_not_observed`（13 货款、3 装备 COD、1 流拍返还），
  仍等待外部领取/付款证明；4 条卖方款业务阻塞、60 条退款为条件义务，另 1 条装备 COD 履约人工核对。
  投递队列保留 3 条历史发送未知；比较完整行摘要及 attempt/fence，均保持原状，未重试或强制结清。
- 只收集 9 份闭集脱敏回执，敏感备份/配置及原始日志留在云端。证据索引为
  `target/dist/sent-mid-20261025-ops-r31/DEPLOYMENT-SUMMARY.json` 与
  `target/operations-evidence/delivery-fulfillment-health-20261025/FINDINGS.json`。
  本地 Public/Admin PID 与运行程序哈希不变、未启动 Worker；正式构建刷新了本地 Public 开发 payload，Admin payload 未变。
  本轮范围为静态门禁、部署核验和用户授权的既有队列观察；未新增或执行行为测试、浏览器测试，未新增业务样例。

外部 HTTP/JSON/事件、配置选择器及数据库版本不变；剩余待领取/付款、业务阻塞与人工核对如实保留。
运维解释见[已知 MID 履约读取](ops/mail/diagnostics.md#已知-mid-的履约读取)。

### 2026-10-25 R29 Public 竞价历史说明移除（授权单角色发布）

Public owner 移除竞价日志和装备详情表格中的额外说明，五语同步；保留仅屏幕阅读器可读的表格标题，
不改变事实、顺序、分页、列宽或竞价规则。冻结源码 `df8b037e78e496cbfb49ee36ff65c5a2d92b1d0d`，
完整 `just verify` 与 `just release-check` 通过；归档 SHA256
`115d04cac80c87e70ebc2576196c90a6670ba0bc41660f81d38cf5c8c6f48f52`。
验证 schema63 完整备份后仅替换 Public 内嵌程序，Worker/Admin 在该次切换中未重启，未迁移或修改配置。
发布后 readiness 与 16 项静态资源核验通过；该页面修改已包含在后续 R31 同源发布中。
回执索引 `target/dist/public-bid-note-20261025-ops-r29/DEPLOYMENT-SUMMARY.json`；未执行浏览器或竞价测试。

### 2026-10-25 R28 云端发布与 schema 63（授权停机迁移与制品核验）

发布 owner 为 `ci/release`、`auctionctl` 分体停机/迁移控制器及三角色装配；新增持久对象由
`auction-storage` 的 mailbox sweep / delivery owner 拥有。用户明确要求重新编译、更新线上并迁移数据库。
冻结干净提交 `a8d7d19f45cb25afeeaac2ff6d69f91ede0f81eb`，完整 `just verify` 与 `just release-check` 通过。
API-only 四程序重新编译；embedded 四程序及双端 WASM 已属同一提交、同一输入/环境/工具身份，
经当前门禁验证后复用，不把旧版本验证结果代替本次检查。通用 GNU x86_64，未使用 `target-cpu=native`。

归档 `auction-rust-0.1.0-a8d7d19f45cb.tar.gz` 的 SHA-256 为
`b912fb8e410f1f52db50ebcb5c131d076a5990e8c823554daea6f4e45e74963a`。
云端新建 R28 一次性私有发布目录，核验 30 个包文件、原 R27 磁盘/运行身份及已发布 59 项迁移 checksum。
保留旧程序、配置和 unit，Public → Admin → Worker 正常停稳，Worker clean drain；旧 CLI 创建 schema59 全库备份，
新 CLI 验证 manifest、归档 SHA-256 与归档格式。该恢复点的归档 SHA-256 为
`30312ef07d7f68b9c577884d6e75e11c16fbd70b76545b2f4f6009bb25251c0f`，未执行恢复或数据库回滚。

同版 CLI 只追加 `000060`–`000063`。84 张旧业务表迁移前后的逐表行数与摘要完全一致；
新建维护游标、Send 尝试/MID 绑定和 Take 尝试四张表，启动前均为空，不回填旧账号或发送/领取证明。
全部角色配置、shared runtime、managed 开关、邮件预算、unit/tmpfiles 保持原值；runtime 锁仍为原 inode 656282、0600。
未人工重排队、改写或重发历史未知投递，预检及后续观察均有 3 条 `manual_reconcile`。

01:08:29 UTC 按 Worker → Admin → Public 恢复，PID 分别为 255590/255611/255622；
readiness 均为 200、启动回执匹配、NRestarts 均为 0。两端 Unix socket 及 Public HTTPS 共 16 项静态资源逐字节匹配。
初始日志无 ERROR/WARN；01:10:55 UTC 再观察，三角色身份不变、无 ERROR，Worker 有 29 条 WARN，
其中 28 条显式 `failure_code=account_busy`，另 1 条 poll cycle 错误链也含该码，是同类失败的多层记录，不等于 29 次独立投递失败。
对当前 Main 加密证据做官方离线认证导出，确认有 `battle_main`、没有正常邮件 DOM；临时明文随后删除。
该战斗页会阻塞邮件读取，未让程序代操作游戏、切换 CK 或重寄未知邮件。

本地 Public/Admin PID 及运行程序哈希未变，Worker 未启动；双端开发 payload 清单摘要也未变。
10 份闭集脱敏回执与完整摘要位于 `target/dist/schema63-20261025-ops-r28/`；
数据库归档、敏感配置备份和原始日志仅保留云端私有目录。本轮验证为静态/发布门禁、停机迁移、
就绪/资源/日志核验及本地加密证据检查，未额外触发注册、竞价、真实补发、论坛或浏览器业务测试。
升级包含会话及浏览器协议修复，已有页面须刷新，Admin 须重新登录；源码升级前提见
[部署说明](ops/README.md#发送引用索引与稳定性修复升级)。

### 2026-10-24 全项目复审与通用构建优化

基线为干净的 `b7db347138931d95ca20a3ccb94c942e746bf1cb`。覆盖 20 个工作区包、1415 个 Rust 文件，以及
SQL、注释、48 份正式文档、依赖清单、构建工具、CI/发布脚本和浏览器资源。方法为全路径风险扫描、
按 owner 审查关键调用链及定向运行，不是逐行形式化证明或全业务验收。本轮修改 121 个 Rust 源文件；
12 项已有测试及其 8 个测试模块保持原文，没有为减少数量删除真实边界保护，没有新增或运行永久测试套件。

#### 源码修复与不变量

- domain/application：装备 header 借用分词，密文/nonce getter 改为切片并迁移全部调用者；删除四个无消费者竞价常量。
  邮件起拍价分词、文本规范化、附件移交及安全展示减少中间集合、复制和无意义摘要；无附件 key 仍检查 URL key。
  名单判重由平方扫描改为借用集合；告警一次选取可容纳的最大非空前缀，保留排序、ordinal、正文、摘要和原错误优先级。
- storage：结算 `final → settling` 领取从 8 次收敛至含首次最多 3 次，保持 Serializable、稳定命令及明确中止分类；
  拍卖/补偿列表先分页再汇总，结算只联接活动代理，withdrawn 上的活动授权仍阻断。
  `sweep_retries` 不再被自动 claim/outcome/expired 清零，Admin 显式 reset 语义不变；Recovery relist 持久事件使用真正的
  auction `state_version`，payload 的 preview revision 不变。入站时间按已有 PostgreSQL 微秒约定比较；
  Admin 邮件分页改用只读 Repeatable Read，摘要/JSON 借用绑定及固定证明 SQL 缓存不改变事实或权限。
- integrations：通知模板在追加前执行既有 64 KiB 正文上限，拒绝合法模板的巨大变量展开，不截断或发送部分内容；
  Sent lookup 明确失效后撤销原 Cookie，owner 检查和失效回写共用原 deadline。
  论坛导航继承正文/签名状态，避免逐链接重扫祖先；分类排序、BBCode、分页身份、金额/附件辅助及 nonce 解密移除重复分配。
- jobs/HTTP/observability：周排期在只读等待后复核 drain，不再新建后续轮次或邮箱命令，已提交结果保留。
  Dashboard 在同一请求任务并发等待独立 owner，取消收口全部读取；XFF 不建整链 Vec，仍校验完整地址链。
  W3C carrier 借用头字段，OTLP 注释按完整 endpoint 原样传入修正；未改缓存真值、WS/Cookie/限流或指标契约。
- CLI/config/装配：灾备 hash/fsync/限量读取统一使用不跟随 symlink 的非阻塞描述符校验；
  当前格式和 v28 manifest 的结构与 SHA 绑定同一次原始字节读取。Sent 诊断只做一次已包含 Shared 的 active 凭据查询；
  hex 输出预分配，Admin 装配删除不可达的其他数据库分支。未放宽文件权限、精确目标确认、凭据和资金边界。
- Web：Recovery、世界启停、改名回滚接入稳定未决宿主，冻结 UUID/完整负载及首次 fence；
  unknown 后 stale 不换新意图。Public 修复世界/历史辅助读取、会话错误展示、隐藏论坛轮询、浮层旧正文及时间格式器重复构造。
  实际浏览器另证明单个静态组件 key 不足以保证身份 Scope 销毁：Public 订阅显式依赖用户 UUID，
  Admin 认证树及其他有状态目标使用真实 keyed iterator，只需重读的组件改为显式资源依赖。
  冷启动 Admin query 不再丢失；对账条件 option 的显示选择与提交动作一致；不安全 return 不再留下空白登录页。
  CSS、五语键、业务规则、HTTP 字段及公开事件 payload 未变，未新增迁移。

#### 通用构建与依赖

本地 dev 使用 `debug="line-tables-only"`，保留增量编译、断言和文件/行号回溯；完整调试可显式恢复。
CI verify/MSRV 不生成调试信息，受控工具回执有效时不再运行源码构建依赖的 apt 安装。生产仍为
`opt-level=3`、Fat LTO、单 CGU、非增量和 `panic=unwind`，WASM 仍以体积为先；
没有加入 `native`、PGO/BOLT 或未经代表性负载证明的“最优性能”承诺。
固定 Rust/MSRV 均已默认使用 GNU x86_64 LLD，不再建议多余的全局 linker 覆盖。

发布指纹批量读取文件 metadata/SHA-256，并纳入 Cargo 层级配置、C/C++ 参数；阶段复核同时验证环境，
修复修改 `.cargo/config.toml` 或编译参数后仍复用旧制品的问题。旧指纹自动失效，不改变资源隔离或归档白名单。

依赖核对覆盖 262 项直接声明；cargo-machete 未发现未使用的直接库，只移除无消费者的 Axum `original-uri` feature。
四份锁文件均与基线逐字相同，应用仍为 536 个锁条目，不为减少包数强行统一上游不兼容版本。
独立联网 RustSec 快照为 1352 条公告，提交 `21d909e4c8bd4e14c7aea58bf980c881a0053ab1`：
应用与 cargo-audit 锁均零漏洞/零警告；dx 仍为 13 个漏洞匹配项、11 个警告，sccache 为 5/9。
相关工具包仍在 GNU 活跃图，不加入公告忽略项。应用 MPL 和 dx 的 GPL/IJG/MPL 分发义务见
[工具风险与许可边界](ci/tools/README.md#上游公告与实际使用范围)，安装回执不消除这些风险。

#### 实际验证与局部测量

- 最终完整 `CARGO_TARGET_DIR=target/audits/project-review-20261024/verify-build just verify` 通过，
  包含策略、格式、workspace all-targets、独立 PostgreSQL/Redis feature、Clippy `-D warnings` 和双端 WASM。
  Rust 1.94.1 workspace all-targets/all-features 通过；ShellCheck、Actionlint 和依赖检查通过。
  受控工具两档重建并校验；原生通用 release `auctionctl --features postgres` 完成链接及 `--help` 运行，
  不等于完整四程序发布包或 Actions 制品上传已通过。
- 真实领域/应用/集成源码 before/after：598 个 header 输入、18,270 个应用等价场景、1,125 个邮箱规范化输入、
  2,744 个集成规范化输入通过；另覆盖加密往返、原信封不变、错误 AAD/篡改拒绝和模板 UTF-8/字节边界。
  这些是临时程序，不是新增永久行为测试。
- 私有 Unix socket PostgreSQL 18.6：两版真实 migrator 的 63 项迁移；领取前及提交期 `40001/40P01` 重试
  8→3、普通约束错误只一次、失败无状态/账本/审计/事件残留、第三次成功及有效租约排他均通过。
  16 组源码 SQL 结果等价，保存 8 份真实计划；每版 18 组拥有/借用参数往返及 NUL 拒绝一致。
  Take 未知/过期计数由反复为零变为 `0,1,2` 后停止，原身份/fence、终态计数与 Admin reset/幂等通过；
  relist 的 auction/event 从 `8/4` 变为 `8/8`，后段失败整体回滚。纳秒入站由自身冲突变为合法微秒持久化，
  重放保留首次时间。并发插入期间，Admin 分页的 total/行仍来自同一只读快照。
- 本机运行：HTTP 两个各 300 ms 的读取在 500 ms 预算下由 504 变为 200；更慢读取仍超时并释放全部 guard，
  认证/query 优先级、嵌套 Uri/MatchedPath、404/405/Allow/HEAD/no-store 保持。Worker 8 组前后场景确认 drain 边界；
  只读失效邮箱由两次外呼变为一次，后续明确凭据不可用。
  CLI 文件 48 次调用、每版 168 项配置及 18 项凭据场景通过；真实 pg_dump/pg_restore --list 的 manifest 替换
  验证覆盖当前格式与 v28 格式，但不证明 v28 恢复/接管。FIFO 摘要不再挂起，32 字节 hex 由逐字节分配变为单次分配。
- Chromium 加真实调试 WASM：Public 九组主路径通过，Admin 三组 frozen intent、六组 actor/CSRF 隔离及新身份新工厂、
  三组暂时会话失败保留通过；固定绝对截止在两版都能三次 GET 后回登录，不把此等价观察冒充旧版故障。
  附加 Public 生命周期 8/8、Admin 生命周期 8/8（167 项检查）通过。冷启动 URL/控件/首个 API 请求保留世界与数量；
  Requeue 的显示与实际 retry 请求一致。Xvfb 原生隐藏标签 31 秒窗口由 11 个论坛周期请求变为零，恢复可见后继续；
  同场换号关闭旧 WS，新连接保留 `after=7`；五语时间和旧浮层关闭/键盘重开已核对。

下表是明确输入下的分配请求/构建步骤测量，不是进程 RSS、全服务吞吐或生产 p99：

| 局部路径与输入 | 修改前 | 修改后 |
| --- | ---: | ---: |
| 构建输入指纹，同一不可变源码，三次中位数 | 5.009 秒 | 0.185 秒 |
| 告警 64 条压至一条前缀，分配次数 | 4288 | 1 |
| 1500 行分类排序，分配次数 | 13838 | 1 |
| 模板重复展开，额外分配请求峰值 | 18874368 字节 | 73728 字节 |
| 100 个时间单元格切换五语，formatter 构造次数 | 500 | 5 |

SQL 计划中，Public/Admin 的聚合场次由 206/240 限至各 25，补偿账本读取行次由 824 限至 100；
终态代理历史连接输出由 3002 降至 100，结果仍为 `89/0/2`，活动授权继续阻断结算。
单次合成数据计划不能外推生产负载性能。

早期 Admin 漏删一处恒真后端条件、RSX 所有权错误均在静态门禁发现并修复。临时探针的 DTO、选择器、权限、NUL
预期及首次 `dx bundle --profile dev` 调用错误已保留回执；共享 target 的跨源码树构建曾生成相同旧 WASM，
分离 target 并核对实际 SHA 后才接受新版浏览器结果。部分基线场景本就通过，Public 的三个 CDP 超时不计为已定位源码失败。
LSP references 仍以 code 0 退出，使用调用图、精确引用及实际编译补充；未用空 affected 结果代替测试审查。
48 份正式文档、1297 个站内链接/锚点的一次性结构核对通过，没有新增永久文档门禁。
私有 PostgreSQL、fixture、浏览器和显示服务均已停止；临时源码、WASM 样本和含虚构密钥的目录已清理，
保留脱敏 JSON、日志、截图、公告快照及可复用静态检查/工具缓存。

证据入口：`target/audits/project-review-20261024/evidence.json`。未提交、推送、部署、访问真实论坛/游戏，
未迁移运行库；原生/浏览器/数据库证据均限所述隔离范围。正式升级前提见
[本轮升级边界](ops/README.md#性能与页面生命周期修复升级)。实际 Actions 耗时、完整发布归档、生产并发/性能及灾备切换仍未验收。

### 2026-10-24 论坛主题增加 Auction 前缀

主题标题改为 `[Auction] {Main|Isekai} Equipment Auction - Session {sequence:03} ({state})`；
期号至少三位，状态仍取同源快照。用户明确选择完整格式并放宽本地限制，renderer 与论坛客户端共用 80 字符预算，
不截断期号；最大合法 i64 期号与最长状态的完整标题为 76 字符。远端是否接受长标题尚未验证。

原无前缀状态标题保留为精确只读对账候选，兼容旧短标题、旧无状态完整标题和 HV# 摘要标题。
应用端候选上限由 15 调整为 22，连同当前标题最多 23 个，仍合并一次版面 GET；历史形式保留原长度边界。
不修改正文、拍卖状态、未知写入门禁、HTTP 字段或持久化结构，不批量改写既有主题。

完整 `CARGO_TARGET_DIR=target/audits/source-audit-20261023-build just verify` 通过。
临时原生 probe 实际执行两世界、七状态及五种期号的 70 个 renderer 场景；本机 HTTP 对端验证新/旧标题定位、
同一主题的历史别名、冲突主题拒绝、80/81 字符边界、候选数量边界及截断标题不误匹配，共 5 次 GET、无 POST。
临时 probe 的导入路径和无关正文措辞断言曾失败，已仅修正探针并保留回执；正式行为测试未新增或运行。
证据：`target/audits/forum-title-prefix-20261024/evidence.json`；探针和独立构建目录已清理。
本项未提交、推送、部署或访问真实论坛；工作区原有审计改动保留，不把本项无新迁移解读为整个工作树无需升级迁移。

### 2026-10-23 最新源码、性能与稳定性审计

基线为干净的 `67342f9df5ff99609347e5dd9b761abba4e24b27`。覆盖 20 个工作区包、修改前 1414 个 Rust 文件，
以及相关 SQL、配置、CI/发布脚本、CSS、五语资源和正式文档。方法为全路径结构/风险扫描、关键调用链审读及定向运行；
不是逐行形式化证明，也不是全业务或生产验收。本轮修改 141 个正式路径，其中 91 个 Rust 文件；全部 12 项现有行为测试保留，没有为减少数量删掉真实边界保护，
未新增永久测试、未运行既有行为测试套件。本轮另用执行后删除的临时探针取得下述证据。

主要修复与 owner：

- domain/application：结算曲线先校验一次并用二分定位锚点，曲线处理由 `O(N×M)` 降至 `O(M+N log M)`；
  公开入口仍严格校验，完整计划、义务、整数取整和金额守恒不变。旧 Go 结算摘要接受 RFC3339Nano，
  并保留已发布 Rust AutoSi 重放；旧 compose 改比精确 EID，Credits 条目不再误作装备计数。
  移动已消费装备快照、借用规则 Markdown 校验、去除逐字节 hex 临时字符串。
- application/integrations/storage/jobs：`000063` 在 Take 的 Sending/审计事务冻结原 world/MID/凭据尝试，
  prepared 固定同一 session；维护的缺失观察也携身份，恢复同时复核原 credential ID/version、当前 generation 和候选 fence。
  重启 generation 与原尝试分开；旧无绑定及配置 Cookie 无持久身份的未知 Take 仍人工核对，不回填或自动重发。
  Take 重排清除旧 `claimed_at`，避免 pending 被 Admin 安全投影拒绝；Send `000061` 和 Return 证明不弱化。
- storage：凭据失效/启停时间保留创建和既有更新时间下界；删除旧结算只读分支未使用的 bids 扫描，
  固定 SQL 只组合一次，摘要及心跳 JSON 改借用。`000062` 的非空外部引用部分索引加速旧 MID 碰撞检查，
  不新增唯一约束，不移除 Serializable 谓词或历史碰撞门。
- integrations：新论坛主题 action 在 POST 前核对路径/动作/目标；PM 写后明确失效撤销 Cookie，但仍保留 unknown。
  删除单 Cookie 下不可达轮换循环，统一无临时集合的金额解析；超大读取预算失败关闭而不 panic，
  AEAD 缓冲预留标签/头部容量，原密文、AAD、摘要与错误分类保持。
- jobs/Worker：PM 使用领取前绝对截止，迟到回执不再启动下一阶段；PM 退出、panic、取消触发原 readiness 撤销与有界 drain。
  Fulfillment 过期/零预算不首次 poll 外呼；取消优先的指标合并复用既有规则，邮箱上下文直接转移。
- HTTP/装配/CLI/config：WebSocket 只在升级前受原请求预算约束；Public/Admin 初始化可被终止信号取消；
  CLI/server readiness 只接受完整 CRLF 状态行。TCP peer 改在请求扩展注入，避免逐连接重装 Router。
  CLI 直接消费联合预检快照，不二次读取未共同校验的配置；OTLP 预检与 exporter 均在严格百分号解码后校验。
- Web：Public 两项偏好的 unknown 跨会话 GET 瞬时失败保留，换号仍清理；可见预览跨开场时间补读权威状态。
  Admin 写前绑定原 actor/CSRF，临时故障禁写但保留 pending，迟到邮箱反馈不串目标；结算展示去掉中间深复制。
  实际浏览器还发现并修复时间语言滞后、窄屏命令 ID 溢出；两端改用显式 I18n 时间格式，删除旧 DOM 语言入口，
  截断宽度同时受容器约束。邮箱键五语标签修正，业务表结构和 HTTP/事件字段不变。

依赖核对另发现 direct tungstenite 0.30 与 Axum 实际使用的 0.29 不同型，静态编译不能发现 downcast 恒不匹配；
已对齐 0.29 并移除多余锁条目，恢复超限帧约定的 1009 空原因关闭。三个工具锁统一 Tokio 1.53.3，
处理 [RUSTSEC-2026-0386](https://rustsec.org/advisories/RUSTSEC-2026-0386.html)，并更新四项可兼容的 yanked 依赖。

实际验证：

- Rust/cargo 1.99.0：最终完整 `CARGO_TARGET_DIR=target/audits/source-audit-20261023-build just verify` 通过，
  含策略、格式、workspace all-targets、独立 PostgreSQL/Redis feature、Clippy `-D warnings`、双端 WASM。
  Rust 1.94.1 的 workspace all-targets/all-features 检查通过；ShellCheck 与 Actionlint 通过。
- 真实领域/应用源码 before/after 每版 808 个场景：745 个完整计划、14 个插值、10 个历史摘要、8 个 compose、
  19 个 Take 应用边界、4 个 hex、8 个规则输入。计划/插值/hex/规则结果完全一致；记录型窄端口只证明应用编排。
- 私有 Unix socket 的 PostgreSQL 18.6：完整 63 个迁移、重复迁移及全部原文件 SHA-384 通过；
  Take/Sending/审计原子回滚、追加保护、原 fence 解析、跨账号/版本/generation 拒绝和两个真实行锁竞争通过。
  同凭据合法重启恢复、Admin pending 投影、配置 Cookie 即时 Take、旧缺绑定拒绝、Send 唯一 MID/重放与 Return 回归通过。
  凭据真实旧/新 UPDATE 复现旧 CHECK 失败与倒退，新幂等/时间下界/CAS 通过并回滚。
- 本机合成 HTTP/原生探针：63 个论坛/PM/URL/指纹/信封场景通过；金额解析 137257 个短输入加 10 个边界与旧算法一致。
  邮箱共享读取、取消、fresh 屏障、512 次真实请求预算、401 一次失效、Take 原 session/fence 撤销及带身份观察通过。
  Worker before/after 各 18 个真实 Tokio/窄端口场景通过；迟到 fence 的发送 poll、零预算应用 poll 均从 1 降至 0。
- 真实 CLI 配置生成/权限与预检通过；配置冲突前后退出码为 0/2/0，11 组 readiness、19 组 OTLP 接受集合通过。
  Server 原函数另有 11 组状态行前后断言。真实 TCP/Unix peer/XFF、挂起握手 504、长连接存活及 1001 drain 通过；
  4097 字节帧在历史基线为无 Close 断开，对齐依赖后收到 1009。两 API 的 TERM/INT 在假 PostgreSQL 握手阻塞及
  真实 PG 表锁未释放时均及时非零退出、释放监听；真实锁等待 WS 在约 1 秒返回 504，解锁后同 token 为 401。
- 受控 dx 构建真实双端调试 WASM，在 Chromium 合成 API 夹具中复现旧 Public 命令丢失、旧 Admin 跨会话写入及反馈串目标；
  新版保留同 UUID/负载重试，换号/退出隔离，同身份续期可写，临时失败禁新写且草稿恢复。预览及时补读，
  服务端未开场不提前开拍，匿名不建认证 WS；Xvfb headed Chromium 实测隐藏期间零补读、恢复可见后一轮。
  五语邮箱标签/时间、320px 容器与截断修复均实际核对。夹具不证明真实账户、邮箱或生产业务流程。

20 万合成 delivery、PostgreSQL generic plan 的单次定向观察如下，不是吞吐/p99/RSS 基准：

| 路径 | 修改前 | 修改后 |
| --- | --- | --- |
| 无命中旧发送引用碰撞查询 | Parallel Seq Scan，24.469 ms | 部分索引 Index Scan，0.052 ms |
| 旧结算只读核验 | 执行未消费的 bids 子查询 | 不访问该子查询，消费字段相同 |

最终 RustSec 快照为 1352 条公告，提交 `8a5b8090ef20c9c1b425f3373f28b0b23cc5c7a0`。
应用 536 个锁条目与 cargo-audit 独立锁均无漏洞/警告，cargo-machete 未发现未使用直接依赖。
dx 仍有 13 个漏洞匹配项、11 个警告，sccache 为 5/9；相关漏洞包仍在 GNU 活跃图，不能宣称工具无风险。
四项 MPL-2.0 包的分发义务仍须履行。两档工具均已重建/校验，剩余风险与限制见[工具审计](ci/tools/README.md#上游公告与实际使用范围)。

早期 Public 借用/RSX key、Clippy 借用警告及临时探针/构建参数接缝失败均保留回执，修复后才记录最终通过；
未降低 lint 或公告门禁。CodeGraph 已同步；Rust LSP references 仍以 code 0 提前退出，使用调用图与全调用点核对，
空 affected 结果不替代测试审查。47 份正式文档、1290 个站内链接/锚点的一次性结构核对通过，不新增永久文档门禁。

脱敏证据入口：`target/audits/latest-source-review-20261023/evidence.json`。本轮没有提交、推送、部署、真实论坛/游戏调用，
没有迁移运行数据库；临时环境和探针按本轮清理记录回收。上线须按[稳定性修复升级](ops/README.md#发送引用索引与稳定性修复升级)
停旧 writer、追加 `000062`/`000063`、同版更新并刷新页面。不承诺 HTTP/2 CONNECT、完整现金授权、完整 Worker 进程 drain、
真实远端 HTML、生产并发、生产性能或灾备验收。

### 2026-10-23 邮箱共享读取与冻结发送证明

用户批准隔离 PostgreSQL 18 与本机合成邮件对端验证。修改前工作区干净；本轮未执行 Git 提交、推送或部署，
未读取真实邮箱、修改现有凭据或迁移运行数据库。

- integrations：Inbox、Read Mail、Sent 共享有界读取与详情请求合并；普通读取不等待 compose 操作锁。
  列表扫描和改变已读状态的详情分阶段协调，Read Mail 只恢复已知 MID，不重放历史入站；
  POST 开始、结束和取消均形成新鲜屏障，失效凭据/owner 的旧观察拒绝继续使用。
- application/storage：新增 `000061`，实际发送只渲染一次；主题、正文摘要、资产矩阵与 Sending、节奏和审计同事务冻结。
  原发送 fence 与当前 CAS fence 分开，过期隔离后仍能核对原尝试；唯一 MID 绑定及强证明谓词贯通履约、资金和恢复。
  正文、登录 key、Cookie 不进入新证据表，历史缺证据不补造、不自动重寄。
- jobs/Worker：各已配置世界的 Inbox、Read Mail 与 Sent/草稿维护并行、分别有界；一方失败不撤销其他 owner 的事实。
  普通观察不授予现金或库存提交权限，原独占授权上下文、generation、租约和绝对截止继续生效。
- Admin：准备阶段 `failed/invalid_payload` 不伪造 `sending_at`；服务端安全投影与浏览器 Rust decoder 接受同一明确未写入形状，
  矛盾写入标记、未知发送、缺失领取时间、倒序时间或外部引用仍拒绝。

复审中实际复现并修复五类边界：临时读取错误缓存阻断真实重试、成功提醒仍被旧 Recovery 阻断、
同内容已领取候选被跳过而错绑另一封、收件人大小写使跨批次竞争漏查，以及准备失败破坏 Admin 投影/解码。
保留失败前证据，不把后续通过覆盖为历史全绿。

实际验证：

- Rust/cargo 1.99.0：最终完整 `just verify` 通过，包括策略、格式、workspace all-targets、独立 feature、
  Clippy `-D warnings`、Public/Admin WASM；Rust 1.94.1 的 workspace all-targets/all-features 静态检查通过。
- 独立 PostgreSQL 18.6、私有 Unix socket：**44 项场景通过**。覆盖全部 61 个迁移的 checksum/幂等与约束、
  发送摘要/围栏/审计原子提交及回滚、全部发送来源的冻结恢复、MID 冲突、原 fence 恢复、真实行锁等待跨 TTL、
  取消/丢弃应用确认、资金关联与 unknown/spent 收口、提醒父子投影及严格下游证明。
- 本机合成 HTTP/typed 对端：**18 项邮箱流程通过**。共享列表/详情各一次网络读取、持 compose 时三类读取继续、
  等待者取消、凭据隔离/撤销、新鲜屏障、并发/请求/响应/保留字节边界、渲染冻结、重复 MID 拒绝及真实待读账本恢复。
  内存场景用同一代次的并发响应验证，不把跨 30 秒复用窗口的顺序读取总量当作同时保留量。
- 额外真实 Admin 存储投影及矛盾写入标记拒绝通过；实际 Admin Rust decoder 的临时原生探针失败前/通过后均留证，
  该临时测试已移除。未新增永久测试，也未运行已有行为测试套件。

证据索引：`target/audits/mailbox-redesign-20261023/evidence.json`。临时 PostgreSQL 已退出，数据库目录及探针源码已清理，
只保留脱敏 JSON 证据。没有真实邮箱协议、完整现金 reserve/授权、PostgreSQL 线级 COMMIT 故障、浏览器视觉、
RSS/120 秒到期实测或生产负载验收声明；资金验证范围限真实终态记账及关联保护。
升级须按[共享读取与发送证明升级](ops/README.md#邮箱共享读取与发送证明升级)停止旧 writer、追加迁移并同版更新。

### 2026-10-23 全项目性能与稳定性复审

基线工作树干净，HEAD 为 `ff5bb4591dc0d81ef2d77251e8e53898ef98b399`。按 11 个互斥 owner 切片覆盖
20 个包、修改前 1410 个 Rust 文件，并核对相关注释、SQL、Web/五语言、配置及 CI/发布脚本。
覆盖方式为全路径结构/风险扫描与重点调用链审读，不是逐行形式化证明。调整 124 个源码路径
（123 个 Rust 路径含删除 1 个未装配模块，另 1 个 CSS 仅删过期注释），父级另改 Admin feature、发布脚本和 CI 缓存门禁。
保留 12 项现有行为测试，只精简一处无关 fixture；不新增永久测试，也未运行已有测试套件。

主要 owner 与落地结果：

- domain/application：普通命令第二条代理反击的确定性身份与机制绑定、收款人原字段互斥、未知 Take 的精确资产矩阵；
  统一 Go JSON 摘要、栈内 token 解码、附件/投稿去重。竞价规则、历史合法金额、账本和审计不变。
- storage：会话在池等待/行锁等待后复核截止；16 个论坛、告警、recovery、履约及 PM 入口先锁行再检查实时租约；
  迁移/接管专用连接在申请 session lock 前标记退出关闭。补齐授权分页唯一尾键，只缓存不可变迁移验证视图，
  删除 readiness 冗余查询；义务索引、关闭计划、资金二分查找及非结算论坛历史分支减少重复工作。
- integrations/jobs：金额脚本不明改写、系统框外拒绝诱饵、残缺 Take 详情均失败关闭；读取 deadline 不能放大配置预算；
  明确关闭传输自动重试并标记敏感 Cookie，写后登录失效仍保留未知写入。删除单凭据死轮换与正文副本，
  批次/附件用借用集合去重，控制 socket 增加可信祖先链、权限写前身份门禁及正确析构顺序。
- HTTP/装配/运维：普通、发现及 pending upgrade 的 WebSocket 纳入原 10 秒 drain；control 提前退出、panic、取消触发
  Worker 收口。systemctl 输出在读取时限界并回收子进程，stderr 并发排空；pg_dump 保留原生路径字节，
  tracing flush 只处理有限队列前缀，Public/control socket 冲突在预检拒绝。
- Web：本地排序和 Outbox 分区查询隔离；凭据删除、邮箱及 Dashboard 命令不随展示卸载丢失；
  最后计时分片结束仍检查可见性。修复 Markdown 围栏，实时事实只清理淘汰键，补偿流水只传展开行，
  装备标色 getter 改借用。无视觉声明、五语言键、HTTP/事件字段变化。
- CI/依赖：原生复用判定取消紧邻的第二次全输入扫描；cargo-machete 缓存按其真实裸版本号比较，避免无谓强制重装。
  Admin 移除未用 UUID v5，独立 WASM 活跃图从 177 到 176 个节点，移出 `sha1_smol`；根及工具锁文件不变。

定向分配/工作量观察，仅代表相应隔离输入，不是生产吞吐、RSS、p99 或整体体积基准：

| 路径与输入 | 修改前 | 修改后 |
| --- | ---: | ---: |
| 五字段 wildcard cron 规范化分配次数 | 7 | 2 |
| 含 1000 个 sender 的告警 world 成员判断分配次数 | 1004 | 0 |
| 200 条装备标色规则 getter 分配次数 | 2001 | 0 |
| 有效非 `start` Public 游标临时分配次数 | 2 | 0 |
| 同一次原生制品复用的完整输入扫描次数 | 2 | 1 |

实际验证：

- 完整 `CARGO_TARGET_DIR=target/audits/source-audit-20261023-build just verify` 通过：
  策略、格式、workspace all-targets、独立 PostgreSQL/Redis feature、Clippy `-D warnings`、双端 WASM。
  Rust/cargo 1.99.0；独立 Rust 1.94.1 的 `cargo check --locked --workspace --all-targets --all-features` 通过。
  随后文档/示例注释及 CI 缓存条件同步，受影响的 `actionlint`、`all fmt` 再次通过。
- 8 个 shell 脚本 `bash -n`、ShellCheck 0.11.0、Actionlint 1.7.12 通过；修改后的真实 cargo-machete 缓存步骤复用了
  已安装 0.9.2，没有执行 install。最终 cargo-machete 未报告未使用直接依赖。
- 独立 PostgreSQL 18.6、仅私有 Unix socket：Admin/Public 等锁跨截止旧查询各接受 1 行，新查询 0 行且时间字段不变；
  正常、永久、撤销、错误 CSRF、inactive、并发撤销/延长及迟到只读路径符合断言。
  16 个租约入口 × 前后版本 × 有效/跨截止/旧 token 共 96 项通过。
  迁移取消前后证明旧 session 可留锁、新连接关闭释放；真实 `DatabaseStore::migrate` 的 `Dirty(1)` 失败保持分类且不留锁。
  完整/旧前缀/缺口/失败/checksum 损坏及兼容历史校验通过；未结算 generic plan 的 bids 子计划实际执行次数为 0。
- 原生临时探针实际执行摘要/编码等价、重放/资产/模板拒绝、集合边界、论坛/邮件 parser 与仅回环 HTTP、
  control 权限/替换/drop、子进程双管道与回收、非 UTF-8 路径、SIGTERM/drain、持续补入 tracing flush。
  真实 auctionctl 预检的合法配置返回 0、Public/control 冲突返回 2，没有连接业务数据库。
- 真实回环 Hyper/Axum/Tungstenite：普通与发现 WS 在 HTTP 结束后仍等待，实际收到 1001 后排空；
  pending upgrade、失败/取消、迟到注册及 256 次最后许可释放竞争通过。授权端口是本地 fixture，不证明数据库鉴权。
- 使用受控 dx 构建两端独立调试 WASM，在真实 Chromium 中以仅回环的合成 API fixture 验证：
  Users 排序重排而 GET 数不增；Outbox 单区翻页只新增该区请求，切 poll 不重取；
  删除/邮件/Dashboard 未决命令跨卡片替换或快照错误保留原 UUID/版本，后续明确拒绝不清除 unknown；
  邮件成功只重读当前邮箱，不重读入站列表。五语言保留同一意图，320px 保留表格结构且无横向溢出。
  Xvfb headed Chromium 实测最后分片转隐藏时 Admin/Public 均不新增轮询，恢复后仅执行一轮；
  Markdown 与补偿展开/等式实际渲染通过。Public 的请求取消 `ERR_ABORTED` 保留在回执中，未观察到未处理 JS 异常。

依赖审计使用 1350 条 RustSec 公告：应用 537 个锁图条目及 cargo-audit 独立锁均无漏洞/警告。
dx 工具完整锁仍有 13 个漏洞匹配项、12 个警告，sccache 有 5 个漏洞匹配项、12 个警告；这些漏洞包仍在相关 GNU 活跃图，
不能把受限调用路径当成依赖无风险。未添加忽略项、未跨不兼容范围强替工具内部库；限制见
[构建工具审计](ci/tools/README.md#上游公告与实际使用范围)。四项 MPL-2.0 依赖的分发义务仍须履行，不构成法律合规认证。

脱敏证据入口：`target/audits/source-optimization-20261023/evidence.json`，含 owner 清单、原始通过/失败回执、
依赖图及浏览器截图。早期临时探针的缺依赖、截断快照、模块定位等装配问题仅修复于 target，未放宽正式门禁；
headless 无法切换真实可见状态后，改用 headed 浏览器完成该项验证。CodeGraph 已同步；LSP references 仍不可用，
空 affected 结果不能替代人工核对的 12 项已有测试。

临时探针源码、独立构建输出、数据库和浏览器 profile 已清理，所有本轮服务/标签页已关闭，仅保留 JSON/文本/图片证据。
本轮没有提交、推送、部署、真实论坛/游戏调用或执行项目迁移 SQL，没有新增迁移；
既有同版升级与 `000060` 要求仍有效。升级前检查 control 目录的 group 写权限及非法模板括号。
上述验证不是完整业务事务、真实外部协议、HTTP/2 NACK、生产并发或性能验收。

### 2026-10-23 公开竞价历史完整展示（静态验证）

owner 为 `auction-storage::database::bid_log` 和 `auction-web-public`。公开竞价日志的明细及 total
取消“同一普通命令被代理立即反击时隐藏普通行”的条件；普通报价与代理回应分别保留，不改原五字段降序、
cursor、可见性、筛选或逐条分页。分页边界仍可能分开同事务两条事实，不合并或补造记录。
普通 Web 行明确显示来源标记，两处竞价表增加原生 caption；领先状态改为“当前领先／当前未领先”，
不再把已接受但未领先的报价读作无效。`en/zh-CN/zh-TW/ja/ko` 同步，不公开代理上限。

前置授权只读诊断在云端 Clo09（`ea99163f-4db5-4004-b12b-451b142e3bf1`）核对到 14 条原始事实、
9 条公开行，缺少的 5 条普通出价均与另一账户的代理回应在同一命令内；持久审计及对应 Public POST 日志相符。
证据位于 `target/operations-evidence/lot-proxy-diagnosis-20261023/`。这是修复前 R27 的线上观察，
不是新代码运行回执；原数据库事实完整，无需补写出价或更改价格。

本项未新增 HTTP 路径、JSON 字段、事件、配置、迁移或依赖，竞价规则与命令结果不变；
上线时配套更新 Public API 与 WASM。当前工作树仍包含前轮审计修复和 `000060`，
本项无需新增迁移不代表整个工作树可以跳过既有[升级门禁](ops/README.md#全源码审计修复升级)。

实跑 `cargo fmt --all`，以及独立 target 的完整 `just verify`（策略、格式、工作区 all-targets、
PostgreSQL/Redis feature、Clippy `-D warnings`、双端 WASM）和 Rust 1.94.1
`cargo check --locked --workspace --all-targets --all-features` 均通过。
验证使用 `target/audits/source-audit-20261023-build` 与 `target/audits/source-repair-20261023-msrv`，
脱敏回执为 `target/audits/bid-history-transparency-20261023/evidence.json`。
CodeGraph 已同步，affected 未找到关联测试；RSX 组件调用未被 callers 完整识别，已按实际调用点核对并报告工具限制。
Rust LSP references 仍以 server exited code 0 结束，不能算作引用检查通过。

本轮未新增、扩展或运行行为测试，未执行新代码的数据库/浏览器回归、发布构建、迁移、部署或 Git 提交/推送。
此前线上只读诊断不替代修改后的页面和分页运行验收；既有工作树改动与历史验证记录保留。

### 2026-10-23 全源码审计修复（静态验证）

修复本轮对 20 个工作区包、修复前 1406 个 Rust 源码文件及相关 SQL、Web/译文、配置与发布脚本审计列出的全部 37 项：
1 项依赖公告、30 项 P2、6 项 P3。实现按既有 owner 落地，不把静态通过写成行为或生产验收。

- `auction-domain` / `auction-application`：历史 Slow 的有效 1B 上限与同价优先、事实数量、
  `cod_return_limit` 冻结、暂时装备读取错误的重试分类。
- `auction-storage`：Admin/MM 大小写身份定位与锁后聚合时间、拉黑/会话锁序、删除凭据后的历史命令重放；
  COD 重寄固定原 v1 七天窗口并追加同事务 Delivery 审计，WithdrawnReturn 图与来源 Take 闭合；
  完整迁移历史的 retention 判断、全量 realtime lag 聚合。
- `application` / `storage` / `integrations` / Worker：新 pending 和旧 prepared 退款都要求真实同账号、正确世界 Armory
  正向库存与原提交截止；移除公开装备详情伪 custody。合法退信 MID 仅按严格矩阵投影，不能充当 Sent 证明。
- `auction-jobs` / 邮件维护：整轮共享 fetch deadline、持久公平轮转、20/200 边界及一次性 discard 围栏；
  空 Sent EID 明确拒绝，不再索引空集合。
- 论坛 owner：真实 POST 进度区分纯读取超时与写后未知；稳定 post ID/原负载不随可变楼层重新编号；
  当前连续楼层允许在完整扫描与 CAS 下下降，pending 仍阻断；真实 credential scope 贯通外呼并在 LIMIT 前筛选。
  目录事实允许合法非整千金额，用户出价网格不放宽。
- Public / `auction-web-shared` / HTTP：普通会话 GET 不再写 Cookie，显式 PUT 与登录、退出、偏好保存共用原生 Web Lock；
  未知命令在快照故障和 Live/Final 切换时保留；Final 合法动作、权威跨世界实时目标及三语言 27 个消息值同步修复。
- Admin：临时认证重读错误保留命令状态但关闭新写入；七类旧流程迁入完整负载/UUID 冻结与累计未知保护；
  TCP/Unix 互斥字段清理，提前开拍与 Key 交付文案修正，邮件五态和全局实时积压不再伪报健康。
- `auction-config` / 部署装配：普通 PostgreSQL URL 在驱动解析前拒绝未知参数；
  Admin 新候选按真实 Worker 启动基值校验跨组预算；三角色装备密钥一并比较。
  分体共享运行根改由 tmpfiles 拥有，Worker 只拥有自己的 control 子目录。
- 应用及独立 sccache 锁中的 `ctutils` 升至 0.4.4，修复 `RUSTSEC-2026-0385`。
  原问题是依赖公告风险，不声称发现本项目已发生的利用。

新增迁移 `000060_mailbox_sweep_cursors.up.sql`，旧 59 个迁移未修改；新增 P06 `PUT /api/v1/session`、
Admin `worker_config_file` 和必填 `realtime.data.total_lag`。同版升级、旧标签页刷新、Web Locks/安全上下文、
tmpfiles 安装及旧未知任务边界见[升级说明](ops/README.md#全源码审计修复升级)。

本轮实跑并通过：

- `CARGO_TARGET_DIR=target/audits/source-audit-20261023-build just verify`：策略、格式、workspace all-targets、
  独立 PostgreSQL/Redis feature、all-features Clippy `-D warnings`、Public/Admin WASM。
- `CARGO_TARGET_DIR=target/audits/source-repair-20261023-msrv cargo +1.94.1 check --locked --workspace --all-targets --all-features`；
  主工具链 rustc/cargo 1.99.0，独立 MSRV rustc/cargo 1.94.1，just 1.59.0。
- `cargo audit --deny warnings`，以及最终 `bash ci/checks/check.sh dependency-audit dependency-unused`：
  1350 条公告、537 个依赖，无漏洞或未使用直接依赖报告。
- `bash ci/tools/bootstrap.sh install release` 重新构建 dx 0.7.10 / sccache 0.18.0，
  随后的 release/audit 工具 receipt 校验通过。上游 sccache 编译仍有一条既有 dead_code warning，
  不是产品 Clippy 警告，也不据此宣称全部工具依赖均无公告。
- 八个受影响 CI/发布脚本的 `bash -n`。CodeGraph 已同步；定向 affected 未找到关联测试，不能视为测试通过。

集中检查先发现并修复 Admin 回调参数推导缺失、storage 新 timeout 对 Tokio 普通依赖的 feature 声明遗漏、
Worker 两个失效直接依赖；最终检查基于修复后的完整工作树。独立 target 避开审计基线中默认 target 的旧接口诊断，
没有将旧失败忽略为当前通过。Rust LSP references 两次以 server exited code 0 结束，无可用引用结果，
调用点核对依据当前源码、CodeGraph 与完整编译。

脱敏记录位于 `target/audits/source-repair-20261023/evidence.json`。未新增、扩展或运行行为测试，未启动产品/浏览器，
未执行数据库迁移、真实论坛/邮件操作、部署或 Git 提交/推送；双端 WASM 只做编译，不是页面交互或发布 payload 验收。
此前 R27/schema 59 部署及历史任务处置证据保持原样，本轮源码修复没有更新那些运行进程。

### 2026-10-23 历史投递安全恢复（授权真实任务处置）

用户选择“安全恢复”：只处理云端 `auction_test` 的 24 条原单次 Sent 基线读取失败，重排队 4 条注册回复和 6 条仍有效的
超价提醒，终结 14 条已永久失效提醒；保留原 1 条实际发送未知及 10 条已有专项处置的旧结算失败。判断同时核对原发布版本
sender 的错误传播：基线错误以 `operation=sent/stage=read` 返回，写后证明错误统一包装为 `operation=send/write_unknown`，
并核验原单次 attempt/fence、零资产、无资金绑定及当前来源资格，不仅凭 `write_started=false` 或 Sent 零命中授权重发。

先将完整目标行、来源通知、标记、领先周期、原未知和旧结算行保存为云端 root 私有行级补偿备份，落盘同步并复核 SHA-256；
原 R27 升级前全库恢复点继续保留。本次经正式 Admin 会话、CSRF 与 reconcile API 执行，24 个稳定 command ID、
原 fencing version CAS、命令账本及追加审计逐条确认；未直接写业务 SQL，处置后注销临时管理会话。原 metadata 已在私有备份留存。

09:39:51 UTC 24 项管理命令全部完成。至 09:51:39 UTC，正常 Worker 已将其中 9 条补发证明为 `sent_exact_payload_v1`
（3 条注册回复、6 条提醒）；14 条过期提醒为 `failed/lot_watch_invalidated`。剩余注册回复
`9b2c4e59-7d51-5a6d-942d-accad8ca24ac` 在这次实际发送后缺少完整 Sent 证明，进入 `manual_reconcile/write_unknown`，
未再次重试；原未知 `332fa19b-fd0b-59a4-b1c6-38053070ac48` 和旧结算投递逐行摘要保持不变。人工核对总数由 25 降至 2，
不把发后未知判为成功或未发送。新未知响应的加密证据已离线认证核对，只留脱敏摘要，临时明文已删除。

处置前两份认证响应确有 `battle_main` DOM、没有正常邮件 DOM，说明当时 `account_busy` 为游戏战斗页；之后邮件读取恢复，
未让程序代操作战斗或更换 CK。Worker PID `188169`、invocation 与重启计数保持原状；未改配置、迁移或启动本地 Worker。
脱敏回执位于 `target/operations-evidence/delivery-recovery-20261023/`，完整行级备份留在云端私有目录。
本次为获准历史任务实际补发及只读结果核验，没有额外触发注册、竞价、论坛或浏览器测试。

### 2026-10-23 R27 云端发布与 schema 59（授权迁移、启动与制品核验）

发布 owner 为 `ci/release`、`auctionctl` 分体停机/迁移控制器和三角色装配；邮件、凭据及论坛能力的 owner 见下方源码记录。
干净提交 `1ae2cfb97638de8ec2629978283d7cb6900212c9`（包含 Admin 凭据获取提交 `af511375`）通过完整 `just verify` 与
`just release-check`，实际重建通用 Linux x86_64 GNU API-only/embedded 四程序和双端 WASM，资源隔离及确定性归档门禁通过。
归档 SHA-256 为 `d5f14d798c37ecf6e3e85d36c8c1588111acab64d602a70fe11d9dd6b5fc9e61`，未使用 native CPU 参数。
本轮 Rust LSP 对 62 个变更文件未取得服务器答复，静态结论依据 Cargo、Clippy、WASM 与仓库策略门禁。

云端原 R26/schema 58 的全部已发布迁移 checksum 匹配。保存旧程序、配置、unit 和可验证 schema 58 数据库备份后，
Public → Admin → Worker 正常停稳，Worker clean drain、无遗留连接或未决 runtime 命令；同版 CLI 只追加 `000059`。
迁移前后 83 张旧业务表（不含迁移账本）的行数与逐表摘要完全一致，新节奏表七类起点与既有 `sending_at` 高水位一致。
未重写历史竞价、会话、凭据、投递或人工核对记录，未回退数据库。

配置仅修改 `worker.toml` 四项：Sent/Inbox 页数 20→100、fetch 总预算 30→120 秒、完整 delivery 预算 180→300 秒、
sending grace 300→360 秒。HTTP 最小间隔仍为 1000 毫秒，lease/renew 仍为 60/10 秒，批次预算仍为 600 秒；
mail/worker 的 runtime 接管开关仍为 false，七类发送间隔缺省全零，其他配置值、凭据与启用开关保持原状。
runtime 锁保留原 inode、属主和精确 0600 权限。准备/写后证明与未知结果边界见[升级说明](ops/README.md#邮件发送节奏与-sent-准备升级)。

09:05:51 UTC 同版 Worker/Admin/Public 按序恢复，PID 为 `188169/188193/188202`；readiness 均 200、NRestarts 均零。
09:07:36 UTC 后验确认运行/磁盘哈希、启动回执、schema、配置及权限一致；双端 Unix 与 Public HTTPS 共 16 项静态资源逐字节匹配，
该核验窗口无 ERROR/WARN。09:15:47 UTC 后续只读观察进程身份仍不变、无 ERROR；Worker 有 18 条包含 `account_busy` 的收件读取/退避告警
（包含同一次失败的多层记录），Public/Admin 无告警。保留该现象，不将部署就绪写成邮件实际送达验收。

脱敏回执位于 `target/dist/mail-cadence-20261023-ops-r27/DEPLOYMENT-SUMMARY.json`、`cloud-evidence/` 与 `stability.json`；
云端应用备份目录名为 `mail-cadence-20261023-schema59-r27`，数据库恢复点在 root 私有同名维护目录，完整路径与哈希留在私有回执。
数据库归档、配置正文和原始日志未下载。本轮只做静态构建、授权部署及只读后验，未运行登录、竞价、发信、论坛或浏览器业务测试，
未人工重发或清草稿。代理日志原因细分仍为讨论方案，未纳入本版。未启动本地 Worker 或替换本地 Public/Admin 运行程序；
`target/dx` 开发资源目录由正式 Web 构建刷新。

### 2026-10-23 Admin 论坛账号获取 CK（真实登录与隔离数据库验证）

owner 为 application credentials、integrations `credentials/acquire`、contracts/HTTP 与 Admin Web；
A32 新增互斥的账号密码来源，游戏导入可直接使用完整论坛 CK，只密封 `ipb_member_id` 与 `ipb_pass_hash`。
每次只导入所选类型/世界；密码不持久化、取得的 CK 不返回浏览器，原事务、审计、单 active 与 Worker 重启要求不变。
无新增数据库迁移或配置字段，精确协议见[外部凭据](docs/contracts/routes.md#外部凭据)。

用户配合浏览器退出/登录，核对实际表单字段与 Set-Cookie 名称；全新无 Cookie 会话与已登录浏览器不同：
表单 action 携带 `s`，referer 可带双重 HTML 转义的 `amp;act`，控制台个人链接也可能为 `s + showuser`。
前两次真实获取分别在表单目标、控制台证明处返回 503，未写入凭据；修复为保留唯一合法 session 参数、
限定同源登录动作及唯一账号链接。相同真实匿名/认证页面样本在旧程序失败、修复后通过；
重复 session、跨源目标、不同动作、非法 session 或重复 showuser 仍失败关闭。

独立 PostgreSQL 18.6 临时库使用当时工作树的 59 项迁移（含另一项尚未提交的邮件节奏迁移），运行本地 `postgres,web-embed` Admin，不启动 Worker。
最终用户真实账号登录返回 201，密码表单清空；原完整命令重放仍为 201、同一凭据，A44 论坛认证返回 valid。
新取得论坛 CK 的两个共享字段与已验证游戏 CK 一致；游戏主世界、异世界只读认证均为 valid。
直接核对临时库密文解密后的字段集合：游戏仅两项，论坛为四项 IPB 字段；三次成功导入只有三条 import 账本和三条导入审计，
命令结果不含密码或 Cookie 字段。临时库、私有配置、服务和验证页面已清理，未更新线上凭据或部署。

真实 HTTP 还覆盖混合来源、嵌套未知字段、非法用户名、缺失认证字段、同 ID 同负载重放及不同负载 409；
本地 Cookie 提取探针另验证重复认证字段与非法 hash 被拒绝。
浏览器在服务器已提交后丢弃响应，表单冻结原 UUID/来源/值；手动重试只产生一次导入并清空敏感输入。
11.1 秒迟到响应仍可处理，20.1 秒无完整响应进入冻结未知状态；五语言 × 1440/375/320px 共 15 组无横向溢出。
本地协议探针验证新鲜隐藏字段、重复 Set-Cookie 更新、正向认证证明、拒绝/挑战、重定向、正文超限与未知字段；
未把隔离样本当作所有远端挑战、并发或生产运行验收。

最终源码完整 `just verify` 通过；Admin WASM payload 与原生验证程序构建成功，未新增永久行为测试文件。
首次 Web 构建因构建期间译文变化被指纹门禁拒绝，输入稳定后重建成功；LSP 初始化退出，引用由 CodeGraph、调用点及编译核对。
脱敏证据为 `target/audits/credential-acquisition-20261023/evidence.json` 与 `live-success.png`，
不保存原始登录请求、密码、Cookie 或带 session 的远端 URL。

### 2026-10-23 邮件发送节奏、Sent 准备与论坛标题（源码阶段）

application/delivery 定义七类发送间隔和非权威唤醒端口，config/Admin Web 接入接管、严格数值校验及五语表单；
storage/delivery_scheduler 经 `000059` 保存发送起点、fence 内复核及提交后通知，jobs 优先领取超价 MM 并保留断线轮询。
integrations 把完整 Sent baseline 移到同一独占 session 的 sending fence 前，区分 `sent_scan_page_limit`/`sent_scan_timeout`，
写后未知仍不重寄；现金证据在基线完成后另以原 30 秒窗口取得并持有至提交。论坛新标题采用完整世界/Session/状态格式，
保留旧短标题及更早标题的精确核对候选，不以改名重新授权发帖。

本轮工作区 `cargo check --locked --workspace --all-targets --all-features` 已通过；其后仍有修改，完整门禁与发布结果另行登记。
未新增或运行行为测试，未迁移/启动本地 Worker，尚未把这些修改部署云端。旧人工核对投递不批量重发。

### 2026-10-23 R26 云端发布与 schema 58（授权迁移、启动与制品核验）

用户授权本地构建后迁移部署。发布 owner 为 `ci/release`、`auctionctl` 的分体停机/迁移控制器及三角色装配；
配置兼容由 `auction-config` 拥有。干净提交 `003a45033a3547ad97527d742e6ad13491d00f09` 通过完整 `just verify` 与
`just release-check`；同源 API-only/embedded 制品按输入指纹校验复用，通用 Linux x86_64 GNU 四程序、双端页面、资源隔离及归档门禁通过。
归档 SHA-256 为 `4d81630b0e547c22e96b3e0ce7f067b3a618cfec4832992d208b9278c6d68781`；未启用 native CPU 参数。

云端 `auction_test` 从 R25/schema 56 升至 58，旧 56 项迁移 checksum 全部匹配；先保存旧程序、配置、unit 及可验证数据库恢复点，
Public → Admin → Worker 停稳且 Worker 正常 drain、无其他数据库连接/runtime 未决命令后，用同版 CLI 追加 `000057/000058`。
迁移前后全部 Public 会话行、用户旧字段的摘要与行数一致，新偏好不回填旧会话。P09 等级筛选、P27/P28 登录时长及邮件修复随同版程序上线，
契约分别见[HTTP](docs/contracts/routes.md)、[Public 登录升级](ops/README.md#public-登录时长升级)与既有邮件记录。

首轮候选配置预检因废弃 `session_signing_key` 拒绝，未停服或修改线上配置，失败证据保留。后续只移除 Public 两个旧 TTL 字段、
Public/Admin 敏感文件中从未生效的 `session_signing_key`，其他 TOML 值、凭据与开关保持原值。05:08:06 UTC 迁移完成后，
Admin 因旧 runtime 锁为 `0644` 启动失败并自动重启 11 次，总控退出并停稳三角色。保留失败日志后，在原 inode `656282` 的独占锁内
只将权限收紧至 `0600`，属主及内容不变；未回退数据库或程序。重新启动仅做 schema 58 的已应用迁移校验，未重复改写历史事实。

05:12:32 UTC 同版 Worker/Admin/Public 按序恢复，PID 分别 `151829/151846/151854`，三端 readiness 均 200。
后续独立核对运行/磁盘哈希、启动回执、schema 与配置一致，进程身份未变且本次启动重启计数均零；双端 Unix 与 Public HTTPS 共 16 项
静态资源字节全部匹配。该核验窗口无 ERROR；日志保留 Public 的 `there is no transaction in progress` 与 Worker Sent
`remote_temporary` 维护告警，后续另观察到发送失败后的 compose 重读告警；未人工清草稿、触发投递或重发。

脱敏回执位于 `target/dist/public-session-20261023-ops-r26-attempt2/DEPLOYMENT-SUMMARY.json` 及 `cloud-evidence/`；
云端应用备份目录名为 `public-session-20261023-schema58-r26-attempt2`，数据库恢复点位于 root 私有目录
`hv-auction-public-session-20261023-schema58-r26-attempt2`，完整路径见私有回执。首轮预检失败目录继续保留。
本次只做静态发布门禁、授权迁移/启动与只读部署后验，未运行浏览器、登录、竞价或邮件行为测试；本地服务 PID 未变，Worker/CK/轮询未启用。

### 2026-10-22 Public 登录时长设置（隔离 PostgreSQL 与浏览器验证）

owner 为 application sessions、storage sessions、contracts/HTTP、config 与 Public Web。「我的」新增永久（默认）及分钟/小时/天
自定义时长，保存影响当前会话和后续登录，其他设备现有期限不变。`000058` 添加账号偏好及独立版本、成对 nullable 截止，
不回填旧会话或撤销事实；P27/P28 新增 GET/PUT，P06 支持永久截止投影。Admin 会话配置和生命周期保持独立。
Public 旧部署 TTL 字段明确拒绝，升级边界见[运维说明](ops/README.md#public-登录时长升级)；保留策略元数据同步到 schema 58。

静态门禁分阶段完成：repository/layout/config/i18n、格式、workspace/all-targets、独立 PostgreSQL/Redis feature、
Clippy 及 Public/Admin WASM 均通过。最初 `just verify` 在新时长换算的 `manual_is_multiple_of` lint 处失败，
修复后 `just lint` 通过；WASM 暴露 FixedOffset 与 UTC 相减的类型错误，归一化 UTC 后 `just wasm-check` 通过。
Public Web bundle、`postgres,web-embed` Public dev 程序和 `postgres` auctionctl 构建成功；未新增永久行为测试文件。
Public payload SHA-256 为 `7361c7da3a1593e8cd8e91361960e08b84c99334167b2167055feb2574b91802`，
验证程序 SHA-256 为 `5c40cd4aad12652a9fcd6672c965ce9b0053adfd02205230ea5e66a729b43513`。

独立 PostgreSQL 18.6 空库通过临时探针调用正式 `DatabaseStore::migrate` 到 58。真实 HTTP 验证默认永久、400 天 Cookie、
有限固定期限、后续登录继承、其他会话不改、1 分钟/365 天边界、非法/缺失字段、Origin/身份门禁、同 ID 重放及跨会话冲突。
并发同版本保存得到一个 200、一个 409；已提交后断开响应的浏览器重试保持原完整负载，只新增一条审计。
行锁等待 3030ms 跨过旧截止后保存返回 401，偏好和审计不写入；永久退出后的读取和重放均为 401。
永久 WebSocket 握手成功，退出后 29998ms 收到 1008“会话已失效”，没有绕过周期撤销复核。

Chromium 验证五语 × 320/375/1440px 共 15 组无横向溢出；零值草稿可修正，半小时/天数换算、刷新恢复及永久切换生效。
1 分钟保存的数据库截止与审计时间精确相差 60 秒，之后自然退回登录页、设置卸载；未把浏览器观察时刻当作精确到期延迟基准。
配置诊断确认 Public 默认/空区块仅保留并发上限 5，两个旧 TTL 字段分别拒绝；Admin 省略区块仍为 900/28800 秒，
既有显式部分区块仍为 1800/604800 秒。LSP 退出未取得引用结果，源码消费者通过 CodeGraph 与编译核对。

探针首次误对编码后的 Cookie 字符串取摘要，未命中会话行；改为解码后 32 字节摘要并断言恰好命中一行后完成行锁验证。
原始结果、配置证据和桌面/手机截图保存在本地 `target/audits/public-session-lifetime-20261022.COEpXa/`。
仅使用合成账号和独立数据库；未替换现有服务、修改现有数据库或部署，未执行真实换钥/封禁、竞价、邮件或论坛流程。

### 2026-10-22 Public 等级框与类型行紧凑调整（本地浏览器验证）

owner 为 `auction-web-public::surface::catalog::filters` 与 Public 样式、五语提示。桌面等级框由 112×44px
缩至 80×28px，内边距由上下 4px／左右 8px 改为上下 0／左右 4px；触控与窄屏仍保留 44px 输入高度及 16px 字号。
移除等级应用按钮与五语废弃键，回车或离开字段提交有效草稿，非法输入保留错误且不替换已应用条件；
装备类型标签与“显示列”按同一首行高度居中对齐。HTTP、查询字段、数据库、列偏好及竞价门禁不变。

`bash ci/checks/check.sh fmt i18n`、Public all-targets Clippy（`-D warnings`）、Public WASM check
及 Public Web bundle 均通过。新 payload SHA-256：
`cec1a36c61918c491a56e87be21a4c9b883c1e2e621db3f7147942036e69d202`。
Rust LSP 启动后退出，未取得引用查询结果；源码定位补用 CodeGraph，编译结论来自上述 Cargo 命令。

Chromium 使用新 bundle，经仅允许 GET/HEAD 的临时 loopback 预览读取现有本地 Public API。
320/375/1440px 与五语的 15 组布局均无横向溢出，类型标签与列按钮中心线一致；已观察失焦提交 400、
回车提交 399、非法输入保留原 URL、输入清错、清空上限、直接切换复选框、同值失焦不增历史、返回及刷新恢复，
并确认显示列展开、Escape 关闭与焦点恢复。证据与桌面/手机截图位于本地 `target/audits/catalog-filter-density-20261022/`。
未新增行为测试文件，未替换运行服务、迁移、登录、出价或部署；不将局部界面验证视为数据库或生产验收。

### 2026-10-23 Public 等级筛选与类型工具栏（静态检查、本地更新）

owner 为 `auction-application::ports::auction::queries::lot` 的等级条件、`auction-storage::database::lots` 的同快照筛选/统计/分页、
`auction-http::routes::reads::lots` 的查询边界，以及 `auction-web-public::surface::catalog::filters`、看板查询状态和本端五语/样式。
分组同行新增包含式等级上限（默认留空）和独立的“显示无等级装备”复选框（默认勾选）；等于上限保留，高于上限隐藏，
None 不当零级。原生输入支持回车/应用，复选框变化提交有效草稿；条件写回 URL、变化回第一页并隔离旧响应，非法条件不发出宽查询。
“显示列”与装备类型固定为同一网格行的左右两列，类型选项内部折行，按钮不另起一行；表格列宽及列偏好不变。

P09 仅增加可选 `max_level`、`include_unleveled` 查询，旧默认读取及 JSON 字段保持兼容；范围和错误见
[装备筛选契约](docs/contracts/routes.md#p09p11-装备分组)。筛选先于类别统计和分页，浏览器只复核响应归属、不删本页行。
装备事实、详情可见性、标记/竞价门禁、金额、事件、配置和数据库结构均不变；账户完整标记读取显式保持不限等级。

完整 `just verify`、workspace/all-targets/all-features check、Public WASM check、格式与 diff 门禁通过；Public Web bundle
及 `postgres,web-embed` dev 宿主构建完成。主动 LSP 对 15 个 Rust 文件未取得答复，CSS 既有 important/vertical-align 提示保留；
未新增或运行行为测试、浏览器、登录或竞价流程，布局状态依据源码与静态构建。

02:08:46 UTC 仅本地 Public 更新为 PID `3506289`，readiness 200；Admin PID `3401418` 未改变。
先备份原 Public 程序与 Web payload，再优雅停止 Public，重建资源并原子替换程序；未运行迁移或更新云端，配置逐字节未变。
Worker 未运行，active CK 和未暂停邮箱均为零。六文件 payload SHA-256：
`2e2e49286e1a4caf001e2c6f4fe9bfc0f326c92d3155d68a9e719ff89615d51f`。
本地私有回执 `local-test/state/PUBLIC-LEVEL-FILTER-20261023.json`、备份
`local-test/backups/public-level-filter-20261023T020715Z/` 记录原/新身份；仅启动就绪确认，不作为浏览器验收或正式制品。

### 2026-10-23 Public 列设置密度与匿名出价入口（静态检查、本地资源更新）

owner 为 `auction-web-public::surface::catalog::{columns,actions}` 及本端样式。显示列内联面板将恢复全部与关闭移至标题行，
内边距改为 8px，网格弹性基准由 12rem 收至 8rem，行/列间距为 4px/8px；保留原生复选框与 44px 点击区域，
不改变列权重、至少一列约束、偏好键或命令宿主。匿名“登录后出价”增加独立 BEM 类，与“登录后标记”共用按钮视觉、
可见焦点及触控尺寸，仍使用原登录导航和安全返回地址；无 HTTP/JSON、权限、金额或业务命令变化，无新依赖或译文。

格式、Public WASM check、包级全目标 Clippy（`-D warnings`）、repository/layout/config/i18n 和 diff 静态门禁通过；
Public Web bundle 与 `postgres,web-embed` dev 宿主构建通过。Rust LSP 未取得答复；CSS 原有 important/vertical-align 提示保留，
未计 LSP 全绿，未扩改无关样式。未新增或运行行为测试、浏览器、登录或竞价流程。

仅更新本地 Public 资源并优雅重启，Admin PID `3401418` 保持不变。首轮派生后身份登记失败，核验同一 Public PID 后恢复状态，
未重复派生；后续使用有界 `/proc` 身份等待。01:17:15 UTC 最终 Public PID `3446240`，readiness 200，
Worker 未运行、active CK 和未暂停邮箱均为零；未访问或更新云端。最终六文件 Public payload SHA-256 为
`d8d5dff885f6bb3a746f7c95b4cf986df20a390c6d341c1a31776a01275252d2`。
本地 `state/PUBLIC-COLUMNS-DENSITY-20261023.json` 保留首轮记录，`state/PUBLIC-UI-20261023.json` 记录最终资源和进程身份；
dev 原生二进制哈希未变化，资源来自本地源码构建目录，不作为正式发布制品或浏览器验收。

### 2026-10-23 线上业务的本地隔离副本（授权准备与启动就绪）

用户确认更新现有本地 `127.0.0.1:55432/auction_test`、隔离认证，并仅保留 Public/Admin 局域网网页。
owner 为 `auctionctl` 备份/重建/恢复/迁移、`auction-integrations::cryptobox` 装备转封及两 HTTP 装配根；
未修改线上数据库或停止其服务；系统 5432 仅在定位阶段于维护库只读列库，全部覆盖与迁移只作用于 55432 目标。
流程见[隔离手册](ops/deploy/RECOVERY.md#授权的本地业务快照隔离)。

00:19:52 UTC 取得线上 schema 56 的只读一致性快照；84 表计数与 pg_dump 共用导出 snapshot，逐项 checksum 匹配源码。
archive SHA-256 为 `deb6f2319c7f293c3ebf5e8638090d9c81b4f22f992f78c0adfa267ef3833759`。
本地三角色停止后，原 schema 57 库备份通过官方离线 verify；沿用精确目标、维护凭据、零连接与空库门禁重建恢复，
恢复后的 84 表逐表行数一致，再显式前向迁移至 57。业务数据保留用户 62、拍卖 3、装备/标的 76、入站邮件 89、投递 154。

仅在本地原子禁用两条 active CK、撤销 160 条 Public/1 条 Admin 会话、暂停邮箱并保持游标；
76 条装备信封经正式 opener/sealer 校验原 AAD/指纹、转封到原本地密钥并重新认证。
线上装备数据密钥只在短命进程内使用，不保存；不复制 CK 解密密钥或线上敏感 TOML，本地三份敏感 TOML 逐字节保持不变。
追加审计 `81771cfc-652c-48ee-90e8-dd79ed3a7fe6`，未更改命令、历史审计、金额或投递结果。

双端 Web payload、Public/Admin `postgres,web-embed` dev 制品及 auctionctl 构建通过；只更新本地程序，未构建正式发布包。
00:40:37 UTC 两端 readiness 均 200，active CK、未撤销导入会话、未暂停邮箱均为零，Worker 进程不存在；
本地启动脚本只启动 Public/Admin，并拒绝开启外部集成、配置 CK/.keyenv 或改变隔离数据库目标。
正常 HTTP 启动仅另增加进程心跳和本地 realtime 位点元数据；未新增或运行测试套件、浏览器、业务流程或真实外部调用。

证据位于私有 `local-test/backups/cloud-sync-20261023T001327Z/`，含原本地备份、源 archive/manifest、
rebuild/restore、隔离计划/回执及最终后验；`local-test/state/CLOUD-REFRESH-20261023.json` 和
`LOCAL-BUILD-20261023.json` 分别记录就绪/数据与二进制/Web 身份。旧本地验证和原发布回执保留，不改写历史结论。

### 2026-10-22 Admin 入站处理定位（源码静态验证）

owner 为 `auction-application` 观测端口、`auction-integrations` 实际详情目标、`auction-jobs` 有界进程快照与取消守卫，
Worker 装配同一 tracker，`auction-http/contracts` 提供显式诊断投影，`auction-web-admin` 负责独立状态卡、严格解码及五语。
`/mails` 加载/刷新指定邮箱时显示安全已完成 MID、真实当前阶段/MID、最近失败阶段/MID/时间及观测时间；
完成、失败和取消收口当前目标，不从待读队首猜测。Worker 不可用、尚未观测和空闲分别呈现，进程重启清空最近失败观测。

A16 仅在 `include_diagnostics=true` 增加可选 `polling.diagnostics`，旧查询及暂停/恢复的字段集合保持兼容；
同一 peer-authenticated control 增加只读 `/v1/mail/progress`，旧 status 闭集不变。仅投影安全数字 MID，
不导出原始 cursor、claim token、发件人、正文或凭据；不新增业务写入、配置、依赖或迁移。
协议见[邮件契约](docs/contracts/routes.md#邮件审查与轮询)，操作见[入站观测](ops/mail/diagnostics.md#admin-入站处理观测)。

完整 `just verify` 退出 0，workspace/all-targets/all-features check 与 Clippy（`-D warnings`）、
`repository/layout/config/i18n` 策略门禁、格式和 `git diff --check` 通过。主动 LSP 对 35 个变更 Rust 文件未取得服务器答复，
未计为通过。未新增、修改或执行行为测试，未做浏览器验证、构建发布制品或部署云端。

### 2026-10-22 MoogleMail 完整发件人与单封临时跳过（静态检查、限定线上维护）

本条时间采用 UTC。源码 owner 为 `auction-integrations` 的 Inbox parser/gateway；`auction-application` 仅补充
待读快照端口说明，`auction-config` 同步白名单注释与示例。入站、注册/重绑及回复身份只采信详情完整发件人，
列表显示名只保留恢复快照兼容，不再与详情比对或用于 allowlist。候选窗口仍有界，详情未获白名单授权时保留待读项、
失败关闭，不领取、退回、回复或越过；未关闭白名单且名单为空时拒绝详情读取。MID/world/时间、详情结构与资产校验、
租约/完成前缀及命令账本保持原义；无新增 HTTP/JSON 字段、迁移或 Sent 成功证明放宽。

Rust 1.99.0 下完整 `just verify` 退出 0，覆盖策略、格式、全部已有目标、独立 PostgreSQL/Redis feature、
全特性 Clippy（`-D warnings`）及双端 WASM；`cargo fmt --all -- --check`、`git diff --check` 通过。
主动 LSP 探测四个 Rust 文件均未取得服务端答复，未计为通过。未新增、修改或执行行为测试、浏览器或真实 MM 验证；
源码修复未构建发布制品、未上线。

授权只读排查确认主邮箱 MID `3120535` 的持久列表显示名为 15 字节、认证详情完整名为 18 字节，
旧 parser/gateway 的完整相等比较导致重复失败。详情证据 `2fa5ceb9-a231-4685-bb34-5d2087223134`
确认为 `register`，`attach_count=0`、`attach_cod=0`、`mail_state=2` 且无附件列表；离线解密临时明文已清理。
用户随后明确确认仅跳过云端 `auction_test / mooglemail:main / 3120535`，并知悉不自动注册或回复。

16:31:40 的单次维护先保存并核验原状态、目标待读记录和计划；持同一 Serializable 事务的邮箱/队列行锁，
确认无 claim、窗口仅有待读序号 17、尚无入站事实后，原子将 cursor `3120513 → 3120535`、版本 `5382 → 5383`，
仅移除目标待读项并追加 `mail.operator_skip_unprocessed` 审计。稳定 command/audit ID 为
`e247e7e4-a6ab-48a2-b7fb-6ac740ce27eb`；没有伪造处理成功或创建 delivery，没有直接 MM 写入、重发或重启。
原失败历史、调度及其他邮箱保持不变。行级备份 SHA-256 为
`bc41da40d19c648cd3ff5a8daf14c2a11f9e963b9a3d1dded58e00e9ba5de4ef`；
云端私有 `mail-skip-3120535-20261022T163127Z-e247e7e4/` 保留原记录、计划、回执和加密原证据，文件均为 `0600`。
脱敏回执及本次一次性脚本位于 `target/operations-evidence/mm-skip-20261022/`，回执 SHA-256 为
`400d6626ae3892f531fdc2460ce3967a599f416c84d677099dccc8d8eaf66f39`；
[运维边界](ops/mail/diagnostics.md#显式单封跳过的运维边界)不授权未来自动跳过其他邮件。

16:35 的正常轮询转而在同一发件人的重发注册 MID `3120551` 失败；认证证据
`eedd3dd8-392c-4d05-b33d-949435b41921` 同样为零资产。该项序号 21 位于六条待读记录中，前面仍有三封其他邮件，
因此用户再次明确确认只隔离这封、保持 cursor `3120535` 及其他五条记录。首计划
`ac2b9b3f-791c-4fd1-b6c6-bfde8aa789f7` 因预检后邮箱快照变化在 mutation 前中止，未写入；确认该审计 ID 不存在后，
新计划改为在持续持有同一邮箱/窗口行锁期间取快照、同步核验完整窗口备份并提交。

16:43:44 确认无 claim、目标尚未入站后，原子仅移除 `3120551`，版本 `5391 → 5392`，cursor 保持 `3120535`；
提交前逐项确认 `3120539 / 3120540 / 3120550 / 3120569 / 3120572` 五条记录和顺序均保留。
新 command/audit ID `b367e8a2-4d2c-45c4-94f0-f44b24d88065` 的 `mail.operator_quarantine_pending` 审计绑定
原失败计划、证据、请求及备份摘要。没有注册、回复、创建入站成功事实、直接 MM 写入或重启。
云端私有 `mail-quarantine-3120551-20261022T164344Z-b367e8a2/` 保留完整窗口、计划、回执及加密原证据；
行级备份 SHA-256 为 `522760f5675a02da53432acd4e727e014fffabee0b6480f6c0fe402dc49e5dd2`。
同一本地证据目录中的 `receipt-3120551.json` SHA-256 为
`c4f367bf1a3c106ce0b13570ee7c435807fa75b9b6f66fc5b5b8a7978e3892f1`，先前中止计划及脚本均保留。

16:47:09 只读复核：16:45 的正常轮询已推进 cursor 至 `3120572`、连续失败归零、待读队列为空；
五条受保护邮件均已入站，其中四条 `registration_accepted`、一条 `archived`。两封跳过邮件仍无伪造入站事实，
两条成功运维审计存在，首个中止计划无审计。新建 `mail_intake` 出站为成功 1、发送中 1、待发 2；
Worker 仍为原 PID 2123、自动重启 0，隔离后日志无新增 warning/error。此为正常业务流的限定只读观察，
未扩展成测试套件或执行主动重发。

16:49 UTC 续查：上述四条新注册回复均已 `succeeded`，逐条具备 `sent_exact_payload_v1` 和 external reference；
cursor `3120572`、失败计数为零且待读为空，两封跳过件仍无入站事实。正常恢复未部署详情发件人的根因修复，
也不授权未来自动跳过其他邮件。

### 2026-10-22 公开装备列显隐与目录留白（本地页面验证）

owner 为 `auction-web-public::surface::catalog::columns`、目录表格及 Public 样式/五语资源。
装备类型行右侧新增“显示列”，实时/已结算/预览目录默认显示 13/12/11 列；原生复选框立即生效，
支持显示全部、Escape/关闭后的焦点恢复，以及 `hv.public.catalog-columns.v1` 本地偏好。
至少保留一列，布局切换无可用已选列时临时显示装备列；损坏或受限存储退回默认，写入失败不撤销本次选择。
表头、单元格和可见权重同步，打印保持隐藏；操作列宿主不因偏好隐藏销毁，剩余时间隐藏时卸载秒级订阅。
宽屏目录释放左右空白，保留小边距及 safe-area；其他 Public 页面沿用原容器宽度。
无 HTTP/JSON、权限、金额、命令身份或数据库变更；只补齐本包实际使用的 web-sys features，没有新增运行依赖。

完整 `just verify`、MSRV 1.94.1 的 workspace/all-targets/all-features check 和 Public Web bundle 通过。
首轮修正 Dioxus `key` 必须使用格式化字面量的编译错误。实际页面发现目录在 `.public-lot-watches` 包装内，
旧直接子代 `:has(> .public-auction-page)` 未命中；改为限定目录后代后重建，最终六文件 payload 已用于浏览器验证。
临时纯 Rust 诊断覆盖 8191 个合法位集、三布局及 638898 次 setter 尝试；VirtualDom 覆盖五语、开关/重置/关闭/Escape、
末列保护和模式兜底，均通过。

Chromium 在本地合成响应下验证三布局、所有五种语言、偏好重载、读写失败、损坏恢复、筛选 URL 不变、
可见表头/行/col 数量及 100% 归一化、打印隐藏和 1920/2560/375/320 宽度无横向溢出。
宽度按 `documentElement.clientWidth` 计算，排除桌面滚动条；规则页保持原宽度。
操作列隐藏/恢复保留同一动作 DOM；两个故意断连的本地模拟出价使用相同 command_id 和金额，
确认未知命令宿主未重建，没有真实出价。剩余时间叶子由两个变为零、恢复后回到两个。
证据与截图在 `target/audits/catalog-columns-20261022/`。临时浏览器、服务、源码及诊断二进制已清理；
未新增永久测试、运行完整业务测试套件、访问生产数据库或部署线上。LSP 启动失败，未计为通过。

### 2026-10-22 论坛倒计时数字与主题状态（本地隔离验证，schema 57）

针对[问题帖 302292](https://forums.e-hentai.org/index.php?showtopic=302292)及用户提供的
[参考帖 302192](https://forums.e-hentai.org/index.php?showtopic=302192)，用户确认 Auction Timer 表示剩余时长。
只读线上 SVG 在 `prefers-reduced-motion: reduce` 下显示 `See deadline below`，原因是本地样式主动隐藏了数字。
`auction-http/public/countdown` 现默认显示数字，减少动态效果仍逐秒离散换字；动画不可用时保留加载时数字快照，
不加脚本、轮询或第三方倒计时依赖，原 URL、时间来源、UTC 截止、终态与安全头契约不变。

论坛主题采用 `[Auction] Main #002 (Live)` 形式，状态由 application/storage 的一致快照提供，
integrations 渲染，jobs 同步标题/说明；短格式覆盖最长合法序号且不超过论坛 50 字符。
当前及历史状态、原无状态标题、HV# 摘要标题以有界精确集合在一次目录 GET 中查找，冲突失败关闭。
首帖正文相同不再掩盖标题单独变化；未知写入仍只读对账。已有 Preview/Cancelled 主题可刷新，
取消不创建主题或导入竞价；终态完成复用原字段、版本/租约/CAS 与历史 Settled 豁免，无新增迁移。

完整 `just verify`、MSRV 1.94.1 全目标/全特性检查及更新后的配置注释门禁通过。临时 Rust 诊断直接使用实际 renderer、
回读匹配与 Worker 门禁函数：56 个世界/状态/序号组合通过，最大标题 49 字符；回环 EhentaiClient 验证元数据
读写、缺失/冲突拒绝及九个标题仅一次 GET，未知写入不重复 POST。私有 PostgreSQL 18.6 执行重定向到临时表的实际
SQL，七状态资格、历史豁免、取消终态、pending、陈旧版本/token、过期租约及两个完成入口通过并全部回滚。
Chromium 验证减少动态效果和普通模式、分/时/日进位、毫秒边界、到期与静态数字；实际 `<img>` 在 reduce 模式正常加载并换字。

证据在 `target/audits/countdown-status-20261022/`；临时源码、夹具、数据库与预览服务已清理。
未新增永久测试，未运行完整业务测试套件、仓库迁移或线上部署；线上仅只读查看给定帖子与 SVG，未发帖、编辑或解除人工门禁。
LSP 引用请求启动失败，未计为通过；调用迁移经 CodeGraph 和全目标编译核对。

### 2026-10-22 全源码再审与边界修复（本地隔离验证，schema 57）

覆盖 20 个工作区包及 CI/发布、配置和部署源码；按 owner 清点全部模块并逐段读取关键路径，
不是逐行形式证明。保留开始时已有的 136 个工作树修改，不把上一轮成果算作本轮新修复。
未增加运行依赖、迁移、业务路由或永久测试；外部 JSON、命令身份、金额算法和未知外部写入门禁保留。

- domain/application/storage：零软截止窗口不在截止等值重新开启延期；规则新建与版本更新分开 CAS；
  Admin 创建先取得原有锁再读实时钟，周轮次不再使用事务开始时间判断锁后截止。
  realtime acknowledge/replay 统一锁序，补齐已有事务 runner 的死锁分类；资产冲突历史完整流式分类，移除 JSON 深复制。
- HTTP/cache：拒绝静态资源路径穿越及缓存别名；固定游标先检查长度再栈内解码；
  账号偏好方法错误正确声明 `GET, PUT`。Redis 原始错误不进入错误链，损坏缓存删除失败后不继续回填。
- Worker/runtime：control 健康状态单次原子快照，论坛恢复不覆盖其他降级；拒绝迟到续租和已过绝对截止的排队任务；
  HTTP/control 组名查询采用有界可重入缓冲。manifest、回执、规则正文同描述符限量读取，心跳 JSON 校验不构造完整字符串。
- integrations：论坛创建要求真实确认页证明，错误页旧主题链接不再算成功；正文遍历维护状态栈；
  入站摘要排序附件引用，清理指定秘密缓冲并减少副本。`retryable=true` 仍可进入只读对账，不代表可以再次 POST。
- Web：隐藏等待取消即注销 Waker；Admin 每命令 busy 改为集合条目状态，避免 Signal 保留至页面卸载。
  Public 成功、空结果和错误均绑定请求身份；装备标记建立共享索引，Markdown 列宽只扫描有界字符。
- config/setup：普通 TOML、keyenv 及 runtime 读取预算/文件门禁补齐，默认发现只在明确不存在时回退；
  两端与集成适配对齐 IPv6 回环。实际生成诊断另外发现 TOML 1.1 `Value::FromStr` 不能解析完整模板，
  改用文档反序列化后原始 `write_to` 场景通过，未放宽校验。
- 文档/发布：同步 owner 契约与中文注释，修正 Public 唯一路径计数及过期 notify 缺口说明；
  构建失败清理本次候选目录，独立 Web 构建在签发复用标记前再次核对输入。

Rust 1.99.0 下完整 `just verify`、MSRV 1.94.1 的 workspace/all-targets/all-features check、全部 CI ShellCheck、
actionlint 和两份修改后发布脚本的 `bash -n` 通过。首轮修正格式，随后按已有 MSRV 惯例给两个 `fetch_update`
调用添加最小 deprecated 例外；首次配置模板修复后完整静态门禁再次通过。`just web-build` 两端各生成 6 个 payload 文件；
没有构建后端发布包或部署。LSP 启动退出，未计为通过；调用关系使用 CodeGraph，索引已同步。

隔离 PostgreSQL 18.6 使用本次新建私有 cluster、专用空库和 Unix socket，禁 TCP，不执行仓库迁移：
旧规则 SQL 重现缺失文档直接创建 revision 5，新 SQL 六种 CAS 场景通过；实际行锁等待跨截止后，
旧事务时钟误判未错过、新实时钟正确判错过；旧 realtime 相反锁序产生 `40P01`，新顺序串行等待后双方提交，
offset/failure 一致。仅证明选定 SQL 与真实锁行为，不代替完整 Rust 用例、SSI 或重试 runner 验收。

临时 Rust/VirtualDom 诊断通过领域、Debug/wire、缓存故障、配置/FIFO/权限、命令生命周期、Router/HEAD/缓存头、
游标、真实 RustEmbed 开发态读取、邮件/论坛解析、摘要、秘密缓冲、文件增长竞态及 Worker 截止场景。
NSS 使用内存替身，不宣称真实系统后端时限；root 读取其他 UID runtime 的场景因本机非 root 明确跳过。
探针自身的 retryable 误解和混选 MSRV rlib 已修正，未为使探针通过改变生产协议或关闭应用 lint。

以下是隔离诊断计数，不是耗时、吞吐或峰值 RSS：

| 场景 | 修改前 | 修改后 |
| --- | --- | --- |
| Public 隐藏等待 poll 1000 次，取消后保留的 Waker | 1000 | 0 |
| Admin 隐藏等待 poll 64 次，取消后保留的 Waker | 64 | 0 |
| 2000 层论坛正文的迭代步数 | 2013022 | 2010 |
| 8 MiB 心跳值校验的分配请求字节 | 25165850 | 40 |

新构建 Public WASM 在独立 Chromium 中只消费同源合成 GET：语言切换的延迟响应期间不显示旧规则正文；
黑名单第一页失败后切第二页，等待期间不显示旧错误，完成后显示第二页及正确总数，未记录 JavaScript 异常。
本地摘要和截图位于 `target/audits/source-reaudit-20261022/`。临时数据库、浏览器及预览服务已关闭；
诊断源码与临时数据已清理。未运行测试套件、真实论坛/邮件、完整迁移、生产切换或性能基准。
同步 systemctl/备份恢复/NSS 等待、可信 UDS 空闲连接与大库完整责任扫描仍有已登记资源边界，未擅加破坏运维契约的总超时。

### 2026-10-22 再次全源码审计与资源优化（静态检查，schema 源码 57）

覆盖 Cargo 元数据中的 20 个工作区包、CI/发布脚本及部署配置；按 owner 核对模块、调用路径、SQL、取消和安全边界，
关键路径逐段读取，不把模块级审计称为逐行形式证明。没有新增依赖、数据库对象或迁移，路由、wire、金额和幂等协议保持不变。

- core：结算关联改用借用索引，删除整计划复制、折扣临时数组及重复校验；竞价资格集合使用至多两项的栈内数组，
  保留 UUID 锁序。实时 relay 移交事件所有权，不为随后只读 ID/序号深复制 JSON。
- storage：账户代理、用户黑名单历史和 recovery action 改为当前页/报告 UUID 的批量读取；四类分页计数和记录使用
  一致只读快照。纯加锁查询不收集弃用行集，历史通知载荷逐行分类；完整资金责任和历史证明仍不截断。
- HTTP/cache：只读 dashboard/CIDR 数据共享，减少请求头、路径及投影复制；Hub 先预留容量再复制事件，原位淘汰慢连接。
  缓存预算、失败回源和 WS 配额/关闭码保持不变，没有新增共享 wire 缓存。
- Worker/runtime：邮箱领取与续租按发起前单调截止收口，存储在锁后及最终 CAS 检查数据库实时时钟；论坛领取等待计入
  单项预算。control 响应流式限制 64 KiB，停机通知保留许可，心跳跳过漏 tick；tracing 关闭超时/Drop 取消所属任务，
  导出健康快照一致读取，supervisor 强制回收不因首个错误放弃其他角色。CLI 帮助恢复成功退出。
- config/setup：保留部署的 COD 证据轮询窗口，拒绝单边 Admin 凭据；runtime 启动/管理读取在解析前执行 4 MiB 预算，
  管理读取核验同一描述符，锁文件要求私有、同 UID、单链接普通文件。写后回读失败属于结果未知；设置终态与退出通知共锁。
- integrations：取消的限速等待不占未来时间槽；战斗状态只认真实 DOM 节点，论坛链接检查完整祖先链。
  分块 HMAC 和认证缓冲移交减少秘密副本，收紧失败清理；发送、附件摘要和脚本预筛选减少复制与重复扫描。
  Return 仍要求完整 RTS，未知 Send 的零条 Sent 匹配仍不能授权重寄。
- Web：响应绑定完整查询身份，换世界/场次/lot/邮箱不呈现旧查询数据，同查询后台重读保留完成快照；lot UUID 隔离读取
  与命令宿主，Admin 替换轮询时取消旧任务。表格移动已拥有行集，memo 借读查询，decoder/Fluent/Markdown 减少临时分配。
  contracts、应用及浏览器认证镜像的 Debug 遮蔽 key/token/CSRF；未观察到实际日志泄露。五语登录提示不再误称 32 位 Key。
- 发布/文档：tar 直接流式压缩，装包退出统一清理本次临时目录；修正包内部署链接、缓存复用说明和过期注释。
  分体总控停止模板由 120 秒改为 155 秒，覆盖三角色合计 135 秒及 20 秒余量；未修改已安装 unit 或线上配置。

以下是源码可数的 SQL 调用数，包含 BEGIN、隔离级别设置及 COMMIT，不是耗时或吞吐实测。N 为非空当前页/报告的项数：

| 读取路径 | 修改前 | 修改后 |
| --- | --- | --- |
| 账户列表 | `7 + 2N` | `9` |
| Admin 用户列表 | `5 + N` | `6` |
| 已存在拍卖的 Recovery 报告 | `5 + N` | `6` |

空页不执行批量补查，原调用数保持不变；最近代理竞价仍有 LATERAL 子查询，不宣称数据库计算量恒定。
新增只读快照会增加事务控制开销，取舍是响应一致性。

Rust 1.99.0 下完整 `just verify` 退出 0，覆盖策略、格式、全部已有目标、PostgreSQL/Redis 独立 feature、
全特性 Clippy（`-D warnings`）及双端 WASM。首轮发现 Admin 轮询漏导入 `dioxus::core::Task`，补齐后完整重跑通过；
随后五语提示修改再执行 `bash ci/checks/check.sh i18n wasm` 退出 0。
`cargo +1.94.1 check --locked --workspace --all-targets --all-features`、全部 CI ShellCheck、
`actionlint .github/workflows/ci.yml`、发布脚本 `bash -n` 均退出 0；按开发指南安装 cargo-machete 0.9.2 后，
`dependency-unused` 通过，未发现未用直接依赖。未重跑联网 RustSec 审计，未改依赖锁。

CodeGraph 已同步；LSP 因固定工具链未安装 rust-analyzer 而不可用，不计为通过。未新增、修改或运行行为测试，
未做手工冒烟、浏览器、SQL 执行计划/并发锁、基准、真实论坛/邮件、发布构建、迁移或部署。本轮只证明源码及静态门禁，
不宣称实际吞吐、峰值内存或生产稳定性已经测得提升。同步文件/外部子进程等待、隐藏页取消时 Waker 存留、
可信 UDS 空闲连接及大库完整责任读取的资源边界仍需获准的运行证据。


### 2026-10-22 全源码审计确认项修复（本地验证，schema 源码 57）

owner 涵盖 domain、application、storage、HTTP、integrations、jobs、config、双端 Web/shared、五个装配根及运维。
修复 P14 含 key 响应的缓存优先级；Sent 恢复与资金 spent/履约/审计的原子收口；realtime replay 的连续消费游标；
恢复拉黑和会话撤销的行时间下界；改名后已发送履约的冻结地址及旧退款 v1 精确摘要重建；多附件解析、RTS-only
退回证明和共享 Cookie 的完整逻辑操作互斥。旧原名称无法从冻结证明或同 UUID 历史重建时持久化人工核对，
不猜摘要、不重复资金化、不释放原资金或补发。

代理 Fast/SlowDue/Final 及代理间自动反超统一使用 +1000，普通出价仍用冻结的 PPM/固定经济增量；
不重写历史竞价和重放结果。Preview/Open/Final 与调度完成 fence 同事务，取消后恢复已由排期暂停的邮箱，
周邮箱命令绑定观测版本。手工排期只要求 `preview < open < end`，两小时是邮箱暂停提前量，不是手工场次最小间隔。
论坛代码支持 `One100` 等完整分类编号；全局补偿余额读取全部 world 的权威当前池合计。

runtime 写时校验复用当前部署的启动校验，并纳入非敏感的 OTLP headers 存在性门禁；迁移前检查实际 Admin
unit 身份与 runtime 0640/父目录。首次生成论坛地址使用实际 Public origin，废弃 `session_signing_key` 明确拒绝，
Sent 证据导出复用私有目录与文件/目录 fsync。追加 `000057_account_read_indexes`，原 56 个迁移不变；
本轮没有执行数据库迁移。未知 Send 的零条 Sent 匹配不是未发送证明，保留人工核对而非自动重发，相关旧说明已更正。

Public 的完整 Router 位置、关闭世界历史深链、分页纠偏 Replace、账户响应页码和 realtime 恢复状态同步；
shared 空 busy 弹窗聚焦面板、Markdown 链接与有界扫描、Time 直接订阅语言；Admin 本地时区文案和标题规范化同步。
合并 supervisor 传播关闭期子进程非零状态，同时继续回收其余角色。

完整 `just verify` 退出 0，覆盖仓库/布局/配置/i18n、格式、全部已有目标、PostgreSQL/Redis 独立 feature、
全特性 Clippy（`-D warnings`）及双端 WASM。首轮发现 watch 本地分页遗漏新增纠偏回调，补齐后完整重跑通过；
最终 `cargo fmt --all -- --check` 退出 0。`just web-build` 退出 0，每端生成 6 个 payload 文件；没有构建或部署后端发布包。

临时本地 Rust 诊断例程直接执行实际领域、Public router、邮件解析/RTS proof 和 runtime 文件适配器，均退出 0；
确认自动反超 105000→106000、人工 101000 未绕过 105000 门槛、代理间 200000→201000 和历史余数保留，
P14/lot 依赖失败响应 `no-store`、普通 rules `no-cache`，多附件及无关畸形正文可读，重复 Return 和未读 Inbox
缺席证明被拒绝、完整 RTS 被接受；规范 runtime 可接受，非规范 Origin/endpoint、headers 冲突和废弃密钥选项被拒绝。
诊断源码执行后已删除，未新增永久测试。

新构建 Public WASM 在独立本地 Chromium 中使用明确标识的合成只读响应，禁止写请求及外部域；
观察关闭世界回退、历史 Router Link 的世界/场次保留、既有时间的语言切换、自动纠偏后的 Back、空弹窗 Tab 约束、
恶意 Markdown 路径不生成链接，以及 262144 字符畸形 Markdown 完成渲染。此为隔离表面证据，不是数据库或真实业务验收。
本地摘要和两张截图位于 `target/audits/source-audit-fixes/`；临时浏览器、预览服务、服务脚本及 profile 均已关闭/清理。
未运行 PostgreSQL 并发/锁/迁移验证、真实论坛或邮件操作、认证 Admin 浏览器业务流程、测试套件或生产切换。

### 2026-10-22 安全依赖、构建工具与云端系统修复（R25 已部署，schema 56）

本条时间采用 UTC。owner 为 `ci/tools`、发布脚本、GitHub Actions 与运维；发布源码提交为 `c95dd77`，
源码指纹为 `dcd56dbe7d14dc917782f5eb67c87dc16a647f19676b7990ab780f2ecc6ce90f`。
主程序继承下条已修复的三个证书依赖版本；本轮未改主 `Cargo.lock`、业务源码、HTTP/DTO 契约或迁移。
`dx`、`sccache`、`cargo-audit` 使用独立依赖锁和受控源码构建，安装回执绑定脚本、补丁、锁文件及实际二进制摘要。
项目发布入口固定工具来源、隔离旧工具缓存和 sccache daemon；Dioxus 禁用遥测和隐式工具下载，配套工具按固定摘要安装。
本机和 CI 的 rustup 更新至 1.29.2，应用 Rust 工具链及 MSRV 保持不变。

完整 `just verify`、依赖审计/未使用依赖检查、`build-api`、`build-release` 和 `verify-artifacts` 均退出 0；
ShellCheck、actionlint 及修改脚本语法检查通过。主程序 537 个包对 1348 条 RustSec 公告审计为零漏洞、零警告；
修复后的 cargo-audit 自身锁图经 RustSec/OSV 核对无命中。构建工具完整锁图的其他公告按实际启用 feature 和调用路径逐项核对，
范围见[工具说明](ci/tools/README.md)，未向主程序审计加入忽略项。
[本次 Actions](https://github.com/ngnlAYY/Auction/actions/runs/39707515978) 的 docs、verify、MSRV、security-audit、artifacts 五项均成功，
受控工具构建、完整发布包校验、Linux 制品及校验清单上传均完成。

R25 制品位于 `target/dist/cve-security-20261022-schema56-r25/`，部署与重启证据位于
`target/dist/cve-security-20261022-ops-r25/`。已核验数据库备份，保留 R24 四程序、八份配置和 unit 定义，
按 Worker → Admin → Public 切换；schema 56 的全部迁移校验和一致，迁移检查为 no-op，配置内容、权限和 owner 均保持一致。
重启前后均核对三项 readiness 为 200、四程序及进程摘要匹配、双端 origin 和 Public HTTPS 共 16 项资源匹配。
重启后 30 秒观察期内无自动重启，启动日志 error/warning 均为 0，三个 unit 的原启用状态和链接均已还原。
部署和重启汇总为 `DEPLOYMENT-SUMMARY.json`、`REBOOT-SUMMARY-RECONCILED.json`，包含封存与补证回执摘要链。

云端内核已重启生效为 `6.12.113+deb13-amd64`；Caddy 2.11.7 在同一官方源码上以 Go 1.26.10、`x/net 0.60.0`
重新构建并替换，二进制漏洞扫描未发现受影响的包或符号，实际进程摘要及代理配置一致。
SSH 通过 `Compression no` 缓解压缩相关问题，并核验有效配置及新连接；系统 CA 已移除 emSign ECC Root CA - G3，
curl/libcurl 为发行版修复包 `8.14.1-2+deb13u7`。平台最终回执为 `target/cve-remediation-20261022-platform-final.json`。
后续官方 Caddy 包升级须继续核对编译器和依赖修复基线，SSH 缓解待发行版补丁后再评估。

维护中，disabled/inactive unit 被 systemd 回收，导致首次停机退出字段检查和重启后的对象读取门禁失败；
一次单次属性读取尝试又因空 hook 字段被省略而停止。所有失败现场均保留，期间服务保持停止和禁用。
按原 Worker 的 boot/PID/InvocationID 联核 drain 日志与 PID 1 正常停止事件后完成补证；
随后通过同一 D-Bus 连接持有 `RefUnit`，完整复用原停止契约和恢复门禁成功启动，运维规则已同步。
本轮运行核对限定为依赖、发布、配置、schema、备份、受控重启、就绪及静态资源，未运行单元、集成或浏览器业务测试。

### 2026-10-21 GitHub CI 证书依赖安全审计修复（本地静态检查）

提交 `ad87dd4` 的 [Actions 运行](https://github.com/ngnlAYY/Auction/actions/runs/39615922774)
在 `security-audit` 命中新公布的三个证书信任公告；同次 docs、verify 与 MSRV job 已通过，制品 job 被阻断。
依赖 owner 为根 `Cargo.lock`，仅更新 `rustls-webpki 0.103.15 → 0.103.16`、
`webpki-root-certs 1.0.9 → 1.0.10` 和 `webpki-roots 1.0.9 → 1.0.10`，对应
[RUSTSEC-2026-0382](https://github.com/RustSec/advisory-db/blob/main/crates/rustls-webpki/RUSTSEC-2026-0382.md)、
[RUSTSEC-2026-0383](https://github.com/RustSec/advisory-db/blob/main/crates/webpki-root-certs/RUSTSEC-2026-0383.md) 和
[RUSTSEC-2026-0381](https://github.com/RustSec/advisory-db/blob/main/crates/webpki-roots/RUSTSEC-2026-0381.md)。
锁图仍为 537 个包，其余版本与依赖关系保持一致；三个补丁的 MSRV 均为 1.71，项目最低版本仍为 1.94.1。
CI 保留 `cargo audit --deny warnings`，未增加忽略项或调整 TLS feature、信任配置及应用外部契约。

完整 `just verify`、`cargo +1.94.1 check --locked --workspace --all-targets --all-features`、
`bash ci/checks/check.sh dependency-audit` 和 `dependency-unused` 均退出 0。
使用与 CI 相同的 cargo-audit 0.22.2、cargo-machete 0.9.2；RustSec 数据库提交
`4b587f00996ebe5c05a73eaef105ef241e876414` 共 1348 条公告，审计为零漏洞、零警告。
日志位于 `target/github-security-fix-{verify,msrv,audit,unused}.log`；本轮未运行行为测试。

### 2026-10-20 装备详情事实栏单行排列（R24 已部署，schema 56）

owner 为 `auction-web-public`。详情将装备分组、分组值、标记按钮、分类、等级与平均 Roll 保持在同一行，
标签和值、星形与标记状态同行；移除分组跨行和手机端纵排规则。窄屏仅此信息栏支持触控及键盘横向查看，
保留按钮命中区域、焦点提示与错误反馈。路由、DTO、标记命令、配置及数据库契约保持一致。

完整 `just verify`、`just web-build`、`just build-api`、`just build-release`、`just verify-artifacts` 均退出 0。
按既有部署授权封存当前工作树，源码指纹为 `5e1d6d826cbb5ad92d2875603d600665fe3999d5ae5ef34d3a1ec7383141dcdb`；
制品位于 `target/dist/inline-facts-20261020-schema56-r24/`，运维证据位于 `target/dist/inline-facts-20261020-ops-r24/`。
相对 R23 仅 Public 程序及其 Web payload 变化，其他三个程序逐字节一致。

已验证数据库备份并保留 R23 四程序、八份配置及服务定义，按 Worker → Admin → Public 完成切换。
56 项迁移校验和一致，迁移检查为 no-op；配置内容与权限保持一致。独立后验的三项 readiness 均为 200，
自动重启次数为 0，本次启动日志 error/warning 均为 0，双端 origin 与 Public HTTPS manifest/WASM 共 16 项资源匹配封存。
回执为 `cloud-evidence-r24/final-r24-confirmed.json`、`web-postflight-r24.json`、`DEPLOYMENT-SUMMARY.json`。
本轮完成源码、构建资源及部署核对，未运行浏览器或业务流程测试。

### 2026-10-20 移动适配、独立分组列与 MM 正文（R23 已部署，schema 56）

按用户要求，将双端手机布局、公开页“装备 → 分组 → 平均 Roll”列顺序、详情分组行标记按钮、同步提示精简及
英文 MM 正文发布到现有云端分体环境。Web owner 为 Public/Admin/shared；邮件模板与价格明细由
domain/application/config/storage/integrations 共同承接，旧三变量待发任务继续兼容，新竞得通知使用冻结结算明细。

完整 `just verify`、`just build-api`、`just build-release`、`just verify-artifacts` 均退出 0。
本次按用户明确部署授权封存当前工作树，记录实际 dirty 状态、HEAD、全部文件状态与构建前后相同的源码指纹
`310240ddeb5c762376e5f937c1308519342dcc000e105114112228fe0877fc76`；制品位于
`target/dist/mobile-mail-20261020-schema56-r23/`，运维证据位于 `target/dist/mobile-mail-20261020-ops-r23/`。

已验证数据库备份，保留 R22 四程序、现场全部配置及服务定义；停稳后持有部署锁和既有 runtime 文件锁，
核对配置命令账本无 pending/unknown，仅替换 `runtime/admin.toml` 的 14 项 `mail_templates` 值。
其余解析值、模板表之外的原始字节、各配置 owner/mode 均保持一致。新正文在共享 runtime 中显式生效，
未修改邮件启用开关、发送预算、凭据或其他业务参数。尝试启动新版本后，因八变量待发任务的兼容边界，禁止自动回退 R22。

56 条迁移校验和全部一致，迁移检查为 no-op；按 Worker → Admin → Public 启动后，四程序和运行进程摘要匹配封存，
三项 readiness 均为 200，自动重启次数为 0，本次启动日志 error/warning 均为 0。
双端 origin 资源与 Public HTTPS manifest/WASM 共 16 项逐字节匹配；回执为
`cloud-evidence-r23/final-r23-confirmed.json`、`web-postflight-r23.json` 和 `DEPLOYMENT-SUMMARY.json`。
本次运行核对限定为部署、配置、schema、服务就绪及静态资源，未进行真实竞价、测试寄信或手机浏览器流程验证。

### 2026-10-20 双端移动适配与公开页独立分组列（本地静态检查）

owner 为 `auction-web-public`、`auction-web-admin` 和 `auction-web-shared`。双端增加手机与触控横屏布局、
安全区、原生输入提示及触控操作间距；共享层按 VisualViewport 调整弹窗可见范围，完善隐藏控件焦点过滤和
弹窗内长文本浮层。业务表保留原生表头、行列及完整百分比宽度，管理端黑名单历史支持点击查看。

公开页预览、竞价与已结算目录新增纯文字分组列，顺序统一为“装备、分组、平均 Roll”；详情标记按钮移到装备分组行。
移除服务器标记加载及未同步缓存提示，保留加载忙碌语义、失败反馈与修改门禁。路由、DTO、配置和数据库结构保持一致。

Rust 1.99.0 / Dioxus CLI 0.7.10 下，完整 `just verify` 退出 0；双端 Web 发布资源构建成功，
随后 `just web-build` 核对源码指纹与 payload 清单并复用成功，每端 6 个文件。完整静态日志为
`target/mobile-ui-verify-final.log`，构建日志为 `target/mobile-public-build.log`、`target/mobile-admin-build.log`。
本次未运行行为测试或部署到云端；手机浏览器实际操作仍待单独验证。

### 2026-10-20 公开页装备列移除分组标记（R22 已部署，schema 56）

按用户要求，Public 的实时、已结算及预览目录装备列仅显示名称；顶部装备分组筛选及详情分组字段保留。
owner 为 `auction-web-public`，路由、DTO、分组规则、配置和数据库结构保持一致；当前展示契约与玩家指南同步。

完整 `just verify`、`just build-api`、`just build-release`、`just verify-artifacts` 均退出 0，封存源码指纹为
`5b7a3c41c15cadccd90e26e73ae8a9d707246172830d8818b349e61fa12165da`。
制品位于 `target/dist/equipment-groups-20261020-schema56-r22/`，证据位于
`target/dist/equipment-groups-20261020-ops-r22/`。相对 R21 仅 Public 程序和公开页 payload 变化，
已备份数据库、R21 四程序、配置与服务定义，按现有完整发布流程切换。
全部配置及 56 条迁移校验和一致，三服务 readiness 均为 200、自动重启为 0，启动日志 error/warning 为 0。

双端 origin 资源与 Public HTTPS manifest/WASM 共 16 项逐字节匹配封存；浏览器只读核对默认列表及稀有筛选，
两次可见的 10 个装备名称单元格均无分组标记，筛选仍位于类型上方，稀有筛选返回总数 17。
回执为 `final-r22-confirmed.json`、`web-postflight-r22.json`、`browser-postflight-r22.json`；未新增或运行其他行为测试。

### 2026-10-20 装备解析与公开页分组（R21 已部署，schema 56）

将下述装备种类/部位解析、Admin 三品质候选及配置默认收起、Public 分组筛选发布到现有云端分体环境。
owner 为 domain、application、storage、integrations、contracts/HTTP、config 与双端 Web；本次未新增迁移，
全部 56 条迁移校验和保持一致。正式拍品排序和编号保持原值，测试帖排序与重新编号仍仅用于展示验证。

部署前完整 `just verify`、`just build-api`、`just build-release`、`just verify-artifacts` 均退出 0。
封存源码指纹为 `ee0067cde242959c5ebe52cc20de68043d43076096697c45b84aeded9944ae65`；制品目录为
`target/dist/equipment-groups-20261020-schema56-r21/`，运维证据为 `target/dist/equipment-groups-20261020-ops-r21/`。
已备份并核验数据库，同时保存旧四程序、配置及服务定义；Public 配置增加对现有 `runtime/admin.toml` 的引用，
其余配置保持一致。按 Worker → Admin → Public 启动后，四程序哈希匹配封存，三服务 readiness 均为 200、
自动重启次数为 0，本次启动日志的 error/warning 计数均为 0。

限定只读部署核对确认预览装备共 44 件，其中稀有 17 件、融合 27 件；稀有分页和“融合 + 布甲”16 件结果正确，
原有装备字段、编号与顺序保持一致。浏览器确认装备分组位于类型上方；双端 origin 资源与公开 HTTPS manifest/WASM
共 16 项逐字节匹配封存。回执为 `final-r21-confirmed.json`、`public-postflight-r21.json`、`web-postflight-r21.json`；
本次未执行竞价或管理业务写入验证。

### 2026-10-20 云端预览装备的论坛标色展示（单帖验证完成）

按用户明确授权，只读云端实际两条标色规则、Main 第 2 期 preview 的 44 件装备及原始属性；
快照为 `preview_revision=15`、`state_version=17`。临时 Rust helper 复用当前 `ForumThreadRenderer`，
去掉正式运行标记、倒计时和竞价操作说明，加入展示测试声明，经已登录浏览器在 Test Forum（板块 4）发布
[测试主题 302239](https://forums.e-hentai.org/index.php?showtopic=302239)。发布后页面确认 44 个装备链接、
16 件红色后缀、1 件 Peerless 彩虹品质词、27 件未标色装备，测试声明保留，未新增续帖。
输入快照、正文及发布回读计数/哈希保存在 `target/audits/forum-color-preview-20261020/`，最终回执为
`published-result.json`。本次覆盖论坛渲染与真实发帖展示；未部署新 Public 分组功能，也未修改云端配置或拍卖数据。

随后按用户选定的测试帖范围，在同一主题内按装备类型、纯种类、前缀、护甲部位、后缀及品质排序，
各类型从 `01` 连续生成示例编号，44 件中 38 件编号变化；同种类与前缀连续展示。
排序使用当前名称解析，纠正快照中一件 `Great Mace` 被旧属性拆为 `Mace` / `Ethereal Great` 的排序键。
本地逐装备核对除编号外的行内容与链接一致，提交后重新打开主题确认 44 件顺序与编号全部匹配，
仍为 17 件上色、27 件未上色。原正文、编号对照、局部校验及发布回读分别保存为
`test-post-original.json`、`renumbering.json`、`sort-verification.json` 和 `sort-published-result.json`；
本轮仅调整展示测试 helper 与测试帖，正式编号和云端拍卖数据保持原样。

用户进一步明确 `Great Mace` 与 `Mace` 是同一种类的新旧名称。测试 helper 随后将二者归为同一排序组，
保留装备名称原文，按前缀将三件双手武器依次编号为 `Two01` Demonic Mace、`Two02` Ethereal Great Mace、
`Two03` Tempestuous Great Mace；其他类别的排序和编号保持一致。更新同一测试帖后回读确认全部 44 件
顺序和编号匹配，标色计数保持 17 / 27；上一轮制品保存在 `*-pre-mace-alias.json`，本轮回执为
`mace-alias-published-result.json`。此别名归组仅应用于测试展示。

随后按用户要求，仅将测试帖布甲改为后缀、纯种类、前缀、部位、品质的优先顺序，16 件布甲按
`Clo01`—`Clo16` 连续编号，其中 14 件编号较上一版变化；非布甲顺序、编号及 Mace 别名归组保持一致。
本地核对逐装备正文除编号外完全一致，发布回读确认全部 44 件顺序与编号匹配、标色计数仍为 17 / 27。
上一版保存在 `*-pre-cloth-suffix.json`，本轮回执为 `cloth-suffix-published-result.json`；范围仍为展示测试。

### 2026-10-20 公开页稀有与融合分组（源码完成，静态检查通过）

application 统一论坛实际标色与装备分组判定，Peerless 固定彩虹归稀有；Public 消费 `rare` / `fusion`，
分组筛选位于装备类型上方，列表标签与详情字段共用服务端结果。URL、翻页、返回与实时刷新保留组合条件。
storage 在同一 Serializable 快照中先按分组筛选和统计，再读取当前页；不选分组保留既有 SQL 分页路径。
HTTP/DTO、严格浏览器解码、五语文案与配置说明同步，字段和统计范围见[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)。

Public 从三角色共享 runtime 读取装备规则窄投影，分体/合并预检要求同源且规则快照一致；初始配置生成和示例同步。
没有新增迁移或改写装备/拍卖事实。旧部署的 Public runtime 引用及 Public/Worker 重启要求见
[配置说明](ops/config/README.md#runtime-共享与重启)。

完整 `just verify` 退出 0，覆盖策略、格式、工作区全部目标、独立 PostgreSQL/Redis feature、全特性 Clippy
（`-D warnings`）及双端 WASM；日志为 `target/audits/equipment-groups-20261020/verify.log`。
最后补充 Admin 保存后的重启提示及模块文档，仓库/布局/配置/i18n 门禁、Admin WASM 编译和 `git diff --check` 再次通过；
补充编译日志为 `target/audits/equipment-groups-20261020/admin-wasm-final.log`。
本轮未新增或运行行为测试，未发布部署；浏览器交互及真实数据库执行留待明确授权后的运行验证。

### 2026-10-20 装备种类与护甲部位拆分（源码完成，静态检查通过）

domain 名称结果独立保存种类、部位词与语义部位，仅 Cloth/Light/Heavy 有部位；类型仍取详情 header 并交叉校验。
Legendary 新来源名称必须具有前后缀，Magnificent 保留可缺词缀；解析词典保留其它品质兼容。
integrations 将历史完整基础名明确映射到 canonical `equipment_type`、快照 `attributes.type` 及既有指纹，
保留历史数据、精确规则和命令结果。Admin 新种类候选只列非护甲种类及护甲种类族，已有完整名继续回显，
品质推荐候选仅含 Peerless/Legendary/Magnificent；相关边界见[装备录入不变量](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)。

Rust 1.99.0 下完整 `just verify` 退出 0，覆盖策略、格式、工作区全部目标、独立 PostgreSQL/Redis feature、
全特性 Clippy（`-D warnings`）及双端 WASM；日志为 `target/audits/equipment-name-semantics-20261020/verify.log`。
本次仅完成源码与静态检查，未新增或运行行为测试，未发布部署；下述 R20 发布证据保留为原版本记录。

### 2026-10-20 统一装备解析与复审修复（R20 已上线，schema 56）

经明确部署授权，将下述统一词典、分层解析、Admin 服务端候选及复审修复一并发布到现有分体环境。
owner 为 domain、integrations、application、contracts/HTTP、Admin 与 storage；新增 `000056` 扩展诊断约束，
保留历史数据和前 55 条迁移校验和。原静态检查范围及外部契约见后两条源码记录。

embedded 发布构建与独立封存通过；四个 GNU/Linux x86_64 程序均更新，使用通用 CPU 配置，
16 项文件清单、双端 payload 隔离及全部 56 条内嵌迁移核对通过。
构建起止源码指纹为 `e03b559f2070966ecc4e5570005ab043151afcfd15ac4b980433457e2e236c21`，
`SHA256SUMS` 摘要为 `e02b8406e3adc7d8ce7d37b022d3851a2accd8402c75bfc34c3868cf56b017fe`。
Public payload 为 `5c1bce09d5ab4933723135574cb5f2e47ff403550d1ec7e7b42cd2d861423953`，
Admin payload 为 `3476c548b5c2ad46fd3cc4a0306ff976b9bbe9bf31b1fe52be387c3c99ddbb02`。

2026-10-20 13:18:17 UTC 完成正常 Worker 排空与三角色停止；使用旧版控制器完成 schema 55 数据库备份，
新版控制器离线核验归档及 manifest 后原子替换四程序，由 `deployment start` 持锁执行 55→56 迁移。
13:18:18 UTC 完成 Worker → Admin → Public 启动；13:18:23 UTC 独立后验确认三 readiness 均为 200，
四个磁盘/运行映像与发布包一致，全部 56 条迁移成功且 checksum 匹配，启动回执与 PID/InvocationID 一致。
13:19:45 UTC 复核 PID 未变、自动重启为零，新进程日志均为 INFO；八份配置与本次部署前快照一致，
保留了上次发布后用户对 runtime 的修改。迁移尝试后的失败路径只安全停机，不自动恢复旧程序或数据库。

静态资源后验中，两端 Unix 源站各七项文件全部返回 200 且 SHA-256 匹配封存包；公网 Public 的
payload 标记和 WASM 同样匹配。公网首页为 Cloudflare `DYNAMIC`/`no-store` 响应，
源站首页 5,425 字节与封存一致，公网增加 1,472 字节并改写模块 script 类型，涉及 Rocket Loader、
Insights 与 challenge-platform；原资源引用保留。因此保留原始 17 项检查中的首页字节不等结果，
另存 `index-https-comparison-r20/` 的差异证据。仅移除三段已识别 Cloudflare 注入脚本、模块类型动态前缀
及额外换行后，剩余 HTML 与源站/封存逐字节一致；没有修改 CDN 或代理配置。

封存包在 `target/dist/equipment-audit-fixes-20261020-schema56-r20/`，构建、封包、安装回执及
124 项已核验云端证据在 `target/dist/equipment-audit-fixes-20261020-ops-r20/`。
云端证据与旧四程序/配置备份在 `backups/equipment-audit-fixes-20261020-schema56-r20/`，
数据库归档及 manifest 保留于独立 root 私有目录。
本次执行发布构建、迁移及部署后验，没有新增或运行行为测试套件。

### 2026-10-20 装备重构复审修复（源码完成，静态检查通过）

本次复审发现前轮解析诊断的 Rust 闭集扩展未同步数据库 CHECK；storage 追加 `000056`，
仅为 parse/protocol_drift 放行九个来源原因，保留原证据约束和历史空原因，避免解析拒收被旁路诊断阻断入站/游标事务。
`ci/checks/equipment-contract.sh` 只读比对枚举、最新约束、Admin 解码和五语言说明，接入默认 `all`/`just verify`。
迁移、执行顺序与不改写历史数据的边界见[数据库契约](docs/contracts/schema.md)及[诊断说明](ops/mail/diagnostics.md#装备读取失败诊断)。

integrations 对 HTML 唯一 header、必要属性区域和非空属性集合进行校验；已识别属性行的非法 Base 或列损坏明确报错，
保留合法可选说明，普通连字符文本不视为数值。MM 诊断读取复用 URL/allowlist 校验，canonical JSON 字段/指纹不变。
domain 缓存最长种类匹配顺序并复用已解析种类和静态词缀切片；HTTP 缓存静态词典主体，
为当前文档已有的合法有限语法种类增加独立 `resolved_kinds`，不混入确证目录或配置指纹。
Admin 精确关联这些候选，并在同词典版本刷新时保留草稿仍选中的既有服务端解析项。
没有新增依赖、路由、运行配置字段或业务表；相关 owner 和响应契约同步于模块文档及[架构](docs/design/architecture.md)。

定向 domain/integrations/contracts/HTTP/Admin 编译、格式、Shell 语法与 ShellCheck、诊断静态门禁通过；
各实现已交叉源码复核。完整 `just verify` 退出 0，覆盖新增诊断闭集门禁、仓库/布局/配置/i18n、格式、
工作区全部目标、独立 PostgreSQL/Redis feature、全特性 Clippy（`-D warnings`）及双端 WASM；
日志为 `target/audits/equipment-audit-fixes-20261020/verify.log`，最终差异检查通过。
本次未新增或运行行为测试，未执行数据库迁移、服务启动或云端部署。以下历史验证记录保留原样。

### 2026-10-20 统一装备词典与分层解析重构（源码完成，静态检查通过）

`auction-domain::equipment::taxonomy` 唯一拥有七大类、77 个确证完整种类、十二个可选材质族和历史
Chainmail、十档现行及两档历史品质、五个语义部位、词缀推荐和历史兼容关系。词条保留 Wiki 来源及修订，
Chain/Reactive 的八个 Heavy 部位仅作受限读取语法，不展开为确证候选；最长完整种类优先，
未知种类和非法名称明确拒绝，未知前后缀保留原文。Astral/Quintessential 的历史识别限武器大类，
自由词缀、规范身份字段及旧 `the ` 比较口径保持兼容。

`auction-integrations` 将页面提取、header 等级/交易状态和领域身份适配分离；HTML 与 canonical JSON
共用词典并交叉校验大类。未赋等级、普通等级、Tier 和未声明等级在进程内分开，既有 DTO、属性 JSON
和指纹仍投影为原 `Option<i32>`。`auction-application` 保持原收件品质、交易资格、等级条件与旧标色
单值精确匹配，材质归类及后缀比较改为复用领域词典；既有装备、审计、命令摘要和配置指纹不重写。

contracts/HTTP 为三条 runtime 配置路由增加显式 `include_equipment_taxonomy=true` 的只读投影选择，
无查询请求仍返回旧三字段集合；Admin 删除本地分类和词缀副本，读取服务端候选、保留已选历史值，
词典刷新独立于草稿和保存文档。来源解析的九项闭集原因接入现有 MM 诊断 sidecar 和五语言说明，
Admin 手工录入保留原 HTTP 错误并记录安全闭集诊断。没有新增依赖、数据库迁移、运行配置字段或路由。
owner、兼容边界和响应字段见[架构](docs/design/architecture.md#crate-所有权)、
[装备录入不变量](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)及
[运行配置契约](docs/contracts/routes.md#runtime-配置与文件事务)。

完成领域/集成交叉源码复核、HTTP/DTO/诊断链独立复核；初次及最终源码的完整 `just verify` 均退出 0，
覆盖格式、仓库/布局/配置/i18n、工作区全部目标、独立 feature、全特性 Clippy 和双端 WASM。
静态证据位于 `target/audits/equipment-taxonomy-20261020/verify.log` 与 `verify-final.log`；
本轮没有新增或运行行为测试，也未执行云端部署。
此前 R19 的发布证据和历史结果保留如下。

### 2026-10-20 Wiki 装备解析与标色兼容修复（R19 包已上线）

本轮针对 Wiki 核对发现的五项问题：装备解析补齐种类、纠正 `Leather Helmet`、按完整名称识别 `Great Mace`，
并将明确的 `Level Unassigned` 映射为缺省等级。精确词表共 77 项，包含 22 项武器/盾牌、50 项护甲和
5 项旧 Chainmail 完整种类；Chain/Reactive 仅按精确材质及八个 Heavy 公共部位提供受限读取语法，
不将所有组合声明为确证的表单选项。来源适配仍校验种类与大类一致，
默认收件品质、可交易性、等级限制及录入事务沿用原有规则，既有装备快照与审计事实不自动改写。
Admin 标色表单切换类型或种类只更新推荐候选，保留其它维度的全部已选值；
历史精确种类不会被自动清空成不限条件，历史前后缀也不会因当前推荐列表缺项而消失。
模块 owner 为 `auction-integrations` 与 `auction-web-admin`，没有新增 HTTP/JSON 字段、配置字段或数据库迁移。
契约见[装备录入不变量](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)
及[标色编辑规则](docs/contracts/routes.md#runtime-配置与文件事务)。

完整 `just verify` 退出 0，覆盖格式、仓库/布局/配置/i18n、工作区全部目标、独立 feature、
全特性 Clippy 及双端 WASM。解析器与 Admin 变更分别完成独立源码复核。
正式 embedded 发布构建与封存退出 0；16 项清单、42 项 Public 和 21 项 Admin 标记、GNU/glibc、
无 CPU 特化、资源隔离及全部 55 项迁移校验和通过。构建起止源码指纹均为
`fa57f191708d6d3d93e85245c41ac4b27ef4e2811b7300e93e76acec11a60c9f`，`SHA256SUMS` 摘要为
`4fd7c91d6061f7f8c826e86eac1003c85968d3d0a13e43736511a4b2ef5cee7e`。
R19 包更新 Admin/Worker 内容；Public 与 `auctionctl` 字节复用 R18，并在部署计划及安装回执中分别记录包版本和内容版本。
Public payload 仍为 `5c1bce09d5ab4933723135574cb5f2e47ff403550d1ec7e7b42cd2d861423953`，
Admin payload 为 `7d8f059e9fa501c260c19d996f697c0f4734c8743b2046e6fbfe5508b4435cc6`。

2026-10-20 11:24:19 UTC 完成正常 Worker 排空及三角色停止，验证停机数据库备份后原子替换四程序；
11:24:21 UTC 总控完成 Worker → Admin → Public 恢复，迁移为 schema 55 的 no-op。
11:24:26 UTC 独立后验确认三 readiness 均为 200、三 receipt matched，`started` 回执与实际
PID/InvocationID/单调启动时间一致，自动重启为零。Worker/Admin/Public PID 分别为 163634/163650/163658，
四个运行或磁盘映像均匹配 R19 包清单；线上 runtime 配置八文件哈希保持原样。
本轮执行静态检查、发布构建和受控部署检查，没有新增或运行行为测试套件。

封存包在 `target/dist/equipment-parser-compat-20261020-schema55-r19/`；静态日志 `verify-r19.log`、
构建封存回执、安装回执及 124 项已逐项核对的云端证据在
`target/dist/equipment-parser-compat-20261020-ops-r19/`。
`final-all-r19-confirmed.json` SHA-256 为
`e22077f5ab158746bb791315c40eb44599bf4d252b32d34ff11aa1351987ce30`。
云端记录在 `backups/equipment-parser-compat-20261020T190000Z-r19/`，数据库备份独立保存在
root 私有目录并已验证归档及 manifest 校验和；旧四程序与八份配置备份保留。

### 2026-10-20 预览静默更新、已结算列与标色多选（R18 已上线）

Public 的 `surface/board` 按账号 UUID、世界、场次、阶段及筛选分页隔离完整目录快照；预览继续每 60 秒
在可见页面后台读取、恢复可见时补读。同一上下文更新期间保留已对齐目录，修订对齐后整体替换，不再用加载提示替换列表。
首次读取、上下文切换和明确失败继续使用既有反馈，对齐补读显式订阅状态与修订键。`surface/catalog` 按权威
`auction_status=settled` 同步去掉列宽、表头和行操作单元格，保留真实成交投影及完整十一列百分比布局。

Admin 的 `operations/config/form/equipment_colors` 使用原生复选框选择类型、种类、品质与前后缀；
支持 72 个完整种类、12 个护甲材质总类、10 档品质。护甲按材质选择全部部位或按完整种类精确选择，词缀按多选范围求并集。
应用与配置 owner 增加可省略空数组的 `categories`、`equipment_types`、`qualities`，同维 OR、跨维 AND；
新种类数组支持带实际大类及完整词边界校验的材质族，旧单值继续精确匹配，表单保留未编辑的旧材质精确规则。
HTTP、runtime TOML、持久命令及论坛渲染沿用同一完整文档；规则仍在 Worker 启动时冻结，收件允许策略和 schema 不变。
契约与说明同步至 [HTTP 契约](docs/contracts/routes.md#runtime-配置与文件事务)、[Admin 设计](crates/auction-web-admin/DESIGN.md#运行配置)
及 [Public 设计](crates/auction-web-public/DESIGN.md#6-竞价与代理交互)。

完整 `just verify`、正式 embedded 发布构建和封存均退出 0；16 项清单、42 项 Public 及 20 项 Admin 标记、
GNU/glibc、无 CPU 特化、资源隔离及全部 55 项迁移校验和通过。R18 构建起止源码指纹均为
`5aa19aa0b7c79eaf239a38f582a33d31f023f961586db447a9e71ec604813f19`，`SHA256SUMS` 摘要为
`76296b4163a07efbc317763b96c7c2e86570cb240158e1819dc1f2552d5e03c3`。
首次部署预检因 systemd 基础属性返回标量而在停止服务前退出；保留该记录，修正部署脚本后使用同一封存包重试。

2026-10-20 10:29:59 UTC 完成正常 Worker 排空及三角色停止，验证停机数据库备份后原子替换四程序，
10:30:01 UTC 由新总控按 Worker → Admin → Public 恢复；迁移为 schema 55 的 no-op。10:30:04 UTC 独立后验确认
三 readiness 均为 200、三 receipt matched、`started` 回执与实际 PID/InvocationID/单调启动时间一致，自动重启为零。
Worker/Admin/Public PID 分别为 141407/141428/141436，四个运行或磁盘映像均匹配 R18。
本轮仅更新程序，线上 runtime 配置八文件哈希保持原样，没有运行行为测试套件或修改线上标色规则。

封存包在 `target/dist/preview-silent-settled-actions-20261020-schema55-r18/`，构建与静态证据在
`target/dist/preview-silent-settled-actions-20261020-ops/`，成功部署证据在同名 `-ops-retry1/`；
124 项云端证据已下载并逐项核对，`final-all-r18-confirmed.json` SHA-256 为
`f27656742b74c85fa168481eef2ee1e8478193a169e81df6fa2970001545cdef`。
云端记录在 `backups/preview-silent-multiselect-settled-20261020T180000Z-retry1/`，数据库备份独立保存在
root 私有目录并已验证归档及 manifest 校验和；旧四程序与八份配置备份保留。

### 2026-10-20 全装备大类标色词缀核对（Admin 已上线）

`auction-web-admin` 的 `operations/config/form/equipment_colors/options/` 独占标色表单的分类选项，
按 Equipment Basics 的大类与 Detailed Equip Characteristics 的材质、种类关联词缀。
原先 Cloth、Light、Heavy 共用护甲总表，导致分类混用且漏掉 Shade、Power 等专属词条；现在各大类按本类材质求去重并集，
Light/Heavy 分开，兼容后缀 `the `/`The `，已有候选原文保留，显式切换分类后才清理不兼容选项。
其他五大类也已逐项核对：补齐六种武器/盾牌候选、Rapier 的 Swiftness、Katalox 的熟练度后缀；
Wiki 总表与分项说明不一致时采用两处明确词条的兼容并集。七大类、22 种武器/盾牌及 12 种护甲材质的展示口径、
来源与完整 Light/Heavy 对应表见 [Admin 设计](crates/auction-web-admin/DESIGN.md#运行配置)。
模块说明及架构 owner 同步；HTTP/DTO、配置字段、采集接受策略、数据库与既有规则值均沿用原契约。

完整 `just verify`、正式 embedded 发布构建及封存门禁均退出 0；十六项清单、37 项 Public 与 13 项 Admin 资源标记、
GNU/glibc、无 CPU 特化、双端资源隔离及 1–55 全部迁移 checksum 核对通过。
R16 构建起止源码指纹均为 `1a6435c92eeb915c3dca8ea9765cfe38ded60c933c26c7fe9a1001e1f3a8a366`，
`SHA256SUMS` 摘要为 `6d74a111470bb088c1960a9b56d72f69d8faa0931cc5372aa676caaf8a21e605`；
Admin payload 为 `86a92fbd90eab8e672f2a0b276e4756121a135e1fe42ced5c4952988a431014b`，
Admin ELF 为 `6b85a6cec3c88c5160e11390d3b303ebb7aec4acd84a7b49edf427709a418018`，Public payload 与 ELF 均保持 R14。

生产预检核对 55 项 SQLx checksum、进程/磁盘映像、既有回执与停止依赖闭包：Public 的反向停止关系为空，
Admin 的 RequiredBy 精确仅为本次受控的 Public，其他停止传播关系及两个停止钩子均为空；Worker 的 StopWhenUnneeded 为 no。
仅按 Public → Admin 停止 HTTP 角色，原子替换 Admin，再按 Admin → Public 启动，以满足既有 Worker < Admin < Public
启动时间硬门，并通过旧总控 all-running `start` 取得 `adopted` 回执。2026-10-20 09:45:08 UTC 完成安装，
09:45:11 UTC 后验三个 Unix readiness 均为 200、三份 receipt 均 matched、自动重启为零。
Admin PID 123303 运行映像匹配 R16；Public PID 123319 继续使用 R14 映像。
Worker PID 97793 的映像、InvocationID、单调启动时间和磁盘 `auctionctl` 均保持 R9，数据库未迁移。

R16 封存包在 `target/dist/equipment-color-taxonomy-all-20261020-schema55-r16/`，本地证据在
`target/dist/equipment-color-taxonomy-all-20261020-ops/`，云端证据在 `backups/equipment-color-taxonomy-all-20261020T094000Z/`。
158 项云端证据已下载并逐项核对；`final-admin-r16.json` SHA-256 为
`4ab55b4383ec6c7be4cdec65b53a9391bc4bcf5b1cf23d6c7452e0582dfd201b`。
本轮范围为源码与 Wiki 核对、静态检查、制品检查、已授权发布及就绪/回执观察；没有运行行为测试套件或修改线上标色配置。

### 2026-10-20 操作顺序、账户后台更新及手动刷新反馈（Public 已上线）

`auction-web-public` 将开拍操作列调整为 18%，装备名称列为 22%；按钮按出价、代理、标记的 DOM 顺序显示，
两级等分 grid 为完整标记文字分配较宽位置，保留窄屏换行与键盘顺序。五语言未选中文字同步为“未标记”等对应译文。
账户当期标记列表在后台加载时保留已完成快照和表格节点，取消与竞价事件无关的重读；异步身份复核使用非订阅读取，
渲染身份校验与切换隔离保留。实时 owner 仅在已认证且权威场次为 `live` 或 `final` 时创建 WebSocket；
预览显示“预览”，其余非竞价阶段停用实时，继续使用既有分钟轮询与恢复可见刷新发现开拍。
“立即刷新”显示旋转图标和五语言忙碌文字、禁用重复点击，随后执行整页 reload；保留 reduced-motion 与导航失败提示。
自动更新仍由资源读取完成。模块说明、Public 设计、架构、表格与客户端实时启用说明同步；外部 wire、配置及 schema 保持既有契约。

完整 `just verify` 与正式 embedded 发布构建均退出 0；十六项封存、GNU/glibc、无 CPU 特化、双端资源隔离及旧页面标记检查通过。
R14 构建起止源码指纹均为 `48bc0dfa9450ac8c28801e08e99eda8e598fd56b49d64268db19110a63d84620`，
`SHA256SUMS` 摘要为 `c0ab51ddf738f35e2015d2f267cda058e0378affbe440c3dcb50004dfe5820c7`；
Public payload 为 `0e477510373829d120d1c6dbe38f83494c1b31f95e39f037859b9f7b95c16043`，
Public ELF 为 `0981c27bf01c05c459281b7a2b8493996deca3dd1024a244f402c5c93f5d4f26`。

生产预检只读核对 schema 55 全部 SQLx checksum；2026-10-20 08:57:26 UTC 仅 Public 更新至 R14，
随后既有总控以 all-running `start` 完成 `adopted` 回执。08:57:29 UTC 后验确认三份 receipt 均 matched、
三个 Unix readiness 均为 200、自动重启为零，Public PID 104088 的运行映像与上述 ELF 一致。
Worker PID 97793、Admin PID 97809 的运行映像、InvocationID 与单调启动时间，以及磁盘 `auctionctl` 均保持 R9；数据库未迁移。

用户授权的真实账号只读观察中，旧版账户表格在分钟更新时被移除；R14 同一表格节点跨过该周期仍保留。
全新已登录浏览器会话观察中，预览徽标为 `Preview`/`idle`，没有创建 WebSocket；
点击“立即刷新”观察到忙碌文字与真实页面导航。跨版本旧会话观察单独保留，网络计数以全新 R14 会话为准。
本轮没有新增或运行测试套件，没有修改标记、竞价、代理或通知偏好；预览页面观察不覆盖开拍后的竞价流程。

R14 封存包在 `target/dist/public-actions-state-20261020-schema55-r14/`，本地部署及页面观察证据在
`target/dist/public-actions-state-20261020-ops/`，云端证据在 `backups/public-actions-state-20261020T085000Z/`。
76 项云端证据已下载并逐项核对，最终 `final-public-r14.json` SHA-256 为
`8d4456f4dfedacfe585510dcf98ca29dfc1cb14fff7b4dc15a732ae5212b694f`。

### 2026-10-20 当期标记装备与操作列（Public 已上线）

`auction-web-public` 将开拍目录的操作列从 15% 加宽到 22%，显示操作表头，标记、出价及代理按钮保留完整文字并在本列换行。
标记按钮使用空心/实心星形和原有本地化文字，继续消费权威状态、身份隔离、版本与原命令重试。
预览目录同时移除操作列的列宽、表头和行单元格，保持十列，编号仍进入详情。
账户 `identity/account/watches/` 组合既有当前场次、完整账号标记及公开目录，显示各启用世界当期装备的编号、真实名称、
世界场次、状态与详情入口，包含预览场次；目录完整有界读取、末尾复核及失败关闭后，按真实交集本地分页。
五语言、模块说明、架构 owner 及第 28 张表格登记同步；本轮没有修改 HTTP、DTO、数据库或配置契约。

R12 和收紧账户数量提示留白后的 R13，完整 `just verify`、正式 embedded 发布构建及十六项封存检查均退出 0；
GNU/glibc、无 CPU 特化、双端资源隔离、旧补偿/历史搜索标记与新账户/操作列标记核对通过。
R13 构建起止源码指纹为 `01ab84b97840f85697b2606652314ab87aba4fef9a11b6837baf4d0e703120bc`，
`SHA256SUMS` 摘要为 `9e7f19351940e376a6986773c1a25a477916ef6816f3cbc41263ef17ad5eca15`；
Public payload 为 `1f22428473a2b920e84898bd07deae4e66d7d033e7f075a3f60189e5310ef224`，
Public ELF 为 `9615d77b48d964b3fbf910c1b884fc93e3720f6f75e8c9ea16c020db0c6784bf`，Admin 资源与 R11 相同。

生产预检只读核对 schema 55 全部已发布 SQLx checksum。08:06:57 UTC 首次安装 R12，
08:14:38 UTC 仅停止 Public，08:14:39 UTC R13 readiness 为 200 并完成既有总控 all-running `start` 的 `adopted` 回执。
08:14:42 UTC 后验确认三个 Unix readiness 均为 200、三份 receipt 均 matched、自动重启均为零；Public PID 103231 运行映像匹配 R13。
Worker PID 97793、Admin PID 97809 的映像、InvocationID、单调启动时间及磁盘 `auctionctl` 全程保持 R9，数据库未迁移。

经用户授权使用真实账号只读查看公开站点的预览目录、账户和装备详情：预览表的 colgroup、表头、每行均为十列；
账户显示已有的一件 One01 标记及真实装备名、详情入口；数量提示实际为左对齐、无额外块级内边距。
等待详情的权威标记读取完成后，选中按钮显示 `★Marked` 且 `aria-pressed=true`。
本轮没有新增或运行测试套件，没有修改用户标记、竞价、代理或通知偏好；当前预览场次的页面观察不覆盖开拍后的竞价流程。

R12 证据保留在 `target/dist/watch-actions-20261020-ops/` 与云端 `backups/watch-actions-20261020T080000Z/`；
最终 R13 封存包在 `target/dist/watch-actions-spacing-20261020-schema55-r13/`，本地证据在
`target/dist/watch-actions-spacing-20261020-ops/`，云端证据在 `backups/watch-actions-spacing-20261020T081000Z/`。
两个版本各 76 项部署证据已下载并逐项核对；R13 `final-public-r13.json` SHA-256 为
`99c664683428ccabe14a86d9d9f3b11f6bee66f61d14c1418de08a71ffcec920`。

### 2026-10-20 历史成交价缩写搜索与独立容器（Public 已上线）

按用户提供的保存网页及固定提交参考源码核对，`auction-web-public` 将搜索区置于页面顶部的独立全宽容器，
与结果区域为兄弟容器，表格列宽与字号适配只作用于结果；移除可见页面标题和介绍，保留隐藏的可访问标题与导航名称。
默认及清空后按装备名分组，每组独立边框、展开按钮及完整九列表格；组内笔数仅代表当前页，完整匹配集排序、统计与整体分页保留。
Public URL 与表单统一按 Unicode 空白或英文逗号拆词，五语言提示加入 `Leg Sav Pow Hel Bal` 示例。
`auction-http` 同步名称分隔符，应用端八词、每词 128 字节、总长 1024 字节及控制字符边界保留；
`auction-storage` 的字面转义和全部词条忽略大小写包含匹配保持原义，未改变金额、排序、游标、DTO 或 schema。
外部名称查询契约见[路由](docs/contracts/routes.md)，容器及展示 owner 见[Public 模块](crates/auction-web-public/README.md)。

最终源码完整 `just verify`、定向原生/WASM 编译、i18n、格式和 diff 检查，以及 `just build-release` 均退出 0。
R11 十六项清单、GNU/glibc、无 CPU 特化、双端隔离、独立搜索及结果框标记、隐藏标题源码核对通过；
构建起止指纹均为 `50c78f910aa32dc3d3f6fd0687d761b64c4bbe2fe56533000bec43638c88ba39`，
`SHA256SUMS` 摘要为 `3e1f19007107154519f5d9720d0739178071030627a9148a8e889693f2f091f2`。
Public payload 为 `76fc5b9bd4c87dfc171d18df011a5b7170d9d12e2980982b21a43a9e876ddd19`，
Public ELF 为 `a0df350a66f7007a8b7de74ca96f1b1fd3079358ba6b6c4b72b2b8661e319a31`；Admin 资源与 R10 相同。

生产预检只读核对全部 55 项已发布 SQLx checksum；2026-10-20 07:07:23 UTC 仅停止 Public，
07:07:24 UTC 新进程 readiness 为 200，07:07:25 UTC 以既有总控 all-running `start` 完成 `adopted` 回执。
实际仅 Public 更新到 R11，Worker、Admin 与磁盘 `auctionctl` 保持 R9，数据库保持 schema 55，未执行迁移。
07:07:52 UTC 只读后验确认三个 Unix readiness 均为 200、三份 receipt 均 matched、自动重启均为零；
Public PID 101148 的运行映像匹配新 ELF，Worker PID 97793、Admin PID 97809 的运行映像、InvocationID 和单调启动时间全程不变。

云端证据在 `backups/history-search-prefix-20261020T065000Z/`，本地证据在
`target/dist/history-search-prefix-20261020-ops/`，封存包在 `target/dist/history-search-prefix-20261020-schema55-r11/`。
最终 `final-public-r11.json` 的 SHA-256 为 `2db7b992134218e6fcd6934c879ddedf82f04430e39865d39c8d3d284a776bba`。
本轮范围为保存网页静态分析、源码及制品检查、Public 单角色切换和进程/readiness 观察；未新增或运行行为测试。

### 2026-10-20 历史成交价参考改进（Public 已上线）

`auction-web-public` 参考固定提交的 AmyBotV2-web 交互，独立实现装备名、买家或卖家的本页分组、
展开收起及玩家快捷筛选。页面明确分组仅包含当前页，默认逐条展示；默认居中搜索且提交有效条件后才加载列表。
Public URL 分别持久化 `history_sort`、`history_group`，排序改变回第一页，切换分组不重取相同成交页。
金额输入支持整数 C、最多三位小数 K 和六位小数 M，以 checked 整数运算转换，允许 0；非法条件保留字段错误并阻止请求。
原名称与日期端点校验、补偿池跨列末行保留，五种语言同步。

`auction-application` 拥有排序闭集及统计校验，`auction-storage` 在同一读取快照内对完整过滤集排序、分页和统计，
`auction-http` 为既有 P12 增加可选 `sort`，非默认排序与旧 cursor 组合返回 400。
`auction-contracts` 增加可选 `stats.median_exact`，中位数以精确 numeric 计算并保留半 C，旧整数 `median`
保持半数取偶兼容口径；Public 优先显示精确字段。外部契约见[路由](docs/contracts/routes.md)，展示范围见
[Public 设计](crates/auction-web-public/DESIGN.md#5-表格与信息密度)，参考来源与 owner 见
[Public 模块](crates/auction-web-public/README.md)。

完整 `just verify`、定向原生/WASM 编译、i18n 与 diff 检查，以及 `just build-release` 均退出 0。
编号页 SQL 的静态语法及十一项参数绑定核对通过；十六项制品清单、GNU/glibc、无 CPU 特化、双端资源隔离及新旧页面标记核对通过。
构建起止源码指纹均为 `1761ac0b07b892dcde22a847e987599a4bb1d5e490d204a5857c278ba3968b57`，
R10 `SHA256SUMS` 摘要为 `0c1bfe0070e669cf2b6180b37d26b4b38f55c3ae12cc815671f31409a4a16a67`。
Public payload 为 `e741846b892f01af887ff48f931d3281590ebfdd48d9a8150538f12368e5eb2a`，
实际部署 Public ELF 为 `deccd6a18ec120532d537b914f0ffa1f464173ea3bf2b8f787e4e1fcc31c3f14`。

生产预检只读核对 `auction_test` 已发布 1–55 全部 SQLx checksum 与源码一致，未迁移数据库。
首次叶子门因 `systemctl show` 省略空的 ExecStop 数组而在停止服务前退出，失败证据保留；
改用 systemd D-Bus 显式核对两个停止钩子的类型化空数组，并在停止前再次复核五项反向依赖及两个钩子。
2026-10-20 06:13:32 UTC 仅停止 Public，06:13:33 UTC 新进程 readiness 为 200，06:13:34 UTC 发布完成。
Worker、Admin 和磁盘 `auctionctl` 保持 R9，未安装封存包中的其他三个新程序；Worker PID 97793、Admin PID 97809
的 InvocationID、单调启动时间及运行映像全程不变。既有总控以 all-running `start` 刷新 `adopted` 回执，未触发迁移或其他角色启停。
06:13:46 UTC 只读后验确认三个 Unix readiness 均为 200、三份 receipt 均 matched、自动重启均为零，
Public PID 100235 的实际运行映像与上述制品一致。

云端证据在 `backups/history-reference-20261020T055000Z/`，本地证据在
`target/dist/history-reference-20261020-ops/`，封存包在 `target/dist/history-reference-20261020-schema55-r10/`。
最终后验 `final-public-r10.json` 的 SHA-256 为
`ddab24208e22171c9b2bbe67ccec2c60f7e7f0f340932f782c51469d0cadac10`。
本轮范围为静态检查、制品核对、Public 单角色切换及进程/readiness 观察；未新增或运行行为测试。

### 2026-10-20 履约激活、待处理计数与历史成交价修复（已上线）

只读生产日志确认 Katana 卖家款的激活事务违反既有
`settlement_obligation_fulfillments_check2`：仍持有 claim 时提前写入 `next_attempt_at`。
`auction-storage` 激活继续保留 token/fencing 且将该字段保持为空；`auction-application` 在紧接的
reconcile 释放 claim 时原子写入共享追踪窗口，COD、carrier、卖家货款维持五分钟窗口，普通装备维持原重试间隔。
存储的履约水位只将 `ready` 的 `pending/prepared/enqueued` 计为待处理，contingent 退款保留原事实，
不进入待处理及积压年龄；独立的阻塞、失败、人工核对计数保持原义。响应字段及 schema 55 均未变更。

`auction-web-public` 补齐名称的表单与 URL 同规则校验；非法名称显示五语字段错误并阻止请求，纯分隔符视为空条件。
年月未变时保留 URL 原有日端点，清空按钮合并为一个；装备名旁显示既有分类标的号。
默认不加载装备列表、居中搜索、完整查询与分页大小绑定响应，以及补偿池跨全列表尾均保留；没有新增 API、DTO 或排序参数。

完整 `just verify` 与 `just build-release` 退出 0，定向 Public 原生和 WASM check、i18n、格式与 diff 检查通过。
R9 封存制品的十六项清单、GNU/glibc、无 CPU 特化、双端资源隔离和迁移内嵌核对通过，
`000053`–`000055` checksum 与已发布 R8 一致。源码指纹始末为
`ba08d01bf3847168a856effa0b356bdfb219c163e8b7e819c1adc5719eb68c03`；
新 Public payload 为 `a23118ca4ca22ee4582ba7895c78149d1a1e34793160e86d0fdd5fa80037c1b2`，
Admin 资源保持 `f859d5a6e135b9c9aecb2da719fc6a54a87467e15ec6d61aa923eda683230a07`。

2026-10-20 04:27:24 UTC 停止三角色，验证最终停机备份后替换四程序，04:27:33 UTC 经部署总控恢复。
三份启动 receipt、实际运行 ELF 与新制品匹配，三个 Unix readiness 均为 200，自动重启为零。
04:27:57 UTC 只读后验为总义务 74、旧计数 22、新待处理 1、未触发退款 21，履约阻塞/失败/人工核对均为零；
十笔自寄修复与七笔旧收款认领逐笔通过，来源及分录守恒，现金健康 N/A 且没有新增余额观察。
旧租约于 04:30:06 UTC 自然到期后，新进程于 04:30:44 UTC 成功激活原卖家 delivery，清除 claim 并写入下一共享追踪时间。
04:32:27 UTC 首次发送成功，精确 Sent MID 为 `3120097`，`sent_exact_payload_v1`、Sent 验证及外部引用证明均为真；
Main、Nagoriha、49,000 C 完整载荷、原义务以及 prepare/activate 身份逐项匹配，发送 lease 已释放。
剩余一项履约为等待卖家领取的正常 enqueued，下一次领取核验为 04:35 UTC；未将已发款误记为对方已领取。
04:33:49 UTC 发款后再验十七笔原修复事实、资金来源及分录守恒均通过，未知/发送中用途为零，
49,000 C 已记入 spent，现金健康仍为 N/A 且没有新增余额观察。
日常 MM 于 04:30:20 UTC 自动成功，连续失败及待恢复读取均为零，下一轮为 04:35 UTC。
04:34:49 UTC 最终复核三份启动 receipt 和运行映像仍匹配，三个 readiness 均为 200，三角色自动重启仍为零。

最终备份 `before-upgrade-r9.dump` 的 SHA-256 为
`57e3a685fa14987247c4747c2568c1736d65136eb829b2cc796c6296b04395b6`，已下载本地并匹配 manifest。
云端证据保留在 `backups/bookkeeping-repair-20261020T023903Z/`，本地在
`target/dist/fulfillment-repair-20261020-ops/`，封存包在
`target/dist/fulfillment-repair-20261020-schema55-r9/`。验证范围为静态检查、停机备份、制品/运行映像、
只读后验与正常自动调度观察；未新增或运行行为测试。

### 2026-10-20 旧 Bot 结算修复与仅记账启用（schema 55 已上线）

按用户授权在既有 PostgreSQL 18.6 `auction_test` 完成 `000053`–`000055`，迁移 checksum 与源码一致，
升级前保存并验证停机备份。最终 R8 制品来自 `72e6ad1` 后未提交工作树，源码指纹为
`1529a2cae0859bab93f75e54caa01a98c78e72847306f4c18192aa332f853992`；
完整 `just verify` 与 `just build-release` 退出 0，四程序、双端内嵌资源及十六项包清单核对通过。
2026-10-20 04:04:10 UTC 由部署总控按 Worker → Admin → Public 恢复；三份启动 receipt、运行 ELF
及 Unix readiness 200 均匹配，04:06 UTC 自动重启计数为零。

显式修复逐笔提交九件旧流拍自持、一件成交自寄重新进入内部付款，以及七件旧 Bot 卖家已收款认领。
七笔合计留存 6,185,400 C、税 390,064 C，仅转移原隔离责任，没有重复收款来源或收入。
账号所有人选择持久 `record_only`；初始化登记原责任 9,433,706 C，不读取账号余额、不预发 P/C 额度。
Katana 的 Bot 买家内部付款于 04:04:15 UTC 完成，有限来源为 B 48,000 C 与补偿 1,000 C，未产生买家外部寄信。
04:05:58 UTC 的 UTC 只读后验确认十笔自寄修复、七笔认领、资金来源及分录守恒全部匹配，原投递历史保留，
现金健康为 `not_applicable`，仅记账启用后没有新增余额观察。

04:06 UTC 仍有一笔 Katana 卖家款 49,000 C 处于 prepared，后续激活修复另行登记；
二十一条未触发买家退款仍为 contingent，旧待处理投影将它们计入，计数修复与该卖家款分开处理。
另外两条旧零资产拒收通知保留人工核对：Sent 中存在多个同收件人、同正文候选，未重发或强行绑定。
日常 MM 于 04:05:18 UTC 自动成功，暂停为 false、连续失败为零、待恢复读取为空，下一轮为 04:10 UTC。

受控证据在云端 `backups/fulfillment-repair-20261020T014800Z/`、
`backups/bookkeeping-repair-20261020T023903Z/`，本地证据在
`target/dist/fulfillment-repair-20261020-ops/`，封存制品在
`target/dist/fulfillment-repair-20261020-schema55-r8/`。验证范围为静态检查、备份/迁移、逐笔授权修复、
运行映像与正常调度观察；未新增或运行行为测试。

### 2026-10-19 Bot 仅记账策略（源码与静态检查通过）

按账号所有人的明确要求，新增 `000055` 与独立 `bot-funds-record-only`：仅对全局尚无资金账户的场景显式初始化
UID 的持久 `record_only` 策略，默认 `strict_cash` 维持原现金保护。初始化仍需真实联合 UID、干净 compose、原责任
预览、版本、摘要、稳定命令及精确确认，不读取或比较账号现金，也不要求停止个人用款或人为设置 P/C 预算。
内部购买和补偿仅按原标的冻结 B/C 创建唯一有限的会计来源，旧卖家款保留精确 Sent-collected 与隔离责任内转，
不重复记入旧收入；跨世界外部付款的 1C 载体另绑定原 Isekai 义务、完整 payload 及 delivery 唯一来源。
未知投递、装备冲突、退款授权及实际发送证据继续按原义务保护。

应用、投递适配器与存储最终事务复核同一持久策略；两种模式不混开户，不提供隐式转换。
自动恢复健康的现金/现金证据状态明确为 `not_applicable`，停止自动余额读取及余额不足告警，仍刷新真实履约积压；
管理页五语言显示仅记账状态并隐藏旧余额及现金证据年龄。联合 MM/论坛证据增加闭集失败阶段和原 typed 错误类别诊断，
显式 CLI 证据执行使用独立 JSON stderr 订阅器，stdout 保留命令结果，不输出原始页面、HTTP/SQL错误正文、父 span 或凭据。

完整 `just verify` 退出 0，覆盖格式/策略、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）
和双端 WASM；补齐模式混用、跨世界载体及 CLI 诊断后再次完整通过。55 份迁移执行视图及 980 条完整存储 SQL 字面量语法解析通过，
`000055` 新增函数通过 PL/pgSQL 解析；动态 SQL 沿用已核对组装，后续数据库语义校验及部署证据单独登记。
修正旧自寄预览的物理列引用为 `leader_user_id`；该修正版六条关键查询已在目标数据库只读 `PREPARE` 成功。
未新增或运行行为测试。

### 2026-10-19 旧 Bot 结算显式修复（源码与静态检查通过）

追加 `000053`/`000054` 及 `auctionctl legacy-settlement self-send/adopt-collected`，默认单目标数据库只读预览。
执行以原版本、摘要、稳定命令、真实联合 UID、干净 compose、精确库存或原 Sent-collected/当前现金为门禁；
旧自寄原投递与指针快照永久追加保存。流拍按真实自持事实满足，明确拒绝的成交付款只恢复内部准备，
后续固定修复 UID 且不外发；已处置旧投递永久拒绝新的通用 Admin 可变动作，原成功命令保持重放。
旧 Bot 卖家款只从开户已隔离责任转为原标的税/留存，不新增 source、receipt 或发行旧 B。

投递健康聚合补齐 `FROM public.deliveries`，failed 水位只按完整可信修复事实排除已结清的旧自寄，
历史投递状态和原失败证据保留。资金预览精确排除对应已修复旧投递及严格解码的零资产纯通知，
不消除通知本身的人工核对状态，不放行未知现金/装备责任。

完整 `just verify` 退出 0，覆盖格式/策略、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）
和双端 WASM；`git diff --check` 通过。54 份迁移执行视图、959 条完整 SQL 字面量静态解析通过，
四处新增动态谓词组装后的完整查询单独解析通过，四处既有片段沿用调用组装复核；新增两份函数通过 PL/pgSQL 解析。
本条记录止于源码与静态检查，后续制品、迁移及真实修复证据独立登记；未新增或运行行为测试。

### 2026-10-19 重审修复全量升级（已上线至 schema 52）

按用户授权更新云端 `/home/ngnl/HV-Auction`，目标为 PostgreSQL 18.6 的既有 `auction_test`，
2026-10-20 00:23:38 UTC 停止三角色，00:23:40 UTC 按 Worker → Admin → Public 完成启动及就绪。
先保存旧四程序与配置，使用匹配 schema 50 的旧 `auctionctl` 生成停机最终备份，
再以新工具校验归档、应用 `000051`/`000052` 并替换四程序。迁移历史 1–52 连续成功，
新增迁移 checksum 与源码一致；两张新表的十项约束均 validated，只追加触发器已启用。
十四张既有业务表在停机、备份后及迁移后的行数和内容摘要一致。

制品来自 `72e6ad1` 后六项修复的未提交工作树，源码指纹为
`2c877ec9298725a7965fd81ebaa4ddef451fc3386d6d3af645cc143aa2ef72a1`。
`just build-release` 完成双端 WASM 与 GNU 四程序构建；十六项包清单、页面内嵌隔离及目标 glibc 兼容核对通过。

| 程序 | 上线 SHA-256 |
| --- | --- |
| Public | `65ef21afcd898eaf2c855c275cfe0364466a2a24a7800cb47e62d75557ce4b49` |
| Admin | `f6271ad0e09212cbcba658f8adf6468d10eb83f03ed1fd9d27bb43723b256420` |
| Worker | `158711f4a6d116fbf5ebb3326682f55cfe0dd7a6758ca107e72c622f2344847e` |
| auctionctl | `e6a18d28378c0b91a02865619f7ef8fc46c26d2aba6ed269afbdebacd1b92df2` |

00:25:54 UTC 复核三角色均 active/running、自动重启为零、运行映像匹配，三个 Unix readiness 均为 200。
日常 MM 于 00:25:27 UTC 自动完成三页维护及收件轮询；连续失败为零、失败码/时间为空、claim 已释放，
待恢复读取队列为空，下一次计划为 00:30 UTC。本次启动后至该观察点的三角色日志无 WARN/ERROR。
部署验证限于备份校验、迁移后验、服务就绪、运行映像及正常调度观察，未手动触发邮件或执行业务流程测试。

最终备份及旧制品保留在云端 `backups/schema52-20261020T001402Z/`；`final.dump` 的 SHA-256 为
`07a12c18c38e1728b40a98f24649709605d1f26313d047bf6309a0b623b76f21`，已下载本地并核对归档与 manifest。
本地受控目录 `target/dist/upgrade-20261019-schema52-ops/` 保存备份、部署与只读观察回执；
对应封存制品及构建清单在 `target/dist/upgrade-20261019-schema52/`。未执行数据库恢复。

### 2026-10-19 提交后重审六项修复（静态检查通过）

以 `72e6ad1` 为基线重新审计后，修复六项问题。Recovery 在 `000051` 增加只追加的退件消费事实，
与计数、动作同事务写入；新回收命令按 action 与来源 MID 派生，激活跟随持久 prepare，旧投递继续原身份。
Mail-poll 在 `000052` 增加详情读取前的持久摘要队列，按 MID 恢复已读但未提交的邮件，
入站事实、命令账本、cursor 与队列完成前缀同事务提交；论坛 owner 失效接入 Worker 既有有界排空及非零退出。

配置与直接存储连接拒绝缩略或缺少明确目标的 PostgreSQL URI；合法旧 URI 省略端口时，在 SQLx 首次解析及
pgpass 查找前固定为 5432。原生与 Web 缓存共用构建环境摘要，profile、rustflags、目标参数或包装器变化使缓存失效。
同步架构、对象、不变量、模块说明和停机升级流程；保留政策绑定 schema 52，并登记两张新表，未增加删除授权。

完整 `just verify` 退出 0，包含格式、策略、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy（`-D warnings`）
及双端 WASM；发布脚本 `bash -n`、ShellCheck 和 `git diff --check` 通过。52 份迁移执行视图及 915 条完整 Storage SQL
通过只读语法解析，四处既有动态模板片段按组装调用复核；新增消费表函数另通过 PL/pgSQL 静态解析。
本轮仅静态验证，未新增或运行行为测试、执行迁移或部署云端；并发、故障恢复及真实邮件/论坛协议仍待运行验证。

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
论坛目录中的游戏装备直链及装备 key 是公开数据，镜像明文不再列为实现缺口；2026-10-26 的隐私风险判断及由此提出的
加密要求已撤销，历史审计记录保留更正说明。当前按[公开链接契约](docs/contracts/schema.md#论坛镜像与公开装备链接)
保存和返回正文，固定楼层身份、正文对账、未知外写处理及认证凭据保护继续执行既有规则。

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

旧版本提前物化的 Draft、缺失的邮件完成证明、未闭合 recovery 和不一致 runtime 仍须独立确认，本轮未清理、重放或改写历史现场；提示与停止预算的后续修复和剩余边界如下。

[中文玩家指南](docs/guide/player-guide.md)此前核对出登录提示的长度偏差；本轮已删除五语占位文案中的“32 位”，按[邮件 Key 协议](crates/auction-application/src/ports/mail/user/security.rs)继续提示使用完整 43 字符 Key，未改变认证协议或取得浏览器运行证据。
当前[论坛主题正文](crates/auction-integrations/src/forum/render.rs)已明确 `notify mm` 只请求单次拍卖通知，不是超价订阅，`notify pm` 不可用；
与[通知 owner](crates/auction-jobs/src/worker/forum/reply/notification.rs)一致。本轮仅更正过期的缺口说明，未改通知语义，也无新增真实邮件证据。

[分体总控 unit](ops/deploy/systemd/auction-stack.service.example)此前 `TimeoutStopSec=120` 小于三角色各 45 秒的合计 135 秒；本轮模板调整为 155 秒，保留 20 秒总控余量。既有安装不会自动更新，仍须按[systemd 手册](ops/README.md#systemd-服务与顺序控制)在维护窗口同步；本轮未执行停止流程或部署。

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
