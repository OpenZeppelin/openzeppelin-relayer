use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ExprArray, ItemFn};

/// Proc macro to automatically add permission checking to endpoint functions.
///
/// requires raw_request and data as arguments of the endpoint function to be passed in the function body
///
/// # Example
/// ```rust
/// #[require_permissions([RELAYERS_LIST])]
/// async fn list_relayers(raw_request: HttpRequest, data: web::ThinData<DefaultAppState>) -> impl Responder {
///     relayer::list_relayers(raw_request, data).await
/// }
///
/// #[require_permissions([RELAYERS_READ])]
/// async fn get_relayer(relayer_id: web::Path<String>, data: web::ThinData<DefaultAppState>) -> impl Responder {
///     relayer::get_relayer(relayer_id.into_inner(), data).await
/// }
/// ```
///
#[proc_macro_attribute]
pub fn require_permissions(args: TokenStream, input: TokenStream) -> TokenStream {
    let permissions_array = parse_macro_input!(args as ExprArray);
    let mut input_fn = parse_macro_input!(input as ItemFn);

    let permissions: Vec<_> = permissions_array.elems.iter().collect();

    // Build permission check as the first statement in the function
    let permission_check = quote! {
        crate::utils::validate_api_key_permissions(
            &raw_request,
            data.api_key_repository.as_ref(),
            &[#(#permissions),*]
        ).await?;
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
