use std::hash::{Hash, Hasher};

pub const MAX_KEY_SIZE: usize = 256;
pub const MAX_VALUE_SIZE: usize = 1024;

#[repr(C)]
#[derive(Clone, Debug)]
pub struct Key {
    len: u16,
    data: [u8; MAX_KEY_SIZE],
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        self.as_bytes().hash(state);
    }
}

impl Key {
    pub fn new(data: &[u8]) -> Option<Self> {
        if data.len() > MAX_KEY_SIZE {
            return None;
        }

        let mut key = Key {
            len: data.len() as u16,
            data: [0u8; MAX_KEY_SIZE],
        };
        key.data[..data.len()].copy_from_slice(data);
        Some(key)
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Value {
    len: u16,
    data: [u8;MAX_VALUE_SIZE],
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len.hash(state);
        self.as_bytes().hash(state);
    }
}

impl Value {
    pub fn new(data: &[u8]) -> Option<Self> {
        if data.len() > MAX_VALUE_SIZE {
            return None;
        }

        let len = data.len() as u16;
        let mut v_data = [0u8; MAX_VALUE_SIZE];
        v_data[..data.len()].copy_from_slice(data);

        Some(Value{
            len,
            data: v_data,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

#[cfg(test)]
mod tests {
}