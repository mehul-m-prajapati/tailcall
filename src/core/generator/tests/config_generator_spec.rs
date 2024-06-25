use std::path::Path;
use std::sync::Arc;

use http::NativeHttpTest;
use tailcall::cli::generator::Generator;
use tailcall::core::blueprint::Blueprint;
use tailcall::core::config;
use tailcall::core::generator::Generator as ConfigGenerator;
use tokio::runtime::Runtime;

mod fs_cache;
mod http;

datatest_stable::harness!(
    run_config_generator_spec,
    "src/core/generator/tests/fixtures/generator",
    r"^.*\.json"
);

pub fn run_config_generator_spec(path: &Path) -> datatest_stable::Result<()> {
    let path = path.to_path_buf();
    let runtime = Runtime::new().unwrap();
    runtime.block_on(async move {
        run_test(&path.to_string_lossy()).await?;
        Ok(())
    })
}

async fn run_test(path: &str) -> anyhow::Result<()> {
    let mut runtime = tailcall::cli::runtime::init(&Blueprint::default());
    runtime.http = Arc::new(NativeHttpTest::default());

    let generator = Generator::new(path, runtime);
    let config = generator.read().await?;
    let preset: config::transformer::Preset = config.preset.clone().unwrap_or_default().into();

    // resolve i/o's
    let input_samples = generator.resolve_io(config).await?;

    let config = ConfigGenerator::default()
        .inputs(input_samples)
        .transformers(vec![Box::new(preset)])
        .generate(false)?;

    insta::assert_snapshot!(path, config.to_sdl());
    Ok(())
}