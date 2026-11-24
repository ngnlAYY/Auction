//! 寄售代管 position、处分代与实物交接历史。

mod generations;
mod history;
mod positions;

pub(crate) use positions::apply_take_custody_in_tx;
