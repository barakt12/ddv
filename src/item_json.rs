use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use rust_decimal::Decimal;
use serde_json::Value;

use crate::{
    data::{Attribute, Item, KeySchemaType},
    util::from_base64_str,
};

/// Parse the DynamoDB-typed JSON representation (as rendered by `RawJsonItem`,
/// e.g. `{"age":{"N":"30"},"name":{"S":"x"}}`) back into an `Item`.
///
/// This is lossless: it preserves N vs S, sets (SS/NS/BS) and binary (B).
pub fn item_from_typed_json(s: &str) -> Result<Item, String> {
    let value: Value = serde_json::from_str(s).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "top level must be a JSON object".to_string())?;
    let mut attributes = std::collections::HashMap::new();
    for (k, v) in obj {
        attributes.insert(k.clone(), typed_attribute(v)?);
    }
    Ok(Item { attributes })
}

fn typed_attribute(v: &Value) -> Result<Attribute, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| "each attribute must be a `{\"<type>\": <value>}` object".to_string())?;
    if obj.len() != 1 {
        return Err("each attribute object must have exactly one type key".to_string());
    }
    let (t, val) = obj.iter().next().unwrap();
    match t.as_str() {
        "S" => Ok(Attribute::S(as_str(val, "S")?.to_string())),
        "N" => Ok(Attribute::N(as_decimal(val, "N")?)),
        "B" => Ok(Attribute::B(as_blob(val, "B")?)),
        "BOOL" => Ok(Attribute::BOOL(
            val.as_bool().ok_or("BOOL must be a boolean")?,
        )),
        "NULL" => Ok(Attribute::NULL),
        "L" => {
            let arr = val.as_array().ok_or("L must be an array")?;
            let vs = arr.iter().map(typed_attribute).collect::<Result<_, _>>()?;
            Ok(Attribute::L(vs))
        }
        "M" => {
            let m = val.as_object().ok_or("M must be an object")?;
            let mut map = BTreeMap::new();
            for (k, v) in m {
                map.insert(k.clone(), typed_attribute(v)?);
            }
            Ok(Attribute::M(map))
        }
        "SS" => {
            let arr = val.as_array().ok_or("SS must be an array")?;
            let mut set = BTreeSet::new();
            for e in arr {
                set.insert(as_str(e, "SS element")?.to_string());
            }
            Ok(Attribute::SS(set))
        }
        "NS" => {
            let arr = val.as_array().ok_or("NS must be an array")?;
            let mut set = BTreeSet::new();
            for e in arr {
                set.insert(as_decimal(e, "NS element")?);
            }
            Ok(Attribute::NS(set))
        }
        "BS" => {
            let arr = val.as_array().ok_or("BS must be an array")?;
            let mut set = BTreeSet::new();
            for e in arr {
                set.insert(as_blob(e, "BS element")?);
            }
            Ok(Attribute::BS(set))
        }
        other => Err(format!("unknown attribute type: {other}")),
    }
}

/// Parse the plain JSON representation (as rendered by `PlainJsonItem`,
/// e.g. `{"age":30,"name":"x"}`) into an `Item`, inferring DynamoDB types.
///
/// Cannot express sets or binary (see [`is_plain_convertible`]); numbers become
/// `N`, arrays `L`, objects `M`, `null` `NULL`.
pub fn item_from_plain_json(s: &str) -> Result<Item, String> {
    let value: Value = serde_json::from_str(s).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "top level must be a JSON object".to_string())?;
    let mut attributes = std::collections::HashMap::new();
    for (k, v) in obj {
        attributes.insert(k.clone(), plain_attribute(v)?);
    }
    Ok(Item { attributes })
}

fn plain_attribute(v: &Value) -> Result<Attribute, String> {
    match v {
        Value::Null => Ok(Attribute::NULL),
        Value::Bool(b) => Ok(Attribute::BOOL(*b)),
        Value::String(s) => Ok(Attribute::S(s.clone())),
        Value::Number(n) => {
            Decimal::from_str(&n.to_string())
                .map(Attribute::N)
                .map_err(|e| format!("invalid number {n}: {e}"))
        }
        Value::Array(arr) => {
            let vs = arr.iter().map(plain_attribute).collect::<Result<_, _>>()?;
            Ok(Attribute::L(vs))
        }
        Value::Object(m) => {
            let mut map = BTreeMap::new();
            for (k, v) in m {
                map.insert(k.clone(), plain_attribute(v)?);
            }
            Ok(Attribute::M(map))
        }
    }
}

/// Whether an item can be represented in plain JSON without loss. Plain JSON has
/// no way to express string/number/binary sets or binary scalars.
pub fn is_plain_convertible(item: &Item) -> bool {
    item.attributes.values().all(attribute_plain_convertible)
}

fn attribute_plain_convertible(attr: &Attribute) -> bool {
    match attr {
        Attribute::SS(_) | Attribute::NS(_) | Attribute::BS(_) | Attribute::B(_) => false,
        Attribute::L(l) => l.iter().all(attribute_plain_convertible),
        Attribute::M(m) => m.values().all(attribute_plain_convertible),
        _ => true,
    }
}

