use crate::models::{ApiError, RelayerRepoModel};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightTransactionRequest {
    pub guaranteed_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub intents: Vec<MidnightIntentRequest>,
    #[serde(default)]
    pub fallible_offers: Vec<MidnightFallibleOfferRequest>,
    pub ttl: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightIntentRequest {
    pub segment_id: u16,
    #[serde(default)]
    pub actions: Vec<MidnightContractAction>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightOfferRequest {
    #[serde(default)]
    pub inputs: Vec<MidnightInputRequest>,
    #[serde(default)]
    pub outputs: Vec<MidnightOutputRequest>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct MidnightContractAction {}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightInputRequest {
    pub origin: String,
    pub token_type: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightOutputRequest {
    pub destination: String,
    pub token_type: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightFallibleOfferRequest {
    pub segment_id: u16,
    pub offer: MidnightOfferRequest,
}

impl MidnightTransactionRequest {
    pub fn validate(&self, _relayer: &RelayerRepoModel) -> Result<(), ApiError> {
        if self.guaranteed_offer.is_none()
            && self.intents.is_empty()
            && self.fallible_offers.is_empty()
        {
            return Err(ApiError::BadRequest(
                "At least one of guaranteed_offer, intents, or fallible_offers must be provided"
                    .to_string(),
            ));
        }

        Ok(())
    }
}
