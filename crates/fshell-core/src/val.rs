// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_hash::{FxBuildHasher, FxHashMap};
use indexmap::IndexMap;
use std::path::PathBuf;
use std::sync::Arc;
use ustr::Ustr;

/// Custom alias for IndexMap using FxBuildHasher for fast, order-preserving lookups.
pub type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;

/// Unique identifier for a node within an ObjectGraph.
pub type NodeId = u64;

/// Custom serde helper modules for Ustr and FxIndexMap containing Ustr.
pub mod ustr_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use ustr::{Ustr, ustr};

    pub fn serialize<S>(val: &Ustr, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(val.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Ustr, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(ustr(&s))
    }
}

pub mod map_ustr_serde {
    use super::{FxIndexMap, Val};
    use fshell_hash::FxBuildHasher;
    use serde::{Deserialize, Deserializer, Serializer};
    use ustr::{Ustr, ustr};

    pub fn serialize<S>(map: &FxIndexMap<Ustr, Val>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(map.len()))?;
        for (k, v) in map {
            seq.serialize_element(&(k.as_str(), v))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<FxIndexMap<Ustr, Val>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let vec = Vec::<(String, Val)>::deserialize(deserializer)?;
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        for (k, v) in vec {
            map.insert(ustr(&k), v);
        }
        Ok(map)
    }
}

fn serialize_float<S>(f: &f64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if f.is_nan() {
        serializer.serialize_str("NaN")
    } else if f.is_infinite() {
        if f.is_sign_positive() {
            serializer.serialize_str("inf")
        } else {
            serializer.serialize_str("-inf")
        }
    } else {
        serializer.serialize_f64(*f)
    }
}

fn deserialize_float<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct FloatVisitor;
    impl<'de> serde::de::Visitor<'de> for FloatVisitor {
        type Value = f64;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a float or the string \"NaN\"")
        }
        fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v)
        }
        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v as f64)
        }
        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(v as f64)
        }
        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            match v {
                "NaN" => Ok(f64::NAN),
                "inf" | "Infinity" | "+inf" | "+Infinity" => Ok(f64::INFINITY),
                "-inf" | "-Infinity" => Ok(f64::NEG_INFINITY),
                _ => v.parse::<f64>().map_err(serde::de::Error::custom),
            }
        }
    }
    deserializer.deserialize_any(FloatVisitor)
}

/// A value in fshell's gradual type system.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(
        #[serde(
            serialize_with = "serialize_float",
            deserialize_with = "deserialize_float"
        )]
        f64,
    ),
    String(String),
    List(Vec<Val>),
    #[serde(with = "map_ustr_serde")]
    Map(FxIndexMap<Ustr, Val>),
    DateTime(chrono::DateTime<chrono::Utc>),
    Blob(Vec<u8>),
    ObjectGraph {
        root: NodeId,
        graph: Arc<GraphStorage>,
    },
    Capability(ResourceHandle),
    #[serde(skip)]
    ReactiveStream(#[serde(skip)] tokio::sync::watch::Receiver<Vec<Val>>),
}

impl PartialEq for Val {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Val::Null, Val::Null) => true,
            (Val::Bool(a), Val::Bool(b)) => a == b,
            (Val::Int(a), Val::Int(b)) => a == b,
            (Val::Float(a), Val::Float(b)) => {
                if a.is_nan() && b.is_nan() {
                    true
                } else {
                    a == b
                }
            }
            (Val::String(a), Val::String(b)) => a == b,
            (Val::List(a), Val::List(b)) => a == b,
            (Val::Map(a), Val::Map(b)) => a == b,
            (Val::DateTime(a), Val::DateTime(b)) => a == b,
            (Val::Blob(a), Val::Blob(b)) => a == b,
            (
                Val::ObjectGraph {
                    root: r1,
                    graph: g1,
                },
                Val::ObjectGraph {
                    root: r2,
                    graph: g2,
                },
            ) => r1 == r2 && (Arc::ptr_eq(g1, g2) || *g1 == *g2),
            (Val::Capability(a), Val::Capability(b)) => a == b,
            (Val::ReactiveStream(a), Val::ReactiveStream(b)) => *a.borrow() == *b.borrow(),
            _ => false,
        }
    }
}

