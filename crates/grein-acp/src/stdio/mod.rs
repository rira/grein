mod driver;
mod parser;
mod process;
mod protocol;
#[cfg(test)]
mod tests;

use std::sync::{Mutex, MutexGuard};

pub use driver::AcpStdioDriver;
pub use process::{StdioProcessConfig, StdioProcessRunner};
pub use protocol::{
    AcpConfigChoice, AcpConfigOption, AcpInitializeResult, AcpModeOption, AcpModelOption,
    AcpPermissionOption, AcpSessionInfo, AcpSlashCommand, AcpUsage, AgentDriver, DriverEvent,
    JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse,
    StdioRunnerError,
};

pub(super) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
