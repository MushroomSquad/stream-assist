use serde_json::{Value, json};
use std::{
    fs,
    io::{Cursor, Write},
    path::Path,
    sync::Mutex,
};
use stream_assist_lib::voice_lab::{
    mixer::{DpStrategy, RecipeInput, new_recipe},
    project::{export_project, import_project},
    store::{RenderParams, Store},
    style::{canonical_bytes, sha256_hex},
};
use zip::{ZipWriter, write::SimpleFileOptions};

fn style() -> supertonic3_tts::Style {
    supertonic3_tts::Style {
        ttl: ndarray::Array3::from_elem((1, 50, 256), 0.25),
        dp: ndarray::Array3::from_elem((1, 8, 16), 0.5),
    }
}
fn params() -> RenderParams {
    RenderParams {
        text: "Привет".into(),
        lang: "ru".into(),
        speed: 1.0,
        quality: 8,
    }
}
fn wav() -> Vec<u8> {
    let mut c = Cursor::new(Vec::new());
    {
        let mut w = hound::WavWriter::new(
            &mut c,
            hound::WavSpec {
                channels: 1,
                sample_rate: 8000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..8000 {
            w.write_sample(0i16).unwrap();
        }
        w.finalize().unwrap();
    }
    c.into_inner()
}
fn manifest() -> Value {
    let sha = sha256_hex(&canonical_bytes(&style()));
    json!({"kind":"voice-lab-project","schemaVersion":1,"assetRevision":stream_assist_lib::voice_lab::models::MODEL_REVISION,"createdAt":"today",
        "profiles":[{"file":format!("profiles/{sha}.json"),"sha256":sha,"name":"A","tags":["tag"],"origin":"builtin","recipe":null,"meta":{"sourceId":42,"keep":true}}],"renders":[]})
}
fn archive(path: &Path, m: &Value, entries: &[(&str, Vec<u8>)]) {
    let mut z = ZipWriter::new(fs::File::create(path).unwrap());
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    z.start_file("manifest.json", opts).unwrap();
    z.write_all(&serde_json::to_vec(m).unwrap()).unwrap();
    for (name, data) in entries {
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}
fn import_fixture(m: &Value, bytes: Vec<u8>, code: &str) {
    let d = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    let p = d.path().join("x.svlab");
    archive(
        &p,
        m,
        &[(m["profiles"][0]["file"].as_str().unwrap(), bytes)],
    );
    assert_eq!(import_project(&s, &p).unwrap_err().code(), code);
    let s = s.lock().unwrap();
    assert!(s.profiles().unwrap().is_empty());
    assert!(s.renders(10).unwrap().is_empty());
    assert_eq!(fs::read_dir(s.root().join("tmp")).unwrap().count(), 0);
}
#[test]
fn roundtrip_profiles_and_favorites() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(a.path()).unwrap());
    let recipe = new_recipe(
        vec![RecipeInput {
            profile_sha256: sha256_hex(&canonical_bytes(&style())),
            name: "base".into(),
            weight: 100.0,
        }],
        DpStrategy::Weighted,
        None,
        "today".into(),
    );
    {
        let s = s.lock().unwrap();
        let p = s
            .put_profile(
                &style(),
                "A",
                "mix",
                Some(&recipe),
                Some(json!({"sourceId":9})),
            )
            .unwrap();
        s.rename_profile(p.id, "A", &["tag".into()]).unwrap();
        for favorite in [true, false] {
            let r = s
                .add_render(Some(p.id), Some(&recipe), &params(), 999, 42)
                .unwrap();
            fs::write(s.render_path(r.id), wav()).unwrap();
            s.set_favorite(r.id, favorite).unwrap();
        }
    }
    let path = a.path().join("x.svlab");
    export_project(&s, &path).unwrap();
    let dst = Mutex::new(Store::open(b.path()).unwrap());
    let report = import_project(&dst, &path).unwrap();
    assert_eq!((report.profiles, report.renders, report.skipped), (1, 1, 0));
    assert!(!report.revision_mismatch);
    let dst = dst.lock().unwrap();
    let p = &dst.profiles().unwrap()[0];
    assert_eq!(p.tags, vec!["tag"]);
    assert_eq!(p.recipe, Some(recipe.clone()));
    assert_eq!(p.origin, "import");
    dst.prune_renders(0).unwrap();
    let r = &dst.renders(10).unwrap()[0];
    assert!(r.favorite);
    assert_eq!(r.duration_ms, 1000);
    assert_eq!(r.recipe, Some(recipe));
    assert_eq!(r.profile_id, Some(p.id));
    assert_eq!(fs::read(dst.render_path(r.id)).unwrap(), wav());
}
#[test]
fn builtin_not_exported() {
    let d = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(d.path()).unwrap());
    s.lock()
        .unwrap()
        .put_profile(&style(), "Builtin", "builtin", None, None)
        .unwrap();
    let p = d.path().join("x.svlab");
    export_project(&s, &p).unwrap();
    let b = tempfile::tempdir().unwrap();
    let dst = Mutex::new(Store::open(b.path()).unwrap());
    assert_eq!(import_project(&dst, &p).unwrap().profiles, 0);
}
#[test]
fn bad_entry_name_rejected() {
    for name in ["../x", "profiles/x.json"] {
        let mut m = manifest();
        m["profiles"][0]["file"] = json!(name);
        import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
    }
}
#[test]
fn zip_bomb_rejected() {
    import_fixture(
        &manifest(),
        vec![b' '; 2 * 1024 * 1024 + 1],
        "PROJECT_TOO_LARGE",
    );
}
#[test]
fn tampered_profile_rejected_nothing_applied() {
    let mut m = manifest();
    m["profiles"][0]["sha256"] = json!("0".repeat(64));
    import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
}
#[test]
fn builtin_origin_imported_as_import() {
    let d = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    let p = d.path().join("x.svlab");
    let m = manifest();
    archive(
        &p,
        &m,
        &[(
            m["profiles"][0]["file"].as_str().unwrap(),
            canonical_bytes(&style()),
        )],
    );
    import_project(&s, &p).unwrap();
    let s = s.lock().unwrap();
    let p = &s.profiles().unwrap()[0];
    assert_eq!(p.origin, "import");
    assert_eq!(
        p.meta,
        Some(json!({"keep":true,"importedOrigin":"builtin"}))
    );
}
#[test]
fn too_many_entries_rejected() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.svlab");
    let mut z = ZipWriter::new(fs::File::create(&p).unwrap());
    for i in 0..3000 {
        z.start_file(format!("{i}"), SimpleFileOptions::default())
            .unwrap();
    }
    z.finish().unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    assert_eq!(
        import_project(&s, &p).unwrap_err().code(),
        "PROJECT_TOO_LARGE"
    );
}
#[test]
fn duplicate_profiles_skipped() {
    for origin in ["import", "builtin"] {
        let d = tempfile::tempdir().unwrap();
        let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
        {
            let s = s.lock().unwrap();
            let p = s
                .put_profile(&style(), "Local", origin, None, None)
                .unwrap();
            s.rename_profile(p.id, "Local", &["local".into()]).unwrap();
        }
        let p = d.path().join("x.svlab");
        let m = manifest();
        archive(
            &p,
            &m,
            &[(
                m["profiles"][0]["file"].as_str().unwrap(),
                canonical_bytes(&style()),
            )],
        );
        let r = import_project(&s, &p).unwrap();
        assert_eq!((r.profiles, r.skipped), (0, 1));
        let s = s.lock().unwrap();
        let p = &s.profiles().unwrap()[0];
        assert_eq!(p.name, "Local");
        assert_eq!(p.tags, vec!["local"]);
        assert_eq!(p.origin, origin);
    }
}
#[test]
fn wrong_kind_rejected() {
    let mut m = manifest();
    m["kind"] = json!("wrong");
    import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
}
#[test]
fn revision_mismatch_reported() {
    let d = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    let p = d.path().join("x.svlab");
    let mut m = manifest();
    m["assetRevision"] = json!("other");
    archive(
        &p,
        &m,
        &[(
            m["profiles"][0]["file"].as_str().unwrap(),
            canonical_bytes(&style()),
        )],
    );
    assert!(import_project(&s, &p).unwrap().revision_mismatch);
}

