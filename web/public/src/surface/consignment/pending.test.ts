// 未知结果重放、刷新优先查回执、敏感字段和换号隔离。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";

import {
  ALICE,
  acceptedReceipt,
  assertNoSecret,
  BOB,
  COMMAND_PATH,
  EQUIPMENT_URL,
  expectedUser,
  fill,
  ISEKAI_URL,
  isWrite,
  listing,
  memory,
  mountPrivate,
  receipt,
  requestBody,
  submitRegistration,
} from "./private-fixture.test";
import {
  controlEvent,
  failure,
  fakeClock,
  fakeFetch,
  LISTING_A,
  LISTING_B,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";

const NEW = "/consignment/new";
const DETAIL = `/consignment/me/${LISTING_A}`;
const RETRY = "Retry original request";
const REPRICE = "Change asking price";
const WITHDRAW = "Cancel listing";
const PENDING_KEY = `hv.consignment.pending.${ALICE.user_id}`;
const OPERATIONS = ["register", "reprice", "withdraw"] as const;
type Operation = (typeof OPERATIONS)[number];

function attemptWrite(page: Awaited<ReturnType<typeof mountPrivate>>, operation: Operation) {
  if (operation === "register") {
    fill(page);
    submitRegistration(page);
    return;
  }
  const label = operation === "reprice" ? REPRICE : WITHDRAW;
  controlEvent(page.button(label), "onClick");
  const form = page.form(label);
  if (form === null) return;
  if (operation === "reprice") {
    controlEvent(
      page.find((node) => node.attributes.inputmode === "numeric"),
      "onInput",
      "102",
    );
  }
  controlEvent(form, "onSubmit");
}

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
    const firstId = requestBody(transport.requests.find(isWrite)).command_id;
    const marker = parseJson(recorded ?? "");
    expect(marker).toMatchObject({ ok: true, value: { operation, command_id: firstId } });
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
    await assertWriteRelease(
      operation,
      () => failure(400, "invalid_query"),
      operation !== "register",
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
        const data = receipt(commandId, {
          operation,
          listing_version: version,
          disposition: registration ? "registered" : withdraw ? "closed" : "repriced",
          state: registration ? "resolving" : withdraw ? "closed" : "displayed",
          price: registration ? 101 : withdraw ? null : 102,
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

test("已存拒绝回执必须匹配目标，错挂单仍可查询，正确回执才清除未决", async () => {
  const storage = memory();
  let query = 0;
  const transport = fakeFetch((request) => {
    if (isWrite(request)) return failure(503, "outcome_unknown");
    if (request.url.pathname.startsWith(COMMAND_PATH)) {
      query += 1;
      const commandId = request.url.pathname.slice(COMMAND_PATH.length + 1);
      return response({
        code: "stale_version",
        http_status: 409,
        receipt: {
          ...receipt(commandId, {
            operation: "reprice",
            listing_id: query === 1 ? LISTING_B : LISTING_A,
            listing_version: 4,
            state: "displayed",
            price: null,
          }),
          disposition: "rejected",
        },
      });
    }
    return response(listing("displayed", { allowed_actions: ["reprice"] }));
  });
  const page = await mountPrivate(DETAIL, ALICE, storage);
  try {
    await settle();
    attemptWrite(page, "reprice");
    await settle();
    const recorded = storage.session.get(PENDING_KEY);
    expect(recorded).toContain(LISTING_A);
    controlEvent(page.button("Retry"), "onClick");
    await settle();
    expect(query).toBe(1);
    expect(storage.session.get(PENDING_KEY)).toBe(recorded);
    expect(page.button(REPRICE)?.attributes.disabled).toBe(true);
    expect(page.button("Retry")).not.toBeNull();
    attemptWrite(page, "reprice");
    controlEvent(page.button("Retry"), "onClick");
    await settle();
    expect(query).toBe(2);
    expect(storage.session.get(PENDING_KEY)).toBe("null");
    expect(page.button(REPRICE)?.attributes.disabled).toBe(false);
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("登记超时锁住新 ID；刷新查 404 后重新输入，世界和 EID 不符拒绝，原参数重送", async () => {
  const clock = fakeClock();
  const storage = memory();
  let writes = 0;
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH)
      return response({ ...status(true), fee_bps: writes > 0 ? 800 : 500 });
    if (pathname.startsWith(COMMAND_PATH)) return failure(404, "command_not_found");
    if (isWrite(request)) {
      writes += 1;
      if (writes === 1) {
        return new Promise<Response>((_, reject) =>
          request.init?.signal?.addEventListener(
            "abort",
            () => reject(new DOMException("测试超时", "AbortError")),
            { once: true },
          ),
        );
      }
      return acceptedReceipt(receipt(String(requestBody(request).command_id)));
    }
    return response(listing("resolving"));
  });
  let page = await mountPrivate(NEW, ALICE, storage);
  try {
    await settle();
    fill(page);
    assertNoSecret(page);
    submitRegistration(page);
    assertNoSecret(page);
    const recorded = [...storage.session.values()][0];
    if (recorded === undefined) throw new Error("登记发出前缺少会话标记");
    const marker = JSON.parse(recorded);
    const first = transport.requests.find(isWrite);
    expect(marker).toEqual({
      operation: "register",
      command_id: requestBody(first).command_id,
      expected_user_id: ALICE.user_id,
      world_id: "main",
      eid: "42",
      mode: "consigned",
      price: 101,
      expected_fee_bps: 500,
    });
    await clock.advance(10_000);
    expect(page.text()).toContain("timed out");
    expect(page.button("Submit registration")?.attributes.disabled).toBe(true);
    submitRegistration(page);
    expect(transport.requests.filter(isWrite)).toHaveLength(1);

    page.dispose();
    const atReload = transport.requests.length;
    page = await mountPrivate(NEW, ALICE, storage);
    await settle();
    const privateReads = transport.requests
      .slice(atReload)
      .filter(({ url }) => url.pathname !== STATUS_PATH);
    expect(privateReads.map(({ url }) => url.pathname)).toEqual([
      `${COMMAND_PATH}/${marker.command_id}`,
    ]);
    expect(expectedUser(privateReads[0])).toBe(ALICE.user_id);
    expect(page.button(RETRY)?.attributes.disabled).toBe(true);
    expect(page.input("password")?.attributes.value).toBe("");
    assertNoSecret(page);

    for (const mismatch of [ISEKAI_URL, EQUIPMENT_URL.replace("/42/", "/43/")]) {
      controlEvent(page.input("password"), "onInput", mismatch);
      controlEvent(page.form(RETRY), "onSubmit");
      await settle();
      expect(page.text()).toContain("Enter an equipment URL");
      expect(transport.requests.filter(isWrite)).toHaveLength(1);
    }
    controlEvent(page.input("password"), "onInput", EQUIPMENT_URL);
    controlEvent(page.form(RETRY), "onSubmit");
    await settle();
    const attempts = transport.requests.filter(isWrite);
    expect(attempts).toHaveLength(2);
    expect(attempts[0]?.init?.body === attempts[1]?.init?.body).toBe(true);
    expect(expectedUser(attempts[1])).toBe(ALICE.user_id);
    expect(page.location.pathname).toBe(DETAIL);
    expect([...storage.session.values()]).toEqual(["null"]);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
    clock.restore();
  }
});

test.each(["查到成功", "查到原拒绝", "503 后继续查"] as const)("刷新恢复：%s", async (scenario) => {
  const storage = memory();
  let commandId = "";
  let queries = 0;
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return response(status(true));
    if (isWrite(request)) {
      commandId = String(requestBody(request).command_id);
      return failure(503, "outcome_unknown");
    }
    if (pathname.startsWith(COMMAND_PATH)) {
      queries += 1;
      if (scenario === "503 后继续查" && queries === 1)
        return failure(503, "consignment_unavailable");
      if (scenario === "查到原拒绝")
        return response({
          code: "already_open",
          http_status: 409,
          receipt: {
            ...receipt(commandId),
            disposition: "rejected",
            price: null,
            state: null,
            listing_id: null,
            listing_version: null,
          },
        });
      return response({ code: "accepted", http_status: 202, receipt: receipt(commandId) });
    }
    return response(listing("resolving"));
  });
  let page = await mountPrivate(NEW, ALICE, storage);
  try {
    await settle();
    fill(page);
    submitRegistration(page);
    await settle();
    expect(queries).toBe(0);
    page.dispose();
    page = await mountPrivate(NEW, ALICE, storage);
    await settle();
    expect(queries).toBe(1);
    if (scenario === "503 后继续查") {
      expect(page.text()).toContain("service is temporarily unavailable");
      expect(page.button(RETRY)).toBeNull();
      controlEvent(page.button("Retry"), "onClick");
      await settle();
      expect(queries).toBe(2);
    }
    if (scenario === "查到原拒绝") {
      expect(page.location.pathname).toBe(NEW);
      expect(page.text()).toContain("already have an active registration");
      expect(page.button(RETRY)).toBeNull();
    } else {
      expect(page.location.pathname).toBe(DETAIL);
    }
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each(["reprice", "withdraw"] as const)(
  "刷新后 %s 只允许原路径、用户、版本和命令正文",
  async (operation) => {
    const storage = memory();
    const label = operation === "reprice" ? "Change asking price" : "Cancel listing";
    let first = true;
    const transport = fakeFetch((request) => {
      if (request.url.pathname.startsWith(COMMAND_PATH)) return failure(404, "command_not_found");
      if (!isWrite(request))
        return response(listing("displayed", { allowed_actions: [operation] }));
      if (first) {
        first = false;
        return failure(503, "outcome_unknown");
      }
      const command = requestBody(request);
      return response(
        receipt(
          String(command.command_id),
          operation === "reprice"
            ? {
                operation,
                listing_version: 4,
                state: "displayed",
                disposition: "repriced",
                price: 102,
              }
            : {
                operation,
                listing_version: 4,
                state: "closed",
                disposition: "closed",
                price: null,
              },
        ),
      );
    });
    let page = await mountPrivate(DETAIL, ALICE, storage);
    try {
      await settle();
      controlEvent(page.button(label), "onClick");
      if (operation === "reprice") {
        controlEvent(
          page.find((node) => node.tag === "input"),
          "onInput",
          "102",
        );
      }
      controlEvent(page.form(label), "onSubmit");
      await settle();
      expect(transport.requests.filter(isWrite)).toHaveLength(1);
      expect(page.button(label)?.attributes.disabled).toBe(true);
      page.dispose();
      page = await mountPrivate(DETAIL, ALICE, storage);
      await settle();
      expect(page.input("password")).toBeNull();
      expect(page.button(RETRY)?.attributes.disabled).toBe(false);
      controlEvent(page.button(RETRY), "onClick");
      await settle();
      const attempts = transport.requests.filter(isWrite);
      expect(attempts).toHaveLength(2);
      expect(attempts[0]?.url.href === attempts[1]?.url.href).toBe(true);
      expect(attempts[0]?.init?.body === attempts[1]?.init?.body).toBe(true);
      expect(expectedUser(attempts[1])).toBe(ALICE.user_id);
      expect(requestBody(attempts[1]).expected_version).toBe(3);
      expect(page.text()).not.toContain("Checking the result");
    } finally {
      page.dispose();
      transport.restore();
    }
  },
);

test("换号撤下未决入口和链接，取消在途写入，迟到回执不得导航", async () => {
  const request = Promise.withResolvers<Response>();
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? response(status(true)) : request.promise,
  );
  const page = await mountPrivate(NEW);
  try {
    await settle();
    fill(page);
    submitRegistration(page);
    await settle();
    const old = transport.requests.find(isWrite);
    expect(old?.init?.signal?.aborted).toBe(false);
    expect(page.text()).toContain("Checking the result");
    await page.identify(BOB);
    expect(old?.init?.signal?.aborted).toBe(true);
    expect(page.text()).not.toContain("Checking the result");
    expect(page.input("password")?.attributes.value).toBe("");
    expect(page.button("Submit registration")?.attributes.disabled).toBe(true);
    request.resolve(acceptedReceipt(receipt(String(requestBody(old).command_id))));
    await settle();
    expect(page.location.pathname).toBe(NEW);
    expect(
      transport.requests.filter(({ url }) => url.pathname.startsWith(COMMAND_PATH)),
    ).toHaveLength(0);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("换号清除未发送的明文草稿，安全记录仅恢复给原用户", async () => {
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH
      ? response(status(true))
      : failure(
          url.pathname.startsWith(COMMAND_PATH) ? 404 : 503,
          url.pathname.startsWith(COMMAND_PATH) ? "command_not_found" : "outcome_unknown",
        ),
  );
  const page = await mountPrivate(NEW);
  try {
    await settle();
    fill(page);
    const oldInput = page.input("password");
    if (oldInput === null) throw new Error("缺少原账户的链接输入");
    expect(oldInput.attributes.value === EQUIPMENT_URL).toBe(true);
    await page.identify(BOB);
    expect(Reflect.get(oldInput, "value")).toBe("");
    expect(page.input("password")?.attributes.value).toBe("");
    await page.identify(ALICE);
    expect(page.input("password")?.attributes.value).toBe("");
    fill(page);
    submitRegistration(page);
    await settle();
    await page.identify(BOB);
    expect(page.button(RETRY)).toBeNull();
    await page.identify(ALICE);
    expect(page.text()).toContain("Re-enter the original equipment URL");
    const read = transport.requests.find(({ url }) => url.pathname.startsWith(COMMAND_PATH));
    expect(expectedUser(read)).toBe(ALICE.user_id);
    assertNoSecret(page);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("安全标记取不到写入权限或已有损坏记录时，失败关闭且不发新命令", async () => {
  const storage = memory();
  const transport = fakeFetch(() => response(status(true)));
  let page = await mountPrivate(NEW, ALICE, storage);
  try {
    await settle();
    const persist = storage.session.set.bind(storage.session);
    storage.session.set = () => {
      throw new Error("测试会话存储不可写");
    };
    fill(page);
    submitRegistration(page);
    await settle();
    expect(page.text()).toContain("Auction data is temporarily unavailable");
    expect(transport.requests.filter(isWrite)).toHaveLength(0);

    const key = `hv.consignment.pending.${ALICE.user_id}`;
    persist(key, "{}");
    storage.session.set = persist;
    page.dispose();
    page = await mountPrivate(NEW, ALICE, storage);
    await settle();
    fill(page);
    expect(page.button("Submit registration")?.attributes.disabled).toBe(true);
    submitRegistration(page);
    await settle();
    expect(transport.requests.filter(isWrite)).toHaveLength(0);
    expect(storage.session.get(key)).toBe("{}");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("回执 404 后仍可再查，操作不符的回执不解除未决", async () => {
  let commandId = "";
  let checks = 0;
  const transport = fakeFetch((request) => {
    if (request.url.pathname === STATUS_PATH) return response(status(true));
    if (isWrite(request)) {
      commandId = String(requestBody(request).command_id);
      return failure(503, "outcome_unknown");
    }
    if (request.url.pathname.startsWith(COMMAND_PATH)) {
      checks += 1;
      if (checks === 1) return failure(404, "command_not_found");
      if (checks === 2)
        return response({
          code: "ok",
          http_status: 200,
          receipt: receipt(commandId, {
            operation: "reprice",
            state: "displayed",
            disposition: "repriced",
            listing_version: 4,
          }),
        });
      return response({ code: "accepted", http_status: 202, receipt: receipt(commandId) });
    }
    return response(listing("resolving"));
  });
  const page = await mountPrivate(NEW);
  try {
    await settle();
    fill(page);
    submitRegistration(page);
    await settle();
    controlEvent(page.button("Retry"), "onClick");
    await settle();
    expect(page.text()).toContain("Re-enter the original equipment URL");
    controlEvent(page.button("Retry"), "onClick");
    await settle();
    expect(page.location.pathname).toBe(NEW);
    expect(page.button("Submit registration")?.attributes.disabled).toBe(true);
    controlEvent(page.button("Retry"), "onClick");
    await settle();
    expect(checks).toBe(3);
    expect(page.location.pathname).toBe(DETAIL);
    expect(transport.requests.filter(isWrite)).toHaveLength(1);
  } finally {
    page.dispose();
    transport.restore();
  }
});
