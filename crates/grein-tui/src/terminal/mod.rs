pub mod env;
pub mod host;
pub mod port;
pub mod providers;
pub mod registry;

pub use crate::terminal_decoder as decoder;
pub use env::TerminalEnv;
pub use host::TerminalSessionHost;
pub use port::{
    ActiveTerminalModes, ColorCapability, HintLevel, KeyboardProtocolPolicy, TerminalAdapter,
    TerminalCapabilities, TerminalError, TerminalHint, TerminalKind, TerminalProtocolPlan,
    TerminalSessionOptions,
};
pub use registry::TerminalRegistry;
