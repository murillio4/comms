fn main() {
    println!("cargo:rerun-if-changed={}/resources", env!("CARGO_MANIFEST_DIR"));
}
