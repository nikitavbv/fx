use fx::PtrWithLen;

#[derive(Clone)]
pub struct FxCloudClient {}

impl FxCloudClient {
    pub fn new() -> Self {
        Self {}
    }
}

pub mod list_functions {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct Function {
        pub id: String,
    }

    impl FxCloudClient {
        pub fn list_functions(&self) -> Vec<Function> {
            let ptr_and_len = PtrWithLen::new();
            unsafe { sys::list_functions(ptr_and_len.ptr_to_self()) };
            /*ptr_and_len.read_decode::<Vec<fx_runtime_common::Function>>()
                .into_iter()
                .map(|v| Function {
                    id: v.id,
                })
                .collect()*/
                unimplemented!()
        }
    }
}

mod sys {
    #[link(wasm_import_module = "fx_cloud")]
    unsafe extern "C" {
        pub(crate) fn list_functions(output_ptr: i64);
    }
}
