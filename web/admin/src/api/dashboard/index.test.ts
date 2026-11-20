// 经真实看板读取入口核对等待领取投影的请求地址。

import { expect, test } from "bun:test";
import { createAdminRequestHarness } from "../api-request-fixture";
import { adminFixtureText } from "../t11-contract-fixture.test";
import { fetchDashboard } from "./index";

test("dashboard_url_requests_the_collection_wait_projection", async () => {
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const requests = harness.installFetch([
      { body: adminFixtureText("dashboard-collection-wait") },
    ]);
    const { host } = harness.registerHost();
    const result = await fetchDashboard(host, new AbortController().signal);
    expect(result.ok).toBe(true);
    expect(requests).toEqual([
      {
        method: "GET",
        url: "/api/v1/admin/dashboard?include=collection_wait",
        body: "",
      },
    ]);
  } finally {
    harness.cleanup();
  }
});
