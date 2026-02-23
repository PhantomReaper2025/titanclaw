//! WASM tool metadata introspection via WIT exports (`schema()` / `description()`).
//!
//! Uses a minimal host implementation with all side-effectful capabilities disabled.

use std::time::Duration;

use wasmtime::Store;
use wasmtime::component::{Component, Linker};
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiView};

use crate::tools::wasm::error::WasmError;

wasmtime::component::bindgen!({
    path: "wit/tool.wit",
    world: "sandboxed-tool",
    async: false,
    with: {},
});

#[derive(Debug, Clone, Copy)]
pub enum MetadataSource {
    WitExport,
    Fallback,
}

#[derive(Debug)]
pub struct ProbedMetadata {
    pub description: Option<String>,
    pub schema: Option<serde_json::Value>,
    pub source: MetadataSource,
}

struct ProbeStore {
    wasi: WasiCtx,
    table: ResourceTable,
}

impl ProbeStore {
    fn new() -> Self {
        Self {
            wasi: WasiCtxBuilder::new().build(),
            table: ResourceTable::new(),
        }
    }
}

impl WasiView for ProbeStore {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl near::agent::host::Host for ProbeStore {
    fn log(&mut self, level: near::agent::host::LogLevel, message: String) {
        match level {
            near::agent::host::LogLevel::Trace => {
                tracing::trace!(target: "wasm_metadata_probe", "{}", message)
            }
            near::agent::host::LogLevel::Debug => {
                tracing::debug!(target: "wasm_metadata_probe", "{}", message)
            }
            near::agent::host::LogLevel::Info => {
                tracing::info!(target: "wasm_metadata_probe", "{}", message)
            }
            near::agent::host::LogLevel::Warn => {
                tracing::warn!(target: "wasm_metadata_probe", "{}", message)
            }
            near::agent::host::LogLevel::Error => {
                tracing::error!(target: "wasm_metadata_probe", "{}", message)
            }
        }
    }

    fn now_millis(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_millis() as u64
    }

    fn workspace_read(&mut self, _path: String) -> Option<String> {
        None
    }

    fn http_request(
        &mut self,
        _method: String,
        _url: String,
        _headers_json: String,
        _body: Option<Vec<u8>>,
        _timeout_ms: Option<u32>,
    ) -> Result<near::agent::host::HttpResponse, String> {
        Err("HTTP capability unavailable during metadata probe".to_string())
    }

    fn tool_invoke(&mut self, _alias: String, _params_json: String) -> Result<String, String> {
        Err("Tool invocation unavailable during metadata probe".to_string())
    }

    fn secret_exists(&mut self, _name: String) -> bool {
        false
    }
}

fn add_probe_host_functions(linker: &mut Linker<ProbeStore>) -> Result<(), WasmError> {
    wasmtime_wasi::add_to_linker_sync(linker)
        .map_err(|e| WasmError::ConfigError(format!("Failed to add WASI functions: {}", e)))?;
    near::agent::host::add_to_linker(linker, |state| state)
        .map_err(|e| WasmError::ConfigError(format!("Failed to add host functions: {}", e)))?;
    Ok(())
}

pub fn probe_tool_metadata(
    engine: &wasmtime::Engine,
    component: &Component,
    tool_name: &str,
    fuel_enabled: bool,
) -> Result<ProbedMetadata, WasmError> {
    let mut store = Store::new(engine, ProbeStore::new());
    if fuel_enabled {
        store
            .set_fuel(200_000)
            .map_err(|e| WasmError::ConfigError(format!("Failed to set probe fuel: {}", e)))?;
    }

    store.epoch_deadline_trap();
    store.set_epoch_deadline(2);

    let mut linker = Linker::new(engine);
    add_probe_host_functions(&mut linker)?;

    let instance = SandboxedTool::instantiate(&mut store, component, &linker)
        .map_err(|e| WasmError::InstantiationFailed(format!("Metadata probe failed: {}", e)))?;

    let tool = instance.near_agent_tool();

    let description = match tool.call_description(&mut store) {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                tracing::warn!(tool = %tool_name, "WASM tool returned empty description()");
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Err(e) => {
            tracing::warn!(tool = %tool_name, error = %e, "Failed to call WASM description()");
            None
        }
    };

    let schema = match tool.call_schema(&mut store) {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(v) if v.is_object() => Some(v),
            Ok(_) => {
                tracing::warn!(
                    tool = %tool_name,
                    "WASM tool schema() returned non-object JSON; using fallback"
                );
                None
            }
            Err(e) => {
                tracing::warn!(
                    tool = %tool_name,
                    error = %e,
                    "WASM tool schema() returned invalid JSON; using fallback"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(tool = %tool_name, error = %e, "Failed to call WASM schema()");
            None
        }
    };

    let source = if description.is_some() || schema.is_some() {
        MetadataSource::WitExport
    } else {
        MetadataSource::Fallback
    };

    Ok(ProbedMetadata {
        description,
        schema,
        source,
    })
}
