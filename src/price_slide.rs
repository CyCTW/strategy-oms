//! Sliding-window price locator: a circular direct-mapped window of `N` slots
//! that follows the near-market prices, plus an adaptive sorted array
//! (`FlatDeque`, converting to the B-tree pair when large) for every price
//! outside the window.
//!
//! Slots are addressed by `price mod N`, so sliding the window only moves
//! `base`; entries inside the window never move. Invariant: a price lives in
//! the window iff `base <= price < base + N`; the outside locator never holds
//! a price in that range.
//!
//! A new price within `N / 2` of the window slides it (with `N / 4` of slack
//! so a drifting market does not slide on every tick): levels that fall out
//! are evicted to the outside array, outside prices that fall in are pulled
//! in. A farther price (an outlier) goes to the outside array directly. Inside
//! the window get/insert/remove/set_working are O(1) bit and slot operations;
//! best and ordered ranges rotate the bitmaps into price order.
//!
//! A Book starts with the outside array only: the window (N handles) is
//! created once the Book holds more than `TIER` prices, centered on the price
//! being inserted, so small Books and many-Book workloads never pay for it.
//!
//! Prices are raw storage units; with a tick larger than 1, `N` slots cover
//! `N / tick` ticks.
use crate::{
    model::Price,
    pool::Handle,
    price_deque::{AdaptiveLocator, AdaptiveRange, FlatDeque},
};
use std::ops::{BitAnd, BitOr, Not};

/// Bitmap word covering all `N` slots.
pub(crate) trait Bits:
    Copy + Eq + BitAnd<Output = Self> + BitOr<Output = Self> + Not<Output = Self>
{
    const N: u32;
    const ZERO: Self;
    fn bit(i: u32) -> Self;
    fn rotr(self, n: u32) -> Self;
    fn lz(self) -> u32;
    fn tz(self) -> u32;
    /// Clears the lowest set bit.
    fn clear_lowest(self) -> Self;
    /// Bits `lo..=hi` set (lo <= hi < N).
    fn span(lo: u32, hi: u32) -> Self;
}
macro_rules! bits_impl {
    ($t:ty) => {
        impl Bits for $t {
            const N: u32 = <$t>::BITS;
            const ZERO: Self = 0;
            #[inline]
            fn bit(i: u32) -> Self {
                1 << i
            }
            #[inline]
            fn rotr(self, n: u32) -> Self {
                self.rotate_right(n)
            }
            #[inline]
            fn lz(self) -> u32 {
                self.leading_zeros()
            }
            #[inline]
            fn tz(self) -> u32 {
                self.trailing_zeros()
            }
            #[inline]
            fn clear_lowest(self) -> Self {
                self & self.wrapping_sub(1)
            }
            #[inline]
            fn span(lo: u32, hi: u32) -> Self {
                (<$t>::MAX << lo) & (<$t>::MAX >> (<$t>::BITS - 1 - hi))
            }
        }
    };
}
bits_impl!(u64);
bits_impl!(u128);

struct Window<B: Bits, const N: usize> {
    base: Price,
    occupied: B,
    working: B,
    /// Indexed by `price mod N`; valid where `occupied` has the bit.
    slots: [Handle; N],
}

/// Prices a Book may hold before its window is created.
pub(crate) const TIER: usize = 8;

pub(crate) struct SlideLocator<B: Bits = u64, const N: usize = 64> {
    /// Allocated on the first insert, reused afterwards.
    window: Option<Box<Window<B, N>>>,
    outside: AdaptiveLocator<FlatDeque>,
}

impl<B: Bits, const N: usize> Default for SlideLocator<B, N> {
    fn default() -> Self {
        Self {
            window: None,
            outside: AdaptiveLocator::default(),
        }
    }
}

impl<B: Bits, const N: usize> Window<B, N> {
    const VALID: () = assert!(N as u32 == B::N && N.is_power_of_two());
    const MAX_BASE: Price = Price::MAX - (N as Price - 1);

    #[inline]
    fn contains(&self, p: Price) -> bool {
        (p as u64).wrapping_sub(self.base as u64) < N as u64
    }
    #[inline]
    fn slot(p: Price) -> u32 {
        (p as u64 & (N as u64 - 1)) as u32
    }
    #[inline]
    fn has(&self, p: Price) -> bool {
        self.occupied & B::bit(Self::slot(p)) != B::ZERO
    }
    #[inline]
    fn put(&mut self, p: Price, h: Handle, working: bool) {
        let s = Self::slot(p);
        let m = B::bit(s);
        self.slots[s as usize] = h;
        self.occupied = self.occupied | m;
        self.working = (self.working & !m) | if working { m } else { B::ZERO };
    }
    #[inline]
    fn clear(&mut self, p: Price) {
        let m = !B::bit(Self::slot(p));
        self.occupied = self.occupied & m;
        self.working = self.working & m;
    }
    /// Rotates a slot-space bitmap so bit k is price `base + k`.
    #[inline]
    fn ordered(&self, x: B) -> B {
        x.rotr(Self::slot(self.base))
    }
    fn best(&self, buy: bool) -> Option<Price> {
        let r = self.ordered(self.working);
        if r == B::ZERO {
            None
        } else if buy {
            Some(self.base + (N as u32 - 1 - r.lz()) as Price)
        } else {
            Some(self.base + r.tz() as Price)
        }
    }
}

