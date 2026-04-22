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

/// Maximum number of inputs or outputs accepted inside a single offer.
/// The ZK prover's work is linear in this count; callers that need more are
/// rejected at the API boundary to avoid a single request monopolising a
/// prover slot and starving other transactions.
const MAX_OFFER_ENTRIES: usize = 50;

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

        // The builder currently only honors `guaranteed_offer`. Accepting
        // non-empty `intents` / `fallible_offers` would silently drop the
        // caller's data (the builder stores them but never feeds them to
        // `tx_info`), so reject explicitly until intent/fallible-segment
        // wiring lands. See docs/midnight-architecture.md for the roadmap.
        if !self.intents.is_empty() {
            return Err(ApiError::BadRequest(
                "intents are not yet supported by the Midnight transaction builder".to_string(),
            ));
        }
        if !self.fallible_offers.is_empty() {
            return Err(ApiError::BadRequest(
                "fallible_offers are not yet supported by the Midnight transaction builder"
                    .to_string(),
            ));
        }

        if let Some(offer) = &self.guaranteed_offer {
            if offer.inputs.len() > MAX_OFFER_ENTRIES {
                return Err(ApiError::BadRequest(format!(
                    "guaranteed_offer.inputs exceeds limit of {MAX_OFFER_ENTRIES}"
                )));
            }
            if offer.outputs.len() > MAX_OFFER_ENTRIES {
                return Err(ApiError::BadRequest(format!(
                    "guaranteed_offer.outputs exceeds limit of {MAX_OFFER_ENTRIES}"
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{NetworkType, RelayerMidnightPolicy, RelayerNetworkPolicy};

    fn relayer() -> RelayerRepoModel {
        RelayerRepoModel {
            id: "test_relayer".to_string(),
            name: "Test Midnight Relayer".to_string(),
            network: "preview".to_string(),
            network_type: NetworkType::Midnight,
            policies: RelayerNetworkPolicy::Midnight(RelayerMidnightPolicy::default()),
            signer_id: "test_signer".to_string(),
            address: String::new(),
            ..Default::default()
        }
    }

    fn simple_offer() -> MidnightOfferRequest {
        MidnightOfferRequest {
            inputs: vec![MidnightInputRequest {
                origin: "self".into(),
                token_type: "NIGHT".into(),
                value: "1000000".into(),
            }],
            outputs: vec![MidnightOutputRequest {
                destination: "self".into(),
                token_type: "NIGHT".into(),
                value: "1000000".into(),
            }],
        }
    }

    #[test]
    fn rejects_request_with_no_content() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("At least one")));
    }

    #[test]
    fn accepts_guaranteed_offer_only() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_offer()),
            intents: vec![],
            fallible_offers: vec![],
            ttl: None,
        };
        assert!(req.validate(&relayer()).is_ok());
    }

    #[test]
    fn rejects_non_empty_intents() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_offer()),
            intents: vec![MidnightIntentRequest {
                segment_id: 1,
                actions: vec![],
            }],
            fallible_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("intents are not yet supported"))
        );
    }

    #[test]
    fn rejects_non_empty_fallible_offers() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_offer()),
            intents: vec![],
            fallible_offers: vec![MidnightFallibleOfferRequest {
                segment_id: 2,
                offer: simple_offer(),
            }],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("fallible_offers are not yet supported"))
        );
    }

    #[test]
    fn rejects_oversized_inputs() {
        let input = MidnightInputRequest {
            origin: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![input; MAX_OFFER_ENTRIES + 1],
                outputs: vec![],
            }),
            intents: vec![],
            fallible_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("inputs exceeds")));
    }

    #[test]
    fn rejects_oversized_outputs() {
        let output = MidnightOutputRequest {
            destination: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![],
                outputs: vec![output; MAX_OFFER_ENTRIES + 1],
            }),
            intents: vec![],
            fallible_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("outputs exceeds")));
    }

    #[test]
    fn accepts_at_limit_boundary() {
        let input = MidnightInputRequest {
            origin: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest {
                inputs: vec![input; MAX_OFFER_ENTRIES],
                outputs: vec![],
            }),
            intents: vec![],
            fallible_offers: vec![],
            ttl: None,
        };
        assert!(req.validate(&relayer()).is_ok());
    }

    #[test]
    fn rejects_unknown_fields_at_parse_time() {
        // `#[serde(deny_unknown_fields)]` on the request struct is the
        // first line of defense — exercise it so a future edit that drops
        // the attribute is caught by a test failure rather than a silent
        // accept-then-ignore.
        let json = r#"{"guaranteed_offer": null, "mystery_field": 1}"#;
        let parsed: Result<MidnightTransactionRequest, _> = serde_json::from_str(json);
        assert!(parsed.is_err());
    }
}
