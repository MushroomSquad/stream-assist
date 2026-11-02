use std::path::Path;

use sha2::{Digest, Sha256};
use supertonic3_tts::{Style, helper::VoiceStyleData};

use super::{VlError, err};

pub const MAX_STYLE_BYTES: u64 = 2_097_152;

pub fn parse_style(bytes: &[u8]) -> Result<Style, VlError> {
    if bytes.len() as u64 > MAX_STYLE_BYTES {
        return Err(err("STYLE_TOO_LARGE"));
    }
    let data: VoiceStyleData =
        serde_json::from_slice(bytes).map_err(|_| err("STYLE_BAD_SCHEMA"))?;
    for component in [&data.style_ttl, &data.style_dp] {
        if component.dtype != "float32" {
            return Err(err("STYLE_BAD_SCHEMA"));
        }
    }
    for (component, dims) in [
        (&data.style_ttl, [1, 50, 256]),
        (&data.style_dp, [1, 8, 16]),
    ] {
        if component.dims != dims
            || component.data.len() != dims[0]
            || component
                .data
                .iter()
                .any(|batch| batch.len() != dims[1] || batch.iter().any(|row| row.len() != dims[2]))
        {
            return Err(err("STYLE_WRONG_SHAPE"));
        }
    }
    if [&data.style_ttl, &data.style_dp].iter().any(|component| {
        component
            .data
            .iter()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
    }) {
        return Err(err("STYLE_NOT_FINITE"));
    }
    Style::from_voice_style_data(&data).map_err(Into::into)
}

pub fn read_style_file(path: &Path) -> Result<Style, VlError> {
    parse_style(&read_style_bytes(path)?)
}

pub fn load_verified(path: &Path, sha: &str) -> Result<Style, VlError> {
    let bytes = read_style_bytes(path)?;
    if sha256_hex(&bytes) != sha {
        return Err(err("PROFILE_HASH_MISMATCH"));
    }
    parse_style(&bytes)
}

/// Reads at most `MAX_STYLE_BYTES + 1` bytes of a style file.
pub fn read_style_bytes(path: &Path) -> Result<Vec<u8>, VlError> {
    let mut file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_STYLE_BYTES {
        return Err(err("STYLE_TOO_LARGE"));
    }
    // Bound the read as well, in case the file grows after checking metadata.
    use std::io::Read;
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_STYLE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub fn canonical_bytes(style: &Style) -> Vec<u8> {
    serde_json::to_vec(&style.to_voice_style_data()).expect("voice style is JSON serializable")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_json(ttl_dims: [usize; 3], dp_dims: [usize; 3], fill: &str) -> String {
        fn component(dims: [usize; 3], fill: &str) -> String {
            let row = format!("[{}]", vec![fill; dims[2]].join(","));
            let batch = format!("[{}]", vec![row; dims[1]].join(","));
            format!(
                r#"{{"dims":{dims:?},"type":"float32","data":[{}]}}"#,
                vec![batch; dims[0]].join(",")
            )
        }
        format!(
            r#"{{"style_ttl":{},"style_dp":{}}}"#,
            component(ttl_dims, fill),
            component(dp_dims, fill)
        )
    }

    fn valid_json() -> String {
        style_json([1, 50, 256], [1, 8, 16], "0.25")
    }

    #[test]
    fn valid_style_roundtrips() {
        let style = parse_style(valid_json().as_bytes()).unwrap();
        let canonical = canonical_bytes(&style);
        let again = parse_style(&canonical).unwrap();
        assert_eq!(style.ttl, again.ttl);
        assert_eq!(style.dp, again.dp);
        assert!(
            !String::from_utf8(canonical)
                .unwrap()
                .contains("\"metadata\"")
        );
    }

    #[test]
    fn wrong_shape_rejected() {
        assert_eq!(
            parse_style(style_json([1, 50, 128], [1, 8, 16], "0").as_bytes())
                .unwrap_err()
                .code(),
            "STYLE_WRONG_SHAPE"
        );
        for key in ["style_ttl", "style_dp"] {
            for axis in 0..3 {
                let mut value: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
                let mut data = &mut value[key]["data"];
                for _ in 0..axis {
                    data = &mut data[0];
                }
                data.as_array_mut().unwrap().pop();
                assert_eq!(
                    parse_style(&serde_json::to_vec(&value).unwrap())
                        .unwrap_err()
                        .code(),
                    "STYLE_WRONG_SHAPE"
                );
            }
        }
        let mut value: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
        let element = value["style_ttl"]["data"][0][0]
            .as_array_mut()
            .unwrap()
            .pop()
            .unwrap();
        value["style_ttl"]["data"][0][1]
            .as_array_mut()
            .unwrap()
            .push(element);
        assert_eq!(
            parse_style(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code(),
            "STYLE_WRONG_SHAPE"
        );
    }

    #[test]
    fn nonfinite_rejected() {
        assert_eq!(
            parse_style(style_json([1, 50, 256], [1, 8, 16], "1e39").as_bytes())
                .unwrap_err()
                .code(),
            "STYLE_NOT_FINITE"
        );
    }

    #[test]
    fn bad_schema_rejected() {
        let mut missing: serde_json::Value = serde_json::from_str(&valid_json()).unwrap();
        missing.as_object_mut().unwrap().remove("style_dp");
        for json in [
            "[]".to_string(),
            "{}".to_string(),
            missing.to_string(),
            valid_json().replace("float32", "float16"),
            "not JSON".to_string(),
            style_json([1, 50, 256], [1, 8, 16], "1e999"),
        ] {
            assert_eq!(
                parse_style(json.as_bytes()).unwrap_err().code(),
                "STYLE_BAD_SCHEMA"
            );
        }
    }

    #[test]
    fn too_large_rejected() {
        let bytes = vec![b' '; MAX_STYLE_BYTES as usize + 1];
        assert_eq!(parse_style(&bytes).unwrap_err().code(), "STYLE_TOO_LARGE");
        let path = std::env::temp_dir().join(format!("vl-style-{}.json", std::process::id()));
        let file = std::fs::File::create_new(&path).unwrap();
        file.set_len(MAX_STYLE_BYTES + 1).unwrap();
        let result = read_style_file(&path);
        drop(file);
        std::fs::remove_file(path).unwrap();
        assert_eq!(result.unwrap_err().code(), "STYLE_TOO_LARGE");
    }

    #[test]
    fn deep_nesting_rejected() {
        let deep = format!("{}0{}", "[".repeat(10_000), "]".repeat(10_000));
        for json in [deep.clone(), format!(r#"{{"metadata":{deep}}}"#)] {
            assert_eq!(
                parse_style(json.as_bytes()).unwrap_err().code(),
                "STYLE_BAD_SCHEMA"
            );
        }
    }

    #[test]
    fn metadata_ignored() {
        let original = valid_json();
        let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
        value["metadata"] = serde_json::json!({"source": "private", "extra": [1, 2, 3]});
        let style = parse_style(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            canonical_bytes(&style),
            canonical_bytes(&parse_style(original.as_bytes()).unwrap())
        );
    }

    #[test]
    fn sha_is_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let bytes = canonical_bytes(&parse_style(valid_json().as_bytes()).unwrap());
        assert_eq!(
            sha256_hex(&bytes),
            sha256_hex(&canonical_bytes(&parse_style(&bytes).unwrap()))
        );
    }
}
