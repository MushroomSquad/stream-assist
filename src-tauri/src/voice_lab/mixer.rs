use ndarray::{Array3, Axis};
use rand::{RngCore, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use supertonic3_tts::Style;

use super::{VlError, err, models::MODEL_REVISION};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum DpStrategy {
    Dominant,
    Weighted,
    Select,
}

pub fn interp_weights(points: usize) -> Vec<[f64; 2]> {
    (0..points)
        .map(|i| {
            let t = i as f64 / (points - 1) as f64;
            [100.0 * (1.0 - t), 100.0 * t]
        })
        .collect()
}

pub fn dirichlet_weights(n: usize, k: usize, seed: u32) -> Vec<Vec<f64>> {
    let mut key = [0; 32];
    key[..4].copy_from_slice(&seed.to_le_bytes());
    let mut rng = ChaCha8Rng::from_seed(key);
    (0..k)
        .map(|_| {
            let mut weights: Vec<_> = (0..n)
                .map(|_| {
                    let u = (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
                    -(1.0 - u).ln()
                })
                .collect();
            let total: f64 = weights.iter().sum();
            for weight in &mut weights {
                *weight = 100.0 * *weight / total;
            }
            weights
        })
        .collect()
}

pub fn mix(
    inputs: &[(&Style, f64)],
    dp: DpStrategy,
    select_index: Option<usize>,
) -> Result<Style, VlError> {
    if !(1..=8).contains(&inputs.len())
        || inputs
            .iter()
            .any(|(_, w)| !w.is_finite() || !(0.0..=100.0).contains(w))
        || (dp == DpStrategy::Select && select_index.is_none_or(|i| i >= inputs.len()))
    {
        return Err(err("MIX_INVALID"));
    }
    let total: f64 = inputs.iter().map(|(_, w)| w).sum();
    if total == 0.0 {
        return Err(err("ZERO_MIX_WEIGHTS"));
    }
    if inputs.len() == 1 {
        return Ok(inputs[0].0.clone());
    }
    let ttl = mix_rows(inputs.iter().map(|(s, w)| (&s.ttl, w / total)));
    let dp = match dp {
        DpStrategy::Weighted => mix_rows(inputs.iter().map(|(s, w)| (&s.dp, w / total))),
        DpStrategy::Dominant => {
            let mut dominant = 0;
            for i in 1..inputs.len() {
                if inputs[i].1 > inputs[dominant].1 {
                    dominant = i;
                }
            }
            inputs[dominant].0.dp.clone()
        }
        DpStrategy::Select => inputs[select_index.expect("validated select index")]
            .0
            .dp
            .clone(),
    };
    Ok(Style { ttl, dp })
}

fn mix_rows<'a>(inputs: impl Iterator<Item = (&'a Array3<f32>, f64)>) -> Array3<f32> {
    let inputs: Vec<_> = inputs.collect();
    let mut mixed = Array3::<f64>::zeros(inputs[0].0.raw_dim());
    for (tensor, weight) in &inputs {
        mixed.zip_mut_with(tensor, |sum, &value| *sum += weight * f64::from(value));
    }
    for (r, mut row) in mixed
        .index_axis_mut(Axis(0), 0)
        .rows_mut()
        .into_iter()
        .enumerate()
    {
        let target: f64 = inputs
            .iter()
            .map(|(tensor, weight)| {
                let batch = tensor.index_axis(Axis(0), 0);
                weight
                    * batch
                        .row(r)
                        .iter()
                        .map(|&x| f64::from(x).powi(2))
                        .sum::<f64>()
                        .sqrt()
            })
            .sum();
        let norm = row.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm >= 1e-12 {
            row *= target / norm;
        }
    }
    mixed.mapv(|x| x as f32)
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Recipe {
    pub schema_version: u32,
    pub kind: String,
    pub engine: EngineRef,
    pub inputs: Vec<RecipeInput>,
    pub dp_strategy: DpStrategy,
    pub selected_dp_profile_sha256: Option<String>,
    pub created_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineRef {
    pub id: String,
    pub version: String,
    pub asset_revision: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecipeInput {
    pub profile_sha256: String,
    pub name: String,
    pub weight: f64,
}

pub fn new_recipe(
    inputs: Vec<RecipeInput>,
    dp: DpStrategy,
    selected: Option<String>,
    created_at: String,
) -> Recipe {
    Recipe {
        schema_version: 1,
        kind: "voice-lab-recipe".into(),
        engine: EngineRef {
            id: "supertonic".into(),
            version: "3".into(),
            asset_revision: MODEL_REVISION.into(),
        },
        inputs,
        dp_strategy: dp,
        selected_dp_profile_sha256: selected,
        created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::{Array3, Axis};

    fn unit_style(axis: usize) -> Style {
        Style {
            ttl: Array3::from_shape_fn((1, 50, 256), |(_, _, c)| f32::from(c == axis)),
            dp: Array3::from_shape_fn((1, 8, 16), |(_, _, c)| f32::from(c == axis)),
        }
    }

    fn assert_style_eq(a: &Style, b: &Style) {
        assert_eq!(a.ttl, b.ttl);
        assert_eq!(a.dp, b.dp);
    }

    fn assert_unit_rows(tensor: &Array3<f32>) {
        for row in tensor.index_axis(Axis(0), 0).rows() {
            let norm = row
                .iter()
                .map(|&x| f64::from(x).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
            assert!((row[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
            assert!((row[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        }
    }

    #[test]
    fn interp_endpoints() {
        assert_eq!(
            interp_weights(5),
            vec![
                [100.0, 0.0],
                [75.0, 25.0],
                [50.0, 50.0],
                [25.0, 75.0],
                [0.0, 100.0]
            ]
        );
    }

    #[test]
    fn dirichlet_is_seeded() {
        let reference = [69.53124093477585, 30.19481578700302, 0.2739432782211399];
        let weights = dirichlet_weights(3, 1, 7);
        for (actual, expected) in weights[0].iter().zip(reference) {
            assert!((actual - expected).abs() < 1e-12);
        }
        assert_eq!(dirichlet_weights(3, 8, 7), dirichlet_weights(3, 8, 7));
        assert_ne!(dirichlet_weights(3, 8, 7), dirichlet_weights(3, 8, 8));
    }

    #[test]
    fn dirichlet_sums_to_100() {
        for n in 2..=8 {
            for k in 2..=8 {
                for seed in [0, 7, u32::MAX] {
                    let sets = dirichlet_weights(n, k, seed);
                    assert_eq!(sets.len(), k);
                    for weights in sets {
                        assert_eq!(weights.len(), n);
                        assert!(weights.iter().all(|&w| w > 0.0));
                        assert!((weights.iter().sum::<f64>() - 100.0).abs() < 1e-9);
                    }
                }
            }
        }
    }

    #[test]
    fn dirichlet_mixes_ok() {
        for n in 2..=8 {
            let styles: Vec<_> = (0..n).map(unit_style).collect();
            for weights in dirichlet_weights(n, 8, 7) {
                let inputs: Vec<_> = styles.iter().zip(weights).collect();
                for dp in [DpStrategy::Dominant, DpStrategy::Weighted] {
                    let mixed = mix(&inputs, dp, None).unwrap();
                    assert!(
                        mixed
                            .ttl
                            .iter()
                            .chain(mixed.dp.iter())
                            .all(|x| x.is_finite())
                    );
                }
            }
        }
    }

    #[test]
    fn single_input_is_identity() {
        let mut s = unit_style(0);
        s.ttl *= 3.0;
        for dp in [
            DpStrategy::Dominant,
            DpStrategy::Weighted,
            DpStrategy::Select,
        ] {
            assert_style_eq(&mix(&[(&s, 25.0)], dp, Some(0)).unwrap(), &s);
        }
    }

    #[test]
    fn mix_keeps_row_norms() {
        let (a, b) = (unit_style(0), unit_style(1));
        assert_unit_rows(
            &mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Dominant, None)
                .unwrap()
                .ttl,
        );
    }

    #[test]
    fn weights_are_normalized() {
        let (a, b, c) = (unit_style(0), unit_style(1), unit_style(2));
        let x = mix(
            &[(&a, 55.0), (&b, 30.0), (&c, 15.0)],
            DpStrategy::Weighted,
            None,
        )
        .unwrap();
        let y = mix(
            &[(&a, 11.0), (&b, 6.0), (&c, 3.0)],
            DpStrategy::Weighted,
            None,
        )
        .unwrap();
        assert_style_eq(&x, &y);
    }

    #[test]
    fn parallel_rows_scale_linearly() {
        let a = unit_style(0);
        let mut b = a.clone();
        b.ttl *= 3.0;
        b.dp *= 3.0;
        let mut expected = a.clone();
        expected.ttl *= 2.0;
        expected.dp *= 2.0;
        assert_style_eq(
            &mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Weighted, None).unwrap(),
            &expected,
        );
    }

    #[test]
    fn dp_dominant_picks_max() {
        let (a, b) = (unit_style(0), unit_style(1));
        assert_eq!(
            mix(&[(&a, 20.0), (&b, 80.0)], DpStrategy::Dominant, None)
                .unwrap()
                .dp,
            b.dp
        );
    }

    #[test]
    fn dp_dominant_tie_picks_first() {
        let (a, b) = (unit_style(0), unit_style(1));
        assert_eq!(
            mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Dominant, None)
                .unwrap()
                .dp,
            a.dp
        );
    }

    #[test]
    fn dp_weighted_normalizes_rows() {
        let (a, b) = (unit_style(0), unit_style(1));
        assert_unit_rows(
            &mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Weighted, None)
                .unwrap()
                .dp,
        );
    }

    #[test]
    fn dp_select_uses_index() {
        let (a, b) = (unit_style(0), unit_style(1));
        assert_eq!(
            mix(&[(&a, 100.0), (&b, 0.0)], DpStrategy::Select, Some(1))
                .unwrap()
                .dp,
            b.dp
        );
    }

    #[test]
    fn invalid_inputs() {
        let (a, b) = (unit_style(0), unit_style(1));
        for inputs in [vec![], vec![(&a, 1.0); 9], vec![(&a, -1.0)]] {
            assert_eq!(
                mix(&inputs, DpStrategy::Dominant, None).unwrap_err().code(),
                "MIX_INVALID"
            );
        }
        for weight in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 100.5] {
            assert_eq!(
                mix(&[(&a, 1.0), (&b, weight)], DpStrategy::Dominant, None)
                    .unwrap_err()
                    .code(),
                "MIX_INVALID"
            );
        }
        for index in [None, Some(5)] {
            assert_eq!(
                mix(&[(&a, 1.0), (&b, 1.0)], DpStrategy::Select, index)
                    .unwrap_err()
                    .code(),
                "MIX_INVALID"
            );
        }
        assert_eq!(
            mix(&[(&a, 1.0)], DpStrategy::Select, Some(1))
                .unwrap_err()
                .code(),
            "MIX_INVALID"
        );
        for inputs in [vec![(&a, 0.0)], vec![(&a, 0.0), (&b, 0.0)]] {
            assert_eq!(
                mix(&inputs, DpStrategy::Dominant, None).unwrap_err().code(),
                "ZERO_MIX_WEIGHTS"
            );
        }
    }

    #[test]
    fn shapes_preserved() {
        let (a, b) = (unit_style(0), unit_style(1));
        let result = mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Weighted, None).unwrap();
        assert_eq!(result.ttl.shape(), &[1, 50, 256]);
        assert_eq!(result.dp.shape(), &[1, 8, 16]);
    }

    #[test]
    fn cancelling_rows_stay_finite() {
        let a = unit_style(0);
        let mut b = a.clone();
        b.ttl *= -1.0;
        b.dp *= -1.0;
        let result = mix(&[(&a, 50.0), (&b, 50.0)], DpStrategy::Weighted, None).unwrap();
        assert!(result.ttl.iter().chain(result.dp.iter()).all(|&x| x == 0.0));
    }

    #[test]
    fn recipe_json_keys() {
        let inputs = vec![RecipeInput {
            profile_sha256: "abc".into(),
            name: "A".into(),
            weight: 55.0,
        }];
        let recipe = new_recipe(
            inputs.clone(),
            DpStrategy::Dominant,
            None,
            "2026-11-02".into(),
        );
        let json = serde_json::to_value(&recipe).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "schemaVersion": 1, "kind": "voice-lab-recipe",
                "engine": {"id": "supertonic", "version": "3", "assetRevision": MODEL_REVISION},
                "inputs": [{"profileSha256": "abc", "name": "A", "weight": 55.0}],
                "dpStrategy": "dominant", "selectedDpProfileSha256": null, "createdAt": "2026-11-02"
            })
        );
        assert_eq!(serde_json::from_value::<Recipe>(json).unwrap(), recipe);
        for (dp, name) in [
            (DpStrategy::Weighted, "weighted"),
            (DpStrategy::Select, "select"),
        ] {
            let recipe = new_recipe(inputs.clone(), dp, Some("abc".into()), "now".into());
            let json = serde_json::to_value(&recipe).unwrap();
            assert_eq!(json["dpStrategy"], name);
            assert_eq!(json["selectedDpProfileSha256"], "abc");
            assert_eq!(serde_json::from_value::<Recipe>(json).unwrap(), recipe);
        }
    }
}
