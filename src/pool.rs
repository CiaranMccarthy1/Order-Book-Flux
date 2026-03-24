use alloc::vec::Vec;

use crate::types::{Price, Quantity, Side};

#[derive(Debug, Copy, Clone)]
pub struct Order {
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
    pub ts_nanos: u64,
}

pub struct OrderPool {
    storage: Vec<Order>,
    free: Vec<usize>,
}

impl OrderPool {
    pub fn with_capacity(capacity: usize) -> Self {
        let mut storage = Vec::with_capacity(capacity);
        let mut free = Vec::with_capacity(capacity);

        for idx in 0..capacity {
            storage.push(Order {
                side: Side::Bid,
                price: 0,
                qty: 0,
                ts_nanos: 0,
            });
            free.push(idx);
        }

        Self { storage, free }
    }

    #[inline]
    pub fn acquire_index(&mut self) -> Option<usize> {
        self.free.pop()
    }

    #[inline]
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut Order> {
        self.storage.get_mut(idx)
    }

    #[inline]
    pub fn release_index(&mut self, idx: usize) {
        if idx < self.storage.len() {
            self.free.push(idx);
        }
    }

    #[inline]
    pub fn free_len(&self) -> usize {
        self.free.len()
    }
}
