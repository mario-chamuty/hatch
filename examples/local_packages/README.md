# Local Packages Example

This example shows how to link local packages for development.

## Features

- Local package development
- Path-based dependencies
- Package linking without publishing

## Structure

```
local_packages/
├── test_local/          # Main app
│   └── hatch.yaml
└── my_package/         # Local package
    ├── pubspec.yaml    # Package manifest
    └── lib/
        └── my_local_package.dart
```

## Running

```bash
cd test_local
hatch install
```

## Manifest

```yaml
# test_local/hatch.yaml
name: test_local_app

require:
  my_local_package: any

local-packages:
  my_local_package: "../my_package"
```

## Use Cases

1. **Monorepo development** - Multiple packages in one repository
2. **Testing changes** - Test package changes before publishing
3. **Private packages** - Use internal packages without a registry
4. **Fork development** - Work on package forks locally