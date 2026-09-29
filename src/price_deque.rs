//! Adaptive price locator: a sorted double-ended array while a Book has few
//! prices, the standard BTreeMap/BTreeSet pair once it grows.
//!
//! Small mode keeps `(price, level, working)` ascending in a double-ended
//! store. The locator does not know the Book side, so bids keep their best
//! price at the back and asks at the front; the store makes a new best or
//! worst price O(1) on both sides and shifts the shorter side for interior
//! prices. There is no price-count or price-range limit.
//!
//! Past `PROMOTE` prices the Book converts to the B-tree pair (random interior
//! inserts would otherwise cost O(P) moves); below `DEMOTE` it converts back.
//! The gap is hysteresis so a Book hovering near one threshold does not
//! convert back and forth. A conversion copies every price once: an O(P)
//! spike on that single operation.
use crate::{model::Price, pool::Handle};
use std::collections::{BTreeMap, BTreeSet, VecDeque, btree_map, vec_deque};

/// Prices at which a small Book converts to the B-tree pair. Measured
/// crossover for random repricing (docs/sorted-deque.md), not a capacity limit.
pub(crate) const PROMOTE: usize = 1024;
/// Prices below which a large Book converts back to the array.
pub(crate) const DEMOTE: usize = 256;
/// Up to this many prices a scan beats binary search.
const LINEAR_MAX: usize = 16;

#[derive(Clone, Copy)]
pub(crate) struct Entry {
    price: Price,
    level: Handle,
    working: bool,
}

/// Ascending small-mode storage.
pub(crate) trait SmallStore: Default + FromIterator<Entry> {
    type Iter<'a>: DoubleEndedIterator<Item = &'a Entry>
    where
        Self: 'a;
    fn len(&self) -> usize;
    /// First position whose price is >= p.
    fn lower(&self, p: Price) -> usize;
    fn at(&self, i: usize) -> Option<&Entry>;
    fn at_mut(&mut self, i: usize) -> Option<&mut Entry>;
    fn insert_at(&mut self, i: usize, e: Entry);
    fn remove_at(&mut self, i: usize);
    fn iter_from(&self, i: usize) -> Self::Iter<'_>;
}

/// Ring buffer. Every index goes through wrap-around arithmetic.
impl SmallStore for VecDeque<Entry> {
    type Iter<'a> = vec_deque::Iter<'a, Entry>;
    fn len(&self) -> usize {
        VecDeque::len(self)
    }
    fn lower(&self, p: Price) -> usize {
        if self.len() <= LINEAR_MAX {
            self.iter().position(|e| e.price >= p).unwrap_or(self.len())
        } else {
            self.partition_point(|e| e.price < p)
        }
    }
    fn at(&self, i: usize) -> Option<&Entry> {
        self.get(i)
    }
    fn at_mut(&mut self, i: usize) -> Option<&mut Entry> {
        self.get_mut(i)
    }
    fn insert_at(&mut self, i: usize, e: Entry) {
        if i == self.len() {
            self.push_back(e);
        } else if i == 0 {
            self.push_front(e);
        } else {
            self.insert(i, e);
        }
    }
    fn remove_at(&mut self, i: usize) {
        self.remove(i);
    }
    fn iter_from(&self, i: usize) -> Self::Iter<'_> {
        self.range(i..)
    }
}

/// Contiguous buffer with spare slots at both ends: live entries are
/// `buf[head..tail]`, so searches run on one slice. When an end fills up it
/// recenters if a quarter is spare, otherwise doubles. Spare slots hold stale
/// copies and are never read.
#[derive(Default)]
pub(crate) struct FlatDeque {
    buf: Vec<Entry>,
    head: usize,
    tail: usize,
}

