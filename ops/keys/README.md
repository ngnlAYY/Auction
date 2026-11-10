# 数据密钥环与凭据轮换

[运维手册](../README.md) · [配置参考](../config/README.md) · [备份与恢复](../deploy/RECOVERY.md) · [auction-cli](../../apps/auction-cli/README.md)

本页说明四组数据密钥的轮换、离线重加密、旧密钥保留期与外部 Cookie 轮换。所有命令都只输出非秘密元数据（key ID、数量、时间）；密钥材料、明文、密文与指纹不得写入日志、回执或工单。

## 四个独立密钥域

| 域 | 当前配置字段 | 加密内容 | 解密消费者 |
| --- | --- | --- | --- |
| `equipment` | `equipment_data_key_id` / `equipment_data_key` | `equipment_secrets` 的装备短时 key 信封 | Public、Admin、Worker |
| `user_login` | `user_login_data_key_id` / `user_login_data_key` | `user_login_key_envelopes` 的 MoogleMail 登录 key 信封，以及 `user_admin_commands` 中持久化的命令响应信封 | Admin、Worker |
| `credential` | `credential_data_key_id` / `credential_data_key` | `credential_keys` 的外部 Cookie 信封 | Admin、Worker |
| `mail_diagnostics` | `mail_diagnostics_data_key_id` / `mail_diagnostics_data_key` | MoogleMail 加密诊断归档 | Worker、`auction-cli mail export-evidence` |

规则：

1. 每个域使用独立的 32 字节随机材料（标准 Base64，`openssl rand -base64 32`），不同域不得复用。
2. 新写入永远使用该域的当前密钥；信封记录自己的 key ID。
3. 解密只在该域的密钥环内按信封 key ID 解析；未知 key ID 直接失败，不回退到当前密钥，也不跨域尝试。
4. `mail_diagnostics` 未配置时，诊断归档沿用 `credential`、缺失再沿用 `equipment`（历史部署兼容）；配置后新归档写入独立域，历史归档仍按文件头 `key_source` 选择对应域密钥解密。

## 轮换步骤

1. **生成新密钥**：`openssl rand -base64 32`，并选定新的 key ID（1–64 字节，字母数字开头，可含 `.`、`_`、`-`）。
2. **把原当前密钥移入旧密钥数组**，带上停止写入的时间：

   ```toml
   equipment_data_key_id = "equipment-2026-11"
   equipment_data_key = "<新密钥>"

   [[equipment_previous_data_keys]]
   id = "equipment-2025-11"
   key = "<原密钥>"
   retired_at = "2026-11-05T00:00:00Z"   # RFC 3339，不得在未来
   ```

   同样形状的数组：`user_login_previous_data_keys`、`credential_previous_data_keys`、`mail_diagnostics_previous_data_keys`。

3. **三角色同时更新**并 `auction-cli config check --strict`。该检查会拒绝成对缺失、未知字段、非法 key ID、重复 key ID、与当前密钥冲突以及未来的 `retired_at`。
4. **核对状态**：`auction-cli keys status --strict`。任一信封仍引用未配置的 key ID 时该命令以非零退出。
5. **离线重加密**：先演练，再写入。

   ```bash
   # 演练：只解密并验证，不写库
   auction-cli keys reencrypt --domain credential
   # 写入：逐行 CAS（旧 key ID 必须匹配）替换，并追加审计
   auction-cli keys reencrypt --domain credential \
       --apply --actor ops@example --receipt /var/lib/auction/keys/credential-20261105.json
   ```

   `--domain` 省略时依次处理 `equipment`、`user_login`、`credential`。`user_login` 同时重加密 `user_login_key_envelopes` 与 `user_admin_commands` 中持久化的命令响应信封；两类信封都完成后旧 key 才无引用。命令可重复执行：第二次扫描不再发现旧 key 的信封，`rewritten` 为 0，回执仍会生成（幂等）。任何信封在重加密期间被并发改写都会中止并提示重新运行。
6. **再次核对**：`auction-cli keys status --strict`，确认各域只剩当前 key ID，且旧 key 的 `still_referenced` 为 `false`。

回执（`auction-reencrypt-receipt-v1`）与审计只包含：格式、回执 UUID、是否写入、起止时间、各域当前 key ID、来源 key ID 与数量、扫描/重写数量与本机有效 UID。审计动作为 `system.data_key_reencrypt`，`entity_type = data_key`，`entity_id` 为域。

## 旧密钥保留期

- `retain_until` 恒等于 `retired_at + 12 个月`（`DATA_KEY_RETENTION_MONTHS`），与最长备份保留期一致；配置不提供更短的覆盖值。
- 旧密钥在 12 个月内**必须**保留在密钥环中，即使数据库已无引用：备份与恢复可能带回旧信封，PITR 后仍需要它解密。
- `auction-cli keys status` 会列出每个旧 key 的 `retired_at`、`retain_until`、`still_referenced` 与 `removable`。只有 `removable = true`（超过保留期且无引用）才允许从配置删除。
- `--strict` 在两种情况下失败：存在引用未配置 key ID 的信封；或已过保留期却仍有引用的旧 key。第二种情况必须先完成重加密。
- 诊断归档是一次写入的加密证据文件，命令不会重写它们。旧 `credential`/`equipment` 密钥至少要保留到对应归档超过 `diagnostics.retention_days`（上限 365 天）；12 个月下限覆盖该窗口。

## 外部 Cookie 轮换

Cookie 本身不是数据密钥，但同样需要明确流程：

1. 在来源站点重新登录，取得新的 Cookie 值；不要复用旧会话。
2. 停止依赖该凭据的 Worker 能力（MoogleMail 收件/发送、论坛同步），避免读到半更新状态。
3. 用 `auction-cli credential import` 从 stdin 写入新 Cookie（不经命令行参数或环境变量），会重新密封当前 `credential` 密钥并记录审计。旧的 `credential_keys` 行随后应为 `disabled`/`invalid`，或由 Admin 处置。
4. 重启 Worker，确认 `credential_invalid` 告警消失、readiness 正常，并完成一次真实收发/读取验证。导入成功不等于远端认证成功。
5. 若轮换期间同时更换 `credential_data_key`，先完成上文的信封重加密，再更新 Cookie；两者顺序不可颠倒，否则旧 Cookie 信封可能在 Worker 重启前无法解密。
6. 论坛与邮件 Cookie 分属不同 `credential_kind`，可以独立轮换；`shared` scope 的记录会被两个世界共同使用，轮换前确认没有第二个世界仍引用旧值。

## 本地演练

`--apply` 会写库，只能在明确窗口内执行。演练请使用隔离的 PostgreSQL 18 与停机的 Worker，并在回执中保留 key ID 与数量，验证：

1. 演练模式下 `rewritten = 0`，且所有候选信封都能用旧 key 解密。
2. 写入模式回执的 `rewritten` 等于候选数量，第二次运行 `scanned = 0`。
3. 移除旧 key 后仅保留新 key 的进程仍能读取全部信封。
