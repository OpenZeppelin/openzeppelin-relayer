use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Request structure for Midnight transactions
/// For simple transfers, use only the guaranteed_offer field
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightTransactionRequest {
    /// The guaranteed offer (used for simple transfers and guaranteed operations)
    pub guaranteed_offer: Option<MidnightOfferRequest>,

    /// Intents for complex contract interactions
    pub intents: Vec<MidnightIntentRequest>,

    /// Fallible offers that may fail independently
    pub fallible_offers: Vec<(u16, MidnightOfferRequest)>, // (segment_id, offer)

    /// Transaction time-to-live as ISO timestamp
    pub ttl: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightIntentRequest {
    pub segment_id: u16,
    pub actions: Vec<MidnightContractAction>,
}

/// Offer request that will be converted to Midnight's OfferInfo
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightOfferRequest {
    /// Inputs to be consumed
    pub inputs: Vec<MidnightInputRequest>,
    /// Outputs to be created
    pub outputs: Vec<MidnightOutputRequest>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightContractAction {
    // Contract-specific action data
    // TODO: Define based on Midnight contract interaction patterns
}

/// Input request that will be converted to Midnight's InputInfo
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightInputRequest {
    /// Origin wallet seed (hex encoded)
    pub origin: String,
    /// Token type (e.g., "tDUST" or "NATIVE_TOKEN")
    pub token_type: String,
    /// Amount to consume (in smallest unit)
    pub value: String,
}

/// Output request that will be converted to Midnight's OutputInfo
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MidnightOutputRequest {
    /// Destination wallet seed (hex encoded)
    /// TODO: Change this to wallet address once updated by Midnight team
    pub destination: String,
    /// Token type (e.g., "tDUST" or "NATIVE_TOKEN")
    pub token_type: String,
    /// Amount to send (in smallest unit)
    pub value: String,
}