impl FlatDeque {
    fn live(&self) -> &[Entry] {
        &self.buf[self.head..self.tail]
    }
    fn reallocate(&mut self, cap: usize, filler: Entry) {
        let n = self.tail - self.head;
        let head = (cap - n) / 2;
        let mut buf = vec![filler; cap];
        buf[head..head + n].copy_from_slice(self.live());
        *self = Self {
            buf,
            head,
            tail: head + n,
        };
    }
    /// Requires len < capacity afterwards to have room at the requested end.
    fn make_room(&mut self, front: bool, filler: Entry) {
        let cap = self.buf.len();
        let n = self.tail - self.head;
        if (cap - n) * 4 < cap {
            self.reallocate(cap * 2, filler);
            return;
        }
        let spare = cap - n;
        let head = if front { spare.div_ceil(2) } else { spare / 2 };
        self.buf.copy_within(self.head..self.tail, head);
        self.head = head;
        self.tail = head + n;
    }
}

impl FromIterator<Entry> for FlatDeque {
    fn from_iter<I: IntoIterator<Item = Entry>>(iter: I) -> Self {
        let live: Vec<Entry> = iter.into_iter().collect();
        let Some(&filler) = live.first() else {
            return Self::default();
        };
        let cap = (live.len() * 2).max(8);
        let head = (cap - live.len()) / 2;
        let mut buf = vec![filler; cap];
        buf[head..head + live.len()].copy_from_slice(&live);
        Self {
            buf,
            head,
            tail: head + live.len(),
        }
    }
}

impl SmallStore for FlatDeque {
    type Iter<'a> = std::slice::Iter<'a, Entry>;
    fn len(&self) -> usize {
        self.tail - self.head
    }
    fn lower(&self, p: Price) -> usize {
        let s = self.live();
        if s.len() <= LINEAR_MAX {
            // Branch-free: count the prices below p.
            return s.iter().map(|e| usize::from(e.price < p)).sum();
        }
        s.partition_point(|e| e.price < p)
    }
    fn at(&self, i: usize) -> Option<&Entry> {
        self.live().get(i)
    }
    fn at_mut(&mut self, i: usize) -> Option<&mut Entry> {
        let (head, tail) = (self.head, self.tail);
        self.buf[head..tail].get_mut(i)
    }
    fn insert_at(&mut self, i: usize, e: Entry) {
        let n = self.len();
        if n == self.buf.len() {
            self.reallocate((n * 2).max(8), e);
        }
        if i == n {
            if self.tail == self.buf.len() {
                self.make_room(false, e);
            }
            self.buf[self.tail] = e;
            self.tail += 1;
            return;
        }
        if i == 0 {
            if self.head == 0 {
                self.make_room(true, e);
            }
            self.head -= 1;
            self.buf[self.head] = e;
            return;
        }
        // Shift the shorter side; if its end is full, recenter (or grow)
        // first instead of shifting the longer side.
        let left = i < n - i;
        if left && self.head == 0 {
            self.make_room(true, e);
        } else if !left && self.tail == self.buf.len() {
            self.make_room(false, e);
        }
        if left {
            self.buf
                .copy_within(self.head..self.head + i, self.head - 1);
            self.head -= 1;
        } else {
            self.buf
                .copy_within(self.head + i..self.tail, self.head + i + 1);
            self.tail += 1;
        }
        self.buf[self.head + i] = e;
    }
    fn remove_at(&mut self, i: usize) {
        let n = self.len();
        if i + 1 == n {
            self.tail -= 1;
        } else if i == 0 {
            self.head += 1;
        } else if i < n - i {
            self.buf
                .copy_within(self.head..self.head + i, self.head + 1);
            self.head += 1;
        } else {
            self.buf
                .copy_within(self.head + i + 1..self.tail, self.head + i);
            self.tail -= 1;
        }
        if self.head == self.tail {
            self.head = self.buf.len() / 2;
            self.tail = self.head;
        }
    }
    fn iter_from(&self, i: usize) -> Self::Iter<'_> {
        self.live()[i..].iter()
    }
}

pub(crate) enum AdaptiveLocator<
    S: SmallStore = VecDeque<Entry>,
    const P: usize = PROMOTE,
    const D: usize = DEMOTE,
