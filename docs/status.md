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
| 迁移 | PostgreSQL 57 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
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