/// A handle for capability-based resource access control.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Hash)]
pub enum ResourceHandle {
    ReadDir(PathBuf),
    WriteDir(PathBuf),
    ReadFile(PathBuf),
    WriteFile(PathBuf),
    NetworkSocket(String), // Host/domain constraint
    NetworkAll,            // Full network access
    ReadEnv(String),
    WriteEnv(String),
    ProcessSpawn,
    ProcessSpawnPath(String),
}

/// In-memory storage for an object graph's nodes and edges.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct GraphStorage {
    pub nodes: FxHashMap<NodeId, NodeData>,
    pub edges: FxHashMap<NodeId, Vec<EdgeData>>,
}

/// A node in an ObjectGraph with associated properties.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct NodeData {
    #[serde(with = "ustr_serde")]
    pub label: Ustr,
    #[serde(with = "map_ustr_serde")]
    pub properties: FxIndexMap<Ustr, Val>,
}

/// A directed edge in an ObjectGraph connecting to a target node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EdgeData {
    pub target: NodeId,
    #[serde(with = "ustr_serde")]
    pub label: Ustr,
    #[serde(with = "map_ustr_serde")]
    pub properties: FxIndexMap<Ustr, Val>,
}

impl Val {
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::Null => "Null",
            Val::Bool(_) => "Bool",
            Val::Int(_) => "Int",
            Val::Float(_) => "Float",
            Val::String(_) => "String",
            Val::List(_) => "List",
            Val::Map(_) => "Map",
            Val::DateTime(_) => "DateTime",
            Val::Blob(_) => "Blob",
            Val::ObjectGraph { .. } => "ObjectGraph",
            Val::Capability(_) => "Capability",
            Val::ReactiveStream(_) => "ReactiveStream",
        }
    }

    pub fn to_int(&self) -> Option<i64> {
        match self {
            Val::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn to_float(&self) -> Option<f64> {
        match self {
            Val::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn to_bool(&self) -> Option<bool> {
        match self {
            Val::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::String(s) => Some(s),
            _ => None,
        }
    }

    /// Utility function to create a new empty map.
    pub fn empty_map() -> Self {
        Val::Map(IndexMap::with_hasher(FxBuildHasher::default()))
    }

    /// Convert a Val to plain text representation.
    ///
    /// Lists are newline-separated, maps are JSON (with a key: value fallback).
    /// This is the canonical text serialization used by the CLI and @text operator.
    pub fn to_text(&self) -> String {
        match self {
            Val::Null => String::new(),
            Val::Bool(b) => b.to_string(),
            Val::Int(i) => i.to_string(),
            Val::Float(f) => f.to_string(),
            Val::String(s) => s.clone(),
            Val::DateTime(dt) => dt.to_rfc3339(),
            Val::Blob(b) => String::from_utf8_lossy(b).into_owned(),
            Val::List(list) => {
                let items: Vec<String> = list.iter().map(Val::to_text).collect();
                items.join("\n")
            }
            Val::Map(_) => match serde_json::to_string(self) {
                Ok(s) => s,
                Err(_) => {
                    let items: Vec<String> = self
                        .as_map()
                        .iter()
                        .map(|(k, v)| format!("{}: {}", k, v.to_text()))
                        .collect();
                    items.join("\n")
                }
            },
            Val::ObjectGraph { root, .. } => format!("ObjectGraph({})", root),
            Val::Capability(c) => format!("Capability({:?})", c),
            Val::ReactiveStream(_) => "ReactiveStream".to_string(),
        }
    }

    /// Helper to extract map entries (for non-panicking access).
    fn as_map(&self) -> &FxIndexMap<Ustr, Val> {
        match self {
            Val::Map(m) => m,
            _ => {
                static EMPTY: std::sync::LazyLock<FxIndexMap<Ustr, Val>> =
                    std::sync::LazyLock::new(|| FxIndexMap::with_hasher(FxBuildHasher::default()));
                &EMPTY
            }
        }
    }
}

impl From<serde_json::Value> for Val {
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Val::Null,
            serde_json::Value::Bool(b) => Val::Bool(b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Val::Int(i)
                } else if let Some(f) = n.as_f64() {
                    Val::Float(f)
                } else {
                    Val::String(n.to_string())
                }
            }
            serde_json::Value::String(s) => Val::String(s),
            serde_json::Value::Array(arr) => Val::List(arr.into_iter().map(Val::from).collect()),
            serde_json::Value::Object(obj) => {
                let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
                for (k, v) in obj {
                    map.insert(ustr::ustr(&k), Val::from(v));
                }
                Val::Map(map)
            }
        }
    }
}

