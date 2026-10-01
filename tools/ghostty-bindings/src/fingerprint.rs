use std::fs;
use std::path::{Path, PathBuf};

#[allow(dead_code)] // shared with the binding generator, which only hashes headers
pub fn file_fingerprint(path: &Path) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    hash_bytes(
        &mut hash,
        &fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    );
    format!("fnv1a64:{hash:016x}")
}

pub fn header_fingerprint(include_dir: &Path) -> String {
    let mut headers = Vec::new();
    collect_headers(include_dir, include_dir, &mut headers);
    headers.sort_by(|left, right| left.0.cmp(&right.0));

    let mut hash = FNV_OFFSET_BASIS;
    for (relative, path) in headers {
        hash_bytes(&mut hash, relative.as_bytes());
        hash_bytes(&mut hash, &[0]);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        hash_bytes(&mut hash, &bytes);
        hash_bytes(&mut hash, &[0xff]);
    }
    format!("fnv1a64:{hash:016x}")
}

fn collect_headers(root: &Path, directory: &Path, output: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "failed to read an entry under {}: {error}",
                directory.display()
            )
        });
        let path = entry.path();
        if path.is_dir() {
            collect_headers(root, &path, output);
        } else if path.extension().and_then(|value| value.to_str()) == Some("h") {
            let relative = path
                .strip_prefix(root)
                .expect("header must be below include directory")
                .to_string_lossy()
                .replace('\\', "/");
            output.push((relative, path));
        }
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;
