//! Asset types and conversions for Stellar transactions

use crate::models::SignerError;
use serde::{Deserialize, Serialize};
use soroban_rs::xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, PublicKey as XdrPublicKey,
    Uint256,
};
use std::convert::TryFrom;
use std::str::FromStr;
use stellar_strkey::ed25519::PublicKey;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssetSpec {
    Native,
    Credit4 { code: String, issuer: String },
    Credit12 { code: String, issuer: String },
}

impl TryFrom<AssetSpec> for Asset {
    type Error = SignerError;
    fn try_from(a: AssetSpec) -> Result<Self, Self::Error> {
        Ok(match a {
            AssetSpec::Native => Asset::Native,
            AssetSpec::Credit4 { code, issuer } => {
                let b = code.as_bytes();
                if !(1..=4).contains(&b.len()) {
                    return Err(SignerError::ConversionError("asset code 1-4 chars".into()));
                }
                let mut buf = [0u8; 4];
                buf[..b.len()].copy_from_slice(b);

                let issuer_pk = PublicKey::from_str(&issuer)
                    .map_err(|e| SignerError::ConversionError(format!("Invalid issuer: {}", e)))?;

                let uint256 = Uint256(issuer_pk.0);
                let pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
                let account_id = AccountId(pk);

                Asset::CreditAlphanum4(AlphaNum4 {
                    asset_code: AssetCode4(buf),
                    issuer: account_id,
                })
            }
            AssetSpec::Credit12 { code, issuer } => {
                let b = code.as_bytes();
                if !(5..=12).contains(&b.len()) {
                    return Err(SignerError::ConversionError("asset code 5-12 chars".into()));
                }
                let mut buf = [0u8; 12];
                buf[..b.len()].copy_from_slice(b);

                let issuer_pk = PublicKey::from_str(&issuer)
                    .map_err(|e| SignerError::ConversionError(format!("Invalid issuer: {}", e)))?;

                let uint256 = Uint256(issuer_pk.0);
                let pk = XdrPublicKey::PublicKeyTypeEd25519(uint256);
                let account_id = AccountId(pk);

                Asset::CreditAlphanum12(AlphaNum12 {
                    asset_code: AssetCode12(buf),
                    issuer: account_id,
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_PK: &str = "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF";

    #[test]
    fn test_native_asset() {
        let spec = AssetSpec::Native;
        let asset = Asset::try_from(spec).unwrap();
        assert!(matches!(asset, Asset::Native));
    }

    #[test]
    fn test_credit4_asset() {
        let spec = AssetSpec::Credit4 {
            code: "USDC".to_string(),
            issuer: TEST_PK.to_string(),
        };
        let asset = Asset::try_from(spec).unwrap();
        assert!(matches!(asset, Asset::CreditAlphanum4(_)));
    }

    #[test]
    fn test_invalid_asset_code() {
        let spec = AssetSpec::Credit4 {
            code: "TOOLONG".to_string(),
            issuer: TEST_PK.to_string(),
        };
        assert!(Asset::try_from(spec).is_err());
    }

    #[test]
    fn test_asset_spec_serde() {
        let spec = AssetSpec::Credit4 {
            code: "USDC".to_string(),
            issuer: TEST_PK.to_string(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("credit4"));
        assert!(json.contains("USDC"));

        let deserialized: AssetSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_asset_spec_json_format() {
        // Test Native
        let native = AssetSpec::Native;
        let native_json = serde_json::to_value(&native).unwrap();
        assert_eq!(native_json, serde_json::json!({"type": "native"}));

        // Test Credit4
        let credit4 = AssetSpec::Credit4 {
            code: "USDC".to_string(),
            issuer: TEST_PK.to_string(),
        };
        let credit4_json = serde_json::to_value(&credit4).unwrap();
        assert_eq!(
            credit4_json,
            serde_json::json!({
                "type": "credit4",
                "code": "USDC",
                "issuer": TEST_PK
            })
        );

        // Test Credit12
        let credit12 = AssetSpec::Credit12 {
            code: "LONGASSET".to_string(),
            issuer: TEST_PK.to_string(),
        };
        let credit12_json = serde_json::to_value(&credit12).unwrap();
        assert_eq!(
            credit12_json,
            serde_json::json!({
                "type": "credit12",
                "code": "LONGASSET",
                "issuer": TEST_PK
            })
        );
    }
}