/// Build an empty-value item skeleton from a key schema, e.g.
/// `{"PK": {"S": ""}, "SK": {"S": ""}}` (typed) so a new item starts from the keys.
pub fn new_item_skeleton(schema: &KeySchemaType) -> Item {
    let mut attributes = std::collections::HashMap::new();
    match schema {
        KeySchemaType::Hash(pk) => {
            attributes.insert(pk.clone(), Attribute::S(String::new()));
        }
        KeySchemaType::HashRange(pk, sk) => {
            attributes.insert(pk.clone(), Attribute::S(String::new()));
            attributes.insert(sk.clone(), Attribute::S(String::new()));
        }
    }
    Item { attributes }
}

/// Confirm the item carries the table's key attributes (required before a put).
pub fn validate_has_keys(item: &Item, schema: &KeySchemaType) -> Result<(), String> {
    let missing: Vec<&str> = match schema {
        KeySchemaType::Hash(pk) => [pk.as_str()]
            .into_iter()
            .filter(|k| !item.attributes.contains_key(*k))
            .collect(),
        KeySchemaType::HashRange(pk, sk) => [pk.as_str(), sk.as_str()]
            .into_iter()
            .filter(|k| !item.attributes.contains_key(*k))
            .collect(),
    };
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("missing key attribute(s): {}", missing.join(", ")))
    }
}

fn as_str<'a>(v: &'a Value, ctx: &str) -> Result<&'a str, String> {
    v.as_str().ok_or_else(|| format!("{ctx} must be a string"))
}

fn as_decimal(v: &Value, ctx: &str) -> Result<Decimal, String> {
    // DynamoDB-typed N is a string; also accept a bare JSON number for convenience.
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(format!("{ctx} must be a numeric string")),
    };
    Decimal::from_str(&s).map_err(|e| format!("invalid {ctx} number {s}: {e}"))
}

fn as_blob(v: &Value, ctx: &str) -> Result<Vec<u8>, String> {
    let s = as_str(v, ctx)?;
    from_base64_str(s).map_err(|e| format!("{ctx} must be base64: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::RawJsonItem;

    #[test]
    fn typed_json_round_trips_losslessly() {
        // typed input covering every DynamoDB type
        let input = r#"{
  "pk": {"S": "u#1"},
  "n": {"N": "30"},
  "flag": {"BOOL": true},
  "nothing": {"NULL": true},
  "tags": {"SS": ["a", "b"]},
  "nums": {"NS": ["1", "2.5"]},
  "bin": {"B": "YWJj"},
  "nested": {"M": {"inner": {"L": [{"S": "x"}, {"N": "1"}]}}}
}"#;
        let item = item_from_typed_json(input).unwrap();
        let schema = KeySchemaType::Hash("pk".into());
        // re-serialize with the typed serializer and parse again — must be identical
        let reser = serde_json::to_string(&RawJsonItem::new(&item, &schema)).unwrap();
        let item2 = item_from_typed_json(&reser).unwrap();
        assert_eq!(item.attributes, item2.attributes);
        assert_eq!(item.attributes.len(), 8);
        assert_eq!(item.attributes.get("n"), Some(&Attribute::N(30.into())));
        assert_eq!(item.attributes.get("bin"), Some(&Attribute::B(b"abc".to_vec())));
    }

    #[test]
    fn plain_json_infers_types() {
        let item = item_from_plain_json(r#"{"pk":"u#1","n":30,"flag":true,"list":[1,"x"],"m":{"k":2}}"#)
            .unwrap();
        assert_eq!(item.attributes.get("pk"), Some(&Attribute::S("u#1".into())));
        assert_eq!(item.attributes.get("n"), Some(&Attribute::N(30.into())));
        assert_eq!(item.attributes.get("flag"), Some(&Attribute::BOOL(true)));
        match item.attributes.get("list").unwrap() {
            Attribute::L(v) => assert_eq!(v.len(), 2),
            other => panic!("expected L, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        assert!(item_from_typed_json("{not json").is_err());
        assert!(item_from_plain_json("[]").is_err()); // top level must be object
        assert!(item_from_typed_json(r#"{"a": {"S": "x", "N": "1"}}"#).is_err()); // two type keys
        assert!(item_from_typed_json(r#"{"a": {"WAT": "x"}}"#).is_err()); // unknown type
    }

    #[test]
    fn plain_convertible_detects_sets_and_binary() {
        let plain = item_from_plain_json(r#"{"a":"x","l":[1,2]}"#).unwrap();
        assert!(is_plain_convertible(&plain));

        let with_set = item_from_typed_json(r#"{"a":{"SS":["x"]}}"#).unwrap();
        assert!(!is_plain_convertible(&with_set));

        let with_bin = item_from_typed_json(r#"{"a":{"B":"YWJj"}}"#).unwrap();
        assert!(!is_plain_convertible(&with_bin));
    }

    #[test]
    fn validate_keys_and_skeleton() {
        let schema = KeySchemaType::HashRange("PK".into(), "SK".into());
        let skel = new_item_skeleton(&schema);
        assert!(validate_has_keys(&skel, &schema).is_ok());

        let missing = item_from_plain_json(r#"{"PK":"x"}"#).unwrap();
        assert!(validate_has_keys(&missing, &schema).is_err());
    }
}
