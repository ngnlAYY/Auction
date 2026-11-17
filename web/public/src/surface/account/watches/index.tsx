//! “我的”当前场次标记列表；账号、世界与场次各有独立资源边界。

import type { WorldsResponse } from "@auction/contracts/dto/public/account";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Match, Show, Switch, useContext } from "solid-js";

import { fetchWorlds } from "../../../api/queries/current";
import { PublicRefreshGenerationContext } from "../../board/refresh";
import { supportedWorlds, worldLabel } from "../../board/world";
import {
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "../../index";
import { useContentLoading } from "../../loading";
import { load, type WatchedAuction } from "./load";
import { WatchedTable } from "./table";

function currentWorlds(worlds: WorldsResponse): string[] {
  return supportedWorlds(worlds).filter((world) => worlds.worlds[world]?.enabled === true);
}

function identityMatches(session: PublicSessionResource, userId: string): boolean {
  return snapshotMatches(session(), userId);
}

// 异步读取前后只复核当前身份，不能把会话续期时间等非身份变化订阅成目录重读依赖。
function currentIdentityMatches(session: PublicSessionResource, userId: string): boolean {
  return snapshotMatches(session.peek(), userId);
}

function snapshotMatches(snapshot: PublicSessionResult | undefined, userId: string): boolean {
  return (
    snapshot?.ok === true &&
    snapshot.value !== null &&
    snapshot.value.authenticated &&
    snapshot.value.user_id === userId
  );
}

/** 已认证账号的独立列表宿主，不随竞价和代理分页卸载。 */
export function CurrentWatches(props: { readonly userId: string }): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const refresh = useContext(PublicRefreshGenerationContext);
  const worlds = createTaskResource(
    () => refresh(),
    (_refresh, context) => fetchWorlds(context.signal),
  );
  const identityCurrent = createValueMemo(() => identityMatches(session, props.userId));
  const currentWorldList = createValueMemo(() => {
    const snapshot = worlds();
    return snapshot?.ok === true ? currentWorlds(snapshot.value) : [];
  });
  const worldsFailed = () => worlds()?.ok === false;
  useContentLoading(() => identityCurrent() && worlds() === undefined);
  return (
    <Show when={identityCurrent()}>
      <section class="public-account-section" aria-labelledby="account-watches-title">
        <h2 id="account-watches-title">{t("account-watches-title")}</h2>
        {/* 同一账号的后台世界目录重读保留已确认宿主，避免卸载世界列表及其分页。 */}
        <Switch>
          <Match when={worlds() === undefined}>{null}</Match>
          <Match when={worldsFailed()}>
            <div role="alert">
              <p>{t("account-watches-error")}</p>
              <button type="button" onClick={() => worlds.restart()}>
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={worlds()?.ok === true}>
            <Show when={currentWorldList().length === 0}>
              <p class="public-account-empty">{t("account-watches-no-current")}</p>
            </Show>
            <For each={currentWorldList()} keyed={(world) => `${props.userId}:${world}`}>
              {(world) => <WorldWatches userId={props.userId} world={world()} />}
            </For>
          </Match>
        </Switch>
      </section>
    </Show>
  );
}

function WorldWatches(props: { readonly userId: string; readonly world: string }): JSX.Element {
  const session = useContext(PublicSessionResourceContext);
  const refresh = useContext(PublicRefreshGenerationContext);
  const data = createTaskResource(
    () => refresh(),
    async (_refresh, context): Promise<Result<WatchedAuction | null, null>> => {
      const userId = props.userId;
      let result: Result<WatchedAuction | null, null>;
      if (currentIdentityMatches(session, userId)) {
        result = await load(userId, props.world, context.signal);
        if (!context.alive()) {
          return result;
        }
      } else {
        result = err(null);
      }
      if (!currentIdentityMatches(session, userId)) {
        result = err(null);
      }
      return result;
    },
  );
  // 此列表不展示价格或领先者；竞价通知不改变这四列，生命周期变更仍通过全局修订重读。
  // 标记只能在看板上变更，回到本页时整页重新挂载并读取。
  const identityCurrent = createValueMemo(() => identityMatches(session, props.userId));
  useContentLoading(() => identityCurrent() && data() === undefined);
  const updating = () => data.state() !== "ready";
  const snapshot = () => data();
  const failed = () => snapshot()?.ok === false;
  const noCurrent = () => {
    const current = snapshot();
    return current?.ok === true && current.value === null;
  };
  // Resource 保留上次完成值；同一身份和世界刷新时继续显示它，结果返回后再按场次 key 替换。
  const watchedValues = createValueMemo((): readonly WatchedAuction[] => {
    const current = snapshot();
    return current?.ok === true && current.value !== null ? [current.value] : [];
  });
  return (
    <Show when={identityCurrent()}>
      <section
        class="public-account-section"
        aria-label={worldLabel(props.world)}
        aria-busy={updating() ? "true" : "false"}
      >
        <Switch>
          <Match when={snapshot() === undefined}>
            <h3>{worldLabel(props.world)}</h3>
          </Match>
          <Match when={failed()}>
            <h3>{worldLabel(props.world)}</h3>
            <div role="alert">
              <p>{t("account-watches-error")}</p>
              <button type="button" onClick={() => data.restart()}>
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={noCurrent()}>
            <h3>{worldLabel(props.world)}</h3>
            <p class="public-account-empty">{t("account-watches-no-current")}</p>
          </Match>
          <Match when={watchedValues().length > 0}>
            <For
              each={watchedValues()}
              keyed={(watched) => `${props.userId}:${props.world}:${watched.auction.id}`}
            >
              {(watched) => <WatchedTable watched={watched()} />}
            </For>
          </Match>
        </Switch>
      </section>
    </Show>
  );
}
