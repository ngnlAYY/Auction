// 公开寄售的离线响应、响应式挂载、可控时钟和浏览器事件替身。

import type {
  ConsignmentStatus,
  ListingDetail,
  ListingSummary,
} from "@auction/contracts/dto/public/consignment/catalog";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { buildBundle } from "@auction/web-shared/ui/i18n/resource";
import { createLocaleSession } from "@auction/web-shared/ui/i18n/session";
import { children, createComponent, createEffect, createRoot, type Element, flush } from "solid-js";

import { PublicSessionResourceContext, type PublicSessionResult } from "..";
import { findElement, TestElement } from "../catalog/command-scope-fixture.test";
import { PUBLIC_ROUTES } from "../router/route";

export const LISTING_A = "00000000-0000-4000-8000-0000000000a1";
export const LISTING_B = "00000000-0000-4000-8000-0000000000b2";
export const STATUS_PATH = "/api/v1/consignment/status";
export const LISTINGS_PATH = "/api/v1/consignment/listings";
const TIME = "2026-11-20T00:00:00Z";

export function status(enabled: boolean, ready = enabled): ConsignmentStatus {
  return {
    enabled,
    registration_ready: ready,
    registration_modes: ready ? ["direct", "consigned"] : [],
    registration_worlds: ready ? ["main", "isekai"] : [],
    fee_bps: 500,
    min_price: 1,
    max_price: 1_000_000_000,
    accepts_mail: false,
    purchasable: false,
  };
}

export function summary(listingId = LISTING_A): ListingSummary {
  const direct = listingId === LISTING_A;
  return {
    listing_id: listingId,
    version: 2,
    mode: direct ? "direct" : "consigned",
    state: direct ? "displayed" : "awaiting_deposit",
    world_id: direct ? "main" : "isekai",
    external_equipment_id: direct ? "42" : "43",
    name: direct ? "Fake Oak Staff" : "Fake Iron Shield",
    category: direct ? "Staff" : "Shield",
    level: direct ? { kind: "assigned", value: 42 } : { kind: "unassigned" },
    price: direct ? 1234 : 9999,
    seller_name: direct ? "Fake Seller" : "Fake Trader",
    updated_at: TIME,
    expires_at: TIME,
    purchasable: false,
  };
}

export function detail(listingId = LISTING_A): ListingDetail {
  const listing = summary(listingId);
  return {
    summary: listing,
    equipment: {
      name: listing.name,
      alt_name: "Fake alternative name",
      category: listing.category,
      level: listing.level,
      tradeable: true,
      source: "hvdata-equip-v1",
      observed_at: TIME,
      stat_groups: [
        {
          name: "Fake offense",
          stats: [
            { name: "Accuracy", value: 3.5, base: null },
            { name: "Damage", value: -2.25, base: 0 },
          ],
        },
      ],
    },
  };
}

export function response(data: unknown): Response {
  return Response.json({ code: "ok", message: "ok", data });
}

export function failure(http: number, code: string): Response {
  return Response.json(
    { code, message: "fake server message", data: null, request_id: null },
    { status: http },
  );
}

export interface RequestRecord {
  readonly url: URL;
  readonly init: RequestInit | undefined;
}

export function fakeFetch(handle: (request: RequestRecord) => Response | Promise<Response>) {
  const old = globalThis.fetch;
  const requests: RequestRecord[] = [];
  globalThis.fetch = Object.assign(
    async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(input.toString(), "https://auction.test");
      const request = { url, init };
      requests.push(request);
      return handle(request);
    },
    { preconnect: old.preconnect },
  );
  return {
    requests,
    restore: () => {
      globalThis.fetch = old;
    },
  };
}

export function pageResponse(url: URL, items: ListingSummary[], total = items.length): Response {
  return response({
    items,
    total,
    page: Number(url.searchParams.get("page")),
    page_size: Number(url.searchParams.get("page_size")),
  });
}

export async function settle(): Promise<void> {
  for (let step = 0; step < 20; step += 1) {
    flush();
    await Promise.resolve();
  }
  flush();
}

