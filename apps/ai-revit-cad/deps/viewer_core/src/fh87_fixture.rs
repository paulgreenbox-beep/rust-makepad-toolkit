//! Generic parser for FH87MSH1 tessellated mesh scenes.
//!
//! The byte format stores scene-object identifiers, display colors, vertex positions, and
//! triangle indices. It does not load or identify bundled project CAD data.

use crate::{Mesh, Scene, SceneObject, Triangle, Vec3};

const MAGIC: &[u8; 8] = b"FH87MSH1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecodeError {
    Truncated,
    InvalidMagic,
    InvalidUtf8,
    InvalidGeometry,
    TrailingBytes,
}

struct Reader<'a> {
    data: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, cursor: 0 }
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        let end = self
            .cursor
            .checked_add(count)
            .ok_or(DecodeError::Truncated)?;
        let bytes = self
            .data
            .get(self.cursor..end)
            .ok_or(DecodeError::Truncated)?;
        self.cursor = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        let bytes: [u8; 2] = self.bytes(2)?.try_into().expect("checked length");
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        let bytes: [u8; 4] = self.bytes(4)?.try_into().expect("checked length");
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, DecodeError> {
        let bytes: [u8; 8] = self.bytes(8)?.try_into().expect("checked length");
        Ok(u64::from_le_bytes(bytes))
    }

    fn f32(&mut self) -> Result<f32, DecodeError> {
        let bytes: [u8; 4] = self.bytes(4)?.try_into().expect("checked length");
        let value = f32::from_le_bytes(bytes);
        if value.is_finite() {
            Ok(value)
        } else {
            Err(DecodeError::InvalidGeometry)
        }
    }
}

