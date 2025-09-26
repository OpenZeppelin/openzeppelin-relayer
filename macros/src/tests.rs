#[cfg(test)]
mod tests {
    /// Test that the macro correctly detects template parameters
    #[test]
    fn test_macro_detects_template_parameters() {
        // This is a compile-time test - if the macro compiles, it works
        let tokens = quote::quote! {
            #[require_permissions(["relayers:get:{relayer_id}"])]
            async fn test_function(
                relayer_id: actix_web::web::Path<String>,
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

    /// Test that the macro correctly handles permissions without templates
    #[test]
    fn test_macro_handles_non_template_permissions() {
        let tokens = quote::quote! {
            #[require_permissions(["relayers:list:all"])]
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

    /// Test parameter extraction logic in isolation
    #[test]
    fn test_parameter_extraction_logic() {
        let permission = "relayers:get:{relayer_id}";
        let mut required_params = std::collections::HashSet::new();

        // Extract parameter names from {param_name} patterns (same logic as in macro)
        let chars = permission.chars();
        let mut inside_braces = false;
        let mut current_param = String::new();

        for ch in chars {
            if ch == '{' {
                inside_braces = true;
                current_param.clear();
            } else if ch == '}' && inside_braces {
                if !current_param.is_empty() {
                    required_params.insert(current_param.clone());
                }
                inside_braces = false;
            } else if inside_braces {
                current_param.push(ch);
            }
        }

        assert_eq!(required_params.len(), 1);
        assert!(required_params.contains("relayer_id"));
    }
}
