//! The hands' model: SteamVR's own hand (`resources/rendermodels/vr_glove/
//! vr_glove_<side>_model_slim.glb` of the installed runtime, read from there
//! and not carried with us), a skinned mesh of some 10,000 vertices on the
//! 31 bones of SteamVR's hand skeleton.
//!
//! The model's axes: the fingers along +z, the thumb's side +y, in metres,
//! the wrist at the origin. A finger's three joints (`finger_<name>_0..2`)
//! bend about their own z axis; the model rests a little bent. Here a
//! finger is bent by a number from 0 (flat) to 1 (a fist) through all three
//! joints, the way the controller's grip and trigger say; the same bones
//! take SteamVR's skeletal input when that is read one day.

use super::json::{self, Value};
use super::math::{M34, V3, add, from_axes, mul, point, scale, turn, unit};

/// A vertex as the scene draws it: place, normal, texture place.
pub(super) type Vertex = [f32; 8];

/// Which bend number moves a joint ([`super::hands`]' channels), and by how
/// much: degrees about the joint's z at 0 and at 1.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bend {
    channel: usize,
    flat: f32,
    fist: f32,
}

struct Bone {
    parent: Option<usize>,
    at: V3,
    turn: [f32; 4],
    bend: Option<Bend>,
    /// The inverse of the bone's place in the model at rest.
    unbind: M34,
}

pub(super) struct Glove {
    bones: Vec<Bone>,
    /// Places and normals at rest, texture places, four bones and weights.
    rest: Vec<([f32; 3], [f32; 3], [f32; 2], [u16; 4], [f32; 4])>,
    pub(super) indices: Vec<u16>,
}

fn rotation(q: [f32; 4]) -> [V3; 3] {
    let [x, y, z, w] = q;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w)],
        [2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w)],
        [2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// `a` then turned by `b` in `a`'s own frame.
fn times(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz]
}

/// How a joint named `name` bends.
fn bend_of(name: &str) -> Option<Bend> {
    use super::hands::{INDEX, OTHERS, THUMB};
    let rest = name.strip_prefix("finger_")?;
    let (finger, rest) = rest.split_once('_')?;
    let joint = rest.split('_').next()?.parse::<u32>().ok()?;
    let channel = match finger {
        "thumb" => THUMB,
        "index" => INDEX,
        "middle" | "ring" | "pinky" => OTHERS,
        _ => return None,
    };
    let (flat, fist) = match (channel == THUMB, joint) {
        (true, 0) => (0.0, 14.0),
        (true, 1) => (-10.0, 38.0),
        (true, 2) => (-8.0, 55.0),
        (false, 0) => (-24.0, 62.0),
        (false, 1) => (-18.0, 88.0),
        (false, 2) => (-10.0, 62.0),
        _ => return None,
    };
    Some(Bend { channel, flat, fist })
}

