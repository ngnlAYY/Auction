// 命令宿主的身份判定与新提交条件。

import { expect, spyOn, test } from "bun:test";
import { SecondTickerContext, useSecondTicker } from "@auction/web-shared/platform/clock";
import { createPageVisibility } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource, type ResourceState } from "@auction/web-shared/reactive/resource";
import { err, ok } from "@auction/web-shared/result";
import { RouterContext } from "@auction/web-shared/router";
import { openRouter } from "@auction/web-shared/router/test-window";
import {
  type Context,
  children,
  createComponent,
  createEffect,
  createRoot,
  type Element,
  flush,
  useContext,
} from "solid-js";

import {
  PublicCommandOwnerContext,
  PublicContentLoadingContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "..";
import { PublicDisplayedAuctionContext, PublicPageRealtimeTargetContext } from "../board/realtime";
import { PublicBidRefreshContext, PublicRefreshGenerationContext } from "../board/refresh";
import { CompetitionCommandsContext, createCompetitionCommands } from "../commands/host";
import { usePublicQueryState } from "../query";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  SelectedWorldContext,
} from "../shell/contexts";
import { findElement, loadCommandScopeFixture, TEST_AUCTION } from "./command-scope-fixture.test";

const {
  board: { mountAuctionBoard },
  scope: { CommandDialogContext, catalogCommandSession },
} = await loadCommandScopeFixture();

const USER_ID = "018f4c8e-7b2a-7c3d-8e4f-000000000001";
const OTHER_USER_ID = "018f4c8e-7b2a-7c3d-8e4f-000000000002";
const LOT_ID = "67e55044-10b1-426f-9247-bb680e5fe0c8";

function authenticated(userId: string): PublicSessionResult {
  return ok({
    authenticated: true,
    user_id: userId,
    username: "viewer",
    expires_at: null,
    absolute_expires_at: null,
  });
}

function provide<T>(context: Context<T>, value: T) {
  return (render: () => Element) => () =>
    createComponent(context, {
      value,
      get children() {
        return render();
      },
    });
}

interface BidRequest {
  readonly body: string;
  readonly signal: AbortSignal;
  readonly respond: (response: Response) => void;
}

function mountBidBoard() {
  const opened = openRouter("/?auction=truncated");
  Object.assign(opened.fake, { setInterval: () => 0, clearInterval() {} });
  const requests: BidRequest[] = [];
  const fetch = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      (url: RequestInfo | URL, init?: RequestInit) => {
        if (typeof url === "string" && url.startsWith("/api/v1/browser-errors?")) {
          return Promise.resolve(new Response(null, { status: 204 }));
        }
        if (typeof url === "string" && url.startsWith("/api/v1/auctions/")) {
          return Promise.resolve(new Response(null, { status: 503 }));
        }
        const signal = init?.signal;
        if (
          url !== `/api/v1/lots/${LOT_ID}/bids` ||
          typeof init?.body !== "string" ||
          signal == null
        ) {
          throw new Error(`意外的竞价请求 ${String(url)}`);
        }
        const { promise, resolve, reject } = Promise.withResolvers<Response>();
        const abort = () => reject(signal.reason);
        signal.addEventListener("abort", abort, { once: true });
        requests.push({
          body: init.body,
          signal,
          respond(response) {
            signal.removeEventListener("abort", abort);
            resolve(response);
          },
        });
        return promise;
      },
      { preconnect: () => {} },
    ),
  );
  const mounted = createRoot((dispose) => {
    const session: PublicSessionResource = Object.assign(
      createCell<PublicSessionResult | undefined>(authenticated(USER_ID)),
      {
        state: (): ResourceState => "ready",
        restart: () => {},
        cancel: () => {},
        clear: () => {},
      },
    );
    const refresh = createCell(0);
    const commands = createCompetitionCommands(session, refresh);
    commands.replaceIdentity(USER_ID);
    const providers = [
      provide(RouterContext, opened.router),
      provide(PublicSessionResourceContext, session),
      provide(PublicCommandOwnerContext, createCell<string | null>(USER_ID)),
      provide(CompetitionCommandsContext, commands),
      provide(PublicContentLoadingContext, createCell<ReadonlySet<number>>(new Set())),
      provide(SelectedWorldContext, createCell("main")),
      provide(
        CurrentAuctionContext,
        createTaskResource(
          () => null,
          async () => ["main", ok(TEST_AUCTION)] as const,
        ),
      ),
      provide(
        PublicReadinessContext,
        createTaskResource(
          () => null,
          async () => ok(true),
        ),
      ),
      provide(PublicPageRealtimeTargetContext, createCell(null)),
      provide(PublicDisplayedAuctionContext, createCell(null)),
      provide(SecondTickerContext, useSecondTicker()),
      provide(PageVisibilityContext, createPageVisibility()),
      provide(PublicRefreshGenerationContext, refresh),
      provide(PublicBidRefreshContext, { revision: createCell(0), notify() {} }),
    ];
    let open = (): void => {
      throw new Error("目录替身未挂载");
    };
    const view = () =>
      mountAuctionBoard(usePublicQueryState().state, (props) => {
        const dialog = useContext(CommandDialogContext);
        if (dialog === null) {
          throw new Error("目录未挂载到实际竞价宿主");
        }
        open = () => {
          if (!props.invalid) {
            dialog.set({
              lotId: LOT_ID,
              lotCode: "One12",
              lotName: "Test Lot",
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
            });
          }
        };
        return null;
      });
    const tree = children(providers.reduceRight((child, wrap) => wrap(child), view));
    createEffect(tree, () => {});
    flush();
    return {
      dispose,
      open: () => open(),
      form: () => findElement(tree(), (element) => element.tag === "form"),
      retry: () =>
        findElement(
          tree(),
          (element) =>
            element.tag === "button" &&
            String(element.attributes["aria-describedby"]).includes("public-command-dialog-"),
        ),
      refresh,
    };
  });
  return {
    requests,
    router: opened.router,
    get refreshes() {
      return mounted.refresh.peek();
    },
    async openFromCurrent() {
      opened.router.push("/");
      await Bun.sleep(0);
      flush();
      mounted.open();
      flush();
      const form = mounted.form();
      if (form === null) {
        throw new Error("实际宿主没有显示竞价表单");
      }
      return form;
    },
    back() {
      opened.fake.history.back();
      flush();
    },
    form: mounted.form,
    retry: mounted.retry,
    close() {
      mounted.dispose();
      flush();
      opened.disposeRouter();
      opened.restore();
      fetch.mockRestore();
    },
  };
}

