use crate::{
    index::Memberships,
    model::*,
    pool::{Handle, Pool},
};
use std::{collections::HashMap, ops::Index};
pub(crate) struct Entry {
    pub order: Order,
    pub memberships: Memberships,
}
pub(crate) struct OrderStore {
    ids: HashMap<OrderId, Handle>,
    pool: Pool<Entry>,
}
impl OrderStore {
    pub fn new() -> Self {
        Self {
            ids: HashMap::new(),
            pool: Pool::default(),
        }
    }
    pub fn len(&self) -> usize {
        self.pool.len()
    }
    pub fn contains_key(&self, id: &OrderId) -> bool {
        self.ids.contains_key(id)
    }
    pub fn get(&self, id: &OrderId) -> Option<&Order> {
        Some(&self.pool.get(*self.ids.get(id)?)?.order)
    }
    pub fn insert(&mut self, id: OrderId, order: Order) -> &mut Entry {
        let h = *self.ids.entry(id).or_insert_with(|| {
            self.pool.insert(Entry {
                order,
                memberships: Memberships::default(),
            })
        });
        let e = self.pool.get_mut(h).unwrap();
        e.order = order;
        e
    }
    pub fn values(&self) -> impl Iterator<Item = &Order> {
        self.pool.values().map(|e| &e.order)
    }
}
impl Index<&OrderId> for OrderStore {
    type Output = Order;
    fn index(&self, id: &OrderId) -> &Order {
        self.get(id).expect("known order")
    }
}
