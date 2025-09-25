use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ExprArray, ItemFn};

#[cfg(test)]
mod tests;

/// Proc macro to automatically add API key permission checking to endpoint functions.
///
/// This macro validates that the incoming request has the required permissions before
/// executing the endpoint function. It supports both static permissions and template
/// permissions with parameter substitution.
///
/// # Requirements
/// - The function must have `raw_request: HttpRequest` parameter
/// - The function must have `data: web::ThinData<T>` parameter where T has an `api_key_repository` field
/// - The function must return a type that works with the `?` operator (Result or similar)
///
/// # Permission Types
///
/// ## Static Permissions
/// Use static permission strings for endpoints that don't require resource-specific access:
/// ```rust, ignore
/// #[require_permissions(["relayers:get:all"])]
/// async fn list_relayers(
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // Implementation
/// }
/// ```
///
/// ## Template Permissions with Parameter Substitution
/// Use template permissions with `{parameter_name}` placeholders for resource-specific access:
/// ```rust, ignore
/// #[require_permissions(["relayers:get:{relayer_id}"])]
/// async fn get_relayer(
///     relayer_id: web::Path<String>,  // This parameter value is extracted
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // The macro extracts the "sepolia-example" value from relayer_id parameter
///     // and checks if API key has permission "relayers:get:sepolia-example"
/// }
/// ```
///
/// ## Multiple Template Parameters
/// The macro can handle multiple parameters in a single permission:
/// ```rust, ignore
/// #[require_permissions(["transactions:get:{relayer_id}"])]
/// async fn get_transaction(
///     path: web::Path<TransactionPath>,  // Contains relayer_id and transaction_id
///     raw_request: HttpRequest,
///     data: web::ThinData<DefaultAppState>
/// ) -> impl Responder {
///     // Only relayer_id is used for permission checking
/// }
/// ```
///
/// # How It Works
///
/// 1. **Static Permissions**: Calls `validate_api_key_permissions()` directly
/// 2. **Template Permissions**:
///    - Detects `{parameter_name}` patterns in permission strings
///    - Extracts matching parameters from function signature using `.as_ref().to_string()`
///    - Substitutes parameters into permission templates
///    - Calls `validate_api_key_permissions_with_params()` with the parameter map
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

    // Check if any permission contains {} brackets and extract parameter names
    let mut has_template_params = false;
    let mut required_params = std::collections::HashSet::new();

    for permission in &permissions {
        if let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(lit_str),
            ..
        }) = permission
        {
            let permission_str = lit_str.value();
            if permission_str.contains('{') && permission_str.contains('}') {
                has_template_params = true;

                // Extract parameter names from {param_name} patterns
                let chars = permission_str.chars();
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
            }
        }
    }

    // Build parameter extraction snippets for template parameters
    let mut param_extractions = Vec::new();
    for param_name in &required_params {
        let param_ident = syn::Ident::new(param_name, proc_macro2::Span::call_site());
        let param_exists = input_fn.sig.inputs.iter().any(|arg| {
            if let syn::FnArg::Typed(pat_type) = arg {
                if let syn::Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                    return pat_ident.ident == param_ident;
                }
            }
            false
        });

        if param_exists {
            param_extractions.push(quote! {
                let __param_value: String = #param_ident.as_ref().to_string();
                tracing::debug!("Extracted parameter: {} = {}", #param_name, __param_value);
                __param_values.insert(#param_name.to_string(), __param_value);
            });
        }
    }

    let permission_check = if has_template_params {
        // Use the enhanced validation with parameter substitution
        quote! {{
            let mut __param_values = std::collections::HashMap::new();
            #(#param_extractions)*

            crate::utils::validate_api_key_permissions_with_params(
                &raw_request,
                data.api_key_repository.as_ref(),
                &[#(#permissions),*],
                &__param_values
            ).await?;
        }}
    } else {
        // Use original validation for permissions without templates
        quote! {{
            crate::utils::validate_api_key_permissions(
                &raw_request,
                data.api_key_repository.as_ref(),
                &[#(#permissions),*]
            ).await?;
        }}
    };

    // Parse the permission check as statements and prepend to function body
    let permission_check_stmt: syn::Stmt = syn::parse2(permission_check).unwrap();
    input_fn.block.stmts.insert(0, permission_check_stmt);

    // Return the modified function
    quote! {
        #input_fn
    }
    .into()
}
