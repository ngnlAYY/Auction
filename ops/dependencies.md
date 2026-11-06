# 依赖与工具链升级

[ops/README.md](README.md) · [ci/README.md](../ci/README.md) · [docs/development.md](../docs/development.md) · [ops/dependencies/upgrade-windows.json](dependencies/upgrade-windows.json) · [docs/plans/architecture-improvement/w13-regression-gates.md](../docs/plans/architecture-improvement/w13-regression-gates.md)

依赖与工具链不随功能发布滚动升级：每季度设一个固定窗口，按依赖族分批升级，逐批通过门禁。Dependabot 只作提示，常规版本升级 PR 已关闭；本文件规定节奏、冻结日历、门禁与留证方式，具体窗口日期以[ops/dependencies/upgrade-windows.json](dependencies/upgrade-windows.json)为准。

## 节奏与范围

- 每季度一个窗口：1、4、7、10 月的第二个周一至周五（UTC），共 5 个工作日。窗口内只做依赖与工具链升级，不夹带功能、路由、配置字段或契约变更。
- 下一次窗口同时登记在[ops/README.md#发布日历与依赖升级窗口](README.md#发布日历与依赖升级窗口)；窗口开始前 14 天由[依赖升级窗口提醒](../.github/workflows/dependency-cadence.yml)输出 warning。
- 覆盖范围与顺序：
  1. Rust 工具链：[rust-toolchain.toml](../rust-toolchain.toml)、CI 的 `dtolnay/rust-toolchain` 固定版本与 MSRV 作业（1.94.1）；只提高工具链时 MSRV 基线保持不变。
  2. 异步运行时：Tokio LTS 及其直接依赖。
  3. HTTP 栈：Axum 与 tungstenite 必须同批升级——`ci/checks/check.sh dependency-versions` 要求正常依赖图只有一个 tungstenite 版本，Axum 的 WebSocket 错误按版本 downcast。
  4. 数据库：sqlx 与 PostgreSQL 驱动。
  5. 前端：Dioxus、`dx` 受控构建工具与 `wasm-bindgen`、`wasm-opt` 固定版本。
  6. GitHub Actions 固定提交。
  7. PostgreSQL 18 小版本：不占用窗口，按[ops/README.md#postgresql-小版本升级](README.md#postgresql-小版本升级)另作停机安排。
- 每个依赖族单独 PR（分支 `chore/deps-<窗口 ID>-<族>`），不混入无关修复；同一窗口内按上述顺序合入，前一族未通过门禁不开始下一族。工具链升级必须同时核对 `rust-toolchain.toml`、CI 工具链版本、MSRV 作业与 `Cargo.lock`。

## Dependabot 只作提示

- [.github/dependabot.yml](../.github/dependabot.yml) 的 `open-pull-requests-limit: 0` 关闭常规版本升级 PR；Dependabot 不再把新版本变成待合并分支，只在仓库启用 Dependabot security updates 时按安全公告创建修复 PR（安全更新有独立的 10 个开放 PR 上限）。
- 看到依赖有新版本时，把它记入当季窗口的范围，而不是直接合并 Dependabot 分支；Dependabot 分支不能作为“已评估”或“已通过门禁”的证据。
- 安全公告修复可以随时按修复交付，不受窗口限制；涉及数据库或 HTTP 栈时同样补跑下列门禁，且不自动合并。
- 关闭版本升级 PR 不等于关闭提醒：每周 `security-audit` 的 `cargo audit` 与手动运行的 `cargo machete` 仍检查锁定依赖与未使用依赖，仓库 Dependabot alerts 继续提供公告。

## 发布冻结日历

- 换季冻结（决策 57）：从赛季前场次开拍到首场早鸟场次结算完毕，只部署修复，不发布行为变更或含 `breaking` 迁移的版本。按默认排期（赛季开始 05-01、11-01 09:00 UTC）冻结区间为 04-28 至 05-04、10-29 至 11-04；实际以 Admin 排期与首场早鸟结算为准。
- 窗口与冻结区间重叠、或某个 `v<版本>` 的发布校验、部署、回退仍在进行时，顺延到下一个不冲突的自然周，并在日历 JSON 的 `shifted_reason` 写明原因；不得为赶窗口在冻结期内合入升级。
- 发布日（打 tag、发布工作流运行、部署与回退）不安排升级；同一窗口内先完成升级合入与验证，再进入发布流程。

## 门禁

| 变更 | 必须运行 |
| --- | --- |
| 任意依赖或工具链升级 | `just verify`；`just test`（W6 已落地入口） |
| 外部页面解析（MoogleMail、论坛） | 确认 W6.3 脱敏回放语料与 W6.6 替身覆盖本次变更路径，再运行 `just test` |
| sqlx、PostgreSQL 驱动或数据库连接层 | 加 `just test-db`（隔离 PG18） |
| 数据库或 HTTP 栈（Axum、tungstenite、hyper、Tokio 运行时、sqlx 连接层） | 加 W12.3 本地 PG18 负载演练：Final 阶段并发出价与匿名读突发，并与窗口前基线比较 |
| 新增或改写迁移 | 按[ops/deploy/MIGRATION-DRILL.md](deploy/MIGRATION-DRILL.md)完成 W10.6 演练并留存回执 |
| 合并到 `main` 前 | 上述项目全部通过；不把未过门禁的依赖族留在窗口内 |

W12.3 尚未交付（随 M4），见[延后](#延后)；在它落地前，涉及数据库或 HTTP 栈的升级只做评估与准备，不合入。

## 窗口流程

1. 窗口前 14 天按 [.github/ISSUE_TEMPLATE/dependency-upgrade-window.md](../.github/ISSUE_TEMPLATE/dependency-upgrade-window.md) 建立窗口 issue，填写窗口 ID、拟升级族与冻结核对结果。
2. 开始前确认 Admin 排期没有换季冻结、没有进行中的发布或回退；有冲突就顺延，并在日历 JSON 记 `shifted_reason`。
3. 窗口内按族升级、逐族过门禁；每族合并后把实际命令、环境与未覆盖范围写进 issue。
4. 窗口结束更新日历：本窗口改为 `completed`，追加下一季度窗口与来年冻结区间，并同步[ops/README.md#发布日历与依赖升级窗口](README.md#发布日历与依赖升级窗口)。
5. 窗口本身不部署生产；升级随下一次发布按[ops/README.md#升级](README.md#升级)的发布包、迁移演练与回退流程上线。

## 自动提醒

```bash
perl ci/checks/dependency-cadence.pl                          # 以当前 UTC 日期核对
perl ci/checks/dependency-cadence.pl --today 2027-01-05       # 指定日期复现提醒
perl ci/checks/dependency-cadence.pl --file /path/to/calendar.json
```

[.github/workflows/dependency-cadence.yml](../.github/workflows/dependency-cadence.yml) 每周一 05:07 UTC 运行同一脚本：日历结构错误、窗口逾期仍为 `scheduled` 或缺少下一次窗口时作业失败；窗口进入 14 天提前提醒期或正在进行时输出 warning，并把窗口表写入作业摘要。脚本只读取仓库文件与本地 UTC 日期，不联网、不修改日历、不创建 issue、不升级依赖。

[ops/dependencies/upgrade-windows.json](dependencies/upgrade-windows.json) 的字段：

| 字段 | 内容 |
| --- | --- |
| `version` | 固定为 `1` |
| `lead_days` | 提前提醒天数（当前 14） |
| `windows[]` | 季度窗口：`id`（`YYYY-Qn`）、`status`（`scheduled`、`completed`、`skipped`、`deferred`）、`starts_on`/`ends_on`（UTC 日期）、`note`，顺延时另加 `shifted_reason` |
| `blocked_ranges[]` | 换季发布冻结区间，窗口不得与之重叠 |
| `deferred[]` | 尚未满足的升级前置条件（`item`、`reason`），显示在提醒摘要中 |

## 回退

- 单个依赖族升级出问题时 revert 对应 PR；工具链回退同时恢复 `rust-toolchain.toml`、CI 工具链版本与 `Cargo.lock`，并重跑门禁。
- 已部署版本出现依赖问题时按[ops/README.md#升级](README.md#升级)与[ops/deploy/RECOVERY.md#程序回退](deploy/RECOVERY.md#程序回退)回退程序；迁移只向前，不能靠回退依赖版本改写已迁移数据库。
- 移除本机制只删除提醒工作流与脚本；日历与文档不参与运行时，不影响构建或部署。

## 延后

- W12.3 本地 PG18 负载演练随 M4 交付；在此之前，数据库或 HTTP 栈升级只做评估，不在窗口内合入。
- W6.2 存储端口覆盖与 W6.6 写表单成功路径仍在推进；升级前按所属 owner 确认本次变更路径已有测试，不以 `just test` 全绿代替覆盖声明。
- 本轮只建立节奏、日历、模板与提醒，不升级任何依赖或工具链。
