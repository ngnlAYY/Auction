// 用真实标记缓存读取验证会话变化前后的本地快照。

import { expect, test } from "bun:test";
import type { PublicLotWatchesResponse } from "@auction/contracts/dto/public/watch";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, ok } from "@auction/web-shared/result";
import { createRoot, For, flush } from "solid-js";
import { object } from "../../api/decoder";
import { fetchCurrentAuction } from "../../api/queries/current";
import { loginPublic, logoutPublic, PUBLIC_SESSION_CHANGE_STORAGE_KEY } from "../../api/session";
import {
  publicSessionIdentityRevision,
  publicSessionPrimaryRevision,
  publicWatchCacheRevision,
  registerPublicSessionCheckTrigger,
} from "../../api/session/revision";
import { fetchLotWatches } from "../../api/watches";
import { PublicRefreshGenerationContext } from "../board/refresh";
import { PublicCommandOwnerContext, PublicSessionResourceContext } from "../index";
import {
  ALICE,
  BOB,
  brokenBody,
  fixture,
  loginReceipt,
  reject,
  success,
} from "../shell/session-fixture";
import { LotWatchScope, WatchContext } from ".";
import { load, save } from "./cache";
import { useWatchCommandOwner, WatchCommandOwnerContext } from "./commands";
import { watchToggleState } from "./control";

const AUCTION_ID = "00000000-0000-4000-8000-000000000021";
const OTHER_AUCTION_ID = "00000000-0000-4000-8000-000000000022";
const LOT_ID = "00000000-0000-4000-8000-000000000023";
const OTHER_USER_ID = "00000000-0000-4000-8000-000000000003";
const STORAGE_KEY = `hv.public.lot-watches.v1:${ALICE.user_id}:${AUCTION_ID}`;
const CACHE_FIXTURE = new URL(
  "../../../../../crates/auction-web-public/src/surface/watches/cache_entry.v2.fixture.json",
  import.meta.url,
);
const PAGE: PublicLotWatchesResponse = {
  user_id: ALICE.user_id,
  auction_id: AUCTION_ID,
  watches: [{ lot_id: LOT_ID, watched: true, channel: "mm", version: 1 }],
};

