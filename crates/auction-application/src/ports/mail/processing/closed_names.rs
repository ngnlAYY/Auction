//! 邮件处理闭集枚举的声明宏。

/// 声明闭集枚举：一次写出“变体 => 稳定名称”，同时生成枚举、按声明顺序的 `ALL` 与 `as_str`。
/// 新增变体只写这一处，`ALL` 与稳定名称不会漏登记；派生属性与文档写在声明处。
macro_rules! closed_names {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $stable:literal,)+
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $($(#[$variant_meta])* $variant,)+
        }

        impl $name {
            /// 全部变体，按声明顺序。
            pub const ALL: [Self; [$($stable),+].len()] = [$(Self::$variant),+];

            /// 持久化的稳定名称；已发布值逐字不可改写。
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $stable,)+
                }
            }
        }
    };
}

pub(super) use closed_names;
