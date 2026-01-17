pub const MAX_KEY_SIZE: usize = 256;
pub const MAX_VALUE_SIZE: usize = 1024;

#[repr(C)]
#[derive(Clone, Hash, PartialEq, Debug)]
pub struct Key {
    len: u16,
    data: [u8; MAX_KEY_SIZE],
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
}

pub struct Value {
    len: u16,
    data: [u8;MAX_VALUE_SIZE],
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
}