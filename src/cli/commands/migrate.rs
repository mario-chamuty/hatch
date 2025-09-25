use anyhow::{Context, Result};
use std::path::Path;
use std::collections::HashMap;
use serde_yaml::Value;
use colored::Colorize;

use crate::manifest::schema::{HatchManifest, SdkConstraints};

pub async fn execute(pubspec_path: Option<String>, output_path: Option<String>) -> Result<()> {
    println!("🔄 {} from pubspec.yaml to hatch.json", "Migrating".cyan().bold());

    let pubspec_path = pubspec_path.unwrap_or_else(|| "pubspec.yaml".to_string());
    let output_path = output_path.unwrap_or_else(|| "hatch.json".to_string());

    if !Path::new(&pubspec_path).exists() {
        return Err(anyhow::anyhow!("pubspec.yaml not found at: {}", pubspec_path));
    }

    println!("📖 Reading {}...", pubspec_path);
    let pubspec_content = std::fs::read_to_string(&pubspec_path)
        .context("Failed to read pubspec.yaml")?;

    let pubspec: serde_yaml::Value = serde_yaml::from_str(&pubspec_content)
        .context("Failed to parse pubspec.yaml")?;

    let hatch_manifest = convert_pubspec_to_hatch(pubspec)?;

    let hatch_json = serde_json::to_string_pretty(&hatch_manifest)?;

    if Path::new(&output_path).exists() {
        println!("⚠️  {} already exists", output_path.yellow());
        print!("Overwrite? (y/N): ");
        use std::io::{self, Write};
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Migration cancelled");
            return Ok(());
        }
    }

    std::fs::write(&output_path, hatch_json)?;
    println!("✅ {} created successfully!", output_path.green());

    println!("\n📋 Migration Summary:");
    println!("   Project: {}", hatch_manifest.name.green());
    if let Some(desc) = &hatch_manifest.description {
        println!("   Description: {}", desc);
    }

    let deps_count = hatch_manifest.require.as_ref().map_or(0, |d| d.len());
    let dev_deps_count = hatch_manifest.require_dev.as_ref().map_or(0, |d| d.len());
    let local_count = hatch_manifest.local_packages.as_ref().map_or(0, |d| d.len());

    println!("   Dependencies: {}", deps_count.to_string().cyan());
    println!("   Dev Dependencies: {}", dev_deps_count.to_string().cyan());
    if local_count > 0 {
        println!("   Local Packages: {}", local_count.to_string().cyan());
    }

    if let Some(flutter) = &hatch_manifest.sdk.flutter {
        println!("   Flutter SDK: {}", flutter.green());
    }
    if let Some(dart) = &hatch_manifest.sdk.dart {
        println!("   Dart SDK: {}", dart.green());
    }

    println!("\n💡 Next steps:");
    println!("   1. Review the generated {}", output_path);
    println!("   2. Run {} to install dependencies", "hatch install".cyan());
    println!("   3. Optionally remove pubspec.yaml after verifying the migration");

    Ok(())
}

