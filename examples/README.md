# Hatch Examples

This directory contains various examples demonstrating Hatch's features.

## Examples

### 1. [Basic](./basic/test_project/)
Simple Flutter project with common dependencies like `provider` and `http`.

```yaml
require:
  cupertino_icons: ^1.0.2
  provider: ^6.0.0
  http: ^1.0.0
```

### 2. [Local Packages](./local_packages/test_local/)
Demonstrates linking to local packages for development.

```yaml
local-packages:
  my_local_package: "../my_package"
```

### 3. [Version Aliases](./version_aliases/test_alias/)
Shows Composer-style version aliasing and overrides.

```yaml
overrides:
  # Use older version but pretend it's newer
  path: "1.8.3 as 1.9.5"

  # Use latest but pretend it's older
  collection: "dev-main as 1.15.0"

  # Use local fork
  http: "path:../http_fork as 1.5.0"
```

### 4. [Complex Dependencies](./complex_deps/)
Example with transitive dependencies and conflict resolution.

## Running Examples

Navigate to any example directory and run:

```bash
hatch install
```

To use a specific profile:

```bash
hatch install --profile dev
```

To see dependency resolution:

```bash
hatch why <package_name>
```