impl<B: Bits, const N: usize> SlideLocator<B, N> {
    pub fn get(&self, p: Price) -> Option<Handle> {
        if let Some(w) = &self.window
            && w.contains(p)
        {
            return w.has(p).then(|| w.slots[Window::<B, N>::slot(p) as usize]);
        }
        self.outside.get(p)
    }

    /// Moves the window to start at `base`: evicts occupied levels that fall
    /// out to the outside array and pulls outside prices that fall in.
    fn slide(&mut self, base: Price) {
        let w = self.window.as_mut().unwrap();
        let base = base.min(Window::<B, N>::MAX_BASE);
        let (old, n) = (w.base, N as Price);
        let overlap = N as i128 - (i128::from(base) - i128::from(old)).abs();
        if base == old {
            // Nothing leaves; a new window still pulls in below.
        } else if overlap <= 0 || w.occupied == B::ZERO {
            // Disjoint (or empty): every occupied level leaves.
            let mut bits = w.ordered(w.occupied);
            while bits != B::ZERO {
                let p = old + bits.tz() as Price;
                bits = bits.clear_lowest();
                let s = Window::<B, N>::slot(p);
                let working = w.working & B::bit(s) != B::ZERO;
                self.outside.insert(p, w.slots[s as usize], working);
            }
            w.occupied = B::ZERO;
            w.working = B::ZERO;
        } else {
            // Leaving span, in window-order offsets relative to `old`
            // (0 < |base - old| < n here).
            let (lo, hi) = if base > old {
                (0, (base - old - 1) as u32)
            } else {
                (overlap as u32, N as u32 - 1)
            };
            let mut bits = w.ordered(w.occupied) & B::span(lo, hi);
            while bits != B::ZERO {
                let p = old + bits.tz() as Price;
                bits = bits.clear_lowest();
                let s = Window::<B, N>::slot(p);
                let working = w.working & B::bit(s) != B::ZERO;
                self.outside.insert(p, w.slots[s as usize], working);
                w.clear(p);
            }
        }
        w.base = base;
        // Pull in outside prices now covered (only the newly covered span can
        // hold any, but scanning the whole window range is equally correct).
        let top = base + (n - 1);
        while let Some((p, h)) = self.outside.range(base, top).next() {
            let working = self.outside.is_working(p);
            self.outside.remove(p);
            w.put(p, h, working);
        }
    }

    /// For a price outside the window: the window start that would cover it,
    /// or None when it is an outlier that belongs to the outside array.
    /// `Some(None)` means no window exists yet.
    fn slide_target(&self, p: Price) -> Option<Option<Price>> {
        let n = N as Price;
        let Some(w) = self.window.as_ref() else {
            // No window until the Book outgrows the small array.
            return (self.outside.len() >= TIER).then_some(None);
        };
        // Unsigned distances: prices may span the whole i64 range.
        let above = (p as u64).wrapping_sub(w.base as u64);
        let below = (w.base as u64).wrapping_sub(p as u64);
        if w.occupied == B::ZERO {
            Some(Some(p.saturating_sub(n / 2)))
        } else if p >= w.base && above < (n + n / 2) as u64 {
            // Above: leave n/4 of room above p.
            Some(Some(p - (n - 1) + n / 4))
        } else if p < w.base && below <= (n / 2) as u64 {
            // Below: leave n/4 of room below p.
            Some(Some(p.saturating_sub(n / 4)))
        } else {
            None
        }
    }

    /// Slides (or creates) the window for `target` from `slide_target`.
    fn cover(&mut self, target: Option<Price>, p: Price, filler: Handle) {
        let () = Window::<B, N>::VALID;
        let n = N as Price;
        let Some(target) = target else {
            self.window = Some(Box::new(Window {
                base: p.saturating_sub(n / 2).min(Window::<B, N>::MAX_BASE),
                occupied: B::ZERO,
                working: B::ZERO,
                slots: [filler; N],
            }));
            let base = self.window.as_ref().unwrap().base;
            self.slide(base); // pulls in covered outside prices
            return;
        };
        self.slide(target);
        debug_assert!(self.window.as_ref().unwrap().contains(p));
    }

