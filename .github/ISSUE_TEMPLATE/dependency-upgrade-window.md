---
name: 依赖升级窗口
about: 每季度依赖与工具链升级窗口的执行与留证
title: "chore(deps): YYYY-Qn 依赖升级窗口"
---

## 窗口

- 窗口 ID：`YYYY-Qn`
- 日期（UTC）：`YYYY-MM-DD` 至 `YYYY-MM-DD`
- 日历记录：`ops/dependencies/upgrade-windows.json`
- 规则：[ops/dependencies.md](https://github.com/ngnlAYY/Auction/blob/main/ops/dependencies.md)

## 开始前核对

- [ ] 窗口未与换季发布冻结重叠（默认 04-28 至 05-04、10-29 至 11-04），或已在日历 `shifted_reason` 记录顺延原因
- [ ] 没有进行中的发布 tag 校验、部署或回退
- [ ] 已列出本窗口升级范围（依赖族与目标版本），每个依赖族单独 PR
- [ ] 需要数据库或 HTTP 栈变更的族已确认本机 PG18 负载演练可用；尚不可用时只做评估，不合入

## 逐族门禁

- [ ] `just verify` 通过（写明实际执行范围）
- [ ] `just test` 运行受影响范围；外部解析族确认脱敏回放语料与本地替身覆盖变更路径
- [ ] sqlx 或数据库驱动变更加跑 `just test-db`（隔离 PG18）
- [ ] 数据库或 HTTP 栈变更加跑本地 PG18 负载演练，并与窗口前基线比较
- [ ] 含迁移时完成迁移演练并附回执
- [ ] 合并前未夹带功能、契约或配置字段变更

## 收尾

- [ ] 每个依赖族的实际命令、环境、未覆盖范围与结论已记录在本 issue
- [ ] 日历中本窗口改为 `completed`，追加下一季度窗口与来年冻结区间
- [ ] `ops/README.md` 的“下一次升级窗口”已同步
- [ ] 未完成的依赖族留到下一窗口并写明原因，不留在本窗口的半成品分支
- [ ] 本窗口未部署生产；部署仍按运维手册的升级与回退流程
