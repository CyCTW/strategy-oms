use crate::{
    model::*,
    pool::{Handle, Pool},
    price_deque::{AdaptiveLocator, AdaptiveRange},
    price_pages::{PageLocator, PagePool, PageRange},
    price_slide::{SlideLocator, SlideRange},
    price_tree::{PriceForest, TreeRange},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::RangeInclusive,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub confirmed_leaves: Qty,
    pub pending_new_qty: Qty,
    /// Subset of confirmed_leaves, not additional exposure.
    pub pending_cancel_leaves: Qty,
    pub pending_replace_in: Qty,
    pub pending_replace_out: Qty,
    pub uncertain_orders: u64,
}
impl Totals {
    fn add(&mut self, x: Self) {
        self.confirmed_leaves += x.confirmed_leaves;
        self.pending_new_qty += x.pending_new_qty;
        self.pending_cancel_leaves += x.pending_cancel_leaves;
        self.pending_replace_in += x.pending_replace_in;
        self.pending_replace_out += x.pending_replace_out;
        self.uncertain_orders += x.uncertain_orders;
    }
    fn sub(&mut self, x: Self) {
        self.confirmed_leaves -= x.confirmed_leaves;
        self.pending_new_qty -= x.pending_new_qty;
        self.pending_cancel_leaves -= x.pending_cancel_leaves;
        self.pending_replace_in -= x.pending_replace_in;
        self.pending_replace_out -= x.pending_replace_out;
        self.uncertain_orders -= x.uncertain_orders;
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum IndexBackend {
    /// BTreeMap/BTreeSet for every Book; still selectable explicitly.
    Standard,
    PooledAvl,
    /// Experimental sparse pages; each page spans 64 raw price units.
    PooledPages,
    /// Sorted double-ended array per Book, converting to the Standard B-tree
    /// pair past 1,024 prices and back below 256 (docs/sorted-deque.md).
    Adaptive,
    /// Default. A 128-slot circular window that slides with the near-market
    /// prices, plus a sorted array (B-tree pair when large) for prices outside
    /// it; Books with at most 8 prices use the array only
    /// (docs/sliding-window.md).
    #[default]
    SlidingWindow,
}
/// Process-local, index-specific handle. Invalid after Engine recreation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookHandle {
    owner: u64,
    slot: Handle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelSummary {
    pub totals: Totals,
    pub order_count: usize,
}
/// Read-only facade. Membership iteration order is unspecified, not FIFO or ID order.
#[derive(Clone, Copy)]
pub struct PriceLevel<'a> {
    pub totals: Totals,
    pub order_ids: OrderIds<'a>,
}
#[derive(Clone, Copy)]
pub struct OrderIds<'a> {
    pool: &'a Pool<Member>,
    next: Option<Handle>,
    remaining: usize,
}
impl OrderIds<'_> {
    pub fn len(&self) -> usize {
        self.remaining
    }
    pub fn is_empty(&self) -> bool {
        self.remaining == 0
    }
    pub fn iter(&self) -> Self {
        *self
    }
}
impl Iterator for OrderIds<'_> {
    type Item = OrderId;
    fn next(&mut self) -> Option<Self::Item> {
        let member = self.pool.get(self.next?).expect("member handle");
        self.next = member.next;
        self.remaining -= 1;
        Some(member.order_id)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for OrderIds<'_> {}
impl std::fmt::Debug for OrderIds<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}
impl std::fmt::Debug for PriceLevel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PriceLevel")
            .field("totals", &self.totals)
            .field("order_ids", &self.order_ids)
            .finish()
    }
}
#[derive(Default)]
struct Level {
    totals: Totals,
    head: Option<Handle>,
    count: usize,
}
#[derive(Clone, Copy)]
struct Member {
    order_id: OrderId,
    level: Handle,
    previous: Option<Handle>,
    next: Option<Handle>,
}
/// Stored with the order, avoiding another ID map lookup on updates.
#[derive(Default)]
pub(crate) struct Memberships {
    slots: [Option<Handle>; 2],
    book: Option<BookHandle>,
}