impl Glove {
    /// The model in the file's bytes; why not, otherwise.
    pub(super) fn read(file: &[u8]) -> Result<Self, String> {
        let word = |at: usize| file.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
        if file.get(..4) != Some(b"glTF") {
            return Err("not a binary glTF file".into());
        }
        let head = word(12).ok_or("cut short")?;
        let header = json::parse(file.get(20..20 + head).ok_or("cut short")?).ok_or("its header is not JSON")?;
        let data_at = 20 + head + 8;
        let data = file.get(data_at..data_at + word(20 + head).ok_or("cut short")?).ok_or("its data is cut short")?;
        let field = |v: &Value, key: &str| v.get(key).and_then(Value::index);
        // An accessor's elements of `width` bytes each.
        let view = |accessor: usize, width: usize| -> Result<(&[u8], usize), String> {
            let a = header.get("accessors").and_then(|l| l.at(accessor)).ok_or("no such accessor")?;
            let which = field(a, "bufferView").ok_or("an accessor with no view")?;
            let v = header.get("bufferViews").and_then(|l| l.at(which)).ok_or("no such view")?;
            let from = field(v, "byteOffset").unwrap_or(0) + field(a, "byteOffset").unwrap_or(0);
            let count = field(a, "count").filter(|c| *c > 0).ok_or("an accessor with no count")?;
            let stride = field(v, "byteStride").unwrap_or(width).max(width);
            let bytes = data.get(from..from + (count - 1) * stride + width).ok_or("an accessor past the data")?;
            Ok((bytes, stride))
        };
        let floats = |accessor: usize, n: usize| -> Result<Vec<Vec<f32>>, String> {
            let (bytes, stride) = view(accessor, 4 * n)?;
            Ok(bytes.chunks(stride).map(|c| c[..4 * n].chunks(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()).collect())
        };
        let shorts = |accessor: usize, n: usize| -> Result<Vec<Vec<u16>>, String> {
            let (bytes, stride) = view(accessor, 2 * n)?;
            Ok(bytes.chunks(stride).map(|c| c[..2 * n].chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect()).collect())
        };
        let primitive = header.get("meshes").and_then(|m| m.at(0)).and_then(|m| m.get("primitives")).and_then(|p| p.at(0)).ok_or("no mesh")?;
        let attribute = |name: &str| primitive.get("attributes").and_then(|a| field(a, name)).ok_or_else(|| format!("no {name}"));
        let (places, normals, uvs) = (floats(attribute("POSITION")?, 3)?, floats(attribute("NORMAL")?, 3)?, floats(attribute("TEXCOORD_0")?, 2)?);
        let (joints, weights) = (shorts(attribute("JOINTS_0")?, 4)?, floats(attribute("WEIGHTS_0")?, 4)?);
        let indices: Vec<u16> = shorts(field(primitive, "indices").ok_or("no indices")?, 1)?.into_iter().map(|i| i[0]).collect();
        let n = places.len();
        if [normals.len(), uvs.len(), joints.len(), weights.len()] != [n; 4] || n > usize::from(u16::MAX) || indices.iter().any(|&i| usize::from(i) >= n) {
            return Err("its vertex data do not agree".into());
        }
        // The skin's joints, in its order: the model's bones.
        let skin = header.get("skins").and_then(|s| s.at(0)).ok_or("no skin")?;
        let nodes = header.get("nodes").ok_or("no nodes")?;
        let joint_nodes: Vec<usize> = skin.get("joints").map(Value::list).unwrap_or(&[]).iter().filter_map(Value::index).collect();
        let unbinds = floats(field(skin, "inverseBindMatrices").ok_or("no bind matrices")?, 16)?;
        if joint_nodes.is_empty() || unbinds.len() != joint_nodes.len() {
            return Err("its skin does not agree".into());
        }
        let bone_of = |node: usize| joint_nodes.iter().position(|&j| j == node);
        let mut bones = Vec::with_capacity(joint_nodes.len());
        for (k, &node) in joint_nodes.iter().enumerate() {
            let spec = nodes.at(node).ok_or("a joint that is no node")?;
            let numbers = |key: &str, n: usize, otherwise: &[f32]| spec.get(key).and_then(Value::numbers).filter(|v| v.len() == n).unwrap_or_else(|| otherwise.to_vec());
            let (t, r) = (numbers("translation", 3, &[0.0; 3]), numbers("rotation", 4, &[0.0, 0.0, 0.0, 1.0]));
            let parent = nodes.list().iter().position(|p| p.get("children").map(Value::list).unwrap_or(&[]).iter().any(|c| c.index() == Some(node))).and_then(bone_of);
            // Column-major 4x4: the rotation's columns, then the place.
            let m = &unbinds[k];
            bones.push(Bone {
                parent,
                at: [t[0], t[1], t[2]],
                turn: [r[0], r[1], r[2], r[3]],
                bend: spec.get("name").and_then(Value::text).and_then(bend_of),
                unbind: from_axes([m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]], [m[12], m[13], m[14]]),
            });
        }
        if bones.iter().enumerate().any(|(k, b)| b.parent.is_some_and(|p| p >= k)) {
            return Err("its bones are not in order".into());
        }
        let rest = (0..n)
            .map(|i| {
                let j = &joints[i];
                let j = [j[0], j[1], j[2], j[3]].map(|j| j.min(bones.len() as u16 - 1));
                ([places[i][0], places[i][1], places[i][2]], [normals[i][0], normals[i][1], normals[i][2]], [uvs[i][0], uvs[i][1]], j, [weights[i][0], weights[i][1], weights[i][2], weights[i][3]])
            })
            .collect();
        Ok(Self { bones, rest, indices })
    }

    pub(super) fn vertices(&self) -> usize {
        self.rest.len()
    }

