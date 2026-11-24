// 寄件指引页：none/active、复制、过期刷新、签发重放与结果未知。

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
import { children, createComponent, createEffect, createRoot, flush } from "solid-js";

import { findElement, type TestElement } from "../catalog/command-scope-fixture.test";
import {
  PublicCommandOwnerContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../index";
import { ALICE } from "../shell/session-fixture";
import { listing } from "./private-fixture.test";
import {
  controlEvent,
  DOMText,
  failure,
  fakeFetch,
  LISTING_A,
  response,
  settle,
} from "./read-fixture.test";

const PATH = `/consignment/me/${LISTING_A}/deposit-instructions`;
const OWNED = `/api/v1/me/consignment/listings/${LISTING_A}`;
const INSTRUCTIONS = `/api/v2/me/consignment/listings/${LISTING_A}/deposit-instructions`;
const RESERVATIONS = `/api/v2/me/consignment/listings/${LISTING_A}/deposit-reservations`;
const CODE = `C1-${"A".repeat(43)}`;
const RESERVATION = "00000000-0000-4000-8000-0000000000b1";
const NAMESPACE = "public-consignment-deposit-render";
const SOURCES = new Set(
  [
    "./deposit-instructions.tsx",
    "./private-page.tsx",
    "./pending-view.tsx",
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
        if (!SOURCES.has(path)) throw new Error(`未登记寄件指引测试组件 ${path}`);
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

async function loadPage(): Promise<typeof import("./deposit-instructions")> {
  const path = resolve(import.meta.dir, "deposit-instructions.tsx");
  if (!SOURCES.has(path)) throw new Error("寄件指引页未登记");
  registerRenderer();
  return import(`${NAMESPACE}:${path}`) as Promise<typeof import("./deposit-instructions")>;
}

export { createEffect as effect, createMemo as memo } from "solid-js";
export {
  createComponent,
  insert,
  insertNode,
  setProp,
} from "../catalog/command-scope-fixture.test";
export { createElement, createTextNode, ref, spread } from "../shell/navigation-fixture.test";

function active(expiresAt = "2099-01-01T00:00:00Z") {
  return {
    state: "active",
    world: "main",
    bot: "FixtureBot",
    subject: "consign",
    body: CODE,
    reservation_id: RESERVATION,
    expires_at: expiresAt,
  };
}

interface Harness {
  readonly find: (match: (node: TestElement) => boolean) => TestElement | null;
  readonly text: () => string;
  readonly button: (text: string) => TestElement | null;
  readonly storage: Map<string, string>;
  readonly location: { href: string };
  readonly clipboard: string[];
  dispose(): void;
}

async function mount(storage = new Map<string, string>()): Promise<Harness> {
  const installed = installFakeWindow(PATH);
  const clipboard: string[] = [];
  const previousNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: { clipboard: { writeText: async (text: string) => clipboard.push(text) } },
  });
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
  const pageModule = await loadPage();
  const page = createRoot((dispose) => {
    const { DepositInstructionsPage } = pageModule;
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
                      return createComponent(DepositInstructionsPage, {
                        params: { listing_id: LISTING_A },
                        query: {},
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
    return { dispose, tree };
  });
  const find = (match: (node: TestElement) => boolean) => findElement(page.tree(), match);
  return {
    find,
    text: () => DOMText(page.tree()),
    button: (text: string) => find((node) => node.tag === "button" && DOMText(node) === text),
    storage,
    location: installed.fake.location,
    clipboard,
    dispose() {
      page.dispose();
      flush();
      if (before === undefined) Reflect.deleteProperty(globalThis, "document");
      else Object.defineProperty(globalThis, "document", before);
      if (previousNavigator === undefined) Reflect.deleteProperty(globalThis, "navigator");
      else Object.defineProperty(globalThis, "navigator", previousNavigator);
      installed.restore();
    },
  };
}

function write(request: { readonly init: RequestInit | undefined }): Record<string, unknown> {
  const body = request.init?.body;
  if (typeof body !== "string") throw new Error("缺少写入正文");
  return JSON.parse(body);
}

test("none 签发后读取 active，并只把预约码留在页面内存", async () => {
  let current: unknown = { state: "none" };
  const transport = fakeFetch((request) => {
    if (request.url.pathname === OWNED) {
      return response(listing("awaiting_deposit", { version: 3 }));
    }
    if (request.url.pathname === INSTRUCTIONS) return response(current);
    if (request.url.pathname === RESERVATIONS) {
      const body = write(request);
      expect(body.command_id).toMatch(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
      );
      expect(body.expected_version).toBe(3);
      current = active();
      return Response.json(
        {
          code: "created",
          message: "",
          data: {
            schema_version: 2,
            operation: "issue_reservation",
            disposition: "reservation_issued",
            command_id: body.command_id,
            listing_id: LISTING_A,
            listing_version: 4,
            state: "awaiting_deposit",
            reservation_id: RESERVATION,
            expires_at: "2099-01-01T00:00:00Z",
          },
        },
        { status: 201 },
      );
    }
    throw new Error(`未期望请求 ${request.url.pathname}`);
  });
  const page = await mount();
  try {
    await settle();
    const issue = page.button("Create delivery instructions");
    expect(issue).not.toBeNull();
    controlEvent(issue, "onClick");
    await settle();
    expect(page.text()).toContain(CODE);
    expect(page.text()).toContain("FixtureBot");
    expect(page.text()).toContain("consign");
    expect(page.location.href.includes(CODE)).toBe(false);
    expect([...page.storage.values()].join("").includes(CODE)).toBe(false);
    const copy = page.button("Copy code");
    expect(copy).not.toBeNull();
    controlEvent(copy, "onClick");
    await settle();
    expect(page.clipboard).toEqual([CODE]);
    expect(page.text()).toContain("Reservation code copied.");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("过期后刷新回到 none，不再显示旧预约码", async () => {
  let current: unknown = active("2000-01-01T00:00:00Z");
  const transport = fakeFetch((request) => {
    if (request.url.pathname === OWNED) {
      return response(listing("awaiting_deposit", { version: 3 }));
    }
    if (request.url.pathname === INSTRUCTIONS) return response(current);
    throw new Error(`未期望请求 ${request.url.pathname}`);
  });
  const page = await mount();
  try {
    await settle();
    expect(page.text()).toContain("The deadline for sending the equipment has passed.");
    current = { state: "none" };
    controlEvent(page.button("Refresh instructions"), "onClick");
    await settle();
    expect(page.text()).toContain("No active delivery reservation.");
    expect(page.text().includes(CODE)).toBe(false);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("outcome_unknown 保留同一命令与正文，只允许原样重送", async () => {
  let current: unknown = { state: "none" };
  let writes = 0;
  const bodies: string[] = [];
  const transport = fakeFetch((request) => {
    if (request.url.pathname === OWNED) {
      return response(listing("awaiting_deposit", { version: 3 }));
    }
    if (request.url.pathname === INSTRUCTIONS) return response(current);
    if (request.url.pathname === RESERVATIONS) {
      writes += 1;
      bodies.push(String(request.init?.body));
      if (writes === 1) return failure(503, "outcome_unknown");
      current = active();
      const body = write(request);
      return Response.json(
        {
          code: "created",
          message: "",
          data: {
            schema_version: 2,
            operation: "issue_reservation",
            disposition: "reservation_issued",
            command_id: body.command_id,
            listing_id: LISTING_A,
            listing_version: 4,
            state: "awaiting_deposit",
            reservation_id: RESERVATION,
            expires_at: "2099-01-01T00:00:00Z",
          },
        },
        { status: 201 },
      );
    }
    throw new Error(`未期望请求 ${request.url.pathname}`);
  });
  const page = await mount();
  try {
    await settle();
    controlEvent(page.button("Create delivery instructions"), "onClick");
    await settle();
    expect(page.text()).toContain("Checking the result of your last submission");
    const retry = page.button("Retry original request");
    expect(retry).not.toBeNull();
    controlEvent(retry, "onClick");
    await settle();
    expect(writes).toBe(2);
    expect(bodies[1]).toBe(bodies[0]);
    expect(page.text()).toContain(CODE);
    expect(page.storage.get(`hv.consignment.pending.${ALICE.user_id}`)).toBe("null");
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("非可信 5xx 仍只允许原命令原样重送", async () => {
  let current: unknown = { state: "none" };
  let writes = 0;
  const bodies: string[] = [];
  const transport = fakeFetch((request) => {
    if (request.url.pathname === OWNED) {
      return response(listing("awaiting_deposit", { version: 3 }));
    }
    if (request.url.pathname === INSTRUCTIONS) return response(current);
    if (request.url.pathname === RESERVATIONS) {
      writes += 1;
      bodies.push(String(request.init?.body));
      if (writes === 1) return failure(500, "unknown_error");
      current = active();
      const body = write(request);
      return Response.json(
        {
          code: "created",
          message: "",
          data: {
            schema_version: 2,
            operation: "issue_reservation",
            disposition: "reservation_issued",
            command_id: body.command_id,
            listing_id: LISTING_A,
            listing_version: 4,
            state: "awaiting_deposit",
            reservation_id: RESERVATION,
            expires_at: "2099-01-01T00:00:00Z",
          },
        },
        { status: 201 },
      );
    }
    throw new Error(`未期望请求 ${request.url.pathname}`);
  });
  const page = await mount();
  try {
    await settle();
    controlEvent(page.button("Create delivery instructions"), "onClick");
    await settle();
    expect(page.text()).toContain("Retry original request");
    controlEvent(page.button("Retry original request"), "onClick");
    await settle();
    expect(writes).toBe(2);
    expect(bodies[1]).toBe(bodies[0]);
    expect(page.text()).toContain(CODE);
  } finally {
    page.dispose();
    transport.restore();
  }
});

test("可信 mail_unavailable 结束首次提交，不保留伪未决", async () => {
  const transport = fakeFetch((request) => {
    if (request.url.pathname === OWNED) {
      return response(listing("awaiting_deposit", { version: 3 }));
    }
    if (request.url.pathname === INSTRUCTIONS) return response({ state: "none" });
    if (request.url.pathname === RESERVATIONS) return failure(503, "mail_unavailable");
    throw new Error(`未期望请求 ${request.url.pathname}`);
  });
  const page = await mount();
  try {
    await settle();
    controlEvent(page.button("Create delivery instructions"), "onClick");
    await settle();
    expect(page.text()).toContain("The mail account cannot accept consignments right now.");
    expect(page.storage.get(`hv.consignment.pending.${ALICE.user_id}`)).toBe("null");
    expect(page.button("Retry original request")).toBeNull();
  } finally {
    page.dispose();
    transport.restore();
  }
});
