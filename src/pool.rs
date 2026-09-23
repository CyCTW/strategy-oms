//! Segmented arena. Objects never move when the directory grows. A block is
//! an allocation unit, not a capacity limit. No unsafe code or raw references
//! survive mutations; generation-checked handles protect reused slots.
const BLOCK: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Handle {
    slot: usize,
    generation: u64,
}

struct Slot<T> {
    generation: u64,
    value: Option<T>,
    next_free: Option<usize>,
}

pub(crate) struct Pool<T> {
    blocks: Vec<Box<[Slot<T>]>>,
    free: Option<usize>,
    len: usize,
}

impl<T> Default for Pool<T> {
    fn default() -> Self {
        Self {
            blocks: Vec::new(),
            free: None,
            len: 0,
        }
    }
}

impl<T> Pool<T> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn capacity(&self) -> usize {
        self.blocks.len() * BLOCK
    }
    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    pub fn insert(&mut self, value: T) -> Handle {
        if self.free.is_none() {
            let base = self.capacity();
            let slots: Box<[_]> = (0..BLOCK)
                .map(|i| Slot {
                    generation: 1,
                    value: None,
                    next_free: if i + 1 < BLOCK {
                        Some(base + i + 1)
                    } else {
                        None
                    },
                })
                .collect();
            self.blocks.push(slots);
            self.free = Some(base);
        }
        let index = self.free.expect("free block");
        let slot = &mut self.blocks[index / BLOCK][index % BLOCK];
        self.free = slot.next_free.take();
        slot.value = Some(value);
        self.len += 1;
        Handle {
            slot: index,
            generation: slot.generation,
        }
    }

    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self
            .blocks
            .get(handle.slot / BLOCK)?
            .get(handle.slot % BLOCK)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_ref())
            .flatten()
    }

    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self
            .blocks
            .get_mut(handle.slot / BLOCK)?
            .get_mut(handle.slot % BLOCK)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_mut())
            .flatten()
    }

    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self
            .blocks
            .get_mut(handle.slot / BLOCK)?
            .get_mut(handle.slot % BLOCK)?;
        if slot.generation != handle.generation {
            return None;
        }
        let value = slot.value.take()?;
        self.len -= 1;
        // Never wrap generations. An exhausted slot is permanently retired.
        if let Some(next) = slot.generation.checked_add(1) {
            slot.generation = next;
            slot.next_free = self.free;
            self.free = Some(handle.slot);
        }
        Some(value)
    }

    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.blocks
            .iter()
            .flat_map(|b| b.iter())
            .filter_map(|s| s.value.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses_stay_stable_and_reused_handles_are_invalidated() {
        let mut pool = Pool::default();
        let first = pool.insert(7);
        let address = pool.get(first).unwrap() as *const i32;
        for i in 0..4096 {
            pool.insert(i);
        }
        assert_eq!(pool.get(first).unwrap() as *const i32, address);
        assert_eq!(pool.remove(first), Some(7));
        assert!(pool.get(first).is_none());
        let next = pool.insert(8);
        assert_ne!(next, first);
        assert_eq!(next.slot, first.slot);
        assert!(pool.get_mut(first).is_none());
        assert!(pool.remove(first).is_none());
    }
}