#[test]
fn zip_bomb_with_forged_size_rejected() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("x.svlab");
    let m = manifest();
    archive(
        &path,
        &m,
        &[(
            m["profiles"][0]["file"].as_str().unwrap(),
            vec![b' '; 2 * 1024 * 1024 + 1],
        )],
    );
    let mut bytes = fs::read(&path).unwrap();
    // Lie about the uncompressed size in both local and central profile headers.
    for i in 0..bytes.len() - 46 {
        if bytes[i..].starts_with(b"PK\x03\x04") {
            let n = u16::from_le_bytes(bytes[i + 26..i + 28].try_into().unwrap()) as usize;
            if bytes[i + 30..i + 30 + n].starts_with(b"profiles/") {
                bytes[i + 22..i + 26].copy_from_slice(&1u32.to_le_bytes());
            }
        } else if bytes[i..].starts_with(b"PK\x01\x02") {
            let n = u16::from_le_bytes(bytes[i + 28..i + 30].try_into().unwrap()) as usize;
            if bytes[i + 46..i + 46 + n].starts_with(b"profiles/") {
                bytes[i + 24..i + 28].copy_from_slice(&1u32.to_le_bytes());
            }
        }
    }
    fs::write(&path, bytes).unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    assert_eq!(
        import_project(&s, &path).unwrap_err().code(),
        "PROJECT_TOO_LARGE"
    );
    assert!(s.lock().unwrap().profiles().unwrap().is_empty());
}