test("合法地址打开实际竞价弹窗，后退到非法场次后旧确认不能发请求", async () => {
  const mounted = mountBidBoard();
  try {
    const form = await mounted.openFromCurrent();
    form.submit();
    flush();
    expect(mounted.requests).toHaveLength(0);
    const confirm = findElement(
      form,
      (element) => element.tag === "button" && element.attributes.type === "submit",
    );
    expect(confirm?.attributes.autofocus).toBe(true);
    expect(confirm?.attributes.disabled).toBe(false);
    mounted.back();
    expect(mounted.router.currentLocationString()).toBe("/?auction=truncated");
    expect(confirm?.attributes.disabled).toBe(true);
    // 即使旧 DOM 的提交事件已排队，实际表单处理器仍须失败关闭。
    form.submit();
    flush();
    expect(mounted.requests).toHaveLength(0);
  } finally {
    mounted.close();
  }
});

test("实际宿主跨后退保留在途竞价和原样重试入口", async () => {
  const mounted = mountBidBoard();
  try {
    const form = await mounted.openFromCurrent();
    form.submit();
    flush();
    form.submit();
    await Bun.sleep(0);
    const first = mounted.requests[0];
    expect(first?.body).toMatch(/"command_id":"[0-9a-f-]+"/u);
    expect(first?.body).toContain('"amount":2000');
    mounted.back();
    expect(mounted.router.currentLocationString()).toBe("/?auction=truncated");
    expect(first?.signal.aborted).toBe(false);
    first?.respond(new Response(null, { status: 503 }));
    await Bun.sleep(0);
    flush();
    expect(mounted.refreshes).toBe(1);

    const retry = mounted.retry();
    if (retry === null) {
      throw new Error("非法场次的实际命令宿主丢失了重试按钮");
    }
    retry.click();
    await Bun.sleep(0);
    const replayed = mounted.requests[1];
    expect(mounted.requests).toHaveLength(2);
    expect(replayed?.body).toBe(first?.body);
    replayed?.respond(
      Response.json({
        code: "ok",
        message: "ok",
        data: {
          bid: {
            lot_id: LOT_ID,
            current_price: 2000,
            effective_end_at: "2026-11-18T01:00:00Z",
            state_version: 2,
            leading: true,
          },
        },
      }),
    );
    await Bun.sleep(0);
    flush();
    expect(mounted.refreshes).toBe(2);
    expect(mounted.form()).toBeNull();
  } finally {
    mounted.close();
  }
});

test("同身份复核时保留命令表单，复核成功后恢复新提交", () => {
  const snapshot = authenticated(USER_ID);
  expect(catalogCommandSession(snapshot, false, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: false,
  });
  expect(catalogCommandSession(snapshot, true, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: true,
  });
});

test("暂时读取失败仍保留已确认身份的草稿，暂停新提交", () => {
  expect(catalogCommandSession(err("network_error"), true, USER_ID, USER_ID)).toEqual({
    authenticated: true,
    canSubmit: false,
  });
});

test("匿名、不同身份、从未确认的会话都不能启用表单或提交", () => {
  for (const [snapshot, confirmed] of [
    [ok(null), USER_ID],
    [authenticated(OTHER_USER_ID), USER_ID],
    [undefined, null],
    [authenticated(USER_ID), null],
  ] as const) {
    expect(catalogCommandSession(snapshot, true, USER_ID, confirmed)).toEqual({
      authenticated: false,
      canSubmit: false,
    });
  }
});
