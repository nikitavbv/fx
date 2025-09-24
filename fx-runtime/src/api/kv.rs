use {
    wasmer::{FunctionEnvMut, Value},
    crate::{
        runtime::{ExecutionEnv, read_memory_owned, write_memory, write_memory_obj, PtrWithLen},
        kv::KVStorage,
    },
};

pub fn handle_kv_get(mut ctx: FunctionEnvMut<ExecutionEnv>, binding_addr: i64, binding_len: i64, k_addr: i64, k_len: i64, output_ptr: i64) -> i64 {
    let binding = String::from_utf8(read_memory_owned(&ctx, binding_addr, binding_len)).unwrap();
    let storage = match ctx.data().storage.get(&binding) {
        Some(v) => v,
        None => return 1,
    };

    let key = read_memory_owned(&ctx, k_addr, k_len);
    let value = storage.get(&key).unwrap();
    let value = match value {
        Some(v) => v,
        None => return 2,
    };

    let (data, mut store) = ctx.data_and_store_mut();

    let len = value.len() as i64;
    let ptr = data.client_malloc().call(&mut store, &[Value::I64(len)]).unwrap()[0].i64().unwrap();
    write_memory(&ctx, ptr, &value);

    write_memory_obj(&ctx, output_ptr, PtrWithLen { ptr, len });

    0
}
