use proc_macro::TokenStream;

/// Runtime builds of teloxide only require the attribute to preserve its item.
#[proc_macro_attribute]
pub fn aquamarine(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
