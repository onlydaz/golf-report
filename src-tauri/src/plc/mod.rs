//! AS200 Modbus acquisition, durable PostgreSQL receipt delivery and connection lifecycle.
pub mod as200;
pub mod connection_settings;
pub(crate) mod diagnostics;
pub mod simulator_config;
pub mod worker;
