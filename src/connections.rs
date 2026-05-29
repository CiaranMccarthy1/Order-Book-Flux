#![cfg(feature = "std")]

use serde::de::IgnoredAny;
use serde::Deserialize;
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::HeaderValue;
use tungstenite::{connect, Message};
use url::Url;

#[cfg(feature = "simd-json")]
use simd_json;

use crate::engine::OfiEngine;
use crate::types::Side;

const DEFAULT_WS_URL: &str = "wss://fstream.binance.com/public/ws/btcusdt@depth";
const DEFAULT_REST_URL: &str = "https://api.binance.com";
const DEFAULT_ORIGIN: &str = "";

#[derive(Debug, Clone)]
pub struct ExchangeConfig {
    pub symbol: String,
    pub price_scale: u32,
    pub size_scale: u32,
    pub url: String,
    pub rest_url: String,
    pub stream: String,
    pub origin: String,
}

impl ExchangeConfig {
    pub fn new(symbol: impl Into<String>) -> Self {
        let symbol = symbol.into().to_uppercase();
        let stream = format!("{}@depth", symbol.to_lowercase());
        Self {
            symbol,
            price_scale: 2,
            size_scale: 8,
            url: format!("wss://fstream.binance.com/public/ws/{}", stream),
            rest_url: DEFAULT_REST_URL.to_string(),
            stream,
            origin: DEFAULT_ORIGIN.to_string(),
        }
    }
}

impl Default for ExchangeConfig {
    fn default() -> Self {
        let mut config = Self::new("BTCUSDT");
        config.url = DEFAULT_WS_URL.to_string();
        config
    }
}

#[derive(Debug)]
pub enum StreamError {
    Url(url::ParseError),
    WebSocket(tungstenite::Error),
    Json(serde_json::Error),
    ParseDecimal(ParseDecimalError),
    Http(reqwest::Error),
    InvalidHeader(tungstenite::http::header::InvalidHeaderValue),
    OutOfSync {
        last_update_id: u64,
        first_update_id: u64,
        final_update_id: u64,
    },
    Binance(String),
    HandlerStopped,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Url(err) => write!(f, "url parse error: {}", err),
            StreamError::WebSocket(err) => write!(f, "websocket error: {}", err),
            StreamError::Json(err) => write!(f, "json error: {}", err),
            StreamError::ParseDecimal(err) => write!(f, "decimal parse error: {}", err),
            StreamError::Http(err) => write!(f, "http error: {}", err),
            StreamError::InvalidHeader(err) => write!(f, "header error: {}", err),
            StreamError::OutOfSync {
                last_update_id,
                first_update_id,
                final_update_id,
            } => write!(
                f,
                "out of sync: last={} first={} final={}",
                last_update_id, first_update_id, final_update_id
            ),
            StreamError::Binance(message) => write!(f, "binance error: {}", message),
            StreamError::HandlerStopped => write!(f, "handler requested stop"),
        }
    }
}

impl std::error::Error for StreamError {}

impl From<url::ParseError> for StreamError {
    fn from(err: url::ParseError) -> Self {
        StreamError::Url(err)
    }
}

impl From<tungstenite::Error> for StreamError {
    fn from(err: tungstenite::Error) -> Self {
        StreamError::WebSocket(err)
    }
}

impl From<serde_json::Error> for StreamError {
    fn from(err: serde_json::Error) -> Self {
        StreamError::Json(err)
    }
}

impl From<ParseDecimalError> for StreamError {
    fn from(err: ParseDecimalError) -> Self {
        StreamError::ParseDecimal(err)
    }
}

impl From<reqwest::Error> for StreamError {
    fn from(err: reqwest::Error) -> Self {
        StreamError::Http(err)
    }
}

impl From<tungstenite::http::header::InvalidHeaderValue> for StreamError {
    fn from(err: tungstenite::http::header::InvalidHeaderValue) -> Self {
        StreamError::InvalidHeader(err)
    }
}

