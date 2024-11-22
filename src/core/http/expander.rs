use serde_json::Value;

use crate::core::blueprint::DynamicValue;

pub struct Expander;

impl Expander {
    // Takes ownership of the request body and returns the expanded Value.
    pub fn expand(
        dynamic_value: &DynamicValue<serde_json::Value>,
        batch_size: usize,
    ) -> anyhow::Result<DynamicValue<serde_json::Value>> {
        let str_value = dynamic_value.to_string()?;
        let value: serde_json::Value = serde_json::from_str(&str_value)?;
        Ok(DynamicValue::Value(Self::expand_inner(value, batch_size)))
    }

    fn expand_inner(value: Value, batch_size: usize) -> Value {
        match value {
            Value::Object(map) => {
                let expanded_map = map
                    .into_iter()
                    .map(|(k, v)| (k, Self::expand_inner(v, batch_size)))
                    .collect();
                Value::Object(expanded_map)
            }
            Value::Array(list) => {
                let expanded_list: Vec<Value> = list
                    .into_iter()
                    .map(|v| Self::expand_inner(v, batch_size))
                    .collect();

                let mut final_ans = Vec::with_capacity(expanded_list.len());

                for index in 0..batch_size {
                    let expanded_batch: Vec<Value> = expanded_list
                        .iter()
                        .cloned()
                        .map(|v| Self::update_mustache_expr(v, index))
                        .collect();
                    final_ans.extend(expanded_batch);
                }
                Value::Array(final_ans)
            }
            other => other, // Return as is for other variants.
        }
    }

    fn update_mustache_expr(value: Value, index: usize) -> Value {
        match value {
            Value::Object(map) => {
                let updated_map = map
                    .into_iter()
                    .map(|(k, v)| (k, Self::update_mustache_expr(v, index)))
                    .collect();
                Value::Object(updated_map)
            }
            Value::Array(list) => {
                let updated_list = list
                    .into_iter()
                    .map(|v| Self::update_mustache_expr(v, index))
                    .collect();
                Value::Array(updated_list)
            }
            Value::String(s) => {
                if s.contains("{{.value.") || s.contains("{{value.") {
                    let updated_string = s
                        .replace("{{.value.", &format!("{{{{.value.{}.", index))
                        .replace("{{value.", &format!("{{{{value.{}.", index));
                    Value::String(updated_string)
                } else {
                    Value::String(s)
                }
            }
            other => other, // Return as is for other variants.
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_expander() {
        // Test Option 1
        let input1 = json!({
            "a": { "b": { "c": { "d": ["{{.value.userId}}"] } } }
        });

        let actual = Expander::expand_inner(input1, 2);
        let expected = json!({
            "a": { "b": { "c": { "d": ["{{.value.0.userId}}", "{{.value.1.userId}}"] } } }
        });
        assert_eq!(actual, expected);

        // Test Option 2
        let input2 = json!([
            { 
                "userId": "{{.value.id}}", 
                "title": "{{.value.name}}",
                "content": "Hello World" 
            }
        ]);
        let actual = Expander::expand_inner(input2, 2);
        let expected = json!([
            { 
                "userId": "{{.value.0.id}}", 
                "title": "{{.value.0.name}}",
                "content": "Hello World" 
            },
            { 
                "userId": "{{.value.1.id}}", 
                "title": "{{.value.1.name}}",
                "content": "Hello World" 
            }
        ]);
        assert_eq!(actual, expected);

        // Test Option 3
        let input3 = json!([
            { 
                "metadata": "xyz", 
                "items": "{{.value.userId}}" 
            }
        ]);
        let actual = Expander::expand_inner(input3, 2);
        let expected = json!([
            { 
                "metadata": "xyz", 
                "items": "{{.value.0.userId}}" 
            },
            { 
                "metadata": "xyz", 
                "items": "{{.value.1.userId}}" 
            }
        ]);
        assert_eq!(actual, expected);

        // Test Option 4
        let input4 = json!({ 
            "metadata": "xyz", 
            "items": [
                { 
                    "key": "id", 
                    "value": "{{.value.userId}}" 
                }
            ] 
        });
        let actual = Expander::expand_inner(input4, 2);
        let expected = json!({
            "metadata": "xyz",
            "items": [
                { 
                    "key": "id", 
                    "value": "{{.value.0.userId}}" 
                },
                { 
                    "key": "id", 
                    "value": "{{.value.1.userId}}" 
                }
            ]
        });
        assert_eq!(actual, expected);
    }
}
