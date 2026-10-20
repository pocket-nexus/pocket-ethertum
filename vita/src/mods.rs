//! Mods on the console: where their files are read from and when they are read again.
//!
//! The base mod is compiled in (`ether_mod::base`). Other mods are `<name>/main.js` under the USB share's
//! `ether/mods` in a development build, else under `ux0:data/pocket-ethertum/mods`, listed one name per line in
//! `index.txt` there: the USB share cannot list a directory. A `base` entry replaces the compiled-in base mod.
//!
//! A thread reads the listed files once a second and sends the whole set to the frame thread when any byte of
//! them changed; the frame thread loads them into a new realm (`Mods::load`) and applies what the registry's
//! diff says changed.

use std::sync::mpsc;
use std::time::Duration;

use ether_mod::Source;

use crate::{hostfs, paths};

fn dir() -> &'static str {
    if cfg!(feature = "usb-debug") && hostfs::read(&format!("{}/index.txt", paths::MODS_HOST), 64 * 1024).is_some() {
        paths::MODS_HOST
    } else {
        paths::MODS_CARD
    }
}

/// The base mod and every listed mod, in order.
pub fn read() -> Vec<Source> {
    let dir = dir();
    let mut out = Vec::new();
    let mut base = ether_mod::base();
    let index = hostfs::read(&format!("{dir}/index.txt"), 64 * 1024).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    for name in index.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some(text) = hostfs::read(&format!("{dir}/{name}/main.js"), 1 << 20) else { continue };
        let source = Source::new(name, &String::from_utf8_lossy(&text));
        if name == "base" {
            base = source;
        } else {
            out.push(source);
        }
    }
    out.insert(0, base);
    out
}

fn fingerprint(sources: &[Source]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for s in sources {
        for b in s.name.bytes().chain([0]).chain(s.text.bytes()).chain([0]) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// Sends the mods again whenever their files change.
pub fn watch(first: &[Source]) -> mpsc::Receiver<Vec<Source>> {
    let (tx, rx) = mpsc::channel();
    let mut last = fingerprint(first);
    let _ = std::thread::Builder::new().name("ether-mods".into()).stack_size(128 * 1024).spawn(move || loop {
        std::thread::sleep(Duration::from_millis(1000));
        let now = read();
        let f = fingerprint(&now);
        if f != last {
            last = f;
            if tx.send(now).is_err() {
                return;
            }
        }
    });
    rx
}
