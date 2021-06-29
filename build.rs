use anyhow::Result;
use vergen::{vergen, Config};

fn main() -> Result<()> {
    let mut config = Config::default();
    *config.git_mut().semver_dirty_mut() = Some("-dirty");

    vergen(config)
}
