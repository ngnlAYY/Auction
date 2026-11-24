//! 寄售第 4 期收件与代管用例。

mod commands;
pub mod encoding;

pub use commands::{
    ApplyMailProofInput, DepositCommandService, ReleaseQuarantineAdminInput,
    StartDispositionWriteInput,
};

#[cfg(test)]
mod tests;
