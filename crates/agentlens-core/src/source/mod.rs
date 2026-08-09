//! 各数据来源的解析与扫描实现。
//!
//! 子模块归属（并行开发边界，勿跨模块编辑）：
//! `opencode` = todo 5，`opencode_legacy` = todo 7，`claude_code` = Claude Code 适配器。

pub mod claude_code;
pub mod codex;
pub mod hermes;
pub mod opencode;
pub mod opencode_legacy;
