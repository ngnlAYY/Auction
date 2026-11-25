// 第 6、7 期寄售页面的跨页验收（8B-c，D9 §6.3）：只核对，不改页面。
// 正例：订单、购买、重列、权利、损失回执与 Admin 订单 / 证据 / 损失页在直达与刷新下解析到登记页面，
// 页面引用的译文键五种语言齐备。反例：503 outcome_unknown 只允许原样重放（正文逐字节相同）、
// 受限页与本人购买 / 重列 / 预约 / 撤回先查回执再给重放、非本人 404 只显示未找到；
// 本人详情只在服务端宣告 relist 时给出重列入口。
// 刷新时服务端对同一路径返回 SPA 入口，由 entry-independence.test.ts 锁定的
// routes.rs 与 Solid 路由表同集合保证，这里不重复解析 routes.rs。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import type { RouteDefinition, RoutePage } from "@auction/web-shared/router";
import { compileRoutes, resolveRoute } from "@auction/web-shared/router/pattern";
import type { LocaleCode } from "@auction/web-shared/ui/i18n";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";
import { createComponent } from "solid-js";

import { createAdminRequestHarness, installWriteFetch } from "../admin/src/api/api-request-fixture";
import {
  prepareAdminConfirmLoss,
  prepareAdminLossOutcome,
} from "../admin/src/api/consignment/loss";
import { prepareAdminOrderResolve } from "../admin/src/api/consignment/order-resolve";
import { type AdminWriteFailure, classifyAdminWriteFailure } from "../admin/src/api/failure";
import { failure as adminFailure } from "../admin/src/surface/consignment/fixtures";
import { lossCasePath } from "../admin/src/surface/consignment/position-loss";
import { ADMIN_PAGES } from "../admin/src/surface/router";
import { ADMIN_ROUTES } from "../admin/src/surface/router/route";
import type { CommandError } from "../public/src/api/commands";
import {
  purchaseConsignmentListing,
  purchaseWriteRejected,
} from "../public/src/api/consignment-orders";
import { relistConsignmentListing, relistRejected } from "../public/src/api/consignment-relist";
import { claimRight, rightWriteRejected } from "../public/src/api/consignment-rights";
import { detail as purchasableDetail } from "../public/src/surface/consignment/order-fixture.test";
import { orderHref } from "../public/src/surface/consignment/order-labels";
import {
  OWNED_PATH,
  listing as ownedListing,
  VIEWS,
} from "../public/src/surface/consignment/private-fixture.test";
import {
  controlEvent,
  failure,
  fakeFetch,
  type RequestRecord,
  response,
  STATUS_PATH,
  status,
} from "../public/src/surface/consignment/read-fixture.test";
import {
  ALICE,
  loadPages,
  mountPage,
  settle,
} from "../public/src/surface/consignment/rights-fixture.test";
import { PUBLIC_PAGES, PUBLIC_ROUTES } from "../public/src/surface/router/route";

const webRoot = resolve(import.meta.dir, "..");
const repositoryRoot = resolve(webRoot, "..");
const LOCALES: readonly LocaleCode[] = ["en", "zh-CN", "zh-TW", "ja", "ko"];
const SIGNAL = () => new AbortController().signal;

// 明显的假标识；不对应任何真实挂单、合同或权利。
const LISTING = "00000000-0000-4000-8000-0000000000a1";
const CONTRACT = "00000000-0000-4000-8000-000000000501";
const POSITION = "00000000-0000-4000-8000-0000000000c3";
const COMMAND = "00000000-0000-4000-8000-000000000510";
const OTHER_COMMAND = "00000000-0000-4000-8000-000000000511";
const EVIDENCE = "00000000-0000-4000-8000-000000000520";

type Surface = "public" | "admin";

/** 一张第 6、7 期页面：直达地址、应解析到的路由与参数、页面源文件及其导出的入口组件。 */
interface PageCase {
  readonly surface: Surface;
  readonly label: string;
  readonly url: string;
  readonly route: string;
  readonly params: Readonly<Record<string, string>>;
  readonly source: string;
  readonly component: string;
}