fn render_entry() -> Value {
    json!({"file":"renders/1.wav","text":"Привет","lang":"ru","speed":1.0,"quality":8,"recipe":null,"profileSha256":null})
}
#[test]
fn invalid_render_prevents_profile_application() {
    for bad in [b"not wav".to_vec(), {
        let mut w = wav();
        w.truncate(w.len() - 2);
        w
    }] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.svlab");
        let mut m = manifest();
        m["renders"] = json!([render_entry()]);
        archive(
            &path,
            &m,
            &[
                (
                    m["profiles"][0]["file"].as_str().unwrap(),
                    canonical_bytes(&style()),
                ),
                ("renders/1.wav", bad),
            ],
        );
        let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
        assert_eq!(
            import_project(&s, &path).unwrap_err().code(),
            "PROJECT_INVALID"
        );
        let s = s.lock().unwrap();
        assert!(s.profiles().unwrap().is_empty());
        assert_eq!(fs::read_dir(s.root().join("tmp")).unwrap().count(), 0);
    }
}
#[test]
fn invalid_recipe_rejected_for_profiles_and_renders() {
    let r = serde_json::to_value(new_recipe(
        vec![RecipeInput {
            profile_sha256: "a".repeat(64),
            name: "A".into(),
            weight: 100.0,
        }],
        DpStrategy::Weighted,
        None,
        "today".into(),
    ))
    .unwrap();
    for render in [false, true] {
        for mutation in 0..5 {
            let mut recipe = r.clone();
            match mutation {
                0 => recipe["kind"] = json!("wrong"),
                1 => recipe["schemaVersion"] = json!(2),
                2 => recipe["inputs"] = json!([]),
                3 => recipe["inputs"][0]["weight"] = json!(-1),
                _ => recipe["inputs"][0]["weight"] = json!(101),
            }
            let mut m = manifest();
            if render {
                let mut entry = render_entry();
                entry["recipe"] = recipe;
                m["renders"] = json!([entry]);
            } else {
                m["profiles"][0]["recipe"] = recipe;
            }
            import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
        }
    }
}
#[test]
fn duplicate_manifest_files_rejected() {
    let mut m = manifest();
    let p = m["profiles"][0].clone();
    m["profiles"].as_array_mut().unwrap().push(p);
    import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
}
#[test]
fn name_and_tag_limits_rejected() {
    for mutation in 0..3 {
        let mut m = manifest();
        match mutation {
            0 => m["profiles"][0]["name"] = json!("я".repeat(201)),
            1 => m["profiles"][0]["tags"] = json!(vec!["tag"; 21]),
            _ => m["profiles"][0]["tags"] = json!(["я".repeat(51)]),
        }
        import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
    }
}
#[test]
fn export_failure_cleans_tmp_and_preserves_destination() {
    let d = tempfile::tempdir().unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    s.lock()
        .unwrap()
        .put_profile(&style(), "A", "import", None, None)
        .unwrap();
    let sguard = s.lock().unwrap();
    fs::remove_file(sguard.profile_path(&sguard.profiles().unwrap()[0])).unwrap();
    drop(sguard);
    let path = d.path().join("x.svlab");
    fs::write(&path, b"previous").unwrap();
    assert_eq!(
        export_project(&s, &path).unwrap_err().code(),
        "EXPORT_FAILED"
    );
    assert_eq!(fs::read(&path).unwrap(), b"previous");
    assert!(!d.path().join("x.svlab.tmp").exists());
}

