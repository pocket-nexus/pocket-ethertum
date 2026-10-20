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
/// The pack of a packaged build; a development build reads the share's.
pub const PACK_HOST: &str = "host0:ether/ether.pack";
pub const PACK_APP: &str = "app0:ether.pack";
/// Mods: the share's in development, the card's otherwise (each `<name>/main.js`, listed in `index.txt`).
pub const MODS_HOST: &str = "host0:ether/mods";
pub const MODS_CARD: &str = "ux0:data/pocket-ethertum/mods";
/// What the interface asked to have stored, and the world's edits and player.
pub const PREFS_FILE: &str = "ux0:data/pocket-ethertum/interface.json";
pub const SAVE_DIR: &str = "ux0:data/pocket-ethertum/world";

/// Paths a shipped file of the interface (`ether.js`, `ether.pak`) is looked for at, in order: the USB share
/// (development builds), the package, the data folder.
pub fn candidates(name: &str) -> Vec<String> {
    let mut v = Vec::new();
    if cfg!(feature = "usb-debug") {
        v.push(format!("{HOST}/{name}"));
    }
    v.push(format!("app0:{name}"));
    v.push(format!("{DATA}/{name}"));
    v
}

/// Writes a file in the data folder through a temporary file, so a power-off mid-write leaves the previous one.
pub fn write_atomic(path: &str, bytes: &[u8]) {
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::rename(&tmp, path);
    }
}
