//! 世界闭集：MoogleMail 与论坛各自的世界标签。

/// MoogleMail 世界闭集；邮件绝不使用 shared。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailWorld {
    Main,
    Isekai,
}

impl MailWorld {
    pub const ALL: [Self; 2] = [Self::Main, Self::Isekai];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
        }
    }
}

/// 论坛世界闭集；shared 只用于没有权威世界上下文的资料读取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForumWorld {
    Main,
    Isekai,
    Shared,
}

impl ForumWorld {
    pub const ALL: [Self; 3] = [Self::Main, Self::Isekai, Self::Shared];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Isekai => "isekai",
            Self::Shared => "shared",
        }
    }
}
