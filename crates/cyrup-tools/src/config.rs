//! Per-tool configuration (arch-03 §3.4 `ToolsOptions`).

use crate::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, FIND_MAX_RESULTS, GREP_MAX_MATCHES, LS_MAX_ENTRIES,
};

#[derive(Clone, Debug)]
pub struct ReadOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Whether the active model can consume images (R-03-012 non-vision fallback).
    pub supports_images: bool,
    /// Max image bound (both dimensions) before resize.
    pub max_image_dim: u32,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            supports_images: true,
            max_image_dim: 2000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WriteOpts;

#[derive(Clone, Debug, Default)]
pub struct EditOpts;

#[derive(Clone, Debug)]
pub struct BashOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Optional command prefix prepended before the command (R-03-025, arch-07).
    pub command_prefix: Option<String>,
}

impl Default for BashOpts {
    fn default() -> Self {
        Self { max_lines: DEFAULT_MAX_LINES, max_bytes: DEFAULT_MAX_BYTES, command_prefix: None }
    }
}

#[derive(Clone, Debug)]
pub struct GrepOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for GrepOpts {
    fn default() -> Self {
        Self { limit: GREP_MAX_MATCHES, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug)]
pub struct FindOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for FindOpts {
    fn default() -> Self {
        Self { limit: FIND_MAX_RESULTS, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug)]
pub struct LsOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for LsOpts {
    fn default() -> Self {
        Self { limit: LS_MAX_ENTRIES, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolsOptions {
    pub read: ReadOpts,
    pub write: WriteOpts,
    pub edit: EditOpts,
    pub bash: BashOpts,
    pub grep: GrepOpts,
    pub find: FindOpts,
    pub ls: LsOpts,
}
