use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Expr, ExprArray, ItemFn};

#[cfg(test)]
mod tests;

/// Proc macro to automatically add API key permission checking to endpoint functions.
///
/// This macro validates that the incoming request has the required permissions before
/// executing the endpoint function. It supports both global-scoped and ID-scoped permissions.
///
/// # Requirements
/// - The function must have `raw_request: HttpRequest` parameter
/// - The function must have `data: web::ThinData<T>` parameter where T has an `api_key_repository` field
/// - The function must return a type that works with the `?` operator (Result or similar)
///
/// # Permission Syntax
///
/// ## Global-Scoped Permissions (no specific resource ID)
/// Use simple string for endpoints that access resources globally:
/// ```rust, ignore
/// #[require_permissions(["relayers:read"])]
/// async fn list_relayers(
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // Checks if API key has global "relayers:read" permission
/// }
/// ```
///
/// ## ID-Scoped Permissions (specific resource)
/// Use tuple syntax `(action, param_name)` for resource-specific access:
/// ```rust, ignore
/// #[require_permissions([("relayers:read", "relayer_id")])]
/// async fn get_relayer(
///     relayer_id: web::Path<String>,
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // Checks if API key has "relayers:read" permission for the specific relayer_id
/// }
/// ```
///
/// ## Multiple Permissions
/// Combine multiple permissions (ALL must be satisfied - AND logic):
/// ```rust, ignore
/// #[require_permissions([
///     ("relayers:read", "relayer_id"),
///     ("transactions:execute", "relayer_id")
/// ])]
/// async fn send_transaction(
///     relayer_id: web::Path<String>,
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // Checks both permissions for the same relayer_id
/// }
/// ```
///
/// # How It Works
///
/// 1. **Global Permissions (string)**: Calls `validate_api_key_permissions()`
/// 2. **Scoped Permissions (tuple)**:
///    - Extracts the parameter value from function signature
///    - Calls `validate_api_key_permissions_scoped()` with the resource ID
///
/// # Error Handling
///
/// The macro inserts permission validation at the beginning of the function.
/// If validation fails, the function returns early with an `ApiError`.
///
#[proc_macro_attribute]
pub fn require_permissions(args: TokenStream, input: TokenStream) -> TokenStream {
    let permissions_array = parse_macro_input!(args as ExprArray);
    let mut input_fn = parse_macro_input!(input as ItemFn);

    let permissions: Vec<_> = permissions_array.elems.iter().collect();

    // Classify permissions as either global (string) or scoped (tuple)
    let mut global_permissions = Vec::new();
    let mut scoped_permissions = Vec::new();

    for permission in &permissions {
        match permission {
            // Simple string - global permission
            Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(lit_str),
                ..
            }) => {
                global_permissions.push(lit_str.value());
            }
            // Tuple (action, param) - scoped permission
            Expr::Tuple(tuple) => {
                if tuple.elems.len() == 2 {
                    if let (
                        Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(action_str),
                            ..
                        }),
                        Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(param_str),
                            ..
                        }),
                    ) = (&tuple.elems[0], &tuple.elems[1])
                    {
                        scoped_permissions.push((action_str.value(), param_str.value()));
                    } else {
                        panic!("Tuple permissions must be (\"action\", \"param_name\")");
                    }
                } else {
                    panic!("Tuple permissions must have exactly 2 elements: (action, param_name)");
                }
            }
            _ => {
                panic!("Permissions must be either strings \"action\" or tuples (\"action\", \"param\")");
            }
        }
    }

    // Build permission check code
    let mut permission_checks = Vec::new();

    // Add global permission check if there are any
    if !global_permissions.is_empty() {
        let global_perms_strs: Vec<_> = global_permissions.iter().map(|s| s.as_str()).collect();
        permission_checks.push(quote! {
            crate::utils::validate_api_key_permissions(
                &raw_request,
                data.api_key_repository.as_ref(),
                &[#(#global_perms_strs),*]
            ).await?;
        });
    }

    // Add scoped permission checks
    for (action, param_name) in &scoped_permissions {
        // Parse param_name to support dotted paths like "relayer_id.relayer_id"
        let path_parts: Vec<&str> = param_name.split('.').collect();
        let base_param_name = path_parts[0];
        let base_param_ident = syn::Ident::new(base_param_name, proc_macro2::Span::call_site());

        // Verify the base parameter exists in function signature
        let param_exists = input_fn.sig.inputs.iter().any(|arg| {
            if let syn::FnArg::Typed(pat_type) = arg {
                if let syn::Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                    return pat_ident.ident == base_param_ident;
                }
            }
            false
        });

        if !param_exists {
            panic!(
                "Parameter '{}' (from '{}') specified in permission tuple not found in function signature",
                base_param_name,
                param_name
            );
        }

        // Generate extraction code based on whether we have a dotted path
        let extraction_code = if path_parts.len() == 1 {
            // Simple case: just the parameter name
            quote! {
                let __resource_id: String = #base_param_ident.as_ref().to_string();
            }
        } else {
            // Dotted path case: access struct fields
            // Build the access chain for named fields
            let mut access_chain = quote! { #base_param_ident.as_ref() };

            for part in &path_parts[1..] {
                let field_ident = syn::Ident::new(part, proc_macro2::Span::call_site());
                access_chain = quote! { #access_chain.#field_ident };
            }

            quote! {
                let __resource_id: String = #access_chain.clone();
            }
        };

        permission_checks.push(quote! {
            {
                #extraction_code
                tracing::debug!(
                    "Checking scoped permission: action='{}', param='{}', id='{}'",
                    #action,
                    #param_name,
                    __resource_id
                );
                crate::utils::validate_api_key_permissions_scoped(
                    &raw_request,
                    data.api_key_repository.as_ref(),
                    &[#action],
                    &__resource_id
                ).await?;
            }
        });
    }

    // Combine all permission checks
    let combined_checks = quote! {
        #(#permission_checks)*
    };

    // Parse as a statement and prepend to function body
    let permission_check_stmt: syn::Stmt = syn::parse2(combined_checks).unwrap();
    input_fn.block.stmts.insert(0, permission_check_stmt);

    // Return the modified function
    quote! {
        #input_fn
    }
    .into()
}
