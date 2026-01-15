#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key(Vec<u8>);
impl Key {
    pub fn new<T: AsRef<[u8]>>(data: T) -> Self {
        Key(data.as_ref().to_vec())
    }

    pub fn to_string(&self) -> String {
        String::from_utf8(self.0.clone()).unwrap()
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.clone()
    }
}

