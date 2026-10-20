//! Where files live on the console and on the development computer.

/// The USB share's folder for this app, when the wired debug host runs.
pub const HOST: &str = "host0:ether";
pub const GXP_HOST: &str = "host0:ether/gxp";
/// This app's folder on the memory card.
pub const DATA: &str = "ux0:data/pocket-ethertum";
/// Programs compiled on this console.
pub const GXP_CACHE: &str = "ux0:data/pocket-ethertum/gxp";
/// Programs shipped in the package.
pub const GXP_PACKAGED: &str = "app0:gxp";
