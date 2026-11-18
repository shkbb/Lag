//! GNU import-library adapter for sandbox environments without an executable GNU assembler.
//! This is a build helper, not part of the C# audio port or production runtime.
use std::{env, path::Path, process::{Command, exit}};
fn main() {
    let args: Vec<_> = env::args().collect();
    let value = |key: &str| args.iter().position(|s| s == key).and_then(|i| args.get(i + 1)).expect("dlltool argument");
    let def = Path::new(value("-d")).with_extension("lld.def");
    std::fs::write(&def, format!("LIBRARY {}\n{}", value("-D"), std::fs::read_to_string(value("-d")).expect("read def"))).expect("write def");
    let lld = env::var_os("LAG_RUST_LLD").expect("LAG_RUST_LLD must point to rust-lld.exe");
    let status = Command::new(lld).args(["-flavor", "link", "/lib", "/machine:x64"])
        .arg(format!("/def:{}", def.display())).arg(format!("/out:{}", value("-l"))).status().expect("rust-lld import library");
    exit(status.code().unwrap_or(1));
}
