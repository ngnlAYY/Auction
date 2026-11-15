# auction-test-support

[docs/README.md](../../docs/README.md) · [docs/development.md#测试](../../docs/development.md#测试) · [crates/auction-integrations/README.md](../auction-integrations/README.md)

只进 `dev-dependencies` 的外部服务替身：在 IPv4 回环临时端口上用脱敏的真实页面回放 MoogleMail 与 E-Hentai 论坛，让真实 HTTP 客户端在测试中走完整的请求、解析与错误分类路径。不连接真实账号，不进入任何发布二进制。

## 接口

| 接口 | 说明 |
| --- | --- |
| `mooglemail::MailDouble` | `start()` 绑定 `127.0.0.1:0`；`endpoint()` 交给真实客户端配置，路径与查询仍由客户端自己生成。只回放 main（`/`）与 isekai（`/isekai/`）下 `s=Bazaar&ss=mm` 的列表、详情与写信页；装备页（`/equip/`、`/isekai/equip/`）没有语料，只回放注入的账号状态故障 |
| `MailDouble` 状态操作 | `set_fault`；`set_return_receipt_unread` 让唯一同时有 Inbox/Read 详情的 MID 3125357 在两个列表间移动；`remove_sent`/`restore_sent` 让一封邮件从 Sent 列表与详情中消失或在 POST 之后才出现；`set_captured_pages_complete` 把已采集的最后一页按真实末页结构回放，供需要完整枚举的测试显式开启 |
| `MailDouble` 观测 | `read_requests()`、`write_requests()` 分别统计 GET 与 POST（结果未知的写入也计一次）；`requests()` 按到达顺序返回方法与路径查询，包括不支持的路由；`max_concurrent_requests()` 返回同时处理中的最大请求数，用于证明同一账号的物理请求没有重叠 |
| `mooglemail::Fault` | `AccountBusy`、`SlowAccountBusy(d)`（延迟后返回同一账号忙标记，模拟慢请求在途期间才打开熔断窗口）、`CookieInvalid`（登录跳转）、`Timeout(d)`、`ProtocolDrift`、`WriteResultUnknown`、`InsufficientPostage`、`MissingRecipient`、`WriteSucceeded`（脱敏的真实发送成功页）、`SlowWriteSucceeded(d)`（只延迟写入，模拟停机时仍在途的发送）、`StateLockLimited`（读写都只返回客户端已识别的 HV 状态锁限流正文）、`StateLockLimitedTooManyRequests`（同一限流正文配 429 状态码） |
| `mooglemail::UnsupportedService` / `UnsupportedFixture` | `MailDouble::require_fixture` 对没有获准语料的类别（论坛、战斗、登录）一律返回错误，不构造页面 |
| `forum::ForumDouble` | 同样只绑定回环。回放主题 302133（默认只挂第 0 页，其余分页须用 `set_topic_page` 显式提供）、PM 已发送列表（默认只挂第 0 页，`set_pm_sent_page` 可覆盖第 0 页或显式提供 `st>0` 分页）与详情 1558026、登录页与主世界战斗页；`set_pm_payload` 在真实 PM 结构中替换标题与正文；`write_requests()`、`requests()` 同上 |
| `forum::Fault` | `CookieInvalid`（返回真实登录页）、`ProtocolDrift`、`Timeout(d)`、`DeletedPost`（移除帖子正文）、`WriteResultUnknown`（只对已知的回帖与 PM 写入路径生效）、`Battle`（返回真实主世界战斗页；`ForumDouble` 收到 MoogleMail 列表请求时只回放这一种故障，正常邮件内容归 `MailDouble`） |

## 行为约定

- 故障持续生效，直到测试用 `set_fault(None)` 清除，不会被某次请求消费掉。
- 未采集的页面、分页、详情与写入成功页返回 `501 Not Implemented`，不复制或编造内容；页码或偏移量无法解析、论坛查询参数重复时返回 400。
- 装备页 GET 计入 `read_requests()`，只对 `AccountBusy`、`SlowAccountBusy(d)`、`StateLockLimited`、`StateLockLimitedTooManyRequests`、`CookieInvalid` 返回与邮件路由相同的注入响应；`Timeout(d)` 延迟后仍返回 501，其余情况一律 501。
- 只有 MoogleMail 有真实发送成功页。论坛回帖与 PM 的写入成功页没有语料，除注入的未知结果外，论坛 POST 一律返回 501。
- 替身 drop 时中止监听任务，测试不会遗留后台服务。内部状态锁中毒或常量 selector 解析失败时直接 panic：本 crate 只进测试进程，`lib.rs` 因此放宽了 `clippy::expect_used`。

## 语料

页面在编译期从 `crates/auction-integrations/tests/fixtures/` 嵌入：

- `mooglemail/`：MoogleMail 列表、详情与写信页（`manifest.toml`），以及不在生成清单内的发送成功页 `compose-success.html`。
- `forum/`：公开论坛主题、登录页、论坛私信与主世界战斗页（`forum/manifest.toml`）。

来源、脱敏方式、解析入口与摘要写在各清单，说明见[crates/auction-integrations/tests/fixtures/README.md](../auction-integrations/tests/fixtures/README.md)。

邮件语料由 `ci/fixtures/sanitize_mooglemail.py` 生成，玩家撰写的自由文本主题在生成后人工替换为夹具文本；已提交语料的玩家替身与装备密钥替换值由无密钥哈希派生，只防肉眼识别，不防字典反查或暴力复算，派生方式、人工替换范围与 `--check` 的前提见[crates/auction-integrations/tests/fixtures/README.md](../auction-integrations/tests/fixtures/README.md)。新增回放类别必须先补充获准的脱敏语料，不能用合成页冒充真实页面。

## 使用方

- `crates/auction-integrations/tests/mooglemail_double.rs`
- `crates/auction-integrations/tests/forum_double.rs`
- `crates/auction-integrations/tests/mooglemail_maintenance_contention.rs`
- `crates/auction-integrations/src/clients/mooglemail/sender/baseline/tests.rs`
- `crates/auction-integrations/src/clients/mooglemail/client/reads/lists/sent.rs`
- `crates/auction-integrations/src/clients/mooglemail/gateway/internal/compose.rs`
- `crates/auction-integrations/src/clients/mooglemail/transport/delivery/tests.rs`
- `crates/auction-integrations/src/equipment/reader/tests.rs`
- `apps/auction-worker/src/bootstrap/shutdown/tests.rs`

各用例覆盖的场景与仍缺替身的范围见[docs/development.md#测试](../../docs/development.md#测试)。