const PUBLIC_DIR = "public/src/surface/consignment";
const ADMIN_DIR = "admin/src/surface/consignment";

const PAGES: readonly PageCase[] = [
  {
    surface: "public",
    label: "6E 购买确认",
    url: `/consignment/listings/${LISTING}/purchase`,
    route: "ConsignmentPurchase",
    params: { listing_id: LISTING },
    source: `${PUBLIC_DIR}/purchase-confirm.tsx`,
    component: "PurchaseConfirm",
  },
  {
    surface: "public",
    label: "6E 本人订单",
    url: "/consignment/orders",
    route: "ConsignmentOrders",
    params: {},
    source: `${PUBLIC_DIR}/orders.tsx`,
    component: "MyOrders",
  },
  {
    surface: "public",
    label: "6E 本人订单详情",
    url: orderHref(CONTRACT),
    route: "ConsignmentOrderDetail",
    params: { contract_id: CONTRACT },
    source: `${PUBLIC_DIR}/order-detail.tsx`,
    component: "OrderDetail",
  },
  {
    surface: "public",
    label: "7D 受限权利",
    url: "/consignment/rights",
    route: "ConsignmentRights",
    params: {},
    source: `${PUBLIC_DIR}/rights.tsx`,
    component: "RightsPage",
  },
  {
    surface: "public",
    label: "7D 损失回执",
    url: `/consignment/loss/${POSITION}`,
    route: "ConsignmentLossReceipt",
    params: { position_id: POSITION },
    source: `${PUBLIC_DIR}/loss-receipt.tsx`,
    component: "LossReceiptPage",
  },
  {
    surface: "public",
    label: "7D 重列",
    url: `/consignment/me/${LISTING}/relist`,
    route: "ConsignmentRelist",
    params: { listing_id: LISTING },
    source: `${PUBLIC_DIR}/relist.tsx`,
    component: "RelistPage",
  },
  {
    surface: "admin",
    label: "6E 运营订单",
    url: "/consignment/orders",
    route: "ConsignmentOrders",
    params: {},
    source: `${ADMIN_DIR}/orders.tsx`,
    component: "page",
  },
  {
    surface: "admin",
    label: "7E 订单详情与证据",
    url: `/consignment/orders/${CONTRACT}`,
    route: "ConsignmentOrderDetail",
    params: { contract_id: CONTRACT },
    source: `${ADMIN_DIR}/order-detail.tsx`,
    component: "detailPage",
  },
  {
    surface: "admin",
    label: "7E 损失案件",
    url: lossCasePath(POSITION),
    route: "ConsignmentLoss",
    params: { position_id: POSITION },
    source: `${ADMIN_DIR}/loss.tsx`,
    component: "lossCasePage",
  },
  {
    surface: "admin",
    label: "PIWU 挂单详情的灭失办理入口",
    url: `/consignment/listings/${LISTING}`,
    route: "ConsignmentListing",
    params: { listing_id: LISTING },
    source: `${ADMIN_DIR}/detail.tsx`,
    component: "detailPage",
  },
];

const SURFACES: Readonly<
  Record<
    Surface,
    {
      readonly routes: readonly RouteDefinition[];
      readonly pages: Readonly<Record<string, RoutePage>>;
      readonly i18n: string;
    }
  >
> = {
  public: { routes: PUBLIC_ROUTES, pages: PUBLIC_PAGES, i18n: "crates/auction-web-public/i18n" },
  admin: { routes: ADMIN_ROUTES, pages: ADMIN_PAGES, i18n: "crates/auction-web-admin/i18n" },
};

test.each(PAGES.map((page) => [page.label, page] as const))(
  "直达与刷新：%s 解析到登记页面，分块加载出页面源文件的入口组件",
  async (_label, page) => {
    const surface = SURFACES[page.surface];
    const routes = compileRoutes(surface.routes);
    const direct = resolveRoute(routes, page.url);
    expect(direct.match).toEqual({
      kind: "matched",
      name: page.route,
      params: page.params,
      query: {},
    });
    // 刷新即以同一地址重新进入：解析结果与规范串都不变，不依赖内存中的导航状态。
    expect(resolveRoute(routes, direct.routeString)).toEqual(direct);

    const entry = surface.pages[page.route];
    if (entry === undefined || entry.kind !== "lazy")
      throw new Error(`${page.route} 没有按需加载的页面`);
    const module: Record<string, unknown> = await import(resolve(webRoot, page.source));
    expect(module[page.component]).toBe(await entry.load());
  },
);

