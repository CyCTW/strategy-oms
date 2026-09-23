//! Experimental sparse, fixed-price pages. Page storage is shared across Books.
//! A page covers 64 raw price units, not necessarily 64 venue ticks.
use crate::pool::{Handle, Pool};
use std::collections::{BTreeMap, BTreeSet};

struct Page {
    // Only occupied slots may be read. Unoccupied slots may hold stale handles.
    values: [Handle; 64],
    occupied: u64,
    working: u64,
}
#[derive(Default)]
pub(crate) struct PagePool {
    pages: Pool<Page>,
}
impl PagePool {
    pub fn blocks(&self) -> usize {
        self.pages.blocks()
    }
    pub fn len(&self) -> usize {
        self.pages.len()
    }
}

#[derive(Default)]
pub(crate) struct PageLocator {
    pages: BTreeMap<i64, Handle>,
    confirmed: BTreeSet<i64>,
}
impl PageLocator {
    pub fn get(&self, pool: &PagePool, price: i64) -> Option<Handle> {
        let page = pool.pages.get(*self.pages.get(&price.div_euclid(64))?)?;
        let offset = price.rem_euclid(64) as usize;
        (page.occupied & (1u64 << offset) != 0).then_some(page.values[offset])
    }
    fn confirmed_changed(&mut self, key: i64, was: bool, now: bool) {
        if was != now {
            if now {
                self.confirmed.insert(key);
            } else {
                self.confirmed.remove(&key);
            }
        }
    }
    pub fn insert(&mut self, pool: &mut PagePool, price: i64, value: Handle, working: bool) {
        let key = price.div_euclid(64);
        let offset = price.rem_euclid(64) as usize;
        let bit = 1u64 << offset;
        if let Some(&h) = self.pages.get(&key) {
            let page = pool.pages.get_mut(h).unwrap();
            let was = page.working != 0;
            page.values[offset] = value;
            page.occupied |= bit;
            if working {
                page.working |= bit;
            } else {
                page.working &= !bit;
            }
            self.confirmed_changed(key, was, page.working != 0);
        } else {
            let h = pool.pages.insert(Page {
                values: [value; 64],
                occupied: bit,
                working: if working { bit } else { 0 },
            });
            self.pages.insert(key, h);
            self.confirmed_changed(key, false, working);
        }
    }
    pub fn remove(&mut self, pool: &mut PagePool, price: i64) {
        let key = price.div_euclid(64);
        let Some(&h) = self.pages.get(&key) else {
            return;
        };
        let page = pool.pages.get_mut(h).unwrap();
        let was = page.working != 0;
        let bit = 1u64 << price.rem_euclid(64);
        page.occupied &= !bit;
        page.working &= !bit;
        self.confirmed_changed(key, was, page.working != 0);
        if page.occupied == 0 {
            self.pages.remove(&key);
            pool.pages.remove(h).unwrap();
        }
    }
    pub fn set_working(&mut self, pool: &mut PagePool, price: i64, working: bool) {
        let key = price.div_euclid(64);
        let Some(&h) = self.pages.get(&key) else {
            return;
        };
        let page = pool.pages.get_mut(h).unwrap();
        let bit = 1u64 << price.rem_euclid(64);
        if page.occupied & bit == 0 {
            return;
        }
        let was = page.working != 0;
        if working {
            page.working |= bit;
        } else {
            page.working &= !bit;
        }
        self.confirmed_changed(key, was, page.working != 0);
    }
    pub fn best(&self, pool: &PagePool, buy: bool) -> Option<i64> {
        let &key = if buy {
            self.confirmed.last()?
        } else {
            self.confirmed.first()?
        };
        let page = pool.pages.get(self.pages[&key]).unwrap();
        let offset = if buy {
            63 - page.working.leading_zeros()
        } else {
            page.working.trailing_zeros()
        };
        Some(key * 64 + offset as i64)
    }
    pub fn range<'a>(&'a self, pool: &'a PagePool, lo: i64, hi: i64) -> PageRange<'a> {
        // Caller handles lo > hi, matching the other locator implementations.
        PageRange {
            pages: self.pages.range(lo.div_euclid(64)..=hi.div_euclid(64)),
            pool,
            current: None,
            key: 0,
            bits: 0,
            lo,
            hi,
        }
    }
}
pub(crate) struct PageRange<'a> {
    pages: std::collections::btree_map::Range<'a, i64, Handle>,
    pool: &'a PagePool,
    current: Option<&'a Page>,
    key: i64,
    bits: u64,
    lo: i64,
    hi: i64,
}
impl Iterator for PageRange<'_> {
    type Item = (i64, Handle);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.bits != 0 {
                let offset = self.bits.trailing_zeros() as usize;
                self.bits &= self.bits - 1;
                return Some((
                    self.key * 64 + offset as i64,
                    self.current.unwrap().values[offset],
                ));
            }
            let (&key, &h) = self.pages.next()?;
            let page = self.pool.pages.get(h).unwrap();
            self.key = key;
            self.current = Some(page);
            self.bits = page.occupied;
            if key == self.lo.div_euclid(64) {
                self.bits &= u64::MAX << self.lo.rem_euclid(64);
            }
            if key == self.hi.div_euclid(64) {
                self.bits &= u64::MAX >> (63 - self.hi.rem_euclid(64));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_pages_boundaries_working_ranges_and_recycling() {
        let mut values = Pool::default();
        let a = values.insert(1);
        let b = values.insert(2);
        let mut pool = PagePool::default();
        let mut x = PageLocator::default();
        let mut y = PageLocator::default();
        let mut reference = BTreeMap::new();
        let mut seed = 17u64;
        y.insert(&mut pool, 63, b, true);
        for step in 0..20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let p = match step % 41 {
                0 => i64::MIN,
                1 => i64::MAX,
                _ => (seed >> 32) as i64 % 1024 - 512,
            };
            let working = seed & 16 != 0;
            match seed % 4 {
                0 => {
                    x.remove(&mut pool, p);
                    reference.remove(&p);
                }
                1 => {
                    x.set_working(&mut pool, p, working);
                    if let Some(w) = reference.get_mut(&p) {
                        *w = working;
                    }
                }
                _ => {
                    x.insert(&mut pool, p, a, working);
                    reference.insert(p, working);
                }
            }
            assert_eq!(x.get(&pool, p), reference.get(&p).map(|_| a));
            assert_eq!(y.get(&pool, 63), Some(b));
            assert_eq!(
                x.best(&pool, true),
                reference.iter().rev().find(|(_, w)| **w).map(|(&p, _)| p)
            );
            assert_eq!(
                x.best(&pool, false),
                reference.iter().find(|(_, w)| **w).map(|(&p, _)| p)
            );
            if step % 127 == 0 {
                for (lo, hi) in [
                    (i64::MIN, i64::MAX),
                    (-65, 64),
                    (p, p),
                    (i64::MAX, i64::MAX),
                ] {
                    assert_eq!(
                        x.range(&pool, lo, hi).map(|(p, _)| p).collect::<Vec<_>>(),
                        reference
                            .range(lo..=hi)
                            .map(|(&p, _)| p)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        for &p in reference.keys() {
            x.remove(&mut pool, p);
        }
        assert_eq!(pool.len(), 1);
        y.remove(&mut pool, 63);
        assert_eq!(pool.len(), 0);
        let blocks = pool.blocks();
        for p in [i64::MIN, -1, 0, 63, 64, i64::MAX] {
            x.insert(&mut pool, p, a, false);
        }
        assert_eq!(pool.blocks(), blocks);
        assert_eq!(x.best(&pool, true), None);
    }
}
