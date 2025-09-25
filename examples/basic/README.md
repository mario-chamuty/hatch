# Basic Hatch Example

This example demonstrates basic Hatch usage with a simple Flutter project.

## Features

- Standard Flutter dependencies
- Simple manifest structure
- Basic dependency resolution

## Running

```bash
cd test_project
hatch install
```

## Manifest

```yaml
name: test_flutter_app
description: Basic Flutter app with Hatch

sdk:
  flutter: "3.35.2"
  dart: ">=3.5.0 <4.0.0"

require:
  cupertino_icons: ^1.0.2
  provider: ^6.0.0
  http: ^1.0.0
```

## What This Demonstrates

1. **Basic dependency management** - Shows how Hatch replaces pubspec.yaml
2. **SDK constraints** - Flutter and Dart version specifications
3. **Transitive dependencies** - Hatch automatically resolves all dependencies