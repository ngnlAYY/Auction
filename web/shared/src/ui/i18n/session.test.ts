// 语言会话的翻译失败处理，逐条对照 Rust `ui::i18n` 的测试。

import { expect, test } from "bun:test";

Object.defineProperty(globalThis, "__AUCTION_BUILD_ID__", {
  configurable: true,
  value: "test-build",
});
Object.defineProperty(globalThis, "__AUCTION_PACKAGE_VERSION__", {
  configurable: true,
  value: "0.0.0",
});

const { translationFailed } = await import("./session");

test("translation_failures_panic_in_debug_builds：调试构建下翻译失败抛出错误", () => {
  const previousDev = process.env.DEV;
  process.env.DEV = "true";
  try {
    expect(() => translationFailed("missing-key", "unknown variable")).toThrow(
      "翻译 missing-key 失败",
    );
  } finally {
    if (previousDev === undefined) {
      delete process.env.DEV;
    } else {
      process.env.DEV = previousDev;
    }
  }
});
