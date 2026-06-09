//! EMORELAY panel-server library crate.
//!
//! 模块声明从 `main.rs` 上提到 `lib.rs`,使 integration tests(`tests/` 下)
//! 能直接 `use panel_server::{routes, state, ...}` 而无需 cfg-based 复制。
//! binary (`src/main.rs`) 也从 `panel_server::` 拉模块,逻辑零变化。

pub mod audit;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod db;
pub mod error;
pub mod grpc;
pub mod models;
pub mod routes;
pub mod state;
pub mod sweeper;
pub mod util;
