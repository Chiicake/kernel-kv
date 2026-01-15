// 引入内核必要的 Rust 绑定
use kernel::prelude::*;
use kernel::sync::Mutex;
use kernel::collections::HashMap;
use kernel::str::CStr;

type KVKey = &'static CStr;
type KVValue = u64;

static KV_STORE: Mutex<HashMap<KVKey, KVValue>> = Mutex::new(HashMap::new());

fn kv_set(key: KVKey, value: KVValue) -> Result<(), &'static str> {
    // 获取锁（内核态 Mutex，自动处理死锁检测）
    let mut store = KV_STORE.lock();
    // 插入/更新键值对
    store.insert(key, value);
    Ok(())
}

fn kv_get(key: KVKey) -> Option<KVValue> {
    let store = KV_STORE.lock();
    store.get(&key).copied()
}

#[init]
fn kv_module_init() -> Result<(), &'static str> {
    pr_info!("KV module initialized (Rust)\n");

    let test_key = CStr::from_bytes_with_nul(b"test_key\0").unwrap();
    kv_set(test_key, 12345)?;
    pr_info!("Inserted test_key: 12345\n");

    if let Some(val) = kv_get(test_key) {
        pr_info!("Queried test_key: {}\n", val);
    } else {
        pr_err!("test_key not found!\n");
    }

    Ok(())
}

#[exit]
fn kv_module_exit() {
    pr_info!("KV module exiting (Rust)\n");
    let mut store = KV_STORE.lock();
    store.clear();
    pr_info!("KV store cleared\n");
}


module! {
    name: "rust_kv",
    init: kv_module_init,
    exit: kv_module_exit,
    license: "GPL", // 内核模块必须声明 GPL 许可证
    author: "Chiicake",
    description: "Rust KV store for Linux kernel",
    version: "1.0",
}