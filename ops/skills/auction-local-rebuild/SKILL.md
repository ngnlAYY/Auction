---
name: auction-local-rebuild
description: 更新 Auction 本地隔离环境的 Public/Admin：冻结源码、隔离 worktree 受控构建、恢复点、迁移、切换、验收与记录
---

# 适用范围

用户要求把 Auction 本地隔离环境（`<本地环境根>`）更新到某个源码版本时使用。只处理本地环境，不授权云端部署、数据刷新、浏览器业务测试或任何外部调用；这些需要用户另行明确要求。

权威规则以仓库为准：`AGENTS.md`、`ops/README.md`、`ops/deploy/RECOVERY.md` 与 `apps/auction-cli/src/cli/{mod.rs,database.rs}`（命令与参数）。本技能只记录环境事实、可复用工具和踩过的坑。占位符的含义见 `ops/skills/README.md`。

# 环境事实

- 环境说明与历次记录在 `<本地环境根>/README.md`，“当前状态”段是最新部署记录。`<云端配置副本>` 只供线上演练使用，不能用于本地启动，也不得读取其中的 Cookie、`.keyenv` 等秘密。
- 数据库：PostgreSQL 18，只用 `<本地 PG>/<本地库>`，由 `bin/pg-local.sh status|start|stop` 控制；不连接系统 5432，不用未显式指定目标的 psql。
- 应用：`bin/app-local.sh status|stop|start`，只启停 Admin 与 Public（启动顺序 admin → public，停止相反），不启动 Worker，不隐式迁移。启动前脚本会核对隔离数据库目标、CK/`.keyenv` 禁用、外部集成与排期关闭，不满足即拒绝。
- 入口：Public `<Public 端口>`、Admin `<Admin 端口>`，经用户级 Caddy 在回环与 `<局域网地址>` 上提供；进程身份记录在 `state/roles.json`，日志在 `logs/{public-api,admin-api}.log`。
- `bin/auction-worker` 与旧 `bin/auction-server` 必须保持不可执行（0600）；论坛/MoogleMail endpoint 固定为 `http://127.0.0.1:9`。不得恢复 Worker 执行权限、导入 Cookie 或开启外部功能。
- `<本地环境根>/.config.server/owner.lock` 已存在，因此 `auction-cli db migrate` 走 `guard=managed`；不得手工创建 `owner.lock`、执行迁移 SQL 或伪造迁移账本。

# 可复用工具链

每轮新建 `<本地环境根>/backups/local-rebuild-<UTC 时间戳>/`，把最近一轮的 `operations/*.py` 复制进新目录的 `operations/` 后使用：

| 脚本 | 作用 |
| --- | --- |
| `freeze.py [仓库路径]` | 冻结源码：已跟踪与未忽略文件的逐文件摘要、快照目录、tar 归档与清单（`source-freeze.json`、`source-file-sha256.json`） |
| `freeze_roles.py` | 记录现有角色进程身份与程序摘要 |
| `make_plan.py` | 生成 `plan.json`；构建目录由 `REBUILD_REPO` 指定（worktree 构建时必须设置） |
| `cutover.py <阶段…>` | 阶段 `freeze stop backup before validate install migrate start verify`，按需分批执行 |
| `install.py [--validate-only]` | 校验并原子安装四个程序，断言构建指纹与 plan 一致 |
| `db_snapshot.py <before\|after>` | 迁移前后的 SQLx checksum、表集合、列、行数与业务行摘要 |
| `verify.py` | 进程身份、readiness、HTTP 资源摘要、启动日志、配置、数据库隔离与表计数 |
| `update_readme.py <标题> <范围>` | 在 README“当前状态”插入本轮条目，保留历史 |

每轮必须按现场重新确定的内容：

- `make_plan.py` 的 `schema_before` 由旧 CLI `db status` 的 `latest_version` 实测，`schema_after` 由候选 CLI 的 `expected_version` 实测，不沿用上一轮的值。
- `db_snapshot.py` 的“预期变化表集合”按本轮实际迁移区间推导（例如 108 只新增 `mail_template_events`，`audit_records` 与 `schema_migration_compatibility` 各追加 1 行）。旧轮的硬编码会让下一轮误报或漏报，复制后先改这里。

# 流程

## 1. 定位与冻结

