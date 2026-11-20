//! 寄售写入正文的脱敏解码，仅从闭集键中报告出错字段。

// 不变量 6、7：丢弃可能含输入原文的解码错误，只返回固定字段名与错误类别。
pub(in crate::dto) fn redact_error<E: serde::de::Error>(
    _source: E,
    safe_message: impl std::fmt::Display,
) -> E {
    E::custom(safe_message)
}

macro_rules! write_object {
    (
        $(#[$attr:meta])*
        pub struct $name:ident {
            $($(#[$field_attr:meta])* pub $field:ident: $ty:ty => ($decode:literal, $invalid:literal),)*
        }
    ) => {
        $(#[$attr])*
        #[derive(serde::Serialize)]
        pub struct $name {
            $($(#[$field_attr])* pub $field: $ty,)*
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct RequestVisitor<'a>(&'a std::cell::Cell<bool>);

                impl<'de> serde::de::Visitor<'de> for RequestVisitor<'_> {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        formatter.write_str("寄售写入 JSON 对象")
                    }

                    fn visit_map<M: serde::de::MapAccess<'de>>(
                        self, mut map: M,
                    ) -> Result<Self::Value, M::Error> {
                        self.0.set(true);
                        $(let mut $field = None;)*
                        while let Some(field) = map.next_key::<String>().map_err(|error| {
                            $crate::dto::public::consignment::write::redact_error(error, "寄售写入字段名无效")
                        })? {
                            match field.as_str() {
                                $(stringify!($field) => {
                                    if $field.is_some() {
                                        return Err(serde::de::Error::duplicate_field(stringify!($field)));
                                    }

                                    #[derive(serde::Deserialize)]
                                    #[serde(transparent)]
                                    struct Field(#[serde(deserialize_with = $decode)] $ty);

                                    let value = map.next_value::<Field>().map_err(|error| {
                                        $crate::dto::public::consignment::write::redact_error(
                                            error, concat!(stringify!($field), " 字段", $invalid),
                                        )
                                    })?;
                                    $field = Some(value.0);
                                })*
                                _ => return Err(serde::de::Error::custom("寄售写入含未知字段")),
                            }
                        }
                        Ok($name {
                            $($field: $field.ok_or_else(|| {
                                serde::de::Error::missing_field(stringify!($field))
                            })?,)*
                        })
                    }
                }

                let visited = std::cell::Cell::new(false);
                deserializer.deserialize_map(RequestVisitor(&visited)).map_err(|error| {
                    if visited.get() {
                        return error;
                    }
                    $crate::dto::public::consignment::write::redact_error(error, "寄售写入正文必须为 JSON 对象")
                })
            }
        }
    };
}

pub(in crate::dto) use write_object;
