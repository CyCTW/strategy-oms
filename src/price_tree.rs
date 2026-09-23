//! Pool-backed AVL candidate. Parent links provide allocation-free in-order
//! iteration; subtree flags find confirmed prices without scanning pending ones.
use crate::{
    model::Price,
    pool::{Handle, Pool},
};

#[derive(Clone, Copy)]
struct Node {
    price: Price,
    value: Handle,
    left: Option<Handle>,
    right: Option<Handle>,
    parent: Option<Handle>,
    height: i32,
    working: bool,
    has_working: bool,
}

#[derive(Default)]
pub(crate) struct PriceForest {
    nodes: Pool<Node>,
}

impl PriceForest {
    fn node(&self, h: Handle) -> Node {
        *self.nodes.get(h).expect("tree handle")
    }
    fn height(&self, h: Option<Handle>) -> i32 {
        h.map_or(0, |h| self.node(h).height)
    }
    fn any(&self, h: Option<Handle>) -> bool {
        h.is_some_and(|h| self.node(h).has_working)
    }
    pub fn blocks(&self) -> usize {
        self.nodes.blocks()
    }

    fn fix(&mut self, h: Handle) {
        let n = self.node(h);
        let height = 1 + self.height(n.left).max(self.height(n.right));
        let any = n.working || self.any(n.left) || self.any(n.right);
        let out = self.nodes.get_mut(h).unwrap();
        out.height = height;
        out.has_working = any;
        for child in [n.left, n.right].into_iter().flatten() {
            self.nodes.get_mut(child).unwrap().parent = Some(h);
        }
    }

    fn rotate_left(&mut self, h: Handle) -> Handle {
        let n = self.node(h);
        let right = n.right.unwrap();
        self.nodes.get_mut(h).unwrap().right = self.node(right).left;
        self.nodes.get_mut(right).unwrap().left = Some(h);
        self.nodes.get_mut(right).unwrap().parent = n.parent;
        self.fix(h);
        self.fix(right);
        right
    }

    fn rotate_right(&mut self, h: Handle) -> Handle {
        let n = self.node(h);
        let left = n.left.unwrap();
        self.nodes.get_mut(h).unwrap().left = self.node(left).right;
        self.nodes.get_mut(left).unwrap().right = Some(h);
        self.nodes.get_mut(left).unwrap().parent = n.parent;
        self.fix(h);
        self.fix(left);
        left
    }

    fn balance(&mut self, h: Handle) -> Handle {
        self.fix(h);
        let n = self.node(h);
        let difference = self.height(n.left) - self.height(n.right);
        if difference > 1 {
            let left = self.node(n.left.unwrap());
            if self.height(left.left) < self.height(left.right) {
                let child = self.rotate_left(n.left.unwrap());
                self.nodes.get_mut(h).unwrap().left = Some(child);
            }
            return self.rotate_right(h);
        }
        if difference < -1 {
            let right = self.node(n.right.unwrap());
            if self.height(right.right) < self.height(right.left) {
                let child = self.rotate_right(n.right.unwrap());
                self.nodes.get_mut(h).unwrap().right = Some(child);
            }
            return self.rotate_left(h);
        }
        h
    }

    pub fn get(&self, mut root: Option<Handle>, price: Price) -> Option<Handle> {
        while let Some(h) = root {
            let n = self.node(h);
            if price == n.price {
                return Some(n.value);
            }
            root = if price < n.price { n.left } else { n.right };
        }
        None
    }

    fn insert_inner(
        &mut self,
        root: Option<Handle>,
        price: Price,
        value: Handle,
        working: bool,
    ) -> Handle {
        let Some(h) = root else {
            return self.nodes.insert(Node {
                price,
                value,
                left: None,
                right: None,
                parent: None,
                height: 1,
                working,
                has_working: working,
            });
        };
        let n = self.node(h);
        match price.cmp(&n.price) {
            std::cmp::Ordering::Less => {
                let child = self.insert_inner(n.left, price, value, working);
                self.nodes.get_mut(h).unwrap().left = Some(child);
            }
            std::cmp::Ordering::Greater => {
                let child = self.insert_inner(n.right, price, value, working);
                self.nodes.get_mut(h).unwrap().right = Some(child);
            }
            std::cmp::Ordering::Equal => {
                let node = self.nodes.get_mut(h).unwrap();
                node.value = value;
                node.working = working;
            }
        }
        self.balance(h)
    }

    pub fn insert(
        &mut self,
        root: Option<Handle>,
        price: Price,
        value: Handle,
        working: bool,
    ) -> Option<Handle> {
        let h = self.insert_inner(root, price, value, working);
        self.nodes.get_mut(h).unwrap().parent = None;
        Some(h)
    }

    fn minimum(&self, mut h: Handle) -> Handle {
        while let Some(left) = self.node(h).left {
            h = left;
        }
        h
    }

    fn remove_inner(&mut self, root: Option<Handle>, price: Price) -> Option<Handle> {
        let h = root?;
        let n = self.node(h);
        match price.cmp(&n.price) {
            std::cmp::Ordering::Less => {
                let child = self.remove_inner(n.left, price);
                self.nodes.get_mut(h).unwrap().left = child;
            }
            std::cmp::Ordering::Greater => {
                let child = self.remove_inner(n.right, price);
                self.nodes.get_mut(h).unwrap().right = child;
            }
            std::cmp::Ordering::Equal => {
                if n.left.is_none() || n.right.is_none() {
                    let child = n.left.or(n.right);
                    if let Some(c) = child {
                        self.nodes.get_mut(c).unwrap().parent = n.parent;
                    }
                    self.nodes.remove(h).unwrap();
                    return child;
                }
                let successor = self.node(self.minimum(n.right.unwrap()));
                let right = self.remove_inner(n.right, successor.price);
                let node = self.nodes.get_mut(h).unwrap();
                node.price = successor.price;
                node.value = successor.value;
                node.working = successor.working;
                node.right = right;
            }
        }
        Some(self.balance(h))
    }