async function settle(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

function mountWatchView(mounted: ReturnType<typeof fixture>) {
  const jsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (jsx === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  const oldSession = PublicSessionResourceContext.defaultValue;
  const oldOwner = PublicCommandOwnerContext.defaultValue;
  const oldRefresh = PublicRefreshGenerationContext.defaultValue;
  const oldWatchOwner = WatchCommandOwnerContext.defaultValue;
  let readMark: (() => boolean) | null = null;
  let readVersion: (() => readonly [boolean, number] | null) | null = null;
  PublicSessionResourceContext.defaultValue = mounted.session;
  PublicCommandOwnerContext.defaultValue = mounted.commandOwner;
  PublicRefreshGenerationContext.defaultValue = mounted.refreshGeneration;
  Object.defineProperty(globalThis, "jsx", {
    ...jsx,
    value(tag: unknown, props: Record<string, unknown> | null, child: unknown) {
      if (tag === For && Array.isArray(props?.each) && typeof child === "function") {
        const scope = props.each[0];
        return child(() => scope);
      }
      if (typeof tag === "function" && tag.name === "ScopedLotWatches") {
        return tag({ ...props, children: child });
      }
      const context = props?.value;
      if (
        tag === WatchContext &&
        context !== null &&
        typeof context === "object" &&
        "displayed" in context &&
        typeof context.displayed === "function" &&
        "authoritative" in context &&
        typeof context.authoritative === "function"
      ) {
        const displayed = context.displayed;
        readMark = () => displayed(LOT_ID) === true;
        const authoritative = context.authoritative;
        readVersion = () => {
          const snapshot: unknown = authoritative(LOT_ID);
          if (!Array.isArray(snapshot)) return null;
          const [watched, version] = snapshot;
          return typeof watched === "boolean" && typeof version === "number"
            ? [watched, version]
            : null;
        };
      }
      return null;
    },
  });
  try {
    return createRoot((dispose) => {
      const commands = useWatchCommandOwner(ALICE.user_id);
      WatchCommandOwnerContext.defaultValue = commands;
      LotWatchScope({ auctionId: AUCTION_ID, children: null });
      flush();
      return {
        dispose,
        commands,
        displayed() {
          if (readMark === null) {
            throw new Error("标记视图未装配");
          }
          return readMark();
        },
        toggle() {
          if (readVersion === null) throw new Error("标记视图未装配");
          return watchToggleState(commands, readVersion(), commands.commands().has(LOT_ID));
        },
      };
    });
  } finally {
    PublicSessionResourceContext.defaultValue = oldSession;
    PublicCommandOwnerContext.defaultValue = oldOwner;
    PublicRefreshGenerationContext.defaultValue = oldRefresh;
    WatchCommandOwnerContext.defaultValue = oldWatchOwner;
    Object.defineProperty(globalThis, "jsx", jsx);
  }
}

test.each([false, true])("普通会话复核后在途标记 GET 照常交付（写入后 %s）", async (afterWrite) => {
  const mounted = fixture(ALICE);
  const delayed = Promise.withResolvers<Response>();
  const initial = { ...PAGE, watches: [] };
  let reads = 0;
  try {
    await settle();
    mounted.server.mutate = (path, method) => {
      if (path === `/api/v1/lots/${LOT_ID}/watch` && method === "PUT") {
        return success({ user_id: ALICE.user_id, auction_id: AUCTION_ID, watch: PAGE.watches[0] });
      }
      if (path !== `/api/v1/me/lot-watches?auction_id=${AUCTION_ID}` || method !== "GET") {
        throw new Error(`意外请求 ${method} ${path}`);
      }
      reads += 1;
      return afterWrite && reads === 1 ? success(initial) : delayed.promise;
    };
    const view = mountWatchView(mounted);
    try {
      await settle();
      expect(reads).toBe(1);
      if (afterWrite) {
        expect(view.toggle()).toEqual({ disabled: false, snapshot: [false, 0] });
        view.commands.submit(AUCTION_ID, LOT_ID, "Clo16", {
          command_id: BOB.user_id,
          watched: true,
          channel: "mm",
          expected_version: 0,
        });
        await settle();
        expect(view.commands.commands.peek().get(LOT_ID)?.awaitingRefresh).toBe(1);
        expect(reads).toBe(2);
      }
      expect(view.toggle()).toEqual({ disabled: true });

      const identityRevision = publicSessionIdentityRevision();
      const primaryRevision = publicSessionPrimaryRevision();
      mounted.browser.focus();
      await settle();
      expect(publicSessionIdentityRevision()).toBeGreaterThan(identityRevision);
      expect(publicSessionPrimaryRevision()).toBe(primaryRevision);
      expect(mounted.session()).toEqual(ok(ALICE));
      expect(reads).toBe(afterWrite ? 2 : 1);

      delayed.resolve(success(PAGE));
      await settle();
      expect(view.toggle()).toEqual({ disabled: false, snapshot: [true, 1] });
      expect(view.commands.commands.peek().has(LOT_ID)).toBe(false);
      expect(view.displayed()).toBe(true);
      expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);
      expect(reads).toBe(afterWrite ? 2 : 1);
    } finally {
      view.dispose();
    }
  } finally {
    delayed.resolve(success(PAGE));
    mounted.dispose();
  }
});

const CHANGED_SESSION_RESPONSES = [
  ["原账号投影", () => success(PAGE)],
  ["其他账号投影", () => success({ ...PAGE, user_id: OTHER_USER_ID })],
  ["401", () => reject("unauthenticated", 401)],
] as const;

test.each(
  ["重登", "换号", "退出", "外壳重新登记"].flatMap((change) =>
    CHANGED_SESSION_RESPONSES.map(([name, response]) => [change, name, response] as const),
  ),
)("%s 后迟到的%s不影响新会话，也不交付标记", async (change, _name, response) => {
  const mounted = fixture(ALICE);
  const oldRead = Promise.withResolvers<Response>();
  const newRead = Promise.withResolvers<Response>();
  const fresh = { ...PAGE, watches: [] };
  let reads = 0;
  let effectsOnNewShell = 0;
  let unregister: (() => void) | null = null;
  try {
    await settle();
    mounted.server.mutate = (_path, method) => {
      if (method === "POST") return loginReceipt(ALICE);
      reads += 1;
      return reads === 1 ? oldRead.promise : newRead.promise;
    };
    const view = mountWatchView(mounted);
    try {
      await settle();
      expect(reads).toBe(1);
      if (change === "重登") {
        expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(ALICE));
      } else if (change === "外壳重新登记") {
        unregister = registerPublicSessionCheckTrigger(
          () => {
            effectsOnNewShell += 1;
          },
          () => {
            effectsOnNewShell += 1;
            return false;
          },
        );
      } else {
        mounted.server.identity = change === "换号" ? BOB : null;
        mounted.browser.focus();
      }
      await settle();
      const identity = change === "换号" ? BOB : change === "退出" ? null : ALICE;
      expect(mounted.commandOwner()).toBe(identity?.user_id ?? null);
      expect(mounted.session()).toEqual(ok(identity));
      expect(reads).toBe(change === "重登" ? 2 : 1);
      const requestCount = mounted.requests.length;
      const revision = publicSessionPrimaryRevision();

      oldRead.resolve(response());
      await settle();
      expect(effectsOnNewShell).toBe(0);
      expect(mounted.requests).toHaveLength(requestCount);
      expect(publicSessionPrimaryRevision()).toBe(revision);
      expect(mounted.commandOwner()).toBe(identity?.user_id ?? null);
      expect(mounted.session()).toEqual(ok(identity));
      expect(view.displayed()).toBe(false);
      expect(view.toggle()).toEqual({ disabled: true });
      expect(mounted.browser.storage.has(STORAGE_KEY)).toBe(false);
      expect(load(OTHER_USER_ID, AUCTION_ID)).toBeNull();
      if (change === "重登") {
        newRead.resolve(success(fresh));
        await settle();
        expect(view.toggle()).toEqual({ disabled: false, snapshot: [false, 0] });
        expect(load(ALICE.user_id, AUCTION_ID)).toEqual(fresh);
      }
    } finally {
      view.dispose();
    }
  } finally {
    unregister?.();
    oldRead.resolve(response());
    newRead.resolve(success(fresh));
    mounted.dispose();
  }
});

