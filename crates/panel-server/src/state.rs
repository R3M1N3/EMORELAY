use std::sync::Arc;

use crate::{
    config::Config,
    grpc::{dispatcher::CommandDispatcher, session::SessionRegistry},
};
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub pool: SqlitePool,
    pub sessions: Arc<SessionRegistry>,
    pub dispatcher: Arc<CommandDispatcher>,
    pub ca: std::sync::Arc<crate::tls::ca::CaBundle>,
    pub crl: std::sync::Arc<crate::tls::crl::Crl>,
}
