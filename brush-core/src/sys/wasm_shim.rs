// wasm_shim.rs — minimal wasm shim for brush-core/sys
#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(missing_docs)]

use crate::error;
#[cfg(not(target_family = "wasm"))]
pub use crate::sys::stubs::*;

#[cfg(target_family = "wasm")]
pub mod terminal {
    use crate::{error, openfiles};
    use serde_json::Value;
    use std::fs;
    use std::io;
    use std::path::Path;
    use std::time::Duration;
    use std::thread::sleep;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use std::sync::Mutex;

    static COOKIE_COUNTER: AtomicU32 = AtomicU32::new(1);
    static RESULTS: OnceLock<Mutex<HashMap<u32, String>>> = OnceLock::new();

    fn next_cookie() -> u32 { COOKIE_COUNTER.fetch_add(1, Ordering::Relaxed) }

    fn write_eval_and_wait(js: &str) -> Result<Value, String> {
        let eval_path = if Path::new("/dev/host/eval").exists() { Path::new("/dev/host/eval").to_path_buf() } else { Path::new("./dev/host/eval").to_path_buf() };
        let _ = fs::create_dir_all(&eval_path);
        let cookie = next_cookie();
        let path_js = eval_path.join(format!("{}.js", cookie));
        fs::write(&path_js, js.as_bytes()).map_err(|e| format!("write js: {}", e))?;
        let path_json = eval_path.join(format!("{}.json", cookie));
        for _ in 0..200 { // 2s
            if path_json.exists() {
                let s = fs::read_to_string(&path_json).map_err(|e| format!("read json: {}", e))?;
                let _ = fs::remove_file(&path_json);
                let v: Value = serde_json::from_str(&s).map_err(|e| format!("parse json: {}", e))?;
                return Ok(v);
            }
            sleep(Duration::from_millis(10));
        }
        Err("host eval timeout".into())
    }

    const RAW_ON: &str = r#"(function(){try{if(process.stdin && typeof process.stdin.setRawMode==='function'){process.stdin.setRawMode(true);return JSON.stringify({ok:true});}else{return JSON.stringify({ok:false,err:'setRawMode not supported'});} }catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;
    const RAW_OFF: &str = r#"(function(){try{if(process.stdin && typeof process.stdin.setRawMode==='function'){process.stdin.setRawMode(false);return JSON.stringify({ok:true});}else{return JSON.stringify({ok:false,err:'setRawMode not supported'});} }catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;
    const GET_SIZE: &str = r#"(function(){try{const cols=process.stdout.columns||80;const rows=process.stdout.rows||24;return JSON.stringify({ok:true,cols,rows});}catch(e){return JSON.stringify({ok:false,err:String(e)});} })();"#;

    #[derive(Clone, Debug)]
    pub struct Config { pub echo_input: bool, pub line_input: bool, pub interrupt_signals: bool, pub output_nl_as_nlcr: bool }
    impl Default for Config { fn default() -> Self { Self { echo_input: true, line_input: true, interrupt_signals: true, output_nl_as_nlcr: false } } }

    impl Config {
        pub fn from_term(_fd: &openfiles::OpenFile) -> Result<Self, error::Error> { let _ = Self::get_window_size(); Ok(Self::default()) }
        pub fn apply_to_term(&self, _fd: &openfiles::OpenFile) -> Result<(), error::Error> {
            let js = if !self.line_input { RAW_ON } else { RAW_OFF };
            let v = write_eval_and_wait(js).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            if v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) { Ok(()) } else { let msg = v.get("err").and_then(|s| s.as_str()).unwrap_or("unknown"); Err(error::Error::from(error::ErrorKind::IoError(io::Error::new(io::ErrorKind::Other, msg.to_string())))) }
        }
        pub fn update(&mut self, settings: &crate::terminal::Settings) {
            if let Some(e) = &settings.echo_input { self.echo_input = *e; }
            if let Some(l) = &settings.line_input { self.line_input = *l; }
            if let Some(i) = &settings.interrupt_signals { self.interrupt_signals = *i; }
            if let Some(o) = &settings.output_nl_as_nlcr { self.output_nl_as_nlcr = *o; }
        }
        pub fn set_raw_mode(&mut self, v: bool) { self.line_input = !v }
        pub fn get_window_size() -> Result<(u16,u16), error::Error> {
            let v = write_eval_and_wait(GET_SIZE).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
            if v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) { Ok((v.get("cols").and_then(|c| c.as_u64()).unwrap_or(80) as u16, v.get("rows").and_then(|r| r.as_u64()).unwrap_or(24) as u16)) } else { Err(error::Error::from(error::ErrorKind::IoError(io::Error::new(io::ErrorKind::Other, "failed to get size")))) }
        }
        pub fn isatty() -> bool { Self::get_window_size().is_ok() }
    }

    pub fn get_parent_process_id() -> Option<crate::sys::process::ProcessId> { None }
    pub fn get_process_group_id() -> Option<crate::sys::process::ProcessId> { None }
    pub fn get_foreground_pid() -> Option<crate::sys::process::ProcessId> { None }
    pub fn move_to_foreground(_pid: crate::sys::process::ProcessId) -> Result<(), error::Error> { Ok(()) }
    pub fn move_self_to_foreground() -> Result<(), error::Error> { Ok(()) }
}

#[cfg(target_family = "wasm")]
#[derive(Debug, thiserror::Error)]
pub enum PlatformError { #[error("{0}")] Message(String), }

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
