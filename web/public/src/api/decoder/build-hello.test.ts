// 构建号首帧解码的回归断言：只接受精确的 hello 形状，事件帧继续交给事件解码器。

import { expect, test } from "bun:test";

const { decodeBuildHello } = await import("./build-hello");

test("accepts_only_the_exact_build_frame：只接受精确的构建号首帧", () => {
  expect(decodeBuildHello('{"type":"hello","build":"build_2026-11-04.abc"}')).toBe(
    "build_2026-11-04.abc",
  );
  expect(decodeBuildHello('{"build":"0.1.0","type":"hello"}')).not.toBeNull();
});

test("prefers_the_optional_independent_build：独立构建号优先，缺省时与原行为一致", () => {
  expect(
    decodeBuildHello('{"type":"hello","build":"build_legacy","independent_build":"build_solid"}'),
  ).toBe("build_solid");
  expect(decodeBuildHello('{"type":"hello","build":"build_legacy"}')).toBe("build_legacy");
  for (const message of [
    '{"type":"hello","build":"b1","independent_build":7}',
    '{"type":"hello","build":"b1","independent_build":""}',
    '{"type":"hello","build":"b1","independent_build":"two words"}',
    '{"type":"hello","independent_build":"b2"}',
    '{"type":"hello","build":"b1","independent_build":"b2","extra":1}',
  ]) {
    expect(decodeBuildHello(message), message).toBeNull();
  }
});

test("rejects_unknown_shapes_and_invalid_builds：拒绝未知形状与非法构建号", () => {
  for (const message of [
    '{"type":"hello"}',
    '{"type":"hello","build":"0.1.0","extra":1}',
    '{"type":"bye","build":"0.1.0"}',
    '{"type":"hello","build":""}',
    '{"type":"hello","build":"two words"}',
    '{"type":"hello","build":7}',
    '["hello"]',
    "not json",
  ]) {
    expect(decodeBuildHello(message), message).toBeNull();
  }
});

test("does_not_swallow_event_frames：不吞掉事件信封", () => {
  // 事件信封不是 hello 形状，必须继续交给严格事件解码器。
  const event =
    '{"event_id":"1f0c","stream_sequence":2,"kind":"auction.opened","schema_version":1,"payload":{},"created_at":"2026-10-30T12:00:00Z"}';
  expect(decodeBuildHello(event)).toBeNull();
});
