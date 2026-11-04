# W6.3 回放语料

本目录按 HTTP surface 分清单：`manifest.toml` 为 MoogleMail，`forum/manifest.toml` 为公开论坛主题与登录页。只包含脱敏后的真实页面；来源、脱敏方式、解析入口、配对与摘要写在各清单。

缺失的论坛 PM 与战斗页没有用合成页冒充；W6.6 对缺失 surface 明确返回不支持。`ci/fixtures/sanitize_mooglemail.py --check` 重新核对邮件语料的确定性输出与泄漏断言。
