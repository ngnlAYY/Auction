// 以替身 Cookie 和 Web Lock 验证外壳的会话复核、身份隔离与提示。

import { expect, spyOn, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { flush } from "solid-js";

import { fetchCurrentAuction } from "../../api/queries/current";
import {
  loginPublic,
  logoutPublic,
  observePublicSessionFailure,
  PUBLIC_SESSION_CHANGE_STORAGE_KEY,
  type PublicSession,
  registerPublicSessionCheckTrigger,
} from "../../api/session";
import { setSessionPreference } from "../../api/session-preference";

import { ALICE, BOB, brokenBody, fixture, loginReceipt, reject, success } from "./session-fixture";

const SESSION_PATH = "/api/v1/session";
const PREFERENCE_PATH = "/api/v1/me/session-preference";

async function settle(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

const durationChange = { command_id: BOB.user_id, duration_seconds: 60, expected_version: 0 };
const FINITE: PublicSession = {
  ...ALICE,
  expires_at: "2090-01-01T12:01:00Z",
  absolute_expires_at: "2090-01-01T12:01:00Z",
};

test.each(["POST", "DELETE", "PUT"])(
  "%s 已收到成功响应头但正文中断，释放写锁后只权威读取一次",
  async (method) => {
    const mounted = fixture(method === "POST" ? null : ALICE);
    try {
      await settle();
      const start = mounted.requests.length;
      const next = method === "DELETE" ? null : method === "POST" ? ALICE : FINITE;
      mounted.server.mutate = (path, sentMethod) => {
        expect(path).toBe(method === "PUT" ? PREFERENCE_PATH : SESSION_PATH);
        expect(sentMethod).toBe(method);
        mounted.server.identity = next;
        return brokenBody();
      };
      const result =
        method === "POST"
          ? await loginPublic("one-use", mounted.signal)
          : method === "DELETE"
            ? await logoutPublic(ALICE.user_id, mounted.signal)
            : await setSessionPreference(ALICE.user_id, durationChange, mounted.signal);
      expect(result.ok).toBe(false);
      await settle();
      expect(mounted.session()).toEqual(ok(next));
      expect(mounted.sessionEndedElsewhere()).toBe(false);
      const mutation = mounted.requests.slice(start);
      expect(mutation.map((request) => request.method)).toEqual([method, "GET"]);
      expect(mutation[0]?.lock).not.toBeNull();
      expect(mutation[1]?.lock).not.toBe(mutation[0]?.lock);
      expect(
        mounted.browser.writes.filter((key) => key === PUBLIC_SESSION_CHANGE_STORAGE_KEY),
      ).toHaveLength(1);
    } finally {
      mounted.dispose();
    }
  },
);

test("未收到响应头的登录保持命令失败，外壳仍复核 Cookie；明确拒绝无需读回", async () => {
  const mounted = fixture(null);
  try {
    await settle();
    const start = mounted.requests.length;
    mounted.server.mutate = () => {
      mounted.server.identity = ALICE;
      throw new Error("连接中断");
    };
    expect(await loginPublic("one-use", mounted.signal)).toMatchObject({ ok: false });
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual(["POST", "GET"]);
    expect(mounted.session()).toEqual(ok(ALICE));

    mounted.server.mutate = () => reject("invalid_credentials", 401);
    expect(await loginPublic("rejected", mounted.signal)).toMatchObject({ ok: false });
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual([
      "POST",
      "GET",
      "POST",
    ]);
    expect(mounted.session()).toEqual(ok(ALICE));
  } finally {
    mounted.dispose();
  }
});

test("请求发出后登录页取消，外壳仍完成对账；发送前取消不启动对账", async () => {
  const mounted = fixture(null);
  try {
    await settle();
    const start = mounted.requests.length;
    const cancelled = new AbortController();
    cancelled.abort();
    expect((await loginPublic("one-use", cancelled.signal)).ok).toBe(false);
    expect(mounted.requests).toHaveLength(start);

    const inFlight = new AbortController();
    mounted.server.mutate = () => {
      mounted.server.identity = ALICE;
      inFlight.abort();
      return brokenBody();
    };
    expect((await loginPublic("one-use", inFlight.signal)).ok).toBe(false);
    mounted.browser.focus();
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual(["POST", "GET"]);
    expect(mounted.session()).toEqual(ok(ALICE));
  } finally {
    mounted.dispose();
  }
});

test("登录时长的完整回执只唤醒一次会话读取，明确版本冲突无需对账", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    const start = mounted.requests.length;
    const receipt = {
      user_id: ALICE.user_id,
      duration_seconds: durationChange.duration_seconds,
      version: 1,
      expires_at: FINITE.expires_at,
    };
    mounted.server.mutate = () => {
      mounted.server.identity = FINITE;
      return success(receipt);
    };
    expect(await setSessionPreference(ALICE.user_id, durationChange, mounted.signal)).toEqual(
      ok(receipt),
    );
    await settle();
    expect(mounted.session()).toEqual(ok(FINITE));
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual(["PUT", "GET"]);

    mounted.server.mutate = () => reject("session_preference_conflict", 409);
    expect(await setSessionPreference(ALICE.user_id, durationChange, mounted.signal)).toEqual(
      err({ code: "session_preference_conflict", status: 409 }),
    );
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual([
      "PUT",
      "GET",
      "PUT",
    ]);
  } finally {
    mounted.dispose();
  }
});

