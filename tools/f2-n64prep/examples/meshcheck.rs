//! Lists the objects of the given models (hex CRC ids) in a directory of object banks and
//! whether each got an F2NM mesh (debug aid for the N64 renderer).
//!
//! `cargo run -p f2-n64prep --example meshcheck -- <objects dir> <id>...`
use f2_formats::objbank::ObjectBank;

fn main() {
    let dir = std::env::args().nth(1).expect("objects dir");
    let want: Vec<u32> = std::env::args().skip(2).map(|s| u32::from_str_radix(&s, 16).expect("hex id")).collect();
    let mut paths: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "dll")).collect();
    paths.sort();
    for p in paths {
        let bank = ObjectBank::parse(&std::fs::read(&p).unwrap()).unwrap();
        let (f2nm, _) = f2_n64fmt::bank::encode_bank(&bank);
        let f = f2_n64fmt::File::parse(&f2nm).unwrap();
        let ids: Vec<u32> = (0..f.n_meshes()).map(|i| f.mesh(i).unwrap().entry.id).collect();
        for (mi, m) in bank.models.iter().enumerate() {
            if !want.is_empty() && !want.contains(&m.id) {
                continue;
            }
            f2_n64fmt::bank::for_each_object(m, |i, o| {
                let id = f2_n64fmt::bank::mesh_id(mi, if m.is_skinned { 0xFFFF } else { i as u16 });
                if let Some(mesh) = &o.mesh {
                    let tex0 = mesh.textures.iter().filter(|&&t| t == 0).count();
                    println!(
                        "{} {:08x} {} skinned={} obj {} {}: flags {:#x} xlu {} faces {} tex0 {} verts {} f2nm={}",
                        p.file_name().unwrap().to_string_lossy(),
                        m.id,
                        m.name(),
                        m.is_skinned,
                        i,
                        o.name,
                        o.flags,
                        o.xlu,
                        mesh.faces.len(),
                        tex0,
                        mesh.vertices.len(),
                        ids.contains(&id)
                    );
                }
            });
        }
    }
}
