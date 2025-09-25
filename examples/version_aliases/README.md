# Version Aliases Example (Composer-Style)

This example demonstrates Hatch's powerful version aliasing feature, inspired by PHP's Composer.

## Features

- Version aliasing (use X as Y)
- Path overrides with version masking
- Conflict resolution through aliasing

## Manifest

```yaml
name: test_alias_app

require:
  path: ^1.9.0
  http: ^1.5.0
  collection: ^1.15.0

overrides:
  # Use older version but pretend it's newer
  path: "1.8.3 as 1.9.5"

  # Use latest/dev version but pretend it's stable
  collection: "dev-main as 1.15.0"

  # Use local fork but pretend it's official
  http: "path:../http_fork as 1.5.0"
```

## Use Cases

### 1. Working Around Version Conflicts
```yaml
# Package A requires path ^1.9.0
# Package B requires path ^1.8.0
# But 1.8.3 works with both!
overrides:
  path: "1.8.3 as 1.9.0"
```

### 2. Testing Pre-release Versions
```yaml
# Test latest dev version while keeping constraints happy
overrides:
  some_package: "dev-main as 2.0.0"
```

### 3. Local Fork Development
```yaml
# Replace package with local fork transparently
overrides:
  broken_package: "path:../my-fix as 1.2.3"
```

## How It Works

1. **Download**: Hatch downloads the "actual" version (1.8.3)
2. **Constraint Checking**: Uses the "pretend" version (1.9.5) for dependency resolution
3. **Transparency**: Shows both versions in output: `path @ 1.9.5 (actually 1.8.3)`

## Running

```bash
cd test_alias
hatch install
```

You'll see output like:
```
📥 Downloading packages...
   path @ 1.9.5 (actually 1.8.3) ... ✓
   collection @ 1.15.0 (actually 1.19.1) ... ✓
   http @ ../http_fork [local] ... ✓
```