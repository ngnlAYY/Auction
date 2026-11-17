//! PostgreSQL `timestamptz` 的编解码，随 `postgres` feature 编译。
//!
//! 线上格式是自 2000-01-01 起的微秒数；写入时纳秒以下被截断，读取结果精度为微秒。

use jiff_sqlx::ToSqlx;
use sqlx::Decode;
use sqlx::Encode;
use sqlx::Postgres;
use sqlx::Type;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::PgArgumentBuffer;
use sqlx::postgres::PgHasArrayType;
use sqlx::postgres::PgTypeInfo;
use sqlx::postgres::PgValueRef;

use crate::instant::Timestamp;

impl Type<Postgres> for Timestamp {
    fn type_info() -> PgTypeInfo {
        <jiff_sqlx::Timestamp as Type<Postgres>>::type_info()
    }
}

impl PgHasArrayType for Timestamp {
    fn array_type_info() -> PgTypeInfo {
        <jiff_sqlx::Timestamp as PgHasArrayType>::array_type_info()
    }
}

impl Encode<'_, Postgres> for Timestamp {
    fn encode_by_ref(&self, buffer: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        Encode::<Postgres>::encode(self.to_jiff().to_sqlx(), buffer)
    }
}

impl<'r> Decode<'r, Postgres> for Timestamp {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let decoded = <jiff_sqlx::Timestamp as Decode<'r, Postgres>>::decode(value)?;
        Ok(Self::from(decoded.to_jiff()))
    }
}
