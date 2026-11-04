# Partify

A Rust CLI that reconstructs images and models as colored Roblox BaseParts within a hard part-count budget. It searches blocks, balls, cylinders, wedges, corner wedges, and the three supplied MeshParts. Outputs are anchored Models with the complete tuning baked into attributes.

## Run

Build locally with Rust 1.90+ and a C/C++ build toolchain:

```sh
cargo build --locked --release
cargo run --release -- image.png --budget 200 --output image.rbxm
cargo run --release -- model.fbx --budget 300 --window --output model.rbxm
```

On Windows the built executable is `target/release/partify.exe`. The standard build includes the egui window. For a server or a build without window dependencies, use `cargo build --release --no-default-features`.

After publishing the first GitHub release, install through [Rokit](https://github.com/rojo-rbx/rokit):

```sh
rokit add nightcycle/partify@0.1.0
partify model.obj --budget 250 --window
```

Equivalent entry in a consuming project's `rokit.toml`:

```toml
[tools]
partify = "nightcycle/partify@0.1.0"
```

The release workflow and packaging are included; this checkout does not itself publish a release.

## Inputs and colors

| Input | Geometry and color |
| --- | --- |
| PNG, JPG, JPEG | Aspect-preserving image in the XY plane, +Y up. PNG transparency becomes empty space. RGB colors become part colors. |
| OBJ | Polygon triangulation, multiple objects, vertex colors, MTL diffuse colors and PNG/JPEG diffuse textures with UVs. Keep the MTL and textures beside the model. |
| FBX | ASCII and binary FBX via ufbx; mesh instances, world transforms, diffuse/base colors, vertex colors, embedded or external PNG/JPEG textures. Imports the static authored pose. |
| RBXM, RBXMX | Nested Models/Folders, native parts and their shape variants, MeshParts, file SpecialMeshes, world CFrames, Color, TextureID and SurfaceAppearance color maps. FileMesh versions 1–7, including Draco geometry. |

The longest source dimension is normalized to `--size` studs (default 32); output is centered at the origin. Image extrusion defaults to 0.5 studs. Existing source positions relative to one another are preserved before normalization.

Roblox assets referenced by a model are separate from the RBXM container. Partify caches public assets in `.partify-cache/<id>.bin`. Use `--offline` to forbid downloads. For private/restricted mesh or texture assets, provide an explicit local map:

```json
{
  "rbxassetid://123456": "meshes/123456.mesh",
  "rbxassetid://789012": "textures/789012.png"
}
```

```sh
partify input.rbxm --budget 250 --asset-map assets.json --offline
```

Map paths are relative to the JSON file. An OBJ can substitute for a mesh asset. The three supplied templates are embedded in the executable, so fitting them requires no network access. Their output MeshIds remain Roblox asset references and must be accessible in the destination experience.

## Algorithm

1. **Features.** Downsample images with alpha preservation and spatially order pixels before subsampling so resolution and sample-limit changes do not create stripes; sample mesh surfaces by triangle area and closed interiors with a deterministic Halton sequence. Sample diffuse textures through UVs. Identify color groups and local color/normal discontinuities, including curved surfaces and sharp edges. Feature importance controls fitting error. The neighborhood radius comes from source sampling, independently of the volume minimum size.
2. **Volumes.** Compare source axes, a principal-axis frame, and a dominant face frame by fitting quality. Use exact source bounds alongside sampled bounds so sparsely sampled corners survive. Propose adaptive binary partitions at spatial quantiles and strong feature boundaries. Image volumes also consider diagonal cuts across their bounds, allowing complementary wedges to share sloping faces. Every split establishes a pair of planar connection surfaces. Volumes vary in dimensions and placement; there is no output voxel lattice.
3. **Fitting.** Compare allowed shapes in the 24 proper axis orientations. Score source coverage, edge-weighted coverage, color error, occupied geometry outside the target, and support of required connection faces. Validate the best candidates against every sample in the volume. Spend each additional part on the split with the largest total improvement. Shrink/translate candidates with coordinate descent to refine the fit.

Image volumes retain convex polygon footprints, and each part's actual geometry must stay inside its volume, including during refinement. Diagonal neighbors may have overlapping bounding boxes while their solid interiors stay disjoint. Mesh volumes retain the bounding-box containment constraint. The overlap metric counts samples strictly inside multiple parts and excludes shared faces within numerical tolerance. The budget is an upper bound: the search stops when another split does not improve the objective enough to pay its complexity penalty. Image footprints are included in reports and baked as `PartifyVolumeFootprint` attributes in the partition frame.

Closed manifold meshes use inside/outside ray tests accelerated by a BVH. Open or non-manifold meshes automatically use a surface shell and report a warning; `--shell` forces that behavior. Connection surfaces describe intended contact and whether the fitted geometry reaches the face center. They do not create welds or guarantee a mechanically connected build. All output parts are anchored.

This is a deterministic geometric heuristic, not semantic object recognition or a guarantee of an optimal reconstruction. Small details require more samples and budget. The common partition frame limits arbitrary local orientations; increasing the budget can approximate such details with smaller parts.

## Tuning and layers

```sh
partify image.png --budget 120 --size 48 --thickness 0.4 --resolution 192 --window
partify model.obj --budget 300 --samples 24000 --set spill_weight=4 --set edge_angle=25
partify image.png --budget 80 --save-tuning preset.json --report layers.json
partify another.png --tuning-from preset.json
partify another.obj --tuning-from previous.rbxm --budget 180
```

`--set KEY=JSON` accepts any field below, including booleans. CLI flags override loaded tuning, then `--set` overrides are applied. Invalid and unknown fields fail with an error.

The window exposes every tuning field in **Features**, **Volumes**, and **Fit** controls. **Rebuild** runs on a background thread with progress and cancellation; optional automatic rebuilds cancel the previous job. The viewer has **Source**, **Features**, **Volumes**, **Connections**, and **Parts** layers, orbit/pan/zoom, front view, and wireframe. Features show color groups with edge samples in white. Connections use green for contact and amber for a gap. Export is disabled while settings differ from the preview, so the saved geometry and attributes always describe the same completed build. Window mode exports only when **Export RBXM + tuning** is clicked.

| Stage | Fields |
| --- | --- |
| Features | `image_resolution`, `sample_limit`, `alpha_threshold`, `color_levels`, `color_threshold`, `edge_angle`, `seed` |
| Volumes | `size`, `thickness`, `shell_thickness`, `solid`, `min_size` |
| Search | `budget`, `split_candidates`, `fit_samples`, `probes`, `refinement_passes` |
| Scoring | `coverage_weight`, `spill_weight`, `color_weight`, `edge_weight`, `connection_weight`, `complexity_penalty` |
| Output and diagnostics | `allow_meshes`, `studs`, `overlap_weight` |

`overlap_weight` weights residual sampled overlap in the reported total loss. The partition constraint already prevents volumetric overlaps, so it does not relax collision constraints. `color_levels` controls feature-group visualization; `color_threshold` controls color-edge detection. Original RGB colors remain the fitting target. `--no-meshes` restricts output to native parts. `--studs` enables the classic top surface on parts.

Every Model stores `PartifyTuning` (complete JSON), individual `PartifyBudget`, `PartifyCoverageWeight`, etc. attributes, `PartifyVersion`, `PartifySource`, `PartifyMetrics`, `PartifyVolumeFrame`, and warnings. Each BasePart stores its shape, volume bounds, fit loss, and JSON connection surfaces. Volume bounds are local to `PartifyVolumeFrame`; connection centers and normals are in output model space. `--report` writes samples, feature labels, volumes, connections, shapes, metrics and tuning as JSON. Reports can be large.

## Boundaries

- The supplied triangle templates use idealized convex proxies derived from their embedded collision hulls. The corner triangle's original hull differs slightly from an ideal tetrahedron. The quad is a surface asset; it is intended for flat image faces, not solid 3D volume filling. The original MeshIds and collision payloads are retained in output.
- UnionOperation/CSG, Terrain, TrussPart and non-file SpecialMeshes are rejected or excluded as appropriate; export those as OBJ/FBX or convert to MeshParts first. Scripts, constraints, lights, animations and other nongeometry instances are not copied. Part transparency below 1 is treated as opaque color; fully transparent parts are skipped.
- Texture support is diffuse/base-color PNG/JPEG with the first UV set; procedural shaders, decals, PBR lighting effects, texture transforms, alpha cutout meshes, skin animation and morph deformation are not reproduced. Raster alpha is supported.
- Mesh color is sampled from the nearest surface when filling interiors. Intersecting closed shells can make ray parity ambiguous; use `--shell` or clean the source geometry. Increase `sample_limit` for thin details.
- Large budgets increase search and visualization cost. Begin with 50–300 parts and tune coverage/spill before raising the budget.

## Development and release

Run the full checks and CLI format tests with Bash (including Git Bash on Windows):

```sh
bash scripts/test.sh
bash scripts/test.sh --input examples/inputs/tree.rbxm --window
bash scripts/test.sh --budget 200 "examples/inputs/fish.jpg"
bash scripts/test.sh --offline --window "C:/Models/my model.obj"
bash scripts/test.sh --asset-map assets.json "C:/Models" "C:/Images/icon.png"
```

The script accepts any number of files or folders in the supported formats, scans folders recursively, and defaults to the current contents of `examples/inputs`. Paths are relative to the directory you invoke it from. Input files are never regenerated. Each run creates a fresh folder under `target/test-run/` containing separate model, tuning and report files, including files with identical names from different folders. Previous outputs can safely be used as new inputs. A failed conversion is reported by filename and the remaining inputs still run; any failures make the script exit unsuccessfully.

To test and preview a specific model or image, pass `--input PATH --window` (a positional path also works). Only the selected user inputs and the built-in reference fixtures are converted; the other files in `examples/inputs` are skipped. Repeat `--input` to select several files. The window opens the first successful `--input` selection, followed by positional inputs if needed. For example, `bash scripts/test.sh --offline --input examples/inputs/fish.jpg --window` previews the fish image using cached dependencies.

`--budget` defaults to 24 and `--samples` to 1024 for quick smoke tests. User inputs have no minimum coverage requirement unless you set `--min-coverage 0.9`; a small budget may intentionally produce a rough approximation. Budget, finite metrics, and overlap checks always apply. The owned FBX/RBXM reference fixtures retain their quality assertions.

`--offline` disables both Cargo and Roblox downloads. For models with referenced meshes or textures, run once without `--offline` to populate the cache before testing offline. Use `--asset-map FILE` or `--asset-cache DIR` for existing local mesh/texture assets. The fixed reference cases always run offline. `--window` opens the first successfully converted user input, even if other conversions fail. Failed conversions still produce a nonzero exit status after the window closes; if no user inputs succeed, no window opens. Individual checks:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
python -m unittest discover -s scripts -p "test_*.py"
cargo run --example fixtures
cargo run --release -- examples/inputs/house.png --budget 24 --window
```

On Linux, native window builds need X11/Wayland/OpenGL development libraries; the CI workflow lists the packages. The headless feature configuration avoids those dependencies. The egui screenshot harness supports `EFRAME_SCREENSHOT_TO=preview.png`: it builds the initial result, captures the native window, and exits.

CI checks formatting, Clippy, tests, the headless build and an image conversion on Windows, Linux and macOS. Push a `v0.1.0` tag matching `Cargo.toml` and `Cargo.lock` to run the release workflow, or dispatch it with an existing tag. It tests the resolved tagged commit, then builds Windows x86-64, Linux x86-64/ARM64, and macOS x86-64/ARM64 archives. Each ZIP has the executable at its root, matching the Rokit conventions used by `imposter-baker` and `roblox-mesh-universal-standard`. The templates are compiled into the executable. All five archives and `SHA256SUMS` are published together after successful checks; existing published releases are not overwritten.