#[derive(Debug)]
pub enum ParseDecimalError {
    InvalidFormat,
    Overflow,
}

impl std::fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseDecimalError::InvalidFormat => write!(f, "invalid format"),
            ParseDecimalError::Overflow => write!(f, "overflow"),
        }
    }
}

impl std::error::Error for ParseDecimalError {}

#[derive(Debug, Deserialize)]
struct BinanceSnapshot {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
struct BinanceDepthUpdate {
    #[serde(rename = "e")]
    _event_type: String,
    #[serde(rename = "E")]
    _event_time: u64,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "U")]
    first_update_id: u64,
    #[serde(rename = "u")]
    final_update_id: u64,
    #[serde(rename = "b")]
    bids: Vec<[String; 2]>,
    #[serde(rename = "a")]
    asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DepthUpdateMessage {
    Update(BinanceDepthUpdate),
    Wrapped { data: BinanceDepthUpdate },
    Error { code: i64, msg: String },
    Ack { result: Option<IgnoredAny>, id: Option<u64> },
    Other(IgnoredAny),
}

/// Connects to Binance depth stream, seeds a REST snapshot, then applies diffs into the engine.
pub fn stream_binance_depth(
    engine: &mut OfiEngine,
    config: ExchangeConfig,
) -> Result<(), StreamError> {
    stream_binance_depth_with_handler(config, |side, price, qty| {
        engine.process_level_update(side, price, qty);
    })
}

/// Seeds a Binance REST snapshot, then streams depth updates into a handler.
pub fn stream_binance_depth_with_handler<F>(
    config: ExchangeConfig,
    mut on_update: F,
) -> Result<(), StreamError>
where
    F: FnMut(Side, u64, u64),
{
    let mut handler = |side, price, qty| {
        on_update(side, price, qty);
        true
    };
    let mut last_update_id = seed_binance_snapshot(&config, &mut handler)?;
    stream_binance_depth_until(config, &mut last_update_id, handler)
}

/// Fetches the Binance REST snapshot and applies it to the handler.
pub fn seed_binance_snapshot<F>(
    config: &ExchangeConfig,
    on_update: &mut F,
) -> Result<u64, StreamError>
where
    F: FnMut(Side, u64, u64) -> bool,
{
    let snapshot = fetch_binance_snapshot(config)?;
    if !apply_binance_snapshot(&snapshot, config, on_update)? {
        return Err(StreamError::HandlerStopped);
    }
    Ok(snapshot.last_update_id)
}

/// Streams Binance depth diffs until the handler returns false.
pub fn stream_binance_depth_until<F>(
    config: ExchangeConfig,
    last_update_id: &mut u64,
    mut on_update: F,
) -> Result<(), StreamError>
where
    F: FnMut(Side, u64, u64) -> bool,
{
    let url = Url::parse(&config.url)?;
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("User-Agent", HeaderValue::from_static("order-book-flux"));
    if !config.origin.is_empty() {
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_str(&config.origin)?);
    }
    let (mut socket, _response) = connect(request)?;

    if needs_subscribe(&config) {
        let subscribe = serde_json::json!({
            "method": "SUBSCRIBE",
            "params": [config.stream.clone()],
            "id": 1
        });
        socket.send(Message::Text(subscribe.to_string()))?;
    }

    loop {
        match socket.read_message()? {
            Message::Text(text) => {
                if let Some(update) = parse_depth_update(text.as_bytes())? {
                    if !apply_depth_update(&config, update, last_update_id, &mut on_update)? {
                        break;
                    }
                }
            }
            Message::Binary(bytes) => {
                if let Some(update) = parse_depth_update(&bytes)? {
                    if !apply_depth_update(&config, update, last_update_id, &mut on_update)? {
                        break;
                    }
                }
            }
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload))?;
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
            _ => {}
        }
    }

    Ok(())
}