    /// The vertices with the fingers bent by `curl` (thumb, index, the
    /// others; 0 flat, 1 a fist), into `out`. `mirrored`: the bends go the
    /// other way round (a model whose joints turn so).
    pub(super) fn posed(&self, curl: [f32; 3], mirrored: bool, out: &mut [Vertex]) {
        let mut placed: Vec<M34> = Vec::with_capacity(self.bones.len());
        for bone in &self.bones {
            let mut q = bone.turn;
            if let Some(bend) = bone.bend {
                let c = curl[bend.channel].clamp(0.0, 1.0);
                let half = (bend.flat + (bend.fist - bend.flat) * c).to_radians() / 2.0 * if mirrored { -1.0 } else { 1.0 };
                q = times(q, [0.0, 0.0, half.sin(), half.cos()]);
            }
            let [x, y, z] = rotation(q);
            let own = from_axes(x, y, z, bone.at);
            placed.push(match bone.parent {
                Some(p) => mul(&placed[p], &own),
                None => own,
            });
        }
        let skins: Vec<M34> = placed.iter().zip(&self.bones).map(|(at, bone)| mul(at, &bone.unbind)).collect();
        for (vertex, (place, normal, uv, joints, weights)) in out.iter_mut().zip(&self.rest) {
            let (mut p, mut n) = ([0.0; 3], [0.0; 3]);
            for k in 0..4 {
                let w = weights[k];
                if w > 0.0 {
                    let skin = &skins[usize::from(joints[k])];
                    p = add(p, scale(point(skin, *place), w));
                    n = add(n, scale(turn(skin, *normal), w));
                }
            }
            let total: f32 = weights.iter().sum();
            let p = if total > 1e-4 { scale(p, 1.0 / total) } else { *place };
            let n = unit(n, *normal);
            *vertex = [p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]];
        }
    }
}

/// Where the wrist is from a controller's own origin: under and behind it.
pub(super) const WRIST: V3 = [0.0, -0.035, 0.115];

/// The model's place on a controller (+x right, +y up, -z forward): turned
/// half round about +y, so that its fingers point forward, the thumb is up
/// and the palm faces the other hand; then the fingers lifted by `tilt`
/// degrees, and the wrist at `wrist` from the controller's own origin (what
/// is sideways in it, the other way for the left hand).
pub(super) fn on_controller(right: bool, tilt: f32, wrist: V3) -> M34 {
    let (s, c) = tilt.to_radians().sin_cos();
    let lifted = from_axes([1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c], [0.0; 3]);
    let round = from_axes([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0; 3]);
    let turned = mul(&lifted, &round);
    let at = [if right { wrist[0] } else { -wrist[0] }, wrist[1], wrist[2]];
    from_axes(super::math::axis(&turned, 0), super::math::axis(&turned, 1), super::math::axis(&turned, 2), at)
}

