//! Map: a map from strings that keeps its keys in the order they were first inserted, for what the engine shows or
//! saves in an order of its own (roles, steps, a run's agents): iterating it, and writing it to state.json, gives that
//! order back.

use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, Serializer};

/// A map from strings that keeps its keys in insertion order: inserting a key it has replaces the value in place. Two
/// maps are equal when they hold the same keys with equal values, in any order. Lookups are linear, which the few
/// keys any of the engine's maps holds make cheap. It serializes as an object in its order, and deserializes from one
/// in the object's order, a repeated key keeping its first place and its last value.
#[derive(Clone, Debug)]
pub struct Map<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for Map<V> {
    fn default() -> Self {
        Map { entries: Vec::new() }
    }
}

impl<V> Map<V> {
    pub const fn new() -> Self {
        Map { entries: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Sets key to value: in its place when the map has key, else last.
    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        let key = key.into();
        match self.get_mut(&key) {
            Some(v) => *v = value,
            None => self.entries.push((key, value)),
        }
    }

    /// The value of key, taken out of the map; None when it has none.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let i = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(i).1)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

impl<V: PartialEq> PartialEq for Map<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl<V: Eq> Eq for Map<V> {}

impl<K: Into<String>, V> FromIterator<(K, V)> for Map<V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = Map::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<V> IntoIterator for Map<V> {
    type Item = (String, V);
    type IntoIter = std::vec::IntoIter<(String, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<V: Serialize> Serialize for Map<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Map<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

struct MapVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for MapVisitor<V> {
    type Value = Map<V>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Map<V>, A::Error> {
        let mut map = Map::new();
        while let Some((k, v)) = access.next_entry::<String, V>()? {
            map.insert(k, v);
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests;
