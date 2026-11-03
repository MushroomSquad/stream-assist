//! Pinned trainer source and WavLM assets measured during the GPU spike.
use super::models::ModelFile;

pub const TRAINER_REV: &str = "84a6f4c220cd7cc57dce14f638d43e13fa777f42";
pub const WAVLM_REV: &str = "c1423ed94bb01d80a3f5ce5bc39f6026a0f4828c";
/// Version used to generate the frozen Python environment.
pub const MIN_UV: (u32, u32, u32) = (0, 10, 4);

pub const WAVLM_FILES: &[ModelFile] = &[
    ModelFile {
        path: "config.json",
        sha256: "a3d8fe831aaf63d725b54a8ac36f3549cd4365c5086774b2c89cabbc6f9e129d",
        size: 2222,
    },
    ModelFile {
        path: "preprocessor_config.json",
        sha256: "60ca5a31e13f69ee2fbf147504c8676db5f6398fd7a6b12294341dff838edfcf",
        size: 214,
    },
    ModelFile {
        path: "pytorch_model.bin",
        sha256: "fdee460e529396ddb2f8c8e8ce0ad74cfb747b726bc6f612e666c7c1e1963c9d",
        size: 1261990257,
    },
];
