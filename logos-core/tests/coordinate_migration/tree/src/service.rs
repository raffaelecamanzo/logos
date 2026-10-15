use crate::model::{Order, Priced};

/// Places orders.
pub struct OrderService {
    placed: Vec<Order>,
}

impl OrderService {
    pub fn new() -> Self {
        Self { placed: Vec::new() }
    }

    pub fn place(&self, order: Order) -> u32 {
        let price = order.price();
        log::info!("placed {price}");
        validate(&order);
        price
    }
}

fn validate(order: &Order) -> bool {
    order.total() > 0 && external::check(order.qty)
}
