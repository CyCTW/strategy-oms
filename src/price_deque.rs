//! Adaptive price locator: a sorted double-ended array while a Book has few
//! prices, the standard BTreeMap/BTreeSet pair once it grows.
//!
//! Small mode keeps `(price, level, working)` ascending in a `VecDeque`. The
//! locator does not know the Book side, so bids keep their best price at the
//! back and asks at the front; a ring buffer makes a new best or worst price
//! O(1) on both sides, and `VecDeque::insert`/`remove` shift the shorter side
//! for interior prices. There is no price-count or price-range limit.
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
/// Up to this many prices a front-to-back scan beats binary search.
const LINEAR_MAX: usize = 16;

#[derive(Clone, Copy)]
pub(crate) struct Entry {
    price: Price,
    level: Handle,
    working: bool,
}

pub(crate) enum AdaptiveLocator<const P: usize = PROMOTE, const D: usize = DEMOTE> {
    Small(VecDeque<Entry>),
    Large {
        levels: BTreeMap<Price, Handle>,
        confirmed: BTreeSet<Price>,
    },
}

impl<const P: usize, const D: usize> Default for AdaptiveLocator<P, D> {
    fn default() -> Self {
        Self::Small(VecDeque::new())
    }
}

/// First position whose price is >= p.
fn lower(q: &VecDeque<Entry>, p: Price) -> usize {
    if q.len() <= LINEAR_MAX {
        q.iter().position(|e| e.price >= p).unwrap_or(q.len())
    } else {
        q.partition_point(|e| e.price < p)
    }
}

impl<const P: usize, const D: usize> AdaptiveLocator<P, D> {
    const VALID: () = assert!(D < P, "demote threshold must be below promote");

    pub fn get(&self, p: Price) -> Option<Handle> {
        match self {
            Self::Small(q) => {
                let i = lower(q, p);
                q.get(i).filter(|e| e.price == p).map(|e| e.level)
            }
            Self::Large { levels, .. } => levels.get(&p).copied(),
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
                let i = lower(q, p);
                if let Some(x) = q.get_mut(i).filter(|x| x.price == p) {
                    *x = e;
                } else if i == q.len() {
                    q.push_back(e);
                } else if i == 0 {
                    q.push_front(e);
                } else {
                    q.insert(i, e);
                }
                if q.len() > P {
                    *self = Self::Large {
                        levels: q.iter().map(|e| (e.price, e.level)).collect(),
                        confirmed: q.iter().filter(|e| e.working).map(|e| e.price).collect(),
                    };
                }
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
    pub fn remove(&mut self, p: Price) {
        match self {
            Self::Small(q) => {
                let i = lower(q, p);
                if q.get(i).is_some_and(|e| e.price == p) {
                    q.remove(i);
                }
            }
            Self::Large { levels, confirmed } => {
                levels.remove(&p);
                confirmed.remove(&p);
                if levels.len() < D {
                    // Ascending source: every element is a push_back.
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
                let i = lower(q, p);
                if let Some(e) = q.get_mut(i).filter(|e| e.price == p) {
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
    /// Scans inward from the side's best end past pending-only prices. The
    /// Index caches best and only calls this when the cache is invalidated.
    pub fn best(&self, buy: bool) -> Option<Price> {
        match self {
            Self::Small(q) if buy => q.iter().rev().find(|e| e.working).map(|e| e.price),
            Self::Small(q) => q.iter().find(|e| e.working).map(|e| e.price),
            Self::Large { confirmed, .. } if buy => confirmed.last().copied(),
            Self::Large { confirmed, .. } => confirmed.first().copied(),
        }
    }
    /// Callers pass a non-empty range.
    pub fn range(&self, low: Price, high: Price) -> AdaptiveRange<'_> {
        match self {
            Self::Small(q) => AdaptiveRange::Small {
                iter: q.range(lower(q, low)..),
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

pub(crate) enum AdaptiveRange<'a> {
    Small {
        iter: vec_deque::Iter<'a, Entry>,
        high: Price,
    },
    Large(btree_map::Range<'a, Price, Handle>),
}
impl Iterator for AdaptiveRange<'_> {
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
    fn check<const P: usize, const D: usize>(
        t: &AdaptiveLocator<P, D>,
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
    fn random_against_reference<const P: usize, const D: usize>(span: u64) -> usize {
        let hs = handles(64);
        let mut t = AdaptiveLocator::<P, D>::default();
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
            match r % 5 {
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
        assert_eq!(random_against_reference::<PROMOTE, DEMOTE>(40), 0);
    }

    #[test]
    fn converting_modes_match_reference() {
        // Tiny promote threshold: the Book settles near 30 prices, so most of
        // the run is in Large mode (oscillation below covers demotion).
        assert!(random_against_reference::<16, 4>(60) >= 1);
    }

    /// Size oscillates across both thresholds with mixed working flags, so
    /// promotion and demotion must carry every flag.
    #[test]
    fn threshold_oscillation_keeps_working_flags() {
        let hs = handles(8);
        let mut t = AdaptiveLocator::<16, 4>::default();
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

    /// Pushes at both ends and removals from both ends and the middle.
    #[test]
    fn both_ends_growth_and_removal() {
        let hs = handles(4);
        let mut t = AdaptiveLocator::<PROMOTE, DEMOTE>::default();
        let mut reference: BTreeMap<Price, (Handle, bool)> = BTreeMap::new();
        let mut s = 5;
        for i in 0..600i64 {
            let p = match i % 3 {
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
