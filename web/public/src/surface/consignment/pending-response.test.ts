// 写响应的可信拒绝与结果未知边界。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";

import {
  ALICE,
  attemptWrite,
  DETAIL,
  isWrite,
  listing,
  memory,
  mountPrivate,
  NEW,
  OPERATIONS,
  type Operation,
  PENDING_KEY,
  receipt,
  requestBody,
  STORED_OPERATION,
  withdrawReceipt,
} from "./private-fixture.test";
import { failure, fakeFetch, response, STATUS_PATH, settle, status } from "./read-fixture.test";

async function assertWriteRelease(
  operation: Operation,
  pretendReply: (commandId: string) => Response,
  releases: boolean,
  pausesSession = false,
) {
  const storage = memory();
  const transport = fakeFetch((request) => {
    if (request.url.pathname === STATUS_PATH) return response(status(true));
    if (isWrite(request)) return pretendReply(String(requestBody(request).command_id));
    return response(listing("displayed", { allowed_actions: ["reprice", "withdraw"] }));
  });
  const page = await mountPrivate(operation === "register" ? NEW : DETAIL, ALICE, storage);
  try {
    await settle();
    attemptWrite(page, operation);
    const recorded = storage.session.get(PENDING_KEY);
    // v2 撤回在发包前要先动态加载解码模块，请求不是同步发出的。
    await settle();
    const firstId = requestBody(transport.requests.find(isWrite)).command_id;
    const marker = parseJson(recorded ?? "");
    expect(marker).toMatchObject({
      ok: true,
      value: { operation: STORED_OPERATION[operation], command_id: firstId },
    });
    expect(typeof firstId).toBe("string");
    await settle();
    expect(storage.session.get(PENDING_KEY)).toBe(releases ? "null" : recorded);
    if (pausesSession) {
      expect(transport.requests.filter(isWrite)).toHaveLength(1);
      return;
    }
    attemptWrite(page, operation);
    await settle();
    const writes = transport.requests.filter(isWrite);
    expect(writes.map((request) => requestBody(request).command_id)).toHaveLength(releases ? 2 : 1);
    if (releases) expect(requestBody(writes[1]).command_id).not.toBe(firstId);
  } finally {
    page.dispose();
    transport.restore();
  }
}

for (const operation of OPERATIONS) {
  test(`${operation} 成功状态响应头到达后正文断流，保留未决且禁发新 ID`, () =>
    assertWriteRelease(
      operation,
      () =>
        new Response(
          new ReadableStream({
            start(stream) {
              stream.enqueue(new TextEncoder().encode('{"code":'));
              stream.error(new Error("测试正文断流"));
            },
          }),
          { status: operation === "register" ? 202 : 200 },
        ),
      false,
    ));

  test(`${operation} 未知 409 保留未决且禁发新 ID`, () =>
    assertWriteRelease(operation, () => failure(409, "fake_new_rejection"), false));

  test(`${operation} 状态与协议码错配的 429 invalid_price 保留未决且禁发新 ID`, () =>
    assertWriteRelease(operation, () => failure(429, "invalid_price"), false));

  test(`${operation} 400 invalid_price 缺 data 或 request_id，保留未决且禁发新 ID`, async () => {
    for (const code of ["invalid_price", "invalid_listing_request"]) {
      const partial = { code, message: "" };
      for (const body of [partial, { ...partial, data: null }, { ...partial, request_id: null }]) {
        await assertWriteRelease(operation, () => Response.json(body, { status: 400 }), false);
      }
    }
  });

  test(`${operation} 路由拒绝与共享守卫须匹配完整状态和协议码`, async () => {
    const [http, code] =
      operation === "register"
        ? ([409, "fee_changed"] as const)
        : ([409, "stale_version"] as const);
    await assertWriteRelease(operation, () => failure(http, code), true);
    await assertWriteRelease(
      operation,
      () => failure(409, operation === "register" ? "stale_version" : "fee_changed"),
      false,
    );
    // v1 的改价与撤回认 invalid_query；v2 撤回的拒绝表不含它，须保留未决。
    await assertWriteRelease(
      operation,
      () => failure(400, "invalid_query"),
      operation === "reprice",
    );
    for (const [status, guard] of [
      [400, "invalid_expected_user_id"],
      [401, "unauthenticated"],
      [409, "session_changed"],
      [429, "rate_limited"],
    ] as const) {
      await assertWriteRelease(
        operation,
        () => failure(status, guard),
        true,
        status === 401 || guard === "session_changed",
      );
    }
  });

  test(`${operation} 5xx 始终保留未决且禁发新 ID`, async () => {
    for (const [http, code] of [
      [503, "outcome_unknown"],
      [503, "consignment_unavailable"],
      [503, "storage_unavailable"],
      [504, "timeout"],
    ] as const) {
      await assertWriteRelease(operation, () => failure(http, code), false);
    }
  });

  test(`${operation} 舍入为合法版本的原始 JSON 回执保留未决且禁发新 ID`, () => {
    const registration = operation === "register";
    const withdraw = operation === "withdraw";
    const version = registration ? 1 : 4;
    return assertWriteRelease(
      operation,
      (commandId) => {
        const data = withdraw
          ? withdrawReceipt(commandId, version)
          : receipt(commandId, {
              operation,
              listing_version: version,
              disposition: registration ? "registered" : "repriced",
              state: registration ? "resolving" : "displayed",
              price: registration ? 101 : 102,
            });
        const body = JSON.stringify({ code: registration ? "accepted" : "ok", message: "", data });
        return new Response(
          body.replace(
            `"listing_version":${version}`,
            `"listing_version":${version}.00000000000000001`,
          ),
          { status: registration ? 202 : 200 },
        );
      },
      false,
    );
  });
}