function replaceGlobal(
  key: "document" | "setTimeout" | "clearTimeout",
  value: unknown,
): () => void {
  const previous = Object.getOwnPropertyDescriptor(globalThis, key);
  Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  return () => {
    if (previous === undefined) Reflect.deleteProperty(globalThis, key);
    else Object.defineProperty(globalThis, key, previous);
  };
}

export function fakeClock() {
  let now = 0;
  let nextId = 0;
  const pending = new Map<number, { at: number; callback: () => void }>();
  const restoreTimeout = replaceGlobal("setTimeout", (callback: () => void, delay = 0) => {
    nextId += 1;
    pending.set(nextId, { at: now + delay, callback });
    return nextId;
  });
  const restoreClear = replaceGlobal("clearTimeout", (timerId: number) => pending.delete(timerId));
  return {
    async advance(millis: number) {
      const until = now + millis;
      for (;;) {
        const next = [...pending]
          .filter(([, timer]) => timer.at <= until)
          .sort((left, right) => left[1].at - right[1].at)[0];
        if (next === undefined) break;
        const [timerId, timer] = next;
        pending.delete(timerId);
        now = timer.at;
        timer.callback();
        await settle();
      }
      now = until;
    },
    activeIn(millis: number) {
      return [...pending.values()].filter((timer) => timer.at - now === millis).length;
    },
    restore() {
      restoreTimeout();
      restoreClear();
    },
  };
}

export function mountConsignment(initialUrl: string, render: () => Element, authenticated = false) {
  const installed = installFakeWindow(initialUrl);
  const doc = Object.assign(new EventTarget(), { hidden: false });
  const restoreDocument = replaceGlobal("document", doc);
  const session = createCell<PublicSessionResult | undefined>(
    ok(
      authenticated
        ? {
            authenticated: true,
            username: "Fake Seller",
            user_id: "00000000-0000-4000-8000-000000000012",
            expires_at: TIME,
            absolute_expires_at: TIME,
          }
        : null,
    ),
  );

  const ftl = Bun.file(`${import.meta.dir}/../../../../../crates/auction-web-public/i18n/en.ftl`);
  const mount = async () => {
    const bundle = buildBundle("en", await ftl.text(), "en", null);
    if (!bundle.ok) throw new Error("测试译文加载失败");
    return createRoot((dispose) => {
      createLocaleSession(bundle.value, "en");
      const router = createRouter({ routes: PUBLIC_ROUTES });
      const tree = children(() =>
        createComponent(RouterContext, {
          value: router,
          get children() {
            return createComponent(PublicSessionResourceContext, {
              value: Object.assign(session, {
                state: () => "ready" as const,
                restart() {},
                cancel() {},
                clear: () => session.set(undefined),
              }),
              get children() {
                return render();
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
      return { dispose, router, tree };
    });
  };

  let disposed = false;
  return mount().then(
    ({ dispose, router, tree }) => ({
      router,
      location: installed.fake.location,
      go(href: string) {
        router.push(href);
        flush();
      },
      back() {
        installed.fake.history.back();
        flush();
      },
      focus() {
        installed.fake.dispatchEvent(new Event("focus"));
      },
      setHidden(hidden: boolean) {
        doc.hidden = hidden;
        doc.dispatchEvent(new Event("visibilitychange"));
      },
      text: () => DOMText(tree()),
      find: (match: (node: TestElement) => boolean) => findElement(tree(), match),
      dispose() {
        if (disposed) return;
        disposed = true;
        dispose();
        flush();
        restoreDocument();
        installed.restore();
      },
    }),
    (error: unknown) => {
      restoreDocument();
      installed.restore();
      throw error;
    },
  );
}

export function DOMText(node: unknown): string {
  if (typeof node === "function") return DOMText(node());
  if (Array.isArray(node)) return node.map(DOMText).join("");
  if (node instanceof TestElement) {
    return node.tag === "#text" ? String(node.attributes.text) : node.nodes.map(DOMText).join("");
  }
  return typeof node === "string" || typeof node === "number" ? String(node) : "";
}

export function controlEvent(node: TestElement | null, name: string, value = ""): void {
  const handler = node?.attributes[name];
  if (typeof handler !== "function") throw new Error(`未绑定测试事件 ${name}`);
  handler({ currentTarget: { value }, button: 0, preventDefault() {} });
  flush();
}
