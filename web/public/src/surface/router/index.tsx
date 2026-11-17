//! Public 语言目录与启动入口；路由表见子模块 `route`，页面外壳见 `shell` 模块。

import {
  type ThemeState,
  ThemeStateContext,
  useThemeEffects,
} from "@auction/web-shared/platform/theme";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { StaleBuildBanner } from "@auction/web-shared/ui/build-notice";
import {
  buildBundle,
  createLocaleSession,
  DEFAULT_PUBLIC_LOCALE,
  LocaleSessionContext,
  t,
  useLocaleEffects,
} from "@auction/web-shared/ui/i18n";
import type { FluentBundle } from "@fluent/bundle";
import type { JSX } from "@solidjs/web";
import { createEffect, Show } from "solid-js";
import {
  type InitialPublicLocation,
  InitialPublicLocationContext,
  initialPublicLocation,
} from "../query";
import {
  InitialLoginReturnLocationContext,
  initialLoginReturnLocation,
} from "../session/login-return";
import { PublicShell } from "../shell";
import { PUBLIC_LOCALE_CATALOG } from "./locale-catalog";
import { NotFoundPage, PUBLIC_ROUTES } from "./route";

export { PUBLIC_LOCALE_CATALOG } from "./locale-catalog";

export function App(): JSX.Element {
  // Router 会规范化未声明的 query；在它挂载前分别保留页面查询状态和认证返回来源。
  const initialLocation = initialPublicLocation();
  const built = buildBundle(
    DEFAULT_PUBLIC_LOCALE,
    PUBLIC_LOCALE_CATALOG.defaultFtl,
    DEFAULT_PUBLIC_LOCALE,
    null,
  );
  return (
    <InitialPublicLocationContext value={initialLocation}>
      <InitialLoginReturnLocationContext value={initialLoginReturnLocation()}>
        <Show
          when={built.ok ? built.value : undefined}
          fallback={<div class="app-root">Unable to initialize translations.</div>}
        >
          {(bundle) => <LocalizedApp bundle={bundle()} initialLocation={initialLocation} />}
        </Show>
      </InitialLoginReturnLocationContext>
    </InitialPublicLocationContext>
  );
}

function LocalizedApp(props: {
  bundle: FluentBundle;
  initialLocation: InitialPublicLocation;
}): JSX.Element {
  const session = createLocaleSession(props.bundle, DEFAULT_PUBLIC_LOCALE);
  session.bootstrap(PUBLIC_LOCALE_CATALOG);
  useLocaleEffects(session);
  // 首次渲染前恢复 hv.theme，应用显示时已是目标主题。
  const themeState: ThemeState = useThemeEffects();
  // Router 只在语言就绪后挂载：已保存的非默认语言首帧只渲染空壳，此时作废快照会让
  // 冷加载地址里的 query 在 Router 规范化后无从恢复。快照保留到 Router 首次渲染结束。
  const routerRendered = createValueMemo(() => session.isReady());
  createEffect(routerRendered, (ready) => {
    if (ready) {
      props.initialLocation.expire();
    }
  });
  return (
    <LocaleSessionContext value={session}>
      <ThemeStateContext value={themeState}>
        <Show when={routerRendered()} fallback={<div class="app-root"></div>}>
          <div class="app-root">
            <StaleBuildBanner
              message={t("stale-build-notice")}
              refreshLabel={t("stale-build-refresh")}
            />
            <RouterHost />
          </div>
        </Show>
      </ThemeStateContext>
    </LocaleSessionContext>
  );
}

function RouterHost(): JSX.Element {
  const router = createRouter({ routes: PUBLIC_ROUTES });
  return (
    <RouterContext value={router}>
      <Show when={router.current().kind === "matched"} fallback={<NotFoundPage />}>
        <PublicShell />
      </Show>
    </RouterContext>
  );
}
