// C2 builds the observability foundation before C3 wires it into runtime paths.
#![allow(dead_code)]

pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod events;
pub(crate) mod panic;
pub(crate) mod redaction;
pub(crate) mod rotation;
pub(crate) mod schema;
pub(crate) mod sinks;
pub(crate) mod support_bundle;
pub(crate) mod usage;
