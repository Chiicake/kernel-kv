use std::collections::{HashMap, HashSet, BTreeMap};
use std::time::Instant;

#[derive(Debug, Clone)]
pub enum ValueData {
    String(Vec<u8>),
    List(Vec<Vec<u8>>),
    Hash(HashMap<Vec<u8>, Vec<u8>>),
    Set(HashSet<Vec<u8>>),
    ZSet(BTreeMap<f64, HashSet<Vec<u8>>>),
}

#[derive(Debug, Clone)]
pub struct Value {
    data: ValueData,
    expire_at: Option<Instant>,
}