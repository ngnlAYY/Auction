# auction-integrations

[docs/README.md](../../docs/README.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

外部世界的适配器：HentaiVerse MoogleMail 与 E-Hentai 论坛的 HTTP 客户端与 HTML 解析、装备来源读取、论坛主题渲染、外部凭据与登录 key 的加密，以及装备 key 的密文信封。它实现应用层的端口，不读取配置文件、不访问数据库，所需参数由装配根传入。外部端点固定（`https://hentaiverse.org` 的 main/isekai 路径、`https://forums.e-hentai.org`），测试可改为回环；所有响应有字节上限、请求有最小间隔与超时，解析失败一律视为协议漂移而非成功。

## 模块

| 模块 | 内容 |
| --- | --- |
| [crates/auction-integrations/src/clients/mooglemail](src/clients/mooglemail) | 协议模型与 URL 构造、受限 HTTP 客户端、HTML 解析（收件/Sent/余额/库存/身份）、分步写入与发送后 Sent 证明、投递 transport、周期维护、单 Cookie 会话互斥、联合远端证据、加密诊断归档 |
| [crates/auction-integrations/src/clients/ehentai](src/clients/ehentai) | 论坛客户端：主题分页读取、精确标题查找、发帖与编辑、登录态只读证明、目标资料读取、私信 |
| [crates/auction-integrations/src/credentials](src/credentials) · [cryptobox](src/cryptobox) · [keyring](src/keyring) · [login_key](src/login_key) | 凭据规范化、登录换取 Cookie 与 AES-256-GCM 密封；`DataKey` 与信封；单域密钥环（未知 key ID 失败关闭）；登录 key 签发与解密 |
| [crates/auction-integrations/src/equipment](src/equipment) · [forum](src/forum) | 装备来源读取（host/path allowlist、可交易判定、canonical 事实）与 Public URL 解析；BBCode 渲染与快照预览解析 |
| [crates/auction-integrations/src/heartbeat](src/heartbeat) · [operations_alert](src/operations_alert) | 备份打卡（一次 HTTPS GET，禁 userinfo/fragment/重定向/代理/重试，结果脱敏）；告警 Webhook（只发送冻结 JSON） |
| [crates/auction-integrations/src/worker_control](src/worker_control) | Admin/CLI 经 Worker control 的 client 适配器（设备读取、Sent 查询、恢复扫描、状态/进度/触发）；只拥有 Unix socket 传输与错误分类。协议常量与线上 DTO 由 `auction-application` 拥有 |
| [crates/auction-integrations/src/safe_diagnostics](src/safe_diagnostics) · [backup](src/backup) | 集成错误的同步安全投影；可选 `backup` feature 的 R2 适配器（参数由 CLI 显式传入） |

## 关键行为

- **单 Cookie 会话**：compose、资产操作与联合证据持有同一排他会话守卫；普通列表与详情使用按凭据、world、运行代次隔离的有界共享读取。联合证据（身份、干净 compose、Main 现金、Sent 观察）只能在原排他会话内取得，普通读取不能提升为资产授权。
- **发送证明**：compose → 附件/COD → 发送分步进行；发送后读取基线之外的新邮件，核对完整收件人、标题、正文与资产、唯一候选及响应 MID 提示，并再次证明 compose 已干净才算成功。无法证明时报告“写入未知”，交由上层只读对账，绝不重发。
- **发送前基线**：`sent_baseline_mode` 默认 `shadow`，旧完整排除集唯一裁决并比较水位分支；`watermark` 每次发送前 fresh 读完最新分钟组取最大 MID 与分钟时间，写后核对锚点。非数字、溢出、前导零或顺序异常不改变 opaque MID 契约：写前回退完整扫描，写后锚点缺失则完整枚举并保留原边界。切换条件见[ops/README.md#sent-发送基线影子观察](../../ops/README.md#sent-发送基线影子观察)。
- **Sent 维护与收件人投影**：常规轮从最新页扫描到水位锚点并读完同分钟边界，前缀只缩小观察窗口，旧候选缺席不证明未发送。原生列表收件人可能按字节截断，列表筛选仅把非空前缀当候选；解析、transport 与联合证据始终保留详情完整收件人，最终做完整身份比较。
- **Take / Return 证明**：Take 要求同一邮件身份、资产归零及领取回执；Return 要求与原邮件身份、正文和完整资产绑定的 `[RTS]` 回执。详情 GET 会让邮件离开 Inbox，Inbox 缺席或普通成功提示都不是完成证明。
- **装备出库与属性**：按冻结装备名读取 Armory 库存，提示缺失或未覆盖时回退完整七类扫描；名称可来自内联声明或同源 `dynjs` 脚本（只解码 JSON 字面量，不执行 JavaScript）。属性键保留 `分组 / 名称`，相同键、损坏列或单位仍拒绝，不丢事实。
- **诊断归档**：拒绝或异常的响应可加密保存为本地证据（父密钥优先 `mail_diagnostics_data_key`，否则 `credential_data_key`，再否则 `equipment_data_key`），按保留天数与字节上限清理，`auction-cli mail export-evidence` 离线导出；头部记录写入域与 key ID。
- **论坛写入**：固定 origin；POST 后只有能证明远端结果才算成功，否则返回 `write_unknown` 并由上层只读对账。登录态通过用户控制台只读证明；读取在请求前与正文后复核同一 owner 与 Cookie，读后失锁不交付响应。主题读取最多 20 页、每页 8 MiB、累计 32 MiB。
- **R2 副本**：`R2BackupStore` 使用显式目的地/凭据与 `auto` region，不用 from_env 或实例元数据；128 MiB 及以下条件写入，更大对象分片（part ≥64 MiB、并发 ≤2），HEAD 后完整回读校验 SHA-256；不提供覆盖、删除或换名入口，失败可中止未完成分片。

## 维护约定

- 解析器只返回不含原始 HTML 的类型化结果；Cookie、页面 token 与原始正文不进入日志或错误文本。
- 远端页面结构变化应表现为明确的解析失败（并留下诊断证据），不能退化为“看似成功”。
- 修改加密 AAD、指纹域或信封格式会使既有密文无法解开，必须以新版本前缀并保留旧版解密路径。
- 多 key 解密只在该域密钥环内进行：未知 key ID 不回退当前 key，也不尝试其它域；新写入永远使用当前 key。
- 论坛复用原持久 claim，`submit_form` 在第一个实际 POST 前登记；已登记的工作项继续完成必要 POST/回读，结果提交由应用/Worker owner 确认。外写未知仍只读对账，绝不自动重发。

## 外部客户端指标边界

本 crate 经应用层窄端口 `ExternalMetricsObserver` 记录同步事件（无 I/O、无 `.await`，关闭指标或非 Worker 角色为 no-op），不依赖 Prometheus，也不改变请求间隔、锁顺序、deadline、重试、取消或写入围栏。物理请求在通过校验时计数，共享读取在任务内部计数（等待者不重复），锁等待只测获取；账号忙熔断按账号与世界退避 1→2→5→10 分钟，只限制请求节奏、状态可丢失。客户端层、Worker 层、HTTP 层与数据库层计数口径不同，不可相加。完整名称、合法组合与桶见[crates/auction-observability/README.md#外部账号客户端指标](../auction-observability/README.md#外部账号客户端指标)。

## 安全错误记录

集成错误在折叠边界复用应用层 `diagnostics::record_error`：私有 `project` 只匹配类型、闭集变体与非敏感标志，日志只接收安全投影，不接收原错误；`reqwest::Error` 只看超时/构造/正文类别，不读 URL 与错误链。字段为 `kind`、`phase`、`safe_kind` 与 world/业务 UUID/`write_started`/`evidence_id`；绝不记录 URL、响应片段、请求头、Cookie、玩家名、主题/正文或凭据，也不调用原错误的 Display/Debug/source。**113 类闭集以 `SafeErrorKind` 的关联常量与 `from_label` 映射为唯一事实源**，未知标签才归 `other`。内部不变量与加密失败用 ERROR，传输/远端拒绝/解析漂移/未知结果用 WARN，缓存回退与不能比较的影子样本用 DEBUG；高频日志复用 60 秒诊断门。owner 清单与过滤见[ops/README.md#安全错误与未持久化指标](../../ops/README.md#安全错误与未持久化指标)。