/// The model of the `right` or left hand from the runtime in `folder`.
pub(super) fn load(folder: &std::path::Path, right: bool) -> Result<Glove, String> {
    let name = format!("vr_glove_{}_model_slim.glb", if right { "right" } else { "left" });
    let path = folder.join("resources").join("rendermodels").join("vr_glove").join(name);
    let file = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Glove::read(&file).map_err(|why| format!("{}: {why}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model of one triangle on two bones, the second a finger joint.
    fn tiny() -> Vec<u8> {
        let mut data: Vec<u8> = Vec::new();
        let mut views = Vec::new();
        let mut push = |bytes: Vec<u8>, data: &mut Vec<u8>| {
            while data.len() % 4 != 0 {
                data.push(0);
            }
            views.push((data.len(), bytes.len()));
            data.extend(bytes);
        };
        let f = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let s = |v: &[u16]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        push(s(&[0, 1, 2]), &mut data);
        // Two vertices on the wrist, one a metre along the finger.
        push(f(&[0.0, 0.0, 0.0, 0.0, 0.1, 0.0, 1.0, 0.0, 0.0]), &mut data);
        push(f(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]), &mut data);
        push(f(&[0.0; 6]), &mut data);
        push(s(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]), &mut data);
        push(f(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]), &mut data);
        // The finger joint sits half a metre along x: its unbind takes that off.
        let mut unbind = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let mut second = unbind.clone();
        second[12] = -0.5;
        unbind.extend(second);
        push(f(&unbind), &mut data);
        let kinds = [(5123, "SCALAR", 3), (5126, "VEC3", 3), (5126, "VEC3", 3), (5126, "VEC2", 3), (5123, "VEC4", 3), (5126, "VEC4", 3), (5126, "MAT4", 2)];
        let accessors: Vec<String> = kinds.iter().enumerate().map(|(i, (c, t, n))| format!(r#"{{"bufferView":{i},"componentType":{c},"type":"{t}","count":{n}}}"#)).collect();
        let views: Vec<String> = views.iter().map(|(at, len)| format!(r#"{{"buffer":0,"byteOffset":{at},"byteLength":{len}}}"#)).collect();
        let header = format!(
            r#"{{"accessors":[{}],"bufferViews":[{}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":1,"NORMAL":2,"TEXCOORD_0":3,"JOINTS_0":4,"WEIGHTS_0":5}},"indices":0}}]}}],"skins":[{{"joints":[0,1],"inverseBindMatrices":6}}],"nodes":[{{"name":"wrist_r","children":[1]}},{{"name":"finger_index_1_r","translation":[0.5,0,0]}}]}}"#,
            accessors.join(","),
            views.join(",")
        );
        let mut header = header.into_bytes();
        while header.len() % 4 != 0 {
            header.push(b' ');
        }
        let mut file = b"glTF".to_vec();
        file.extend(2u32.to_le_bytes());
        file.extend(0u32.to_le_bytes());
        file.extend((header.len() as u32).to_le_bytes());
        file.extend(b"JSON");
        file.extend(header);
        file.extend((data.len() as u32).to_le_bytes());
        file.extend(b"BIN\0");
        file.extend(data);
        file
    }

    /// For looking at, not a check: with `VICTORY_VR_GLOVE_DUMP=<folder>`,
    /// the two hands of the installed SteamVR as they sit on a controller,
    /// flat, as fists and pointing, written there as OBJ files.
    #[test]
    fn the_installed_hands_can_be_looked_at() {
        let Some(folder) = std::env::var_os("VICTORY_VR_GLOVE_DUMP") else {
            return;
        };
        let folder = std::path::PathBuf::from(folder);
        let runtime = crate::vr::runtime_folder().expect("SteamVR is installed");
        let mirror = std::env::var("VICTORY_VR_GLOVE_MIRROR").unwrap_or_default();
        for right in [false, true] {
            let glove = load(&runtime, right).unwrap();
            let on = on_controller(right, 0.0, WRIST);
            for (name, curl) in [("flat", [0.0; 3]), ("half", [0.5; 3]), ("fist", [1.0; 3]), ("point", [1.0, 0.0, 1.0])] {
                let mut out = vec![[0.0; 8]; glove.vertices()];
                glove.posed(curl, mirror.contains(if right { 'r' } else { 'l' }), &mut out);
                let mut text = String::new();
                for v in &out {
                    let (p, n) = (point(&on, [v[0], v[1], v[2]]), turn(&on, [v[3], v[4], v[5]]));
                    text += &format!("v {} {} {}\nvn {} {} {}\n", p[0], p[1], p[2], n[0], n[1], n[2]);
                }
                for t in glove.indices.chunks(3) {
                    text += &format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1);
                }
                std::fs::write(folder.join(format!("{}_{name}.obj", if right { "right" } else { "left" })), text).unwrap();
            }
            let names: Vec<String> = glove.bones.iter().map(|b| format!("{:?}", b.bend.map(|b| b.channel))).collect();
            println!("right {right}: {} bones, bends {}", glove.bones.len(), names.join(" "));
        }
    }

    #[test]
    fn a_finger_bends_about_its_joint() {
        let glove = Glove::read(&tiny()).unwrap();
        assert_eq!((glove.vertices(), glove.indices.len(), glove.bones.len()), (3, 3, 2));
        assert_eq!(glove.bones[1].bend.map(|b| b.channel), Some(crate::vr::hands::INDEX));
        let mut out = [[0.0; 8]; 3];
        // With the joint's flat and fist angles a bend of 18/106 is no turn at
        // all: the model as it rests.
        let rest = 18.0 / 106.0;
        glove.posed([0.0, rest, 0.0], false, &mut out);
        assert!((out[2][0] - 1.0).abs() < 1e-4 && out[2][1].abs() < 1e-4, "{:?}", out[2]);
        // A fist turns the far vertex about the joint at x = 0.5, about z:
        // it stays half a metre from the joint, and the wrist's stay put.
        glove.posed([0.0, 1.0, 0.0], false, &mut out);
        let (dx, dy) = (out[2][0] - 0.5, out[2][1]);
        assert!(((dx * dx + dy * dy).sqrt() - 0.5).abs() < 1e-4 && dy > 0.4, "{:?}", out[2]);
        assert!(out[0][..3] == [0.0, 0.0, 0.0] && (out[1][1] - 0.1).abs() < 1e-6);
        // The other way round for a mirrored model.
        glove.posed([0.0, 1.0, 0.0], true, &mut out);
        assert!(out[2][1] < -0.4);
        assert!(Glove::read(b"nothing").is_err());
        // On a controller the fingers (+z of the model) point forward, the
        // thumb's side stays up; lifted by 90 degrees they point up.
        let on = on_controller(true, 0.0, WRIST);
        assert_eq!((turn(&on, [0.0, 0.0, 1.0]), turn(&on, [0.0, 1.0, 0.0]), super::super::math::place(&on)), ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], WRIST));
        let up = turn(&on_controller(false, 90.0, [0.01, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        assert!(up[1] > 0.999 && super::super::math::place(&on_controller(false, 90.0, [0.01, 0.0, 0.0]))[0] < 0.0, "{up:?}");
    }
}
