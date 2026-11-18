// 黑名单页在总量减少时只纠正它自己的页码。

import { expect, test } from "bun:test";
import { mountCompetition, settle, waitFor } from "./commands/competition-fixture.test";
import { success } from "./shell/session-fixture";

test("黑名单自动翻回有效页，保留非分页查询和片段", async () => {
  const page = await mountCompetition("main", true);
  const reads = () => page.reads.filter((path) => path.startsWith("/api/v1/blacklist?"));
  const raw = "keep=%2b%20x&keep=%7e&page_size=%31%30";
  try {
    page.server.readPage = (url) =>
      url.pathname === "/api/v1/blacklist"
        ? success({
            users: [],
            page: Number(url.searchParams.get("page")),
            page_size: Number(url.searchParams.get("page_size")),
            total: 1,
          })
        : null;
    page.go(`/blacklist?${raw}&%70age=2#list`);
    await waitFor(() => reads().length === 2);
    await settle();
    expect(page.location.search).toBe(`?${raw}`);
    expect(page.location.hash).toBe("#list");
    expect(reads()).toEqual([
      "/api/v1/blacklist?page=2&page_size=10",
      "/api/v1/blacklist?page=1&page_size=10",
    ]);
  } finally {
    page.dispose();
  }
});
