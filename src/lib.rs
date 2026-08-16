pub mod api;
pub mod client;
pub mod crypto;
pub mod db;
pub mod error;
pub mod federation;
pub mod ingest;
pub mod model;
pub mod node;
pub mod publication;
pub mod service;

pub use api::router;
pub use error::{CommonwakeError, Result};
pub use node::CommonwakeNode;

pub const PROTOCOL_VERSION: &str = "commonwake/0.1";
pub const CONSTITUTION_VERSION: &str = "0.1";
