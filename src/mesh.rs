//! Static geometry in Roblox's chunked FileMesh v6/v7 containers.
use crate::geometry::Triangle;
use anyhow::{Context, Result, bail, ensure};
use binrw::BinReaderExt;
use glam::{Vec2, Vec3};
use rbx_mesh::mesh::{Face2, Vertex2, VertexId2};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read},
};

fn count(r: &mut Cursor<&[u8]>) -> Result<usize> {
    let n = r.read_le::<u32>()? as usize;
    ensure!(n <= 2_000_000, "mesh count exceeds safety limit");
    Ok(n)
}
fn bytes<'a>(r: &mut Cursor<&'a [u8]>, n: usize) -> Result<&'a [u8]> {
    let start = r.position() as usize;
    let data = r
        .get_ref()
        .get(start..start.checked_add(n).context("mesh size overflow")?)
        .context("truncated mesh chunk")?;
    r.set_position((start + n) as u64);
    Ok(data)
}
pub fn chunked(data: &[u8]) -> Result<Vec<Triangle>> {
    let mut r = Cursor::new(data.get(13..).context("truncated mesh header")?);
    let mut chunks = BTreeMap::new();
    while (r.position() as usize) < r.get_ref().len() {
        let tag = std::str::from_utf8(bytes(&mut r, 8)?)?
            .trim_end_matches('\0')
            .to_owned();
        let version = r.read_le::<u32>()?;
        let n = r.read_le::<u32>()? as usize;
        let content = bytes(&mut r, n)?;
        ensure!(
            chunks.insert(tag.clone(), (version, content)).is_none(),
            "duplicate mesh chunk {tag}"
        );
    }
    let (version, core) = chunks.remove("COREMESH").context("missing COREMESH")?;
    let mut r = Cursor::new(core);
    let (vertices, faces) = match version {
        1 => {
            let n = count(&mut r)?;
            ensure!(n <= core.len() / 40, "truncated vertices");
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(r.read_le::<Vertex2>()?)
            }
            let n = count(&mut r)?;
            ensure!(n <= core.len() / 12, "truncated faces");
            let mut f = Vec::with_capacity(n);
            for _ in 0..n {
                f.push(r.read_le::<Face2>()?)
            }
            (v, f)
        }
        2 => {
            let n = r.read_le::<u32>()? as usize;
            draco(bytes(&mut r, n)?)?
        }
        _ => bail!("unsupported COREMESH version {version}"),
    };
    ensure!(
        r.position() as usize == core.len(),
        "trailing COREMESH bytes"
    );
    let mut end = faces.len();
    if let Some((version, data)) = chunks.get("LODS") {
        ensure!(*version == 1, "unsupported LODS version");
        let mut r = Cursor::new(*data);
        let mut header = [0; 3];
        r.read_exact(&mut header)?;
        let n = count(&mut r)?;
        for i in 0..n {
            let offset = r.read_le::<u32>()? as usize;
            if i == 1 && offset > 0 {
                end = offset
            }
        }
    }
    ensure!(end <= faces.len(), "invalid mesh LOD range");
    let mut out = Vec::new();
    for f in faces.iter().take(end) {
        let mut t = Triangle {
            p: [Vec3::ZERO; 3],
            colors: [Vec3::ONE; 3],
            uv: [Vec2::ZERO; 3],
            texture: None,
        };
        for j in 0..3 {
            let v = vertices
                .get(f.0[j].0 as usize)
                .context("mesh index out of bounds")?;
            t.p[j] = Vec3::from_array(v.pos);
            t.uv[j] = Vec2::from_array(v.tex);
            t.colors[j] = Vec3::new(v.color[0] as f32, v.color[1] as f32, v.color[2] as f32) / 255.;
        }
        out.push(t)
    }
    Ok(out)
}
fn draco(bytes: &[u8]) -> Result<(Vec<Vertex2>, Vec<Face2>)> {
    use draco_core::{DataType, DecoderBuffer, Mesh, MeshDecoder, PointIndex};
    let mut mesh = Mesh::new();
    MeshDecoder::new().decode(&mut DecoderBuffer::new(bytes), &mut mesh)?;
    ensure!(
        mesh.num_points() <= 2_000_000 && mesh.faces().len() <= 2_000_000,
        "Draco mesh exceeds safety limit"
    );
    let mut vertices = Vec::new();
    for i in 0..mesh.num_points() {
        let read = |id: u32, components: usize, byte: bool| -> Result<Vec<f32>> {
            let a = mesh
                .attribute_by_unique_id(id)
                .with_context(|| format!("missing Draco attribute {id}"))?;
            ensure!(
                a.num_components() as usize == components,
                "Draco component count mismatch"
            );
            ensure!(
                a.data_type()
                    == if byte {
                        DataType::Uint8
                    } else {
                        DataType::Float32
                    },
                "unsupported Draco attribute type"
            );
            let index = a.mapped_index(PointIndex(i as u32));
            let offset = index.0 as usize * a.byte_stride() as usize;
            let len = components * if byte { 1 } else { 4 };
            let data = a
                .buffer()
                .data()
                .get(offset..offset + len)
                .context("invalid Draco attribute range")?;
            Ok(if byte {
                data.iter().map(|v| *v as f32).collect()
            } else {
                data.chunks_exact(4)
                    .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
                    .collect()
            })
        };
        let pos = read(0, 3, false)?.try_into().unwrap();
        let tex = if mesh.attribute_by_unique_id(2).is_some() {
            read(2, 2, false)?.try_into().unwrap()
        } else {
            [0.; 2]
        };
        let color = if mesh.attribute_by_unique_id(4).is_some() {
            let c = read(4, 4, true)?;
            [c[0] as u8, c[1] as u8, c[2] as u8, c[3] as u8]
        } else {
            [255; 4]
        };
        vertices.push(Vertex2 {
            pos,
            norm: [0.; 3],
            tex,
            tangent: [0; 4],
            color,
        });
    }
    let faces = mesh
        .faces()
        .iter()
        .map(|f| Face2(f.map(|i| VertexId2(i.0))))
        .collect();
    Ok((vertices, faces))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(version: u32, data: &[u8], container: u32) -> Vec<u8> {
        let mut out = format!("version {container}.00\n").into_bytes();
        out.extend(b"COREMESH");
        out.extend(version.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }
    #[test]
    fn raw_chunked_geometry_and_malformed_sizes() {
        use binrw::BinWriterExt;
        let mut core = Cursor::new(Vec::new());
        core.write_le(&3u32).unwrap();
        for pos in [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
            core.write_le(&Vertex2 {
                pos,
                norm: [0., 0., 1.],
                tex: [0.; 2],
                tangent: [0; 4],
                color: [255, 128, 0, 255],
            })
            .unwrap()
        }
        core.write_le(&1u32).unwrap();
        core.write_le(&Face2([VertexId2(0), VertexId2(1), VertexId2(2)]))
            .unwrap();
        for version in [6, 7] {
            let mut data = chunk(1, core.get_ref(), version);
            let t = crate::import::file_mesh(&data).unwrap();
            assert_eq!(t.len(), 1);
            assert_eq!(t[0].p[1], Vec3::X);
            assert!((t[0].colors[0].y - 128. / 255.).abs() < 1e-6);
            data.pop();
            assert!(crate::import::file_mesh(&data).is_err());
        }
    }
    #[test]
    fn draco_chunk_preserves_positions_uvs_and_colors() {
        use draco_core::{
            DataType, EncoderBuffer, EncoderOptions, GeometryAttributeType as Kind, Mesh,
            MeshEncoder, PointAttribute, PointIndex,
        };
        let mut mesh = Mesh::new();
        mesh.set_num_points(3);
        for (id, kind, components, values) in [
            (
                0,
                Kind::Position,
                3,
                vec![0., 0., 0., 1., 0., 0., 0., 1., 0.],
            ),
            (2, Kind::TexCoord, 2, vec![0., 0., 1., 0., 0., 1.]),
        ] {
            let mut a = PointAttribute::new();
            a.init(kind, components, DataType::Float32, false, 3);
            a.set_unique_id(id);
            let bytes: Vec<u8> = values.iter().flat_map(|v: &f32| v.to_le_bytes()).collect();
            a.buffer_mut().update(&bytes, Some(0));
            mesh.add_attribute_preserve_unique_id(a);
        }
        let mut a = PointAttribute::new();
        a.init(Kind::Color, 4, DataType::Uint8, false, 3);
        a.set_unique_id(4);
        a.buffer_mut()
            .update(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255], Some(0));
        mesh.add_attribute_preserve_unique_id(a);
        mesh.add_face([PointIndex(0), PointIndex(1), PointIndex(2)]);
        let mut encoder = MeshEncoder::new();
        encoder.set_mesh(mesh);
        let mut encoded = EncoderBuffer::new();
        encoder
            .encode(&EncoderOptions::new(), &mut encoded)
            .unwrap();
        let mut data = (encoded.data().len() as u32).to_le_bytes().to_vec();
        data.extend(encoded.data());
        let triangles = crate::import::file_mesh(&chunk(2, &data, 7)).unwrap();
        assert_eq!(triangles.len(), 1);
        assert!(triangles[0].p.contains(&Vec3::X));
        assert!(triangles[0].colors.contains(&Vec3::Y));
        assert!(triangles[0].uv.contains(&Vec2::X));
    }
}
