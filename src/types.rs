use serde::Deserialize;

pub type Price = u64;
pub type Quantity = u64;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Bid,
    Ask,
}

#[derive(Debug, Deserialize)]
pub struct MarketDataMessage<'a> {
    #[serde(borrow)]
    pub symbol: &'a str,
    pub side: Side,
    pub price: Price,
    pub qty: Quantity,
    pub ts_nanos: u64,
}
