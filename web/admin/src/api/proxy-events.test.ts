// 对照 Rust 代理历史的页码和游标回执。

import { expect, test } from "bun:test";
import { AdminProxyEventListResponseSchema } from "@auction/contracts/dto/admin/proxy";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { fixtureText, LOT_ID, requestFixture, success } from "./auction-contract-fixture.test";
import { fetchAdminProxyEvents } from "./proxy-events";

function readPage(body: string, beacons = 0) {
  return requestFixture(
    [{ body }],
    [["GET", `/api/v1/admin/lots/${LOT_ID}/proxy-events?page=1&page_size=50`]],
    (host, signal) => fetchAdminProxyEvents(host, LOT_ID, 1, 50, signal),
    beacons,
  );
}

test("proxy_events_decode_backend_contract_fixtures", async () => {
  const schema = apiResponseSchema(AdminProxyEventListResponseSchema);
  for (const [name, mode] of [
    ["proxy-events", "slow"],
    ["proxy-events-final", "fast"],
  ] as const) {
    const body = fixtureText(name);
    success(decode(schema, success(parseJson(body))));
    const page = success(await readPage(body));
    expect(page.proxy_events[0]).toMatchObject({ status: "active", mode });
  }
});

test("proxy_cursor_wire_stays_distinct_from_browser_numbered_pages", async () => {
  const body = fixtureText("proxy-events-cursor");
  const schema = apiResponseSchema(AdminProxyEventListResponseSchema);
  const { data } = success(decode(schema, success(parseJson(body))));
  expect(data.pagination.cursor).toBe("start");
  expect(await readPage(body, 1)).toEqual({ ok: false, error: "invalid_response" });
});
