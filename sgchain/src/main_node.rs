use executable_helpers::helpers::setup_executable;
use logger::prelude::*;
use slog_scope::GlobalLoggerGuard;
use std::path::Path;

use config::config::NodeConfig;
use libra_node::main_node::LibraHandle;

pub fn run_node(
    config: Option<&Path>,
    no_logging: bool,
) -> (NodeConfig, Option<GlobalLoggerGuard>, LibraHandle) {
    let (mut config, logger) = setup_executable(config, no_logging);
    debug!("config : {:?}", config);
    crate::star_chain_client::genesis_blob(&config);
    let handler = libra_node::main_node::setup_environment(&mut config);
    (config, logger, handler)
}
