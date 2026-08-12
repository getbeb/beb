use std::process::Command;

#[test]
fn e2e() {
    let bin = env!("CARGO_BIN_EXE_beb");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/e2e.sh");
    let out = Command::new("bash")
        .arg(script)
        .env("BEB", bin)
        .output()
        .expect("bash runs");
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "e2e.sh failed");
}
