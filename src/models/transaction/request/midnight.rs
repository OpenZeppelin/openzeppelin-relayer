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
    /// Token type (e.g., "02000000000000000000000000000000000000000000000000000000000000000000")
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
    /// Token type (e.g., "02000000000000000000000000000000000000000000000000000000000000000000")
    pub token_type: String,
    /// Amount to send (in smallest unit)
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn test_midnight_input_request_creation() {
        let input = MidnightInputRequest {
            origin: "b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c".to_string(),
            token_type: "02000000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            value: "1000000".to_string(),
        };

        assert_eq!(input.origin.len(), 64); // Hex encoded 32 bytes
        assert_eq!(input.token_type.len(), 68); // Hex encoded 34 bytes
        assert_eq!(input.value, "1000000");
    }

    #[test]
    fn test_midnight_output_request_creation() {
        let output = MidnightOutputRequest {
            destination: "ttmnta1a0q8lwqqsm5qc8tgj2kp5wnmvhthpqvutpvfpajpqkcqr4s8xgmkepxskr2jkc"
                .to_string(),
            token_type: "02000000000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
            value: "500000".to_string(),
        };

        assert!(!output.destination.is_empty());
        assert_eq!(output.token_type.len(), 68);
        assert_eq!(output.value, "500000");
    }

    #[test]
    fn test_midnight_offer_request_creation() {
        let offer = MidnightOfferRequest {
            inputs: vec![MidnightInputRequest {
                origin: "b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c"
                    .to_string(),
                token_type: "02000000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
                value: "1000000".to_string(),
            }],
            outputs: vec![MidnightOutputRequest {
                destination:
                    "ttmnta1a0q8lwqqsm5qc8tgj2kp5wnmvhthpqvutpvfpajpqkcqr4s8xgmkepxskr2jkc"
                        .to_string(),
                token_type: "02000000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
                value: "900000".to_string(),
            }],
        };

        assert_eq!(offer.inputs.len(), 1);
        assert_eq!(offer.outputs.len(), 1);
    }

    #[test]
    fn test_midnight_transaction_request_with_guaranteed_offer() {
        let request = MidnightTransactionRequest {
            ttl: Some((Utc::now() + Duration::minutes(5)).to_rfc3339()),
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![MidnightInputRequest {
                    origin: "b49408db310c043ab736fb57a98e15c8cedbed4c38450df3755ac9726ee14d0c"
                        .to_string(),
                    token_type:
                        "02000000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    value: "1000000".to_string(),
                }],
                outputs: vec![MidnightOutputRequest {
                    destination:
                        "ttmnta1a0q8lwqqsm5qc8tgj2kp5wnmvhthpqvutpvfpajpqkcqr4s8xgmkepxskr2jkc"
                            .to_string(),
                    token_type:
                        "02000000000000000000000000000000000000000000000000000000000000000000"
                            .to_string(),
                    value: "900000".to_string(),
                }],
            }),
            intents: vec![],
            fallible_offers: vec![],
        };

        assert!(request.guaranteed_offer.is_some());
        assert_eq!(request.intents.len(), 0);
        assert_eq!(request.fallible_offers.len(), 0);
        assert!(request.ttl.is_some());
    }

    #[test]
    fn test_midnight_intent_request_creation() {
        let intent = MidnightIntentRequest {
            segment_id: 1,
            actions: vec![MidnightContractAction {}],
        };

        assert_eq!(intent.segment_id, 1);
        assert_eq!(intent.actions.len(), 1);
    }

    #[test]
    fn test_midnight_transaction_request_with_fallible_offers() {
        let request = MidnightTransactionRequest {
            ttl: None,
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![
                (1, MidnightOfferRequest {
                    inputs: vec![],
                    outputs: vec![
                        MidnightOutputRequest {
                            destination: "ttmnta1a0q8lwqqsm5qc8tgj2kp5wnmvhthpqvutpvfpajpqkcqr4s8xgmkepxskr2jkc".to_string(),
                            token_type: "02000000000000000000000000000000000000000000000000000000000000000000".to_string(),
                            value: "50000".to_string(),
                        },
                    ],
                }),
                (2, MidnightOfferRequest {
                    inputs: vec![],
                    outputs: vec![
                        MidnightOutputRequest {
                            destination: "ttmnta1a0q8lwqqsm5qc8tgj2kp5wnmvhthpqvutpvfpajpqkcqr4s8xgmkepxskr2jkc".to_string(),
                            token_type: "02000000000000000000000000000000000000000000000000000000000000000000".to_string(),
                            value: "75000".to_string(),
                        },
                    ],
                }),
            ],
        };

        assert_eq!(request.fallible_offers.len(), 2);
        assert_eq!(request.fallible_offers[0].0, 1); // Segment ID
        assert_eq!(request.fallible_offers[1].0, 2); // Segment ID
    }

    #[test]
    fn test_midnight_transaction_request_serialization() {
        let request = MidnightTransactionRequest {
            ttl: Some((Utc::now() + Duration::hours(1)).to_rfc3339()),
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![MidnightInputRequest {
                    origin: "aabbccdd".repeat(8),                    // 64 chars
                    token_type: "02".to_string() + &"00".repeat(33), // 68 chars
                    value: "500000".to_string(),
                }],
                outputs: vec![MidnightOutputRequest {
                    destination: "ttmnta1recipient".to_string(),
                    token_type: "02".to_string() + &"00".repeat(33),
                    value: "450000".to_string(),
                }],
            }),
            intents: vec![],
            fallible_offers: vec![],
        };

        // Test serialization/deserialization
        let serialized = serde_json::to_string(&request).unwrap();
        let deserialized: MidnightTransactionRequest = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.guaranteed_offer.unwrap().inputs.len(), 1);
        assert_eq!(deserialized.ttl, request.ttl);
    }
}