enum Locator {
    Standard {
        levels: BTreeMap<Price, Handle>,
        confirmed: BTreeSet<Price>,
    },
    Pooled {
        root: Option<Handle>,
    },
    Pages(PageLocator),
    Adaptive(AdaptiveLocator),
    Window(SlideLocator<u128, 128>),
}
impl Locator {
    fn new(backend: IndexBackend) -> Self {
        match backend {
            IndexBackend::Standard => Self::Standard {
                levels: BTreeMap::new(),
                confirmed: BTreeSet::new(),
            },
            IndexBackend::PooledAvl => Self::Pooled { root: None },
            IndexBackend::PooledPages => Self::Pages(PageLocator::default()),
            IndexBackend::Adaptive => Self::Adaptive(AdaptiveLocator::default()),
            IndexBackend::SlidingWindow => Self::Window(SlideLocator::default()),
        }
    }
    fn get(&self, forest: &PriceForest, pages: &PagePool, p: Price) -> Option<Handle> {
        match self {
            Self::Standard { levels, .. } => levels.get(&p).copied(),
            Self::Pooled { root } => forest.get(*root, p),
            Self::Pages(locator) => locator.get(pages, p),
            Self::Adaptive(locator) => locator.get(p),
            Self::Window(locator) => locator.get(p),
        }
    }
    fn insert(
        &mut self,
        forest: &mut PriceForest,
        pages: &mut PagePool,
        p: Price,
        h: Handle,
        working: bool,
    ) {
        match self {
            Self::Standard { levels, confirmed } => {
                levels.insert(p, h);
                if working {
                    confirmed.insert(p);
                }
            }
            Self::Pooled { root } => *root = forest.insert(*root, p, h, working),
            Self::Pages(locator) => locator.insert(pages, p, h, working),
            Self::Adaptive(locator) => locator.insert(p, h, working),
            Self::Window(locator) => locator.insert(p, h, working),
        }
    }
    /// One search where the structure allows it; returns (level, created).
    fn find_or_insert(
        &mut self,
        forest: &mut PriceForest,
        pages: &mut PagePool,
        p: Price,
        make: impl FnOnce() -> Handle,
        working: bool,
    ) -> (Handle, bool) {
        match self {
            Self::Standard { levels, confirmed } => match levels.entry(p) {
                std::collections::btree_map::Entry::Occupied(o) => (*o.get(), false),
                std::collections::btree_map::Entry::Vacant(v) => {
                    let h = make();
                    v.insert(h);
                    if working {
                        confirmed.insert(p);
                    }
                    (h, true)
                }
            },
            Self::Adaptive(locator) => locator.find_or_insert(p, make, working),
            Self::Window(locator) => locator.find_or_insert(p, make, working),
            _ => match self.get(forest, pages, p) {
                Some(h) => (h, false),
                None => {
                    let h = make();
                    self.insert(forest, pages, p, h, working);
                    (h, true)
                }
            },
        }
    }
    fn remove(&mut self, forest: &mut PriceForest, pages: &mut PagePool, p: Price) {
        match self {
            Self::Standard { levels, confirmed } => {
                levels.remove(&p);
                confirmed.remove(&p);
            }
            Self::Pooled { root } => *root = forest.remove(*root, p),
            Self::Pages(locator) => locator.remove(pages, p),
            Self::Adaptive(locator) => locator.remove(p),
            Self::Window(locator) => locator.remove(p),
        }
    }
    fn set_working(
        &mut self,
        forest: &mut PriceForest,
        pages: &mut PagePool,
        p: Price,
        working: bool,
    ) {
        match self {
            Self::Standard { confirmed, .. } => {
                if working {
                    confirmed.insert(p);
                } else {
                    confirmed.remove(&p);
                }
            }
            Self::Pooled { root } => forest.set_working(*root, p, working),
            Self::Pages(locator) => locator.set_working(pages, p, working),
            Self::Adaptive(locator) => locator.set_working(p, working),
            Self::Window(locator) => locator.set_working(p, working),
        }
    }
    fn best(&self, forest: &PriceForest, pages: &PagePool, side: Side) -> Option<Price> {
        match self {
            Self::Standard { confirmed, .. } => {
                if side == Side::Buy {
                    confirmed.last().copied()
                } else {
                    confirmed.first().copied()
                }
            }
            Self::Pooled { root } => forest.best(*root, side == Side::Buy),
            Self::Pages(locator) => locator.best(pages, side == Side::Buy),
            Self::Adaptive(locator) => locator.best(side == Side::Buy),
            Self::Window(locator) => locator.best(side == Side::Buy),
        }
    }
    fn range<'a>(
        &'a self,
        forest: &'a PriceForest,
        pages: &'a PagePool,
        range: RangeInclusive<Price>,
    ) -> PriceIter<'a> {
        if range.is_empty() {
            return PriceIter::Empty;
        }
        match self {
            Self::Standard { levels, .. } => PriceIter::Standard(levels.range(range)),
            Self::Pooled { root } => {
                PriceIter::Pooled(forest.range(*root, *range.start(), *range.end()))
            }
            Self::Pages(locator) => {
                PriceIter::Pages(locator.range(pages, *range.start(), *range.end()))
            }
            Self::Adaptive(locator) => {
                PriceIter::Adaptive(locator.range(*range.start(), *range.end()))
            }
            Self::Window(locator) => PriceIter::Window(locator.range(*range.start(), *range.end())),
        }
    }
}
enum PriceIter<'a> {
    Empty,
    Standard(std::collections::btree_map::Range<'a, Price, Handle>),
    Pooled(TreeRange<'a>),
    Pages(PageRange<'a>),
    Adaptive(AdaptiveRange<'a>),
    Window(SlideRange<'a, u128, 128>),
}
impl Iterator for PriceIter<'_> {
    type Item = (Price, Handle);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Empty => None,
            Self::Standard(i) => i.next().map(|(p, h)| (*p, *h)),
            Self::Pooled(i) => i.next(),
            Self::Pages(i) => i.next(),
            Self::Adaptive(i) => i.next(),
            Self::Window(i) => i.next(),
        }
    }
}
struct BookIndex {
    locator: Locator,
    reserved: Qty,
    uncertain: u64,
    best: Option<Price>,
}
#[derive(Debug, Clone, Copy)]
pub struct IndexStats {
    pub books: usize,
    pub live_levels: usize,
    pub live_members: usize,
    pub book_blocks: usize,
    pub level_blocks: usize,
    pub member_blocks: usize,
    pub tree_blocks: usize,
    pub page_blocks: usize,
    pub live_pages: usize,
}
pub(crate) struct Index {
    owner: u64,
    backend: IndexBackend,
    registry: HashMap<Book, BookHandle>,
    books: Pool<BookIndex>,
    levels: Pool<Level>,
    members: Pool<Member>,
    forest: PriceForest,
    pages: PagePool,
}
impl Default for Index {
    fn default() -> Self {
        Self::new(IndexBackend::default())
    }
}

fn contributions(o: &Order) -> [Option<(Price, Totals)>; 2] {
    if o.lifecycle.terminal() && o.pending.is_none() {
        return [None, None];
    }
    let mut t = Totals {
        confirmed_leaves: o.leaves,
        uncertain_orders: u64::from(o.uncertain),
        ..Totals::default()
    };
    let mut second = None;
    if let Some(p) = o.pending {
        match p.kind {
            RequestKind::New => t.pending_new_qty = p.total_qty.saturating_sub(o.cum_filled),
            RequestKind::Cancel => t.pending_cancel_leaves = o.leaves,
            RequestKind::Replace => {
                t.pending_replace_out = o.leaves;
                let qty = p.total_qty.saturating_sub(o.cum_filled);
                if p.price == o.price {
                    t.pending_replace_in = qty;
                } else {
                    second = Some((
                        p.price,
                        Totals {
                            pending_replace_in: qty,
                            uncertain_orders: u64::from(o.uncertain),
                            ..Totals::default()
                        },
                    ));
                }
            }
        }
    }
    [Some((o.price, t)), second]
}

impl Index {
    pub fn new(backend: IndexBackend) -> Self {
        static OWNER: AtomicU64 = AtomicU64::new(1);
        let owner = OWNER
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
            .expect("index identity exhausted");
        Self {
            owner,
            backend,
            registry: HashMap::new(),
            books: Pool::default(),
            levels: Pool::default(),
            members: Pool::default(),
            forest: PriceForest::default(),
            pages: PagePool::default(),
        }
    }
    pub fn stats(&self) -> IndexStats {
        IndexStats {
            books: self.books.len(),
            live_levels: self.levels.len(),
            live_members: self.members.len(),
            book_blocks: self.books.blocks(),
            level_blocks: self.levels.blocks(),
            member_blocks: self.members.blocks(),
            tree_blocks: self.forest.blocks(),
            page_blocks: self.pages.blocks(),
            live_pages: self.pages.len(),
        }
    }
    pub fn register_book(&mut self, book: Book) -> BookHandle {
        if let Some(h) = self.registry.get(&book) {
            return *h;
        }
        let slot = self.books.insert(BookIndex {
            locator: Locator::new(self.backend),
            reserved: 0,
            uncertain: 0,
            best: None,
        });
        let h = BookHandle {
            owner: self.owner,
            slot,
        };
        self.registry.insert(book, h);
        h
    }
    pub fn book_handle(&self, book: Book) -> Option<BookHandle> {
        self.registry.get(&book).copied()
    }
    fn book(&self, h: BookHandle) -> Option<&BookIndex> {
        if h.owner != self.owner {
            return None;
        }
        self.books.get(h.slot)
    }
    pub fn at_handle(&self, h: BookHandle, price: Price) -> Option<PriceLevel<'_>> {
        let level = self
            .book(h)?
            .locator
            .get(&self.forest, &self.pages, price)?;
        Some(self.view(level))
    }
    pub fn summary(&self, h: BookHandle, price: Price) -> Option<LevelSummary> {
        let level = self
            .book(h)?
            .locator
            .get(&self.forest, &self.pages, price)?;
        let l = self.levels.get(level)?;
        Some(LevelSummary {
            totals: l.totals,
            order_count: l.count,
        })
    }
    fn view(&self, h: Handle) -> PriceLevel<'_> {
        let level = self.levels.get(h).expect("level handle");
        PriceLevel {
            totals: level.totals,
            order_ids: OrderIds {
                pool: &self.members,
                next: level.head,
                remaining: level.count,
            },
        }
    }
    pub fn range(
        &self,
        h: Option<BookHandle>,
        range: RangeInclusive<Price>,
    ) -> impl Iterator<Item = (Price, PriceLevel<'_>)> {
        let iter = h.and_then(|h| self.book(h)).map_or(PriceIter::Empty, |b| {
            b.locator.range(&self.forest, &self.pages, range)
        });
        iter.map(|(p, h)| (p, self.view(h)))
    }
    pub fn best(&self, h: BookHandle) -> Option<Price> {
        self.book(h)?.best
    }
    pub fn reserved(&self, h: Option<BookHandle>) -> Qty {
        h.and_then(|h| self.book(h)).map_or(0, |b| b.reserved)
    }
    pub fn uncertain(&self, h: Option<BookHandle>) -> u64 {
        h.and_then(|h| self.book(h)).map_or(0, |b| b.uncertain)
    }

    pub fn update(&mut self, old: Option<&Order>, new: &Order, slots: &mut Memberships) {
        let book = *slots
            .book
            .get_or_insert_with(|| self.register_book(new.book));
        let before = old.map_or([None, None], contributions);
        let after = contributions(new);
        let b = self.books.get_mut(book.slot).unwrap();
        if let Some(old) = old {
            b.reserved -= old.reserved_qty();
            b.uncertain -= u64::from(old.uncertain);
        }
        b.reserved += new.reserved_qty();
        b.uncertain += u64::from(new.uncertain);
        let mut next = [None, None];
        for (i, old_contribution) in before.into_iter().enumerate() {
            let Some((price, totals)) = old_contribution else {
                continue;
            };
            let member = slots.slots[i].expect("old membership");
            let m = *self.members.get(member).unwrap();
            let level = self.levels.get_mut(m.level).unwrap();
            let was_working = level.totals.confirmed_leaves > 0;
            level.totals.sub(totals);
            if let Some(j) = after
                .iter()
                .position(|entry| entry.is_some_and(|(p, _)| p == price))
            {
                level.totals.add(after[j].unwrap().1);
                let working = level.totals.confirmed_leaves > 0;
                if was_working != working {
                    self.working_changed(book, new.book.side, price, working);
                }
                next[j] = Some(member);
            } else {
                level.count -= 1;
                if let Some(prev) = m.previous {
                    self.members.get_mut(prev).unwrap().next = m.next;
                } else {
                    level.head = m.next;
                }
                if let Some(next) = m.next {
                    self.members.get_mut(next).unwrap().previous = m.previous;
                }
                self.members.remove(member).unwrap();
                let empty = level.count == 0;
                let working = level.totals.confirmed_leaves > 0;
                if empty {
                    self.books.get_mut(book.slot).unwrap().locator.remove(
                        &mut self.forest,
                        &mut self.pages,
                        price,
                    );
                    self.levels.remove(m.level).unwrap();
                    if was_working {
                        self.refresh_best(book, new.book.side);
                    }
                } else if was_working != working {
                    self.working_changed(book, new.book.side, price, working);
                }
            }
        }
        for (i, contribution) in after.into_iter().enumerate() {
            let Some((price, totals)) = contribution else {
                continue;
            };
            if next[i].is_some() {
                continue;
            }
            // A new level holds only this contribution, so its working state
            // is known before inserting: one locator search in total.
            let levels = &mut self.levels;
            let (level, created) = self
                .books
                .get_mut(book.slot)
                .unwrap()
                .locator
                .find_or_insert(
                    &mut self.forest,
                    &mut self.pages,
                    price,
                    || levels.insert(Level::default()),
                    totals.confirmed_leaves > 0,
                );
            let l = self.levels.get_mut(level).unwrap();
            let was_working = l.totals.confirmed_leaves > 0;
            let member = self.members.insert(Member {
                order_id: new.id,
                level,
                previous: None,
                next: l.head,
            });
            if let Some(head) = l.head {
                self.members.get_mut(head).unwrap().previous = Some(member);
            }
            l.head = Some(member);
            l.count += 1;
            l.totals.add(totals);
            let working = l.totals.confirmed_leaves > 0;
            if created {
                // Flag already stored by find_or_insert; only the best cache.
                if working {
                    self.note_working(book, new.book.side, price);
                }
            } else if was_working != working {
                self.working_changed(book, new.book.side, price, working);
            }
            next[i] = Some(member);
        }
        slots.slots = next;
    }
    fn working_changed(&mut self, book: BookHandle, side: Side, price: Price, working: bool) {
        let b = self.books.get_mut(book.slot).unwrap();
        b.locator
            .set_working(&mut self.forest, &mut self.pages, price, working);
        if working {
            if b.best.is_none_or(|p| {
                if side == Side::Buy {
                    price > p
                } else {
                    price < p
                }
            }) {
                b.best = Some(price);
            }
        } else if b.best == Some(price) {
            b.best = b.locator.best(&self.forest, &self.pages, side);
        }
    }
    fn note_working(&mut self, book: BookHandle, side: Side, price: Price) {
        let b = self.books.get_mut(book.slot).unwrap();
        if b.best.is_none_or(|p| {
            if side == Side::Buy {
                price > p
            } else {
                price < p
            }
        }) {
            b.best = Some(price);
        }
    }
    fn refresh_best(&mut self, book: BookHandle, side: Side) {
        let b = self.books.get_mut(book.slot).unwrap();
        b.best = b.locator.best(&self.forest, &self.pages, side);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(dead_code)]
    mod legacy {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/benches/support/legacy_index.rs"
        ));
    }

    fn sample(id: usize) -> Order {
        Order {
            id: id as u64 + 1,
            book: Book {
                strategy: (id % 4) as u64,
                account: 1,
                venue: 1,
                instrument: 7,
                side: if id.is_multiple_of(2) {
                    Side::Buy
                } else {
                    Side::Sell
                },
            },
            exchange_id: Some(id as u64),
            price: 100,
            total_qty: 100,
            cum_filled: 0,
            leaves: 100,
            lifecycle: Lifecycle::Working,
            pending: None,
            uncertain: false,
            version: 1,
        }
    }
    fn fields(t: Totals) -> [u64; 6] {
        [
            t.confirmed_leaves,
            t.pending_new_qty,
            t.pending_cancel_leaves,
            t.pending_replace_in,
            t.pending_replace_out,
            t.uncertain_orders,
        ]
    }
    fn old_fields(t: legacy::Totals) -> [u64; 6] {
        [
            t.confirmed_leaves,
            t.pending_new_qty,
            t.pending_cancel_leaves,
            t.pending_replace_in,
            t.pending_replace_out,
            t.uncertain_orders,
        ]
    }
    #[test]
    fn randomized_membership_deltas_match_legacy_for_all_backends() {
        for backend in [
            IndexBackend::Standard,
            IndexBackend::PooledAvl,
            IndexBackend::PooledPages,
            IndexBackend::Adaptive,
            IndexBackend::SlidingWindow,
        ] {
            let mut index = Index::new(backend);
            let mut reference = legacy::Index::default();
            let mut orders = vec![None; 128];
            let mut slots: Vec<_> = (0..128).map(|_| Memberships::default()).collect();
            let mut seed = 11u64;
            for step in 0..10_000 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let id = (seed >> 32) as usize % orders.len();
                let mut n = sample(id);
                n.price = match step % 31 {
                    0 => i64::MIN,
                    1 => i64::MAX,
                    _ => ((seed >> 16) % 17) as i64 - 8,
                };
                n.cum_filled = seed % 50;
                n.leaves -= n.cum_filled;
                n.uncertain = step % 7 == 0;
                match step % 8 {
                    0 => {
                        n.lifecycle = Lifecycle::Canceled;
                        n.leaves = 0;
                    }
                    1 => {
                        n.lifecycle = Lifecycle::PendingNew;
                        n.leaves = 0;
                        n.pending = Some(Request {
                            id: step + 1,
                            order_id: n.id,
                            kind: RequestKind::New,
                            price: n.price,
                            total_qty: 100,
                            state: RequestState::Pending,
                        });
                    }
                    2..=5 => {
                        n.pending = Some(Request {
                            id: step + 1,
                            order_id: n.id,
                            kind: RequestKind::Replace,
                            price: if step % 2 == 0 {
                                n.price
                            } else {
                                n.price.saturating_add(1)
                            },
                            total_qty: 80 + seed % 80,
                            state: RequestState::Pending,
                        });
                    }
                    6 => {
                        n.pending = Some(Request {
                            id: step + 1,
                            order_id: n.id,
                            kind: RequestKind::Cancel,
                            price: n.price,
                            total_qty: n.total_qty,
                            state: RequestState::Pending,
                        });
                    }
                    _ => {}
                }
                index.update(orders[id].as_ref(), &n, &mut slots[id]);
                reference.update(orders[id].as_ref(), &n);
                orders[id] = Some(n);
                if step % 17 == 0 {
                    for (&book, b) in &reference.books {
                        let h = index.book_handle(book).unwrap();
                        assert_eq!(index.reserved(Some(h)), b.reserved);
                        assert_eq!(index.uncertain(Some(h)), b.uncertain_orders);
                        let best = if book.side == Side::Buy {
                            b.confirmed_prices.last()
                        } else {
                            b.confirmed_prices.first()
                        };
                        assert_eq!(index.best(h), best.copied());
                        let prices: Vec<_> = index.range(Some(h), i64::MIN..=i64::MAX).collect();
                        assert_eq!(prices.len(), b.levels.len());
                        for (p, view) in prices {
                            let expected = &b.levels[&p];
                            assert_eq!(fields(view.totals), old_fields(expected.totals));
                            let ids: BTreeSet<_> = view.order_ids.iter().collect();
                            assert_eq!(ids, expected.order_ids);
                            assert_eq!(view.order_ids.len(), ids.len());
                            let summary = index.summary(h, p).unwrap();
                            assert_eq!(summary.order_count, ids.len());
                            assert_eq!(summary.totals, view.totals);
                        }
                    }
                }
            }
            for (i, old) in orders.into_iter().enumerate() {
                if let Some(old) = old {
                    let mut n = old;
                    n.lifecycle = Lifecycle::Canceled;
                    n.leaves = 0;
                    n.pending = None;
                    n.uncertain = false;
                    index.update(Some(&old), &n, &mut slots[i]);
                }
            }
            assert_eq!(index.stats().live_levels, 0);
            assert_eq!(index.stats().live_members, 0);
        }
    }
}
