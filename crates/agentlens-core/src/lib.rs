//! AgentLens 内核库。
//!
//! 本 crate 承载采集、归一化、聚合与归档逻辑，供 Tauri 壳（`agentlens-tauri`）
//! 与 headless 采集二进制（`agentlens-collector`）复用。
//!
//! 模块归属（并行开发边界，勿跨模块编辑）：
//! `archive` = todo 3，`fixture` = todo 2，`host` = todo 4，
//! `hostsource` = todo 12，`ingest` = todo 6，`pricing` = todo 9，`query` = todo 8，
//! `source::opencode` = todo 5，`source::opencode_legacy` = todo 7，
//! `transport::ssh` = todo 11。

pub mod archive;
pub mod fixture;
pub mod host;
pub mod hostsource;
pub mod ingest;
pub mod pricing;
pub mod query;
pub mod source;
pub mod transport;

/// 返回本 crate 的版本号（来自 `CARGO_PKG_VERSION`）。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
