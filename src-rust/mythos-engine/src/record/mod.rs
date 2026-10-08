//! 记录业务：格式、单写者与恢复、有效路径、分页及纯投影。

pub mod calibration;
pub mod estimator;
pub mod facts;
pub mod format;
pub mod grammar;
pub mod history;
pub mod projection;
pub mod session;
#[cfg(test)]
pub(crate) mod test_support;
pub mod view;
pub mod world;

pub mod recap;
pub mod working_set;

pub mod compression;
