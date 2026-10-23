//! The tab's side: what the page calls (`page/main.js`).
//!
//! The page plays the Pocket3D title card and opens the shell on a canvas ([`Ethertum::open`], with the base
//! mod's registry); the pack is read beside the frames ([`Reader::read`]) and handed in when it is there
//! ([`Ethertum::enter`]), and the world loads around the spawn while the interface counts its chunks. A frame is
//! `step`, the turn of the interface's guest, which the page runs in a realm of its own and whose lines pass
//! through `heard` and `say`, then `draw`. When what the guest shows has changed, the page hands its picture to
//! `overlay`. Everything a frame does is in `app`.

use pocket_web_wgpu::gpu::{Gpu, Screen};
use pocket_web_wgpu::source::Source;
use wasm_bindgen::prelude::*;

use crate::app::{App, Held, HEIGHT, SAMPLES, TURNS, WIDTH};
use crate::pack::Pictures;

/// The screen the page gives the canvas, and the guest's turns a second, as JSON.
#[wasm_bindgen]
pub fn shape() -> String {
    format!("{{\"name\":\"vita\",\"width\":{WIDTH},\"height\":{HEIGHT},\"turns\":{TURNS}}}")
}

#[wasm_bindgen]
pub struct Ethertum {
    app: App,
    handed: u32,
}

/// What reads the pack beside the frames, for the GPU the shell draws with.
#[wasm_bindgen]
pub struct Reader {
    gpu: Gpu,
    blocks: bool,
}

/// The pack's pictures, on the GPU.
#[wasm_bindgen]
pub struct Pack {
    pictures: Pictures,
}

#[wasm_bindgen]
impl Reader {
    /// Reads the pack at `url`: the pack's file, or a manifest of its pieces and the pack's name after `#`.
    pub async fn read(self, url: String) -> Result<Pack, JsError> {
        let source = Source::open(&url).await.map_err(|e| JsError::new(&e))?;
        let bytes = source.all().await.map_err(|e| JsError::new(&e))?;
        let pictures = Pictures::load(&self.gpu, &bytes, self.blocks).map_err(|e| JsError::new(&e))?;
        Ok(Pack { pictures })
    }
}

#[wasm_bindgen]
impl Ethertum {
    /// The shell on `canvas` (960 × 544 pixels), with no world yet: its frames show the interface alone.
    /// `registry`: the base mod's, as JSON; `prefs`: the settings the page kept from the last visit.
    pub async fn open(canvas: web_sys::HtmlCanvasElement, registry: String, prefs: String) -> Result<Ethertum, JsError> {
        std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));
        // (the pack holds its pictures in blocks: a GPU that reads them takes them as they are)
        let (gpu, surface) = Gpu::for_canvas_wanting(canvas, pocket_web_wgpu::wgpu::Features::TEXTURE_COMPRESSION_BC).await.map_err(|e| JsError::new(&e))?;
        let screen = Screen::canvas(&gpu, surface, WIDTH, HEIGHT, SAMPLES);
        let mut app = App::open(gpu, screen, &registry).map_err(|e| JsError::new(&e))?;
        if !prefs.is_empty() {
            app.prefs_stored(&prefs);
        }
        Ok(Ethertum { app, handed: 0 })
    }

    /// What reads the pack for this shell.
    pub fn reader(&self) -> Reader {
        Reader { gpu: self.app.gpu.clone(), blocks: self.app.reads_blocks() }
    }

    /// The pack has been read: the world loads around the spawn.
    pub fn enter(&mut self, pack: Pack) {
        self.app.enter(pack.pictures);
    }

    /// Whether the world stands (the title and play draw it).
    pub fn entered(&self) -> bool {
        self.app.entered()
    }

    /// The start failed: the interface says why.
    pub fn fail(&mut self, why: &str) {
        self.app.fail(why);
    }

    /// Words a development host or the address sends (`App::control`).
    pub fn control(&mut self, words: &str) {
        self.app.control(words);
    }

    /// The first half of a frame at `now` (the frame loop's clock, milliseconds). `buttons`: PocketJS's bits of
    /// the buttons held on the page's handheld; the sticks in −1…1, right and up positive. Returns the sixtieths
    /// of a second that passed.
    pub fn step(&mut self, now: f64, buttons: u32, lx: f32, ly: f32, rx: f32, ry: f32) -> u32 {
        self.app.step(now, &Held { buttons, left: [lx, ly], right: [rx, ry] })
    }

    /// The second half: the world, with the interface's picture over it.
    pub fn draw(&mut self) -> Result<(), JsError> {
        self.app.draw().map_err(|e| {
            self.app.trouble = e.clone();
            JsError::new(&e)
        })
    }

    /// A guest has opened the interface's channel.
    pub fn interface_opened(&mut self) {
        self.app.interface_opened();
    }

    /// The guest went with its realm.
    pub fn interface_closed(&mut self) {
        self.app.interface_closed();
    }

    /// The guest's turn in this frame, once a frame after `step`: the sixtieths of a second it is for, or 0 for a
    /// frame without one. `touching`: a contact is on a screen the guest draws, or has just left one.
    pub fn guest_due(&mut self, touching: bool) -> u32 {
        let (ticks, buttons) = self.app.guest_due(touching).unwrap_or((0, 0));
        self.handed = buttons;
        ticks
    }

    /// The buttons for the turn `guest_due` has just allowed.
    pub fn guest_buttons(&self) -> u32 {
        self.handed
    }

    /// The line of state the guest has not seen, for its turn.
    pub fn heard(&mut self) -> Option<String> {
        self.app.heard()
    }

    /// A line the guest sent.
    pub fn say(&mut self, line: &str) {
        self.app.say(line);
    }

    /// The guest's picture: premultiplied RGBA rows of `width` × `height`.
    pub fn overlay(&mut self, pixels: &[u8], width: u32, height: u32) -> Result<(), JsError> {
        self.app.overlay(pixels, width, height).map_err(|e| JsError::new(&e))
    }

    pub fn overlay_hide(&mut self) {
        self.app.overlay_hide();
    }

    /// What the interface asked to have kept since the last call (the page keeps it in `localStorage`).
    pub fn prefs(&mut self) -> Option<String> {
        self.app.prefs_take()
    }

    /// The run as JSON.
    pub fn status(&self) -> String {
        self.app.status()
    }
}
