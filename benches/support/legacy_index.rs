use crate::model::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub confirmed_leaves: Qty,
    pub pending_new_qty: Qty,
    /// Subset of confirmed_leaves, not an additional exposure.
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

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PriceLevel {
    pub totals: Totals,
    pub order_ids: BTreeSet<OrderId>,
}

#[derive(Default)]
pub(crate) struct BookIndex {
    pub levels: BTreeMap<Price, PriceLevel>,
    pub confirmed_prices: BTreeSet<Price>,
    pub reserved: Qty,
    pub uncertain_orders: u64,
}

#[derive(Default)]
pub(crate) struct Index {
    pub books: HashMap<Book, BookIndex>,
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
    pub fn update(&mut self, old: Option<&Order>, new: &Order) {
        let b = self.books.entry(new.book).or_default();
        if let Some(o) = old {
            b.reserved -= o.reserved_qty();
            b.uncertain_orders -= u64::from(o.uncertain);
            for (price, totals) in contributions(o).into_iter().flatten() {
                let l = b.levels.get_mut(&price).expect("index membership");
                l.totals.sub(totals);
                l.order_ids.remove(&o.id);
                if l.totals.confirmed_leaves == 0 {
                    b.confirmed_prices.remove(&price);
                }
                if l.order_ids.is_empty() {
                    b.levels.remove(&price);
                }
            }
        }
        b.reserved += new.reserved_qty();
        b.uncertain_orders += u64::from(new.uncertain);
        for (price, totals) in contributions(new).into_iter().flatten() {
            let l = b.levels.entry(price).or_default();
            l.totals.add(totals);
            l.order_ids.insert(new.id);
            if l.totals.confirmed_leaves > 0 {
                b.confirmed_prices.insert(price);
            }
        }
    }
}
