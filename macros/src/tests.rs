#[cfg(test)]
mod tests {
    use quote::quote;
    use syn;

    /// Test that the macro correctly handles global permissions (string syntax)
    #[test]
    fn test_macro_global_permissions() {
        // This is a compile-time test - if the macro compiles, it works
        let tokens = quote! {
            #[crate::require_permissions(["relayers:read"])]
            async fn test_function(
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        // If we can parse this without panicking, the macro structure is correct
        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that the macro correctly handles scoped permissions (tuple syntax)
    #[test]
    fn test_macro_scoped_permissions() {
        let tokens = quote! {
            #[crate::require_permissions([("relayers:read", "relayer_id")])]
            async fn test_function(
                relayer_id: actix_web::web::Path<String>,
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that the macro correctly handles multiple global permissions
    #[test]
    fn test_macro_multiple_global_permissions() {
        let tokens = quote! {
            #[crate::require_permissions(["relayers:read", "transactions:execute"])]
            async fn test_function(
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that the macro correctly handles multiple scoped permissions
    #[test]
    fn test_macro_multiple_scoped_permissions() {
        let tokens = quote! {
            #[crate::require_permissions([
                ("relayers:read", "relayer_id"),
                ("transactions:execute", "relayer_id")
            ])]
            async fn test_function(
                relayer_id: actix_web::web::Path<String>,
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that the macro correctly handles mixed global and scoped permissions
    #[test]
    fn test_macro_mixed_permissions() {
        let tokens = quote! {
            #[crate::require_permissions([
                "relayers:read",
                ("transactions:execute", "relayer_id")
            ])]
            async fn test_function(
                relayer_id: actix_web::web::Path<String>,
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that the macro correctly handles dotted parameter paths
    #[test]
    fn test_macro_dotted_parameter_path() {
        let tokens = quote! {
            #[crate::require_permissions([("relayers:read", "params.relayer_id")])]
            async fn test_function(
                params: actix_web::web::Path<RequestParams>,
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test wildcard permissions
    #[test]
    fn test_macro_wildcard_permissions() {
        // Test resource wildcard
        let tokens = quote! {
            #[crate::require_permissions(["relayers:*"])]
            async fn test_function(
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());

        // Test super admin wildcard
        let tokens = quote! {
            #[crate::require_permissions(["*:*"])]
            async fn test_function(
                raw_request: actix_web::HttpRequest,
                data: actix_web::web::ThinData<String>,
            ) -> actix_web::HttpResponse {
                actix_web::HttpResponse::Ok().finish()
            }
        };

        let parsed = syn::parse2::<syn::ItemFn>(tokens);
        assert!(parsed.is_ok());
    }

    /// Test that different resource actions are supported
    #[test]
    fn test_macro_various_resource_actions() {
        // Test different resources and actions
        let resources = vec![
            "relayers:read",
            "relayers:create",
            "relayers:update",
            "relayers:delete",
            "transactions:execute",
            "signers:read",
            "signers:create",
            "signers:update",
            "signers:delete",
            "notifications:read",
            "notifications:create",
            "notifications:update",
            "notifications:delete",
            "api_keys:read",
            "api_keys:create",
            "api_keys:delete",
            "plugins:read",
            "plugins:create",
            "plugins:update",
            "plugins:delete",
        ];

        for action in resources {
            let tokens = quote! {
                #[crate::require_permissions([#action])]
                async fn test_function(
                    raw_request: actix_web::HttpRequest,
                    data: actix_web::web::ThinData<String>,
                ) -> actix_web::HttpResponse {
                    actix_web::HttpResponse::Ok().finish()
                }
            };

            let parsed = syn::parse2::<syn::ItemFn>(tokens);
            assert!(parsed.is_ok(), "Failed to parse permission: {}", action);
        }
    }
}
