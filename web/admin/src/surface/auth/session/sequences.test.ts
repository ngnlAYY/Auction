// 会话状态机的有界穷举入口；执行与断言主体放在独立夹具中，便于按长度复用。

import { test } from "bun:test";

import { runExhaustive } from "./sequences-fixture.test";

/** 默认套件穷举到长度 3；长度 4 与 Rust 的 `#[ignore]` 套件对应，默认跳过。 */
test("exhaustive_event_sequences_preserve_session_invariants：穷举事件序列保持会话不变量", () => {
  runExhaustive(3);
}, 30_000);

test.skipIf(process.env.AUCTION_WEB_SESSION_EXHAUSTIVE !== "1")(
  "exhaustive_event_sequences_up_to_length_four：穷举到长度四（耗时较长，与 Rust ignore 一致）",
  () => {
    runExhaustive(4);
  },
  // 四层穷举执行近七千万条序列，给手动入口留出共享机器上的运行预算。
  600_000,
);
