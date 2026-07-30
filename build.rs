use std::{fs, path::Path};

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn hash_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

fn hash_path(path: &Path, mut hash: u64) -> u64 {
    hash = hash_bytes(hash, path.to_string_lossy().as_bytes());
    if path.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("build input directory must be readable")
            .map(|entry| entry.expect("build input entry must be readable").path())
            .collect();
        entries.sort();
        for entry in entries {
            hash = hash_path(&entry, hash);
        }
        hash
    } else {
        hash_bytes(
            hash,
            &fs::read(path).expect("build input file must be readable"),
        )
    }
}

fn main() {
    let mut hash = FNV_OFFSET;
    for input in ["Cargo.toml", "Cargo.lock", "build.rs", "src"] {
        println!("cargo:rerun-if-changed={input}");
        hash = hash_path(Path::new(input), hash);
    }
    println!("cargo:rustc-env=FREEMODEL_BUILD_ID={hash:016x}");
}
