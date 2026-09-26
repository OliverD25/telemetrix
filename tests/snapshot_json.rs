use std::process::Command;

#[test]
fn snapshot_json_has_the_documented_shape() {
    let out = Command::new(env!("CARGO_BIN_EXE_telemetrix"))
        .args(["snapshot", "--json"])
        .output()
        .expect("run telemetrix");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    assert_eq!(doc["schema"], 1);
    assert!(doc["system"]["ram"]["total_bytes"].as_u64().unwrap_or(0) > 0);
    let cpu = doc["system"]["cpu_usage_pct"]
        .as_f64()
        .expect("cpu_usage_pct is a number");
    assert!((0.0..=100.0).contains(&cpu), "cpu_usage_pct = {cpu}");
    assert!(doc["system"]["disks"].is_array());
    // Empty without an NVIDIA driver; with one, every GPU has a name.
    let gpus = doc["system"]["gpus"].as_array().expect("gpus is a list");
    assert!(gpus.iter().all(|g| g["name"].as_str().is_some()));
    assert!(doc["timestamp"].as_str().is_some_and(|t| t.ends_with('Z')));
}
