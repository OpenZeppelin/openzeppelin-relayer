use crate::{
    api::routes::{metrics, relayer},
    models,
};
use utoipa::{
    openapi::security::{Http, HttpAuthScheme, SecurityScheme},
    Modify, OpenApi,
};

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(Http::new(HttpAuthScheme::Bearer)),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    tags((name = "OpenZeppelin Relayer API")),
    info(description = "OpenZeppelin Relayer API", version = "0.1.0", title = "OpenZeppelin Relayer API"),
    paths(
        relayer::get_relayer,
        relayer::list_relayers,
        relayer::get_relayer_balance,
        relayer::update_relayer,
        relayer::get_relayer_transaction_by_nonce,
        relayer::get_relayer_transaction_by_id,
        relayer::list_relayer_transactions,
        relayer::get_relayer_status,
        relayer::relayer_sign_typed_data,
        relayer::relayer_sign,
        relayer::cancel_relayer_transaction,
        relayer::delete_pending_transactions,
        relayer::relayer_rpc,
        relayer::send_relayer_transaction,
        metrics::list_metrics,
        metrics::metric_detail,
        metrics::scrape_metrics,
    ),
    components(schemas(models::RelayerResponse, models::NetworkPolicyResponse, models::EvmPolicyResponse, models::SolanaPolicyResponse, models::StellarPolicyResponse))
)]
pub struct ApiDoc;
