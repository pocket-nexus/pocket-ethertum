//! Items (`item/mod.rs`) and what the player carries (`ClientPlayerInfo`).
//!
//! A stack is a count and an item id (0 is no item). The inventory has 36
//! slots; the first 9 are the hotbar, one of which is selected. Health is 20
//! of 20.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemStack {
    pub count: u8,
    pub item_id: u8,
}

impl ItemStack {
    pub fn new(count: u8, item_id: u8) -> ItemStack {
        ItemStack { count, item_id }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0 || self.item_id == 0
    }

    pub fn clear(&mut self) {
        *self = ItemStack::default();
    }

    pub fn swap(a: &mut ItemStack, b: &mut ItemStack) {
        core::mem::swap(a, b);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub items: Vec<ItemStack>,
}

impl Inventory {
    pub fn new(len: usize) -> Inventory {
        Inventory { items: vec![ItemStack::default(); len] }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerInfo {
    pub inventory: Inventory,
    pub hotbar_index: u32,
    pub health: u32,
    pub health_max: u32,
}

impl PlayerInfo {
    pub const HOTBAR_SLOTS: u32 = 9;

    /// Moves the hotbar selection by `delta` slots, wrapping.
    pub fn scroll_hotbar(&mut self, delta: i32) {
        let n = Self::HOTBAR_SLOTS as i32;
        self.hotbar_index = (self.hotbar_index as i32 + delta).rem_euclid(n) as u32;
    }

    pub fn hotbar(&self) -> &[ItemStack] {
        &self.inventory.items[..Self::HOTBAR_SLOTS as usize]
    }

    pub fn held(&self) -> ItemStack {
        self.inventory.items[self.hotbar_index as usize]
    }
}

impl Default for PlayerInfo {
    fn default() -> PlayerInfo {
        PlayerInfo { inventory: Inventory::new(36), hotbar_index: 0, health: 20, health_max: 20 }
    }
}
