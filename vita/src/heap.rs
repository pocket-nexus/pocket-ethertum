//! Memory for chunk meshes: one GPU-mapped block of main memory, carved by a
//! first-fit free list.
//!
//! Chunks are meshed again on every edit and dropped when they leave the
//! radius, so their vertex and index buffers come and go every few frames. A
//! freed range may still be read by a frame the GPU has not finished: it is
//! retired with the number of the frame that last drew it and returns to the
//! free list once that frame's fence has passed ([`Heap::collect`]).

use std::collections::VecDeque;

use pocket_vita_gxm::mem::{Block, Kind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub offset: u32,
    pub len: u32,
}

pub struct Heap {
    block: Block,
    /// Free ranges, sorted by offset and never adjacent.
    free: Vec<Range>,
    retired: VecDeque<(u64, Range)>,
    pub used: usize,
    pub peak: usize,
}

const ALIGN: u32 = 16;

impl Heap {
    pub unsafe fn new(bytes: usize) -> Result<Heap, String> {
        let block = Block::with_access(Kind::Main, bytes, false)?;
        let size = block.size() as u32;
        Ok(Heap { block, free: vec![Range { offset: 0, len: size }], retired: VecDeque::new(), used: 0, peak: 0 })
    }

    pub fn size(&self) -> usize {
        self.block.size()
    }

    pub fn base(&self) -> *mut u8 {
        self.block.base()
    }

    /// A range of at least `len` bytes, 16-aligned.
    pub fn alloc(&mut self, len: usize) -> Option<Range> {
        let len = (len as u32).div_ceil(ALIGN) * ALIGN;
        let i = self.free.iter().position(|r| r.len >= len)?;
        let r = self.free[i];
        if r.len == len {
            self.free.remove(i);
        } else {
            self.free[i] = Range { offset: r.offset + len, len: r.len - len };
        }
        self.used += len as usize;
        self.peak = self.peak.max(self.used);
        Some(Range { offset: r.offset, len })
    }

    /// Gives a range back once frame `frame` is done on the GPU.
    pub fn retire(&mut self, r: Range, frame: u64) {
        self.retired.push_back((frame, r));
    }

    /// Returns ranges retired by frames up to and including `done`.
    pub fn collect(&mut self, done: u64) {
        while let Some(&(f, r)) = self.retired.front() {
            if f > done {
                break;
            }
            self.retired.pop_front();
            self.release(r);
        }
    }

    fn release(&mut self, r: Range) {
        self.used -= r.len as usize;
        let i = self.free.partition_point(|f| f.offset < r.offset);
        self.free.insert(i, r);
        // Join with the next and the previous range.
        if i + 1 < self.free.len() && self.free[i].offset + self.free[i].len == self.free[i + 1].offset {
            self.free[i].len += self.free[i + 1].len;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].offset + self.free[i - 1].len == self.free[i].offset {
            self.free[i - 1].len += self.free[i].len;
            self.free.remove(i);
        }
    }

    /// The largest range that could be handed out now.
    pub fn largest(&self) -> usize {
        self.free.iter().map(|r| r.len as usize).max().unwrap_or(0)
    }

    /// Writes `bytes` at `r`.
    pub unsafe fn write(&self, r: Range, at: usize, bytes: &[u8]) {
        debug_assert!(at + bytes.len() <= r.len as usize);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.base().add(r.offset as usize + at), bytes.len());
    }

    pub fn ptr(&self, r: Range, at: usize) -> *const u8 {
        unsafe { self.base().add(r.offset as usize + at) }
    }
}
