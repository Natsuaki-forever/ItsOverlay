use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};

#[link(name = "kernel32")]
extern "system" {
    fn SetNamedPipeHandleState(
        pipe: *mut std::ffi::c_void,
        mode: *const u32,
        max_collection: *const u32,
        timeout: *const u32,
    ) -> i32;
}

pub struct Engine {
    pipe: File,
    session: String,
    request_id: u64,
}

impl Engine {
    pub fn connect(path: &str, session: String) -> Result<Self, String> {
        let pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(0x0001_0000) // SECURITY_IDENTIFICATION
            .open(path)
            .map_err(|e| e.to_string())?;
        let message_mode = 2;
        if unsafe {
            SetNamedPipeHandleState(pipe.as_raw_handle(), &message_mode, std::ptr::null(), std::ptr::null())
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self { pipe, session, request_id: 0 })
    }

    pub fn request(&mut self, op: &str, payload: Value, revision: Option<u64>) -> Result<Value, String> {
        self.request_id += 1;
        let mut request = json!({
            "protocol": 1, "session": self.session, "request_id": self.request_id,
            "op": op, "payload": payload
        });
        if let Some(revision) = revision {
            request["base_revision"] = json!(revision);
        }
        let data = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
        let mut response = vec![0; 1024 * 1024];
        if data.len() > response.len() {
            return Err("请求过大".into());
        }
        if self.pipe.write(&data).map_err(|e| e.to_string())? != data.len() {
            return Err("本地请求没有完整发送".into());
        }
        let count = self.pipe.read(&mut response).map_err(|e| e.to_string())?;
        let reply: Value = serde_json::from_slice(&response[..count]).map_err(|e| e.to_string())?;
        if reply["request_id"].as_u64() != Some(self.request_id) {
            return Err("响应编号不匹配".into());
        }
        Ok(reply)
    }
}
