//! AS200 Modbus acquisition, durable PostgreSQL receipt delivery and connection lifecycle.
pub mod as200;
pub mod connection_settings;
mod diagnostics;
pub mod simulator_config;
pub mod worker;
