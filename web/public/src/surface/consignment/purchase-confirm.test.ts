// 购买确认页与订单详情页：金额口径、结果未知只原样重放、金额按角色显示、非本人 404。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";
import { createLocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { transform } from "@solidjs/compiler";
import { children, createComponent, createEffect, createRoot, type Element, flush } from "solid-js";

import { findElement, type TestElement } from "../catalog/command-scope-fixture.test";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { ALICE } from "../shell/session-fixture";
import {
  buyerOrder,
  COMMAND_A,
  CONTRACT_A,
  detail,
  LISTINGS_PATH,
  ORDERS_PATH,
  sellerOrder,
} from "./order-fixture.test";
import {
  controlEvent,
  DOMText,
  failure,
  fakeFetch,
  LISTING_A,
  response,
  STATUS_PATH,
  settle,
  status,
} from "./read-fixture.test";

const NAMESPACE = "public-consignment-purchase-render";
const PURCHASE = `${LISTINGS_PATH}/${LISTING_A}/purchase`;
const RECEIPTS = "/api/v2/me/consignment/commands/";
const SOURCES = new Set(
  [
    "./purchase-confirm.tsx",
    "./order-detail.tsx",
    "./detail.tsx",
    "./private-page.tsx",
    "./read-view.tsx",
    "./filters.tsx",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/ui/components/async-state.tsx",
    "../../../../shared/src/ui/components/dialog.tsx",
    "../../../../shared/src/ui/components/time.tsx",
  ].map((path) => resolve(import.meta.dir, path)),
);
const stripTypes = new Bun.Transpiler({ loader: "ts" });
let rendererRegistered = false;

function registerRenderer(): void {
  if (rendererRegistered) return;
  Bun.plugin({
    name: NAMESPACE,
    setup(build) {
      build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
        path,
        namespace: NAMESPACE,
      }));
      build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
        if (!SOURCES.has(path)) throw new Error(`未登记购买页测试组件 ${path}`);
        const source = readFileSync(path, "utf8").replace(/^import "[^"\n]+\.css";$/gmu, "");
        const code = path.endsWith(".tsx")
          ? transform(source, {
              filename: path,
              generate: "universal",
              moduleName: import.meta.path,
            }).code
          : source;
        const contents = stripTypes
          .transformSync(code)
          .replace(/(\bfrom |\bimport\()"([^"\n]+)"/gu, (_match, prefix, specifier) => {
            const dependency = Bun.resolveSync(specifier, dirname(path));
            return `${prefix}${JSON.stringify(SOURCES.has(dependency) ? `${NAMESPACE}:${dependency}` : dependency)}`;
          });
        return { contents, loader: "js" };
      });
    },
  });
  rendererRegistered = true;
}

async function load<T>(file: string): Promise<T> {
  registerRenderer();
  return import(`${NAMESPACE}:${resolve(import.meta.dir, file)}`) as Promise<T>;
}

export { createEffect as effect, createMemo as memo } from "solid-js";
export {
  createComponent,
  insert,
  insertNode,
  setProp,
} from "../catalog/command-scope-fixture.test";
export { createElement, createTextNode, ref, spread } from "../shell/navigation-fixture.test";

interface Harness {
  readonly text: () => string;
  readonly button: (text: string) => TestElement | null;
  readonly storage: Map<string, string>;
  readonly location: { pathname: string };
  dispose(): void;
}

async function mount(
  path: string,
  page: (module: never) => Element,
  file: string,
  storage = new Map<string, string>(),
): Promise<Harness> {
  const installed = installFakeWindow(path);
  const fakeStorage = {
    getItem: (key: string) => storage.get(key) ?? null,
    setItem: (key: string, value: string) => storage.set(key, value),
    removeItem: (key: string) => storage.delete(key),
  };
  Object.assign(installed.fake, {
    isSecureContext: true,
    localStorage: fakeStorage,
    sessionStorage: fakeStorage,
  });
  const before = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: Object.assign(new EventTarget(), { hidden: false }),
  });
  const ftl = await Bun.file(
    `${import.meta.dir}/../../../../../crates/auction-web-public/i18n/en.ftl`,
  ).text();
  const bundle = buildBundle("en", ftl, "en", null);
  if (!bundle.ok) throw new Error("夹具译文加载失败");
  const session = createCell<PublicSessionResult | undefined>(ok(ALICE));
  const owner = createCell<string | null>(ALICE.user_id);
  const authority: PublicSessionResource = Object.assign(session, {
    state: () => "ready" as const,
    restart() {},
    cancel() {},
    clear: () => session.set(undefined),
    confirmation: () => ({ revision: null }),
  });
  const module = await load<never>(file);
  const mounted = createRoot((dispose) => {
    const locale = createLocaleSession(bundle.value, "en");
    const router = createRouter({ routes: [] });
    const tree = children(() =>
      createComponent(LocaleSessionContext, {
        value: locale,
        get children() {
          return createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(PublicSessionResourceContext, {
                value: authority,
                get children() {
                  return createComponent(PublicCommandOwnerContext, {
                    value: owner,
                    get children() {
                      return page(module);
                    },
                  });
                },
              });
            },
          });
        },
      }),
    );
    createEffect(
      () => DOMText(tree()),
      () => {},
    );
    flush();
    return { dispose, tree };
  });
  return {
    text: () => DOMText(mounted.tree()),
    button: (text: string) =>
      findElement(mounted.tree(), (node) => node.tag === "button" && DOMText(node) === text),
    storage,
    location: installed.fake.location,
    dispose() {
      mounted.dispose();
      flush();
      if (before === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", before);
      installed.restore();
    },
  };
}

