// 手机菜单及触发按钮；只接收展示状态和事件，不按宽度挂载业务子树。

import type { RouteMatch } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";

import { NavigationLinks } from ".";

/** 手机导航开关；元素句柄回到外壳，供导航内 Escape 收起后归还焦点。 */
export function NavigationToggle(props: {
  open: boolean;
  onToggle: () => void;
  onMount: (element: HTMLButtonElement) => void;
}) {
  return (
    <button
      class="admin-shell__nav-toggle"
      type="button"
      aria-controls="admin-primary-navigation"
      aria-expanded={props.open ? "true" : "false"}
      ref={(element) => {
        props.onMount(element);
      }}
      onClick={() => props.onToggle()}
      onKeyDown={(event) => {
        if (props.open && event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          props.onToggle();
        }
      }}
    >
      {t("nav-label")}
    </button>
  );
}

/** 手机导航面板；点击任意链接后关闭，Escape 交给外壳保留焦点与导航状态。 */
export function Navigation(props: {
  currentRoute: RouteMatch;
  open: boolean;
  onNavigate: () => void;
  onKeyDown: (event: KeyboardEvent) => void;
}) {
  return (
    <nav
      id="admin-primary-navigation"
      class={[
        "admin-shell__nav",
        "admin-shell__nav--mobile",
        { "admin-shell__nav--open": props.open },
      ]}
      aria-label={t("nav-label")}
      onClick={() => props.onNavigate()}
      onKeyDown={(event) => props.onKeyDown(event)}
    >
      <NavigationLinks currentRoute={props.currentRoute} />
    </nav>
  );
}
