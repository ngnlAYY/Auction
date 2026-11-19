// 账户页挂载两项真实偏好表单，验证同账号重登时的读回和命令恢复。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { loginPublic } from "../../api/session";
import {
  dispatch,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import { ALICE, BOB, reject, success } from "../shell/session-fixture";

test.each([false, true])("同账号重新登录重读两项偏好并保留脏草稿 %s", async (dirty) => {
  const page = await mountCompetition("main", true);
  const reads = { duration: 0, outbid: 0 };
  const bodies: string[] = [];
  try {
    page.server.readPage = (url, init) => {
      const duration = url.pathname === "/api/v1/me/session-preference";
      if (!duration && url.pathname !== "/api/v1/me/lot-watch-preference") return null;
      if (init?.method === "PUT") {
        bodies.push(String(init.body));
        return reject(duration ? "session_preference_conflict" : "watch_preference_conflict", 409);
      }
      const version = duration ? ++reads.duration : ++reads.outbid;
      return success({
        user_id: ALICE.user_id,
        version,
        ...(duration
          ? { duration_seconds: version === 1 ? null : 3600, expires_at: null }
          : { channel: version === 1 ? "none" : "mm", pm_available: true }),
      });
    };
    page.go("/me");
    const input = (id: string) => page.need((element) => element.getAttribute("id") === id);
    const button = (label: string) =>
      page.find((element) => element.tag === "button" && renderedText(element) === t(label));
    const ready = () => {
      const duration = page.find(
        (element) => element.getAttribute("id") === "session-duration-permanent",
      );
      const outbid = button("outbid-save");
      return duration?.attributes.disabled === false && outbid?.attributes.disabled === false;
    };
    await waitFor(ready);
    if (dirty) {
      dispatch(input("session-duration-custom"), "Change");
      const pm = input("outbid-channel-pm");
      Object.assign(pm, { checked: true });
      dispatch(pm, "Change");
      await settle();
    }
    expect((await loginPublic("one-use", new AbortController().signal)).ok).toBe(true);
    await settle();
    expect(reads).toEqual({ duration: 2, outbid: 2 });
    await waitFor(ready);
    expect(input("session-duration-custom").attributes.checked).toBe(true);
    expect(input("session-duration-amount").attributes.value).toBe("1");
    expect(input("session-duration-unit").attributes.value).toBe(dirty ? "days" : "hours");
    expect(input("outbid-channel-mm").attributes.checked).toBe(!dirty);
    expect(input("outbid-channel-pm").attributes.checked).toBe(dirty);
    const durationForm = page.need((element) => element.tag === "form", input("login-duration"));
    const saveOutbid = page.need((element) => element === button("outbid-save"));
    dispatch(durationForm, "Submit");
    dispatch(saveOutbid, "Click");
    await waitFor(() => bodies.length === 2);
    for (const body of bodies) {
      expect(body).toContain(`"expected_version":${dirty ? 1 : 2}`);
    }
  } finally {
    page.dispose();
  }
});

test("忙碌时多次重登只补读一次，两份旧回执都不能覆盖新数据", async () => {
  const page = await mountCompetition("main", true);
  const oldDuration = Promise.withResolvers<Response>();
  const oldOutbid = Promise.withResolvers<Response>();
  const freshDuration = Promise.withResolvers<Response>();
  const freshOutbid = Promise.withResolvers<Response>();
  const reads = { duration: 0, outbid: 0 };
  try {
    page.server.readPage = (url) => {
      const userId = page.server.identity?.user_id;
      if (url.pathname === "/api/v1/me/session-preference") {
        reads.duration += 1;
        if (reads.duration === 1) return oldDuration.promise;
        if (reads.duration === 2) return freshDuration.promise;
        return success({ user_id: userId, duration_seconds: 3600, version: 2, expires_at: null });
      }
      if (url.pathname === "/api/v1/me/lot-watch-preference") {
        reads.outbid += 1;
        if (reads.outbid === 1) return oldOutbid.promise;
        if (reads.outbid === 2) return freshOutbid.promise;
        return success({ user_id: userId, channel: "pm", version: 2, pm_available: true });
      }
      return null;
    };
    page.go("/me");
    await waitFor(() => reads.duration === 1 && reads.outbid === 1);
    const signIn = () => loginPublic("one-use", new AbortController().signal);
    for (let count = 0; count < 2; count += 1) {
      expect((await signIn()).ok).toBe(true);
      await settle();
      expect(reads).toEqual({ duration: 1, outbid: 1 });
    }
    oldDuration.resolve(
      success({ user_id: ALICE.user_id, duration_seconds: 60, version: 1, expires_at: null }),
    );
    oldOutbid.resolve(
      success({ user_id: ALICE.user_id, channel: "pm", version: 1, pm_available: true }),
    );
    const checked = (id: string) =>
      page.find((element) => element.getAttribute("id") === id)?.attributes.checked === true;
    await waitFor(() => reads.duration === 2 && reads.outbid === 2);
    expect(checked("session-duration-custom")).toBe(false);
    expect(checked("outbid-channel-pm")).toBe(false);
    freshDuration.resolve(
      success({ user_id: ALICE.user_id, duration_seconds: 3600, version: 2, expires_at: null }),
    );
    freshOutbid.resolve(
      success({ user_id: ALICE.user_id, channel: "mm", version: 2, pm_available: true }),
    );
    await waitFor(() => checked("session-duration-custom") && checked("outbid-channel-mm"));
    expect(reads).toEqual({ duration: 2, outbid: 2 });
    page.focus();
    await settle();
    expect(reads).toEqual({ duration: 2, outbid: 2 });

    page.server.nextLogin = BOB;
    expect((await signIn()).ok).toBe(true);
    await waitFor(() => checked("outbid-channel-pm"));
    expect(checked("outbid-channel-mm")).toBe(false);
    expect(reads).toEqual({ duration: 3, outbid: 3 });
  } finally {
    oldDuration.resolve(reject("temporarily_unavailable", 503));
    oldOutbid.resolve(reject("temporarily_unavailable", 503));
    freshDuration.resolve(reject("temporarily_unavailable", 503));
    freshOutbid.resolve(reject("temporarily_unavailable", 503));
    page.dispose();
  }
});

test.each([false, true])(
  "重登期间的未知偏好命令保留至显式重放（登录时长 %s）",
  async (duration) => {
    const page = await mountCompetition("main", true);
    const pathname = duration ? "/api/v1/me/session-preference" : "/api/v1/me/lot-watch-preference";
    let reads = 0;
    const bodies: unknown[] = [];
    try {
      page.server.readPage = (url, init) => {
        if (url.pathname !== pathname) return null;
        const preference = (version: number) =>
          success({
            user_id: ALICE.user_id,
            version,
            ...(duration
              ? { duration_seconds: null, expires_at: null }
              : { channel: "none", pm_available: true }),
          });
        if (init?.method === "PUT") {
          bodies.push(init.body);
          return bodies.length === 1 ? reject("temporarily_unavailable", 503) : preference(2);
        }
        return preference(++reads);
      };
      page.go("/me");
      const section = (id: string) => page.find((element) => element.getAttribute("id") === id);
      const containerId = duration ? "login-duration" : "outbid-notifications";
      const button = (label: string) => {
        const container = section(containerId);
        return container === null
          ? null
          : page.find(
              (element) =>
                element.tag === "button" &&
                element.attributes.disabled !== true &&
                renderedText(element) === t(label),
              container,
            );
      };
      const save = () => button(duration ? "session-duration-save" : "outbid-save");
      const retry = () => button(duration ? "session-duration-retry" : "outbid-retry-command");
      await waitFor(() => save() !== null);
      if (duration) {
        const container = page.need((element) => element.getAttribute("id") === containerId);
        dispatch(
          page.need((element) => element.tag === "form", container),
          "Submit",
        );
      } else {
        dispatch(
          page.need((element) => element === save()),
          "Click",
        );
      }
      await waitFor(() => retry() !== null);
      for (let count = 0; count < 2; count += 1) {
        expect((await loginPublic("one-use", new AbortController().signal)).ok).toBe(true);
        await settle();
        expect(reads).toBe(1);
        expect(bodies).toHaveLength(1);
        expect(retry()).not.toBeNull();
      }
      dispatch(
        page.need((element) => element === retry()),
        "Click",
      );
      await waitFor(() => save() !== null);
      await settle();
      expect(reads).toBe(2);
      expect(bodies).toEqual([bodies[0], bodies[0]]);
    } finally {
      page.dispose();
    }
  },
);
