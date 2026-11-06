# auction-integrations

[docs/README.md](../../docs/README.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

外部世界的适配器：HentaiVerse MoogleMail（游戏邮件）与 E-Hentai 论坛的 HTTP 客户端和 HTML 解析、装备来源读取、论坛主题渲染、外部凭据与登录 key 的加密，以及装备 key 的密文信封。它实现应用层的端口，不读取配置文件、不访问数据库，所需参数由装配根传入。

外部端点固定：MoogleMail 为 `https://hentaiverse.org`（main 与 isekai 两个世界路径），论坛为 `https://forums.e-hentai.org`；测试时可通过配置改为回环地址。所有响应有字节上限、请求有最小间隔与超时，解析失败一律视为协议漂移而非成功。

## 模块

| 模块 | 内容 |
| --- | --- |
| [crates/auction-integrations/src/clients/mooglemail](src/clients/mooglemail) | 协议模型与 URL 构造、受限 HTTP 客户端（限速、大小与超时边界）、HTML 解析器（收件列表/详情、Sent、余额、库存、身份）、分步写入（compose、附件、COD、发送）与发送后 Sent 证明、投递 transport、周期维护、单 Cookie 会话互斥、联合远端证据、加密诊断归档 |
| [crates/auction-integrations/src/clients/ehentai](src/clients/ehentai) | 论坛客户端：主题分页读取、精确标题查找、发帖与编辑、登录态只读证明、按 UID 读取目标资料、论坛私信（装备超价提醒） |
| [crates/auction-integrations/src/credentials](src/credentials) | 外部凭据：Cookie 规范化（邮件只保存 `ipb_member_id`/`ipb_pass_hash`，论坛保留原始 Cookie 头）、单次论坛账号登录获取 Cookie、凭据有效性检查、AES-256-GCM 密封（两类凭据使用不同 AAD 与指纹域） |
| [crates/auction-integrations/src/cryptobox](src/cryptobox) | `DataKey`（32 字节 Base64 材料 + 非秘密标识）与装备 key 密文信封的封装/解封 |
| [crates/auction-integrations/src/keyring/mod.rs](src/keyring/mod.rs) | 单域数据密钥环：当前写入密钥 + 保留解密的旧密钥；按信封 key ID 在本域解析，未知 ID 失败关闭 |
| [crates/auction-integrations/src/login_key](src/login_key) | 玩家登录 key 的签发、Admin 手工封装与解密：Users 保存 SHA-256 摘要和 AES-256-GCM 信封；投递发送时解密，Admin 创建/签发命令及其幂等重放也可通过专用 opener 解密返回生成的 key |
| [crates/auction-integrations/src/equipment](src/equipment) | Admin 录入与邮件投稿的装备来源读取（host/path allowlist、可交易判定、canonical 装备事实）、Public 真实装备 URL 解析 |
| [crates/auction-integrations/src/forum](src/forum) | 论坛主题 BBCode 渲染（含装备名称标色）与 Admin 提交的论坛快照预览解析 |
| [crates/auction-integrations/src/backup](src/backup) | 可选 `backup` feature：独立打卡与 R2 上传/完整回读适配器，参数由 CLI 显式传入，不读取配置或数据库 |

## 关键行为

- **备份打卡**：`send_backup_heartbeat` 省略/空地址时不创建客户端；配置地址只发一次 HTTPS GET，禁止 userinfo、fragment、重定向、环境代理与自动重试，连接三秒、总预算五秒。不读取响应正文，只返回 `sent/failed/not_configured` 和可选状态码，底层 reqwest 错误不向调用者传播，不影响备份结果。默认不启用外部打卡服务。
- **R2 副本**：`R2BackupStore` 使用 `AmazonS3Builder::new()`、显式目的地/成对凭据和 `auto` region，不使用 from_env、profile 或实例元数据。object_store 只开 `aws-base/reqwest/ring`；连接 10 秒、请求 300 秒、最多重试 2 次且重试窗口 30 秒，CLI 另限制总预算。公开错误只有固定分类，不携带 SDK 错误链、远端正文或签名 URL。
- **副本写入与校验**：先 HEAD；已有对象完整回读，一致为 `already_present`，不一致为冲突。128 MiB 及以下使用原生 `PutMode::Create` 和 `S3ConditionalPut::ETagMatch`；更大对象使用原生分片，part 从 64 MiB 起，最多 10000 个、并发最多 2。分片完成接口没有条件写入，不拦截请求或改签名；不可变性依赖必须启用的 bucket lock、唯一 run_id、复用原密文与上传后完整校验。409/412、响应丢失和其他上传错误统一回读对账；不提供对象覆盖、删除或换名入口。失败可中止尚未完成的分片会话，强杀遗留需 bucket 生命周期回收未完成分片。HEAD 后以不超过 64 MiB 的顺序 get_range 覆盖全部字节，累计长度与完整 SHA-256 都匹配才返回成功；每段受单次请求超时、整体受调用方预算约束，ETag 不当作整文件摘要。月标记恢复逐条跳过无效或缺件的完成记录，真实传输/权限错误仍返回调用方。
- **单 Cookie 会话**：compose、资产操作与联合证据持有同一排他会话守卫；普通邮箱列表和详情使用按凭据、world、运行代次隔离的有界共享读取轮次，不等待 compose 锁。详情 GET 与完整 Inbox/Read Mail 扫描另以成员关系守卫协调；写入开始和结束（包括取消）都会使旧读取轮次失效。联合证据（身份、干净 compose 草稿、Main 现金、Sent 观察）只能在原排他会话内取得，普通读取不能提升为资产授权。
- **发送证明**：写入按 compose → 附件/COD → 发送分步进行，发送后读取发送前基线之外的新邮件，核对完整收件人、标题、正文与资产、唯一候选及响应 MID 提示，并再次证明 compose 已干净才视为成功；无法证明时报告“写入未知”，交由上层只读对账，绝不重发。基线随 prepared 句柄在原独占会话内一次消费，不推进持久游标或任务完成状态。
- **发送前基线**：`sent_baseline_mode` 默认 `shadow`，旧完整排除集唯一裁决，同时在同一读取轮次比较水位的新增 MID 集合与列表投影候选集合；`full` 保留旧路径。完整排除集可在同一凭据、world、endpoint、owner 代次与十分钟窗口内复用，fresh 完整枚举及成功后的前缀并集都可更新缓存，失效后重新扫描。`watermark` 每次发送前 fresh 读完最新分钟组，取最大受检数值 MID 及分钟时间；真正空列表且无下一页才构造 Empty。发送后另建 fresh 轮次，核对原锚点并读完边界分钟，按 `MID > W` 筛选，再走完整详情与 compose 证明。非数字、溢出、前导零或顺序异常不改变通用 opaque MID 契约：写前在原预算内回退完整扫描；写后锚点缺失则完整枚举并保留原边界，MID/时间前提失效且没有写前完整集合则保持未知。回退仍受 `maximum_pages ≤ 100`、原截止点、限速和字节限制；缺页或没找到 Sent 都不能证明未发送。该切片不新增持久水位，不改变常规 Sent/Read Mail 扫描或每日校验。切换与浏览器核实条件见[ops/README.md#sent-发送基线影子观察](../../ops/README.md#sent-发送基线影子观察)，配置只在受控重启后生效，默认 shadow 本身不解除 D3。
- **Sent 收件人投影**：原生列表收件人可能按字节截断；列表筛选与列表/详情绑定共用应用层 `mail_recipient_is_list_projection`，仅将非空前缀（允许末尾替换字符或省略号）作为候选。解析、维护 transport 与联合证据始终保留详情完整收件人；最终对请求目标做完整身份比较，不能用共享前缀证明发送。MID、世界、标题、正文、金额、装备集合、时间、唯一候选、fresh 轮次与读取屏障的原有约束保持。
- **Take / Return 证明**：Take 要求同一邮件身份、资产归零及正文领取回执；Return 要求与原邮件身份、正文和完整资产绑定的 `[RTS]` 回执。详情 GET 会让邮件离开只含未读项的 Inbox，Inbox 缺席或普通成功提示都不是动作完成证明。
- **装备出库**：发送前按冻结装备名推导的 Armory 分类读取库存，提示缺失、无法识别或未覆盖全部目标时回退完整七类扫描；精确绑定 EID、原生标签与名称；标签只允许 checkbox 后的单个空白装饰 span。名称可来自内联声明，或当前库存页引用的同源、同世界 `dynjs` 装备/仓库脚本；外部资源复用凭据 owner、限速、超时和 8 MiB 响应上限，只解码唯一 `dynjs_equip` / `dynjs_eqstore` 的 JSON 字面量，不执行 JavaScript。名称内图标不作为状态；全部目标唯一命中后才执行必要的最小出库/解锁，不重试、不回滚、不自动卸下装备。
- **装备属性分组**：HTML 的 `.ep` 属性键保留为 `分组 / 名称`，例如 `Spell Damage / Cold` 与 `Damage Mitigation / Cold`，两项独立保存值与 Base；`.ex` 和主属性原键保持。相同完整键、缺失分组标题、损坏列/单位/Base 仍拒绝，不能用覆盖或跳过重复项丢失事实。Public 与论坛的主属性首字母识别 `Primary Attributes / 名称` 及历史未分组键，既有装备快照不回填。
- **诊断归档**：拒绝或异常的远端响应与解析结果可加密保存为本地证据文件（父密钥优先独立的 `mail_diagnostics_data_key`，否则 `credential_data_key`，再否则 `equipment_data_key`），按保留天数与总字节上限清理，可由 `auction-cli mail export-evidence` 离线解密导出。归档头部记录写入域与 key ID；同域的旧 key 仍可解密历史文件。
- **论坛写入**：固定 origin；POST 发出后只有能证明远端结果（如回执中的主题/帖子身份）才算成功，否则返回 `write_unknown`，由上层按精确标题只读对账，不隐式重复创建。登录态通过读取用户控制台做只读正向证明，公开页面返回 200 不等于已登录。
- **论坛读取凭据边界**：普通页面与 PM/资料 GET 在请求前和完整正文后复核同一 owner 与 Cookie；读后失锁不交付响应。HTTP/协议错误、读后复核失败及取消时清理自有正文缓冲。回复表单必须完全没有 `p` 写入目标，冲突重复字段不能被当作缺失；编辑表单仍逐字段绑定目标主题和帖子。
- **论坛真实页面兼容**：Sent 列表的旧版排序链接允许空 `st=` 仅表示第一页；`href="#"` 工具栏控件不参与分页；PM 详情标题可位于 `.formsubtitle`。主题引用的 `qpid` 可作为帖子身份别名，但必须同时满足同源、同主题、`CODE=02/act=Post` 与目标板块；交叉主题、跨源、错误动作、重复身份仍拒绝。
- **论坛主题总预算**：主题最多读取 20 页、每页最多 8 MiB、累计最多 32 MiB；续页至多 4 个并发请求，共享逐块扣减的总字节额度，超限响应不能先完整下载并解析。取消或失败时清理已接收的部分正文；首屏提取分页与帖子事实后释放原始 HTML。总字节额度约束响应正文，不代表 DOM 或分配器的总内存上限。
- **联合证据解析**：Main 现金和干净 compose 的世界声明校验与字段校验共用同一 DOM；仍分别保留世界证明和页面解析失败阶段，不跳过余额一致性、唯一节点或资产证明。
- **敏感缓冲**：装备读取在凭据复核等待、手工解析失败和取消时清理自有响应缓冲；完整原始响应仍原样交给加密证据准备。数据密钥解码使用固定 32 字节输出；诊断信封在最终缓冲区内原地加密，保持既有 nonce、密文和认证标签格式。
- **正文与输出分配**：发送暂存证明先逐字比较正文，仅在不同时执行原有换行规范化；正文摘要对无 CRLF 的输入直接借用去尾空白切片，保留既有摘要字节。装备等级、主属性缩写和平均卷值直接追加到最终 BBCode 缓冲，不为每件装备构造字段列表和拼接副本；403 分类直接对原始字节匹配 ASCII 协议标记，避免整页转码与小写副本。

## 维护约定

- 解析器只返回不含原始 HTML 的类型化结果；Cookie、页面 token 与原始正文不进入日志或错误文本。
- 远端页面结构变化应表现为明确的解析失败（并留下诊断证据），不能退化为“看似成功”。
- 修改加密 AAD、指纹域或信封格式会使既有密文无法解开，必须以新版本前缀并保留旧版解密路径。
- 多 key 解密只在该域的密钥环内进行：信封的 key ID 未知时不回退到当前 key，也不尝试其它域的父密钥；新写入永远使用当前 key。

论坛复用原持久 claim；`submit_form` 在第一个实际 POST 前通过 `ForumWriteProgress` 同步登记，关闭入口只拒绝首次登记，已登记的同一工作项继续完成必要 POST/回读，结果提交仍由应用/Worker owner 确认。MoogleMail delivery、discard 和 PM 的围栏调用开始即受保护，登记不代替持久围栏确认。装备准备、compose、Sent 核对及 PM 的 POST/Sent 都保持原凭据与结果未知边界，不因排空延长请求或重试外写。

## 外部客户端指标边界（W0.4/W0.5）

本 crate 通过应用层窄端口 `ExternalMetricsObserver` 记录同步事件；
不依赖 Prometheus，记录器无 I/O、无 `.await`，关闭指标或非 Worker 角色为 no-op。
指标不改变请求间隔、锁顺序、deadline、重试、取消、写入围栏或未知结果处理。

- **物理请求**：`requests_total` 在通过校验、即将执行 HTTP 请求时加一；
  锁等待、凭据拒绝、缓存复用不增加请求数。`results_total` 在请求边界终结时加一，
  取消由 guard 补记；`ok` 不表示发送、资产或投递证明完成。
- **共享读取**：Inbox/Read Mail 的物理请求在共享任务内部计数；等待者和缓存命中
  不重复计数。取消等待者不改变既有共享请求的寿命，也不伪造一次新请求。
- **邮件锁**：只记录获取等待 `exclusive/incoming_read/incoming_write`；
  不把持有时间、业务耗时或锁内步骤算作等待。
- **信号**：GET/POST 的超时、429 与战斗页识别分别记录 `timeout`、`rate_limited`、
  `account_busy`；POST 的 429/5xx/写后会话失效仍按原 `write_unknown` 结果处理，
  信号与结果可以同时出现，不能互相替代。
- **账号忙熔断（W1.1）**：连续命中战斗页时按账号与世界退避 1→2→5→10 分钟，窗口内
  只放行一次只读探测；其余读取与全部写入在发出网络之前返回 `account_busy`
  （`write_started=false`），delivery 的可用性检查因此在 sending 围栏前延后 claim。
  熔断状态只在进程内、可丢失，只限制请求节奏，不改变凭据、业务状态或写入未知判定；
  同一忙碌期只保存首份战斗页证据，后续重复命中不再占用诊断容量（W1.2）。
- **论坛写入未知**：每次逻辑写入返回未知结果时加一，包括写后失效、缺少精确证明
  与越过写入边界后的取消；只读对账失败不算新的未知写入。
- **PM**：prepare 只读，send 单次 POST 后核对同一 sender 的 Sent；`not_observed`
  只表示未找到完整证明，不授权重寄。核对是只读操作，不增加新的写入未知。
- **预算**：维护、观察、fetch、批次由 Worker 调用方记录；Sent/Read 扫描与共享读取
  在各自客户端预算边界记录。一次耗尽只由原 owner 记录，不在通用错误构造器累加。
- **World**：邮件只用 `main/isekai`；论坛允许 `shared`，仅用于没有权威世界上下文
  的资料读取，主题与 PM 仍使用各自的权威世界。

客户端层、Worker 层、HTTP 层与数据库层的失败计数口径不同，不可相加当作“总失败”。
完整指标名称、合法组合、直方图桶与缺样语义见
[crates/auction-observability/README.md#外部账号客户端指标w04w05](../auction-observability/README.md#外部账号客户端指标w04w05)。

## 安全错误记录

W0.2 I-B 在错误即将折叠或被丢弃的边界复用应用层 `diagnostics::record_error`。
本 crate 的私有 `diagnostics::project` 仅匹配类型、闭集变体与非敏感控制标志；
日志入口只接收安全投影，不接收原错误。固定类别使用 `SafeErrorKind` 关联常量。
`reqwest::Error` 只看超时、构造、正文/解码类别，不读取 URL 或错误链。
MoogleMail、论坛、PM、Sent 基线、装备来源与运行时开关错误都穷尽匹配。

字段沿用 `kind`（简短 owner）、`phase`（语义步骤）、`safe_kind`（闭集原因），
并按已有上下文附加 world、业务 UUID、`write_started` 或 `evidence_id`。
同一 owner 的超时与普通失败使用不同 phase；不使用编号或模块路径作为类别。
owner 清单与过滤例子见[ops/README.md#安全错误与未持久化指标](../../ops/README.md#安全错误与未持久化指标)。
绝不记录 URL/query、响应片段、请求头、Cookie、玩家名、邮件主题/正文或凭据；
也不调用原错误的 Display、Debug、source。加密归档沿用原格式与访问边界。

内部不变量、诊断归档/加密失败用 ERROR；传输、远端拒绝、解析漂移和未知结果用 WARN；
缓存回退、不能比较的影子样本与正常输入拒绝用 DEBUG。
Sent 指标 observer 先于日志与去重；不改变评估、回退或取消事件的数量和值。
高频日志由 Worker/Public/Admin 的长期 owner 复用 60 秒诊断门，不缓存事件或身份。
CLI 等未安装 owner 的调用方直接输出安全日志，不创建后台任务。
外写未知仍只读对账，绝不自动重发；缓存失败继续原回源路径。

保留的错误折叠须按输入类别解释：Admin 预览、URL/key/数字等闭集输入校验仍返回固定拒绝；
论坛渲染的有界缓冲只产生 BodyTooLarge，保留原映射；共享读取轮次的失败结果由生产者记录，
消费者筛除已失败缓存不重复记录。远端响应解码和本地依赖失败不能归入这些例外。

I-B 新增以下安全类别，均有同名大写关联常量与 `from_label` 的显式映射。
原有 59 类保持原值，当前闭集共 113 类；未知标签才归 other，任意远端文本不进入 label。
完整闭集由应用层投影拥有，新增类别如下：

| 分类 | 新增 safe_kind |
| --- | --- |
| 本地依赖与观测 | `clock_skew` |
| 本地依赖与观测 | `credential_unavailable` |
| 本地依赖与观测 | `cryptographic_error` |
| 装备解析 | `equipment_attributes_invalid` |
| 装备解析 | `equipment_canonical_mismatch` |
| 装备解析 | `equipment_category_mismatch` |
| 装备解析 | `equipment_category_unknown` |
| 装备解析 | `equipment_kind_unknown` |
| 装备解析 | `equipment_level_invalid` |
| 装备解析 | `equipment_name_invalid` |
| 装备解析 | `equipment_page_structure_invalid` |
| 装备解析 | `equipment_quality_unknown` |
| HTTP 响应 | `http_bad_gateway` |
| HTTP 响应 | `http_internal_error` |
| HTTP 响应 | `http_server_error` |
| HTTP 响应 | `http_service_unavailable` |
| 本地依赖与观测 | `io_error` |
| 本地依赖与观测 | `persistence_unavailable` |
| 远端协议 | `remote_account_busy` |
| 远端协议 | `remote_authentication_error` |
| 远端协议 | `remote_body_error` |
| 远端协议 | `remote_challenge` |
| 远端协议 | `remote_identity_mismatch` |
| 远端协议 | `remote_identity_unproven` |
| 远端协议 | `remote_permission_denied` |
| 远端协议 | `remote_protocol_error` |
| 远端协议 | `remote_rate_limited` |
| 远端协议 | `remote_recipient_invalid` |
| 远端协议 | `remote_response_too_large` |
| 远端协议 | `remote_session_expired` |
| 远端协议 | `remote_topic_locked` |
| 远端协议 | `remote_transport_error` |
| 远端协议 | `remote_write_unknown` |
| 本地依赖与观测 | `request_encoding_error` |
| 本地依赖与观测 | `response_parse_error` |
| 运行时开关 | `runtime_switch_corrupt` |
| 运行时开关 | `runtime_switch_outcome_unknown` |
| Sent 基线 | `sent_baseline_anchor_changed` |
| Sent 基线 | `sent_baseline_anchor_missing` |
| Sent 基线 | `sent_baseline_boundary_unavailable` |
| Sent 基线 | `sent_baseline_cache_boundary_delta` |
| Sent 基线 | `sent_baseline_canceled` |
| Sent 基线 | `sent_baseline_invalid_mid` |
| Sent 基线 | `sent_baseline_order_violation` |
| Sent 基线 | `sent_baseline_page_discontinuity` |
| Sent 基线 | `sent_baseline_page_limit` |
| Sent 基线 | `sent_baseline_read_unavailable` |
| Sent 基线 | `sent_baseline_response_hint_conflict` |
| Sent 基线 | `sent_baseline_scope_changed` |
| Sent 基线 | `sent_baseline_timeout` |
| Sent 扫描 | `sent_scan_page_limit` |
| Sent 扫描 | `sent_scan_timeout` |
| 本地依赖与观测 | `shadow_mismatch` |
| 本地依赖与观测 | `task_panic` |