/// Applies a Binance diff payload to a handler and updates the last update id.
pub fn apply_binance_payload<F>(
    config: &ExchangeConfig,
    payload: &[u8],
    last_update_id: &mut u64,
    on_update: &mut F,
) -> Result<(), StreamError>
where
    F: FnMut(Side, u64, u64) -> bool,
{
    if let Some(update) = parse_depth_update(payload)? {
        apply_depth_update(config, update, last_update_id, on_update)?;
    }
    Ok(())
}

fn needs_subscribe(config: &ExchangeConfig) -> bool {
    !config.url.contains('@')
}

fn fetch_binance_snapshot(config: &ExchangeConfig) -> Result<BinanceSnapshot, StreamError> {
    let base = config.rest_url.trim_end_matches('/');
    let url = format!("{}/api/v3/depth?symbol={}&limit=1000", base, config.symbol);
    let client = reqwest::blocking::Client::builder()
        .user_agent("order-book-flux")
        .build()?;
    let response = client.get(url).send()?.error_for_status()?;
    let snapshot = response.json::<BinanceSnapshot>()?;
    Ok(snapshot)
}

fn parse_binance_snapshot(payload: &[u8]) -> Result<BinanceSnapshot, StreamError> {
    Ok(serde_json::from_slice(payload)?)
}

fn parse_depth_update(payload: &[u8]) -> Result<Option<BinanceDepthUpdate>, StreamError> {
    #[cfg(feature = "simd-json")]
    let message: DepthUpdateMessage = {
        let mut buf = payload.to_vec();
        simd_json::from_slice(&mut buf).map_err(|err| {
            StreamError::Json(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                err.to_string(),
            )))
        })?
    };

    #[cfg(not(feature = "simd-json"))]
    let message: DepthUpdateMessage = serde_json::from_slice(payload)?;

    match message {
        DepthUpdateMessage::Update(update) => Ok(Some(update)),
        DepthUpdateMessage::Wrapped { data } => Ok(Some(data)),
        DepthUpdateMessage::Error { code, msg } => Err(StreamError::Binance(format!(
            "{}: {}",
            code, msg
        ))),
        DepthUpdateMessage::Ack { .. } | DepthUpdateMessage::Other(_) => Ok(None),
    }
}

fn apply_binance_snapshot<F>(
    snapshot: &BinanceSnapshot,
    config: &ExchangeConfig,
    on_update: &mut F,
) -> Result<bool, StreamError>
where
    F: FnMut(Side, u64, u64) -> bool,
{
    for level in &snapshot.bids {
        let price = parse_decimal_to_u64(&level[0], config.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], config.size_scale)?;
        if !on_update(Side::Bid, price, qty) {
            return Ok(false);
        }
    }

    for level in &snapshot.asks {
        let price = parse_decimal_to_u64(&level[0], config.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], config.size_scale)?;
        if !on_update(Side::Ask, price, qty) {
            return Ok(false);
        }
    }

    Ok(true)
}

fn apply_depth_update<F>(
    config: &ExchangeConfig,
    update: BinanceDepthUpdate,
    last_update_id: &mut u64,
    on_update: &mut F,
) -> Result<bool, StreamError>
where
    F: FnMut(Side, u64, u64) -> bool,
{
    if update.symbol != config.symbol {
        return Ok(true);
    }

    if update.final_update_id <= *last_update_id {
        return Ok(true);
    }

    if update.first_update_id > last_update_id.saturating_add(1) {
        return Err(StreamError::OutOfSync {
            last_update_id: *last_update_id,
            first_update_id: update.first_update_id,
            final_update_id: update.final_update_id,
        });
    }

    for level in update.bids {
        let price = parse_decimal_to_u64(&level[0], config.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], config.size_scale)?;
        if !on_update(Side::Bid, price, qty) {
            return Ok(false);
        }
    }

    for level in update.asks {
        let price = parse_decimal_to_u64(&level[0], config.price_scale)?;
        let qty = parse_decimal_to_u64(&level[1], config.size_scale)?;
        if !on_update(Side::Ask, price, qty) {
            return Ok(false);
        }
    }

    *last_update_id = update.final_update_id;
    Ok(true)
}

