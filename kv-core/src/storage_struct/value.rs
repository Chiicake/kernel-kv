use std::collections::{HashMap, HashSet, BTreeMap};
#[derive(Debug, Clone)]
pub enum Value {
    String(Vec<u8>),
    List(Vec<Vec<u8>>),
    Hash(HashMap<Vec<u8>, Vec<u8>>),
    Set(HashSet<Vec<u8>>),
    ZSet(BTreeMap<f64, HashSet<Vec<u8>>>),
}
