// 竞价回归使用生产外壳、路由、目录和表单；仅替换 DOM、时钟等待和 HTTP。

import { spyOn } from "bun:test";
import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";
import { invalidateCookieSync, type PublicSession } from "../../api/session";
import { resetStoreForTest } from "../../api/swr-cache";
import { type DomElement, mountPublicApp } from "../shell/navigation-fixture.test";
import { ALICE, loginReceipt, reject, success } from "../shell/session-fixture";

export const AUCTION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2b";
export const LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2c";
export const SECOND_LOT_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2d";
export const AUTHORIZATION_ID = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2e";
export const END_AT = "2037-12-31T08:00:00Z";
const OPEN_AT = "2025-11-15T08:00:00Z";

function lot(id: string, ended: boolean, detail: boolean) {
  return {
    id,
    auction_id: AUCTION_ID,
    lot_number: id === LOT_ID ? 1 : 2,
    status: ended ? "sold" : "live",
    start_price: 1000,
    current_price: 1500,
    next_bid: 2000,
    has_leader: true,
    leader_name: "Leader",
    seller_name: "Seller",
    effective_end_at: END_AT,
    state_version: 3,
    equipment: {
      name: id === LOT_ID ? "Legendary Cotton Robe" : "Magnificent Cotton Robe",
      category: "Cloth",
      equipment_group: "rare",
      level: 450,
      primary_attribute_initials: "IW",
      avg_roll: 70,
      game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
      ...(detail ? { info_summary: "Robe Lv450", stats: {} } : {}),
    },
  };
}

function envelope() {
  return {
    auction_id: AUCTION_ID,
    auction_status: "live",
    projection_kind: "live",
    presentation_mode: "live",
    bidding_open: true,
    opens_at: OPEN_AT,
    preview_revision: 0,
  };
}

export function lotDetailReceipt(
  id: string,
  price = 1500,
  leader = "Leader",
  settledAuction: string | null = null,
): Response {
  return success({
    ...envelope(),
    ...(settledAuction !== null
      ? { auction_id: settledAuction, auction_status: "settled", bidding_open: false }
      : {}),
    lot: {
      ...lot(id, settledAuction !== null, true),
      auction_id: settledAuction ?? AUCTION_ID,
      current_price: price,
      next_bid: price + 500,
      leader_name: leader,
    },
  });
}

export function bidReceipt(lotId = LOT_ID, price = 2000): Response {
  return success({
    bid: {
      lot_id: lotId,
      current_price: price,
      effective_end_at: END_AT,
      state_version: 4,
      leading: true,
    },
  });
}

function proxyProjection(maximum: number, lowerFrom: number | null, mode = "fast") {
  const created = {
    transition_number: 1,
    kind: "created",
    old_maximum_amount: null,
    new_maximum_amount: lowerFrom ?? maximum,
    old_mode: null,
    new_mode: mode,
    old_status: null,
    new_status: "active",
    reason: null,
    resulting_state_version: 1,
    created_at: OPEN_AT,
  };
  const transitions =
    lowerFrom === null
      ? [created]
      : [
          created,
          {
            ...created,
            transition_number: 2,
            kind: "lowered",
            old_maximum_amount: lowerFrom,
            new_maximum_amount: maximum,
            old_status: "active",
            old_mode: mode,
            resulting_state_version: 2,
          },
        ];
  return {
    authorization_id: AUTHORIZATION_ID,
    lot_id: LOT_ID,
    authorization_sequence: 1,
    usage_number: 1,
    mode,
    maximum_amount: maximum,
    quota: { used: 1, limit: 3 },
    transitions,
    status: "active",
    state_version: transitions.length,
  };
}

export function proxyReceipt(kind: "create" | "lower"): Response {
  return success({
    authorization: {
      ...proxyProjection(2000, kind === "lower" ? 5000 : null, kind === "create" ? "slow" : "fast"),
      ...(kind === "create"
        ? {
            bid: {
              lot_id: LOT_ID,
              current_price: 2000,
              effective_end_at: END_AT,
              state_version: 4,
              leading: true,
            },
          }
        : {}),
    },
  });
}

export function ownedProxy(waiting: boolean) {
  return {
    ...proxyProjection(5000, null, waiting ? "slow" : "fast"),
    auction_id: AUCTION_ID,
    lot_number: 1,
    created_at: OPEN_AT,
    updated_at: OPEN_AT,
    equipment: { name: "Legendary Cotton Robe", category: "Cloth" },
    bid: {
      id: "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a2f",
      auction_id: AUCTION_ID,
      lot_id: LOT_ID,
      lot_number: 1,
      amount: 1500,
      fact_index: 1,
      accepted_state_version: 3,
      mechanism: "proxy",
      created_at: OPEN_AT,
      leading: !waiting,
      equipment: { name: "Legendary Cotton Robe", category: "Cloth" },
    },
    capabilities: { can_lower: true, waiting, at_maximum: false },
  };
}

