use crate::models::{ApiError, RelayerRepoModel};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::ToSchema;

/// Segment ID used for the top-level `guaranteed_offer` field.
///
/// The builder uses this as the reserved segment for top-level unshielded
/// offers, so explicit `intents` and `fallible_offers` cannot reuse it when
/// `guaranteed_offer` is set.
pub const GUARANTEED_OFFER_SEGMENT_ID: u16 = 1;

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightTransactionRequest {
    pub guaranteed_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub intents: Vec<MidnightIntentRequest>,
    #[serde(default)]
    pub fallible_offers: Vec<MidnightFallibleOfferRequest>,
    /// Fallible shielded offers keyed by segment_id. Separate from
    /// `fallible_offers` because shielded and unshielded fallibles live
    /// in different slots of the `StandardTransaction` — unshielded on
    /// intent, shielded on `fallible_coins`. Pairing the same
    /// `segment_id` across both lists is how a shield-op is expressed
    /// (unshielded input in one list, shielded output in the other,
    /// balanced by the chain at segment boundary).
    #[serde(default)]
    pub fallible_shielded_offers: Vec<MidnightFallibleOfferRequest>,
    pub ttl: Option<String>,
}

/// A single intent, bundling offers and/or contract actions under one segment.
///
/// An intent must contribute at least one offer or one action — an otherwise
/// empty intent is rejected at [`MidnightTransactionRequest::validate`].
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightIntentRequest {
    pub segment_id: u16,
    #[serde(default)]
    pub guaranteed_unshielded_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub fallible_unshielded_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub guaranteed_shielded_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub fallible_shielded_offer: Option<MidnightOfferRequest>,
    #[serde(default)]
    pub actions: Vec<MidnightContractAction>,
}

/// Offer payload, discriminated by `kind` so a single offer type can carry
/// either unshielded or shielded data without ambiguity.
///
/// **Breaking change note**: this replaces the pre-PR-1 flat
/// `{inputs, outputs}` payload. Callers must include `"kind": "unshielded" | "shielded"`.
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MidnightOfferRequest {
    Unshielded {
        #[serde(default)]
        inputs: Vec<MidnightUnshieldedInputRequest>,
        #[serde(default)]
        outputs: Vec<MidnightUnshieldedOutputRequest>,
    },
    Shielded {
        #[serde(default)]
        inputs: Vec<MidnightShieldedInputRequest>,
        #[serde(default)]
        outputs: Vec<MidnightShieldedOutputRequest>,
    },
}

impl MidnightOfferRequest {
    pub fn input_count(&self) -> usize {
        match self {
            Self::Unshielded { inputs, .. } => inputs.len(),
            Self::Shielded { inputs, .. } => inputs.len(),
        }
    }
    pub fn output_count(&self) -> usize {
        match self {
            Self::Unshielded { outputs, .. } => outputs.len(),
            Self::Shielded { outputs, .. } => outputs.len(),
        }
    }
    pub fn is_shielded(&self) -> bool {
        matches!(self, Self::Shielded { .. })
    }
}

