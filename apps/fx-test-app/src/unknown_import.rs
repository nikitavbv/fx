#[link(wasm_import_module = "_fx_test")]
unsafe extern "C" {
    pub(crate) fn _test_unknown_import(arg: i64);
}
