//! Fleet: multi-machine project-repo sync (docs/xw-fleet-sync-design.md).
//!
//! P0 is read-only towards project repositories: the only write surface is the
//! machine status report, and it writes exclusively into the meta repo cache
//! under the app data directory. `repo_ops` deliberately exposes no mutating
//! git verb (design §5 verb whitelist: fetch / ls-remote style reads only).

pub mod auto_round;
pub mod manifest;
pub mod meta_repo;
pub mod repo_ops;
pub mod service;

pub use service::FleetService;
