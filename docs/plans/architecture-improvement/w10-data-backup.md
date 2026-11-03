# W10 数据生命周期与备份

[计划入口](../architecture-improvement.md) · [执行须知](../architecture-improvement.md#2-执行须知) · [交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [进度](../architecture-improvement.md#7-进度) · [现状基线](baseline.md) · [决策记录](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 数据生命周期与备份：定时与异机备份、迁移演练、WAL 归档、RPO/RTO、恢复演练、按政策清理、PostgreSQL 服务端基线、恢复后外部核对 |
| 优先级 | P0（W10.1、W10.2、W10.6），其余 P1 |
| 里程碑 | W10.1、W10.2 为 M1 首批，W10.6 随 M1；W10.8、W10.7 随 M2a；W10.0、W10.3–W10.5、W10.9 随 M5 |
| 依赖 | W10.7 依赖 W10.2、W10.8；W10.9 依赖 W1.9、W1.10 |
| 迁移 | 视政策 |
| 估算 | 13–17 天 |
| 关联缺陷 | —（详见[已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 6、7、11、13、18、23、24、25、26、29、31、41、48、49、50（详见[决策记录](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W10.0 | 恢复目标已定：启用 WAL 归档（决策 18，实施见 W10.7，M2a）；RPO ≤ 5 分钟（`archive_timeout` 300 秒）、RTO ≤ 2 小时、每月一次恢复演练（决策 31）。本项只剩把这些结论写入保留政策 v2 的 `restore_rto_rpo_and_rehearsal_cadence`，随 W10.4 的政策发布在 M5 完成 |
| W10.1 | 定时备份（M1 首批单独交付）：systemd timer 模板 + `auction-cli backup create/verify`，回执；成功、失败与磁盘水位检查留有可选的打卡地址（成功报到、失败立即报失败，超时未报到由外部服务告警，决策 41）；用户暂不启用外部服务（决策 50），地址留空即不打卡，在 W5.5 并入运维告警之前，失败只体现在回执、systemd 失败状态与日志中；本地保留份数与磁盘水位 |
| W10.2 | 异机副本（M1 首批单独交付）：加密后上传 Cloudflare R2，保留 30 份日备份与 12 份月备份，副本校验（决策 6）；日/月备份分前缀，轮换由 bucket 生命周期规则按前缀与年龄淘汰；R2 令牌只有“读写”与“只读”两档，服务器令牌只授予该 bucket 的对象读写，防删除与防覆盖依靠必须启用的 bucket lock，按前缀设锁定期且不长于生命周期淘汰期：日备份 30 天、月备份 365 天、WAL 与基础备份 21 天（决策 41、49）；上传与校验结果同样经可选打卡地址报到；以 `age` 公钥加密，服务器只持有公钥，私钥离线保存于密码管理器与一份离线副本（决策 13）；三组数据密钥与 `age` 私钥一起离线托管，托管与取回步骤写入[备份与恢复](../../../ops/deploy/RECOVERY.md)（决策 48）；R2 凭据进入敏感 TOML 并纳入 `config check` |
| W10.3 | 定期恢复演练：在隔离实例执行 `db rebuild --ephemeral`/恢复预检并出回执；演练前确认磁盘余量 |
| W10.4 | 按政策只清理运行态数据族（会话、心跳、realtime 事件、诊断等）；审计与资金事实永不删除；每次删除先只读演练并经用户批准（决策 7）。期限：进程心跳、实时事件与邮件诊断见决策 23–25；会话只删除已注销或已过期满 30 天的记录（决策 29），有效会话（含永久会话）不受影响；不提供个人数据删除（决策 26）。前置：发布保留政策 v2（现行 v1 对 realtime、会话为 `retain_forever`，并有未决的 `required_operator_decisions`）；在[实时事件契约](../../contracts/realtime.md)中定义回放保留期限与过期游标行为 |
| W10.5 | 备份范围：除数据库外，纳入 runtime 配置、加密邮件诊断证据与部署回执（数据密钥与备份密钥的离机托管已随 W10.2 在 M1 完成，决策 48） |
| W10.6 | 迁移演练（无预发的替代，提前到 M1）：含迁移的版本部署前，把最新备份恢复到本机或隔离的 PostgreSQL 18 → 用新程序执行迁移 → readiness 检查 → W11.4 巡检（落地后加入）→ 记录迁移耗时与回执；M1 先以手工流程与回执模板交付，W7.2 自动核对回执 |
| W10.7 | WAL 归档与时间点恢复（决策 18、41、49，M2a）：`archive_mode = on`，`archive_command` 只在本机完成 zstd 压缩与 W10.2 的 `age` 公钥加密，写入本地暂存目录并 fsync，任一步失败返回非零由 PostgreSQL 重试；独立的上传器（systemd timer）把暂存文件上传 R2 独立前缀、校验后删除本地副本，对象已存在且校验一致视为成功（bucket lock 下重传返回 409）；每周一次 `pg_basebackup` 物理基础备份同样加密上传；`archive_timeout` 为 300 秒（RPO ≤ 5 分钟，决策 31）；可恢复到过去 14 天内任一时刻：WAL 与基础备份都保留 21 天（同时保有 3 份基础备份），生命周期与锁定期按此对齐（决策 49）；归档、上传与暂存目录大小进入 W0.10 的指标、可选打卡地址与 W5.5 告警，避免积压撑满磁盘；开启 `archive_mode` 需重启 PostgreSQL，受 W7.5 时段门禁约束；W10.3 演练覆盖时间点恢复；同步[备份与恢复](../../../ops/deploy/RECOVERY.md) |
| W10.8 | PostgreSQL 服务端基线（M2a，先于 W10.7）：仓库提供 `postgresql.conf` 片段模板与说明：`max_connections` 按三角色服务档、维护档与 exporter 的连接预算；`shared_preload_libraries = 'pg_stat_statements'`（W12.4 的热点分析依据）；`log_min_duration_statement`、`log_lock_waits`、`log_autovacuum_min_duration`；热行表（`realtime_streams`、`lots`、`users`）与大只追加表的 autovacuum 参数；`archive_*`（W10.7）；只监听本机（W9.6）；以此回答保留政策的 `production_postgresql_topology_autovacuum_and_maintenance_envelope`；需重启的参数受 W7.5 约束 |
| W10.9 | 恢复后外部核对（M5）：Worker 在 W1.9 全局暂停下启动，经 W1.10 的 control 只读扫描备份时刻之后的 Sent 与 Read Mail；把恢复库中仍为 pending/claimed、且与 Sent 精确匹配的投递隔离为人工对账，列出恢复库没有记录的已读入站邮件与论坛新帖，输出回执；替代[备份与恢复](../../../ops/deploy/RECOVERY.md#6-外部历史核对与启动)中的纯人工核对 |

验收：定时备份连续 7 天成功并有回执；R2 中日/月副本按规则轮换且校验通过，服务器令牌删除锁定期内的对象被拒绝；M1 内完成一次端到端演练：从 R2 下载、用离线私钥解密、恢复到本地 PG18 并核对，同时用托管的数据密钥解开加密列（决策 48）；人为制造一次备份失败，回执与 systemd 状态如实反映（启用打卡后由外部服务在阈值内告警）；WAL 归档与上传无积压，并在隔离实例完成一次恢复到指定时间点的演练；迁移演练与恢复演练各至少完成一次并出回执；W10.3 演练中，W10.9 能找出备份时刻之后已寄出的投递。回退：停用 timer 与上传，不影响服务；关闭 `archive_mode` 需重启 PostgreSQL。
