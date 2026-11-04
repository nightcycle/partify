use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Every field is persisted on the output Model, including headless runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tuning {
    pub budget: usize,
    pub size: f32,
    pub image_resolution: u32,
    pub sample_limit: usize,
    pub alpha_threshold: f32,
    pub color_levels: u32,
    pub color_threshold: f32,
    pub edge_angle: f32,
    pub thickness: f32,
    pub shell_thickness: f32,
    pub solid: bool,
    pub min_size: f32,
    pub split_candidates: usize,
    pub fit_samples: usize,
    pub probes: usize,
    pub refinement_passes: usize,
    pub coverage_weight: f32,
    pub spill_weight: f32,
    pub color_weight: f32,
    pub edge_weight: f32,
    pub overlap_weight: f32,
    pub connection_weight: f32,
    pub complexity_penalty: f32,
    pub allow_meshes: bool,
    pub studs: bool,
    pub seed: u32,
}
impl Default for Tuning {
    fn default() -> Self {
        Self {
            budget: 200,
            size: 32.,
            image_resolution: 128,
            sample_limit: 12000,
            alpha_threshold: 0.1,
            color_levels: 16,
            color_threshold: 0.15,
            edge_angle: 35.,
            thickness: 0.5,
            shell_thickness: 0.4,
            solid: true,
            min_size: 0.05,
            split_candidates: 3,
            fit_samples: 192,
            probes: 48,
            refinement_passes: 2,
            coverage_weight: 4.,
            spill_weight: 2.,
            color_weight: 3.,
            edge_weight: 1.,
            overlap_weight: 6.,
            connection_weight: 0.15,
            complexity_penalty: 0.001,
            allow_meshes: true,
            studs: false,
            seed: 1,
        }
    }
}
impl Tuning {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=10000).contains(&self.budget),
            "budget must be 1..10000"
        );
        ensure!(
            (8..=1024).contains(&self.image_resolution),
            "image_resolution must be 8..1024"
        );
        ensure!(
            (128..=250000).contains(&self.sample_limit),
            "sample_limit must be 128..250000"
        );
        ensure!(
            (2..=256).contains(&self.color_levels),
            "color_levels must be 2..256"
        );
        ensure!(
            (1..=9).contains(&self.split_candidates),
            "split_candidates must be 1..9"
        );
        ensure!(
            (16..=4096).contains(&self.fit_samples),
            "fit_samples must be 16..4096"
        );
        ensure!(
            (16..=1024).contains(&self.probes),
            "probes must be 16..1024"
        );
        ensure!(
            self.refinement_passes <= 20,
            "refinement_passes must be 0..20"
        );
        for (name, value) in [
            ("size", self.size),
            ("thickness", self.thickness),
            ("shell_thickness", self.shell_thickness),
            ("min_size", self.min_size),
        ] {
            ensure!(
                value.is_finite() && (0.001..=2048.).contains(&value),
                "{name} must be finite and in 0.001..2048"
            );
        }
        ensure!(
            self.min_size <= self.size && self.min_size <= self.thickness,
            "min_size must not exceed size or thickness"
        );
        for (name, value) in [
            ("alpha_threshold", self.alpha_threshold),
            ("color_threshold", self.color_threshold),
        ] {
            ensure!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "{name} must be 0..1"
            );
        }
        ensure!(
            self.edge_angle.is_finite() && (0.0..=180.0).contains(&self.edge_angle),
            "edge_angle must be 0..180"
        );
        for (name, value) in [
            ("coverage_weight", self.coverage_weight),
            ("spill_weight", self.spill_weight),
            ("color_weight", self.color_weight),
            ("edge_weight", self.edge_weight),
            ("overlap_weight", self.overlap_weight),
            ("connection_weight", self.connection_weight),
            ("complexity_penalty", self.complexity_penalty),
        ] {
            ensure!(
                value.is_finite() && (0.0..=100.0).contains(&value),
                "{name} must be finite and in 0..100"
            );
        }
        ensure!(
            self.coverage_weight > 0.,
            "coverage_weight must be positive"
        );
        Ok(())
    }
}
