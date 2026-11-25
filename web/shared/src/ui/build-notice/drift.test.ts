// 构建漂移检测的回归用例，逐条对照 Rust `ui::build_notice` 的测试。

import { expect, test } from "bun:test";

Object.defineProperty(globalThis, "__AUCTION_BUILD_ID__", {
  configurable: true,
  value: "test-build",
});
Object.defineProperty(globalThis, "__AUCTION_PACKAGE_VERSION__", {
  configurable: true,
  value: "0.0.0",
});

const { buildId } = await import("../bootstrap");
const {
  SERVER_BUILD_HEADER,
  SERVER_INDEPENDENT_BUILD_HEADER,
  buildIsStale,
  buildReportedToSolid,
  observeServerBuildId,
  resetBuildDriftForTests,
  responseReportsCurrentBuild,
  waitForBuildDrift,
} = await import("./drift");

type BuildDriftWait = ReturnType<typeof waitForBuildDrift>;

async function poll(wait: BuildDriftWait): Promise<"ready" | "pending"> {
  let settled = false;
  void wait.promise.then(() => {
    settled = true;
  });
  await Promise.resolve();
  return settled ? "ready" : "pending";
}

test("only_no_store_responses_report_the_current_build：只有 no-store 响应可采信构建号", () => {
  expect(responseReportsCurrentBuild("no-store")).toBe(true);
  expect(responseReportsCurrentBuild("private, No-Store")).toBe(true);
  for (const replayable of [
    "no-cache",
    "public, max-age=300, s-maxage=300, stale-while-revalidate=3600",
    "public, max-age=31536000, immutable",
    "private, max-age=2, stale-while-revalidate=5",
    "",
  ]) {
    expect(responseReportsCurrentBuild(replayable)).toBe(false);
  }
  expect(responseReportsCurrentBuild(null)).toBe(false);
});

test("build_drift_notice_tracks_the_first_mismatch：首次发现构建号偏差后唤醒等待者并保持状态", async () => {
  resetBuildDriftForTests();
  expect(SERVER_BUILD_HEADER).toBe("x-auction-build");

  expect(observeServerBuildId(buildId())).toBe(false);
  expect(buildIsStale()).toBe(false);
  {
    const wait = waitForBuildDrift();
    expect(await poll(wait)).toBe("pending");
    wait.cancel();
  }

  const wait = waitForBuildDrift();
  expect(await poll(wait)).toBe("pending");
  expect(observeServerBuildId("build_other")).toBe(true);
  expect(buildIsStale()).toBe(true);
  expect(await poll(wait)).toBe("ready");
  expect(await poll(waitForBuildDrift())).toBe("ready");
  expect(observeServerBuildId("build_other")).toBe(true);
  expect(buildIsStale()).toBe(true);
  resetBuildDriftForTests();
  expect(buildIsStale()).toBe(false);
  const pending = waitForBuildDrift();
  expect(await poll(pending)).toBe("pending");
  resetBuildDriftForTests();
});

test("solid_compares_the_independent_build_header：回切旧壳时指针 build 不令 Solid 误提示", () => {
  resetBuildDriftForTests();
  expect(SERVER_INDEPENDENT_BUILD_HEADER).toBe("x-auction-build-independent");

  // 正例：通用头报告旧壳 build，独立头仍是 Solid 自身构建号，不偏差。
  const quiet = buildReportedToSolid("build_legacy_shell", buildId());
  expect(quiet).toBe(buildId());
  expect(observeServerBuildId(quiet ?? "")).toBe(false);
  expect(buildIsStale()).toBe(false);

  // 独立头缺失（未启用指针）时退回通用头，真实部署漂移仍然提示。
  expect(buildReportedToSolid("build_other", null)).toBe("build_other");
  // 反例：独立头本身与自身构建号不同，才是新的独立入口 build，必须提示。
  expect(observeServerBuildId(buildReportedToSolid("build_other", "build_next") ?? "")).toBe(true);
  expect(buildIsStale()).toBe(true);
  // 两个头都缺失时没有可比较的值。
  expect(buildReportedToSolid(null, null)).toBeNull();
  resetBuildDriftForTests();
});