1. 读仓库 `AGENTS.md` 与 `<本地环境根>/README.md`；用 `app-local.sh status`、`pg-local.sh status` 核对 Public/Admin/PG 的真实状态。
2. 原服务保持运行。先记录配置摘要与权限、运行中程序的摘要、当前 schema 与迁移 checksum。
3. 确定目标提交（通常是 `main` HEAD）。`git status --short` 不干净时，只冻结目标提交，现场改动不纳入制品，记入 `live_source_differences`。
4. 运行 `freeze.py`，记录提交、源码指纹、文件数与归档 SHA-256。

## 2. 隔离构建

其他会话经常同时编辑主仓库。live 工作树在构建期间被改动时，`release.sh` 会以“发布构建信息与当前构建输入不一致”失败；这是设计行为，保留失败证据到本轮 `failed-build/`，不要绕过。因此默认从冻结提交建立 detached worktree 构建：

1. `git worktree add --detach <本轮目录>/worktree <commit>`，再用 `source-file-sha256.json` 逐文件证明内容一致。worktree 会把部分文件权限规范成 0664，所以构建指纹会与冻结值不同；把 worktree 的构建指纹写进 plan，由 `install.py` 断言。
2. 受控工具回执的 `recipe_sha256` 绑定绝对路径（`cargo_config_fingerprint` 包含各级祖先 `.cargo/config*` 的路径），worktree 里必须重装：先复制主仓库的 `target/secure-build-tools`（约 2.5G），再执行 `NO_DOWNLOADS=1 bash ci/tools/bootstrap.sh install release`，约 4 分钟。
3. 在 worktree 中执行 `CARGO_BUILD_JOBS=2 just verify && CARGO_BUILD_JOBS=2 just release-check`。release profile 约 15 分钟，全程约 25–35 分钟；Fat LTO 内存占用大，不要提高并发。
4. 产物：`target/dist/artifacts/embedded/`（本地安装用）与 `target/dist/package/auction-rust-<版本>-<短提交>.tar.gz`。核对 `release package 检查通过` 行中的 commit、`state=clean` 与指纹。
5. 运行 `make_plan.py` 前设置 `REBUILD_REPO=<worktree>`，使 plan 的 `repository_root` 指向 worktree、`isolated_source_snapshot=true`。

本地构建的包只用于本地环境。线上部署只接受 GitHub Release 资产（见 `auction-production-deploy`），不要把本地包带上线。

## 3. 切换

1. 用候选 CLI 跑严格配置预检：`<候选>/bin/auction-cli --config-dir config config check --strict`，通过后才停止旧服务。
2. 有待执行迁移时：
   - 用匹配当前 schema 的**旧 CLI** `backup create --database <本地库> --output …` 建立恢复点，再用新、旧 CLI 各 `backup verify --database … --manifest …` 一次；
   - 核对目标库精确为 `<本地 PG>/<本地库>`、Public/Admin 已停止且没有其他数据库会话；
   - 执行 `cutover.py migrate`（用刚安装的 CLI 与 `config/owner.secret.toml`），回执必须是 `guard=managed`。
3. 迁移前后运行 `db_snapshot.py`：SQLx checksum、表集合、行数与既有字段摘要只允许出现本轮预期的变化；新增的可空列不算改写既有数据。
4. `install.py` 原子安装四个程序（Worker 与旧 server 保持 0600），之后只启动 Admin 与 Public。

## 4. 验收与记录

1. `verify.py`：实际进程身份与程序摘要、回环与局域网的 readiness、全部 HTTP 资源摘要、启动日志（允许 sqlx 慢查询 WARN）、配置内容与权限、数据库隔离与表计数（只允许 `service_heartbeats` 等运行态变化）。
2. 部署回执写入 `state/DEPLOYMENT-<时间戳>.json`；用 `update_readme.py` 插入本轮条目，写明提交、迁移区间、恢复点及其 SHA-256、两端 payload 摘要和未纳入的现场改动。
3. 本轮目录写 `ROUND-NOTES.md`，并保留制品副本（`artifacts/embedded/`、BUILD-INFO、package 与 SHA-256）和构建收口摘要 `build-log-excerpt.txt`。
4. 确认归档完整后再 `git worktree remove --force <worktree>`（worktree 连同自装工具和构建缓存约 8G）。
5. 最终向用户报告：访问地址、各项检查结果、schema 变化、隔离状态、回执与 README 条目位置，以及未执行的验证（浏览器业务流、外部调用等）。

# 回退

迁移只向前。迁移后不能直接换回旧 schema 的程序。需要恢复时，先停止全部本地角色，再用本轮恢复点和旧程序整套恢复（`ops/deploy/RECOVERY.md`）。同名库重置只经 `auction-cli db rebuild --mode empty-for-restore`，不得手写 `DROP DATABASE`。
