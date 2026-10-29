use super::temporary_root;
use crate::local_tools::context::MAX_FILE_READ_BYTES;
use crate::local_tools::files::read::{
    char_bounded_prefix, read_bounded_line, trim_line_ending, ReadLocalFileTool,
};
use crate::LocalToolContext;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

include!("read_tests.part");
