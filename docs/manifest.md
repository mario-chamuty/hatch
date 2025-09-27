# Hatch Manifest Format Reference

The Hatch manifest (`hatch.json`) is the central configuration file for your Flutter project, replacing `pubspec.yaml` with a more powerful and flexible format.

## Table of Contents
- [Basic Structure](#basic-structure)
- [Project Metadata](#project-metadata)
- [SDK Constraints](#sdk-constraints)
- [Dependencies](#dependencies)
- [Profiles](#profiles)
- [Scripts](#scripts)
- [Registries and Nests](#registries-and-nests)
- [Build Configuration](#build-configuration)
- [Submodules](#submodules)
- [Local Overrides](#local-overrides)

## Basic Structure

```json
{
  "name": "project_name",
  "description": "Project description",
  "version": "1.0.0",
  "sdk": { /* SDK constraints */ },
  "require": { /* Dependencies */ },
  "require-dev": { /* Dev dependencies */ },
  "profiles": { /* Environment profiles */ },
  "nests": [ /* Private registries */ ],
  "scripts": { /* Automation scripts */ },
  "build": { /* Build configuration */ },
  "submodules": [ /* Submodule paths */ ]
}
```

## Project Metadata

### Required Fields

#### `name` (string, required)
Project name following Dart package naming conventions:
- Must start with a lowercase letter
- Can contain lowercase letters, numbers, and underscores
- Maximum 50 characters

```json
"name": "my_flutter_app"
```

### Optional Fields

#### `description` (string, optional)
Human-readable description of the project.

```json
"description": "A revolutionary Flutter application"
```

#### `version` (string, optional)
Project version following semantic versioning.

```json
"version": "1.2.3"
```

## SDK Constraints

### `sdk` (object, required)
Specifies Flutter and Dart SDK version requirements.

#### `flutter` (string, optional)
Flutter SDK version. Can be:
- Specific version: `"3.24.2"`
- Channel: `"stable"`, `"beta"`, `"dev"`
- FVM alias: `"latest"`, `"global"`

```json
"sdk": {
  "flutter": "3.24.2"
}
```

#### `dart` (string, optional)
Dart SDK version constraint.

```json
"sdk": {
  "dart": ">=3.5.0 <4.0.0"
}
```

## Dependencies

### `require` (object, optional)
Production dependencies needed by your application.

#### Simple Format
```json
"require": {
  "http": "^1.0.0",
  "provider": "^6.0.0"
}
```

#### Complex Format
Dependencies can specify additional sources:

##### Path Dependencies
```json
"require": {
  "local_package": {
    "version": "any",
    "path": "../packages/local_package"
  }
}
```

##### Git Dependencies
```json
"require": {
  "git_package": {
    "version": "^1.0.0",
    "git": "https://github.com/user/repo.git",
    "ref": "main"  // branch, tag, or commit SHA
  }
}
```

##### Private Registry Dependencies
```json
"require": {
  "private_package": {
    "version": "^2.0.0",
    "nest": "company-registry"  // must be defined in nests
  }
}
```

#### Version Constraints
- `^1.2.3` - Compatible with version (caret)
- `~1.2.3` - Approximately equivalent (tilde)
- `>=1.0.0` - Greater than or equal
- `>1.0.0 <2.0.0` - Range
- `any` or `*` - Any version
- `1.2.3` - Exact version

### `require-dev` (object, optional)
Development dependencies for testing and tooling.

```json
"require-dev": {
  "test": "^1.24.0",
  "mockito": "^5.4.0",
  "flutter_test": {
    "sdk": "flutter"  // Flutter SDK dependency
  }
}
```

## Profiles

### `profiles` (object, optional)
Environment-specific configurations that extend the base manifest.

```json
"profiles": {
  "production": {
    "require": {
      "sentry_flutter": "^7.0.0"
    },
    "require-dev": {},
    "scripts": {
      "pre-install": "flutter clean"
    }
  },
  "test": {
    "require-dev": {
      "mockito": "^5.4.0"
    },
    "scripts": {
      "post-install": "dart run build_runner build"
    }
  },
  "ci": {
    "scripts": {
      "pre-install": "echo 'CI Environment Setup'"
    }
  }
}
```

#### Profile Usage
```bash
# Install with specific profile
hatch install --profile production

# Set default profile
hatch profile production
```

## Scripts

### `scripts` (object, optional)
Automation scripts that can be run at various lifecycle points or manually.

```json
"scripts": {
  "pre-install": "flutter clean",
  "post-install": "dart run build_runner build",
  "test": "flutter test",
  "build": "flutter build apk",
  "analyze": ["flutter analyze", "dart format --set-exit-if-changed ."],
  "custom-script": "echo 'Running custom script'"
}
```

#### Script Types
- **Lifecycle Scripts**: `pre-install`, `post-install`
- **Custom Scripts**: Any other name

#### Script Formats
- **String**: Single command
- **Array**: Multiple commands run in sequence

#### Running Scripts
```bash
hatch run test
hatch run build
hatch run custom-script -- --extra-args
```

## Registries and Nests (Partially Implemented)

### `nests` (array, optional)
Private package registries (Hatch Nests) configuration. The configuration is validated but nest resolution is not yet implemented.

```json
"nests": [
  {
    "name": "company-registry",
    "url": "https://packages.company.com",
    "auth": true  // Requires authentication
  },
  {
    "name": "team-registry",
    "url": "https://packages.team.internal"
  }
]
```

### `repositories` (array, optional)
Additional package sources beyond pub.dev.

```json
"repositories": [
  {
    "type": "hatch",
    "url": "https://hatch.dev"
  },
  {
    "type": "pub",
    "url": "https://pub.dev"
  },
  {
    "type": "path",
    "url": "../local-packages"
  }
]
```

### `disable-pub` (boolean, optional)
Disable pub.dev as a package source.

```json
"disable-pub": false
```

### `prefer-newest-from` (string, optional)
When multiple sources have the same package, prefer the newest from:
- `"nest"` - Prefer private registry versions
- `"pub"` - Prefer pub.dev versions

```json
"prefer-newest-from": "nest"
```

## Build Configuration (Planned)

### `build` (object, optional)
Configuration for build versioning and artifact management. This field is supported in the manifest but functionality is not yet implemented.

```json
"build": {
  "versioning": {
    "url": "https://hatch.example.com/api",
    "apikey": "${HATCH_API_KEY}",
    "project": "my_app"
  }
}
```

When implemented, this will enable automatic build number synchronization across teams.

## Submodules (Planned)

### `submodules` (array, optional)
Paths to submodules with their own `hatch.json` files. This field is supported in the manifest but functionality is not yet implemented.

```json
"submodules": [
  "modules/auth/hatch.json",
  "modules/payments/hatch.json",
  "modules/shared/hatch.json"
]
```

When implemented, submodules will:
- Have their own dependencies
- Be built independently
- Be automatically linked in the parent project

## Local Overrides

### `overrides` (object, optional)
Version overrides for resolving conflicts.

```json
"overrides": {
  "http": "0.13.6",
  "collection": "1.17.0"
}
```

### Local Override File
Create `hatch.local.json` (git-ignored) for local-only overrides:

```json
{
  "require": {
    "my_package": {
      "version": "any",
      "path": "../debug/my_package"
    }
  },
  "scripts": {
    "pre-install": "echo 'Local development setup'"
  }
}
```

## Complete Example

```json
{
  "name": "super_app",
  "description": "A comprehensive Flutter application",
  "version": "2.1.0",

  "sdk": {
    "flutter": "3.24.2",
    "dart": ">=3.5.0 <4.0.0"
  },

  "require": {
    "http": "^1.1.0",
    "provider": "^6.0.5",
    "shared_lib": {
      "version": "^1.0.0",
      "path": "../packages/shared_lib"
    },
    "analytics": {
      "version": "^2.0.0",
      "nest": "company"
    }
  },

  "require-dev": {
    "test": "^1.24.0",
    "mockito": "^5.4.0"
  },

  "profiles": {
    "production": {
      "require": {
        "firebase_crashlytics": "^3.3.0"
      },
      "scripts": {
        "pre-install": "flutter clean"
      }
    }
  },

  "nests": [
    {
      "name": "company",
      "url": "https://packages.company.com",
      "auth": true
    }
  ],

  "scripts": {
    "test": "flutter test",
    "build:android": "flutter build apk --release",
    "build:ios": "flutter build ios --release"
  },

  "build": {
    "versioning": {
      "url": "https://hatch.company.com/api",
      "apikey": "${HATCH_API_KEY}",
      "project": "super_app"
    }
  },

  "submodules": [
    "modules/core/hatch.json",
    "modules/features/hatch.json"
  ]
}
```

## Validation Rules

1. **Project name**: Must be lowercase, start with letter, contain only letters, numbers, underscores
2. **Dependencies**: Cannot specify multiple sources (git + path, git + nest, etc.)
3. **Git references**: `ref` field requires `git` source
4. **Nest references**: Referenced nests must be defined in `nests` array
5. **Version constraints**: Must follow semantic versioning rules
6. **Flutter version**: Must be valid version number or channel name
7. **Dart constraint**: Must be valid version range

## Environment Variables (Not Implemented)

Environment variable expansion is planned but not yet implemented. The syntax will be:
- `${VAR}` - Simple substitution
- `${VAR:-default}` - With default value

## Migration from pubspec.yaml

Use the migration command to convert existing projects:

```bash
hatch migrate
```

This automatically:
- Converts dependencies to hatch.json format
- Preserves version constraints
- Detects FVM configuration
- Maintains Flutter/Dart SDK constraints
- Creates appropriate scripts from existing configuration