type PurchaseModule = typeof import("./purchase-confirm");
type OrderModule = typeof import("./order-detail");

function purchasePage(storage?: Map<string, string>) {
  return mount(
    `/consignment/listings/${LISTING_A}/purchase`,
    (module) =>
      createComponent((module as PurchaseModule).PurchaseConfirm, {
        params: { listing_id: LISTING_A },
        query: {},
      }),
    "./purchase-confirm.tsx",
    storage,
  );
}

function orderPage() {
  return mount(
    `/consignment/orders/${CONTRACT_A}`,
    (module) =>
      createComponent((module as OrderModule).OrderDetail, {
        params: { contract_id: CONTRACT_A },
        query: {},
      }),
    "./order-detail.tsx",
  );
}

function receipt(commandId: string, price: number, world: "main" | "isekai") {
  return Response.json(
    {
      code: "created",
      message: "",
      data: {
        schema_version: 2,
        operation: "purchase",
        disposition: "purchased",
        command_id: commandId,
        listing_id: LISTING_A,
        listing_version: 6,
        state: "sale_pending",
        contract_id: CONTRACT_A,
        contract_version: 1,
        equipment_id: "00000000-0000-4000-8000-000000000502",
        world,
        price,
        order_state: "awaiting_payment",
      },
    },
    { status: 201 },
  );
}

function body(request: { readonly init: RequestInit | undefined }): Record<string, unknown> {
  const text = request.init?.body;
  if (typeof text !== "string") throw new Error("缺少写入正文");
  return JSON.parse(text);
}

const purchasableStatus = () => response(status(true));

