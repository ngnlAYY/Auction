// 登记与本人挂单测试：真实 Solid 懒加载、受控身份、浏览器存储和脱敏 HTTP 夹具。

import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type { CommandReceipt } from "@auction/contracts/dto/public/consignment";
import type {
  ListingViewV2,
  OwnedListingV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";
import { createLocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { transform } from "@solidjs/compiler";
import { children, createComponent, createEffect, createRoot, flush } from "solid-js";

import { findElement, type TestElement } from "../catalog/command-scope-fixture.test";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import type { createPublicShellSession } from "../shell/session";
import { ALICE, BOB } from "../shell/session-fixture";
import {
  controlEvent,
  DOMText,
  detail,
  LISTING_A,
  LISTINGS_PATH,
  type RequestRecord,
  settle,
} from "./read-fixture.test";

export { createEffect as effect, createMemo as memo } from "solid-js";
export {
  createComponent,
  insert,
  insertNode,
  setProp,
} from "../catalog/command-scope-fixture.test";
export { createElement, createTextNode, ref, spread } from "../shell/navigation-fixture.test";
export { ALICE, BOB };

export const OWNED_PATH = "/api/v2/me/consignment/listings";
/** 新撤回走的 v2 命令路由；v1 的 withdraw 只剩磁盘上遗留记录的重放。 */
export const WITHDRAW_PATH = "/api/v2/consignment/listings";
export const COMMAND_PATH = "/api/v1/me/consignment/commands";
/** 预约与 v2 撤回的本人命令回执查询；未决记录重送前先查这里。 */
export const RECEIPT_V2_PATH = "/api/v2/me/consignment/commands";
export const FIXTURE_KEY = "012345abcd";
export const EQUIPMENT_URL = `https://hentaiverse.org/equip/42/${FIXTURE_KEY}`;
export const ISEKAI_URL = `https://hentaiverse.org/isekai/equip/42/${FIXTURE_KEY}`;
export const COMMAND_ID = "00000000-0000-4000-8000-000000000010";
export const NEW = "/consignment/new";
export const DETAIL = `/consignment/me/${LISTING_A}`;
export const RETRY = "Retry original request";
export const REPRICE = "Change asking price";
export const WITHDRAW = "Cancel listing";
export const PENDING_KEY = `hv.consignment.pending.${ALICE.user_id}`;
export const OPERATIONS = ["register", "reprice", "withdraw"] as const;
/** 寄售卖家的三种写操作。 */
export type Operation = (typeof OPERATIONS)[number];
/** 页面上的撤回按钮登记的是 v2 的 withdraw_deposit，其余与页面操作同名。 */
export const STORED_OPERATION: Record<Operation, string> = {
  register: "register",
  reprice: "reprice",
  withdraw: "withdraw_deposit",
};
export const TIMES = {
  created: "2026-11-19T00:00:00Z",
  updated: "2026-11-20T00:00:00Z",
  expires: "2026-11-21T00:00:00Z",
};

export const VIEWS: Record<ListingViewV2["kind"], ListingViewV2> = {
  resolving: {
    kind: "resolving",
    progress: {
      attempts: 2,
      max_attempts: 6,
      next_attempt_at: TIMES.updated,
      last_reason: "source_timeout",
      paused: true,
      deadline_at: TIMES.expires,
    },
  },
  lookup_failed: {
    kind: "lookup_failed",
    reason: "lookup_exhausted",
    last_lookup_reason: "source_rate_limited",
  },
  ineligible: {
    kind: "ineligible",
    reason: "not_tradeable",
    equipment: {
      ...detail().equipment,
      level: { kind: "soulbound" },
      tradeable: false,
    },
  },
  displayed: { kind: "displayed", equipment: detail().equipment },
  awaiting_deposit: {
    kind: "awaiting_deposit",
    equipment: detail().equipment,
  },
  deposit_accepted: { kind: "deposit_accepted", equipment: detail().equipment },
  available: { kind: "available", equipment: detail().equipment },
  sale_pending: { kind: "sale_pending", equipment: detail().equipment },
  held_off_market: {
    kind: "held_off_market",
    equipment: detail().equipment,
    reason: "operations_hold",
    hold_id: null,
  },
  withdrawal_pending: { kind: "withdrawal_pending", equipment: detail().equipment },
  closed: { kind: "closed", reason: "service_disabled", equipment: detail().equipment },
  expired: { kind: "expired", reason: "lookup_deadline", equipment: null },
};

const CONSIGNED_KINDS = new Set<string>([
  "awaiting_deposit",
  "deposit_accepted",
  "available",
  "sale_pending",
  "held_off_market",
  "withdrawal_pending",
]);
const GENERATED_KINDS = new Set<string>([
  "available",
  "sale_pending",
  "held_off_market",
  "withdrawal_pending",
]);

export function listing(
  kind: ListingViewV2["kind"],
  update: Partial<OwnedListingV2> = {},
): OwnedListingV2 {
  const view = VIEWS[kind];
  const state = kind === "lookup_failed" || kind === "ineligible" ? "rejected" : kind;
  return {
    listing_id: LISTING_A,
    version: 3,
    world_id: "main",
    mode: CONSIGNED_KINDS.has(kind) ? "consigned" : "direct",
    external_equipment_id: "42",
    price: 101,
    pricing_version: 1,
    fee_bps: 0,
    estimated_fee: 0,
    estimated_seller_amount: 101,
    created_at: TIMES.created,
    updated_at: TIMES.updated,
    expires_at: TIMES.expires,
    generation: GENERATED_KINDS.has(kind) ? 1 : null,
    position_id: null,
    loss_case_id: null,
    stored_state: state,
    effective_state: state,
    effective_reason: "reason" in view ? view.reason : null,
    allowed_actions: [],
    view,
    ...update,
  };
}

export function receipt(
  commandId = COMMAND_ID,
  update: Partial<CommandReceipt> = {},
): CommandReceipt {
  return {
    command_id: commandId,
    operation: "register",
    listing_id: LISTING_A,
    listing_version: 1,
    state: "resolving",
    disposition: "registered",
    price: 101,
    ...update,
  };
}

export function acceptedReceipt(result = receipt()): Response {
  return Response.json({ code: "accepted", message: "", data: result }, { status: 202 });
}

export function expectedUser(request: RequestRecord | undefined): string | null {
  return new Headers(request?.init?.headers).get(PUBLIC_EXPECTED_USER_ID_HEADER);
}

export function isWrite({ init }: RequestRecord): boolean {
  return init?.method === "POST" || init?.method === "PATCH";
}

export function writePath({ url }: RequestRecord): boolean {
  return [LISTINGS_PATH, WITHDRAW_PATH].some(
    (base) => url.pathname === base || url.pathname.startsWith(`${base}/`),
  );
}

/** v2 撤回的 200 closed 回执。 */
export function withdrawReceipt(commandId: string, version = 4) {
  return {
    schema_version: 2,
    operation: "withdraw",
    disposition: "closed",
    command_id: commandId,
    listing_id: LISTING_A,
    listing_version: version,
    state: "closed",
  };
}

export function requestBody(request: RequestRecord | undefined): Record<string, unknown> {
  const text = request?.init?.body;
  if (typeof text !== "string") throw new Error("测试请求缺少 JSON 正文");
  return JSON.parse(text);
}

export function pageData(url: URL, items: OwnedListingV2[], total = items.length) {
  return {
    items,
    total,
    page: Number(url.searchParams.get("page")),
    page_size: Number(url.searchParams.get("page_size")),
  };
}

const NAMESPACE = "private-consignment-render";
const SOURCES = new Set(
  [
    "./registration.tsx",
    "./registration-controls.tsx",
    "./direct-registration.tsx",
    "./owned.tsx",
    "./orders.tsx",
    "./owned-detail.tsx",
    "./private-page.tsx",
    "./pending-view.tsx",
    "./filters.tsx",
    "./detail.tsx",
    "./read-view.tsx",
    "../router/route.ts",
    "../../../../shared/src/router/index.tsx",
    "../../../../shared/src/router/outlet.tsx",
    "../../../../shared/src/router/link.tsx",
    "../../../../shared/src/ui/components/async-state.tsx",
    "../../../../shared/src/ui/components/dialog.tsx",
    "../../../../shared/src/ui/components/time.tsx",
    "../../../../shared/src/ui/components/pager/index.tsx",
    "../../../../shared/src/ui/components/pager/size.tsx",
  ].map((path) => resolve(import.meta.dir, path)),
);
const stripTypes = new Bun.Transpiler({ loader: "ts" });
Bun.plugin({
  name: NAMESPACE,
  setup(build) {
    build.onResolve({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => ({
      path,
      namespace: NAMESPACE,
    }));
    build.onLoad({ filter: /.*/u, namespace: NAMESPACE }, ({ path }) => {
      if (!SOURCES.has(path)) throw new Error("未登记的本人挂单测试组件");
      const source = readFileSync(path, "utf8").replace(/^import "[^"\n]+\.css";$/gmu, "");
      const code = path.endsWith(".tsx")
        ? transform(source, { filename: path, generate: "universal", moduleName: import.meta.path })
            .code
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

export function memory() {
  return { local: new Map<string, string>(), session: new Map<string, string>() };
}

function store(values: Map<string, string>) {
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  };
}

/** 用生产路由挂载本人页，可接入真实会话外壳验证卸载边界。 */
export async function mountPrivate(
  initialUrl: string,
  initialUser: typeof ALICE | null = ALICE,
  storage = memory(),
  shell?: Pick<ReturnType<typeof createPublicShellSession>, "session" | "commandOwner">,
) {
  const routeModule = `${NAMESPACE}:${resolve(import.meta.dir, "../router/route.ts")}`;
  const outletModule = `${NAMESPACE}:${resolve(import.meta.dir, "../../../../shared/src/router/outlet.tsx")}`;
  const { PUBLIC_ROUTES, PUBLIC_PAGES }: typeof import("../router/route") = await import(
    routeModule
  );
  const { RouteOutlet }: typeof import("@auction/web-shared/router/outlet") = await import(
    outletModule
  );
  // 先完成测试模块的初始加载，路由出口仍经生产加载器挂载页面。
  await Promise.all(
    [
      "ConsignmentRegistration",
      "ConsignmentMine",
      "ConsignmentOrders",
      "ConsignmentOwnedDetail",
    ].map((name) => {
      const route = PUBLIC_PAGES[name];
      if (route?.kind !== "lazy") throw new Error(`本人寄售路由未懒加载：${name}`);
      return route.load();
    }),
  );
  const ftl = await Bun.file(
    `${import.meta.dir}/../../../../../crates/auction-web-public/i18n/en.ftl`,
  ).text();
  const bundle = buildBundle("en", ftl, "en", null);
  if (!bundle.ok) throw new Error("本人页夹具加载译文失败");

  const installed = installFakeWindow(initialUrl);
  Object.assign(installed.fake, {
    isSecureContext: shell !== undefined,
    localStorage: store(storage.local),
    sessionStorage: store(storage.session),
  });
  const before = Object.getOwnPropertyDescriptor(globalThis, "document");
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: Object.assign(new EventTarget(), { hidden: false }),
  });

  const session = createCell<PublicSessionResult | undefined>(ok(initialUser));
  const owner = createCell<string | null>(initialUser?.user_id ?? null);
  const errors: unknown[] = [];
  const authority: PublicSessionResource & { confirmation: () => { revision: null } } =
    Object.assign(session, {
      state: () => "ready" as const,
      restart() {},
      cancel() {},
      clear: () => session.set(undefined),
      confirmation: () => ({ revision: null }),
    });
  const page = createRoot((dispose) => {
    const locale = createLocaleSession(bundle.value, "en");
    const router = createRouter({ routes: PUBLIC_ROUTES });
    const tree = children(() =>
      createComponent(LocaleSessionContext, {
        value: locale,
        get children() {
          return createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(PublicSessionResourceContext, {
                value: shell?.session ?? authority,
                get children() {
                  return createComponent(PublicCommandOwnerContext, {
                    value: shell?.commandOwner ?? owner,
                    get children() {
                      return createComponent(RouteOutlet, {
                        pages: PUBLIC_PAGES,
                        notFound: null,
                        onLoadError: (error) => errors.push(error),
                      });
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
    return { router, tree, dispose };
  });
  const find = (match: (node: TestElement) => boolean) => findElement(page.tree(), match);
  const location = installed.fake.location;
  return {
    find,
    errors,
    location,
    storage,
    writes: installed.fake.writes,
    text: () => DOMText(page.tree()),
    button: (text: string) => find((node) => node.tag === "button" && DOMText(node) === text),
    form: (button: string) => find((node) => node.tag === "form" && DOMText(node).includes(button)),
    input: (type: string) => find((node) => node.tag === "input" && node.attributes.type === type),
    focus: () => installed.fake.dispatchEvent(new Event("focus")),
    async go(href: string) {
      page.router.push(href);
      await settle();
    },
    async identify(identity: typeof ALICE | null) {
      owner.set(identity?.user_id ?? null);
      session.set(ok(identity));
      await settle();
    },
    dispose() {
      page.dispose();
      flush();
      if (before === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", before);
      installed.restore();
    },
  };
}

type Page = Awaited<ReturnType<typeof mountPrivate>>;

/** 从本人页的真实表单提交指定的写命令。 */
export function attemptWrite(page: Page, operation: Operation) {
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

export function fill(page: Page, mode = "consigned", url = EQUIPMENT_URL, price = "101") {
  controlEvent(
    page.find((node) => node.tag === "select"),
    "onChange",
    mode,
  );
  controlEvent(page.input("password"), "onInput", url);
  controlEvent(
    page.find((node) => node.tag === "input" && node.attributes.inputmode === "numeric"),
    "onInput",
    price,
  );
}

export function submitRegistration(page: Page) {
  controlEvent(page.form("Submit registration"), "onSubmit");
}

export function assertNoSecret(page: Page) {
  const state = JSON.stringify([
    page.location.href,
    page.writes,
    [...page.storage.local],
    [...page.storage.session],
  ]);
  if (state.includes(EQUIPMENT_URL) || state.includes(ISEKAI_URL) || state.includes(FIXTURE_KEY)) {
    throw new Error("URL、历史或浏览器存储泄露了装备凭据");
  }
}