test.each([
  ["不同账号的投影", () => success({ ...PAGE, user_id: BOB.user_id }), BOB],
  ["鉴权失败", () => reject("unauthenticated", 401), null],
] as const)("普通复核后同代标记 GET 的%s触发会话复核", async (_name, response, next) => {
  const mounted = fixture(ALICE);
  const delayed = Promise.withResolvers<Response>();
  const sessionRead = Promise.withResolvers<Response>();
  const sessionReads = () =>
    mounted.requests.filter(({ path, method }) => path === "/api/v1/session" && method === "GET")
      .length;
  try {
    await settle();
    mounted.server.mutate = () => delayed.promise;
    const view = mountWatchView(mounted);
    try {
      await settle();
      const identityRevision = publicSessionIdentityRevision();
      const revision = publicSessionPrimaryRevision();
      mounted.browser.focus();
      await settle();
      expect(publicSessionIdentityRevision()).toBeGreaterThan(identityRevision);
      expect(publicSessionPrimaryRevision()).toBe(revision);
      expect(mounted.session()).toEqual(ok(ALICE));
      const before = sessionReads();

      mounted.server.identity = next;
      mounted.server.read = () => sessionRead.promise;
      delayed.resolve(response());
      await settle();
      expect(sessionReads()).toBe(before + 1);
      expect(mounted.session.state()).toBe("pending");
      expect(mounted.session.peek()).toBeUndefined();
      expect(mounted.commandOwner()).toBe(next?.user_id ?? ALICE.user_id);
      expect(view.displayed()).toBe(false);
      expect(view.toggle()).toEqual({ disabled: true });
      expect(mounted.browser.storage.has(STORAGE_KEY)).toBe(false);
      expect(load(BOB.user_id, AUCTION_ID)).toBeNull();

      sessionRead.resolve(success(next));
      await settle();
      expect(mounted.session()).toEqual(ok(next));
      expect(mounted.commandOwner()).toBe(next?.user_id ?? null);
    } finally {
      view.dispose();
    }
  } finally {
    delayed.resolve(response());
    sessionRead.resolve(success(next));
    mounted.dispose();
  }
});