/** 页面源文件及其在同一寄售目录内的静态相对依赖。 */
function pageSources(entry: string): string[] {
  const root = dirname(resolve(webRoot, entry));
  const seen = new Set<string>();
  const pending = [resolve(webRoot, entry)];
  for (const file of pending) {
    if (seen.has(file)) continue;
    seen.add(file);
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(/\bfrom "(\.{1,2}\/[^"\n]+)"/gu)) {
      const target = Bun.resolveSync(match[1] ?? "", dirname(file));
      if (dirname(target) === root && !target.endsWith(".test.ts")) pending.push(target);
    }
  }
  return [...seen];
}

/** 某一语言文件声明的全部消息 ID。 */
function messageIds(surface: Surface, locale: LocaleCode): Set<string> {
  const ftl = readFileSync(
    resolve(repositoryRoot, SURFACES[surface].i18n, `${locale}.ftl`),
    "utf8",
  );
  return new Set([...ftl.matchAll(/^([a-z][a-z0-9-]*) =/gmu)].map((match) => match[1] ?? ""));
}

/**
 * 页面源码引用的译文键：`t("…")` 的字面量一律计入；键表里的 consignment- 字面量
 * 只有是默认语言（en）的消息 ID 时才计入，以排除同形的元素 id。
 */
function referencedKeys(surface: Surface): string[] {
  const english = messageIds(surface, "en");
  const keys = new Set<string>();
  for (const page of PAGES.filter((candidate) => candidate.surface === surface)) {
    for (const file of pageSources(page.source)) {
      const text = readFileSync(file, "utf8");
      for (const match of text.matchAll(/\bt\(\s*"([a-z0-9-]+)"/gu)) keys.add(match[1] ?? "");
      for (const match of text.matchAll(/"(consignment-[a-z0-9-]+)"/gu)) {
        if (english.has(match[1] ?? "")) keys.add(match[1] ?? "");
      }
    }
  }
  return [...keys].sort();
}

test.each(
  (["public", "admin"] as const).flatMap((surface) =>
    LOCALES.map((locale) => [surface, locale] as const),
  ),
)("五语：%s 第 6、7 期页面引用的译文键在 %s 中都有正文", (surface, locale) => {
  const keys = referencedKeys(surface);
  expect(keys.length).toBeGreaterThan(0);
  const ftl = readFileSync(
    resolve(repositoryRoot, SURFACES[surface].i18n, `${locale}.ftl`),
    "utf8",
  );
  // 只装本语言文件：默认语言不作底，缺键不会被英文掩盖。
  const bundle = buildBundle(locale, ftl, locale, null);
  if (!bundle.ok) throw new Error(`${surface}/${locale}.ftl 解析失败`);
  const missing = keys.filter((key) => {
    const message = bundle.value.getMessage(key);
    return message === undefined || message.value === null;
  });
  expect(missing).toEqual([]);
});

/** 一个 Public 写入口：同一冻结命令发送一次，返回错误与是否已可结案。 */
interface PublicWrite {
  readonly name: string;
  readonly send: () => Promise<{ readonly ok: boolean; readonly error?: CommandError }>;
  readonly decided: (error: CommandError) => boolean;
}

const PUBLIC_WRITES: readonly PublicWrite[] = [
  {
    name: "购买",
    send: () =>
      purchaseConsignmentListing(
        ALICE.user_id,
        LISTING,
        { command_id: COMMAND, expected_version: 5, expected_price: 101 },
        SIGNAL(),
      ),
    decided: purchaseWriteRejected,
  },
  {
    name: "重列",
    send: () =>
      relistConsignmentListing(
        ALICE.user_id,
        LISTING,
        { command_id: COMMAND, expected_version: 5 },
        SIGNAL(),
      ),
    decided: (error) => relistRejected(error, false),
  },
  {
    name: "权利续办",
    send: () =>
      claimRight({ command_id: COMMAND, expected_version: 2, action: "claim_payment" }, SIGNAL()),
    decided: (error) => rightWriteRejected("claim", error),
  },
];

function writes(requests: readonly RequestRecord[]): RequestRecord[] {
  return requests.filter(({ init }) => init?.method === "POST");
}

test.each(PUBLIC_WRITES.map((write) => [write.name, write] as const))(
  "Public %s遇 503 outcome_unknown 不结案，重放正文与目标逐字节相同；409 stale_version 才结案",
  async (_name, write) => {
    const unknown = fakeFetch(() => failure(503, "outcome_unknown"));
    try {
      const first = await write.send();
      const replay = await write.send();
      for (const result of [first, replay]) {
        expect(result.ok).toBe(false);
        expect(result.error).toEqual({ status: 503, code: "outcome_unknown" });
        if (result.error !== undefined) expect(write.decided(result.error)).toBe(false);
      }
      const posted = writes(unknown.requests);
      expect(posted).toHaveLength(2);
      expect(posted[1]?.url.href).toBe(posted[0]?.url.href);
      expect(String(posted[1]?.init?.body)).toBe(String(posted[0]?.init?.body));
      expect(JSON.parse(String(posted[0]?.init?.body)).command_id).toBe(COMMAND);
    } finally {
      unknown.restore();
    }

    const stale = fakeFetch(() => failure(409, "stale_version"));
    try {
      const result = await write.send();
      expect(result.error).toEqual({ status: 409, code: "stale_version" });
      if (result.error !== undefined) expect(write.decided(result.error)).toBe(true);
    } finally {
      stale.restore();
    }
  },
);

type AdminOperation = (
  commandId: string,
  signal: AbortSignal,
) => Promise<{ readonly ok: boolean; readonly error?: AdminWriteFailure }>;

type AdminHost = Parameters<typeof prepareAdminOrderResolve>[0];

/** Admin 写入口；每次调用重新准备，模拟一次表单提交得到的冻结命令。 */
const ADMIN_WRITES: readonly (readonly [string, (host: AdminHost) => AdminOperation])[] = [
  [
    "订单处置",
    (host) =>
      prepareAdminOrderResolve(host, CONTRACT, {
        expectedVersion: 3,
        action: "cancel_unpaid",
        evidenceId: EVIDENCE,
      }),
  ],
  [
    "确认灭失",
    (host) =>
      prepareAdminConfirmLoss(host, POSITION, {
        expectedVersion: 2,
        proofId: EVIDENCE,
        reason: "夹具：确认灭失",
      }),
  ],
  [
    "损失案件人工结果",
    (host) =>
      prepareAdminLossOutcome(host, POSITION, {
        caseId: CONTRACT,
        expectedCaseVersion: 1,
        outcome: "reviewing",
        basisId: EVIDENCE,
        privateNote: "夹具：复核中",
      }),
  ],
];

test.each(ADMIN_WRITES)(
  "Admin %s遇 503 outcome_unknown 判结果未知，只能以原命令 ID 原样重放；换 ID 不发请求",
  async (_name, prepare) => {
    const harness = createAdminRequestHarness();
    const reply = { status: 503, body: "" };
    const transport = installWriteFetch(() => new Response(reply.body, { status: reply.status }));
    try {
      harness.installBrowserRequest();
      const { host } = harness.registerHost();
      Object.assign(reply, adminFailure(503, "outcome_unknown"));
      const operation = prepare(host);
      const first = await operation(COMMAND, SIGNAL());
      const replay = await operation(COMMAND, SIGNAL());
      for (const result of [first, replay]) {
        expect(result.ok).toBe(false);
        if (result.error !== undefined)
          expect(classifyAdminWriteFailure(result.error)).toBe("unknown");
      }
      expect(transport.writes).toHaveLength(2);
      expect(transport.writes[1]?.url).toBe(transport.writes[0]?.url ?? "");
      expect(transport.writes[1]?.body).toBe(transport.writes[0]?.body ?? "");
      expect(JSON.parse(transport.writes[0]?.body ?? "{}").command_id).toBe(COMMAND);

      // 同一份冻结正文不能改挂新命令 ID：本地拒绝，远端写入数不变。
      const renamed = await operation(OTHER_COMMAND, SIGNAL());
      expect(renamed.ok).toBe(false);
      if (renamed.error !== undefined) expect(renamed.error.kind).toBe("not-sent");
      expect(transport.writes).toHaveLength(2);

      // 正例：契约登记的 409 stale_version 是可结案的确定拒绝。
      Object.assign(reply, adminFailure(409, "stale_version"));
      const stale = await prepare(host)(OTHER_COMMAND, SIGNAL());
      expect(stale.ok).toBe(false);
      if (stale.error !== undefined)
        expect(classifyAdminWriteFailure(stale.error)).not.toBe("unknown");
    } finally {
      transport.restore();
      harness.cleanup();
    }
  },
);

const RIGHTS = "/api/v2/consignment/rights";
const CLAIM_KEY = "hv.consignment.right.claim";
const FIXTURES = resolve(repositoryRoot, "crates/auction-contracts/tests/fixtures/public");

function fixtureData(name: string): Record<string, unknown> {
  const parsed: { data: Record<string, unknown> } = JSON.parse(
    readFileSync(resolve(FIXTURES, name), "utf8"),
  );
  return parsed.data;
}

const CLAIM_REQUEST: Record<string, unknown> = JSON.parse(
  readFileSync(resolve(FIXTURES, "consignment-rights-claim-request.json"), "utf8"),
);

/** 已在会话存储里的未决续办：与共享夹具的视图、请求与回执同一命令。 */
function claimStorage(): Map<string, string> {
  const view = fixtureData("consignment-rights-view.json");
  return new Map([[CLAIM_KEY, JSON.stringify({ ...CLAIM_REQUEST, right_id: view.right_id })]]);
}

// 明显的假值：定位符与挑战码都不对应任何真实权利。
const LOCATOR = "R1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const CODE = "FIXTV-RE000-00000-00000-00000";

/** 受限权利的替身服务端：挑战、核验与当前权利固定成功，其余交给用例。 */
function rightsServer(rest: (request: RequestRecord, path: string) => Response) {
  return fakeFetch((request) => {
    const path = request.url.pathname.slice(RIGHTS.length);
    if (path === "/challenges") {
      return Response.json(
        {
          code: "accepted",
          message: "",
          data: fixtureData("consignment-rights-challenge-accepted.json"),
        },
        { status: 202 },
      );
    }
    if (path === "/verify") return response(fixtureData("consignment-rights-verified.json"));
    if (path === "/current") return response(fixtureData("consignment-rights-view.json"));
    return rest(request, path);
  });
}

/** 刷新后重新打开权利页并重新验权；页面没有登录会话，验权后才读取当前权利。 */
async function reopenRights(storage: Map<string, string>) {
  const { RightsPage } = await loadPages();
  const page = await mountPage(
    "/consignment/rights",
    () => createComponent(RightsPage, {}),
    null,
    storage,
  );
  controlEvent(page.input("password"), "onInput", LOCATOR);
  controlEvent(page.form("Send verification code"), "onSubmit");
  await settle();
  controlEvent(page.input("password"), "onInput", CODE);
  controlEvent(page.form("Verify"), "onSubmit");
  await settle();
  return page;
}

function claimPosts(requests: readonly RequestRecord[]): RequestRecord[] {
  return writes(requests).filter(({ url }) => url.pathname === `${RIGHTS}/current/claim`);
}

test("受限页先查回执：重新验权后先查原命令回执，已有回执即结案，不发续办写入", async () => {
  const transport = rightsServer((_request, path) =>
    path === `/current/commands/${CLAIM_REQUEST.command_id}`
      ? response(fixtureData("consignment-rights-claim-receipt.json"))
      : failure(404, "command_not_found"),
  );
  const storage = claimStorage();
  const page = await reopenRights(storage);
  try {
    expect(
      transport.requests.map(({ url, init }) => `${init?.method ?? "GET"} ${url.pathname}`),
    ).toEqual([
      `POST ${RIGHTS}/challenges`,
      `POST ${RIGHTS}/verify`,
      `GET ${RIGHTS}/current`,
      `GET ${RIGHTS}/current/commands/${CLAIM_REQUEST.command_id}`,
    ]);
    expect(claimPosts(transport.requests)).toHaveLength(0);
    expect(storage.get(CLAIM_KEY)).toBe("null");
    expect(page.button("Retry original request")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("受限页回执查询 404 只代表当次未见：不自动重送，人工重放与原记录逐字节相同", async () => {
  const transport = rightsServer((_request, path) =>
    path === "/current/claim" ? failure(503, "outcome_unknown") : failure(404, "command_not_found"),
  );
  const storage = claimStorage();
  const page = await reopenRights(storage);
  try {
    expect(
      transport.requests.some(
        ({ url }) => url.pathname === `${RIGHTS}/current/commands/${CLAIM_REQUEST.command_id}`,
      ),
    ).toBe(true);
    expect(claimPosts(transport.requests)).toHaveLength(0);
    const retry = page.button("Retry original request");
    if (retry === null) throw new Error("未决续办没有原样重放入口");
    for (const _attempt of [1, 2]) {
      controlEvent(retry, "onClick");
      await settle();
    }
    const posted = claimPosts(transport.requests);
    expect(posted).toHaveLength(2);
    expect(String(posted[1]?.init?.body)).toBe(String(posted[0]?.init?.body));
    expect(JSON.parse(String(posted[0]?.init?.body))).toEqual(CLAIM_REQUEST);
    expect(storage.get(CLAIM_KEY)).not.toBe("null");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("非本人损失回执 404：只显示未找到，不显示任何案件内容", async () => {
  const transport = fakeFetch(() => failure(404, "listing_not_found"));
  const { LossReceiptPage } = await loadPages();
  const page = await mountPage(`/consignment/loss/${POSITION}`, () =>
    createComponent(LossReceiptPage, { params: { position_id: POSITION }, query: {} }),
  );
  try {
    await settle();
    expect(transport.requests.map(({ url }) => url.pathname)).toEqual([
      `/api/v2/me/consignment/positions/${POSITION}/loss-case`,
    ]);
    expect(page.text()).toContain("This listing does not exist or is no longer displayed.");
    expect(page.text().includes("Under review")).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

const RECEIPTS = "/api/v2/me/consignment/commands";

/** 刷新后仍留有未决命令的本人页：会话存储里的原记录、写路径、原请求正文与重放按钮。 */
interface PendingPage {
  readonly key: string;
  readonly record: Record<string, unknown>;
  readonly write: string;
  readonly body: Record<string, unknown>;
  readonly retry: string;
  readonly receipt: Record<string, unknown>;
  readonly open: (storage: Map<string, string>) => ReturnType<typeof mountPage>;
}

// 明显的假值：命令、挂单与合同都不对应任何真实对象。
const PENDING_PAGES: readonly (readonly [string, PendingPage])[] = [
  [
    "购买",
    {
      key: `hv.consignment.purchase.pending.${ALICE.user_id}`,
      record: {
        command_id: COMMAND,
        expected_user_id: ALICE.user_id,
        listing_id: LISTING,
        expected_version: 5,
        expected_price: 101,
      },
      write: `/api/v2/consignment/listings/${LISTING}/purchase`,
      body: { command_id: COMMAND, expected_version: 5, expected_price: 101 },
      retry: "Retry the same purchase",
      receipt: {
        schema_version: 2,
        operation: "purchase",
        disposition: "purchased",
        command_id: COMMAND,
        listing_id: LISTING,
        listing_version: 6,
        state: "sale_pending",
        contract_id: CONTRACT,
        contract_version: 1,
        equipment_id: EVIDENCE,
        world: "main",
        price: 101,
        order_state: "awaiting_payment",
      },
      open: async (storage) => {
        const { PurchaseConfirm } = await loadPages();
        return mountPage(
          `/consignment/listings/${LISTING}/purchase`,
          () => createComponent(PurchaseConfirm, { params: { listing_id: LISTING }, query: {} }),
          ALICE,
          storage,
        );
      },
    },
  ],
  [
    "重列",
    {
      key: `hv.consignment.relist.pending.${ALICE.user_id}`,
      record: {
        command_id: COMMAND,
        expected_user_id: ALICE.user_id,
        listing_id: LISTING,
        expected_version: 5,
      },
      write: `/api/v2/consignment/listings/${LISTING}/relist`,
      body: { command_id: COMMAND, expected_version: 5 },
      retry: "Retry original request",
      receipt: {
        schema_version: 2,
        operation: "relist",
        disposition: "relisted",
        command_id: COMMAND,
        listing_id: LISTING,
        listing_version: 6,
        state: "available",
        price: 101,
        generation: 2,
      },
      open: async (storage) => {
        const { RelistPage } = await loadPages();
        return mountPage(
          `/consignment/me/${LISTING}/relist`,
          () => createComponent(RelistPage, { params: { listing_id: LISTING }, query: {} }),
          ALICE,
          storage,
        );
      },
    },
  ],
  [
    "预约",
    {
      key: `hv.consignment.pending.${ALICE.user_id}`,
      record: {
        command_id: COMMAND,
        expected_user_id: ALICE.user_id,
        listing_id: LISTING,
        expected_version: 5,
        operation: "issue_reservation",
      },
      write: `/api/v2/me/consignment/listings/${LISTING}/deposit-reservations`,
      body: { command_id: COMMAND, expected_version: 5 },
      retry: "Retry original request",
      receipt: {
        schema_version: 2,
        operation: "issue_reservation",
        disposition: "reservation_issued",
        command_id: COMMAND,
        listing_id: LISTING,
        listing_version: 6,
        state: "awaiting_deposit",
        reservation_id: EVIDENCE,
        expires_at: "2026-11-21T00:00:00Z",
      },
      open: async (storage) => {
        const { DepositInstructionsPage } = await loadPages();
        return mountPage(
          `/consignment/me/${LISTING}/deposit-instructions`,
          () =>
            createComponent(DepositInstructionsPage, {
              params: { listing_id: LISTING },
              query: {},
            }),
          ALICE,
          storage,
        );
      },
    },
  ],
  [
    "撤回",
    {
      key: `hv.consignment.pending.${ALICE.user_id}`,
      record: {
        command_id: COMMAND,
        expected_user_id: ALICE.user_id,
        listing_id: LISTING,
        expected_version: 5,
        operation: "withdraw_deposit",
      },
      write: `/api/v2/consignment/listings/${LISTING}/withdraw`,
      body: { command_id: COMMAND, expected_version: 5 },
      retry: "Retry original request",
      receipt: {
        schema_version: 2,
        operation: "withdraw",
        disposition: "closed",
        command_id: COMMAND,
        listing_id: LISTING,
        listing_version: 6,
        state: "closed",
      },
      open: async (storage) => {
        const { OwnedDetail } = await loadPages();
        return mountPage(
          `/consignment/me/${LISTING}`,
          () => createComponent(OwnedDetail, { params: { listing_id: LISTING }, query: {} }),
          ALICE,
          storage,
        );
      },
    },
  ],
];

/** 本人页的替身服务端：回执查询与写入交给用例，其余读取给出可购详情与本人挂单。 */
function ownedServer(receipt: () => Response, write: () => Response) {
  return fakeFetch((request) => {
    const path = request.url.pathname;
    if (path === `${RECEIPTS}/${COMMAND}`) return receipt();
    if (request.init?.method === "POST") return write();
    if (path === STATUS_PATH) return response(status(true));
    if (path.endsWith("/deposit-instructions")) return response({ state: "none" });
    if (path.startsWith(OWNED_PATH)) return response(ownedListing("displayed"));
    return response(purchasableDetail(LISTING, true));
  });
}

/** 只保留回执查询与写入，按发生顺序给出“方法 路径”。 */
function receiptAndWrites(requests: readonly RequestRecord[], write: string): string[] {
  return requests
    .filter(({ url }) => url.pathname === `${RECEIPTS}/${COMMAND}` || url.pathname === write)
    .map(({ url, init }) => `${init?.method ?? "GET"} ${url.pathname}`);
}

test.each(PENDING_PAGES)(
  "本人页先查回执：%s 刷新后先查原命令回执，已有回执即结案，零写入",
  async (_label, page) => {
    const transport = ownedServer(
      () => response(page.receipt),
      () => failure(503, "outcome_unknown"),
    );
    const storage = new Map([[page.key, JSON.stringify(page.record)]]);
    const mounted = await page.open(storage);
    try {
      await settle();
      expect(receiptAndWrites(transport.requests, page.write)).toEqual([
        `GET ${RECEIPTS}/${COMMAND}`,
      ]);
      expect(storage.get(page.key)).toBe("null");
      expect(mounted.button(page.retry)).toBeNull();
    } finally {
      mounted.dispose();
      transport.restore();
    }
  },
);

test.each(PENDING_PAGES)(
  "本人页回执 404 只代表当次未见：%s 不自动重放，人工重放先查回执且正文与原记录逐字节相同",
  async (_label, page) => {
    const transport = ownedServer(
      () => failure(404, "command_not_found"),
      () => failure(503, "outcome_unknown"),
    );
    const storage = new Map([[page.key, JSON.stringify(page.record)]]);
    const mounted = await page.open(storage);
    try {
      await settle();
      const check = `GET ${RECEIPTS}/${COMMAND}`;
      expect(receiptAndWrites(transport.requests, page.write)).toEqual([check]);
      const retry = mounted.button(page.retry);
      if (retry === null) throw new Error("未决命令没有原样重放入口");
      for (const _attempt of [1, 2]) {
        controlEvent(retry, "onClick");
        await settle();
      }
      const post = `POST ${page.write}`;
      expect(receiptAndWrites(transport.requests, page.write)).toEqual([
        check,
        check,
        post,
        check,
        post,
      ]);
      const posted = transport.requests.filter(({ url }) => url.pathname === page.write);
      expect(String(posted[1]?.init?.body)).toBe(String(posted[0]?.init?.body));
      expect(JSON.parse(String(posted[0]?.init?.body))).toEqual(page.body);
      expect(storage.get(page.key)).not.toBe("null");
    } finally {
      mounted.dispose();
      transport.restore();
    }
  },
);

// 未付取消收回后的在库下架：重列唯一合法的一类原因（运营止售按决定 24 永不宣告重列）。
const BASE_HELD = VIEWS.held_off_market;
const HELD_VIEW =
  BASE_HELD.kind === "held_off_market"
    ? { ...BASE_HELD, reason: "unpaid_recovered" as const }
    : BASE_HELD;
const RELIST_ENTRY = `/consignment/me/${LISTING}/relist`;

test.each([
  ["宣告 relist", ["relist", "withdraw"], true],
  ["未宣告 relist", ["withdraw"], false],
] as const)(
  "本人详情在服务端%s时重列入口是否出现（页面不重建资格）",
  async (_label, actions, linked) => {
    const transport = fakeFetch((request) =>
      request.url.pathname === STATUS_PATH
        ? response(status(true))
        : response(
            ownedListing("held_off_market", {
              view: HELD_VIEW,
              effective_reason: "unpaid_recovered",
              allowed_actions: [...actions],
            }),
          ),
    );
    const { OwnedDetail } = await loadPages();
    const mounted = await mountPage(`/consignment/me/${LISTING}`, () =>
      createComponent(OwnedDetail, { params: { listing_id: LISTING }, query: {} }),
    );
    try {
      await settle();
      expect(mounted.find((node) => node.attributes.href === RELIST_ENTRY) !== null).toBe(linked);
      expect(mounted.text().includes("Relist")).toBe(linked);
    } finally {
      mounted.dispose();
      transport.restore();
    }
  },
);