    pub fn insert(&mut self, p: Price, h: Handle, working: bool) {
        if let Some(w) = self.window.as_mut()
            && w.contains(p)
        {
            w.put(p, h, working);
            return;
        }
        match self.slide_target(p) {
            Some(target) if self.outside.get(p).is_none() => {
                self.cover(target, p, h);
                self.window.as_mut().unwrap().put(p, h, working);
            }
            _ => self.outside.insert(p, h, working),
        }
    }

    pub fn find_or_insert(
        &mut self,
        p: Price,
        make: impl FnOnce() -> Handle,
        working: bool,
    ) -> (Handle, bool) {
        if let Some(w) = self.window.as_mut()
            && w.contains(p)
        {
            if w.has(p) {
                return (w.slots[Window::<B, N>::slot(p) as usize], false);
            }
            let h = make();
            w.put(p, h, working);
            return (h, true);
        }
        let Some(target) = self.slide_target(p) else {
            // Outlier: one search in the outside array.
            return self.outside.find_or_insert(p, make, working);
        };
        if let Some(h) = self.outside.get(p) {
            return (h, false);
        }
        let h = make();
        self.cover(target, p, h);
        self.window.as_mut().unwrap().put(p, h, working);
        (h, true)
    }

    pub fn remove(&mut self, p: Price) {
        if let Some(w) = self.window.as_mut()
            && w.contains(p)
        {
            w.clear(p);
            return;
        }
        self.outside.remove(p);
    }

    pub fn set_working(&mut self, p: Price, working: bool) {
        if let Some(w) = self.window.as_mut()
            && w.contains(p)
        {
            if w.has(p) {
                let m = B::bit(Window::<B, N>::slot(p));
                w.working = (w.working & !m) | if working { m } else { B::ZERO };
            }
            return;
        }
        self.outside.set_working(p, working);
    }

    pub fn best(&self, buy: bool) -> Option<Price> {
        let inner = self.window.as_ref().and_then(|w| w.best(buy));
        let outer = self.outside.best(buy);
        match (inner, outer) {
            (Some(a), Some(b)) => Some(if buy { a.max(b) } else { a.min(b) }),
            (a, b) => a.or(b),
        }
    }

    /// Callers pass a non-empty range. Ascending: outside prices below the
    /// window, the window, then outside prices above it.
    pub fn range(&self, low: Price, high: Price) -> SlideRange<'_, B, N> {
        let Some(w) = self.window.as_deref() else {
            return SlideRange {
                below: Some(self.outside.range(low, high)),
                window: None,
                above: None,
            };
        };
        let top = w.base + (N as Price - 1);
        // Outside segments only when the outside array can intersect them.
        let bounds = self.outside.bounds();
        let below_high = high.min(w.base.saturating_sub(1));
        let below = (low <= below_high
            && w.base > Price::MIN
            && bounds.is_some_and(|(min, _)| min <= below_high))
        .then(|| self.outside.range(low, below_high));
        let (lo, hi) = (low.max(w.base), high.min(top));
        let window = (lo <= hi).then(|| {
            let span = B::span((lo - w.base) as u32, (hi - w.base) as u32);
            (w, w.ordered(w.occupied) & span)
        });
        let above = (high > top && top < Price::MAX && bounds.is_some_and(|(_, max)| max > top))
            .then(|| self.outside.range(low.max(top + 1), high));
        SlideRange {
            below,
            window,
            above,
        }
    }

    #[cfg(test)]
    fn in_window(&self, p: Price) -> bool {
        self.window
            .as_ref()
            .is_some_and(|w| w.contains(p) && w.has(p))
    }
}

