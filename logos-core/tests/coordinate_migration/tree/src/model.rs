/// One order line.
#[derive(Debug, Clone)]
pub struct Order {
    pub qty: u32,
    pub price: u32,
    pub customer: Customer,
}

/// Who placed it.
#[derive(Debug, Clone, Default)]
pub struct Customer {
    pub name: String,
}

impl Order {
    pub fn new(qty: u32, price: u32) -> Self {
        Self { qty, price, customer: Customer::default() }
    }

    pub fn total(&self) -> u32 {
        self.qty * self.price
    }
}

pub trait Priced {
    fn price(&self) -> u32;
}

impl Priced for Order {
    fn price(&self) -> u32 {
        self.total()
    }
}
