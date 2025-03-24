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
    info(description = "OpenZeppelin Relayer API", version = "0.1.0", title = "OpenZeppelin Relayer API",  license(
        name = "AGPL-3.0 license",
        url = "https://github.com/OpenZeppelin/openzeppelin-relayer/blob/main/LICENSE"
    ),
    contact(
        name = "OpenZeppelin",
        url = "https://www.openzeppelin.com",
    ),
    terms_of_service = "https://www.openzeppelin.com/tos"),
    paths(
        relayer::get_relayer,
        relayer::list_relayers,
        relayer::get_relayer_balance,
        relayer::update_relayer,
        relayer::get_transaction_by_nonce,
        relayer::get_transaction_by_id,
        relayer::list_transactions,
        relayer::get_relayer_status,
        relayer::sign_typed_data,
        relayer::sign,
        relayer::cancel_transaction,
        relayer::delete_pending_transactions,
        relayer::rpc,
        relayer::send_transaction,
        metrics::list_metrics,
        metrics::metric_detail,
        metrics::scrape_metrics,
    ),
    components(schemas(models::RelayerResponse, models::NetworkPolicyResponse, models::EvmPolicyResponse, models::SolanaPolicyResponse, models::StellarPolicyResponse))
)]
pub struct ApiDoc;
