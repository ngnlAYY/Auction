// 浏览器错误类别与构建标识校验的回归用例，逐条对照 Rust `ui::bootstrap` 的测试。

import { expect, spyOn, test } from "bun:test";

Object.defineProperty(globalThis, "__AUCTION_BUILD_ID__", {
  configurable: true,
  value: "test-build",
});
Object.defineProperty(globalThis, "__AUCTION_PACKAGE_VERSION__", {
  configurable: true,
  value: "0.0.0",
});

const { browserErrorClassCode, buildIdIsValid, reportBrowserError, reportBrowserErrorWithBuildId } =
  await import("./index");

test("classes_are_a_closed_wire_set：错误类别使用闭集线上取值", () => {
  expect(browserErrorClassCode("wasm_panic")).toBe("wasm_panic");
  expect(browserErrorClassCode("dto_decode")).toBe("dto_decode");
  expect(browserErrorClassCode("realtime_protocol")).toBe("realtime_protocol");
});

test("build_id_accepts_only_url_safe_bounded_values：构建标识只接受长度受限的 URL 安全字符", () => {
  expect(buildIdIsValid("0.1.0")).toBe(true);
  expect(buildIdIsValid("build_2026-11-04.abc")).toBe(true);
  expect(buildIdIsValid("")).toBe(false);
  expect(buildIdIsValid("two words")).toBe(false);
  expect(buildIdIsValid("a/b")).toBe(false);
  expect(buildIdIsValid("a".repeat(65))).toBe(false);
});

test("non_wasm_reporter_is_a_noop：宿主环境没有 window 时上报不发请求", () => {
  let requests = 0;
  const unexpectedFetch = Object.assign(
    () => {
      requests += 1;
      return Promise.reject(new Error("unexpected fetch"));
    },
    { preconnect: () => undefined },
  );
  const fetchSpy = spyOn(globalThis, "fetch").mockImplementation(unexpectedFetch);
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  try {
    Object.defineProperty(globalThis, "window", { configurable: true, value: undefined });
    reportBrowserError("wasm_panic");
    reportBrowserErrorWithBuildId("dto_decode", "invalid build id");
    expect(requests).toBe(0);
  } finally {
    fetchSpy.mockRestore();
    if (previousWindow === undefined) {
      Reflect.deleteProperty(globalThis, "window");
    } else {
      Object.defineProperty(globalThis, "window", previousWindow);
    }
  }
});
