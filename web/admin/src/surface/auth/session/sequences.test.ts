// 在常规测试内检查有界事件空间，按等价前缀合并归约次数。

import { expect, test } from "bun:test";

import { runExhaustive } from "./sequences-fixture.test";

test("exhaustive_event_sequences_preserve_session_invariants：穷举事件序列保持会话不变量", () => {
  // 19 个起点、62 种输入、长度 0–2，共 74,233 条序列。
  expect(runExhaustive(2)).toBe(74_233);
}, 20_000);

test("exhaustive_event_sequences_up_to_length_four：穷举到长度四", () => {
  // 5 个回执竞争起点、33 种边界输入、长度 0–4，共 6,114,905 条序列。
  expect(runExhaustive(4, true)).toBe(6_114_905);
}, 20_000);
