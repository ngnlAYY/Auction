// 用 Rust 契约的共享夹具核对运营订单列表、详情与证据分页的解码与身份核对。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

import { createAdminRequestHarness } from "../api-request-fixture";
import { fetchAdminOrder, fetchAdminOrderEvidence, fetchAdminOrders, ORDERS_URL } from "./orders";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const SIGNAL = () => new AbortController().signal;
const NO_FILTERS = { world: "", state: "" };

function fixture(name: string): string {
  return readFileSync(new URL(name, FIXTURES), "utf8");
}

interface Envelope {
  code: string;
  message: string;
  data: { contract_id: string; world: string };
}

function orderEnvelope(): Envelope {
  return JSON.parse(fixture("consignment-order-v2.json"));
}

function pageBody(): string {
  const { data, ...rest } = orderEnvelope();
  return JSON.stringify({ ...rest, data: { items: [data], total: 1, page: 1, page_size: 20 } });
}

test("共享夹具的运营订单列表与详情解码，请求目标正确", async () => {
  const envelope = orderEnvelope();
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const requests = harness.installFetch([
      { body: pageBody() },
      { body: JSON.stringify(envelope) },
    ]);
    const list = await fetchAdminOrders(host, NO_FILTERS, 1n, 20, SIGNAL());
    expect(list.ok).toBe(true);
    const one = await fetchAdminOrder(host, envelope.data.contract_id, SIGNAL());
    expect(one.ok).toBe(true);
    expect(requests[0]?.url.startsWith(`${ORDERS_URL}?`)).toBe(true);
    expect(requests[1]?.url).toBe(`${ORDERS_URL}/${envelope.data.contract_id}`);
  } finally {
    harness.cleanup();
  }
});

test("列表回显了筛选之外的世界，或详情返回别的合同，都判响应无效", async () => {
  const envelope = orderEnvelope();
  const otherWorld = envelope.data.world === "main" ? "isekai" : "main";
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    harness.installFetch([{ body: pageBody() }, { body: JSON.stringify(envelope) }]);
    const filtered = await fetchAdminOrders(
      host,
      { ...NO_FILTERS, world: otherWorld },
      1n,
      20,
      SIGNAL(),
    );
    expect(filtered).toEqual({ ok: false, error: "invalid_response" });
    const other = await fetchAdminOrder(host, "00000000-0000-4000-8000-0000000000ee", SIGNAL());
    expect(other).toEqual({ ok: false, error: "invalid_response" });
  } finally {
    harness.cleanup();
  }
});

test("订单证据分页解码，页码与页长必须原样回显", async () => {
  const body = fixture("consignment-order-v2-evidence.json");
  const { data } = JSON.parse(body) as { data: { page: number; page_size: number } };
  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const requests = harness.installFetch([{ body }, { body }]);
    const contractId = orderEnvelope().data.contract_id;
    const ok = await fetchAdminOrderEvidence(host, contractId, data.page, data.page_size, SIGNAL());
    expect(ok.ok).toBe(true);
    const mismatch = await fetchAdminOrderEvidence(
      host,
      contractId,
      data.page + 1,
      data.page_size,
      SIGNAL(),
    );
    expect(mismatch).toEqual({ ok: false, error: "invalid_response" });
    expect(requests[0]?.url.startsWith(`${ORDERS_URL}/${contractId}/evidence?`)).toBe(true);
  } finally {
    harness.cleanup();
  }
});