test("退出收到明确拒绝后正文中断，不掩盖之后真正的非本地会话结束", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    const start = mounted.requests.length;
    mounted.server.mutate = () => brokenBody(403);
    expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(err("network_error"));
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual(["DELETE"]);
    mounted.server.identity = null;
    mounted.browser.focus();
    await settle();
    expect(mounted.sessionEndedElsewhere()).toBe(true);
  } finally {
    mounted.dispose();
  }
});

test("例行复核确认未到期会话已匿名，只提示一次；临时读错不丢确认对象", async () => {
  const mounted = fixture(FINITE);
  try {
    await settle();
    mounted.server.read = () => reject("storage_unavailable", 503);
    mounted.browser.focus();
    await settle();
    expect(mounted.session()).toEqual(err("storage_unavailable"));
    expect(mounted.commandOwner()).toBe(ALICE.user_id);
    expect(mounted.sessionEndedElsewhere()).toBe(false);

    mounted.server.read = () => success(null);
    mounted.browser.focus();
    await settle();
    expect(mounted.session()).toEqual(ok(null));
    expect(mounted.commandOwner()).toBeNull();
    expect(mounted.sessionEndedElsewhere()).toBe(true);
    mounted.sessionEndedElsewhere.set(false);
    mounted.browser.focus();
    await settle();
    expect(mounted.sessionEndedElsewhere()).toBe(false);
  } finally {
    mounted.dispose();
  }
});

test("认证错误只触发复核，读到先前身份时不提示会话结束", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    observePublicSessionFailure(409, "session_changed");
    expect(mounted.commandOwner.peek()).toBe(ALICE.user_id);
    expect(mounted.session.peek()).toBeUndefined();
    expect(mounted.sessionEndedElsewhere.peek()).toBe(false);
    await settle();
    expect(mounted.session()).toEqual(ok(ALICE));
    expect(mounted.owners).toEqual([null, ALICE.user_id]);
    expect(mounted.sessionEndedElsewhere()).toBe(false);
  } finally {
    mounted.dispose();
  }
});

test.each([false, true])(
  "浏览器主动退出时不提示会话在别处结束（另一标签页 %s）",
  async (otherTab) => {
    const mounted = fixture(ALICE);
    const stop = otherTab ? registerPublicSessionCheckTrigger(() => {}) : () => {};
    try {
      await settle();
      mounted.server.mutate = () => {
        mounted.server.identity = null;
        return success(null);
      };
      expect(await logoutPublic(ALICE.user_id, mounted.signal)).toEqual(ok(null));
      if (otherTab) {
        mounted.browser.notifySessionRevision();
      }
      await settle();
      expect(mounted.session()).toEqual(ok(null));
      expect(mounted.commandOwner()).toBeNull();
      expect(mounted.sessionEndedElsewhere()).toBe(false);
    } finally {
      stop();
      mounted.dispose();
    }
  },
);

