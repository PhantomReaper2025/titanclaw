//! JIT WASM Tool for compiling and executing Rust code on the fly.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::fs;
use tokio::process::Command;

use crate::context::JobContext;
use crate::tools::tool::{Tool, ToolError, ToolOutput};
use crate::tools::wasm::{Capabilities, EndpointPattern, HttpCapability, WasmToolRuntime, WasmToolWrapper};

const CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "jit-tool"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[profile.release]
opt-level = "s"
lto = true
"#;

const LIB_RS_TEMPLATE: &str = r#"//! JIT generated tool
use serde::{Deserialize, Serialize};

#[link(wasm_import_module = "env")]
extern "C" {
    fn host_log(level: i32, ptr: *const u8, len: usize);
    fn host_http_request(
        method_ptr: *const u8, method_len: usize,
        url_ptr: *const u8, url_len: usize,
        headers_ptr: *const u8, headers_len: usize,
        body_ptr: *const u8, body_len: usize,
        response_ptr: *mut u8, response_max_len: usize,
    ) -> i32;
}

fn log_info(msg: &str) {
    unsafe { host_log(2, msg.as_ptr(), msg.len()); } // LogLevel::Info is 2
}

fn http_get(url: &str) -> Result<String, String> {
    let method = "GET";
    let mut response_buf = vec![0u8; 65536];
    let result = unsafe {
        host_http_request(
            method.as_ptr(), method.len(),
            url.as_ptr(), url.len(),
            std::ptr::null(), 0,
            std::ptr::null(), 0,
            response_buf.as_mut_ptr(), response_buf.len(),
        )
    };
    if result < 0 { return Err(format!("HTTP error: {}", result)); }
    response_buf.truncate(result as usize);
    String::from_utf8(response_buf).map_err(|e| e.to_string())
}

#[no_mangle]
pub extern "C" fn run(input_ptr: *const u8, input_len: usize) -> u64 {
    let result = run_inner(input_ptr, input_len);
    let json = match result {
        Ok(output) => output,
        Err(e) => format!("{{\"error\":\"{}\"}}", e.replace('"', "'")),
    };
    let bytes = json.into_bytes();
    let ptr = bytes.as_ptr() as u64;
    let len = bytes.len() as u64;
    std::mem::forget(bytes);
    (len << 32) | ptr
}

// INJECTED CODE
{{rust_code}}
"#;

pub struct JitWasmTool {
    runtime: Arc<WasmToolRuntime>,
}

impl JitWasmTool {
    pub fn new(runtime: Arc<WasmToolRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl Tool for JitWasmTool {
    fn name(&self) -> &str {
        "jit_wasm_run"
    }

    fn description(&self) -> &str {
        "Compile and run arbitrary Rust code on-the-fly inside a secure WebAssembly sandbox. Extremely fast execution block bypassing bash/python latency."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "input_json": {
                    "type": "string",
                    "description": "Optional JSON string to pass as input to the WASM module."
                },
                "rust_code": {
                    "type": "string",
                    "description": "The Rust business logic to execute. Must implement `fn run_inner(input_ptr: *const u8, input_len: usize) -> Result<String, String>`. Use `log_info(msg)` to log. Use `http_get(url)` to fetch APIs. Return JSON string."
                }
            },
            "required": ["rust_code"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &JobContext,
    ) -> Result<ToolOutput, ToolError> {
        let rust_code = params
            .get("rust_code")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidParameters("rust_code is required".into()))?;

        let input_json = params
            .get("input_json")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        let temp_dir = tempfile::tempdir()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to create temp dir: {}", e)))?;

        let dir_path = temp_dir.path();
        
        // Write Cargo.toml
        if let Err(e) = fs::write(dir_path.join("Cargo.toml"), CARGO_TOML_TEMPLATE).await {
            return Err(ToolError::ExecutionFailed(format!("Failed to write Cargo.toml: {}", e)));
        }

        // Write src/lib.rs
        let src_dir = dir_path.join("src");
        if let Err(e) = fs::create_dir(&src_dir).await {
            return Err(ToolError::ExecutionFailed(format!("Failed to create src dir: {}", e)));
        }

        let lib_rs = LIB_RS_TEMPLATE.replace("{{rust_code}}", rust_code);
        if let Err(e) = fs::write(src_dir.join("lib.rs"), lib_rs).await {
            return Err(ToolError::ExecutionFailed(format!("Failed to write lib.rs: {}", e)));
        }

        // Compile to WASM using cargo
        let build_output = Command::new("cargo")
            .current_dir(&dir_path)
            .arg("build")
            .arg("--target")
            .arg("wasm32-wasip2")
            .arg("--release")
            .output()
            .await;
            
        let build_output = match build_output {
            Ok(out) => out,
            Err(e) => return Err(ToolError::ExecutionFailed(format!("Cargo execution failed: {}", e))),
        };

        if !build_output.status.success() {
            let stderr = String::from_utf8_lossy(&build_output.stderr);
            return Err(ToolError::ExecutionFailed(format!("Compilation failed:\n{}", stderr)));
        }

        let wasm_path = dir_path.join("target/wasm32-wasip2/release/jit_tool.wasm");
        let wasm_bytes = match fs::read(&wasm_path).await {
            Ok(bytes) => bytes,
            Err(e) => return Err(ToolError::ExecutionFailed(format!("Failed to read compiled wasm: {}", e))),
        };

        // Instantiate and run
        let unique_name = format!("jit_tool_{}", uuid::Uuid::new_v4());
        let prepared = match self.runtime.prepare(&unique_name, &wasm_bytes, None).await {
            Ok(p) => p,
            Err(e) => return Err(ToolError::ExecutionFailed(format!("Failed to prepare WASM: {}", e))),
        };

        // We only grant it the HTTP capability and logging capability.
        // It shouldn't have arbitrary access to the file system or secrets.
        let mut caps = Capabilities::default();
        caps.http = Some(HttpCapability {
            allowlist: vec![EndpointPattern::host("*")],
            ..Default::default()
        });

        let wrapper = WasmToolWrapper::new(Arc::clone(&self.runtime), prepared, caps)
            .with_description("JIT Tool");

        // Pass the input_json directly as the param
        let run_params = match serde_json::from_str::<serde_json::Value>(input_json) {
            Ok(v) => v,
            Err(_) => json!({ "input": input_json }),
        };

        let result = wrapper.execute(run_params, ctx).await;
        
        // Cleanup the unique module from the cache after execution
        self.runtime.remove(&unique_name).await;

        result
    }

    fn requires_sanitization(&self) -> bool {
        true
    }
}