fn parse_decimal_to_u64(input: &str, scale: u32) -> Result<u64, ParseDecimalError> {
    let mut int_part = 0u64;
    let mut frac_part = 0u64;
    let mut frac_digits = 0u32;
    let mut seen_dot = false;
    let mut saw_digit = false;

    for byte in input.bytes() {
        match byte {
            b'0'..=b'9' => {
                let digit = (byte - b'0') as u64;
                saw_digit = true;

                if seen_dot {
                    if frac_digits < scale {
                        frac_part = frac_part
                            .checked_mul(10)
                            .and_then(|v| v.checked_add(digit))
                            .ok_or(ParseDecimalError::Overflow)?;
                        frac_digits += 1;
                    }
                } else {
                    int_part = int_part
                        .checked_mul(10)
                        .and_then(|v| v.checked_add(digit))
                        .ok_or(ParseDecimalError::Overflow)?;
                }
            }
            b'.' => {
                if seen_dot {
                    return Err(ParseDecimalError::InvalidFormat);
                }
                seen_dot = true;
            }
            _ => return Err(ParseDecimalError::InvalidFormat),
        }
    }

    if !saw_digit {
        return Err(ParseDecimalError::InvalidFormat);
    }

    if frac_digits < scale {
        let padding = scale - frac_digits;
        let pad = pow10(padding)?;
        frac_part = frac_part.checked_mul(pad).ok_or(ParseDecimalError::Overflow)?;
    }

    let scale_factor = pow10(scale)?;
    let scaled_int = int_part
        .checked_mul(scale_factor)
        .ok_or(ParseDecimalError::Overflow)?;

    scaled_int
        .checked_add(frac_part)
        .ok_or(ParseDecimalError::Overflow)
}

fn pow10(scale: u32) -> Result<u64, ParseDecimalError> {
    let mut value = 1u64;
    for _ in 0..scale {
        value = value.checked_mul(10).ok_or(ParseDecimalError::Overflow)?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_decimal_scales() {
        assert_eq!(parse_decimal_to_u64("50000.01", 2).unwrap(), 5_000_001);
        assert_eq!(parse_decimal_to_u64("0.1234", 4).unwrap(), 1_234);
        assert_eq!(parse_decimal_to_u64("10", 2).unwrap(), 1_000);
    }

    #[test]
    fn apply_snapshot_and_diff() {
        let config = ExchangeConfig {
            symbol: "BTCUSDT".to_string(),
            price_scale: 2,
            size_scale: 4,
            url: "wss://example.invalid".to_string(),
            rest_url: DEFAULT_REST_URL.to_string(),
            stream: "btcusdt@depth".to_string(),
            origin: DEFAULT_ORIGIN.to_string(),
        };

        let snapshot = br#"{"lastUpdateId":100,"bids":[["50000.01","0.1234"]],"asks":[["50000.02","0.2500"]]}"#;
        let diff = br#"{"e":"depthUpdate","E":1,"s":"BTCUSDT","U":101,"u":102,"b":[["50000.01","0.2000"]],"a":[["50000.02","0"]]}"#;

        let snapshot = parse_binance_snapshot(snapshot).unwrap();
        let mut last_update_id = snapshot.last_update_id;
        let mut updates: Vec<(Side, u64, u64)> = Vec::new();

        {
            let mut handler = |side, price, qty| {
                updates.push((side, price, qty));
                true
            };

            apply_binance_snapshot(&snapshot, &config, &mut handler).unwrap();
            assert_eq!(updates.len(), 2);
            assert_eq!(updates[0], (Side::Bid, 5_000_001, 1_234));
            assert_eq!(updates[1], (Side::Ask, 5_000_002, 2_500));

            updates.clear();
            apply_binance_payload(&config, diff, &mut last_update_id, &mut handler).unwrap();
        }

        assert_eq!(last_update_id, 102);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0], (Side::Bid, 5_000_001, 2_000));
        assert_eq!(updates[1], (Side::Ask, 5_000_002, 0));
    }
}
