use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema)]
pub struct MidnightTransactionRequest {
    pub intents: Vec<MidnightIntentRequest>,
    pub guaranteed_offer: Option<MidnightOfferRequest>,
    pub fallible_offers: Vec<(u16, MidnightOfferRequest)>, // (segment_id, offer)
    pub ttl: Option<String>,                               // ISO timestamp
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct MidnightIntentRequest {
    pub segment_id: u16,
    pub actions: Vec<MidnightContractAction>,
    // TODO: Add more fields as we implement
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct MidnightOfferRequest {
    // TODO: Add more fields as we implement
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct MidnightContractAction {
    // TODO: Add more fields as we implement
}
