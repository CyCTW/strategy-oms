#[path = "../benches/support/locator_candidates.rs"]
mod candidates;
use candidates::*;
use std::collections::BTreeMap;

fn snapshot(t: &impl Locator, lo: i64, hi: i64) -> Vec<(i64, Value)> {
    let mut out = Vec::new();
    t.visit(lo, hi, |p, v| {
        out.push((p, v));
        true
    });
    out
}
fn check<T: Locator>() {
    let mut t = T::default();
    let mut reference = BTreeMap::new();
    let mut seed = 99u64;
    // Force threshold transitions, negative page boundaries, and extremes.
    let prices: Vec<_> = [i64::MIN, i64::MAX].into_iter().chain(-130..=130).collect();
    for step in 0..20_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let p = prices[(seed >> 32) as usize % prices.len()];
        let v = Value {
            slot: step,
            generation: seed,
        };
        let w = seed & 16 != 0;
        match seed % 5 {
            0 => {
                t.remove(p);
                reference.remove(&p);
            }
            1 => {
                if let Some((_, working)) = reference.get_mut(&p) {
                    t.set_working(p, w);
                    *working = w;
                }
            }
            _ => {
                t.insert(p, v, w);
                reference.insert(p, (v, w));
            }
        }
        assert_eq!(t.get(p), reference.get(&p).map(|(v, _)| *v));
        assert_eq!(
            t.best(true),
            reference
                .iter()
                .rev()
                .find(|(_, (_, w))| *w)
                .map(|(&p, _)| p)
        );
        assert_eq!(
            t.best(false),
            reference.iter().find(|(_, (_, w))| *w).map(|(&p, _)| p)
        );
        if step % 97 == 0 {
            for (lo, hi) in [
                (i64::MIN, i64::MAX),
                (-65, 64),
                (p, p),
                (2, 1),
                (i64::MIN, -129),
                (129, i64::MAX),
            ] {
                let expected: Vec<_> = if lo > hi {
                    vec![]
                } else {
                    reference
                        .range(lo..=hi)
                        .map(|(&p, &(v, _))| (p, v))
                        .collect()
                };
                assert_eq!(snapshot(&t, lo, hi), expected);
                let mut first = Vec::new();
                t.visit(lo, hi, |p, v| {
                    first.push((p, v));
                    first.len() < 3
                });
                assert_eq!(first, expected.into_iter().take(3).collect::<Vec<_>>());
            }
        }
    }
    for &p in &prices {
        t.remove(p);
    }
    assert_eq!(t.best(true), None);
    assert!(snapshot(&t, i64::MIN, i64::MAX).is_empty());
    for &p in &prices {
        t.insert(p, Value::default(), false);
    }
    assert_eq!(t.best(false), None);
    assert_eq!(snapshot(&t, i64::MIN, i64::MAX).len(), prices.len());
}
#[test]
fn standard() {
    check::<Standard>();
}
#[test]
fn flat() {
    check::<Flat>();
}
#[test]
fn adaptive4() {
    check::<Adaptive<4>>();
}
#[test]
fn adaptive8() {
    check::<Adaptive<8>>();
}
#[test]
fn adaptive16() {
    check::<Adaptive<16>>();
}
#[test]
fn hash_ordered() {
    check::<HashOrdered>();
}
#[test]
fn paged() {
    check::<Paged>();
}