#[test]
fn too_many_duplicate_zip_entries_rejected() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("x.svlab");
    let mut z = ZipWriter::new(fs::File::create(&path).unwrap());
    for i in 0..2001 {
        z.start_file(format!("renders/{i:09}.wav"), SimpleFileOptions::default())
            .unwrap();
    }
    z.finish().unwrap();
    let mut bytes = fs::read(&path).unwrap();
    for i in 0..bytes.len() - 46 {
        if bytes[i..].starts_with(b"PK\x01\x02") {
            bytes[i + 46..i + 67].copy_from_slice(b"renders/000000000.wav");
        }
    }
    fs::write(&path, bytes).unwrap();
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    assert_eq!(
        import_project(&s, &path).unwrap_err().code(),
        "PROJECT_TOO_LARGE"
    );
}

#[test]
fn non_object_meta_preserved() {
    for meta in [Value::Null, json!([1, 2]), json!("original")] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.svlab");
        let mut m = manifest();
        m["profiles"][0]["meta"] = meta.clone();
        archive(
            &path,
            &m,
            &[(
                m["profiles"][0]["file"].as_str().unwrap(),
                canonical_bytes(&style()),
            )],
        );
        let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
        import_project(&s, &path).unwrap();
        assert_eq!(
            s.lock().unwrap().profiles().unwrap()[0].meta,
            Some(json!({"original":meta,"importedOrigin":"builtin"}))
        );
    }
}

#[test]
fn manifest_limit_enforced() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("x.svlab");
    let mut m = manifest();
    m["padding"] = json!("x".repeat(1024 * 1024));
    archive(&path, &m, &[]);
    let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
    assert_eq!(
        import_project(&s, &path).unwrap_err().code(),
        "PROJECT_TOO_LARGE"
    );
}
#[test]
fn render_params_validated_before_application() {
    for (key, value) in [
        ("text", json!("")),
        ("lang", json!("invalid")),
        ("speed", json!(2.0)),
        ("quality", json!(4)),
    ] {
        let mut m = manifest();
        let mut r = render_entry();
        r[key] = value;
        m["renders"] = json!([r]);
        import_fixture(&m, canonical_bytes(&style()), "PROJECT_INVALID");
    }
}
#[test]
fn wav_format_rejected() {
    for spec in [
        hound::WavSpec {
            channels: 2,
            sample_rate: 8000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
        hound::WavSpec {
            channels: 1,
            sample_rate: 7999,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
        hound::WavSpec {
            channels: 1,
            sample_rate: 48001,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 24,
            sample_format: hound::SampleFormat::Int,
        },
        hound::WavSpec {
            channels: 1,
            sample_rate: 8000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    ] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("x.svlab");
        let mut m = manifest();
        m["renders"] = json!([render_entry()]);
        let mut c = Cursor::new(Vec::new());
        hound::WavWriter::new(&mut c, spec)
            .unwrap()
            .finalize()
            .unwrap();
        archive(
            &path,
            &m,
            &[
                (
                    m["profiles"][0]["file"].as_str().unwrap(),
                    canonical_bytes(&style()),
                ),
                ("renders/1.wav", c.into_inner()),
            ],
        );
        let s = Mutex::new(Store::open(&d.path().join("data")).unwrap());
        assert_eq!(
            import_project(&s, &path).unwrap_err().code(),
            "PROJECT_INVALID"
        );
        assert!(s.lock().unwrap().profiles().unwrap().is_empty());
    }
}
