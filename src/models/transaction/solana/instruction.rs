//! Solana instruction specifications for transaction building

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Specification for a Solana instruction
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct SolanaInstructionSpec {
    /// Program ID (base58-encoded pubkey)
    pub program_id: String,
    /// Account metadata for the instruction
    pub accounts: Vec<SolanaAccountMeta>,
    /// Instruction data (base64-encoded)
    pub data: String,
}

/// Account metadata for a Solana instruction
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq)]
pub struct SolanaAccountMeta {
    /// Account public key (base58-encoded)
    pub pubkey: String,
    /// Whether the account is a signer
    pub is_signer: bool,
    /// Whether the account is writable
    pub is_writable: bool,
}
