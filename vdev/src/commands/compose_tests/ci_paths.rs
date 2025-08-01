use anyhow::Result;

use crate::testing::config::ComposeTestConfig;

pub(crate) fn exec(path: &str) -> Result<()> {
    // paths for each integration are defined in their respective config files.
    for (test_name, config) in ComposeTestConfig::collect_all(path)? {
        if let Some(paths) = config.paths {
            println!("{test_name}:");
            for path in paths {
                println!("- {path:?}");
            }
        }
    }

    Ok(())
}