test.each([true, false])(
  "同账号重新登录重读标记并保留快照（此前退出对账失败 %s）",
  async (logoutFailed) => {
    const mounted = fixture(ALICE);
    const nextWatches = Promise.withResolvers<Response>();
    const fresh = { ...PAGE, watches: [] };
    try {
      await settle();
      const readPublic = () => fetchCurrentAuction("main", mounted.signal);
      expect(await readPublic()).toEqual(ok(null));
      mounted.server.mutate = () => success(PAGE);
      const view = mountWatchView(mounted);
      try {
        await settle();
        expect(view.displayed()).toBe(true);
        expect(view.toggle()).toEqual({ disabled: false, snapshot: [true, 1] });
        if (logoutFailed) {
          mounted.server.mutate = () => {
            mounted.server.identity = null;
            return brokenBody();
          };
          mounted.server.read = () => reject("storage_unavailable", 503);
          expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(err("network_error"));
          await settle();
          expect(mounted.commandOwner()).toBe(ALICE.user_id);
        }
        mounted.server.mutate = (_path, method) => {
          if (method === "POST") {
            mounted.server.identity = ALICE;
            return loginReceipt(ALICE);
          }
          return nextWatches.promise;
        };
        mounted.server.read = () => success(mounted.server.identity);
        expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(ALICE));
        await settle();
        expect(mounted.session()).toEqual(ok(ALICE));
        expect(mounted.owners).toEqual([null, ALICE.user_id]);
        expect(view.displayed()).toBe(true);
        expect(view.toggle()).toEqual({ disabled: true });
        expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
        expect(await readPublic()).toEqual(ok(null));
        expect(
          mounted.requests.filter(({ path }) => path.startsWith("/api/v1/auctions/current?")),
        ).toHaveLength(1);
        expect(
          mounted.requests.filter(({ path }) => path.startsWith("/api/v1/me/lot-watches?")),
        ).toHaveLength(2);
        nextWatches.resolve(success(fresh));
        await settle();
        expect(view.displayed()).toBe(false);
        expect(view.toggle()).toEqual({ disabled: false, snapshot: [false, 0] });
        expect(load(ALICE.user_id, AUCTION_ID)).toEqual(fresh);
      } finally {
        view.dispose();
      }
    } finally {
      nextWatches.resolve(success(fresh));
      mounted.dispose();
    }
  },
);

test("登录回执解码失败时只做会话对账，旧代次的持久快照视为缺失", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    save(PAGE, publicWatchCacheRevision());
    mounted.server.mutate = () => success(null);
    expect(await loginPublic("one-use", mounted.signal)).toMatchObject({
      ok: false,
      error: { code: "invalid_response" },
    });
    await settle();
    expect(mounted.owners).toEqual([null, ALICE.user_id]);
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
  } finally {
    mounted.dispose();
  }
});

