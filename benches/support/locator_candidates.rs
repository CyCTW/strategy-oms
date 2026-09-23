//! Isolated research prototypes. Not used by Engine; no production API changes.
use std::collections::{BTreeMap, BTreeSet, HashMap};

// Matches the current pool handle's two-word payload, without exposing internals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Value {
    pub slot: usize,
    pub generation: u64,
}

pub trait Locator: Default {
    fn get(&self, price: i64) -> Option<Value>;
    fn insert(&mut self, price: i64, value: Value, working: bool);
    fn remove(&mut self, price: i64);
    fn set_working(&mut self, price: i64, working: bool);
    fn best(&self, buy: bool) -> Option<i64>;
    fn visit(&self, lo: i64, hi: i64, f: impl FnMut(i64, Value) -> bool);
}

#[derive(Default)]
pub struct Standard {
    levels: BTreeMap<i64, Value>,
    confirmed: BTreeSet<i64>,
}
impl Locator for Standard {
    fn get(&self, p: i64) -> Option<Value> {
        self.levels.get(&p).copied()
    }
    fn insert(&mut self, p: i64, v: Value, w: bool) {
        self.levels.insert(p, v);
        self.set_working(p, w);
    }
    fn remove(&mut self, p: i64) {
        self.levels.remove(&p);
        self.confirmed.remove(&p);
    }
    fn set_working(&mut self, p: i64, w: bool) {
        if w {
            self.confirmed.insert(p);
        } else {
            self.confirmed.remove(&p);
        }
    }
    fn best(&self, buy: bool) -> Option<i64> {
        if buy {
            self.confirmed.last().copied()
        } else {
            self.confirmed.first().copied()
        }
    }
    fn visit(&self, lo: i64, hi: i64, mut f: impl FnMut(i64, Value) -> bool) {
        if lo > hi {
            return;
        }
        for (&p, &v) in self.levels.range(lo..=hi) {
            if !f(p, v) {
                break;
            }
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Entry {
    price: i64,
    value: Value,
    working: bool,
}

#[derive(Default)]
pub struct Flat {
    entries: Vec<Entry>,
}
impl Locator for Flat {
    fn get(&self, p: i64) -> Option<Value> {
        self.entries
            .binary_search_by_key(&p, |e| e.price)
            .ok()
            .map(|i| self.entries[i].value)
    }
    fn insert(&mut self, p: i64, v: Value, w: bool) {
        let e = Entry {
            price: p,
            value: v,
            working: w,
        };
        match self.entries.binary_search_by_key(&p, |e| e.price) {
            Ok(i) => self.entries[i] = e,
            Err(i) => self.entries.insert(i, e),
        }
    }
    fn remove(&mut self, p: i64) {
        if let Ok(i) = self.entries.binary_search_by_key(&p, |e| e.price) {
            self.entries.remove(i);
        }
    }
    fn set_working(&mut self, p: i64, w: bool) {
        if let Ok(i) = self.entries.binary_search_by_key(&p, |e| e.price) {
            self.entries[i].working = w;
        }
    }
    fn best(&self, buy: bool) -> Option<i64> {
        if buy {
            self.entries.iter().rev().find(|e| e.working)
        } else {
            self.entries.iter().find(|e| e.working)
        }
        .map(|e| e.price)
    }
    fn visit(&self, lo: i64, hi: i64, mut f: impl FnMut(i64, Value) -> bool) {
        let start = self.entries.partition_point(|e| e.price < lo);
        for e in &self.entries[start..] {
            if e.price > hi || !f(e.price, e.value) {
                break;
            }
        }
    }
}

// K is a representation threshold, NOT a supported-price/count limit.
// No hot-path demotion: repeated K/K+1 crossings do not repeatedly rebuild.
// Inline payload inflation is intentionally measured/documented, not hidden.
pub struct Adaptive<const K: usize> {
    entries: [Entry; K],
    len: usize,
    large: Option<Standard>,
}
impl<const K: usize> Default for Adaptive<K> {
    fn default() -> Self {
        Self {
            entries: [Entry::default(); K],
            len: 0,
            large: None,
        }
    }
}
impl<const K: usize> Locator for Adaptive<K> {
    fn get(&self, p: i64) -> Option<Value> {
        if let Some(t) = &self.large {
            return t.get(p);
        }
        self.entries[..self.len]
            .iter()
            .find(|e| e.price == p)
            .map(|e| e.value)
    }
    fn insert(&mut self, p: i64, v: Value, w: bool) {
        if let Some(t) = &mut self.large {
            t.insert(p, v, w);
            return;
        }
        let i = self.entries[..self.len].partition_point(|e| e.price < p);
        let e = Entry {
            price: p,
            value: v,
            working: w,
        };
        if i < self.len && self.entries[i].price == p {
            self.entries[i] = e;
            return;
        }
        if self.len < K {
            self.entries.copy_within(i..self.len, i + 1);
            self.entries[i] = e;
            self.len += 1;
        } else {
            let mut t = Standard::default();
            for e in &self.entries[..self.len] {
                t.insert(e.price, e.value, e.working);
            }
            t.insert(p, v, w);
            self.large = Some(t);
        }
    }
    fn remove(&mut self, p: i64) {
        if let Some(t) = &mut self.large {
            t.remove(p);
            return;
        }
        if let Some(i) = self.entries[..self.len].iter().position(|e| e.price == p) {
            self.entries.copy_within(i + 1..self.len, i);
            self.len -= 1;
        }
    }
    fn set_working(&mut self, p: i64, w: bool) {
        if let Some(t) = &mut self.large {
            t.set_working(p, w);
            return;
        }
        if let Some(e) = self.entries[..self.len].iter_mut().find(|e| e.price == p) {
            e.working = w;
        }
    }
    fn best(&self, buy: bool) -> Option<i64> {
        if let Some(t) = &self.large {
            return t.best(buy);
        }
        let entries = &self.entries[..self.len];
        if buy {
            entries.iter().rev().find(|e| e.working)
        } else {
            entries.iter().find(|e| e.working)
        }
        .map(|e| e.price)
    }
    fn visit(&self, lo: i64, hi: i64, mut f: impl FnMut(i64, Value) -> bool) {
        if let Some(t) = &self.large {
            t.visit(lo, hi, f);
            return;
        }
        for e in &self.entries[..self.len] {
            if e.price > hi {
                break;
            }
            if e.price >= lo && !f(e.price, e.value) {
                break;
            }
        }
    }
}

#[derive(Default)]
pub struct HashOrdered {
    exact: HashMap<i64, Value>,
    ordered: Standard,
}
impl Locator for HashOrdered {
    fn get(&self, p: i64) -> Option<Value> {
        self.exact.get(&p).copied()
    }
    fn insert(&mut self, p: i64, v: Value, w: bool) {
        self.exact.insert(p, v);
        self.ordered.insert(p, v, w);
    }
    fn remove(&mut self, p: i64) {
        self.exact.remove(&p);
        self.ordered.remove(p);
    }
    fn set_working(&mut self, p: i64, w: bool) {
        self.ordered.set_working(p, w);
    }
    fn best(&self, buy: bool) -> Option<i64> {
        self.ordered.best(buy)
    }
    fn visit(&self, lo: i64, hi: i64, f: impl FnMut(i64, Value) -> bool) {
        self.ordered.visit(lo, hi, f);
    }
}

struct Page {
    values: [Value; 64],
    occupied: u64,
    working: u64,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            values: [Value::default(); 64],
            occupied: 0,
            working: 0,
        }
    }
}
#[derive(Default)]
pub struct Paged {
    pages: BTreeMap<i64, Box<Page>>,
    confirmed: BTreeSet<i64>,
}
impl Locator for Paged {
    fn get(&self, p: i64) -> Option<Value> {
        let page = self.pages.get(&p.div_euclid(64))?;
        let offset = p.rem_euclid(64) as usize;
        (page.occupied & (1u64 << offset) != 0).then_some(page.values[offset])
    }
    fn insert(&mut self, p: i64, v: Value, w: bool) {
        let page = self.pages.entry(p.div_euclid(64)).or_default();
        let offset = p.rem_euclid(64) as usize;
        page.values[offset] = v;
        page.occupied |= 1u64 << offset;
        self.set_working(p, w);
    }
    fn remove(&mut self, p: i64) {
        self.set_working(p, false);
        let key = p.div_euclid(64);
        if let Some(page) = self.pages.get_mut(&key) {
            page.occupied &= !(1u64 << p.rem_euclid(64));
            if page.occupied == 0 {
                self.pages.remove(&key);
            }
        }
    }
    fn set_working(&mut self, p: i64, w: bool) {
        let key = p.div_euclid(64);
        if let Some(page) = self.pages.get_mut(&key) {
            let was = page.working != 0;
            if w {
                page.working |= 1u64 << p.rem_euclid(64);
            } else {
                page.working &= !(1u64 << p.rem_euclid(64));
            }
            let now = page.working != 0;
            if was != now {
                if now {
                    self.confirmed.insert(key);
                } else {
                    self.confirmed.remove(&key);
                }
            }
        }
    }
    fn best(&self, buy: bool) -> Option<i64> {
        let &key = if buy {
            self.confirmed.last()?
        } else {
            self.confirmed.first()?
        };
        let bits = self.pages[&key].working;
        let offset = if buy {
            63 - bits.leading_zeros()
        } else {
            bits.trailing_zeros()
        };
        Some(key * 64 + offset as i64)
    }
    fn visit(&self, lo: i64, hi: i64, mut f: impl FnMut(i64, Value) -> bool) {
        if lo > hi {
            return;
        }
        for (&key, page) in self.pages.range(lo.div_euclid(64)..=hi.div_euclid(64)) {
            let mut bits = page.occupied;
            if key == lo.div_euclid(64) {
                bits &= u64::MAX << lo.rem_euclid(64);
            }
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                let p = key * 64 + i as i64;
                if p > hi || !f(p, page.values[i]) {
                    return;
                }
                bits &= bits - 1;
            }
        }
    }
}
