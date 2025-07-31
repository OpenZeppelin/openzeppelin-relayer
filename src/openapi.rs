use crate::{
    api::routes::{
        docs::{notification_docs, plugin_docs, relayer_docs, signer_docs},
        health, metrics,
    },
    domain, models,
    services::plugins,
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
/// # OpenAPI Specification Generator
///
/// This utility generates an OpenAPI specification JSON file from the
/// OpenZeppelin Relayer API definitions.
#[derive(OpenApi)]
#[openapi(
    modifiers(&SecurityAddon),
    tags(
      (name = "Relayers", description = "Relayers are the core components of the OpenZeppelin Relayer API. They are responsible for executing transactions on behalf of users and providing a secure and reliable way to interact with the blockchain."),
      (name = "Plugins", description = "Plugins are TypeScript functions that can be used to extend the OpenZeppelin Relayer API functionality."),
      (name = "Notifications", description = "Notifications are responsible for showing the notifications related to the relayers."),
      (name = "Signers", description = "Signers are responsible for signing the transactions related to the relayers."),
      (name = "Metrics", description = "Metrics are responsible for showing the metrics related to the relayers."),
      (name = "Health", description = "Health is responsible for showing the health of the relayers.")
    ),
    info(description = "OpenZeppelin Relayer API", version = "1.0.0", title = "OpenZeppelin Relayer API",  license(
        name = "AGPL-3.0 license",
        url = "https://github.com/OpenZeppelin/openzeppelin-relayer/blob/main/LICENSE"
    ),
    contact(
        name = "OpenZeppelin",
        url = "https://www.openzeppelin.com",
    ),
    terms_of_service = "https://www.openzeppelin.com/tos"),
    paths(
        relayer_docs::doc_get_relayer,
        relayer_docs::doc_list_relayers,
        relayer_docs::doc_create_relayer,
        relayer_docs::doc_update_relayer,
        relayer_docs::doc_delete_relayer,
        relayer_docs::doc_get_relayer_balance,
        relayer_docs::doc_get_transaction_by_nonce,
        relayer_docs::doc_get_transaction_by_id,
        relayer_docs::doc_list_transactions,
        relayer_docs::doc_get_relayer_status,
        relayer_docs::doc_sign_typed_data,
        relayer_docs::doc_sign,
        relayer_docs::doc_cancel_transaction,
        relayer_docs::doc_delete_pending_transactions,
        relayer_docs::doc_rpc,
        relayer_docs::doc_send_transaction,
        relayer_docs::doc_replace_transaction,
        health::health,
        metrics::list_metrics,
        metrics::metric_detail,
        metrics::scrape_metrics,
        plugin_docs::doc_call_plugin,
        notification_docs::doc_list_notifications,
        notification_docs::doc_get_notification,
        notification_docs::doc_create_notification,
        notification_docs::doc_update_notification,
        notification_docs::doc_delete_notification,
        signer_docs::doc_list_signers,
        signer_docs::doc_get_signer,
        signer_docs::doc_create_signer,
        signer_docs::doc_update_signer,
        signer_docs::doc_delete_signer,
    ),
    components(schemas(
        models::RelayerResponse,
        models::CreateRelayerRequest,
        models::NetworkPolicyResponse,
        models::EvmPolicyResponse,
        models::SolanaPolicyResponse,
        models::StellarPolicyResponse,
        models::UpdateRelayerRequest,
        domain::SignDataRequest,
        domain::SignTypedDataRequest,
        models::PluginCallRequest,
        plugins::PluginCallResponse
    ))
)]
pub struct ApiDoc;
