# auction-integrations

[文档导航](../../docs/README.md) · [邮件流程](../../docs/guide/mail-flows.md) · [论坛同步](../../docs/guide/forum.md)

外部世界的适配器：HentaiVerse MoogleMail（游戏邮件）与 E-Hentai 论坛的 HTTP 客户端和 HTML 解析、装备来源读取、论坛主题渲染、外部凭据与登录 key 的加密，以及装备 key 的密文信封。它实现应用层的端口，不读取配置文件、不访问数据库，所需参数由装配根传入。

外部端点固定：MoogleMail 为 `https://hentaiverse.org`（main 与 isekai 两个世界路径），论坛为 `https://forums.e-hentai.org`；测试时可通过配置改为回环地址。所有响应有字节上限、请求有最小间隔与超时，解析失败一律视为协议漂移而非成功。

## 模块

| 模块 | 内容 |
| --- | --- |
| [`crates/auction-integrations/src/clients/mooglemail/`](src/clients/mooglemail) | 协议模型与 URL 构造、受限 HTTP 客户端（限速、大小与超时边界）、HTML 解析器（收件列表/详情、Sent、余额、库存、身份）、分步写入（compose、附件、COD、发送）与发送后 Sent 证明、投递 transport、周期维护、单 Cookie 会话互斥、联合远端证据、加密诊断归档 |
| [`crates/auction-integrations/src/clients/ehentai/`](src/clients/ehentai) | 论坛客户端：主题分页读取、精确标题查找、发帖与编辑、登录态只读证明、按 UID 读取目标资料、论坛私信（装备超价提醒） |
| [`crates/auction-integrations/src/credentials/`](src/credentials) | 外部凭据：Cookie 规范化（邮件只保存 `ipb_member_id`/`ipb_pass_hash`，论坛保留原始 Cookie 头）、单次论坛账号登录获取 Cookie、凭据有效性检查、AES-256-GCM 密封（两类凭据使用不同 AAD 与指纹域） |
| [`crates/auction-integrations/src/cryptobox/`](src/cryptobox) | `DataKey`（32 字节 Base64 材料 + 非秘密标识）与装备 key 密文信封的封装/解封 |
| [`crates/auction-integrations/src/login_key/`](src/login_key) | 玩家登录 key 的签发、Admin 手工封装与解密：Users 保存 SHA-256 摘要和 AES-256-GCM 信封；投递发送时解密，Admin 创建/签发命令及其幂等重放也可通过专用 opener 解密返回生成的 key |
| [`crates/auction-integrations/src/equipment/`](src/equipment) | Admin 录入与邮件投稿的装备来源读取（host/path allowlist、可交易判定、canonical 装备事实）、Public 真实装备 URL 解析 |
| [`crates/auction-integrations/src/forum/`](src/forum) | 论坛主题 BBCode 渲染（含装备名称标色）与 Admin 提交的论坛快照预览解析 |

## 关键行为

- **单 Cookie 会话**：同一账号的普通读取与资产操作互斥；联合证据（身份、干净 compose 草稿、Main 现金、Sent 观察）在同一会话守卫内取得，守卫释放前不得发生其他写操作。
- **发送证明**：写入按 compose → 附件/COD → 发送分步进行，发送后读取 Sent 基线之外的新邮件，核对收件人、标题、正文与资产，并再次证明 compose 已干净才视为成功；结果无法证明时报告“写入未知”，交由上层只读对账，绝不盲目重发。
- **Take / Return 证明**：Take 要求同一邮件身份、资产归零及正文领取回执；Return 要求与原邮件身份、正文和完整资产绑定的 `[RTS]` 回执。详情 GET 会让邮件离开只含未读项的 Inbox，Inbox 缺席或普通成功提示都不是动作完成证明。
- **装备出库**：发送前证明库存中存在该装备，必要时只做最小的出库/解锁，不重试、不回滚、不自动卸下装备。
- **诊断归档**：拒绝或异常的远端响应与解析结果可加密保存为本地证据文件（父密钥优先 `credential_data_key`，否则 `equipment_data_key`），按保留天数与总字节上限清理，可由 `auction-cli mail export-evidence` 离线解密导出。
- **论坛写入**：固定 origin；POST 发出后只有能证明远端结果（如回执中的主题/帖子身份）才算成功，否则返回 `write_unknown`，由上层按精确标题只读对账，不隐式重复创建。登录态通过读取用户控制台做只读正向证明，公开页面返回 200 不等于已登录。
- **论坛主题总预算**：主题最多读取 20 页、每页最多 8 MiB、累计最多 32 MiB；续页至多 4 个并发请求，共享逐块扣减的总字节额度，超限响应不能先完整下载并解析。取消或失败时清理已接收的部分正文；首屏提取分页与帖子事实后释放原始 HTML。总字节额度约束响应正文，不代表 DOM 或分配器的总内存上限。
- **联合证据解析**：Main 现金和干净 compose 的世界声明校验与字段校验共用同一 DOM；仍分别保留世界证明和页面解析失败阶段，不跳过余额一致性、唯一节点或资产证明。
- **敏感缓冲**：装备读取在凭据复核等待、手工解析失败和取消时清理自有响应缓冲；完整原始响应仍原样交给加密证据准备。数据密钥解码使用固定 32 字节输出；诊断信封在最终缓冲区内原地加密，保持既有 nonce、密文和认证标签格式。

## 维护约定

- 解析器只返回不含原始 HTML 的类型化结果；Cookie、页面 token 与原始正文不进入日志或错误文本。
- 远端页面结构变化应表现为明确的解析失败（并留下诊断证据），不能退化为“看似成功”。
- 修改加密 AAD、指纹域或信封格式会使既有密文无法解开，必须以新版本前缀并保留旧版解密路径。
