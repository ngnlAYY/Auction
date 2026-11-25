// 受限权利页：定位符只进请求正文、未知写只能原样重放、会话失效回到验权。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createComponent } from "solid-js";

import {
  controlEvent,
  failure,
  fakeFetch,
  type RequestRecord,
  response,
} from "./read-fixture.test";
import { loadPages, mountPage, settle } from "./rights-fixture.test";

const FIXTURES = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/public/",
  import.meta.url,
);
const RIGHTS = "/api/v2/consignment/rights";
// 明显的假值：定位符与挑战码都不对应任何真实权利。
const LOCATOR = "R1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const CODE = "FIXTV-RE000-00000-00000-00000";
const CLAIM_KEY = "hv.consignment.right.claim";

function fixture(name: string): { data: unknown } {
  return JSON.parse(readFileSync(new URL(`consignment-rights-${name}.json`, FIXTURES), "utf8"));
}

function accepted(data: unknown, status = 202): Response {
  return Response.json({ code: "accepted", message: "", data }, { status });
}

function body(request: RequestRecord | undefined): Record<string, unknown> {
  return JSON.parse(String(request?.init?.body));
}

function posts(requests: readonly RequestRecord[], suffix: string): RequestRecord[] {
  return requests.filter(
    ({ url, init }) => init?.method === "POST" && url.pathname === `${RIGHTS}${suffix}`,
  );
}

async function openPage() {
  const { RightsPage } = await loadPages();
  return mountPage("/consignment/rights", () => createComponent(RightsPage, {}), null);
}

type Page = Awaited<ReturnType<typeof openPage>>;

/** 走完“定位符 → 挑战码 → 视图”，停在可办理的权利视图。 */
async function verified(page: Page) {
  controlEvent(page.input("password"), "onInput", LOCATOR);
  controlEvent(page.form("Send verification code"), "onSubmit");
  await settle();
  controlEvent(page.input("password"), "onInput", CODE);
  controlEvent(page.form("Verify"), "onSubmit");
  await settle();
}

function server(claim: (request: RequestRecord) => Response | Promise<Response>) {
  return fakeFetch((request) => {
    switch (request.url.pathname.slice(RIGHTS.length)) {
      case "/challenges":
        return accepted(fixture("challenge-accepted").data);
      case "/verify":
        return response(fixture("verified").data);
      case "/current":
        return response(fixture("view").data);
      case "/current/claim":
        return claim(request);
      default:
        return failure(404, "command_not_found");
    }
  });
}

test("定位符与挑战码只进请求正文；提交后输入清空，页面与存储都不留", async () => {
  const transport = server(() => failure(503, "outcome_unknown"));
  const page = await openPage();
  try {
    await verified(page);
    const [challenge] = posts(transport.requests, "/challenges");
    const [verify] = posts(transport.requests, "/verify");
    expect(body(challenge).right_locator).toBe(LOCATOR);
    expect(body(verify).code).toBe(CODE);
    for (const { url, init } of transport.requests) {
      expect(url.href).not.toContain(LOCATOR);
      expect(url.href).not.toContain(CODE);
      expect(new Headers(init?.headers).has("x-expected-user-id")).toBe(false);
    }
    expect(page.text()).not.toContain(LOCATOR);
    expect(page.text()).not.toContain(CODE);
    expect(page.storage.size).toBe(0);
    // 视图只来自服务端的权利视图：种类与动作，没有邮件正文。
    expect(page.text()).toContain("Payment claim");
    expect(page.button("Claim payment")).not.toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["503 outcome_unknown", () => failure(503, "outcome_unknown")],
  ["504 超时", () => failure(504, "timeout")],
  [
    "断线",
    () => {
      throw new TypeError("network down");
    },
  ],
] as const)("续办遇%s：结果未知，只能原样重放同一命令，不生成新命令", async (_name, unknown) => {
  let calls = 0;
  const transport = server((request) => {
    calls += 1;
    return calls === 1
      ? unknown()
      : accepted({
          command_id: body(request).command_id,
          right_id: (fixture("claim-receipt").data as { right_id: string }).right_id,
          disposition: "claim_accepted",
        });
  });
  const page = await openPage();
  try {
    await verified(page);
    controlEvent(page.button("Claim payment"), "onClick");
    await settle();
    const first = body(posts(transport.requests, "/current/claim")[0]);
    // 记录只含命令、权利、版本与动作，不含任何秘密。
    expect(Object.keys(JSON.parse(page.storage.get(CLAIM_KEY) ?? "{}")).sort()).toEqual([
      "action",
      "command_id",
      "expected_version",
      "right_id",
    ]);
    // 未知结果期间不能再发起新的续办。
    expect(page.button("Claim payment")?.attributes.disabled).toBe(true);
    controlEvent(page.button("Retry original request"), "onClick");
    await settle();
    const claims = posts(transport.requests, "/current/claim");
    expect(claims).toHaveLength(2);
    expect(body(claims[1])).toEqual(first);
    expect(page.text()).toContain("Your request was accepted.");
    expect(page.storage.get(CLAIM_KEY)).toBe("null");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("会话失效（401）回到验权：视图消失、输入验证码的表单不再保留，且无令牌痕迹", async () => {
  const transport = server(() => failure(401, "unauthenticated"));
  const page = await openPage();
  try {
    await verified(page);
    controlEvent(page.button("Claim payment"), "onClick");
    await settle();
    expect(page.text()).toContain("Your verification has ended");
    expect(page.button("Claim payment")).toBeNull();
    expect(page.form("Send verification code")).not.toBeNull();
    expect(page.form("Verify")).toBeNull();
    const stored = page.storage.get(CLAIM_KEY) ?? "";
    expect(stored).not.toMatch(/token|hv_right|cookie/iu);
    expect(page.text()).not.toMatch(/token|hv_right/iu);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["challenge", "Send verification code", "/challenges"],
  ["verify", "Verify", "/verify"],
] as const)(
  "%s 遇 503 outcome_unknown：只重放同一 command_id 与同一正文",
  async (step, submit, suffix) => {
    let seen = 0;
    const transport = fakeFetch((request) => {
      const path = request.url.pathname.slice(RIGHTS.length);
      if (path === "/challenges" || path === "/verify") {
        if (path === suffix) seen += 1;
        if (path === suffix && seen === 1) return failure(503, "outcome_unknown");
        return path === "/verify"
          ? response(fixture("verified").data)
          : accepted(fixture("challenge-accepted").data);
      }
      return path === "/current"
        ? response(fixture("view").data)
        : failure(404, "command_not_found");
    });
    const page = await openPage();
    try {
      controlEvent(page.input("password"), "onInput", LOCATOR);
      controlEvent(page.form("Send verification code"), "onSubmit");
      await settle();
      if (step === "verify") {
        controlEvent(page.input("password"), "onInput", CODE);
        controlEvent(page.form(submit), "onSubmit");
        await settle();
      }
      const first = body(posts(transport.requests, suffix)[0]);
      controlEvent(page.button("Retry original request"), "onClick");
      await settle();
      const sent = posts(transport.requests, suffix);
      expect(sent).toHaveLength(2);
      expect(body(sent[1])).toEqual(first);
    } finally {
      page.dispose();
      transport.restore();
    }
  },
);
