use brush_core::sys::wasm::terminal::{WasmHostFuture, _continue, Config};
use std::fs;
use std::path::Path;

#[test]
fn test_wasm_host_future_roundtrip() {
    let js = "(function(){ return JSON.stringify({ok:true, result: {ok:true}}); })()".to_string();
    let fut = WasmHostFuture::new(js);
    let cookie = fut.cookie();
    let path = if Path::new("/dev/host/eval").exists() { Path::new("/dev/host/eval").to_path_buf() } else { Path::new("./dev/host/eval").to_path_buf() };
    let _ = fs::create_dir_all(&path);
    let result_file = path.join(format!("{}.json", cookie));
    let content = r#"{"ok":true, "result": {"ok": true}}"#;
    fs::write(result_file, content.as_bytes()).expect("write cookie json");
    _continue(cookie);
    let res = futures::executor::block_on(fut);
    assert!(res.is_ok());
}

#[test]
fn test_config_apply_to_term_ok() {
    let js = "(function(){ return JSON.stringify({ok:true, result: {ok:true}}); })()".to_string();
    let fut = WasmHostFuture::new(js);
    let cookie = fut.cookie();
    let path = if Path::new("/dev/host/eval").exists() { Path::new("/dev/host/eval").to_path_buf() } else { Path::new("./dev/host/eval").to_path_buf() };
    let _ = fs::create_dir_all(&path);
    let result_file = path.join(format!("{}.json", cookie));
    let content = r#"{"ok":true, "result": {"ok": true}}"#;
    fs::write(result_file, content.as_bytes()).expect("write cookie json");

    _continue(cookie);

    let cfg = Config::from_term(()).expect("from_term ok");
    let res = cfg.apply_to_term(());
    assert!(res.is_ok());
}
