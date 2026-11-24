# 脱敏回放语料

只包含脱敏后的真实页面，按 HTTP surface 分目录；来源、脱敏方式、解析入口、配对与摘要写在各自的清单：

- `mooglemail/manifest.toml`：MoogleMail 列表、详情与写信页，由 `ci/fixtures/sanitize_mooglemail.py` 从本地原始页面确定性生成，再人工替换玩家撰写的自由文本。
  - 派生：已提交夹具的玩家替身与装备密钥替换值已于 2026-11-15 一次性重映射为随机值（保持字符数、UTF-8 字节数、字符类别与列表截断前缀），与真实名称和密钥没有可复算关系，不能用公开玩家名单反查，也不能复算装备密钥。脚本以本地密钥的 HMAC-SHA256 派生（密钥默认 `<raw-dir>/.pseudonym-key`，0600 新建、不入库），重新生成的结果不会与已提交夹具逐字相同；`--check` 需要原始页面与生成时的同一把密钥，只能在维护者本机运行。
  - 自由文本：脚本按设计保留邮件主题与正文。`mooglemail/read-main.html`、`mooglemail/read-isekai.html` 中玩家撰写的自由文本主题已在生成后人工替换为中性夹具文本，保持字符数、UTF-8 字节数与列表截断关系（标题按 35 字节截断，截断处的残缺字符显示为 U+FFFD，`Re: ` 回复前缀不变）。系统固定文案及对它的回复、命令关键字（`auction`、`register`、`rebind`、`rekey`）、`[RTS]` 退信、采集账号自己发出的邮件和无内容的单字符主题原样保留。重新生成后须重做这一步才能入库；`--check` 只比对脚本输出，人工替换过的文件不会通过。
- `forum/manifest.toml`：公开论坛主题与登录页，以及已登录采集的论坛私信与主世界战斗页。论坛回帖与 PM 的写入成功页没有语料，除注入的写入未知故障外，替身对这些写入返回 501。

main 世界 Sent 第 0 页与 Read Mail 第 2 页带有指向未采集页的 Next 链接，默认请求下一页返回 501；只有测试显式调用 `MailDouble::set_captured_pages_complete(true)` 时，替身才把已采集的最后一页按真实末页结构（`<span>Next &gt;</span>`，同 isekai 页）回放，不新增页面内容。

## MoogleMail 发送成功页

`mooglemail/compose-success.html` 不在生成清单内，单独记录：

- 采集：2026-11-06，真实 HV MoogleMail 测试发送（无资产、无 COD）；不提交原始页面或 Cookie。
- 脱敏：玩家名、世界显示名、收件人名替换为 `Fixture*`；移除浏览器扩展与用户脚本注入的节点（含 `#mmail_fin` 之外的遭遇计时链接）；不含 token、MID 或凭据。
- 字节数：6123；SHA-256：`c75fdc40de61d65b771ceba766090c57ef8beb6f3d4f2b9adc2bf3a1164d20d7`
- 用途：`inspect_send_response` 的成功证明分支与 `MailDouble` 的 `WriteSucceeded`、`SlowWriteSucceeded`（POST 先延迟再返回同一页，供停机排空用例）。

## 寄售派生件（派生，未签收）

`mooglemail/consign-*.html` 从已脱敏的 `sentdetail-3125366.html`、`read-main.html` 与
`read-isekai.html` 派生，只替换收件人、发件人、主题、正文、附件清单、世界标记与身份页声明。
它们用于两世界单件/多件、Take 前后、系统 Return 回执、重复原文 EID、规范别名、未知附件、
数量冲突、Credits/COD、错收件人/世界、Take 缺/多/重复/同名 marker、Return 回执与脱敏回归。

- 这些文件不是真实采集样本，**未签收**；验证器版本标为未签收，生产上只能 `reconcile` / 隔离，
  不授处分权。
- 玩家名、装备编号、装备 key 与预约码全部是测试假值；预约码使用 `C1-` 加 43 个 `A`/`B`，
  不与真实签发值对应。
- `consign-identity-*.html` 在 `#mainpane` 注入唯一的 `let uid = 1000000;` 与 `#eqch_outer`，
  用于离线认证身份页；底部原有 `var uid` 不参与声明。