fn decode_scene(data: &[u8]) -> Result<Scene, DecodeError> {
    let mut reader = Reader::new(data);
    if reader.bytes(8)? != MAGIC {
        return Err(DecodeError::InvalidMagic);
    }
    let object_count = reader.u32()? as usize;
    if object_count > 10_000 {
        return Err(DecodeError::InvalidGeometry);
    }
    let mut scene = Scene::new();
    for _ in 0..object_count {
        let object_id = reader.u64()?;
        let name_len = reader.u16()? as usize;
        let name =
            std::str::from_utf8(reader.bytes(name_len)?).map_err(|_| DecodeError::InvalidUtf8)?;
        let color = [reader.f32()?, reader.f32()?, reader.f32()?, reader.f32()?];
        let vertex_count = reader.u32()? as usize;
        let triangle_count = reader.u32()? as usize;
        if vertex_count == 0 || triangle_count == 0 {
            return Err(DecodeError::InvalidGeometry);
        }
        let vertex_bytes = vertex_count
            .checked_mul(12)
            .ok_or(DecodeError::InvalidGeometry)?;
        let triangle_bytes = triangle_count
            .checked_mul(12)
            .ok_or(DecodeError::InvalidGeometry)?;
        if reader.data.len().saturating_sub(reader.cursor) < vertex_bytes + triangle_bytes {
            return Err(DecodeError::Truncated);
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        for _ in 0..vertex_count {
            vertices.push(Vec3::new(
                reader.f32()? as f64,
                reader.f32()? as f64,
                reader.f32()? as f64,
            ));
        }
        let mut triangles = Vec::with_capacity(triangle_count);
        for triangle_id in 0..triangle_count {
            let a = reader.u32()? as usize;
            let b = reader.u32()? as usize;
            let c = reader.u32()? as usize;
            if a >= vertex_count || b >= vertex_count || c >= vertex_count {
                return Err(DecodeError::InvalidGeometry);
            }
            triangles.push(Triangle::new(
                triangle_id as u32,
                vertices[a],
                vertices[b],
                vertices[c],
            ));
        }
        let mut object = SceneObject::new(object_id, name, Mesh::new(triangles));
        object.color = color;
        if scene.insert(object).is_some() {
            return Err(DecodeError::InvalidGeometry);
        }
    }
    if reader.cursor != data.len() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(scene)
}

/// Decodes an FH87MSH1 mesh-scene payload without attaching exact geometry sources.
///
/// This generic display format keeps CAD-source metadata outside the realtime mesh registry.
pub fn decode_fh87_mesh_scene(data: &[u8]) -> Result<Scene, String> {
    decode_scene(data).map_err(|error| format!("invalid FH87MSH1 display scene: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedural_test_scene;

    fn write_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut Vec<u8>, value: u64) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn write_f32(bytes: &mut Vec<u8>, value: f32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn encode_scene(scene: &Scene) -> Vec<u8> {
        let objects: Vec<_> = scene.objects().collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        write_u32(&mut bytes, objects.len() as u32);

        for object in objects {
            write_u64(&mut bytes, object.object_id);
            let name = object.name.as_bytes();
            write_u16(&mut bytes, name.len() as u16);
            bytes.extend_from_slice(name);
            for channel in object.color {
                write_f32(&mut bytes, channel);
            }

            let triangles = object.mesh.triangles();
            write_u32(&mut bytes, (triangles.len() * 3) as u32);
            write_u32(&mut bytes, triangles.len() as u32);
            for triangle in triangles {
                for vertex in triangle.vertices {
                    let world_vertex = object.transform.transform_point(vertex);
                    write_f32(&mut bytes, world_vertex.x as f32);
                    write_f32(&mut bytes, world_vertex.y as f32);
                    write_f32(&mut bytes, world_vertex.z as f32);
                }
            }
            for triangle_index in 0..triangles.len() {
                let first_vertex = (triangle_index * 3) as u32;
                write_u32(&mut bytes, first_vertex);
                write_u32(&mut bytes, first_vertex + 1);
                write_u32(&mut bytes, first_vertex + 2);
            }
        }
        bytes
    }

    #[test]
    fn generic_decoder_preserves_procedural_scene_geometry() {
        let source = procedural_test_scene();
        let decoded = decode_fh87_mesh_scene(&encode_scene(&source))
            .expect("synthetic procedural mesh scene should decode");

        assert_eq!(
            decoded.object_ids().collect::<Vec<_>>(),
            source.object_ids().collect::<Vec<_>>()
        );
        assert_eq!(decoded.visible_count(), source.visible_count());

        for source_object in source.objects() {
            let decoded_object = decoded
                .get(source_object.object_id)
                .expect("decoded scene should retain each procedural object");
            assert_eq!(decoded_object.name, source_object.name);
            assert_eq!(decoded_object.color, source_object.color);
            assert!(!decoded_object.has_exact_geometry());
            assert_eq!(
                decoded_object.mesh.triangles().len(),
                source_object.mesh.triangles().len()
            );

            let expected_bounds = source_object
                .world_bounds()
                .expect("procedural source object has geometry");
            let actual_bounds = decoded_object
                .mesh
                .bounds()
                .expect("decoded object has geometry");
            assert!((actual_bounds.min - expected_bounds.min).length() < 1.0e-5);
            assert!((actual_bounds.max - expected_bounds.max).length() < 1.0e-5);

            for (source_triangle, decoded_triangle) in source_object
                .mesh
                .triangles()
                .iter()
                .zip(decoded_object.mesh.triangles())
            {
                for vertex_index in 0..3 {
                    let source_vertex = source_object
                        .transform
                        .transform_point(source_triangle.vertices[vertex_index]);
                    let decoded_vertex = decoded_triangle.vertices[vertex_index];
                    assert!((source_vertex - decoded_vertex).length() < 1.0e-5);
                }
            }
        }
    }

    #[test]
    fn generic_decoder_rejects_truncated_and_invalid_magic_payloads() {
        let encoded = encode_scene(&procedural_test_scene());
        let mut truncated = encoded.clone();
        truncated.pop();
        assert_eq!(
            decode_scene(&truncated).err(),
            Some(DecodeError::Truncated)
        );

        let mut bad_magic = encoded;
        bad_magic[0] ^= 0xff;
        assert_eq!(
            decode_scene(&bad_magic).err(),
            Some(DecodeError::InvalidMagic)
        );
    }
}
