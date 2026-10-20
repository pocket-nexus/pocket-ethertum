//! Takes `hash` and the `iter` module out of the reference's `util/mod.rs`, which
//! the reference's world generation and meshing call. The rest of that file
//! needs Bevy, egui and an HTTP client, so the two items are cut out by their
//! braces rather than compiled with it.

use std::path::Path;

fn item(source: &str, head: &str) -> String {
    let start = source.find(head).unwrap_or_else(|| panic!("ref/ethertum/src/util/mod.rs has no `{head}`"));
    let mut depth = 0i32;
    for (i, c) in source[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..start + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after `{head}`");
}

fn main() {
    let util = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ref/ethertum/src/util/mod.rs");
    println!("cargo:rerun-if-changed={}", util.display());
    let source = std::fs::read_to_string(&util).expect("ref/ethertum is checked out (git submodule update --init)");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("util_extract.rs");
    std::fs::write(out, format!("{}\n\n{}\n", item(&source, "pub fn hash(i: i32) -> f32"), item(&source, "pub mod iter {"))).unwrap();
}