test("另一标签页退出期间先收到聚焦，再收到修订，不会误报主动退出", async () => {
  const mounted = fixture(ALICE);
  const stop = registerPublicSessionCheckTrigger(() => {});
  const finish = Promise.withResolvers<void>();
  const started = Promise.withResolvers<void>();
  try {
    await settle();
    const start = mounted.requests.length;
    mounted.server.mutate = () => {
      mounted.server.identity = null;
      started.resolve();
      return new Response(
        new ReadableStream({
          async start(controller) {
            await finish.promise;
            controller.error(new Error("退出正文中断"));
          },
        }),
      );
    };
    const logout = logoutPublic(ALICE.user_id, mounted.signal);
    await started.promise;
    mounted.browser.focus();
    await settle();
    expect(mounted.requests.slice(start).map((request) => request.method)).toEqual(["DELETE"]);

    finish.resolve();
    expect(await logout).toEqual(err("network_error"));
    await settle();
    expect(mounted.session()).toEqual(ok(null));
    expect(mounted.sessionEndedElsewhere()).toBe(false);
    mounted.browser.notifySessionRevision();
    await settle();
    expect(mounted.sessionEndedElsewhere()).toBe(false);
  } finally {
    finish.resolve();
    stop();
    mounted.dispose();
  }
});

test("有限会话自然到期后复核为匿名时不提示会话在别处结束", async () => {
  const now = spyOn(Date, "now").mockReturnValue(Date.parse("2090-01-01T12:00:00Z"));
  const mounted = fixture(FINITE);
  try {
    await settle();
    now.mockReturnValue(Date.parse("2090-01-01T12:01:00Z"));
    mounted.server.identity = null;
    mounted.browser.focus();
    await settle();
    expect(mounted.session()).toEqual(ok(null));
    expect(mounted.sessionEndedElsewhere()).toBe(false);
  } finally {
    mounted.dispose();
    now.mockRestore();
  }
});

test("登录、换号和退出保留公开缓存命中，并替换身份隔离的读取宿主", async () => {
  const mounted = fixture(null);
  const read = () => fetchCurrentAuction("main", mounted.signal);
  try {
    await settle();
    expect(await read()).toEqual(ok(null));
    for (const identity of [ALICE, BOB]) {
      mounted.server.mutate = () => {
        mounted.server.identity = identity;
        return loginReceipt(identity);
      };
      expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(identity));
      await settle();
      expect(mounted.commandOwner()).toBe(identity.user_id);
      expect(await read()).toEqual(ok(null));
    }
    mounted.server.mutate = () => {
      mounted.server.identity = null;
      return success(null);
    };
    expect(await logoutPublic(BOB.user_id, mounted.signal)).toEqual(ok(null));
    await settle();
    expect(await read()).toEqual(ok(null));
    expect(mounted.owners).toEqual([null, ALICE.user_id, BOB.user_id, null]);
    expect(
      mounted.requests.filter((request) => request.path.startsWith("/api/v1/auctions/current?")),
    ).toHaveLength(1);
  } finally {
    mounted.dispose();
  }
});

test("已登录冷启动在同一 Cookie 锁内只读一次，随后同步持久期", async () => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    expect(mounted.session()).toEqual(ok(ALICE));
    expect(mounted.requests.map((request) => request.method)).toEqual(["GET", "PUT"]);
    expect(mounted.requests[0]?.lock).not.toBeNull();
    expect(mounted.requests[0]?.lock).toBe(mounted.requests[1]?.lock);
  } finally {
    mounted.dispose();
  }
});

test.each([BOB, null])(
  "冷启动等待其他标签页换号或退出后，只使用锁内读到的身份 %j",
  async (next) => {
    const unlock = Promise.withResolvers<void>();
    const mounted = fixture(ALICE, { waitForLock: unlock.promise });
    try {
      await settle();
      expect(mounted.requests).toHaveLength(0);
      mounted.server.identity = next;
      unlock.resolve();
      await mounted.priorLock;
      await settle();
      expect(mounted.session()).toEqual(ok(next));
      expect(mounted.requests.map((request) => request.method)).toEqual(
        next === null ? ["GET"] : ["GET", "PUT"],
      );
    } finally {
      unlock.resolve();
      mounted.dispose();
    }
  },
);

test.each([null, ALICE])(
  "没有 Web Locks 时仍读取当前会话，需同步的 Cookie 保持失败关闭 %j",
  async (identity) => {
    const mounted = fixture(identity, { locksAvailable: false });
    try {
      await settle();
      expect(mounted.session()).toEqual(
        identity === null ? ok(null) : err("session_lock_unavailable"),
      );
      expect(mounted.requests.map((request) => request.method)).toEqual(["GET"]);
    } finally {
      mounted.dispose();
    }
  },
);
