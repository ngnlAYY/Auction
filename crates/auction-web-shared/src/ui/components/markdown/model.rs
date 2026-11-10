//! Markdown 解析结果的内部模型。

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MarkdownBlock {
    Heading { level: u8, nodes: Vec<Inline> },
    Paragraph(Vec<Inline>),
    UnorderedList(Vec<Vec<Inline>>),
    OrderedList { start: usize, items: Vec<Vec<Inline>> },
    Quote(Vec<Inline>),
    Table { headers: Vec<Vec<Inline>>, rows: Vec<Vec<Vec<Inline>>> },
    Code { language: Option<String>, value: String },
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Link { label: Vec<Inline>, target: String },
    Image { alt: String, source: String },
}