impl From<&Val> for serde_json::Value {
    fn from(val: &Val) -> Self {
        match val {
            Val::Null => serde_json::Value::Null,
            Val::Bool(b) => serde_json::Value::Bool(*b),
            Val::Int(i) => serde_json::Value::Number((*i).into()),
            Val::Float(f) => {
                if let Some(n) = serde_json::Number::from_f64(*f) {
                    serde_json::Value::Number(n)
                } else {
                    serde_json::Value::String(f.to_string())
                }
            }
            Val::String(s) => serde_json::Value::String(s.clone()),
            Val::List(vals) => {
                serde_json::Value::Array(vals.iter().map(serde_json::Value::from).collect())
            }
            Val::Map(map) => {
                let mut obj = serde_json::Map::new();
                for (k, v) in map {
                    obj.insert(k.to_string(), serde_json::Value::from(v));
                }
                serde_json::Value::Object(obj)
            }
            Val::DateTime(dt) => serde_json::Value::String(dt.to_rfc3339()),
            Val::Blob(b) => serde_json::Value::Array(
                b.iter()
                    .map(|&byte| serde_json::Value::Number(byte.into()))
                    .collect(),
            ),
            Val::Capability(handle) => serde_json::Value::String(format!("{handle:?}")),
            other => serde_json::Value::String(other.to_text()),
        }
    }
}

impl From<Val> for serde_json::Value {
    fn from(val: Val) -> Self {
        (&val).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_hash::FxBuildHasher;
    use indexmap::IndexMap;
    use std::sync::Arc;
    use ustr::ustr;

    #[test]
    fn test_basic_variants() {
        let val_int = Val::Int(42);
        let val_str = Val::String("hello".to_string());
        let val_bool = Val::Bool(true);

        assert_eq!(val_int, Val::Int(42));
        assert_eq!(val_str, Val::String("hello".to_string()));
        assert_eq!(val_bool, Val::Bool(true));
    }

    #[test]
    fn test_map_order_preservation() {
        let mut map = IndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("z_key"), Val::Int(1));
        map.insert(ustr("a_key"), Val::Int(2));
        map.insert(ustr("m_key"), Val::Int(3));

        let val_map = Val::Map(map);

        let json_str = serde_json::to_string(&val_map)
            .unwrap_or_else(|e| panic!("Failed to serialize Map: {e}"));

        assert!(json_str.contains("z_key"));
        let pos_z = json_str.find("z_key").unwrap();
        let pos_a = json_str.find("a_key").unwrap();
        let pos_m = json_str.find("m_key").unwrap();

        assert!(pos_z < pos_a);
        assert!(pos_a < pos_m);
    }

    // PartialEq edge cases

    #[test]
    fn test_nan_equality() {
        let nan_a = Val::Float(f64::NAN);
        let nan_b = Val::Float(f64::NAN);
        assert_eq!(nan_a, nan_b);
    }

    #[test]
    fn test_nan_vs_int_not_equal() {
        let nan = Val::Float(f64::NAN);
        let int = Val::Int(0);
        assert_ne!(nan, int);
    }

