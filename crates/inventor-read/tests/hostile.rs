//! Damaged or hostile input must stay within bounded memory and time, without panics.

use inventor_read::synth::{self, Cache, Ending, Solid};
use inventor_read::{brep, oleps, Document};

/// A property set stream with raw values: (property id, offset into the value area).
fn raw_stream(props: &[(u32, usize)], values: &[u8]) -> Vec<u8> {
    let header = 8 + 8 * props.len();
    let mut set = Vec::new();
    set.extend(((header + values.len()) as u32).to_le_bytes());
    set.extend((props.len() as u32).to_le_bytes());
    for &(id, off) in props {
        set.extend(id.to_le_bytes());
        set.extend(((header + off) as u32).to_le_bytes());
    }
    set.extend(values);
    let mut out = vec![0xFE, 0xFF, 0, 0, 6, 0, 2, 0];
    out.extend([0u8; 16]);
    out.extend(1u32.to_le_bytes());
    out.extend([0x11u8; 16]);
    out.extend(48u32.to_le_bytes());
    out.extend(set);
    out
}

#[test]
fn nested_variants_do_not_recurse() {
    // 200 000 × VT_VARIANT: rejected, not followed
    let values: Vec<u8> = std::iter::repeat_n([0x0C, 0, 0, 0], 200_000).flatten().collect();
    let sets = oleps::parse_stream(&raw_stream(&[(2, 0)], &values)).unwrap();
    assert_eq!(sets[0].properties.len(), 1);
    // a vector of variants still reads
    let mut v = vec![0x0C, 0x10, 0, 0, 1, 0, 0, 0, 0x03, 0, 0, 0];
    v.extend(7i32.to_le_bytes());
    let sets = oleps::parse_stream(&raw_stream(&[(2, 0)], &v)).unwrap();
    assert_eq!(sets[0].get(2), Some(&oleps::Value::Vector(vec![oleps::Value::I4(7)])));
}

#[test]
fn shared_offsets_and_empty_vectors_are_bounded() {
    // VT_VECTOR | VT_EMPTY with a million elements, shared by 2000 properties
    let mut values = vec![0x00, 0x10, 0, 0];
    values.extend(1_000_000u32.to_le_bytes());
    let props: Vec<(u32, usize)> = (2..2002).map(|id| (id, 0)).collect();
    let sets = oleps::parse_stream(&raw_stream(&props, &values)).unwrap();
    assert!(sets[0].properties.iter().all(|p| !matches!(p.value, oleps::Value::Vector(_))));
    // a 64 KB blob shared by 2000 properties: decoding stops at the size of the set
    let mut values = vec![0x41, 0, 0, 0];
    values.extend((65_536u32).to_le_bytes());
    values.extend(vec![0xAB; 65_536]);
    let sets = oleps::parse_stream(&raw_stream(&props, &values)).unwrap();
    assert!(sets[0].properties.len() < 10, "{}", sets[0].properties.len());
}

#[test]
fn token_floods_stop_early() {
    // one record of three million one-byte tokens
    let mut seg = b"ASM BinaryFile4".to_vec();
    for v in [22800i32, 0, 2, 0] {
        seg.extend(v.to_le_bytes());
    }
    seg.extend(vec![0x0A; 3_000_000]);
    let b = brep::parse(&seg).unwrap_err();
    assert!(b.contains("empty"), "{b}");
    // and through a whole file (Zstandard packs it into a few kilobytes)
    let bytes = synth::part_properties(synth::File::new(), "X", "Modeling", "2026", 300_000_000).segment("PmBRepSegment", 2, &seg).to_bytes();
    assert!(bytes.len() < 200_000);
    assert!(Document::from_bytes(bytes).unwrap().scene().is_err());
}

#[test]
fn shared_cache_ranges_are_bounded() {
    let mut cache = Cache::default();
    cache.boxed([0.0; 3], [1.0; 3]).repeat_last(40_000);
    let doc = Document::from_bytes(synth::assembly(Some(&cache), &[])).unwrap();
    let scene = doc.scene().unwrap();
    let stats = scene.cache.unwrap();
    assert!(stats.over_budget > 0);
    assert_eq!(scene.meshes.len() + stats.over_budget, 40_001);
    assert!(scene.warnings.iter().any(|w| w.contains("more data than the cache holds")));
}

#[test]
fn intact_files_are_not_affected() {
    let doc = Document::from_bytes(synth::part(&Solid::cube(1.0), 1, 228, Ending::EndRecord)).unwrap();
    assert_eq!(doc.scene().unwrap().meshes[0].triangle_count(), 12);
}
