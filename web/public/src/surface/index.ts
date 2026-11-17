//! 公开端浏览器界面的共享会话、命令归属与资源类型。
//! 路由集合由 `surface/router/route` 定义；HTTP/流 DTO 由 `auction-contracts` 拥有，这里只保留
//! 浏览器展示模型、查询状态和交互编排所需的窄类型。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { type Accessor, createContext } from "solid-js";

import type { PublicSession } from "../api/session";

export type PublicSessionResult = Result<PublicSession | null, string>;

/** Public 外壳唯一拥有的会话快照；页面只消费该资源，避免同一渲染树重复请求 session。 */
export interface PublicSessionResource {
  (): PublicSessionResult | undefined;
  peek(): PublicSessionResult | undefined;
  state(): "pending" | "ready" | "stopped";
  restart(): void;
  cancel(): void;
  clear(): void;
  set(value: PublicSessionResult | undefined): void;
}

/** 命令宿主的已确认身份；暂时会话读取失败不清理，不能用于授权新写入。 */
export type PublicCommandOwner = Accessor<string | null>;

/** Public 外壳唯一拥有的 readiness 快照；默认首页用它协调主内容与首屏门禁。 */
export type PublicReadinessResource = TaskResource<Result<boolean, string>>;

/** Public 外壳按世界读取的当前拍卖快照；结果携带请求世界，切换期间不得消费旧世界缓存。 */
export type CurrentAuctionResource = TaskResource<
  readonly [string, Result<CurrentAuctionItemResponse | null, string>]
>;

/** 会话资源快照的统一展示投影，看板、lot 详情和外壳头部共用。 */
export class PublicSessionView {
  readonly pending: boolean;
  readonly ready: boolean;
  readonly error: boolean;
  readonly authenticated: boolean;
  readonly viewerPlayerId: string | null;

  private constructor(
    pending: boolean,
    ready: boolean,
    error: boolean,
    authenticated: boolean,
    viewerPlayerId: string | null,
  ) {
    this.pending = pending;
    this.ready = ready;
    this.error = error;
    this.authenticated = authenticated;
    this.viewerPlayerId = viewerPlayerId;
  }

  /** 从会话资源当前快照派生展示投影；匿名、未就绪与错误一律失败关闭。 */
  static fromSnapshot(snapshot: PublicSessionResult | undefined): PublicSessionView {
    const pending = snapshot === undefined;
    const ready = snapshot?.ok === true;
    const error = snapshot?.ok === false;
    const identity = snapshot?.ok === true ? snapshot.value : null;
    const authenticated = identity?.authenticated === true;
    return new PublicSessionView(
      pending,
      ready,
      error,
      authenticated,
      authenticated ? (identity?.username ?? null) : null,
    );
  }
}

export const PublicSessionResourceContext = createContext<PublicSessionResource>();
export const PublicCommandOwnerContext = createContext<PublicCommandOwner>();

export {
  type PublicContentLoading,
  PublicContentLoadingContext,
  useContentLoading,
} from "./loading";
