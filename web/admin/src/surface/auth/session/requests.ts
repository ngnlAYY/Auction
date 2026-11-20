// 会话请求与定时器的票号、Cookie 快照和取消边界。

import { unreachable } from "@auction/web-shared/result";
import type { Actions, Draft } from "./draft";
import type { Cookie, Request, Ticket, Timer } from "./index";

/** 标识只在当前外壳内递增；回执必须与派发时的票号一致才被接受。 */
export function ticket(draft: Draft): Ticket {
  draft.nextTicket += 1;
  return draft.nextTicket;
}

/** 以本轮观测为请求登记代际，冻结快照以免随后被改写。 */
export function request(draft: Draft, actions: Actions): Request {
  return { ticket: ticket(draft), cookie: cookieSnapshot(actions.cookie) };
}

/** 冻结观测到的 Cookie 代际，供请求保存当时的证据。 */
export function cookieSnapshot(cookie: Cookie): Cookie {
  switch (cookie.kind) {
    case "unavailable":
      return Object.freeze({ kind: "unavailable" });
    case "missing":
      return Object.freeze({ kind: "missing" });
    case "present":
      return Object.freeze({ kind: "present", value: cookie.value });
    default:
      return unreachable(cookie);
  }
}

/** 读取当前持有的定时器票。 */
export function timerValue(draft: Draft, timer: Timer): Ticket | null {
  return timer === "check" ? draft.checkTimer : draft.renewalTimer;
}

/** 更新指定用途的定时器票。 */
export function setTimer(draft: Draft, timer: Timer, value: Ticket | null): void {
  if (timer === "check") {
    draft.checkTimer = value;
  } else {
    draft.renewalTimer = value;
  }
}

/** 撤销定时器票与浏览器任务。 */
export function cancelTimer(draft: Draft, timer: Timer, actions: Actions): void {
  if (timerValue(draft, timer) !== null) {
    setTimer(draft, timer, null);
    actions.effects.push({ kind: "cancel_timer", timer });
  }
}

/** 用新票替换同用途的定时器，旧唤醒不能归并到新请求。 */
export function armTimer(draft: Draft, timer: Timer, delay: number, actions: Actions): void {
  cancelTimer(draft, timer, actions);
  const armed = ticket(draft);
  setTimer(draft, timer, armed);
  actions.effects.push({ kind: "arm_timer", timer, ticket: armed, delay });
}

/** 撤销读取票与浏览器任务。 */
export function cancelRead(draft: Draft, actions: Actions): void {
  if (draft.check !== null) {
    draft.check = null;
    actions.effects.push({ kind: "cancel_read" });
  }
}
