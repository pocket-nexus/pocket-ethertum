//! Prints the base mod's registry as JSON: what a host that runs no script (the browser's build) reads in
//! place of running `mods/base/main.js`.
//!
//!   cargo run -q -p ether-mod --example registry > base.json

use ether_mod::{base, Config, Mods};

fn main() {
    let mut mods = Mods::new(Config::default());
    if let Err(errors) = mods.load(&[base()]) {
        for e in errors {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }
    println!("{}", serde_json::to_string(mods.registry()).expect("registry as JSON"));
}
