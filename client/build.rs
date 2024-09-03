use std::{env, fs, path::Path};

fn main() {
    let cargo_toml = fs::read_to_string("../Cargo.toml").expect("Unable to read Cargo.toml");
    let version = cargo_toml
        .lines()
        .find(|line| line.starts_with("version = "))
        .and_then(|line| line.split('=').nth(1))
        .map(|v| v.trim_matches(&[' ', '"']).to_string())
        .expect("Unable to find version in Cargo.toml");

    let out_dir = env::var("OUT_DIR").unwrap();

    let version_file_path = Path::new(&out_dir).join("version.rs");
    fs::write(&version_file_path, format!("pub const VERSION: &str = \"{}\";", version))
        .expect("Unable to write version file");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
}
