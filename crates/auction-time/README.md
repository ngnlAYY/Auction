# auction-time

[docs/README.md](../../docs/README.md) · [docs/architecture.md](../../docs/architecture.md)

全仓共用的时间基础类型：UTC 时刻、有符号时长、本地墙上时间到时刻的换算，以及日期与钟点的文本解析。底层运算由 [jiff](https://docs.rs/jiff) 提供，文本格式由本 crate 自己控制；后端各层与两个 Dioxus 浏览器前端都依赖它，本 crate 不依赖任何内部 crate，不含业务规则，不读配置。

## 接口

| 接口 | 说明 |
| --- | --- |
| `Timestamp` | UTC 时刻，纳秒精度。构造（`now`、`from_timestamp*`、`from_ymd_hms`、`from_utc_datetime`）、读取（`timestamp*`、`to_utc_datetime`）、运算（`+`/`-` 与 `checked_*`）、取整（`duration_trunc`、`trunc_subsecs`、`with_second`、`with_nanosecond`、`checked_add_months`）与文本（见下表） |
| `Duration`（别名 `TimeDelta`） | 有符号时长，取值范围是正负 `i64::MAX` 毫秒。构造、运算与反序列化共用这一个范围：`try_*` 与 `checked_*` 超出时返回 `None`，不带前缀的构造函数与运算符超出时 panic，只用于已校验或写死的取值；`/` 按秒与纳秒分别朝零整除 |
| `LocalTime::resolve(timezone, local)` | 本地墙上时间在某时区下对应的时刻：`Unique`、`Repeated { earlier, later }`（回拨）或 `Skipped { after }`（前跳） |
| `parse_clock`、`parse_date`、`parse_minute_datetime` | `时:分`、`年-月-日`、`年-月-日 时:分` 的宽松解析：字段可不补零、之前可有空白，年份可带符号 |
| `Timestamp::parse_from_imf_fixdate` | HTTP `Date` 头的定宽写法（`Sun, 06 Nov 1994 08:49:37 GMT`），星期须与日期相符 |
| `Timestamp::strftime`、`Timestamp::strptime_utc` | 按调用方写死的模式格式化或解析 UTC 字段；浏览器端不使用，避免把通用格式化代码编进 wasm |

`Timestamp::now()` 是读取系统时钟的唯一入口；领域层不调用它，时间一律由调用方传入。浏览器目标（wasm32）没有系统时钟，标准库的读取会直接崩溃；本 crate 在该目标上改读 `Date.now()`（毫秒精度）。

## 文本契约

下列输出逐字节固定。命令幂等摘要、证据摘要、命令账本与轮次快照里的 JSON 都由它们生成，改动任何一种格式都等于改动已经持久化的事实。

| 形式 | 示例 | 说明 |
| --- | --- | --- |
| 序列化（serde）与 `Debug` | `2026-11-17T10:00:00Z`、`2026-11-17T10:00:00.250Z` | 小数位按需取 0、3、6 或 9 位，偏移写作 `Z` |
| `to_rfc3339()` | `2026-11-17T10:00:00+00:00` | 偏移写作 `+00:00` |
| `to_rfc3339_opts(格式, use_z)` | `2026-11-17T10:00:00.000Z` | 小数位由 `SecondsFormat` 指定 |
| `Display` | `2026-11-17 10:00:00 UTC` | 用于日志与运维输出 |
| `Duration` 序列化 | `[3600, 0]` | 向下取整的秒与非负纳秒 |
| 反序列化与 `FromStr` | `2026-11-17 10:00:00.5 +08:00` | 宽松的 RFC 3339：字段可不补零，各部分之间可有空白，偏移还接受 `UTC` |
| `parse_from_rfc3339` | `2026-11-17T10:00:00Z` | 严格的 RFC 3339：字段定宽，偏移为 `Z` 或 `±hh:mm` |

可表示的时刻从 `-9999-01-02T01:59:59Z` 到 `9999-12-30T22:00:00.999999999Z`。闰秒（秒为 60）无法表示，解析时按超出范围拒绝。

日历日期（`jiff::civil::Date`）的文本只在公元 0–9999 年与迁移前一致；负年份的位数不同。排期来源因此只接受公元 1–9999 年的日期（`auction-domain` 的 `AuctionScheduleSource::resolve`），本地钟点的 JSON 由 `auction-domain` 的 `scheduling/local_wire.rs` 按既有格式读写并拒绝闰秒。

回归测试（`src/instant/tests.rs`、`src/duration/tests.rs`、`src/civil_text/tests.rs`、`src/local_time/tests.rs`）里的期望值取自迁移到 jiff 之前的实现；测试失败说明格式或接受范围变了，应修实现而不是改期望。

## 数据库

启用 `postgres` feature 后，`Timestamp` 可直接绑定到 `timestamptz` 参数并从结果行读取（含数组）。PostgreSQL 只保留微秒，写入时多出的纳秒朝 2000-01-01 方向截断。只有 `auction-storage` 启用该 feature。

## 时区

本 crate 不带时区数据库。`auction-domain` 与 `auction-config` 启用 jiff 的 `tzdb-bundle-always`，把 IANA 时区数据编进二进制，不读取宿主的 `/usr/share/zoneinfo`；时区数据随 `jiff-tzdb` 的版本升级而更新。`auction-config` 另启用 `tz-system`，供未声明时区的 cron 表达式读取进程本地时区。

时区数据库按本地时间查找时，会在 UTC 偏移不变的规则切换处多报一个并不存在的重复时刻（例如 `America/Nuuk` 的 2023-10-28 23:00–23:59）。`LocalTime::resolve` 只承认换算回去偏移仍然相符的候选，排期与 cron 都经这一个入口解析墙上时间，不直接使用时区库的本地时间查找结果。
