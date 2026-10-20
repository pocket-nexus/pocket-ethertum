//! The reference derives serde and Bevy reflection on its voxel shape; the
//! oracle needs neither, so these derives emit nothing.

use proc_macro::TokenStream;

#[proc_macro_derive(Serialize, attributes(serde))]
pub fn serialize(_: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_derive(Deserialize, attributes(serde))]
pub fn deserialize(_: TokenStream) -> TokenStream {
    TokenStream::new()
}

#[proc_macro_derive(Reflect, attributes(reflect))]
pub fn reflect(_: TokenStream) -> TokenStream {
    TokenStream::new()
}
