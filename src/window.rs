use crate::{
    config::Tuning,
    export,
    geometry::{Bounds, Shape},
    import::Source,
    pipeline::{self, Build, Part},
};
use anyhow::Result;
use eframe::egui::{self, Color32, Pos2, Stroke, Vec2};
use glam::{Mat3, Vec3};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

enum Event {
    Progress(String),
    Done(Box<Result<Build>>),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Layer {
    Source,
    Features,
    Volumes,
    Connections,
    Parts,
}
struct Viewer {
    source: Arc<Source>,
    tuning: Tuning,
    build: Option<Build>,
    output: PathBuf,
    source_name: String,
    report: Option<PathBuf>,
    preset: Option<PathBuf>,
    rx: Option<mpsc::Receiver<Event>>,
    cancel: Arc<AtomicBool>,
    status: String,
    layer: Layer,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: Vec2,
    wire: bool,
    auto_rebuild: bool,
    rebuild_pending: bool,
}
pub fn open(
    source: Source,
    tuning: Tuning,
    output: PathBuf,
    source_name: String,
    report: Option<PathBuf>,
    preset: Option<PathBuf>,
) -> Result<()> {
    let flat = matches!(source, Source::Image(_));
    let mut app = Viewer {
        source: Arc::new(source),
        tuning,
        build: None,
        output,
        source_name,
        report,
        preset,
        rx: None,
        cancel: Arc::new(AtomicBool::new(false)),
        status: "Preparing".into(),
        layer: Layer::Parts,
        yaw: if flat { 0. } else { 0.65 },
        pitch: if flat { 0. } else { -0.35 },
        zoom: 1.,
        pan: Vec2::ZERO,
        wire: false,
        auto_rebuild: false,
        rebuild_pending: false,
    };
    // eframe's screenshot harness captures the second frame. Finish the initial
    // build first in that mode so visual regression captures show the result.
    if std::env::var_os("EFRAME_SCREENSHOT_TO").is_some() {
        let b = pipeline::run(&app.source, &app.tuning, &app.cancel, |_| {})?;
        app.status = format!("Ready · {} parts", b.parts.len());
        app.build = Some(b);
    } else {
        app.start();
    }
    eframe::run_native(
        "Partify • Roblox part builder",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280., 850.])
                .with_min_inner_size([900., 600.]),
            ..Default::default()
        },
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("window: {e}"))
}
impl Drop for Viewer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed)
    }
}
impl Viewer {
    fn start(&mut self) {
        if self.rx.is_some() {
            self.cancel.store(true, Ordering::Relaxed);
            self.rebuild_pending = true;
            return;
        }
        if let Err(e) = self.tuning.validate() {
            self.status = e.to_string();
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let source = self.source.clone();
        let t = self.tuning.clone();
        self.status = "Starting rebuild…".into();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                pipeline::run(&source, &t, &cancel, |s| {
                    let _ = tx.send(Event::Progress(s.into()));
                })
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("build worker panicked")));
            let _ = tx.send(Event::Done(Box::new(result)));
        });
    }
    fn poll(&mut self) {
        let mut done = false;
        if let Some(rx) = &self.rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    Event::Progress(s) => self.status = s,
                    Event::Done(result) => {
                        done = true;
                        match *result {
                            Ok(b) => {
                                self.status = format!("Ready · {} parts", b.parts.len());
                                self.build = Some(b)
                            }
                            Err(e) => self.status = format!("{e:#}"),
                        }
                    }
                }
            }
        }
        if done {
            self.rx = None;
            if self.rebuild_pending {
                self.rebuild_pending = false;
                self.start()
            }
        }
    }
    fn save(&mut self) {
        let result = (|| -> Result<()> {
            let b = self
                .build
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("rebuild first"))?;
            export::save(b, &self.output, &self.source_name)?;
            if let Some(p) = &self.report {
                export::atomic_write(p, &serde_json::to_vec_pretty(b)?)?
            }
            if let Some(p) = &self.preset {
                export::atomic_write(p, &serde_json::to_vec_pretty(&b.tuning)?)?
            }
            Ok(())
        })();
        self.status = match result {
            Ok(()) => format!("Saved {}", self.output.display()),
            Err(e) => format!("Save failed: {e:#}"),
        };
    }
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("PARTIFY");
        ui.label(&self.source_name);
        ui.add_space(12.);
        let mut value = serde_json::to_value(&self.tuning).unwrap();
        let mut changed = false;
        for (title, fields) in [
            (
                "1 · Features",
                &[
                    "image_resolution",
                    "sample_limit",
                    "alpha_threshold",
                    "color_levels",
                    "color_threshold",
                    "edge_angle",
                    "seed",
                ][..],
            ),
            (
                "2 · Volumes",
                &["size", "thickness", "shell_thickness", "solid", "min_size"][..],
            ),
            (
                "3 · Fit",
                &[
                    "budget",
                    "split_candidates",
                    "fit_samples",
                    "probes",
                    "refinement_passes",
                    "coverage_weight",
                    "spill_weight",
                    "color_weight",
                    "edge_weight",
                    "overlap_weight",
                    "connection_weight",
                    "complexity_penalty",
                    "allow_meshes",
                    "studs",
                ][..],
            ),
        ] {
            egui::CollapsingHeader::new(title)
                .default_open(true)
                .show(ui, |ui| {
                    for &field in fields {
                        ui.horizontal(|ui| {
                            let label = field.replace('_', " ");
                            ui.label(label);
                            match &mut value[field] {
                                serde_json::Value::Bool(v) => {
                                    changed |= ui.checkbox(v, "").changed()
                                }
                                v => {
                                    let old = v.as_f64().unwrap();
                                    let mut number = old;
                                    let (min, max, step, integer) = field_range(field);
                                    changed |= ui
                                        .add(
                                            egui::DragValue::new(&mut number)
                                                .range(min..=max)
                                                .speed(step)
                                                .max_decimals(if integer { 0 } else { 4 }),
                                        )
                                        .changed();
                                    *v = if integer {
                                        serde_json::json!(number.round() as u64)
                                    } else {
                                        serde_json::json!(number)
                                    };
                                }
                            }
                        });
                    }
                });
        }
        if changed && let Ok(t) = serde_json::from_value(value) {
            self.tuning = t;
            if self.auto_rebuild {
                self.start()
            }
        }
        ui.add_space(10.);
        ui.checkbox(&mut self.auto_rebuild, "Rebuild when values change");
        ui.horizontal(|ui| {
            if ui.button("Rebuild").clicked() {
                self.start()
            }
            if self.rx.is_some() && ui.button("Cancel").clicked() {
                self.rebuild_pending = false;
                self.cancel.store(true, Ordering::Relaxed)
            }
        });
        let current =
            self.build.as_ref().is_some_and(|b| b.tuning == self.tuning) && self.rx.is_none();
        if ui
            .add_enabled(current, egui::Button::new("Export RBXM + tuning"))
            .clicked()
        {
            self.save()
        }
        if !current && self.build.is_some() {
            ui.colored_label(
                Color32::YELLOW,
                "Preview uses the last completed build. Rebuild to export these settings.",
            );
        }
        ui.label(self.output.display().to_string());
        if let Some(b) = &self.build {
            ui.separator();
            ui.label(format!("{} / {} parts", b.parts.len(), b.tuning.budget));
            ui.label(format!("Coverage: {:.1}%", b.metrics.coverage * 100.));
            ui.label(format!("Color RMSE: {:.3}", b.metrics.mean_color_error));
            ui.label(format!(
                "Sampled overlap: {:.3}%",
                b.metrics.sampled_overlap * 100.
            ));
            for warning in &b.warnings {
                ui.colored_label(Color32::YELLOW, warning);
            }
        }
    }
    fn viewport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (layer, name) in [
                (Layer::Source, "Source"),
                (Layer::Features, "Features"),
                (Layer::Volumes, "Volumes"),
                (Layer::Connections, "Connections"),
                (Layer::Parts, "Parts"),
            ] {
                ui.selectable_value(&mut self.layer, layer, name);
            }
            ui.separator();
            ui.checkbox(&mut self.wire, "Wireframe");
            if ui.button("Front").clicked() {
                self.yaw = 0.;
                self.pitch = 0.;
                self.pan = Vec2::ZERO
            }
            if ui.button("Orbit").clicked() {
                self.yaw = 0.65;
                self.pitch = -0.35;
                self.pan = Vec2::ZERO
            }
        });
        ui.label("Drag to orbit · right-drag to pan · scroll to zoom");
        let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::drag());
        let rect = response.rect;
        if response.dragged_by(egui::PointerButton::Primary) {
            let delta = ui.input(|i| i.pointer.delta());
            self.yaw += delta.x * 0.008;
            self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.55, 1.55)
        }
        if response.dragged_by(egui::PointerButton::Secondary) {
            self.pan += ui.input(|i| i.pointer.delta())
        }
        if response.hovered() {
            self.zoom =
                (self.zoom * ui.input(|i| i.smooth_scroll_delta.y * 0.001).exp()).clamp(0.1, 10.)
        }
        painter.rect_filled(rect, 0., Color32::from_rgb(18, 22, 30));
        let camera = Mat3::from_rotation_x(self.pitch) * Mat3::from_rotation_y(self.yaw);
        let scale = rect.width().min(rect.height()) * 0.72 / self.tuning.size * self.zoom;
        let project = |p: Vec3| -> (Pos2, f32) {
            let p = camera * p;
            (rect.center() + self.pan + Vec2::new(p.x, -p.y) * scale, p.z)
        };
        let Some(build) = &self.build else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                &self.status,
                egui::FontId::proportional(20.),
                Color32::WHITE,
            );
            return;
        };
        match self.layer {
            Layer::Source | Layer::Features => {
                let mut points: Vec<_> = build
                    .samples
                    .iter()
                    .step_by((build.samples.len() / 25000).max(1))
                    .map(|s| {
                        let (p, z) = project(s.position);
                        (z, p, s)
                    })
                    .collect();
                points.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, p, s) in points {
                    let c = if self.layer == Layer::Features {
                        if s.edge > 0.5 {
                            Color32::WHITE
                        } else {
                            region_color(s.region)
                        }
                    } else {
                        rgb(s.color, 255)
                    };
                    painter.circle_filled(
                        p,
                        if self.layer == Layer::Features && s.edge > 0.5 {
                            2.2
                        } else {
                            1.8
                        },
                        c,
                    );
                }
            }
            Layer::Parts => {
                let mut faces = Vec::new();
                for p in &build.parts {
                    for tri in p.triangles() {
                        let normal = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or_zero();
                        let light =
                            0.6 + 0.4 * normal.dot(Vec3::new(-0.3, 0.8, 0.5).normalize()).abs();
                        let points = tri.map(&project);
                        let depth = points.iter().map(|p| p.1).sum::<f32>() / 3.;
                        faces.push((depth, points.map(|p| p.0), rgb(p.color * light, 255)));
                    }
                }
                faces.sort_by(|a, b| a.0.total_cmp(&b.0));
                for (_, points, c) in faces {
                    painter.add(egui::Shape::convex_polygon(
                        points.to_vec(),
                        if self.wire { Color32::TRANSPARENT } else { c },
                        if self.wire {
                            Stroke::new(0.6, c)
                        } else {
                            Stroke::NONE
                        },
                    ));
                }
                // Delineate each brick without drawing the internal triangle diagonals.
                for p in &build.parts {
                    if p.shape == Shape::Block {
                        draw_box(
                            &painter,
                            p,
                            project,
                            Stroke::new(0.6, Color32::from_black_alpha(100)),
                        );
                    }
                }
            }
            Layer::Volumes | Layer::Connections => {
                for (i, v) in build.volumes.iter().enumerate() {
                    if !v.footprint.is_empty() {
                        let stroke = Stroke::new(1., rgb(v.color, 150));
                        for j in 0..v.footprint.len() {
                            let a = v.footprint[j];
                            let b = v.footprint[(j + 1) % v.footprint.len()];
                            for z in [v.bounds.min.z, v.bounds.max.z] {
                                painter.line_segment(
                                    [
                                        project(build.frame * a.extend(z)).0,
                                        project(build.frame * b.extend(z)).0,
                                    ],
                                    stroke,
                                );
                            }
                            painter.line_segment(
                                [
                                    project(build.frame * a.extend(v.bounds.min.z)).0,
                                    project(build.frame * a.extend(v.bounds.max.z)).0,
                                ],
                                stroke,
                            );
                        }
                        continue;
                    }
                    let p = Part {
                        shape: Shape::Block,
                        center: build.frame * v.bounds.center(),
                        size: v.bounds.size(),
                        rotation: build.frame,
                        color: v.color,
                        loss: 0.,
                        volume: i,
                    };
                    draw_box(&painter, &p, project, Stroke::new(1., rgb(v.color, 150)));
                }
                if self.layer == Layer::Connections {
                    for c in &build.connections {
                        let (p, _) = project(c.center);
                        let (q, _) = project(c.center + c.normal * build.tuning.size * 0.035);
                        let color = if c.touching {
                            Color32::from_rgb(75, 235, 170)
                        } else {
                            Color32::from_rgb(255, 180, 65)
                        };
                        painter.circle_filled(p, 3., color);
                        painter.line_segment([p, q], Stroke::new(2., color));
                    }
                }
            }
        }
        painter.text(
            rect.left_bottom() + Vec2::new(14., -12.),
            egui::Align2::LEFT_BOTTOM,
            format!(
                "{} parts · {} regions · {} connection surfaces",
                build.parts.len(),
                build.metrics.regions,
                build.connections.len()
            ),
            egui::FontId::monospace(12.),
            Color32::LIGHT_GRAY,
        );
    }
}
impl eframe::App for Viewer {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if self.rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.rx.is_some() {
                    ui.spinner();
                }
                ui.label(&self.status);
            });
        });
        egui::SidePanel::left("tuning")
            .resizable(true)
            .default_width(310.)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui));
            });
        egui::CentralPanel::default().show(ctx, |ui| self.viewport(ui));
    }
}
fn field_range(s: &str) -> (f64, f64, f64, bool) {
    match s {
        "budget" => (1., 10000., 1., true),
        "image_resolution" => (8., 1024., 1., true),
        "sample_limit" => (128., 250000., 100., true),
        "color_levels" => (2., 256., 1., true),
        "split_candidates" => (1., 9., 1., true),
        "fit_samples" => (16., 4096., 16., true),
        "probes" => (16., 1024., 8., true),
        "refinement_passes" => (0., 20., 1., true),
        "seed" => (0., u32::MAX as f64, 1., true),
        "edge_angle" => (0., 180., 1., false),
        "alpha_threshold" | "color_threshold" => (0., 1., 0.01, false),
        "size" => (0.001, 2048., 1., false),
        "thickness" | "shell_thickness" | "min_size" => (0.001, 2048., 0.01, false),
        "complexity_penalty" => (0., 100., 0.0001, false),
        _ => (0., 100., 0.05, false),
    }
}
fn rgb(c: Vec3, a: u8) -> Color32 {
    let c = c.clamp(Vec3::ZERO, Vec3::ONE) * 255.;
    Color32::from_rgba_unmultiplied(c.x as u8, c.y as u8, c.z as u8, a)
}
fn region_color(id: u32) -> Color32 {
    let n = id.wrapping_mul(2654435761);
    Color32::from_rgb(
        70 + (n & 127) as u8,
        70 + ((n >> 8) & 127) as u8,
        70 + ((n >> 16) & 127) as u8,
    )
}
fn draw_box(
    painter: &egui::Painter,
    part: &Part,
    project: impl Fn(Vec3) -> (Pos2, f32),
    stroke: Stroke,
) {
    let b = Bounds {
        min: Vec3::splat(-0.5),
        max: Vec3::splat(0.5),
    };
    let corners: Vec<_> = (0..8)
        .map(|i| {
            let q = Vec3::new(
                if i & 1 == 0 { b.min.x } else { b.max.x },
                if i & 2 == 0 { b.min.y } else { b.max.y },
                if i & 4 == 0 { b.min.z } else { b.max.z },
            );
            project(part.center + part.rotation * (part.size * q)).0
        })
        .collect();
    for i in 0..8 {
        for bit in [1, 2, 4] {
            if i & bit == 0 {
                painter.line_segment([corners[i], corners[i | bit]], stroke);
            }
        }
    }
}
