# auction-integrations

[docs/README.md](../../docs/README.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

外部世界的适配器：HentaiVerse MoogleMail 与 E-Hentai 论坛的 HTTP 客户端与 HTML 解析、装备来源读取、论坛主题渲染、外部凭据与登录 key 的加密，以及装备 key 的密文信封。它实现应用层的端口，不读取配置文件、不访问数据库，所需参数由装配根传入。外部端点固定（`https://hentaiverse.org` 的 main/isekai 路径、`https://forums.e-hentai.org`），测试可改为回环；所有响应有字节上限、请求有最小间隔与超时，解析失败一律视为协议漂移而非成功。

## 模块

| 模块 | 内容 |
| --- | --- |
| [crates/auction-integrations/src/clients/mooglemail](src/clients/mooglemail) | 协议模型与 URL 构造、受限 HTTP 客户端、HTML 解析（收件/Sent/余额/库存/身份）、分步写入与发送后 Sent 证明、投递 transport、周期维护、单 Cookie 会话互斥、联合远端证据、加密诊断归档 |
| [crates/auction-integrations/src/clients/ehentai](src/clients/ehentai) | 论坛客户端：主题分页读取、精确标题查找、发帖与编辑、登录态只读证明、目标资料读取、私信 |
| [crates/auction-integrations/src/credentials](src/credentials) · [cryptobox](src/cryptobox) · [keyring](src/keyring) · [login_key](src/login_key) | 凭据规范化与登录换取 Cookie；在线检查分两侧：Admin 侧 `CredentialVerifier` 只在本进程检查论坛 Cookie，邮件 Cookie 只按 ID 与版本转交 Worker control；Worker 侧 `WorkerMailCredentialVerifier` 打开指定信封后经邮件循环同一 gateway 以候选身份读余额页（共享请求门与在途锁，不清除账号忙碌；发出前的排队在检查自己的 future 内，取消或超时即撤票退出；暂停在排队期间、两个世界之间与取得在途锁后的发出前都生效；只有已发出的请求交给独立任务走完，持有在途锁到远端回应或客户端超时并占住检查名额，持锁上限与远端口径见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)）；`DataKey` 与装备 key、外部 Cookie、登录 key 共用的 AES-256-GCM 信封原语（随机 nonce、按用途分域的明文指纹、恒定时间比较与清零），各适配器只拼 AAD 与映射错误；单域密钥环（未知 key ID 失败关闭）；登录 key 签发与解密 |
| [crates/auction-integrations/src/equipment](src/equipment) · [forum](src/forum) | 装备来源读取（host/path allowlist、可交易判定、canonical 事实）与 Public URL 解析；BBCode 渲染与快照预览解析 |
| [crates/auction-integrations/src/outbound](src/outbound) · [heartbeat](src/heartbeat) · [operations_alert](src/operations_alert) | 受限出站 HTTPS 的唯一 owner（只收绝对 HTTPS 地址，禁 userinfo/fragment；客户端禁重定向/代理/重试，3 秒连接、5 秒总超时）；备份打卡（一次 HTTPS GET，结果脱敏；Worker 存活检查长期持有同一客户端）；告警 Webhook（只发送冻结 JSON） |
| [crates/auction-integrations/src/worker_control](src/worker_control) | Admin/CLI 经 Worker control 的 client 适配器（装备读取、Sent 查询、恢复扫描、邮件 Cookie 在线检查、状态/进度/外部账号快照/触发）；只拥有 Unix socket 传输与错误分类。协议常量与线上 DTO 由 `auction-application` 拥有 |
| [crates/auction-integrations/src/safe_diagnostics](src/safe_diagnostics) · [html](src/html) · [backup](src/backup) | 集成错误的同步安全投影与默认上下文的 `map_err` 映射（`warn_map`/`error_map`/`debug_map`）；常量 CSS selector 在调用点只解析一次（`static_selector!`）；可选 `backup` feature 的 R2 适配器（参数由 CLI 显式传入） |

## 关键行为

