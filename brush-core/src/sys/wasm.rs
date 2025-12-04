// Minimal wasm shim for brush-core sys module
#![allow(dead_code)]
#![allow(unused_imports)]

// For non-wasm builds use stub implementations
#[cfg(not(target_arch = "wasm32"))]
pub use crate::sys::stubs::*;

// wasm-only terminal shim
#[cfg(target_arch = "wasm32")]
pub mod terminal {
    use crate::{error, openfiles, sys, terminal};
    use futures::Future;
    use once_cell::sync::OnceCell;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::pin::Pin;
    use std::sync::{Mutex};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::task::{Context, Poll, Waker};

    static COOKIE_COUNTER: AtomicU32 = AtomicU32::new(1);
    static CONTINUATIONS: OnceCell<Mutex<HashMap<u32, Waker>>> = OnceCell::new();
    static RESULTS: OnceCell<Mutex<HashMap<u32, String>>> = OnceCell::new();
    static EVAL_PATH: OnceCell<std::path::PathBuf> = OnceCell::new();

    fn init_maps() {
        let _ = CONTINUATIONS.get_or_init(|| Mutex::new(HashMap::new()));
        let _ = RESULTS.get_or_init(|| Mutex::new(HashMap::new()));
    }

    fn next_cookie() -> u32 { COOKIE_COUNTER.fetch_add(1, Ordering::Relaxed) }

    pub struct WasmHostFuture { cookie: u32 }

    impl WasmHostFuture {
        pub fn new(js: String) -> Self {
            init_maps();
            let cookie = next_cookie();
            let path = Path::new("/dev/host/eval");
            let eval_path = if fs::create_dir_all(path).is_ok() { path.to_path_buf() } else { let fallback = Path::new("./dev/host/eval").to_path_buf(); let _ = fs::create_dir_all(&fallback); fallback };
            let _ = EVAL_PATH.set(eval_path.clone());
            let req = eval_path.join(format!("{}.js", cookie));
            let _ = fs::write(req, js.as_bytes());
            WasmHostFuture { cookie }
        }
        pub fn cookie(&self) -> u32 { self.cookie }
    }

    impl Future for WasmHostFuture {
        type Output = Result<String, &'static str>;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let cookie = self.cookie;
            let results = RESULTS.get_or_init(|| Mutex::new(HashMap::new()));
            if let Some(res) = results.lock().unwrap().remove(&cookie) { return Poll::Ready(Ok(res)); }
            let conts = CONTINUATIONS.get_or_init(|| Mutex::new(HashMap::new()));
            conts.lock().unwrap().insert(cookie, cx.waker().clone());
            Poll::Pending
        }
    }

    #[no_mangle]
    pub extern "C" fn _continue(cookie: u32) {
        let path = EVAL_PATH.get().map(|p| p.clone()).unwrap_or_else(|| Path::new("/dev/host/eval").to_path_buf());
        let result_file = path.join(format!("{}.json", cookie));
        if let Ok(result_json) = std::fs::read_to_string(&result_file) {
            if let Some(results) = RESULTS.get() { results.lock().unwrap().insert(cookie, result_json); }
            let _ = std::fs::remove_file(&result_file);
            if let Some(conts) = CONTINUATIONS.get() { if let Some(waker) = conts.lock().unwrap().remove(&cookie) { waker.wake(); } }
        }
    }

    fn eval_blocking(js: &str) -> Result<String, error::Error> {
        let fut = WasmHostFuture::new(js.to_string());
        match futures::executor::block_on(fut) { Ok(s) => Ok(s), Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e).into()), }
    }

    const RAW_ON: &str  = r#"(function(){try{if(process.stdin && typeof process.stdin.setRawMode==='function'){process.stdin.setRawMode(true);return JSON.stringify({ok:true});}else{return JSON.stringify({ok:false,err:'setRawMode not supported'});} }catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;
    const RAW_OFF: &str = r#"(function(){try{if(process.stdin && typeof process.stdin.setRawMode==='function'){process.stdin.setRawMode(false);return JSON.stringify({ok:true});}else{return JSON.stringify({ok:false,err:'setRawMode not supported'});} }catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;
    const GET_SIZE: &str = r#"(function(){try{const cols=process.stdout.columns||80;const rows=process.stdout.rows||24;return JSON.stringify({ok:true,cols,rows});}catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;

    #[derive(Clone, Debug)]
    pub struct Config { echo_input: bool, line_input: bool, interrupt_signals: bool, output_nl_as_nlcr: bool }
    impl Default for Config { fn default() -> Self { Self { echo_input: true, line_input: true, interrupt_signals: true, output_nl_as_nlcr: false } } }

    impl Config {
        pub fn from_term(_fd: &openfiles::OpenFile) -> Result<Self, error::Error> { let _ = Self::get_window_size(); Ok(Self::default()) }
        pub fn apply_to_term(&self, _fd: &openfiles::OpenFile) -> Result<(), error::Error> {
            let js = if !self.line_input { RAW_ON } else { RAW_OFF };
            let res = eval_blocking(js)?;
            let parsed: Value = serde_json::from_str(&res).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("parse eval response: {}", e)))?;
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) { Ok(()) } else { let err = parsed.get("err").and_then(|v| v.as_str()).unwrap_or("unknown"); Err(std::io::Error::new(std::io::ErrorKind::Other, err.to_string()).into()) }
        }
        pub fn update(&mut self, settings: &terminal::Settings) {
            if let Some(e) = &settings.echo_input { self.echo_input = *e; }
            if let Some(l) = &settings.line_input { self.line_input = *l; }
            if let Some(i) = &settings.interrupt_signals { self.interrupt_signals = *i; }
            if let Some(o) = &settings.output_nl_as_nlcr { self.output_nl_as_nlcr = *o; }
        }
        pub fn set_raw_mode(&mut self, v: bool) { self.line_input = !v }
        pub fn get_window_size() -> Result<(u16, u16), error::Error> {
            let res = eval_blocking(GET_SIZE)?;
            let parsed: Value = serde_json::from_str(&res).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("parse get-size json: {}", e)))?;
            if parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) { let cols = parsed.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16; let rows = parsed.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16; Ok((cols, rows)) } else { Err(std::io::Error::new(std::io::ErrorKind::Other, "failed to get size").into()) }
        }
        pub fn isatty() -> bool { Self::get_window_size().is_ok() }
    }
    pub fn get_parent_process_id() -> Option<sys::process::ProcessId> { None }
    pub fn get_process_group_id() -> Option<sys::process::ProcessId> { None }
    pub fn get_foreground_pid() -> Option<sys::process::ProcessId> { None }
    pub fn move_to_foreground(_pid: sys::process::ProcessId) -> Result<(), error::Error> { Ok(()) }
    pub fn move_self_to_foreground() -> Result<(), error::Error> { Ok(()) }
}

#[cfg(target_arch = "wasm32")]
#[derive(Debug, thiserror::Error)]
pub enum PlatformError { #[error("{0}")] Message(String), }
#[cfg(target_arch = "wasm32")]
impl From<PlatformError> for error::ErrorKind { fn from(err: PlatformError) -> Self { error::ErrorKind::PlatformError(err) } }

// Re-export other sys stubs
pub use crate::sys::stubs::commands;
pub use crate::sys::stubs::fd;
pub use crate::sys::stubs::fs;
pub use crate::sys::stubs::input;
pub(crate) use crate::sys::stubs::network;
pub(crate) use crate::sys::stubs::pipes;
pub use crate::sys::stubs::process;
pub use crate::sys::stubs::resource;
pub use crate::sys::stubs::signal;
pub(crate) use crate::sys::stubs::users;
