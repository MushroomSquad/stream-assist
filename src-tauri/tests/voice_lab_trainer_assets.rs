use stream_assist_lib::voice_lab::trainer::{MIN_UV, TRAINER_REV, WAVLM_FILES, WAVLM_REV};

#[test]
fn pinned_trainer_assets() {
    assert_eq!(TRAINER_REV, "84a6f4c220cd7cc57dce14f638d43e13fa777f42");
    assert_eq!(WAVLM_REV, "c1423ed94bb01d80a3f5ce5bc39f6026a0f4828c");
    assert_eq!(MIN_UV, (0, 10, 4));
    assert_eq!(
        WAVLM_FILES.iter().map(|f| f.path).collect::<Vec<_>>(),
        [
            "config.json",
            "preprocessor_config.json",
            "pytorch_model.bin"
        ]
    );
    for file in WAVLM_FILES {
        assert!(file.size > 0);
        assert_eq!(file.sha256.len(), 64);
        assert!(file.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
