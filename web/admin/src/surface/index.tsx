// 管理端浏览器界面（Solid CSR）。
// 路由集合由 `router/route` 定义，与 `crates/auction-http/src/assets/routes.rs` 的 Admin 清单及
// `docs/contracts/routes.md` 逐条同步。管理端与公开端是不同 surface：本包独立拥有
// 自己的组件树和路由树，不复用公开端组件，公开端也不得依赖本包。
// 界面文案取自 `i18n/*.ftl`，默认语言由 `@auction/web-shared/ui/i18n` 的 `DEFAULT_ADMIN_LOCALE`
// （简体中文）决定。受支持语言与公开端一致，但译文资源彼此隔离。
// 本包不得导入存储、配置或凭据访问路径。

import { ThemeStateContext, useThemeEffects } from "@auction/web-shared/platform/theme";
import type { TaskResource } from "@auction/web-shared/reactive/resource";
import type { Result } from "@auction/web-shared/result";
import { createRouter, RouteOutlet, RouterContext } from "@auction/web-shared/router";
import { StaleBuildBanner } from "@auction/web-shared/ui/build-notice";
import { installSharedUi } from "@auction/web-shared/ui/components";
import {
  buildBundle,
  createLocaleSession,
  DEFAULT_ADMIN_LOCALE,
  type LocaleCatalog,
  LocaleSessionContext,
  t,
  useLocaleEffects,
} from "@auction/web-shared/ui/i18n";
import type { FluentBundle } from "@fluent/bundle";
import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { Show, untrack } from "solid-js";
import enUrl from "../../../../crates/auction-web-admin/i18n/en.ftl?url";
import jaUrl from "../../../../crates/auction-web-admin/i18n/ja.ftl?url";
import koUrl from "../../../../crates/auction-web-admin/i18n/ko.ftl?url";
import zhCN from "../../../../crates/auction-web-admin/i18n/zh-CN.ftl?raw";
import zhTWUrl from "../../../../crates/auction-web-admin/i18n/zh-TW.ftl?url";
import { AdminShell } from "./auth";
import { normalizeColdStartAdminLocation } from "./auth/redirect";
import {
  ADMIN_PAGES,
  InitialAdminLocationContext,
  initialAdminLocation,
  NotFoundPage,
} from "./router";
import { ADMIN_ROUTES } from "./router/route";

/** Admin 登录外壳唯一拥有的 readiness 快照；默认 Dashboard 用它协调首屏门禁。 */
export type AdminReadinessResource = TaskResource<Result<boolean, string>>;

/** Admin 语言目录：默认简体中文编译期内嵌，其余四种从带指纹的静态资源按需取回。 */
export const ADMIN_LOCALE_CATALOG: LocaleCatalog = {
  defaultLocale: DEFAULT_ADMIN_LOCALE,
  defaultFtl: zhCN,
  sources: [
    { id: "en", url: enUrl },
    { id: "zh-TW", url: zhTWUrl },
    { id: "ja", url: jaUrl },
    { id: "ko", url: koUrl },
  ],
};

/** 挂载管理端应用；入口文档缺失 `#main` 属于构建装配错误，直接抛出。 */
export function run(): void {
  const root = document.getElementById("main");
  if (root === null) {
    throw new Error("Admin 入口缺少 #main 挂载点");
  }
  render(() => <App />, root);
}
function App(): JSX.Element {
  const initialLocation = initialAdminLocation();
  // 默认语言编译期内嵌；其余语言按目录从静态资源取回，已保存的非默认语言在首屏等待加载。
  const built = buildBundle(
    DEFAULT_ADMIN_LOCALE,
    ADMIN_LOCALE_CATALOG.defaultFtl,
    DEFAULT_ADMIN_LOCALE,
    null,
  );
  return (
    <InitialAdminLocationContext value={initialLocation}>
      <Show
        when={built.ok ? built.value : null}
        fallback={<div class="app-root">无法初始化界面译文。</div>}
      >
        {(bundle) => <LocalizedApp bundle={bundle()} />}
      </Show>
    </InitialAdminLocationContext>
  );
}

/** 译文初始化成功后才挂载语言与主题的 hook 宿主。 */
function LocalizedApp(props: { readonly bundle: FluentBundle }): JSX.Element {
  // `<Show>` 以 getter 传入 bundle；会话初始化只取一次快照，不在组件体隐式订阅它。
  const session = createLocaleSession(
    untrack(() => props.bundle),
    DEFAULT_ADMIN_LOCALE,
  );
  session.bootstrap(ADMIN_LOCALE_CATALOG);
  useLocaleEffects(session);
  // 首次渲染前恢复 hv.theme；登录、首次设置与正式页面共享同一主题。
  const themeState = useThemeEffects();
  return (
    <LocaleSessionContext value={session}>
      <ThemeStateContext value={themeState}>
        <Show when={session.isReady()} fallback={<div class="app-root"></div>}>
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

/** 冷启动地址只消费一次；路由宿主因语言或主题重挂载时不得改写当前地址。 */
let coldStartAdminLocationInitialized = false;

/** 路由宿主：`/setup` 在认证外壳之外，其余页面都由 `AdminShell` 渲染。
 * 外壳自己渲染认证、登录表单与页面出口，因此这里只负责在两种布局之间切换。 */
function RouterHost(): JSX.Element {
  // 必须在 createRouter 读取并规范化初始地址之前执行；每个文档只消费一次冷启动快照。
  if (!coldStartAdminLocationInitialized) {
    coldStartAdminLocationInitialized = true;
    normalizeColdStartAdminLocation();
  }
  const router = createRouter({ routes: ADMIN_ROUTES });
  const setupRoute = (): boolean => {
    const current = router.current();
    return current.kind === "matched" && current.name === "Setup";
  };
  return (
    <RouterContext value={router}>
      <Show when={setupRoute()} fallback={<AdminShell />}>
        <RouteOutlet pages={ADMIN_PAGES} notFound={<NotFoundPage />} />
      </Show>
    </RouterContext>
  );
}