pub(crate) struct SlideRange<'a, B: Bits = u64, const N: usize = 64> {
    below: Option<AdaptiveRange<'a, FlatDeque>>,
    window: Option<(&'a Window<B, N>, B)>,
    above: Option<AdaptiveRange<'a, FlatDeque>>,
}
impl<B: Bits, const N: usize> Iterator for SlideRange<'_, B, N> {
    type Item = (Price, Handle);
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(x) = self.below.as_mut().and_then(Iterator::next) {
            return Some(x);
        }
        if let Some((w, bits)) = &mut self.window
            && *bits != B::ZERO
        {
            let p = w.base + bits.tz() as Price;
            *bits = bits.clear_lowest();
            return Some((p, w.slots[Window::<B, N>::slot(p) as usize]));
        }
        self.above.as_mut()?.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Pool;
    use std::collections::BTreeMap;

    fn rng(s: &mut u64) -> u64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        *s >> 33
    }
    fn check<B: Bits, const N: usize>(
        t: &SlideLocator<B, N>,
        reference: &BTreeMap<Price, (Handle, bool)>,
        probe: Price,
    ) {
        assert_eq!(t.get(probe), reference.get(&probe).map(|v| v.0));
        let working = reference.iter().filter(|(_, v)| v.1).map(|(p, _)| *p);
        assert_eq!(t.best(true), working.clone().next_back());
        assert_eq!(t.best(false), working.clone().next());
        let got: Vec<_> = t.range(Price::MIN, Price::MAX).collect();
        let expected: Vec<_> = reference.iter().map(|(p, v)| (*p, v.0)).collect();
        assert_eq!(got, expected);
        for (lo, hi) in [
            (probe.saturating_sub(70), probe.saturating_add(70)),
            (probe, probe),
            (Price::MIN, probe),
            (probe, Price::MAX),
            (-50, 50),
        ] {
            let got: Vec<_> = t.range(lo, hi).map(|x| x.0).collect();
            let expected: Vec<_> = reference.range(lo..=hi).map(|x| *x.0).collect();
            assert_eq!(got, expected, "range {lo}..={hi}");
        }
    }

    /// Random walk of a drifting center with far outliers and extremes,
    /// mixing every operation, checked against a BTreeMap after each step.
    fn random_walk<B: Bits, const N: usize>(seed: u64, spread: u64, drift: u64) -> (usize, usize) {
        let mut pool = Pool::default();
        let hs: Vec<Handle> = (0..64).map(|i| pool.insert(i)).collect();
        let mut t = SlideLocator::<B, N>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let mut s = seed;
        let mut center: Price = 0;
        let (mut inside, mut total) = (0, 0);
        for step in 0..20_000 {
            let r = rng(&mut s);
            if step % 10 == 0 {
                center += (r % (2 * drift + 1)) as Price - drift as Price;
            }
            let p = match step % 101 {
                0 => Price::MIN,
                1 => Price::MAX,
                2 => Price::MAX - 5,
                3 => Price::MIN + 7,
                4 => 1_000_000_000,
                _ => center + (r % spread) as Price - (spread / 2) as Price,
            };
            let h = hs[(r % 64) as usize];
            let w = r & 8 != 0;
            match r % 6 {
                0 | 1 => {
                    t.remove(p);
                    reference.remove(&p);
                }
                2 => {
                    if reference.contains_key(&p) {
                        t.set_working(p, w);
                        reference.get_mut(&p).unwrap().1 = w;
                    }
                }
                3 => {
                    let expected = match reference.get(&p) {
                        Some(&(existing, _)) => (existing, false),
                        None => {
                            reference.insert(p, (h, w));
                            (h, true)
                        }
                    };
                    assert_eq!(t.find_or_insert(p, || h, w), expected);
                }
                _ => {
                    t.insert(p, h, w);
                    reference.insert(p, (h, w));
                }
            }
            if reference.contains_key(&p) {
                total += 1;
                inside += usize::from(t.in_window(p));
            }
            check(&t, &reference, p);
        }
        (inside, total)
    }

    #[test]
    fn matches_reference_while_sliding() {
        for (seed, spread, drift) in [(1, 20, 3), (2, 60, 10), (3, 150, 40), (4, 3000, 300)] {
            let (inside, total) = random_walk::<u64, 64>(seed, spread, drift);
            assert!(inside > 0 && inside < total, "64: {inside}/{total}");
            let (inside, total) = random_walk::<u128, 128>(seed, spread * 2, drift);
            assert!(inside > 0 && inside < total, "128: {inside}/{total}");
        }
    }

    /// A rolling book (best level fills, a new level one tick below the
    /// worst) keeps every level in the window by sliding.
    #[test]
    fn rolling_book_stays_in_window() {
        let mut pool = Pool::default();
        let hs: Vec<Handle> = (0..8).map(|i| pool.insert(i)).collect();
        let mut t = SlideLocator::<u64, 64>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let (mut top, mut bottom) = (1000, 1000 - 30);
        for p in bottom..=top {
            t.insert(p, hs[0], p % 3 == 0);
            reference.insert(p, (hs[0], p % 3 == 0));
        }
        for step in 0..5_000 {
            t.remove(top);
            reference.remove(&top);
            top -= 1;
            bottom -= 1;
            let (h, created) = t.find_or_insert(bottom, || hs[step % 8], step % 2 == 0);
            assert!(created);
            reference.insert(bottom, (h, step % 2 == 0));
            assert!(t.in_window(bottom) && t.in_window(top));
            check(&t, &reference, bottom);
        }
    }
}
