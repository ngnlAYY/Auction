// Public 路由到各领域页面的薄包装。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { page as accountPage } from "../account";
import { page as blacklistPage } from "../blacklist";
import { AuctionPage } from "../board";
import { LotDeepLink } from "../catalog/detail/deep-link";
import { poolPage } from "../compensation";
import { page as forumPage } from "../forum";
import { redirectPublicAlias } from "../query";
import { BidLog as BidLogPage } from "../reads/bid-log";
import { Fulfillment as FulfillmentPage } from "../reads/fulfillment";
import { History as HistoryPage } from "../reads/history";
import { page as rulesPage } from "../rules";
import { login as sessionLogin } from "../session/login";
import { page as realtimePage } from "./status";

const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 接受 uuid crate 的简单、连字符、花括号与 URN 形式，并输出规范小写连字符 UUID。 */
function canonicalUuid(value: string): string | null {
  let hyphenated = value;
  if (value.length === 38 && value.startsWith("{") && value.endsWith("}")) {
    hyphenated = value.slice(1, -1);
  } else if (value.length === 45 && value.slice(0, 9).toLowerCase() === "urn:uuid:") {
    hyphenated = value.slice(9);
  } else if (value.length === 32 && /^[0-9a-fA-F]{32}$/u.test(value)) {
    hyphenated = `${value.slice(0, 8)}-${value.slice(8, 12)}-${value.slice(12, 16)}-${value.slice(16, 20)}-${value.slice(20)}`;
  }
  if (
    !/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/u.test(
      hyphenated,
    )
  ) {
    return null;
  }
  return hyphenated.toLowerCase();
}

/** 竞价日志页的路由薄包装。 */
export function BidLog(): JSX.Element {
  return BidLogPage();
}

/** 履约进度页的路由薄包装。 */
export function Fulfillment(): JSX.Element {
  return FulfillmentPage();
}

/** 价格历史页的路由薄包装。 */
export function History(): JSX.Element {
  return HistoryPage();
}

/** 登录页的路由薄包装。 */
export function Login(props: { readonly returnTo?: string }): JSX.Element {
  return sessionLogin({
    get returnTo() {
      return props.returnTo ?? null;
    },
  });
}

/** 账号页的路由薄包装。 */
export function Me(): JSX.Element {
  return accountPage();
}

/** 旧账号路径的兼容跳转。 */
export function Account(): JSX.Element {
  return redirectPublicAlias("/me");
}

/** 拍品直达链接。独立的拍品页已由目录与各列表的详情抽屉取代：参数是拍品 UUID 时（被超提醒
 * 里的链接）解析出它所在的目录位置，并在看板上打开详情抽屉；其余取值（旧书签、早期提醒里
 * 的 lot 代码）无法确定是哪一场的拍品，回到拍卖页并保留原地址中的查询。 */
export function LotDetail(props: { readonly lotCode: string }): JSX.Element {
  const lotId = createValueMemo(() => {
    const parsed = canonicalUuid(props.lotCode);
    return parsed !== null && parsed !== NIL_UUID ? parsed : null;
  });
  return (
    <Show when={lotId()} fallback={<AuctionPage />}>
      {(id) => <LotDeepLink lotId={id()} />}
    </Show>
  );
}

/** 规则页的路由薄包装。 */
export function Rules(): JSX.Element {
  return rulesPage("rules");
}

/** 关于页的路由薄包装。 */
export function About(): JSX.Element {
  return rulesPage("about");
}

/** 黑名单页的路由薄包装。 */
export function Blacklist(): JSX.Element {
  return blacklistPage();
}

/** 补偿池页的路由薄包装。 */
export function CompensationPool(): JSX.Element {
  return poolPage();
}

/** 旧补偿池路径的兼容跳转。 */
export function Compensation(): JSX.Element {
  return redirectPublicAlias("/compensation-pool");
}

/** 论坛页的路由薄包装。 */
export function Forum(): JSX.Element {
  return forumPage();
}

/** 状态页的路由薄包装。 */
export function Status(): JSX.Element {
  return realtimePage();
}

/** 未匹配任何已冻结路由时回到公开端首页。
 * 旧版 Public 对未知路径使用无历史记录跳转，而不是在外壳之外渲染独立 404；复用别名
 * 跳转边界也会保留旧 URL 中的筛选查询，避免分享链接意外丢失上下文。 */
export function NotFound(_props: { readonly segments: readonly string[] }): JSX.Element {
  return redirectPublicAlias("/");
}