test.each([
  ["main", "COD you pay in game (Credits)101", false],
  ["isekai", "COD you pay in game (Credits)102", true],
] as const)("确认页显示标价 P；%s 的 COD 口径与载体本金", async (world, cod, carrier) => {
  const listing = detail(LISTING_A, true);
  const shown = { ...listing, summary: { ...listing.summary, world_id: world, price: 101 } };
  const transport = fakeFetch(({ url }) =>
    url.pathname === STATUS_PATH ? purchasableStatus() : response(shown),
  );
  const page = await purchasePage();
  try {
    await settle();
    expect(page.text()).toContain("Asking price (Credits)101");
    expect(page.text()).toContain(cod);
    expect(page.text().includes("Carrier Credits included1")).toBe(carrier);
    expect(page.button("Buy now")?.attributes.disabled).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test.each([
  ["503 outcome_unknown", () => failure(503, "outcome_unknown")],
  ["504", () => failure(504, "timeout")],
  [
    "断线",
    () => {
      throw new TypeError("测试断线");
    },
  ],
] as const)("购买遇 %s 只能原样重放同一命令，不另起新命令", async (_name, firstReply) => {
  let writes = 0;
  const bodies: string[] = [];
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return purchasableStatus();
    // 回执查询 404 只代表当次未见，才放行原样重放（D9 §6.3）。
    if (pathname.startsWith(RECEIPTS)) return failure(404, "command_not_found");
    if (pathname === PURCHASE) {
      writes += 1;
      bodies.push(String(request.init?.body));
      if (writes === 1) return firstReply();
      return receipt(String(body(request).command_id), 1234, "main");
    }
    return response(detail(LISTING_A, true));
  });
  const page = await purchasePage();
  try {
    await settle();
    controlEvent(page.button("Buy now"), "onClick");
    await settle();
    expect(writes).toBe(1);
    expect(page.text()).toContain("The result of your purchase is unknown");
    // 未决期间不能发起新购买：按钮禁用，点击不产生第二条命令。
    expect(page.button("Buy now")?.attributes.disabled).toBe(true);
    controlEvent(page.button("Buy now"), "onClick");
    await settle();
    expect(writes).toBe(1);
    controlEvent(page.button("Retry the same purchase"), "onClick");
    await settle();
    expect(writes).toBe(2);
    expect(bodies[1]).toBe(bodies[0]);
    expect(page.location.pathname).toBe(`/consignment/orders/${CONTRACT_A}`);
    expect([...page.storage.values()]).toEqual(["null"]);
  } finally {
    page.dispose();
    transport.restore();
  }
});

for (const verified of [false, true]) {
  test(`购买确认的卖家一栏${verified ? "不标" : "标注"}卖家自填、未验证`, async () => {
    const listing = detail(LISTING_A, true);
    const transport = fakeFetch(({ url }) =>
      url.pathname === STATUS_PATH
        ? purchasableStatus()
        : response({ ...listing, summary: { ...listing.summary, seller_verified: verified } }),
    );
    const page = await purchasePage();
    try {
      await settle();
      expect(page.text()).toContain("Buy now");
      expect(page.text().includes("Self-reported by the seller, not verified")).toBe(!verified);
    } finally {
      page.dispose();
      transport.restore();
    }
  });
}

test("确定拒绝 price_changed 结案并允许重新确认，旧命令不再重放", async () => {
  let writes = 0;
  const commands: unknown[] = [];
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return purchasableStatus();
    if (pathname === PURCHASE) {
      writes += 1;
      commands.push(body(request).command_id);
      return failure(409, "price_changed");
    }
    return response(detail(LISTING_A, true));
  });
  const page = await purchasePage();
  try {
    await settle();
    controlEvent(page.button("Buy now"), "onClick");
    await settle();
    expect(page.text()).toContain("The asking price changed");
    expect([...page.storage.values()]).toEqual(["null"]);
    expect(page.button("Retry the same purchase")).toBeNull();
    controlEvent(page.button("Buy now"), "onClick");
    await settle();
    expect(writes).toBe(2);
    expect(commands[1]).not.toBe(commands[0]);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("会话里留有未决购买时刷新页面只给原样重放，不开新购买", async () => {
  const record = {
    command_id: COMMAND_A,
    expected_user_id: ALICE.user_id,
    listing_id: LISTING_A,
    expected_version: 5,
    expected_price: 101,
  };
  const storage = new Map([
    [`hv.consignment.purchase.pending.${ALICE.user_id}`, JSON.stringify(record)],
  ]);
  const bodies: string[] = [];
  const transport = fakeFetch((request) => {
    const { pathname } = request.url;
    if (pathname === STATUS_PATH) return purchasableStatus();
    if (pathname.startsWith(RECEIPTS)) return failure(404, "command_not_found");
    if (pathname === PURCHASE) {
      bodies.push(String(request.init?.body));
      return receipt(COMMAND_A, 101, "main");
    }
    return response(detail(LISTING_A, true));
  });
  const page = await purchasePage(storage);
  try {
    await settle();
    expect(page.button("Buy now")?.attributes.disabled).toBe(true);
    controlEvent(page.button("Retry the same purchase"), "onClick");
    await settle();
    expect(JSON.parse(bodies[0] ?? "{}")).toEqual({
      command_id: COMMAND_A,
      expected_version: 5,
      expected_price: 101,
    });
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("本人订单页按角色显示金额：买方只见 P 与 COD，卖方另见 F 与 S", async () => {
  for (const [order, seesSplit] of [
    [buyerOrder("isekai"), false],
    [sellerOrder("main"), true],
  ] as const) {
    const transport = fakeFetch(() => response(order));
    const page = await orderPage();
    try {
      await settle();
      expect(page.text()).toContain("Asking price (Credits)101");
      expect(page.text().includes("Fee (Credits)5")).toBe(seesSplit);
      expect(page.text().includes("Seller proceeds (Credits)96")).toBe(seesSplit);
      // 买方在 isekai 的 COD 是 P + 1；卖方不付款，不显示 COD。
      expect(page.text().includes("COD you pay in game (Credits)102")).toBe(!seesSplit);
    } finally {
      page.dispose();
      transport.restore();
    }
  }
});

test("非本人订单（404 listing_not_found）只显示未找到，不显示任何订单内容", async () => {
  const transport = fakeFetch((request) =>
    request.url.pathname === `${ORDERS_PATH}/${CONTRACT_A}`
      ? failure(404, "listing_not_found")
      : response(buyerOrder()),
  );
  const page = await orderPage();
  try {
    await settle();
    expect(page.text()).toContain("This order does not exist.");
    expect(page.text().includes("Asking price")).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});
