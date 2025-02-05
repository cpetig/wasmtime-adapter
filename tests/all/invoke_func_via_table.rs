#![cfg(not(miri))]

use anyhow::{Context as _, Result};
use wasmtime::*;

#[test]
fn test_invoke_func_via_table() -> Result<()> {
    let mut store = Store::<()>::default();

    let wat = r#"
      (module
        (func $f (result i64) (i64.const 42))

        (table (export "table") 1 1 funcref)
        (elem (i32.const 0) $f)
      )
    "#;
    let module = Module::new(store.engine(), wat).context("> Error compiling module!")?;
    let instance =
        Instance::new(&mut store, &module, &[]).context("> Error instantiating module!")?;

    let f = instance
        .get_table(&mut store, "table")
        .unwrap()
        .get(&mut store, 0)
        .unwrap()
        .unwrap_func()
        .unwrap()
        .clone();
    let mut results = [Val::I32(0)];
    f.call(&mut store, &[], &mut results).unwrap();
    assert_eq!(results[0].unwrap_i64(), 42);
    Ok(())
}
