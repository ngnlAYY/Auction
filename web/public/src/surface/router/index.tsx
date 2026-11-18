//! Public 语言目录与启动入口；路由表见子模块 `route`，页面外壳见 `shell` 模块。

import {
  type ThemeState,
  ThemeStateContext,
  useThemeEffects,
} from "@auction/web-shared/platform/theme";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { StaleBuildBanner } from "@auction/web-shared/ui/build-notice";
import { installSharedUi } from "@auction/web-shared/ui/components";
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
import { createEffect, Show, useContext } from "solid-js";
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
import { PUBLIC_ROUTES } from "./route";

export { PUBLIC_LOCALE_CATALOG } from "./locale-catalog";

export function App(): JSX.Element {
  // 两个首次地址快照都只在 Router 首次渲染的规范化窗口有效。
  const initialLocation = initialPublicLocation();
  const initialReturn = initialLoginReturnLocation();
  const built = buildBundle(
    DEFAULT_PUBLIC_LOCALE,
    PUBLIC_LOCALE_CATALOG.defaultFtl,
    DEFAULT_PUBLIC_LOCALE,
    null,
  );
  return (
    <InitialPublicLocationContext value={initialLocation}>
      <InitialLoginReturnLocationContext value={initialReturn}>
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
  const initialReturn = useContext(InitialLoginReturnLocationContext);
  // 已保存的非默认语言首帧只渲染空壳；等 Router 真正挂载后才作废首次地址。
  const routerRendered = createValueMemo(() => session.isReady());
  createEffect(routerRendered, (ready) => {
    if (ready) {
      props.initialLocation.expire();
      initialReturn?.expire();
    }
  });
  return (
    <LocaleSessionContext value={session}>
      <ThemeStateContext value={themeState}>
        <Show when={routerRendered()} fallback={<div class="app-root"></div>}>
          <SharedUiEffects />
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

/**
 * 安装两端共用的页面级副作用：表格截断提示与 VisualViewport 监听。
 * 对应 Rust 版根组件里的 `BaseStylesheet {}`：它不输出节点（样式由入口静态装配），
 * 但语言就绪后必须挂载一次，否则手机软键盘与表格提示的行为缺失。
 */
function SharedUiEffects(): JSX.Element {
  installSharedUi();
  return null;
}

function RouterHost(): JSX.Element {
  const router = createRouter({ routes: PUBLIC_ROUTES });
  return (
    <RouterContext value={router}>
      <PublicShell />
    </RouterContext>
  );
}
