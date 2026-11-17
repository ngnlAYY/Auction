// Link 组件：按 Dioxus Router 的 Link 决定 href、激活态属性与点击拦截顺序。
// 拦截顺序必须保持不变；提前返回时调用方的 onClick 不会执行。

import type { JSX } from "@solidjs/web";
import { omit } from "solid-js";

import { createValueMemo } from "../reactive/memo";
import { useRouter } from "./context";
import { isInternalHref } from "./pattern";

type PassthroughAttributes = Omit<
  JSX.AnchorHTMLAttributes<HTMLAnchorElement>,
  "children" | "class" | "href" | "onClick" | "rel" | "target"
>;

/** Link 的属性；其余属性原样透传给 `<a>`。 */
export interface LinkProps extends PassthroughAttributes {
  readonly href: string;
  readonly class?: JSX.ClassValue;
  /** 当前 href 与实际地址相同时追加的类名。 */
  readonly activeClass?: string;
  readonly rel?: string;
  /** 新标签页打开；同时决定 target 与是否拦截点击。 */
  readonly newTab?: boolean;
  /** 在拦截与可选的路由导航之后调用；见 onClickOnly。 */
  readonly onClick?: (event: MouseEvent) => void;
  /**
   * 为 true 时禁止默认的客户端导航，但仍执行 preventDefault 并调用 onClick。
   * Dioxus 在 onClickOnly 下对外部链接也会 preventDefault。
   */
  readonly onClickOnly?: boolean;
  readonly children?: JSX.Element;
}

function hasClassNames(value: JSX.ClassValue): boolean {
  if (value === undefined || value === null || typeof value === "boolean") {
    return false;
  }
  if (typeof value === "number") {
    return true;
  }
  if (typeof value === "string") {
    return value.trim() !== "";
  }
  if (Array.isArray(value)) {
    return value.some(hasClassNames);
  }
  return Object.entries(value).some(([name, enabled]) => enabled === true && name.trim() !== "");
}

/** 客户端导航链接。 */
export function Link(props: LinkProps): JSX.Element {
  const router = useRouter();
  const active = createValueMemo(() => props.href === router.currentLocationString());
  const external = createValueMemo(() => !isInternalHref(props.href));
  const className = createValueMemo(() => {
    const mixed: JSX.ClassValue = [
      props.class,
      props.activeClass !== undefined && active() ? props.activeClass : undefined,
    ];
    return hasClassNames(mixed) ? mixed : undefined;
  });
  const rel = createValueMemo(() => props.rel ?? (external() ? "noopener noreferrer" : undefined));

  const handleClick = (event: MouseEvent): void => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || event.button !== 0) {
      return;
    }
    if (props.newTab === true) {
      return;
    }
    const doDefault = props.onClick === undefined || props.onClickOnly !== true;
    if (doDefault && external()) {
      return;
    }
    event.preventDefault();
    if (doDefault && !external()) {
      router.push(props.href);
    }
    props.onClick?.(event);
  };

  const attributes = omit(
    props,
    "href",
    "class",
    "activeClass",
    "rel",
    "newTab",
    "onClick",
    "onClickOnly",
    "children",
  );

  return (
    <a
      href={props.href}
      onClick={handleClick}
      class={className()}
      rel={rel()}
      target={props.newTab === true ? "_blank" : undefined}
      aria-current={active() ? "page" : undefined}
      {...attributes}
    >
      {props.children}
    </a>
  );
}
