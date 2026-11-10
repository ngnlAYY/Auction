# 脱敏回放语料

本目录按 HTTP surface 分清单：`manifest.toml` 为 MoogleMail，`forum/manifest.toml` 为公开论坛主题与登录页。只包含脱敏后的真实页面；来源、脱敏方式、解析入口、配对与摘要写在各清单。

缺失的论坛 PM 与战斗页没有用合成页冒充；替身对缺失 surface 明确返回不支持。main 世界 Sent 第 0 页与 Read Mail 第 2 页带有指向未采集页的 Next 链接，默认请求下一页返回 501；只有测试显式调用 `MailDouble::set_captured_pages_complete(true)` 时，替身才把已采集的最后一页按真实末页结构（`<span>Next &gt;</span>`，同 isekai 页）回放，不新增页面内容。`ci/fixtures/sanitize_mooglemail.py --check` 重新核对邮件语料的确定性输出与泄漏断言。
