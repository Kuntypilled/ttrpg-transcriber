//! `SequenceMatcher` against Python's `difflib` on a fixed set of pairs
//! (`fixtures/difflib_pairs.json`, made with Python's `difflib` and kept as data):
//! every ratio must be exactly equal, as floats.

use std::fs;
use std::path::Path;

use serde_json::Value;
use ttrpg_core::difflib::SequenceMatcher;

#[test]
fn every_ratio_equals_the_one_python_computed() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/difflib_pairs.json");
    let pairs: Vec<Value> = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert!(pairs.len() > 900);
    let mut distinct = std::collections::BTreeSet::new();
    for p in &pairs {
        let (a, b) = (p["a"].as_str().unwrap(), p["b"].as_str().unwrap());
        let sm = SequenceMatcher::new(a, b);
        let want = |k: &str| p[k].as_str().unwrap().parse::<f64>().unwrap();
        assert_eq!(sm.ratio(), want("ratio"), "ratio of {a:?} and {b:?}");
        assert_eq!(sm.quick_ratio(), want("quick"), "quick_ratio of {a:?} and {b:?}");
        assert_eq!(
            sm.real_quick_ratio(),
            want("real_quick"),
            "real_quick_ratio of {a:?} and {b:?}"
        );
        distinct.insert(want("ratio").to_bits());
    }
    assert!(
        distinct.len() > 60,
        "the pairs cover many different scores: {}",
        distinct.len()
    );
}
