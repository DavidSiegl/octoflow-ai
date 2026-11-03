use super::*;

#[test]
fn maps_compare_without_regard_to_order_and_keep_their_own() {
    let a: Map<i64> = [("x", 1), ("y", 2)].into_iter().collect();
    let b: Map<i64> = [("y", 2), ("x", 1)].into_iter().collect();
    let c: Map<i64> = [("y", 2)].into_iter().collect();
    assert_eq!(a, b);
    assert_ne!(a, c);
    let mut d = a.clone();
    assert_eq!(d.remove("x"), Some(1));
    assert_eq!(d.remove("x"), None);
    assert_eq!(d.keys().collect::<Vec<_>>(), ["y"]);
}

#[test]
fn a_map_serializes_in_its_order_and_reads_back_in_the_objects() {
    let map: Map<i64> = [("b", 1), ("a", 2)].into_iter().collect();
    assert_eq!(serde_json::to_string(&map).unwrap(), r#"{"b":1,"a":2}"#);
    let read: Map<i64> = serde_json::from_str(r#"{"b": 1, "a": 2, "b": 3}"#).unwrap();
    assert_eq!(read.iter().collect::<Vec<_>>(), [("b", &3), ("a", &2)]);
}
