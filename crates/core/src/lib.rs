//! Core of the Outlook-style client: Microsoft Graph access, OAuth login,
//! local cache and the service API used by the UI.

pub mod auth;
pub mod convert;
pub mod error;
pub mod files;
pub mod google;
pub mod graph;
pub mod host;
pub mod imap;
pub mod model;
pub mod service;
pub mod smime;
pub mod store;
pub mod tokens;

pub use error::{Error, Result};
pub use host::Host;
pub use service::{Service, ServiceOptions};