- **单 Cookie 会话**：compose、资产操作与联合证据持有同一排他会话守卫；普通列表与详情使用按凭据、world、运行代次隔离的有界共享读取。联合证据（身份、干净 compose、Main 现金、Sent 观察）只能在原排他会话内取得，普通读取不能提升为资产授权。
- **运行时所有权探测**：邮件、论坛与装备读取在外部请求前后复核 Cookie owner。探测在独立任务中完成并受 1 秒上限约束：调用方的读取轮次、证据截止或 Worker item timeout 到期只放弃等待，不撤销 owner；探测自身超时或失败才永久撤销，Worker 随后排空重启。
- **请求节流**：每个账号一个分级请求门，只拉开请求起点的间隔；MoogleMail 另以单持有者在途锁让物理请求串行到读完正文为止。装备页读取用独立 HTTP 客户端携带同一 Cookie，不经请求门、只按自身的最小间隔节流，但借用同一 MoogleMail 客户端的在途锁（按后台档排队，排队时间计入该次读取的请求超时）与账号忙熔断：不与邮件请求重叠，熔断窗口内不发网络、也不占用窗口的探测额度；战斗页打开同一账号忙窗口，战斗页、429 与状态锁限流正文都按暂时不可用延期，绝不判为来源无效而退件。取得令牌的请求按同一档位在锁上排队，前台写入与写入流程页面插到已排队的后台读取之前。取得锁后、请求发出之前（写入还在写入围栏之前）复核账号忙熔断：排队期间打开的窗口照样拒绝写入，只读请求只放行窗口内第一个发出者作为探测；这类拒绝不算写入未知。只读请求遇到 HV 状态锁限流正文按临时限流处理；写入后的同类正文仍交给页面证明裁决，无法证明即写入未知，绝不重发。
- **读取被放弃**：邮件 GET 与装备页读取都分两段。发出前的排队（邮件 GET 是请求门、在途锁、账号忙与 owner 复核，装备页是自身节奏、在途锁、账号忙与 owner 复核）留在调用方 future 内，调用方放弃（外层读取预算、读取轮次超时、Worker item timeout、control 路由预算、任务取消）即撤销票据与锁登记，不在后台补发。越过发出边界后，调用方一直等待时请求就在调用方任务内走完，诊断上下文与 panic 传播不变；中途放弃只丢弃结果，剩余部分交给独立任务走完，仍按实际结果记请求指标与账号忙状态（战斗页照样打开熔断窗口），在途锁持有到远端回应或该次请求的 HTTP 预算结束：邮件 GET 是含正文读取的整次请求超时 `request_timeout`（单次持锁的名义上限与候选凭据检查同口径，见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)），装备页是该次读取的 `request_timeout`（从排队开始计时）。详情 GET 的收件成员关系写锁同样随剩余部分持有。因此在 HTTP 预算内，紧接的同账号请求不与远端仍在处理的请求重叠；客户端超时不能证明远端已停止处理，超过预算后只保证 Worker 侧串行。独立任务只在放弃时产生，每个已发出的请求至多一个，同账号同一时刻至多一个持有在途锁；邮件 GET 的剩余部分在 HTTP 预算之后还有回应 owner 复核（≤1 秒）与失败留证（≤2 秒），这两步不占在途锁，装备页的剩余部分到读取预算结束即丢弃。写入 POST 不在此列：发出后被放弃的写入随调用方释放在途锁，结果按写入未知交给页面证明或人工对账。候选凭据在线检查另行把已发出的探测交给带检查名额的独立任务，见上表 `credentials` 一行。
- **发送证明**：compose → 附件/COD → 发送分步进行；发送后读取基线之外的新邮件，核对完整收件人、标题、正文与资产、唯一候选及响应 MID 提示，并再次证明 compose 已干净才算成功。无法证明时报告“写入未知”，交由上层只读对账，绝不重发。
- **发送前基线**：`sent_baseline_mode` 默认 `shadow`，旧完整排除集唯一裁决并比较水位分支；`watermark` 每次发送前 fresh 读完最新分钟组取最大 MID 与分钟时间，写后核对锚点。非数字、溢出、前导零或顺序异常不改变 opaque MID 契约：写前回退完整扫描，写后锚点缺失则完整枚举并保留原边界。切换条件见[ops/README.md#sent-发送基线影子观察](../../ops/README.md#sent-发送基线影子观察)。
- **Sent 维护与收件人投影**：常规轮从最新页扫描到水位锚点并读完同分钟边界，前缀只缩小观察窗口，旧候选缺席不证明未发送。原生列表收件人可能按字节截断，列表筛选仅把非空前缀当候选；解析、transport 与联合证据始终保留详情完整收件人，最终做完整身份比较。
- **Take / Return 证明**：Take 要求同一邮件身份、资产归零及领取回执；Return 要求与原邮件身份、正文和完整资产绑定的 `[RTS]` 回执。详情 GET 会让邮件离开 Inbox，Inbox 缺席或普通成功提示都不是完成证明。
- **装备出库与属性**：按冻结装备名读取 Armory 库存，提示缺失或未覆盖时回退完整七类扫描；名称可来自内联声明或同源 `dynjs` 脚本（只解码 JSON 字面量，不执行 JavaScript）。属性键保留 `分组 / 名称`，相同键、损坏列或单位仍拒绝，不丢事实。
- **诊断归档**：拒绝或异常的响应可加密保存为本地证据（父密钥优先 `mail_diagnostics_data_key`，否则 `credential_data_key`，再否则 `equipment_data_key`），按保留天数与字节上限清理，`auction-cli mail export-evidence` 离线导出；头部记录写入域与 key ID。
- **论坛写入**：固定 origin；POST 后只有能证明远端结果才算成功，否则返回 `write_unknown` 并由上层只读对账。登录态通过用户控制台只读证明；读取在请求前与正文后复核同一 owner 与 Cookie，读后失锁不交付响应。主题读取最多 20 页、每页 8 MiB、累计 32 MiB。
- **告警 Webhook**：受限 HTTPS 客户端在构造时建立一次（不跟随重定向、不重试、不走代理，也不保留空闲连接，每次发送都新建连接），构造失败按配置错误在启动期报告。连接建立前失败（DNS、TCP、TLS 握手）时请求尚未写出，按可重试处理并受尝试上限约束；请求写出后的传输失败与超时无法证明远端未收到，转人工核对，绝不自动重发。
- **R2 副本**：`R2BackupStore` 使用显式目的地/凭据与 `auto` region，不用 from_env 或实例元数据；128 MiB 及以下条件写入，更大对象分片（part ≥64 MiB、并发 ≤2），HEAD 后完整回读校验 SHA-256；不提供覆盖、删除或换名入口，失败可中止未完成分片。

## 维护约定

- 解析器只返回不含原始 HTML 的类型化结果；Cookie、页面 token 与原始正文不进入日志或错误文本。
- 远端页面结构变化应表现为明确的解析失败（并留下诊断证据），不能退化为“看似成功”。
- 修改加密 AAD、指纹域或信封格式会使既有密文无法解开，必须以新版本前缀并保留旧版解密路径。
- 多 key 解密只在该域密钥环内进行：未知 key ID 不回退当前 key，也不尝试其它域；新写入永远使用当前 key。
- 论坛复用原持久 claim，`submit_form` 在第一个实际 POST 前登记；已登记的工作项继续完成必要 POST/回读，结果提交由应用/Worker owner 确认。外写未知仍只读对账，绝不自动重发。

## 外部客户端指标边界

本 crate 经应用层窄端口 `ExternalMetricsObserver` 记录同步事件（无 I/O、无 `.await`；非 Worker 角色为 no-op，Worker 关闭指标时只写进程内账号快照、不注册 Prometheus 指标族），不依赖 Prometheus，也不改变请求间隔、锁顺序、deadline、重试、取消或写入围栏。物理请求在通过校验时计数，共享读取在任务内部计数（等待者不重复），锁等待只测获取；账号忙熔断按账号与世界退避 1→2→5→10 分钟，只限制请求节奏、状态可丢失。客户端层、Worker 层、HTTP 层与数据库层计数口径不同，不可相加。完整名称、合法组合与桶见[crates/auction-observability/README.md#外部账号客户端指标](../auction-observability/README.md#外部账号客户端指标)。

## 安全错误记录

集成错误在折叠边界复用应用层 `diagnostics::record_error`：私有 `project` 只匹配类型、闭集变体与非敏感标志，日志只接收安全投影，不接收原错误；`reqwest::Error` 只看超时/构造/正文类别，不读 URL 与错误链。字段为 `kind`、`phase`、`safe_kind` 与 world/业务 UUID/`write_started`/`evidence_id`；绝不记录 URL、响应片段、请求头、Cookie、玩家名、主题/正文或凭据，也不调用原错误的 Display/Debug/source。**闭集以 `SafeErrorKind` 的关联常量为唯一事实源**，各错误类型直接投影为常量、不经字符串往返，不存在 `other` 回退。内部不变量与加密失败用 ERROR，传输/远端拒绝/解析漂移/未知结果用 WARN，缓存回退与不能比较的影子样本用 DEBUG；高频日志复用 60 秒诊断门。命名规则与过滤见[ops/README.md#安全错误与未持久化指标](../../ops/README.md#安全错误与未持久化指标)。

`kind`/`phase` 多数是 `safe_diagnostics` 记录函数与 `*_map` 帮助器的字面量参数；以下位置的标签不是这些调用的直接字面量参数，枚举完整集合时须一并检索：常量 selector 的包装宏（`static_selector!` 及 `pm_selector!`、`login_selector!`、`read_selector!`、`compose_selector!`）、加解密原语的 `SecretPurpose`、在闭集字面量间选择 kind/phase 的映射（各 `diagnostic_phase` 与 `match` 表），以及把 phase 作为参数转发、字面量写在其调用点的帮助函数（`MoogleMailSender::scan_page_limit`、`OwnerProbeError::phase`）。
