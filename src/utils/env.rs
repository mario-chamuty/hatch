use std::env;
use regex::Regex;

/// Expand environment variables in a string
/// Supports ${VAR} and ${VAR:-default} syntax
pub fn expand_env_vars(input: &str) -> String {
    let re = Regex::new(r"\$\{([^}]+)\}").unwrap();

    re.replace_all(input, |caps: &regex::Captures| {
        let var_expr = &caps[1];

        // Check for default value syntax
        if let Some(pos) = var_expr.find(":-") {
            let var_name = &var_expr[..pos];
            let default_value = &var_expr[pos + 2..];

            env::var(var_name).unwrap_or_else(|_| default_value.to_string())
        } else {
            // Simple variable substitution
            env::var(var_expr).unwrap_or_else(|_| format!("${{{}}}", var_expr))
        }
    }).to_string()
}

/// Recursively expand environment variables in a JSON value
pub fn expand_json_env_vars(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = expand_env_vars(s);
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                expand_json_env_vars(v);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                expand_json_env_vars(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_expansion() {
        env::set_var("TEST_VAR", "test_value");
        assert_eq!(expand_env_vars("${TEST_VAR}"), "test_value");
        env::remove_var("TEST_VAR");
    }

    #[test]
    fn test_default_value() {
        assert_eq!(expand_env_vars("${NONEXISTENT:-default}"), "default");
    }

    #[test]
    fn test_no_expansion() {
        assert_eq!(expand_env_vars("no variables here"), "no variables here");
    }

    #[test]
    fn test_mixed_expansion() {
        env::set_var("EXISTS", "value");
        let result = expand_env_vars("Start ${EXISTS} middle ${MISSING:-fallback} end");
        assert_eq!(result, "Start value middle fallback end");
        env::remove_var("EXISTS");
    }
}