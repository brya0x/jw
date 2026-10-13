//! Making and applying patches: the RFC 6902 subset (`add`, `remove`, `replace`) the
//! contract uses. `diff` is the daemon's side, `apply_patch` every client's.

use serde_json::Value;

use crate::PatchOp;

#[derive(Debug, PartialEq, Eq)]
pub struct PatchError(pub String);

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "cannot apply patch: {}", self.0)
    }
}

impl std::error::Error for PatchError {}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// The edits that turn `old` into `new`. Objects are diffed key by key, arrays of the
/// same length element by element; anything else is replaced whole.
pub fn diff(old: &Value, new: &Value) -> Vec<PatchOp> {
    let mut ops = Vec::new();
    diff_at(old, new, String::new(), &mut ops);
    ops
}

fn diff_at(old: &Value, new: &Value, path: String, ops: &mut Vec<PatchOp>) {
    if old == new {
        return;
    }
    match (old, new) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().filter(|k| !b.contains_key(*k)) {
                ops.push(PatchOp::Remove {
                    path: format!("{path}/{}", escape(key)),
                });
            }
            for (key, value) in b {
                let at = format!("{path}/{}", escape(key));
                match a.get(key) {
                    None => ops.push(PatchOp::Add {
                        path: at,
                        value: value.clone(),
                    }),
                    Some(before) => diff_at(before, value, at, ops),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff_at(x, y, format!("{path}/{i}"), ops);
            }
            // Then drop the tail (last first, so indexes stay valid) or append the new items.
            for i in (b.len()..a.len()).rev() {
                ops.push(PatchOp::Remove {
                    path: format!("{path}/{i}"),
                });
            }
            for item in b.iter().skip(a.len()) {
                ops.push(PatchOp::Add {
                    path: format!("{path}/-"),
                    value: item.clone(),
                });
            }
        }
        _ => ops.push(PatchOp::Replace {
            path,
            value: new.clone(),
        }),
    }
}

/// Apply `ops` in order, then set the document's `rev` to `to`.
pub fn apply_patch(doc: &mut Value, ops: &[PatchOp], to: u64) -> Result<(), PatchError> {
    for op in ops {
        apply_op(doc, op)?;
    }
    match doc.as_object_mut() {
        Some(obj) => {
            obj.insert("rev".into(), to.into());
            Ok(())
        }
        None => Err(PatchError("the document is not an object".into())),
    }
}

fn apply_op(doc: &mut Value, op: &PatchOp) -> Result<(), PatchError> {
    let (PatchOp::Add { path, .. } | PatchOp::Remove { path } | PatchOp::Replace { path, .. }) = op;
    let fail = |why: &str| PatchError(format!("{why} at {path:?}"));
    let Some((parent_path, last)) = path.rsplit_once('/') else {
        return match op {
            PatchOp::Replace { value, .. } if path.is_empty() => {
                *doc = value.clone();
                Ok(())
            }
            _ => Err(fail("bad path")),
        };
    };
    let key = last.replace("~1", "/").replace("~0", "~");
    let parent = doc
        .pointer_mut(parent_path)
        .ok_or_else(|| fail("no parent"))?;
    match (op, parent) {
        (PatchOp::Add { value, .. }, Value::Object(m)) => {
            m.insert(key, value.clone());
        }
        (PatchOp::Add { value, .. }, Value::Array(a)) => {
            if key == "-" {
                a.push(value.clone());
            } else {
                let i: usize = key.parse().map_err(|_| fail("bad index"))?;
                if i > a.len() {
                    return Err(fail("index past the end"));
                }
                a.insert(i, value.clone());
            }
        }
        (PatchOp::Remove { .. }, Value::Object(m)) => {
            m.remove(&key).ok_or_else(|| fail("no such key"))?;
        }
        (PatchOp::Remove { .. }, Value::Array(a)) => {
            let i: usize = key.parse().map_err(|_| fail("bad index"))?;
            if i >= a.len() {
                return Err(fail("no such index"));
            }
            a.remove(i);
        }
        (PatchOp::Replace { value, .. }, Value::Object(m)) => {
            *m.get_mut(&key).ok_or_else(|| fail("no such key"))? = value.clone();
        }
        (PatchOp::Replace { value, .. }, Value::Array(a)) => {
            let i: usize = key.parse().map_err(|_| fail("bad index"))?;
            *a.get_mut(i).ok_or_else(|| fail("no such index"))? = value.clone();
        }
        _ => return Err(fail("parent is not a container")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn round(old: Value, new: Value) {
        let ops = diff(&old, &new);
        let mut doc = old.clone();
        apply_patch(&mut doc, &ops, 0).unwrap();
        doc["rev"] = new.get("rev").cloned().unwrap_or(Value::Null);
        let mut want = new.clone();
        want["rev"] = doc["rev"].clone();
        assert_eq!(doc, want, "ops: {ops:?}");
    }

    #[test]
    fn diff_then_apply_reproduces_the_new_document() {
        round(json!({"a": 1}), json!({"a": 2}));
        round(json!({"a": 1}), json!({"a": 1, "b": [1]}));
        round(json!({"a": 1, "b": 2}), json!({"a": 1}));
        round(json!({"l": [1, 2, 3]}), json!({"l": [1, 9, 3]}));
        round(json!({"l": [1, 2, 3]}), json!({"l": [1, 2]}));
        round(json!({"l": [1, 2, 3]}), json!({"l": [1, 2, 3, 4, 5]}));
        round(json!({"l": [1, 2, 3]}), json!({"l": [2, 3]}));
        round(json!({"l": [{"a": 1}]}), json!({"l": [{"a": 1}, {"a": 2}]}));
        round(json!({"l": [1]}), json!({"l": []}));
        round(
            json!({"o": {"x": {"y": [{"z": 1}]}}}),
            json!({"o": {"x": {"y": [{"z": 2}]}}}),
        );
        round(json!({"a/b": 1, "c~d": 2}), json!({"a/b": 3, "c~d": 4}));
    }

    #[test]
    fn an_unchanged_document_has_no_ops() {
        assert!(diff(&json!({"a": [1, {"b": 2}]}), &json!({"a": [1, {"b": 2}]})).is_empty());
    }

    #[test]
    fn appending_an_item_is_one_small_op() {
        let ops = diff(
            &json!({"l": [{"a": 1}]}),
            &json!({"l": [{"a": 1}, {"a": 2}]}),
        );
        assert_eq!(
            ops,
            [PatchOp::Add {
                path: "/l/-".into(),
                value: json!({"a": 2})
            }]
        );
    }

    #[test]
    fn apply_sets_the_revision() {
        let mut doc = json!({"rev": 1, "n": 1});
        apply_patch(
            &mut doc,
            &[PatchOp::Replace {
                path: "/n".into(),
                value: json!(2),
            }],
            5,
        )
        .unwrap();
        assert_eq!(doc, json!({"rev": 5, "n": 2}));
    }

    #[test]
    fn append_with_dash_and_errors() {
        let mut doc = json!({"a": [1]});
        apply_patch(
            &mut doc,
            &[PatchOp::Add {
                path: "/a/-".into(),
                value: json!(2),
            }],
            1,
        )
        .unwrap();
        assert_eq!(doc["a"], json!([1, 2]));
        let bad = [PatchOp::Remove {
            path: "/nope".into(),
        }];
        assert!(apply_patch(&mut doc, &bad, 2).is_err());
        let bad = [PatchOp::Replace {
            path: "/a/9".into(),
            value: json!(0),
        }];
        assert!(apply_patch(&mut doc, &bad, 2).is_err());
    }
}
