// 桌面常驻侧栏；不安装资源、草稿或命令宿主。

import type { RouteMatch } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";

import { NavigationIcon, NavigationLinks } from ".";

/** 桌面侧栏；折叠外观由外壳根节点的 is-sidebar-collapsed 驱动。 */
export function Navigation(props: {
  currentRoute: RouteMatch;
  collapsed: boolean;
  onToggle: () => void;
}) {
  const collapseLabel = () => (props.collapsed ? t("console-expand") : t("console-collapse"));
  return (
    <aside class="console-sidebar" aria-label={t("nav-label")}>
      <NavigationLinks currentRoute={props.currentRoute} />
      <div class="console-sidebar__foot">
        <button
          class="console-collapse"
          type="button"
          aria-expanded={props.collapsed ? "false" : "true"}
          title={collapseLabel()}
          onClick={() => props.onToggle()}
        >
          <NavigationIcon kind="chevron" />
          <span>{collapseLabel()}</span>
        </button>
      </div>
    </aside>
  );
}
