mod evm;
mod solana;
mod stellar;

pub use evm::*;
pub use solana::*;
pub use stellar::*;

use core::time::Duration;

pub trait Network {
    fn average_blocktime(&self) -> Option<Duration>;
    fn public_rpc_urls(&self) -> &'static [&'static str];
    fn explorer_urls(&self) -> &'static [&'static str];
}