/// Contract invocation action inside a `MidnightIntentRequest`.
///
/// Accepted as an empty struct in PR-1 (schema placeholder). PR-2 flips this
/// to a tagged enum with a `Call` variant that carries contract address,
/// entry point, arguments, and a `key_location` for VK resolution.
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct MidnightContractAction {}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightUnshieldedInputRequest {
    pub origin: String,
    pub token_type: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightUnshieldedOutputRequest {
    pub destination: String,
    pub token_type: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightShieldedInputRequest {
    pub origin: String,
    pub token_type: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MidnightShieldedOutputRequest {
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
            && self.fallible_shielded_offers.is_empty()
        {
            return Err(ApiError::BadRequest(
                "At least one of guaranteed_offer, intents, fallible_offers, or fallible_shielded_offers must be provided"
                    .to_string(),
            ));
        }

        let mut seen_segments: HashSet<u16> = HashSet::new();
        if self.guaranteed_offer.is_some() {
            seen_segments.insert(GUARANTEED_OFFER_SEGMENT_ID);
        }
        for (idx, f) in self.fallible_offers.iter().enumerate() {
            if !seen_segments.insert(f.segment_id) {
                return Err(ApiError::BadRequest(format!(
                    "fallible_offers[{idx}].segment_id {} is already in use (segment {} is reserved for the top-level guaranteed_offer)",
                    f.segment_id, GUARANTEED_OFFER_SEGMENT_ID,
                )));
            }
        }
        for (idx, intent) in self.intents.iter().enumerate() {
            if !seen_segments.insert(intent.segment_id) {
                return Err(ApiError::BadRequest(format!(
                    "intents[{idx}].segment_id {} is already in use (segment {} is reserved for the top-level guaranteed_offer)",
                    intent.segment_id, GUARANTEED_OFFER_SEGMENT_ID,
                )));
            }
        }

        if let Some(offer) = &self.guaranteed_offer {
            validate_offer(offer, "guaranteed_offer", /*allow_shielded*/ true)?;
        }
        for (idx, f) in self.fallible_offers.iter().enumerate() {
            validate_offer(
                &f.offer,
                &format!("fallible_offers[{idx}].offer"),
                /*allow_shielded*/ false,
            )?;
        }
        for (idx, intent) in self.intents.iter().enumerate() {
            validate_intent(intent, idx)?;
        }

        // PR-3 v2: `fallible_shielded_offers` lives in a separate
        // segment-id namespace from `fallible_offers` / `intents` because
        // the shielded fallible slot is `StandardTransaction.fallible_coins`
        // not an intent's unshielded offer. Each entry must be kind:shielded
        // and segment_ids within this list are unique (but may collide
        // with unshielded fallibles at the same segment — that's the
        // shield-op pairing).
        let mut shielded_seen: HashSet<u16> = HashSet::new();
        for (idx, f) in self.fallible_shielded_offers.iter().enumerate() {
            if !f.offer.is_shielded() {
                return Err(ApiError::BadRequest(format!(
                    "fallible_shielded_offers[{idx}].offer: must be `kind: \"shielded\"`; use `fallible_offers` for unshielded"
                )));
            }
            if !shielded_seen.insert(f.segment_id) {
                return Err(ApiError::BadRequest(format!(
                    "fallible_shielded_offers[{idx}].segment_id {} is already in use in the shielded fallible list",
                    f.segment_id
                )));
            }
            validate_offer(
                &f.offer,
                &format!("fallible_shielded_offers[{idx}].offer"),
                /*allow_shielded*/ true,
            )?;
        }

        Ok(())
    }
}

/// Per-offer size caps and builder-feature gates.
///
/// `allow_shielded`: Shielded offers are supported in top-level
/// `guaranteed_offer` and in `fallible_shielded_offers`. They are rejected
/// from unshielded fallible slots and intent-nested shielded slots because
/// those API surfaces do not map to the builder yet.
fn validate_offer(
    offer: &MidnightOfferRequest,
    loc: &str,
    allow_shielded: bool,
) -> Result<(), ApiError> {
    if offer.input_count() > MAX_OFFER_ENTRIES {
        return Err(ApiError::BadRequest(format!(
            "{loc}.inputs exceeds limit of {MAX_OFFER_ENTRIES}"
        )));
    }
    if offer.output_count() > MAX_OFFER_ENTRIES {
        return Err(ApiError::BadRequest(format!(
            "{loc}.outputs exceeds limit of {MAX_OFFER_ENTRIES}"
        )));
    }
    if offer.is_shielded() && !allow_shielded {
        return Err(ApiError::BadRequest(format!(
            "{loc}: shielded offers are only supported in top-level `guaranteed_offer` or `fallible_shielded_offers`",
        )));
    }
    validate_supported_offer_fields(offer, loc)?;
    Ok(())
}

fn validate_supported_offer_fields(
    offer: &MidnightOfferRequest,
    loc: &str,
) -> Result<(), ApiError> {
    match offer {
        MidnightOfferRequest::Unshielded { inputs, outputs } => {
            for (idx, input) in inputs.iter().enumerate() {
                if input.origin != "self" {
                    return Err(ApiError::BadRequest(format!(
                        "{loc}.inputs[{idx}].origin: only \"self\" is supported"
                    )));
                }
                if input.token_type != "NIGHT" {
                    return Err(ApiError::BadRequest(format!(
                        "{loc}.inputs[{idx}].token_type: only \"NIGHT\" is supported for unshielded inputs"
                    )));
                }
            }
            for (idx, output) in outputs.iter().enumerate() {
                if output.token_type != "NIGHT" {
                    return Err(ApiError::BadRequest(format!(
                        "{loc}.outputs[{idx}].token_type: only \"NIGHT\" is supported for unshielded outputs"
                    )));
                }
            }
        }
        MidnightOfferRequest::Shielded { inputs, .. } => {
            for (idx, input) in inputs.iter().enumerate() {
                if input.origin != "self" {
                    return Err(ApiError::BadRequest(format!(
                        "{loc}.inputs[{idx}].origin: only \"self\" is supported"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_intent(intent: &MidnightIntentRequest, idx: usize) -> Result<(), ApiError> {
    let prefix = format!("intents[{idx}]");
    let slots: [(&Option<MidnightOfferRequest>, &str); 4] = [
        (
            &intent.guaranteed_unshielded_offer,
            "guaranteed_unshielded_offer",
        ),
        (
            &intent.fallible_unshielded_offer,
            "fallible_unshielded_offer",
        ),
        (
            &intent.guaranteed_shielded_offer,
            "guaranteed_shielded_offer",
        ),
        (&intent.fallible_shielded_offer, "fallible_shielded_offer"),
    ];
    for (offer_opt, slot_name) in slots {
        if let Some(offer) = offer_opt {
            let loc = format!("{prefix}.{slot_name}");
            // Intent-nested shielded slots aren't wire-able in PR-3 v1 (same
            // `Segment::Guaranteed` hardcode constraint as fallible_offers).
            if slot_name.contains("shielded") && !slot_name.contains("unshielded") {
                return Err(ApiError::BadRequest(format!(
                    "{loc}: intent-nested shielded offers are not yet supported; use the top-level `guaranteed_offer` with `kind: \"shielded\"` instead"
                )));
            }
            validate_offer(offer, &loc, /*allow_shielded*/ false)?;
        }
    }
    if !intent.actions.is_empty() {
        return Err(ApiError::BadRequest(format!(
            "{prefix}.actions: contract actions are not yet supported by the Midnight transaction builder",
        )));
    }
    if intent.guaranteed_unshielded_offer.is_none()
        && intent.fallible_unshielded_offer.is_none()
        && intent.guaranteed_shielded_offer.is_none()
        && intent.fallible_shielded_offer.is_none()
        && intent.actions.is_empty()
    {
        return Err(ApiError::BadRequest(format!(
            "{prefix}: intent must contain at least one offer or action",
        )));
    }
    Ok(())
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

    fn simple_unshielded_offer() -> MidnightOfferRequest {
        MidnightOfferRequest::Unshielded {
            inputs: vec![MidnightUnshieldedInputRequest {
                origin: "self".into(),
                token_type: "NIGHT".into(),
                value: "1000000".into(),
            }],
            outputs: vec![MidnightUnshieldedOutputRequest {
                destination: "self".into(),
                token_type: "NIGHT".into(),
                value: "1000000".into(),
            }],
        }
    }

    fn simple_shielded_offer() -> MidnightOfferRequest {
        MidnightOfferRequest::Shielded {
            inputs: vec![],
            outputs: vec![MidnightShieldedOutputRequest {
                destination: "mn_shield-addr_preview1abc".into(),
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
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("At least one")));
    }

    #[test]
    fn accepts_guaranteed_offer_only() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_unshielded_offer()),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        assert!(req.validate(&relayer()).is_ok());
    }

    #[test]
    fn accepts_multi_fallible_with_distinct_segments() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![
                MidnightFallibleOfferRequest {
                    segment_id: 2,
                    offer: simple_unshielded_offer(),
                },
                MidnightFallibleOfferRequest {
                    segment_id: 3,
                    offer: simple_unshielded_offer(),
                },
            ],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        assert!(req.validate(&relayer()).is_ok());
    }

    #[test]
    fn rejects_duplicate_fallible_segment_ids() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![
                MidnightFallibleOfferRequest {
                    segment_id: 2,
                    offer: simple_unshielded_offer(),
                },
                MidnightFallibleOfferRequest {
                    segment_id: 2,
                    offer: simple_unshielded_offer(),
                },
            ],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("already in use")));
    }

    #[test]
    fn rejects_fallible_using_guaranteed_reserved_segment() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_unshielded_offer()),
            intents: vec![],
            fallible_offers: vec![MidnightFallibleOfferRequest {
                segment_id: GUARANTEED_OFFER_SEGMENT_ID,
                offer: simple_unshielded_offer(),
            }],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("reserved")));
    }

    #[test]
    fn accepts_shielded_top_level_guaranteed_offer() {
        // PR-3 v1: shielded offers are wire-able at the top-level
        // `guaranteed_offer` slot (they route to `set_guaranteed_offer` on
        // the tx). The `rejects_shielded_offer_until_pr3` test that used to
        // guard this assertion is intentionally removed.
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(simple_shielded_offer()),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        assert!(req.validate(&relayer()).is_ok());
    }

    #[test]
    fn rejects_shielded_offer_in_fallible_offers() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![],
            fallible_offers: vec![MidnightFallibleOfferRequest {
                segment_id: 2,
                offer: simple_shielded_offer(),
            }],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("top-level `guaranteed_offer` or `fallible_shielded_offers`"))
        );
    }

    #[test]
    fn rejects_unshielded_input_origin_other_than_self() {
        let mut offer = simple_unshielded_offer();
        let MidnightOfferRequest::Unshielded { inputs, .. } = &mut offer else {
            panic!("expected unshielded offer");
        };
        inputs[0].origin = "mn_addr_preview1other".into();
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(offer),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("only \"self\"")));
    }

    #[test]
    fn rejects_unshielded_token_type_other_than_night() {
        let mut offer = simple_unshielded_offer();
        let MidnightOfferRequest::Unshielded { inputs, outputs } = &mut offer else {
            panic!("expected unshielded offer");
        };
        inputs[0].token_type = "DUST".into();
        outputs[0].token_type = "DUST".into();
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(offer),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("only \"NIGHT\"")));
    }

    #[test]
    fn rejects_shielded_input_origin_other_than_self() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest::Shielded {
                inputs: vec![MidnightShieldedInputRequest {
                    origin: "mn_shield-addr_preview1other".into(),
                    token_type: "NIGHT".into(),
                    value: "1".into(),
                }],
                outputs: vec![],
            }),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("only \"self\"")));
    }

    #[test]
    fn rejects_shielded_offer_in_intent_slots() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![MidnightIntentRequest {
                segment_id: 5,
                guaranteed_unshielded_offer: None,
                fallible_unshielded_offer: None,
                guaranteed_shielded_offer: Some(simple_shielded_offer()),
                fallible_shielded_offer: None,
                actions: vec![],
            }],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("intent-nested shielded offers are not yet supported"))
        );
    }

    #[test]
    fn rejects_non_empty_actions_until_pr2() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![MidnightIntentRequest {
                segment_id: 5,
                guaranteed_unshielded_offer: None,
                fallible_unshielded_offer: Some(simple_unshielded_offer()),
                guaranteed_shielded_offer: None,
                fallible_shielded_offer: None,
                actions: vec![MidnightContractAction::default()],
            }],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("contract actions are not yet supported"))
        );
    }

    #[test]
    fn rejects_empty_intent() {
        let req = MidnightTransactionRequest {
            guaranteed_offer: None,
            intents: vec![MidnightIntentRequest {
                segment_id: 5,
                guaranteed_unshielded_offer: None,
                fallible_unshielded_offer: None,
                guaranteed_shielded_offer: None,
                fallible_shielded_offer: None,
                actions: vec![],
            }],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(
            matches!(err, ApiError::BadRequest(ref m) if m.contains("must contain at least one"))
        );
    }

    #[test]
    fn rejects_oversized_inputs() {
        let input = MidnightUnshieldedInputRequest {
            origin: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest::Unshielded {
                inputs: vec![input; MAX_OFFER_ENTRIES + 1],
                outputs: vec![],
            }),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("inputs exceeds")));
    }

    #[test]
    fn rejects_oversized_outputs() {
        let output = MidnightUnshieldedOutputRequest {
            destination: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest::Unshielded {
                inputs: vec![],
                outputs: vec![output; MAX_OFFER_ENTRIES + 1],
            }),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
            ttl: None,
        };
        let err = req.validate(&relayer()).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(ref m) if m.contains("outputs exceeds")));
    }

    #[test]
    fn accepts_at_limit_boundary() {
        let input = MidnightUnshieldedInputRequest {
            origin: "self".into(),
            token_type: "NIGHT".into(),
            value: "1".into(),
        };
        let req = MidnightTransactionRequest {
            guaranteed_offer: Some(MidnightOfferRequest::Unshielded {
                inputs: vec![input; MAX_OFFER_ENTRIES],
                outputs: vec![],
            }),
            intents: vec![],
            fallible_offers: vec![],
            fallible_shielded_offers: vec![],
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

    #[test]
    fn parses_tagged_unshielded_offer_from_json() {
        let json = r#"{
            "guaranteed_offer": {
                "kind": "unshielded",
                "inputs":  [{"origin":"self","token_type":"NIGHT","value":"1"}],
                "outputs": [{"destination":"self","token_type":"NIGHT","value":"1"}]
            }
        }"#;
        let req: MidnightTransactionRequest = serde_json::from_str(json).unwrap();
        assert!(matches!(
            req.guaranteed_offer.as_ref().unwrap(),
            MidnightOfferRequest::Unshielded { .. }
        ));
    }

    #[test]
    fn rejects_missing_kind_discriminator_at_parse_time() {
        // A flat offer body without `"kind"` is the pre-PR-1 shape. After
        // PR-1 it must fail to deserialize (tagged enum requires the tag).
        let json = r#"{
            "guaranteed_offer": {
                "inputs":  [{"origin":"self","token_type":"NIGHT","value":"1"}],
                "outputs": [{"destination":"self","token_type":"NIGHT","value":"1"}]
            }
        }"#;
        let parsed: Result<MidnightTransactionRequest, _> = serde_json::from_str(json);
        assert!(parsed.is_err());
    }
}