> {
    Small(S),
    Large {
        levels: BTreeMap<Price, Handle>,
        confirmed: BTreeSet<Price>,
    },
}

impl<S: SmallStore, const P: usize, const D: usize> Default for AdaptiveLocator<S, P, D> {
    fn default() -> Self {
        Self::Small(S::default())
    }
}

impl<S: SmallStore, const P: usize, const D: usize> AdaptiveLocator<S, P, D> {
    const VALID: () = assert!(D < P, "demote threshold must be below promote");

    pub fn get(&self, p: Price) -> Option<Handle> {
        match self {
            Self::Small(q) => q.at(q.lower(p)).filter(|e| e.price == p).map(|e| e.level),
            Self::Large { levels, .. } => levels.get(&p).copied(),
        }
    }
    fn promote_if_large(&mut self) {
        if let Self::Small(q) = self
            && q.len() > P
        {
            *self = Self::Large {
                levels: q.iter_from(0).map(|e| (e.price, e.level)).collect(),
                confirmed: q
                    .iter_from(0)
                    .filter(|e| e.working)
                    .map(|e| e.price)
                    .collect(),
            };
        }
    }
    pub fn insert(&mut self, p: Price, h: Handle, working: bool) {
        let () = Self::VALID;
        match self {
            Self::Small(q) => {
                let e = Entry {
                    price: p,
                    level: h,
                    working,
                };
                let i = q.lower(p);
                match q.at_mut(i) {
                    Some(x) if x.price == p => *x = e,
                    _ => q.insert_at(i, e),
                }
                self.promote_if_large();
            }
            Self::Large { levels, confirmed } => {
                levels.insert(p, h);
                if working {
                    confirmed.insert(p);
                } else {
                    confirmed.remove(&p);
                }
            }
        }
    }
    /// One search: returns the existing level, or inserts `make()` with the
    /// given working flag at the position that search found.
    pub fn find_or_insert(
        &mut self,
        p: Price,
        make: impl FnOnce() -> Handle,
        working: bool,
    ) -> (Handle, bool) {
        let () = Self::VALID;
        match self {
            Self::Small(q) => {
                let i = q.lower(p);
                if let Some(e) = q.at(i).filter(|e| e.price == p) {
                    return (e.level, false);
                }
                let h = make();
                q.insert_at(
                    i,
                    Entry {
                        price: p,
                        level: h,
                        working,
                    },
                );
                self.promote_if_large();
                (h, true)
            }
            Self::Large { levels, confirmed } => match levels.entry(p) {
                btree_map::Entry::Occupied(o) => (*o.get(), false),
                btree_map::Entry::Vacant(v) => {
                    let h = make();
                    v.insert(h);
                    if working {
                        confirmed.insert(p);
                    }
                    (h, true)
                }
            },
        }
    }
    pub fn remove(&mut self, p: Price) {
        match self {
            Self::Small(q) => {
                let i = q.lower(p);
                if q.at(i).is_some_and(|e| e.price == p) {
                    q.remove_at(i);
                }
            }
            Self::Large { levels, confirmed } => {
                levels.remove(&p);
                confirmed.remove(&p);
                if levels.len() < D {
                    let q = levels
                        .iter()
                        .map(|(&price, &level)| Entry {
                            price,
                            level,
                            working: confirmed.contains(&price),
                        })
                        .collect();
                    *self = Self::Small(q);
                }
            }
        }
    }
    pub fn set_working(&mut self, p: Price, working: bool) {
        match self {
            Self::Small(q) => {
                let i = q.lower(p);
                if let Some(e) = q.at_mut(i).filter(|e| e.price == p) {
                    e.working = working;
                }
            }
            Self::Large { confirmed, .. } => {
                if working {
                    confirmed.insert(p);
                } else {
                    confirmed.remove(&p);
                }
            }
        }
    }
    pub fn len(&self) -> usize {
        match self {
            Self::Small(q) => q.len(),
            Self::Large { levels, .. } => levels.len(),
        }
    }
    /// Lowest and highest stored price, O(1).
    pub fn bounds(&self) -> Option<(Price, Price)> {
        match self {
            Self::Small(q) if q.len() == 0 => None,
            Self::Small(q) => Some((q.at(0)?.price, q.at(q.len() - 1)?.price)),
            Self::Large { levels, .. } => {
                Some((*levels.first_key_value()?.0, *levels.last_key_value()?.0))
            }
        }
    }
    pub fn is_working(&self, p: Price) -> bool {
        match self {
            Self::Small(q) => q.at(q.lower(p)).is_some_and(|e| e.price == p && e.working),
            Self::Large { confirmed, .. } => confirmed.contains(&p),
        }
    }
    /// Scans inward from the side's best end past pending-only prices. The
    /// Index caches best and only calls this when the cache is invalidated.
    pub fn best(&self, buy: bool) -> Option<Price> {
        match self {
            Self::Small(q) if buy => q.iter_from(0).rev().find(|e| e.working).map(|e| e.price),
            Self::Small(q) => q.iter_from(0).find(|e| e.working).map(|e| e.price),
            Self::Large { confirmed, .. } if buy => confirmed.last().copied(),
            Self::Large { confirmed, .. } => confirmed.first().copied(),
        }
    }
    /// Callers pass a non-empty range.
    pub fn range(&self, low: Price, high: Price) -> AdaptiveRange<'_, S> {
        match self {
            Self::Small(q) => AdaptiveRange::Small {
                iter: q.iter_from(q.lower(low)),
                high,
            },
            Self::Large { levels, .. } => AdaptiveRange::Large(levels.range(low..=high)),
        }
    }
    #[cfg(test)]
    fn is_large(&self) -> bool {
        matches!(self, Self::Large { .. })
    }
}

