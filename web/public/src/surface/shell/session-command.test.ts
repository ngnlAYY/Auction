// 会话对账通过时，实际竞价宿主保留已发出的冻结命令。

import { expect, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { openRouter } from "@auction/web-shared/router/test-window";
import {
  children,
  createComponent,
  createEffect,
  createRoot,
  For,
  flush,
  untrack,
  useContext,
} from "solid-js";
import {
  invalidateCookieSync,
  loginPublic,
  registerPublicSessionCheckTrigger,
} from "../../api/session";
import { publicWatchCacheRevision } from "../../api/session/revision";
import { PublicRefreshGenerationContext } from "../board/refresh";
import {
  findElement,
  loadCommandScopeFixture,
  TEST_AUCTION,
} from "../catalog/command-scope-fixture.test";
import { CompetitionCommandsContext } from "../commands/host";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import { load, save } from "../watches/cache";
import { ALICE, BOB, fixture, loginReceipt, reject } from "./session-fixture";

const LOT_ID = "67e55044-10b1-426f-9247-bb680e5fe0c8";
const AUCTION_ID = "00000000-0000-4000-8000-000000000021";
const WATCHES = {
  user_id: ALICE.user_id,
  auction_id: AUCTION_ID,
  watches: [{ lot_id: LOT_ID, watched: true, channel: "mm", version: 1 }],
};
const {
  scope: { CatalogCommandScope, CommandDialogContext },
} = await loadCommandScopeFixture();

function mountBid(mounted: ReturnType<typeof fixture>) {
  // 该宿主和看板共用同一身份键；用实际表单提交与重试。
  return createRoot((dispose) => {
    const tree = children(() =>
      createComponent(PublicSessionResourceContext, {
        value: mounted.session,
        get children() {
          return createComponent(PublicCommandOwnerContext, {
            value: mounted.commandOwner,
            get children() {
              return createComponent(CompetitionCommandsContext, {
                value: mounted.competition,
                get children() {
                  return createComponent(PublicRefreshGenerationContext, {
                    value: mounted.refreshGeneration,
                    get children() {
                      return For({
                        get each() {
                          return [mounted.commandOwner()];
                        },
                        keyed: (userId) => userId,
                        children: (userId) => {
                          const hosted = untrack(userId);
                          return createComponent(CatalogCommandScope, {
                            userId: hosted,
                            auction: TEST_AUCTION,
                            auctionInvalid: false,
                            get children() {
                              const dialog = useContext(CommandDialogContext);
                              if (dialog !== null && hosted === ALICE.user_id) {
                                dialog.set({
                                  lotId: LOT_ID,
                                  lotCode: "One12",
                                  lotName: "测试装备",
                                  action: "bid",
                                  canBid: true,
                                  currentPrice: 1000,
                                  nextBid: 2000,
                                  lotStatus: "live",
                                  allowSlow: true,
                                  proxyDefaultMode: "fast",
                                  ordinaryBlocked: false,
                                  leading: false,
                                  blockedReason: "",
                                  proxy: null,
                                  proxyReadGeneration: null,
                                });
                              }
                              return null;
                            },
                          });
                        },
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
    createEffect(tree, () => {});
    flush();
    return {
      dispose,
      form: () => findElement(tree(), (element) => element.tag === "form"),
      retry: () =>
        findElement(
          tree(),
          (element) =>
            element.tag === "button" &&
            String(element.attributes["aria-describedby"]).includes("public-command-dialog-"),
        ),
    };
  });
}

async function settle(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

async function submitUncertainBid(bid: ReturnType<typeof mountBid>) {
  const form = bid.form();
  if (form === null) throw new Error("实际宿主未挂载竞价表单");
  form.submit();
  flush();
  form.submit();
  await settle();
  const retry = bid.retry();
  expect(retry).not.toBeNull();
  expect(retry).not.toBe(form);
  return retry;
}

test.each([false, true])(
  "同账号复核后已挂载的竞价宿主保留原样重试（另一标签页 %s）",
  async (otherTab) => {
    const mounted = fixture(ALICE);
    const storage = window.localStorage;
    const opened = openRouter("/");
    Object.assign(opened.fake, { isSecureContext: true, localStorage: storage });
    const bodies: string[] = [];
    const stop = otherTab ? registerPublicSessionCheckTrigger(() => {}) : () => {};
    mounted.server.mutate = (path, method, init) => {
      if (path === "/api/v1/session" && method === "POST") {
        return loginReceipt(ALICE);
      }
      if (path === `/api/v1/lots/${LOT_ID}/bids` && typeof init?.body === "string") {
        bodies.push(init.body);
        return reject("storage_unavailable", 503);
      }
      throw new Error(`意外的请求 ${method} ${path}`);
    };
    try {
      await settle();
      const bid = mountBid(mounted);
      try {
        const retry = await submitUncertainBid(bid);
        expect(bodies).toHaveLength(1);
        const first = bodies[0];
        if (first === undefined) throw new Error("未发出竞价请求");
        expect(first).toMatch(/"command_id":"[0-9a-f-]+"/u);

        expect(await loginPublic("one-use", mounted.signal)).toMatchObject({ ok: true });
        if (otherTab) {
          mounted.browser.notifySessionRevision();
        }
        await settle();
        expect(mounted.owners).toEqual([null, ALICE.user_id]);
        expect(bid.retry()).toBe(retry);
        bid.retry()?.click();
        await settle();
        expect(bodies).toEqual([first, first]);

        mounted.server.identity = otherTab ? null : BOB;
        mounted.checkSession("refresh");
        await settle();
        expect(bid.retry()).toBeNull();
      } finally {
        bid.dispose();
      }
    } finally {
      stop();
      opened.disposeRouter();
      opened.restore();
      mounted.dispose();
    }
  },
);

test.each([
  ["登录换号后读回失败", "POST", BOB],
  ["权威读取换号后同步失败", "GET", BOB],
  ["权威读取同账号后同步失败", "GET", ALICE],
] as const)("%s 时按已解码证据归属已挂载的竞价宿主", async (_name, firstMethod, next) => {
  const mounted = fixture(ALICE);
  const storage = window.localStorage;
  const opened = openRouter("/");
  Object.assign(opened.fake, { isSecureContext: true, localStorage: storage });
  const failed = Promise.withResolvers<Response>();
  const unavailable = () => reject("storage_unavailable", 503);
  const expectedCache = next === ALICE ? WATCHES : null;
  try {
    await settle();
    expect(mounted.session()).toEqual(ok(ALICE));
    save(WATCHES, publicWatchCacheRevision());
    mounted.server.mutate = (path, method) => {
      if (path === `/api/v1/lots/${LOT_ID}/bids` && method === "POST") return unavailable();
      if (path === "/api/v1/session" && method === "POST") return loginReceipt(next);
      throw new Error(`意外请求 ${method} ${path}`);
    };
    const bid = mountBid(mounted);
    try {
      const retry = await submitUncertainBid(bid);
      const from = mounted.requests.length;
      mounted.server.identity = next;
      if (firstMethod === "POST") {
        mounted.server.read = () => failed.promise;
        expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(next));
      } else {
        invalidateCookieSync();
        mounted.server.synchronize = () => failed.promise;
        mounted.checkSession("refresh");
      }
      await settle();
      expect(mounted.requests.slice(from).map(({ method }) => method)).toEqual(
        firstMethod === "POST" ? ["POST", "GET"] : ["GET", "PUT"],
      );
      expect(mounted.commandOwner()).toBe(next.user_id);
      expect(bid.retry()).toBe(next === ALICE ? retry : null);
      expect(load(ALICE.user_id, AUCTION_ID)).toEqual(expectedCache);

      failed.resolve(unavailable());
      await settle();
      expect(mounted.session()).toEqual(err("storage_unavailable"));
      expect(mounted.commandOwner()).toBe(next.user_id);
      expect(bid.retry()).toBe(next === ALICE ? retry : null);
      expect(load(ALICE.user_id, AUCTION_ID)).toEqual(expectedCache);
    } finally {
      bid.dispose();
    }
  } finally {
    failed.resolve(unavailable());
    opened.disposeRouter();
    opened.restore();
    mounted.dispose();
  }
});
