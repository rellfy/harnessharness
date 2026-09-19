mod tool;

use proc_macro::TokenStream;
use syn::ItemFn;
use syn::parse_macro_input;

/// Turn a function into a tool: a `static` of type `harnessharness::tool::ToolDef`, named after
/// the function.
///
/// The function doc comment becomes the tool description and parameter doc comments become
/// property descriptions. Parameters are deserialized from the model input; `Option` parameters
/// are optional. A `&ToolContext` parameter receives the call context. The function may return
/// any `Serialize` value, or a `Result` of one with any `Display` error.
#[proc_macro_attribute]
pub fn tool(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    tool::expand(attribute.into(), function)
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}