pub(crate) enum AdaptiveRange<'a, S: SmallStore + 'a = VecDeque<Entry>> {
    Small { iter: S::Iter<'a>, high: Price },
    Large(btree_map::Range<'a, Price, Handle>),
}
impl<'a, S: SmallStore + 'a> Iterator for AdaptiveRange<'a, S> {
    type Item = (Price, Handle);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Small { iter, high } => iter
                .next()
                .filter(|e| e.price <= *high)
                .map(|e| (e.price, e.level)),
            Self::Large(i) => i.next().map(|(p, h)| (*p, *h)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::Pool;

    fn handles(n: usize) -> Vec<Handle> {
        let mut pool = Pool::default();
        (0..n).map(|i| pool.insert(i)).collect()
    }
    fn rng(s: &mut u64) -> u64 {
        *s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
        *s >> 33
    }
    fn check<S: SmallStore, const P: usize, const D: usize>(
        t: &AdaptiveLocator<S, P, D>,
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
        for (lo, hi) in [(-50, 50), (probe, probe), (Price::MIN, -1), (0, Price::MAX)] {
            let got: Vec<_> = t.range(lo, hi).map(|x| x.0).collect();
            let expected: Vec<_> = reference.range(lo..=hi).map(|x| *x.0).collect();
            assert_eq!(got, expected);
        }
    }

    /// Random operations against a BTreeMap reference, with extreme prices,
    /// in both modes (default thresholds stay small; tiny ones convert often).
    /// Inserts alternate between `insert` and `find_or_insert`.
    fn random_against_reference<S: SmallStore, const P: usize, const D: usize>(span: u64) -> usize {
        let hs = handles(64);
        let mut t = AdaptiveLocator::<S, P, D>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let mut s = 17;
        let mut conversions = 0;
        let mut was_large = false;
        for step in 0..30_000 {
            let r = rng(&mut s);
            let p = match step % 97 {
                0 => Price::MIN,
                1 => Price::MAX,
                _ => (r % span) as Price - (span / 2) as Price,
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
            conversions += usize::from(t.is_large() != was_large);
            was_large = t.is_large();
            check(&t, &reference, p);
        }
        conversions
    }

    #[test]
    fn small_mode_matches_reference() {
        // span 40 keeps a Book far below the default thresholds.
        assert_eq!(
            random_against_reference::<VecDeque<Entry>, PROMOTE, DEMOTE>(40),
            0
        );
        assert_eq!(
            random_against_reference::<FlatDeque, PROMOTE, DEMOTE>(40),
            0
        );
        // span 400: past LINEAR_MAX, so binary search runs too.
        assert_eq!(
            random_against_reference::<VecDeque<Entry>, PROMOTE, DEMOTE>(400),
            0
        );
        assert_eq!(
            random_against_reference::<FlatDeque, PROMOTE, DEMOTE>(400),
            0
        );
    }

    #[test]
    fn converting_modes_match_reference() {
        // Tiny promote threshold: the Book settles near 30 prices, so most of
        // the run is in Large mode (oscillation below covers demotion).
        assert!(random_against_reference::<VecDeque<Entry>, 16, 4>(60) >= 1);
        assert!(random_against_reference::<FlatDeque, 16, 4>(60) >= 1);
    }

    /// Size oscillates across both thresholds with mixed working flags, so
    /// promotion and demotion must carry every flag.
    fn threshold_oscillation<S: SmallStore>() {
        let hs = handles(8);
        let mut t = AdaptiveLocator::<S, 16, 4>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let mut s = 99;
        let mut target = 0;
        let (mut conversions, mut was_large) = (0, false);
        for step in 0..40_000 {
            let r = rng(&mut s);
            if step % 64 == 0 {
                target = if r.is_multiple_of(2) { 24 } else { 1 };
            }
            let p = (r % 200) as Price - 100;
            let w = r & 16 != 0;
            if reference.len() < target && !reference.contains_key(&p) {
                t.insert(p, hs[step % 8], w);
                reference.insert(p, (hs[step % 8], w));
            } else if reference.len() > target {
                let victim = *reference.keys().nth(r as usize % reference.len()).unwrap();
                t.remove(victim);
                reference.remove(&victim);
            } else if reference.contains_key(&p) {
                t.set_working(p, w);
                reference.get_mut(&p).unwrap().1 = w;
            }
            conversions += usize::from(t.is_large() != was_large);
            was_large = t.is_large();
            check(&t, &reference, p);
        }
        assert!(conversions > 100);
    }
    #[test]
    fn threshold_oscillation_keeps_working_flags() {
        threshold_oscillation::<VecDeque<Entry>>();
        threshold_oscillation::<FlatDeque>();
    }

    /// Pushes at both ends and removals from both ends and the middle: covers
    /// FlatDeque growth, recentering and the empty reset.
    fn both_ends<S: SmallStore>() {
        let hs = handles(4);
        let mut t = AdaptiveLocator::<S, PROMOTE, DEMOTE>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let mut s = 5;
        for round in 0..3 {
            for i in 0..600i64 {
                let p = match (i + round) % 3 {
                    0 => i * 1000 + 1,
                    1 => -i * 1000 - 1,
                    _ => (rng(&mut s) % 100_000) as Price - 50_000,
                };
                t.insert(p, hs[(i % 4) as usize], true);
                reference.insert(p, (hs[(i % 4) as usize], true));
                check(&t, &reference, p);
            }
            while let Some(&p) = match rng(&mut s) % 3 {
                0 => reference.keys().next(),
                1 => reference.keys().next_back(),
                _ => reference.keys().nth(s as usize % reference.len().max(1)),
            } {
                t.remove(p);
                reference.remove(&p);
                check(&t, &reference, p);
            }
        }
    }
    #[test]
    fn both_ends_growth_and_removal() {
        both_ends::<VecDeque<Entry>>();
        both_ends::<FlatDeque>();
    }
}
