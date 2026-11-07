# W9 安全加固

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 安全加固：密钥环、已知取舍复核、Admin 登录防护、主机加固、操作者身份、数据库最小权限、登录 key 自助轮换、入站与论坛滥用防护 |
| 优先级 | P2（W9.5、W9.7–W9.11 为 P1） |
| 里程碑 | W9.5、W9.7 随 M2a；W9.8、W9.10、W9.11 随 M2b；W9.9 为 M3 首项；W9.3、W9.4、W9.6 随 M5；W9.1、W9.2 取消 |
| 依赖 | W9.9 先于 W2；W9.11 与 W9.10 共用计数 |
| 迁移 | 是（密钥环、运行账号授权、回信预算计数） |
| 估算 | 11–15 天 |
| 关联缺陷 | —（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 4、5、17、30、35、37、43、46、53、63（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W9.1（取消） | Admin 暴露面保持现状，不加来源限制（决策 4） |
| W9.2（取消） | 不加第二因素（决策 5） |
| W9.3 | 数据密钥环：多 key ID 并存解密、新写入用当前 key、离线重加密命令与回执；邮件诊断归档改用独立密钥域；旧密钥保留期不短于最长备份保留期（W10.2 为 12 个月）；凭据 Cookie 轮换流程文档化 |
| W9.4 | 复核 CSP `unsafe-eval` 等已知取舍并记录接受理由；增加 HSTS 与 `form-action 'self'` |
| W9.5 | Admin 补偿控制（不改变决策 4、5；提前到 M2a）：登录失败计数指标与告警；Admin 只有一个账号，登录失败另加不按来源地址的全局限流与退避；限流键对 IPv6 按 /64 聚合；首次设置为 `admin_secret` 生成随机值，`config check --strict` 对短于 32 字节的密钥与生产形态（`cookie_secure = true`、loopback 监听）下未配置可信代理报警；`admin_secret` 轮换流程；若 W0.12 核实站点前有 CDN 代理（决策 43），Caddy 只信任 CDN 地址段并从其头部取得真实客户端地址，应用的 `trusted_proxy_cidrs` 只信任 Caddy，`config check --strict` 提示该链路缺失；在 `docs/architecture.md#安全边界`与运维手册中按决策 4、5 改写为“已接受风险 + 补偿控制” |
| W9.6 | 主机加固：systemd 模板补资源上限（MemoryMax、TasksMax）、`LimitCORE=0` 与可用的沙箱选项，用 `systemd-analyze security` 记录评分；运维手册补主机层基线：SSH 禁止 root 与口令登录、防火墙只开放必要端口、PostgreSQL 只监听本机、自动更新的重启窗口避开 Final（或关闭自动重启改人工）、时间同步、journald 持久化与容量上限（`SystemMaxUse`）、PostgreSQL 小版本升级纳入 W7.5 时段门禁 |
| W9.7 | 操作者身份（单人运营，决策 17；M2a，先于 W2）：Admin 维持共享身份；CLI 写入审计时自动附带系统 UID 与 `SUDO_USER`，`--actor` 仍为必填说明；审计与处置记录如实标注来源（Admin/CLI） |
| W9.8 | Public 会话上限（决策 30，M2b）：登录 key 与会话默认永久保持不变；`max_public_sessions` 默认值由 5 改为 10（配置示例、[ops/config/README.md](../../../ops/config/README.md)与[docs/business-rules.md](../../business-rules.md)同步）；满额登录时在同一事务、同一用户行锁内把最久未活动的有效会话标记为已注销（不删除记录），再建立新会话；被注销的设备用原 key 重新登录即可，并看到五语言的“已在其他设备登录”提示；同步 Admin 发布的五语言规则文档 |
| W9.9 | 数据库最小权限（决策 35；M3 首项，先于 W2）：迁移 owner 账号只供 CLI 的迁移、重建与恢复使用；Public、Admin、Worker 共用一个无 DDL、无 `TRUNCATE`、对只追加表无 `UPDATE`/`DELETE` 的运行账号，权限由迁移内的 `GRANT` 与默认权限维护（含序列与 `_sqlx_migrations` 只读）；敏感 TOML 区分运行账号与维护账号，`config check --strict` 拒绝服务使用 owner 账号；首次设置与合并部署同步；授权变更在部署时由用户执行；同步[docs/contracts/database.md](../../contracts/database.md)、[ops/config/README.md](../../../ops/config/README.md)与运维手册；是否再拆为按角色账号交 W8.2 评估 |
| W9.10 | 登录 key 自助轮换（决策 37，M2b，与 W9.8 同批）：新增邮件主题 `rekey`（不带资产）：在同一事务内轮换该账号的登录 key、注销其全部有效会话并回信新 key；`register` 仍重发原 key；`register` 与 `rekey` 每个账号 24 小时内各自只回信一次，超出的照常记录命令与审计但不回信（决策 53）；被拉黑或身份冲突时与 `register` 一样回信说明；新增回信模板进入配置示例；同步[docs/guide/mail-flows.md](../../guide/mail-flows.md)、[docs/business-rules.md](../../business-rules.md)与 Admin 发布的五语言规则文档 |
| W9.11 | 入站与论坛滥用防护（M2b，与 W9.10 共用计数）：按发件人与论坛账号限制每 24 小时最多 10 封说明信（决策 63），覆盖 `rebind` 结果、`auction` 拒收说明、`user_assets_rejected` 与论坛回执；超出只记命令与审计、不寄说明信，附带资产照常退回（资产属于发件人）；超额次数进入 W14.8 指标；阈值进入配置并纳入 W7.10 热读取；同步[docs/guide/mail-flows.md](../../guide/mail-flows.md)与 Admin 发布的五语言规则文档 |

验收：`systemd-analyze security` 评分写入运维手册；响应含 HSTS 与 `form-action`；Admin 登录失败超过阈值时产生告警且全局限流生效；密钥轮换在本地 PG18 演练并出回执；服务进程以运行账号连接时无法执行 DDL、`TRUNCATE` 或改写只追加表；`rekey` 后旧 key 无法登录、原有会话全部失效；同一发件人超出说明信预算后不再收到说明信，附带资产仍被退回。回退：安全头与限流参数可经配置回退；密钥环保留旧 key，可回到单 key 写入；运行账号可临时换回 owner 账号（`--strict` 下报错提示）；`rekey` 主题可经配置停用。