test("同账号退出再登录，标记读取在权威结果前无法恢复旧快照", async () => {
  const mounted = fixture(ALICE);
  const watchRead = Promise.withResolvers<Response>();
  try {
    await settle();
    const otherAuction = { ...PAGE, auction_id: OTHER_AUCTION_ID };
    const otherUser = { ...PAGE, user_id: BOB.user_id };
    save(PAGE, publicWatchCacheRevision());
    save(otherAuction, publicWatchCacheRevision());
    save(otherUser, publicWatchCacheRevision());
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);

    mounted.server.mutate = () => {
      mounted.server.identity = null;
      return success(null);
    };
    expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(ok(null));
    await settle();
    mounted.server.mutate = () => {
      mounted.server.identity = ALICE;
      return loginReceipt(ALICE);
    };
    expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(ALICE));
    await settle();

    mounted.server.mutate = (path, method) => {
      expect(path).toBe(`/api/v1/me/lot-watches?auction_id=${AUCTION_ID}`);
      expect(method).toBe("GET");
      return watchRead.promise;
    };
    const read = fetchLotWatches(ALICE.user_id, AUCTION_ID, mounted.signal);
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
    expect(load(ALICE.user_id, OTHER_AUCTION_ID)).toBeNull();
    expect(load(BOB.user_id, AUCTION_ID)).toBeNull();
    const fresh = { ...PAGE, watches: [] };
    watchRead.resolve(success(fresh));
    const result = await read;
    expect(result).toEqual(ok(fresh));
    if (result.ok) {
      save(result.value, publicWatchCacheRevision());
    }
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(fresh);
  } finally {
    watchRead.resolve(success({ ...PAGE, watches: [] }));
    mounted.dispose();
  }
});

test("共用标记缓存只接受第 2 版和发起读取时的十进制会话修订", async () => {
  const mounted = fixture(ALICE);
  const { storage, writes } = mounted.browser;
  try {
    await settle();
    storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, ALICE.user_id);
    storage.set(STORAGE_KEY, JSON.stringify(PAGE));
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();

    const sample = await Bun.file(CACHE_FIXTURE).text();
    save(PAGE, publicWatchCacheRevision());
    expect(storage.get(STORAGE_KEY)).toBe(sample);
    storage.set(STORAGE_KEY, sample);
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);
    const decoded = parseJson(sample);
    const entry = decoded.ok ? object(decoded.value) : null;
    if (entry === null) throw new Error("标记缓存样本损坏");
    storage.set(STORAGE_KEY, JSON.stringify({ ...entry, version: 1 }));
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
    const missingRevision = { ...entry };
    delete missingRevision.session_revision;
    storage.set(STORAGE_KEY, JSON.stringify(missingRevision));
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();

    const oldRevision = publicWatchCacheRevision();
    storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, BOB.user_id);
    // 另一标签页更换修订以后，才到达原读取所启动的写入。
    storage.set(STORAGE_KEY, sample);
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
    const before = writes.length;
    save(PAGE, oldRevision);
    expect(writes).toHaveLength(before);
    save(PAGE, publicWatchCacheRevision());
    expect(writes.length).toBeGreaterThan(before);
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);
  } finally {
    mounted.dispose();
  }
});

test("广播前已发出的整场读取不会回填内存或持久快照", async () => {
  const mounted = fixture(ALICE);
  const previous = Promise.withResolvers<Response>();
  try {
    await settle();
    mounted.server.mutate = () => previous.promise;
    const view = mountWatchView(mounted);
    try {
      await settle();
      mounted.browser.storage.set(PUBLIC_SESSION_CHANGE_STORAGE_KEY, ALICE.user_id);
      previous.resolve(success(PAGE));
      await settle();
      expect(view.displayed()).toBe(false);
      expect(mounted.browser.storage.has(STORAGE_KEY)).toBe(false);

      mounted.server.mutate = () => success(PAGE);
      mounted.browser.notifySessionRevision();
      await settle();
      expect(view.displayed()).toBe(true);
      expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);
    } finally {
      view.dispose();
    }
  } finally {
    previous.resolve(success(PAGE));
    mounted.dispose();
  }
});

test.each([BOB, null])("权威身份变化清除旧账号标记，普通同账号复核保留快照 %j", async (next) => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    save(PAGE, publicWatchCacheRevision());
    mounted.browser.focus();
    await settle();
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(PAGE);

    mounted.server.identity = next;
    mounted.browser.focus();
    await settle();
    expect(mounted.session()).toEqual(ok(next));
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
  } finally {
    mounted.dispose();
  }
});