    #[test]
    fn test_nan_vs_float_not_equal() {
        let nan = Val::Float(f64::NAN);
        let pi = Val::Float(std::f64::consts::PI);
        assert_ne!(nan, pi);
    }

    #[test]
    fn test_float_vs_int_not_equal() {
        let f = Val::Float(3.0);
        let i = Val::Int(3);
        assert_ne!(f, i);
    }

    #[test]
    fn test_float_equality() {
        assert_eq!(Val::Float(1.5), Val::Float(1.5));
        assert_eq!(Val::Float(-0.0), Val::Float(0.0));
    }

    #[test]
    fn test_object_graph_same_arc_eq() {
        let storage = Arc::new(GraphStorage {
            nodes: FxHashMap::default(),
            edges: FxHashMap::default(),
        });
        let a = Val::ObjectGraph {
            root: 1,
            graph: Arc::clone(&storage),
        };
        let b = Val::ObjectGraph {
            root: 1,
            graph: storage,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_object_graph_different_arc_eq() {
        let storage_a = Arc::new(GraphStorage {
            nodes: FxHashMap::default(),
            edges: FxHashMap::default(),
        });
        let storage_b = Arc::new(GraphStorage {
            nodes: FxHashMap::default(),
            edges: FxHashMap::default(),
        });
        let a = Val::ObjectGraph {
            root: 1,
            graph: storage_a,
        };
        let b = Val::ObjectGraph {
            root: 1,
            graph: storage_b,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn test_object_graph_different_root_not_eq() {
        let storage = Arc::new(GraphStorage {
            nodes: FxHashMap::default(),
            edges: FxHashMap::default(),
        });
        let a = Val::ObjectGraph {
            root: 1,
            graph: Arc::clone(&storage),
        };
        let b = Val::ObjectGraph {
            root: 2,
            graph: storage,
        };
        assert_ne!(a, b);
    }

    #[test]
    fn test_reactive_stream_equality() {
        let (tx1, rx1) = tokio::sync::watch::channel(vec![Val::Int(1)]);
        let (tx2, rx2) = tokio::sync::watch::channel(vec![Val::Int(2)]);
        let (tx3, rx3) = tokio::sync::watch::channel(vec![Val::Int(1)]);
        let a = Val::ReactiveStream(rx1);
        let b = Val::ReactiveStream(rx2);
        let c = Val::ReactiveStream(rx3);
        assert_ne!(a, b, "streams with different content should not be equal");
        assert_eq!(a, c, "streams with same content should be equal");
        drop(tx1);
        drop(tx2);
        drop(tx3);
    }

    #[test]
    fn test_reactive_stream_vs_null_not_equal() {
        let (_tx, rx) = tokio::sync::watch::channel(vec![Val::Int(1)]);
        assert_ne!(Val::ReactiveStream(rx), Val::Null);
    }

    #[test]
    fn test_null_equality() {
        assert_eq!(Val::Null, Val::Null);
    }

    #[test]
    fn test_null_vs_bool_not_equal() {
        assert_ne!(Val::Null, Val::Bool(false));
    }

    #[test]
    fn test_null_vs_int_not_equal() {
        assert_ne!(Val::Null, Val::Int(0));
    }

    #[test]
    fn test_null_vs_string_not_equal() {
        assert_ne!(Val::Null, Val::String(String::new()));
    }

    #[test]
    fn test_bool_equality() {
        assert_eq!(Val::Bool(true), Val::Bool(true));
        assert_eq!(Val::Bool(false), Val::Bool(false));
        assert_ne!(Val::Bool(true), Val::Bool(false));
    }

    #[test]
    fn test_blob_equality() {
        assert_eq!(Val::Blob(vec![1, 2, 3]), Val::Blob(vec![1, 2, 3]));
        assert_ne!(Val::Blob(vec![1, 2, 3]), Val::Blob(vec![1, 2]));
        assert_ne!(Val::Blob(vec![1, 2, 3]), Val::Null);
    }

    #[test]
    fn test_capability_equality() {
        let a = Val::Capability(ResourceHandle::ProcessSpawn);
        let b = Val::Capability(ResourceHandle::ProcessSpawn);
        let c = Val::Capability(ResourceHandle::NetworkAll);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_capability_vs_int_not_equal() {
        assert_ne!(Val::Capability(ResourceHandle::ProcessSpawn), Val::Int(42));
    }

    #[test]
    fn test_datetime_equality() {
        use chrono::TimeZone;
        let dt1 = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let dt2 = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let dt3 = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(Val::DateTime(dt1), Val::DateTime(dt2));
        assert_ne!(Val::DateTime(dt1), Val::DateTime(dt3));
    }

    #[test]
    fn test_datetime_vs_null_not_equal() {
        use chrono::TimeZone;
        let dt = chrono::Utc
            .with_ymd_and_hms(2024, 6, 15, 12, 30, 0)
            .unwrap();
        assert_ne!(Val::DateTime(dt), Val::Null);
    }

    #[test]
    fn test_string_vs_int_not_equal() {
        assert_ne!(Val::Int(42), Val::String("42".to_string()));
    }

    #[test]
    fn test_list_vs_list_not_equal() {
        assert_eq!(
            Val::List(vec![Val::Int(1), Val::Int(2)]),
            Val::List(vec![Val::Int(1), Val::Int(2)])
        );
        assert_ne!(
            Val::List(vec![Val::Int(1)]),
            Val::List(vec![Val::Int(1), Val::Int(2)])
        );
    }

    // Val::empty_map()

    #[test]
    fn test_empty_map_creates_empty() {
        let m = Val::empty_map();
        assert!(matches!(m, Val::Map(ref map) if map.is_empty()));
    }

    #[test]
    fn test_empty_map_is_map_variant() {
        assert!(matches!(Val::empty_map(), Val::Map(_)));
    }

    #[test]
    fn test_empty_map_can_insert_into() {
        let Val::Map(mut map) = Val::empty_map() else {
            panic!("expected Map");
        };
        map.insert(ustr("key"), Val::Int(1));
        assert_eq!(map.len(), 1);
    }

    // Serde roundtrip

    #[test]
    fn test_serde_roundtrip_int() {
        let v = Val::Int(-999);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_string() {
        let v = Val::String("hello world".to_string());
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_bool_true() {
        let v = Val::Bool(true);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_bool_false() {
        let v = Val::Bool(false);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_null() {
        let v = Val::Null;
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_float() {
        let v = Val::Float(3.14159);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_json_nan_serializes_as_nan_string() {
        // NaN serializes as a "NaN" string sentinel to prevent data corruption/loss
        let v = Val::Float(f64::NAN);
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(json, r#"{"type":"Float","value":"NaN"}"#);
    }

    #[test]
    fn test_serde_roundtrip_list() {
        let v = Val::List(vec![
            Val::Int(1),
            Val::String("two".to_string()),
            Val::Bool(false),
        ]);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_roundtrip_map() {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("name"), Val::String("fshell".to_string()));
        map.insert(ustr("version"), Val::Int(1));
        let v = Val::Map(map);
        let json = serde_json::to_string(&v).unwrap();
        let back: Val = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn test_serde_json_structure_int() {
        let json = serde_json::to_string(&Val::Int(42)).unwrap();
        assert_eq!(json, r#"{"type":"Int","value":42}"#);
    }

    #[test]
    fn test_serde_json_structure_string() {
        let json = serde_json::to_string(&Val::String("hi".to_string())).unwrap();
        assert_eq!(json, r#"{"type":"String","value":"hi"}"#);
    }

    #[test]
    fn test_serde_json_structure_bool() {
        let json = serde_json::to_string(&Val::Bool(true)).unwrap();
        assert_eq!(json, r#"{"type":"Bool","value":true}"#);
    }

    #[test]
    fn test_serde_json_structure_null() {
        let json = serde_json::to_string(&Val::Null).unwrap();
        assert_eq!(json, r#"{"type":"Null"}"#);
    }

    #[test]
    fn test_serde_json_structure_float() {
        let json = serde_json::to_string(&Val::Float(2.5)).unwrap();
        assert_eq!(json, r#"{"type":"Float","value":2.5}"#);
    }

    // ResourceHandle

    #[test]
    fn test_resource_handle_debug_output() {
        let h = ResourceHandle::NetworkSocket("example.com".to_string());
        let debug = format!("{:?}", h);
        assert!(debug.contains("NetworkSocket"));
        assert!(debug.contains("example.com"));
    }

    #[test]
    fn test_resource_handle_debug_read_dir() {
        let h = ResourceHandle::ReadDir("/tmp".into());
        let debug = format!("{:?}", h);
        assert!(debug.contains("ReadDir"));
        assert!(debug.contains("/tmp"));
    }

    #[test]
    fn test_resource_handle_equality() {
        assert_eq!(ResourceHandle::ProcessSpawn, ResourceHandle::ProcessSpawn);
        assert_eq!(ResourceHandle::NetworkAll, ResourceHandle::NetworkAll);
    }

    #[test]
    fn test_resource_handle_inequality() {
        assert_ne!(ResourceHandle::ProcessSpawn, ResourceHandle::NetworkAll);
        assert_ne!(
            ResourceHandle::ReadDir("/a".into()),
            ResourceHandle::ReadDir("/b".into())
        );
        assert_ne!(
            ResourceHandle::WriteDir("/a".into()),
            ResourceHandle::ReadDir("/a".into())
        );
    }

    #[test]
    fn test_resource_handle_env_variants() {
        let read = ResourceHandle::ReadEnv("PATH".to_string());
        let write = ResourceHandle::WriteEnv("PATH".to_string());
        assert_ne!(read, write);
        assert_eq!(read, ResourceHandle::ReadEnv("PATH".to_string()));
    }

    #[test]
    fn test_resource_handle_network_socket() {
        let a = ResourceHandle::NetworkSocket("api.example.com".to_string());
        let b = ResourceHandle::NetworkSocket("api.example.com".to_string());
        let c = ResourceHandle::NetworkSocket("other.example.com".to_string());
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    // Map with FxBuildHasher

    #[test]
    fn test_map_insertion_order() {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("first"), Val::Int(1));
        map.insert(ustr("second"), Val::Int(2));
        map.insert(ustr("third"), Val::Int(3));
        let v = Val::Map(map);

        let Val::Map(inner) = &v else {
            panic!("not a map")
        };
        let keys: Vec<_> = inner.keys().cloned().collect();
        assert_eq!(keys, vec![ustr("first"), ustr("second"), ustr("third")]);
    }

    #[test]
    fn test_map_key_lookup() {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("a"), Val::Int(10));
        map.insert(ustr("b"), Val::String("bee".to_string()));
        let v = Val::Map(map);

        let Val::Map(inner) = &v else {
            panic!("not a map")
        };
        assert_eq!(inner.get(&ustr("a")), Some(&Val::Int(10)));
        assert_eq!(inner.get(&ustr("b")), Some(&Val::String("bee".to_string())));
    }

    #[test]
    fn test_map_missing_key() {
        let map = FxIndexMap::with_hasher(FxBuildHasher::default());
        let v = Val::Map(map);

        let Val::Map(inner) = &v else {
            panic!("not a map")
        };
        assert_eq!(inner.get(&ustr("nonexistent")), None);
    }

    #[test]
    fn test_map_overwrite_key() {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("key"), Val::Int(1));
        map.insert(ustr("key"), Val::Int(2));
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&ustr("key")), Some(&Val::Int(2)));
    }

    // Nested structures

    #[test]
    fn test_list_of_maps() {
        let mut m1 = FxIndexMap::with_hasher(FxBuildHasher::default());
        m1.insert(ustr("x"), Val::Int(1));
        let mut m2 = FxIndexMap::with_hasher(FxBuildHasher::default());
        m2.insert(ustr("y"), Val::Int(2));
        let list = Val::List(vec![Val::Map(m1), Val::Map(m2)]);

        let Val::List(items) = &list else {
            panic!("not a list")
        };
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_map_containing_lists() {
        let mut map = FxIndexMap::with_hasher(FxBuildHasher::default());
        map.insert(ustr("nums"), Val::List(vec![Val::Int(1), Val::Int(2)]));
        map.insert(ustr("words"), Val::List(vec![Val::String("a".to_string())]));
        let v = Val::Map(map);

        let Val::Map(inner) = &v else {
            panic!("not a map")
        };
        let Val::List(nums) = inner.get(&ustr("nums")).unwrap() else {
            panic!("not a list");
        };
        assert_eq!(nums.len(), 2);
        let Val::List(words) = inner.get(&ustr("words")).unwrap() else {
            panic!("not a list");
        };
        assert_eq!(words.len(), 1);
    }

    #[test]
    fn test_nested_list_equality() {
        let a = Val::List(vec![Val::List(vec![Val::Int(1), Val::Int(2)])]);
        let b = Val::List(vec![Val::List(vec![Val::Int(1), Val::Int(2)])]);
        assert_eq!(a, b);
    }

    #[test]
    fn test_nested_map_equality() {
        let mut inner = FxIndexMap::with_hasher(FxBuildHasher::default());
        inner.insert(ustr("deep"), Val::Int(99));
        let mut outer_a = FxIndexMap::with_hasher(FxBuildHasher::default());
        outer_a.insert(ustr("inner"), Val::Map(inner.clone()));
        let mut outer_b = FxIndexMap::with_hasher(FxBuildHasher::default());
        outer_b.insert(ustr("inner"), Val::Map(inner));
        assert_eq!(Val::Map(outer_a), Val::Map(outer_b));
    }

    // Blob

    #[test]
    fn test_blob_from_vec() {
        let data = vec![0u8, 1, 2, 255, 128];
        let blob = Val::Blob(data.clone());
        assert_eq!(blob, Val::Blob(data));
    }

    #[test]
    fn test_blob_empty() {
        let blob = Val::Blob(vec![]);
        assert_eq!(blob, Val::Blob(vec![]));
    }

    #[test]
    fn test_blob_ne_int() {
        assert_ne!(Val::Blob(vec![0]), Val::Int(0));
    }

    #[test]
    fn test_blob_ne_string() {
        assert_ne!(
            Val::Blob(b"hello".to_vec()),
            Val::String("hello".to_string())
        );
    }

    // DateTime

    #[test]
    fn test_datetime_creation() {
        use chrono::{Datelike, TimeZone};
        let dt = chrono::Utc
            .with_ymd_and_hms(2024, 12, 25, 10, 30, 0)
            .unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 12);
        assert_eq!(dt.day(), 25);
    }

    #[test]
    fn test_datetime_ordering_different_vals() {
        use chrono::TimeZone;
        let earlier = chrono::Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let later = chrono::Utc.with_ymd_and_hms(2021, 1, 1, 0, 0, 0).unwrap();
        let v_early = Val::DateTime(earlier);
        let v_late = Val::DateTime(later);
        assert_ne!(v_early, v_late);
    }

    #[test]
    fn test_datetime_same_instant() {
        use chrono::TimeZone;
        let dt1 = chrono::Utc.with_ymd_and_hms(2023, 6, 15, 12, 0, 0).unwrap();
        let dt2 = chrono::Utc.with_ymd_and_hms(2023, 6, 15, 12, 0, 0).unwrap();
        assert_eq!(Val::DateTime(dt1), Val::DateTime(dt2));
    }

    #[test]
    fn test_datetime_vs_int() {
        use chrono::TimeZone;
        let dt = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_ne!(Val::DateTime(dt), Val::Int(2024));
    }

    #[test]
    fn test_nan_roundtrip() {
        let original = Val::Float(f64::NAN);
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Val = serde_json::from_str(&json).unwrap();

        match deserialized {
            Val::Float(f) => assert!(f.is_nan(), "NaN should round-trip as NaN"),
            _ => panic!("expected Float, got {:?}", deserialized),
        }
    }
}
