#![cfg(not(target_arch = "wasm32"))]

// Compile the native harness helpers under the test harness as well.
#[allow(dead_code)]
#[path = "../benches/shader.rs"]
mod shader;

#[test]
fn appended_tight_rounds_cannot_mask_candidate_spread() {
    let baseline = [99., 100., 100., 100., 101.].repeat(3);
    let candidate = [80., 90., 110., 120., 130.];
    assert_eq!(
        shader::native::verdict(&baseline, &candidate, 0.),
        ("within noise", 60.)
    );
}

#[test]
fn minority_process_drift_counts_even_when_baseline_mad_is_zero() {
    let baseline: Vec<_> = [100.; 10].into_iter().chain([120.; 5]).collect();
    let scene = serde_json::json!({"rounds": baseline.iter().enumerate()
        .map(|(i, value)| serde_json::json!({"round": i % 5 + 1, "p25_ms": value,
            "process_id": 1})) // Reused PID must not hide separate invocations.
        .collect::<Vec<_>>()});
    let spread = shader::native::process_spread(&scene).unwrap();
    assert_eq!(spread, 10.);
    assert_eq!(
        shader::native::verdict(&baseline, &[120.; 5], spread),
        ("within noise", 30.)
    );
}