export interface SentRequest {
  readonly path: string;
  readonly body: string;
  readonly signal: AbortSignal;
  readonly expectedUser: string | null;
  respond(response: Response): void;
}

/** 将真实表单送到实际 fetch 边界，未答复的写入可独立观察取消。 */
export async function mountCompetition(
  world = "main",
  currentEmpty = false,
  initial?: {
    readonly href: string;
    readonly storage?: ReadonlyMap<string, string>;
    readonly rejectHistoryWrites?: boolean;
    readonly readCurrent?: (url: URL) => Response | Promise<Response> | null;
    readonly readDirectory?: (url: URL) => Response | Promise<Response> | null;
  },
) {
  invalidateCookieSync();
  resetStoreForTest();
  const commands: SentRequest[] = [];
  const reads: string[] = [];
  const server = {
    identity: ALICE as PublicSession | null,
    nextLogin: ALICE,
    readSession: (): Response | Promise<Response> => success(server.identity),
    syncSession: (): Response | Promise<Response> => success(server.identity),
    logout: (): Response | Promise<Response> => {
      server.identity = null;
      return success(null);
    },
    currentWorld: world,
    currentEmpty,
    readCurrent: initial?.readCurrent ?? ((_url: URL): Response | Promise<Response> | null => null),
    readRounds: (_url: URL, _init?: RequestInit): Response | Promise<Response> | null => null,
    readSummary: (): Response | Promise<Response> => reject("not_found", 404),
    readEvents: (_url: URL): Response | Promise<Response> | null => null,
    readBidLog: (_url: URL): Response | Promise<Response> | null => null,
    readPage: (_url: URL, _init?: RequestInit): Response | Promise<Response> | null => null,
    directoryFailed: false,
    readDirectory:
      initial?.readDirectory ?? ((_url: URL): Response | Promise<Response> | null => null),
    lotEnded: false,
    activeProxy: false,
    waitingProxy: false,
    readProxy: (_url: URL): Response | Promise<Response> | null => null,
    readLot: (id: string, _signal?: AbortSignal | null): Response | Promise<Response> =>
      lotDetailReceipt(id),
  };
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const url = new URL(String(input), "https://auction.test");
        const method = init?.method ?? "GET";
        if (method === "POST" && url.pathname.startsWith("/api/v1/lots/")) {
          const signal = init?.signal;
          if (typeof init?.body !== "string" || signal == null) {
            throw new Error("竞价请求缺少正文或取消信号");
          }
          const { promise, resolve, reject } = Promise.withResolvers<Response>();
          const abort = () => reject(signal.reason);
          signal.addEventListener("abort", abort, { once: true });
          commands.push({
            path: url.pathname,
            body: init.body,
            signal,
            expectedUser: new Headers(init.headers).get(PUBLIC_EXPECTED_USER_ID_HEADER),
            respond(response) {
              signal.removeEventListener("abort", abort);
              resolve(response);
            },
          });
          return promise;
        }
        reads.push(url.pathname + url.search);
        switch (url.pathname) {
          case "/health/ready":
            return success({ ready: true });
          case "/api/v1/session":
            if (method === "POST") {
              server.identity = server.nextLogin;
              return loginReceipt(server.nextLogin);
            }
            if (method === "DELETE") {
              return server.logout();
            }
            return method === "PUT" ? server.syncSession() : server.readSession();
          case "/api/v1/worlds":
            return success({ worlds: { main: { enabled: true }, isekai: { enabled: true } } });
          case "/api/v1/auctions/current":
            return (
              server.readCurrent(url) ??
              success({
                auction:
                  !server.currentEmpty && url.searchParams.get("world") === server.currentWorld
                    ? {
                        id: AUCTION_ID,
                        world: server.currentWorld,
                        number: 12,
                        status: "live",
                        opens_at: OPEN_AT,
                        scheduled_ends_at: END_AT,
                        presentation_mode: "live",
                        bidding_open: true,
                        preview_revision: 0,
                        proxy_default_mode: "fast",
                      }
                    : null,
              })
            );
          case `/api/v1/auctions/${AUCTION_ID}/lots`: {
            const response = server.readDirectory(url);
            if (response !== null) return response;
            if (server.directoryFailed && url.searchParams.has("category")) {
              return reject("temporarily_unavailable", 503);
            }
            return success({
              ...envelope(),
              lots: [lot(LOT_ID, server.lotEnded, false), lot(SECOND_LOT_ID, false, false)],
              total: 2,
              page: 1,
              page_size: Number(url.searchParams.get("page_size")),
              category_counts: { all: 2, one: 0, two: 0, sta: 0, shd: 0, clo: 2, lig: 0, hea: 0 },
            });
          }
          case `/api/v1/lots/${LOT_ID}`:
          case `/api/v1/lots/${SECOND_LOT_ID}`:
            return server.readLot(url.pathname.split("/").at(-1) ?? "", init?.signal);
          case `/api/v1/auctions/${AUCTION_ID}/summary`:
            return server.readSummary();
          case "/api/v1/auctions":
            return (
              server.readRounds(url, init) ??
              success({
                auctions: [],
                page: 1,
                page_size: Number(url.searchParams.get("page_size")),
                total: 0,
              })
            );
          case "/api/v1/events":
            return server.readEvents(url) ?? reject("not_found", 404);
          case `/api/v1/lots/${LOT_ID}/proxy-authorizations/mine`:
          case `/api/v1/lots/${SECOND_LOT_ID}/proxy-authorizations/mine`:
            return (
              server.readProxy(url) ??
              success({
                authorization:
                  server.activeProxy && url.pathname.includes(LOT_ID)
                    ? ownedProxy(server.waitingProxy)
                    : null,
              })
            );
          case "/api/v1/me/lot-watches":
            return success({
              user_id: server.identity?.user_id,
              auction_id: AUCTION_ID,
              watches: [],
            });
          default:
            if (url.pathname.endsWith("/bid-log"))
              return server.readBidLog(url) ?? reject("not_found", 404);
            return server.readPage(url, init) ?? reject("not_found", 404);
        }
      },
      { preconnect: () => {} },
    ),
  );
  const mounted = await mountPublicApp(
    initial?.href ?? (world === "main" ? "/" : `/?world=${world}`),
    initial?.storage,
    initial?.rejectHistoryWrites,
  );
  return {
    ...mounted,
    commands,
    reads,
    server,
    need(match: (element: DomElement) => boolean, parent?: DomElement): DomElement {
      const node = mounted.find(match, parent);
      if (node === null) {
        throw new Error("实际页面没有挂载所需控件");
      }
      return node;
    },
    dispose() {
      mounted.dispose();
      fetcher.mockRestore();
      invalidateCookieSync();
      resetStoreForTest();
    },
  };
}