fn convert_pubspec_to_hatch(pubspec: Value) -> Result<HatchManifest> {
    // Extract SDK constraints first
    let sdk = if let Some(environment) = pubspec.get("environment") {
        SdkConstraints {
            dart: extract_string(environment, "sdk"),
            flutter: extract_string(environment, "flutter").or_else(|| {
                // Check if using Flutter SDK
                if pubspec.get("dependencies").and_then(|d| d.get("flutter")).is_some() {
                    Some("stable".to_string())
                } else {
                    None
                }
            }),
        }
    } else {
        SdkConstraints {
            dart: Some(">=3.0.0 <4.0.0".to_string()),
            flutter: Some("stable".to_string()),
        }
    };

    let mut manifest = HatchManifest {
        name: extract_string(&pubspec, "name").unwrap_or_else(|| "unnamed_project".to_string()),
        description: extract_string(&pubspec, "description"),
        version: extract_string(&pubspec, "version"),
        sdk,
        require: None,
        require_dev: None,
        scripts: None,
        profiles: None,
        overrides: None,
        local_packages: None,
        repositories: None,
        build: None,
        submodules: None,
    };

    // Extract dependencies
    if let Some(deps) = pubspec.get("dependencies") {
        if let Some(deps_map) = deps.as_mapping() {
            let mut require = HashMap::new();
            let mut local_packages = HashMap::new();

            for (key, value) in deps_map {
                if let Some(key_str) = key.as_str() {
                    // Skip Flutter SDK dependency
                    if key_str == "flutter" {
                        continue;
                    }

                    // Check if it's a path dependency
                    if let Some(mapping) = value.as_mapping() {
                        if let Some(path) = mapping.get("path").and_then(|p| p.as_str()) {
                            local_packages.insert(key_str.to_string(), path.to_string());
                            continue;
                        }
                    }

                    let version = extract_dependency_version(value);
                    require.insert(key_str.to_string(), version);
                }
            }

            if !require.is_empty() {
                manifest.require = Some(require);
            }
            if !local_packages.is_empty() {
                manifest.local_packages = Some(local_packages);
            }
        }
    }

    // Extract dev dependencies
    if let Some(dev_deps) = pubspec.get("dev_dependencies") {
        if let Some(deps_map) = dev_deps.as_mapping() {
            let mut require_dev = HashMap::new();

            for (key, value) in deps_map {
                if let Some(key_str) = key.as_str() {
                    // Skip Flutter test SDK dependency
                    if key_str == "flutter_test" {
                        continue;
                    }

                    let version = extract_dependency_version(value);
                    require_dev.insert(key_str.to_string(), version);
                }
            }

            if !require_dev.is_empty() {
                manifest.require_dev = Some(require_dev);
            }
        }
    }

    // Extract dependency overrides
    if let Some(overrides) = pubspec.get("dependency_overrides") {
        if let Some(overrides_map) = overrides.as_mapping() {
            let mut dep_overrides = HashMap::new();

            for (key, value) in overrides_map {
                if let Some(key_str) = key.as_str() {
                    let version = extract_dependency_version(value);
                    dep_overrides.insert(key_str.to_string(), version);
                }
            }

            if !dep_overrides.is_empty() {
                manifest.overrides = Some(dep_overrides);
            }
        }
    }

    // Add common Flutter scripts
    let mut scripts = HashMap::new();
    scripts.insert("test".to_string(), serde_json::json!("flutter test"));
    scripts.insert("build".to_string(), serde_json::json!("flutter build"));
    scripts.insert("clean".to_string(), serde_json::json!("flutter clean"));
    scripts.insert("analyze".to_string(), serde_json::json!("flutter analyze"));
    scripts.insert("format".to_string(), serde_json::json!("dart format ."));
    manifest.scripts = Some(scripts);

    Ok(manifest)
}

fn extract_string(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(|s| s.to_string())
}

fn extract_dependency_version(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Mapping(map) => {
            // Handle complex dependency specifications
            if let Some(version) = map.get("version") {
                if let Some(v) = version.as_str() {
                    return v.to_string();
                }
            }

            // Handle git dependencies
            if let Some(git) = map.get("git") {
                if let Some(url) = git.get("url").and_then(|u| u.as_str()) {
                    return format!("git:{}", url);
                }
            }

            // Handle path dependencies
            if let Some(path) = map.get("path") {
                if let Some(p) = path.as_str() {
                    return format!("path:{}", p);
                }
            }

            // Handle hosted dependencies
            if let Some(hosted) = map.get("hosted") {
                if let Some(version) = map.get("version").and_then(|v| v.as_str()) {
                    return version.to_string();
                }
            }

            "^1.0.0".to_string()
        }
        _ => "^1.0.0".to_string()
    }
}