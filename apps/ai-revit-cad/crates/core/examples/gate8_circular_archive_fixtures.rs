//! Explicit producer for portable synthetic archive/persistence test fixtures.
//! Run with `--features test-fixtures --example gate8_circular_archive_fixtures`.
//! Numerical verification and engineering acceptance remain separate.
use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("Usage: gate8_circular_archive_fixtures <new-output-directory>".into());
    }
    ai_revit_core::test_fixtures::produce_circular_archive_fixtures(&PathBuf::from(&args[0]))
}
