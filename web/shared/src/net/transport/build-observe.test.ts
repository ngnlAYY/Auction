// transport 收到响应时的构建号接线回归：Solid 页面只比独立入口头，缺失时才退回通用头。

import { afterEach, expect, test } from "bun:test";

Object.defineProperty(globalThis, "__AUCTION_BUILD_ID__", {
  configurable: true,
  value: "solid-build",
});
Object.defineProperty(globalThis, "__AUCTION_PACKAGE_VERSION__", {
  configurable: true,
  value: "0.0.0",
});

const { buildId } = await import("../../ui/bootstrap");
const { buildIsStale, resetBuildDriftForTests } = await import("../../ui/build-notice/drift");
const { sendGet } = await import("./index");

// 同进程里其他测试文件可能已先加载 bootstrap，自身构建号以实际值为准。
const ownBuild = buildId();
const realFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = realFetch;
  resetBuildDriftForTests();
});

async function respondWith(headers: Record<string, string>): Promise<void> {
  globalThis.fetch = (async () =>
    new Response("{}", { status: 200, headers })) as unknown as typeof fetch;
  const result = await sendGet("/api/v1/probe");
  expect(result.ok).toBe(true);
}

test("legacy_shell_build_does_not_flag_drift_when_independent_header_matches：旧壳通用头不误报", async () => {
  await respondWith({
    "cache-control": "no-store",
    "x-auction-build": "legacy-shell-build",
    "x-auction-build-independent": ownBuild,
  });
  expect(buildIsStale()).toBe(false);
});

test("independent_header_mismatch_flags_drift：独立头不同仍提示", async () => {
  await respondWith({
    "cache-control": "no-store",
    "x-auction-build": ownBuild,
    "x-auction-build-independent": `${ownBuild}-newer`,
  });
  expect(buildIsStale()).toBe(true);
});

test("generic_header_alone_behaves_as_before：只有通用头时按通用头判定", async () => {
  await respondWith({ "cache-control": "no-store", "x-auction-build": ownBuild });
  expect(buildIsStale()).toBe(false);
  await respondWith({ "cache-control": "no-store", "x-auction-build": `${ownBuild}-other` });
  expect(buildIsStale()).toBe(true);
});
