// 重新上架：入口只按 allowed_actions 出现；结果未知的命令只能原样重放同一记录。

import { expect, test } from "bun:test";
import { createComponent } from "solid-js";

import {
  ALICE,
  DETAIL,
  isWrite,
  listing,
  mountPrivate,
  OWNED_PATH,
  requestBody,
} from "./private-fixture.test";
import {
  controlEvent,
  detail,
  failure,
  fakeFetch,
  LISTING_A,
  type RequestRecord,
  response,
  settle,
} from "./read-fixture.test";
import { loadPages, mountPage } from "./rights-fixture.test";

const RELIST_PATH = `/api/v2/consignment/listings/${LISTING_A}/relist`;
const RECEIPTS = "/api/v2/me/consignment/commands/";
const RELIST = "Relist";
const RETRY = "Retry original request";
const PENDING_KEY = `hv.consignment.relist.pending.${ALICE.user_id}`;

/** 未付款收回后留在代管中的件：服务端据此给出 relist。 */
function recovered(actions: readonly ("relist" | "withdraw")[]) {
  return listing("held_off_market", {
    effective_reason: "unpaid_recovered",
    view: {
      kind: "held_off_market",
      equipment: detail().equipment,
      reason: "unpaid_recovered",
      hold_id: null,
    },
    allowed_actions: [...actions],
  });
}

function relisted(request: RequestRecord): Response {
  const command = requestBody(request);
  return response({
    schema_version: 2,
    operation: "relist",
    disposition: "relisted",
    command_id: command.command_id,
    listing_id: LISTING_A,
    listing_version: Number(command.expected_version) + 1,
    state: "available",
    price: 101,
    generation: 2,
  });
}

test.each([
  [["relist"], true],
  [["withdraw"], false],
  [[], false],
] as const)(
  "本人详情的重列入口只在 allowed_actions 含 relist 时出现：%j",
  async (actions, shown) => {
    const transport = fakeFetch(() => response(recovered(actions)));
    const page = await mountPrivate(DETAIL);
    try {
      await settle();
      expect(
        page.find((node) => node.attributes.href === `/consignment/me/${LISTING_A}/relist`) !==
          null,
      ).toBe(shown);
      expect(transport.requests.filter(isWrite)).toHaveLength(0);
    } finally {
      page.dispose();
      transport.restore();
    }
  },
);

async function openRelist(storage?: Map<string, string>) {
  const { RelistPage } = await loadPages();
  return mountPage(
    `/consignment/me/${LISTING_A}/relist`,
    () => createComponent(RelistPage, { params: { listing_id: LISTING_A }, query: {} }),
    ALICE,
    storage,
  );
}

test("服务端未给出 relist 时页面没有提交按钮，也不发写请求", async () => {
  const transport = fakeFetch(({ url }) =>
    url.pathname === `${OWNED_PATH}/${LISTING_A}` ? response(recovered([])) : failure(404, "x"),
  );
  const page = await openRelist();
  try {
    await settle();
    expect(page.button(RELIST)).toBeNull();
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("重列遇 503 结果未知：保留原记录，只原样重放同一命令，成功后清除", async () => {
  let writes = 0;
  const transport = fakeFetch((request) => {
    // 回执查询 404 只代表当次未见，才放行原样重放（D9 §6.3）。
    if (request.url.pathname.startsWith(RECEIPTS)) return failure(404, "command_not_found");
    if (!isWrite(request)) return response(recovered(["relist"]));
    writes += 1;
    return writes === 1 ? failure(503, "outcome_unknown") : relisted(request);
  });
  const page = await openRelist();
  try {
    await settle();
    controlEvent(page.button(RELIST), "onClick");
    await settle();
    const first = requestBody(transport.requests.find(isWrite));
    expect(transport.requests.find(isWrite)?.url.pathname).toBe(RELIST_PATH);
    expect(first.expected_version).toBe(3);
    expect(JSON.parse(page.storage.get(PENDING_KEY) ?? "null").command_id).toBe(first.command_id);
    // 未决期间不能再发新命令。
    expect(page.button(RELIST)?.attributes.disabled).toBe(true);
    controlEvent(page.button(RETRY), "onClick");
    await settle();
    const sent = transport.requests.filter(isWrite);
    expect(sent).toHaveLength(2);
    expect(requestBody(sent[1])).toEqual(first);
    expect(page.text()).toContain("The item is on sale again.");
    expect(page.storage.get(PENDING_KEY)).toBe("null");
  } finally {
    page.dispose();
    transport.restore();
  }
});

async function afterUnknown(replyToRetry: (request: RequestRecord) => Response) {
  let writes = 0;
  const transport = fakeFetch((request) => {
    // 回执查询 404 只代表当次未见，才放行原样重放（D9 §6.3）。
    if (request.url.pathname.startsWith(RECEIPTS)) return failure(404, "command_not_found");
    if (!isWrite(request)) return response(recovered(["relist"]));
    writes += 1;
    return writes === 1 ? failure(503, "outcome_unknown") : replyToRetry(request);
  });
  const page = await openRelist();
  await settle();
  controlEvent(page.button(RELIST), "onClick");
  await settle();
  const first = requestBody(transport.requests.find(isWrite));
  controlEvent(page.button(RETRY), "onClick");
  await settle();
  return { page, transport, first };
}

test.each([
  [409, "session_changed"],
  [401, "unauthenticated"],
] as const)("未决重放遇 %i %s：记录保留，再次重放仍是同一 command_id", async (status, code) => {
  let guarded = true;
  const { page, transport, first } = await afterUnknown((request) =>
    guarded ? failure(status, code) : relisted(request),
  );
  try {
    expect(JSON.parse(page.storage.get(PENDING_KEY) ?? "null").command_id).toBe(first.command_id);
    guarded = false;
    controlEvent(page.button(RETRY), "onClick");
    await settle();
    const sent = transport.requests.filter(isWrite);
    expect(sent.map(requestBody).map(({ command_id }) => command_id)).toEqual([
      first.command_id,
      first.command_id,
      first.command_id,
    ]);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("200 回执的 command_id 不符按 invalid_response：记录保留，不当作成功", async () => {
  const { page, transport, first } = await afterUnknown((request) => {
    const reply = relisted(request);
    return reply.clone().status === 200
      ? Response.json({
          code: "ok",
          message: "ok",
          data: {
            schema_version: 2,
            operation: "relist",
            disposition: "relisted",
            command_id: "00000000-0000-4000-8000-0000000000ff",
            listing_id: LISTING_A,
            listing_version: 4,
            state: "available",
            price: 101,
            generation: 2,
          },
        })
      : reply;
  });
  try {
    expect(page.text()).not.toContain("The item is on sale again.");
    expect(JSON.parse(page.storage.get(PENDING_KEY) ?? "null").command_id).toBe(first.command_id);
  } finally {
    page.dispose();
    transport.restore();
  }
});