    pub fn remove(&mut self, root: Option<Handle>, price: Price) -> Option<Handle> {
        let root = self.remove_inner(root, price);
        if let Some(h) = root {
            self.nodes.get_mut(h).unwrap().parent = None;
        }
        root
    }

    pub fn set_working(&mut self, mut root: Option<Handle>, price: Price, working: bool) {
        while let Some(h) = root {
            let n = self.node(h);
            if n.price == price {
                self.nodes.get_mut(h).unwrap().working = working;
                let mut current = Some(h);
                while let Some(h) = current {
                    self.fix(h);
                    current = self.node(h).parent;
                }
                return;
            }
            root = if price < n.price { n.left } else { n.right };
        }
        panic!("missing price for working flag");
    }

    pub fn best(&self, mut root: Option<Handle>, highest: bool) -> Option<Price> {
        while let Some(h) = root {
            let n = self.node(h);
            if !n.has_working {
                return None;
            }
            let (first, second) = if highest {
                (n.right, n.left)
            } else {
                (n.left, n.right)
            };
            if self.any(first) {
                root = first;
            } else if n.working {
                return Some(n.price);
            } else {
                root = second;
            }
        }
        None
    }

    pub fn range(&self, mut root: Option<Handle>, low: Price, high: Price) -> TreeRange<'_> {
        let mut first = None;
        if low <= high {
            while let Some(h) = root {
                let n = self.node(h);
                if n.price >= low {
                    first = Some(h);
                    root = n.left;
                } else {
                    root = n.right;
                }
            }
        }
        TreeRange {
            forest: self,
            next: first,
            high,
        }
    }

    fn successor(&self, h: Handle) -> Option<Handle> {
        let n = self.node(h);
        if let Some(right) = n.right {
            return Some(self.minimum(right));
        }
        let mut current = h;
        let mut parent = n.parent;
        while let Some(p) = parent {
            let n = self.node(p);
            if n.left == Some(current) {
                return Some(p);
            }
            current = p;
            parent = n.parent;
        }
        None
    }
}

pub(crate) struct TreeRange<'a> {
    forest: &'a PriceForest,
    next: Option<Handle>,
    high: Price,
}
impl Iterator for TreeRange<'_> {
    type Item = (Price, Handle);
    fn next(&mut self) -> Option<Self::Item> {
        let h = self.next?;
        let n = self.forest.node(h);
        if n.price > self.high {
            self.next = None;
            return None;
        }
        self.next = self.forest.successor(h);
        Some((n.price, n.value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn validate(
        f: &PriceForest,
        root: Option<Handle>,
        parent: Option<Handle>,
        low: Option<Price>,
        high: Option<Price>,
    ) -> (i32, bool) {
        let Some(h) = root else {
            return (0, false);
        };
        let n = f.node(h);
        assert_eq!(n.parent, parent);
        assert!(low.is_none_or(|p| n.price > p));
        assert!(high.is_none_or(|p| n.price < p));
        let (lh, lw) = validate(f, n.left, Some(h), low, Some(n.price));
        let (rh, rw) = validate(f, n.right, Some(h), Some(n.price), high);
        assert!((lh - rh).abs() <= 1);
        assert_eq!(n.height, 1 + lh.max(rh));
        assert_eq!(n.has_working, n.working || lw || rw);
        (n.height, n.has_working)
    }

    #[test]
    fn rotations_deletions_ranges_and_working_augmentation_match_btree() {
        let mut f = PriceForest::default();
        let mut root = None;
        let mut values = Pool::default();
        let value = values.insert(());
        let mut reference = BTreeMap::new();
        let mut seed = 31u64;
        for i in 0..20_000 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let price = ((seed >> 32) % 2000) as i64 - 1000;
            match seed % 4 {
                0 => {
                    root = f.remove(root, price);
                    reference.remove(&price);
                }
                1 if reference.contains_key(&price) => {
                    let w = i % 3 == 0;
                    f.set_working(root, price, w);
                    reference.insert(price, w);
                }
                _ => {
                    let w = i % 3 == 0;
                    root = f.insert(root, price, value, w);
                    reference.insert(price, w);
                }
            }
            if i % 31 == 0 {
                validate(&f, root, None, None, None);
                assert_eq!(
                    f.range(root, i64::MIN, i64::MAX)
                        .map(|(p, _)| p)
                        .collect::<Vec<_>>(),
                    reference.keys().copied().collect::<Vec<_>>()
                );
                assert_eq!(
                    f.best(root, false),
                    reference.iter().find(|(_, w)| **w).map(|(p, _)| *p)
                );
                assert_eq!(
                    f.best(root, true),
                    reference.iter().rev().find(|(_, w)| **w).map(|(p, _)| *p)
                );
                assert_eq!(
                    f.range(root, -100, 100).map(|(p, _)| p).collect::<Vec<_>>(),
                    reference
                        .range(-100..=100)
                        .map(|(p, _)| *p)
                        .collect::<Vec<_>>()
                );
            }
            assert_eq!(f.get(root, price).is_some(), reference.contains_key(&price));
        }
        for &p in reference.keys() {
            root = f.remove(root, p);
            validate(&f, root, None, None, None);
        }
        assert!(root.is_none());
        assert_eq!(f.nodes.len(), 0);
        for p in [i64::MIN, 0, i64::MAX] {
            root = f.insert(root, p, value, true);
        }
        assert_eq!(f.range(root, i64::MIN, i64::MAX).count(), 3);
        assert_eq!(f.range(root, 1, -1).count(), 0);
    }
}
