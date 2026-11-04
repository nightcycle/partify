use anyhow::{Context, Result, ensure};
use clap::Parser;
use partify::{
    config::Tuning,
    export,
    import::{self, Assets},
    pipeline,
};
use std::{path::PathBuf, sync::atomic::AtomicBool};

#[derive(Parser)]
#[command(
    version,
    about = "Rebuild images and models with colored Roblox parts under a hard part budget"
)]
struct Args {
    /// PNG/JPG image or RBXM/RBXMX/OBJ/FBX model
    input: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Maximum number of output BaseParts (may use fewer when splits do not improve fit)
    #[arg(short = 'b', long, alias = "part-count")]
    budget: Option<usize>,
    /// Open the interactive egui tuning and layer viewer
    #[arg(long)]
    window: bool,
    /// JSON tuning preset, or an RBXM/RBXMX with baked PartifyTuning
    #[arg(long)]
    tuning_from: Option<PathBuf>,
    /// Longest source dimension in studs
    #[arg(long)]
    size: Option<f32>,
    #[arg(long)]
    thickness: Option<f32>,
    #[arg(long)]
    resolution: Option<u32>,
    #[arg(long)]
    samples: Option<usize>,
    #[arg(long)]
    seed: Option<u32>,
    /// Fit a surface shell even when the source mesh is watertight
    #[arg(long)]
    shell: bool,
    #[arg(long)]
    no_meshes: bool,
    #[arg(long)]
    studs: bool,
    /// Set any tuning field, e.g. --set spill_weight=4 --set edge_angle=25
    #[arg(long = "set", value_name = "KEY=JSON")]
    overrides: Vec<String>,
    #[arg(long, default_value = ".partify-cache")]
    asset_cache: PathBuf,
    /// JSON object mapping exact Roblox content URIs to local mesh/texture files
    #[arg(long)]
    asset_map: Option<PathBuf>,
    #[arg(long)]
    offline: bool,
    /// Save all layers, metrics, connections and tuning as JSON
    #[arg(long)]
    report: Option<PathBuf>,
    /// Save the resolved tuning as a reusable JSON preset
    #[arg(long)]
    save_tuning: Option<PathBuf>,
}
fn main() -> Result<()> {
    let args = Args::parse();
    let mut t = if let Some(p) = &args.tuning_from {
        export::read_tuning(p)?
    } else {
        Tuning::default()
    };
    if let Some(v) = args.budget {
        t.budget = v
    }
    if let Some(v) = args.size {
        t.size = v
    }
    if let Some(v) = args.thickness {
        t.thickness = v
    }
    if let Some(v) = args.resolution {
        t.image_resolution = v
    }
    if let Some(v) = args.samples {
        t.sample_limit = v
    }
    if let Some(v) = args.seed {
        t.seed = v
    }
    if args.shell {
        t.solid = false
    }
    if args.no_meshes {
        t.allow_meshes = false
    }
    if args.studs {
        t.studs = true
    }
    let mut json = serde_json::to_value(t)?;
    for s in &args.overrides {
        let (key, value) = s.split_once('=').context("--set requires KEY=JSON")?;
        ensure!(json.get(key).is_some(), "unknown tuning field {key}");
        json[key] =
            serde_json::from_str(value).with_context(|| format!("invalid JSON for {key}"))?;
    }
    let t: Tuning = serde_json::from_value(json)?;
    t.validate()?;
    let output = args.output.unwrap_or_else(|| {
        args.input.with_file_name(format!(
            "{}.partified.rbxm",
            args.input.file_stem().unwrap_or_default().to_string_lossy()
        ))
    });
    ensure!(output != args.input, "output must differ from input");
    ensure!(
        output
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("rbxm") || s.eq_ignore_ascii_case("rbxmx")),
        "output must end in .rbxm or .rbxmx"
    );
    let input_path = std::fs::canonicalize(&args.input)
        .with_context(|| format!("opening input {}", args.input.display()))?;
    let mut destinations = Vec::new();
    for path in std::iter::once(&output)
        .chain(args.report.iter())
        .chain(args.save_tuning.iter())
    {
        let resolved = std::fs::canonicalize(path).unwrap_or(std::path::absolute(path)?);
        ensure!(
            resolved != input_path,
            "output paths must not overwrite the input"
        );
        ensure!(
            !destinations.contains(&resolved),
            "model, report and tuning outputs must use different paths"
        );
        destinations.push(resolved);
    }
    let assets = Assets::new(args.asset_cache, args.offline, args.asset_map.as_deref())?;
    let source = import::load(&args.input, &assets)?;
    let source_name = args
        .input
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    if args.window {
        #[cfg(feature = "window")]
        {
            return partify::window::open(
                source,
                t,
                output,
                source_name,
                args.report,
                args.save_tuning,
            );
        }
        #[cfg(not(feature = "window"))]
        {
            anyhow::bail!(
                "this build has no window support; install the standard release or rebuild with default features"
            );
        }
    }
    let build = pipeline::run(&source, &t, &AtomicBool::new(false), |s| eprintln!("{s}"))?;
    export::save(&build, &output, &source_name)?;
    if let Some(p) = args.report {
        export::atomic_write(&p, &serde_json::to_vec_pretty(&build)?)?
    }
    if let Some(p) = args.save_tuning {
        export::atomic_write(&p, &serde_json::to_vec_pretty(&t)?)?
    }
    for warning in &build.warnings {
        eprintln!("Warning: {warning}")
    }
    println!(
        "{} parts / {} budget; {:.1}% sample coverage; {:.2}% sampled overlap\n{}",
        build.parts.len(),
        t.budget,
        build.metrics.coverage * 100.,
        build.metrics.sampled_overlap * 100.,
        output.display()
    );
    Ok(())
}