/** 给编译后的实际 DOM 元素分发可取消事件。 */
export function dispatch(element: DomElement, event: string): void {
  const handler = element.attributes[`on${event}`];
  if (typeof handler !== "function") throw new Error(`未接线的 ${event} 事件`);
  const dispatched = new Event(event.toLowerCase(), { cancelable: true });
  Object.defineProperties(dispatched, {
    currentTarget: { value: element },
    target: { value: element },
    button: { value: 0 },
  });
  handler(dispatched);
  flush();
}

/** 忽略 DOM 替身中没有布局的部分，读取实际插槽文本。 */
export function renderedText(node: unknown): string {
  if (typeof node === "function") return renderedText(node());
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(renderedText).join("");
  if (node === null || typeof node !== "object") return "";
  if (
    "tag" in node &&
    node.tag === "#text" &&
    "attributes" in node &&
    node.attributes !== null &&
    typeof node.attributes === "object" &&
    "text" in node.attributes
  )
    return renderedText(node.attributes.text);
  return "nodes" in node && Array.isArray(node.nodes) ? node.nodes.map(renderedText).join("") : "";
}

export function hasClass(element: DomElement, name: string): boolean {
  const classes = element.attributes.class;
  const names = Array.isArray(classes) ? classes : [classes];
  return names.some((value) => {
    return typeof value === "string"
      ? value.split(" ").includes(name)
      : value !== null && typeof value === "object" && Reflect.get(value, name) === true;
  });
}

export type CompetitionPage = Awaited<ReturnType<typeof mountCompetition>>;

/** 等候实际 DOM 或收包条件，避免把分块加载耗时写进断言。 */
export async function waitFor(condition: () => boolean, timeout = 2000): Promise<void> {
  const until = performance.now() + timeout;
  do {
    flush();
    if (condition()) return;
    await Bun.sleep(1);
  } while (performance.now() < until);
  throw new Error("等待页面或传输条件超时");
}

export async function openBid(page: CompetitionPage, code = "Clo01"): Promise<DomElement> {
  const match = (element: DomElement) =>
    element.tag === "button" &&
    element.getAttribute("aria-label") === t("catalog-bid-lot", { code });
  await waitFor(() => page.find(match) !== null);
  const button = page.need(match);
  dispatch(button, "Click");
  return button;
}

/** 提交实际渲染的两阶段表单，返回本次写请求。 */
export async function confirm(page: CompetitionPage, form: DomElement): Promise<SentRequest> {
  const index = page.commands.length;
  dispatch(form, "Submit");
  dispatch(form, "Submit");
  await waitFor(() => page.commands.length > index);
  const sent = page.commands[index];
  if (sent === undefined) throw new Error("确认没有发出竞价请求");
  return sent;
}

/** 等待真实路由分块、传输及渲染完成一轮。 */
export async function settle(): Promise<void> {
  for (let tick = 0; tick < 6; tick += 1) {
    await Bun.sleep(0);
    flush();
  }
}
