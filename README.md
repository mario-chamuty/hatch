# Hatch - Next-Gen Dependency and Build Manager for Flutter

<div align="center">

![Hatch Logo](docs/images/hatch-logo.png)

**The unified toolchain Flutter has been missing**

[![Version](https://img.shields.io/badge/version-0.1.0-blue.svg)](https://github.com/yourusername/hatch)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Flutter](https://img.shields.io/badge/Flutter-3.24.2+-blue.svg)](https://flutter.dev)
[![Rust](https://img.shields.io/badge/Rust-1.70+-orange.svg)](https://www.rust-lang.org)

[Getting Started](#-getting-started) • [Documentation](docs/) • [Examples](examples/) • [Contributing](CONTRIBUTING.md)

</div>

## ⚡ Overview

Hatch is a blazingly fast, Rust-powered dependency and build manager for Flutter that replaces `pub` with a smarter, more efficient solution. It features advanced dependency resolution, integrated Flutter version management via FVM, and comprehensive build automation.

### Why Hatch?

- **🚀 Fast** - Parallel downloads, smart caching, and optimized resolution
- **🧠 Smart Resolution** - Handles complex dependency conflicts
- **📦 Direct Package Resolution** - No pubspec.yaml required; Flutter/Dart uses packages directly from Hatch cache
- **🔄 FVM Integration** - Automatic Flutter SDK management built-in
- **🏗️ Build Automation** - (Planned) Version syncing, artifact publishing, and team-wide build distribution
- **🎯 Profiles & Scripts** - Environment-specific dependencies and automation
- **🌐 Multi-Source** - Support for pub.dev, git, and local packages (private registries planned)

## 🎯 Key Features

### Smart Dependency Resolution
- **Ultra Resolver**: Fast parallel metadata fetching with batch tracking
- **Automatic Conflict Resolution**: Handles version conflicts
- **Lockfile Support**: Reproducible builds with `hatch.lock`
- **Batch Download Tracking**: Visual feedback showing dependency resolution levels

### Package Sources
- **pub.dev**: Full compatibility with existing Flutter packages
- **Local Packages**: Path-based dependencies for monorepos (working)
- **Git Dependencies**: (Configuration supported, resolution not implemented)
- **Private Registries**: (Configuration supported, resolution not implemented)

### Flutter Version Management
- **FVM Commands**: Manual Flutter SDK installation via `hatch fvm` commands
- **Per-Project Versions**: Different Flutter versions for different projects
- **SDK Update Management**: Update Flutter/Dart versions in manifest

### Advanced Features
- **Profiles**: Development, test, production, and custom profiles (configuration supported, switching not yet implemented)
- **Scripts**: Pre/post install hooks and custom commands
- **Submodules**: (Planned) Monorepo support with nested hatch.json files
- **Build Versioning**: (Planned) Automatic build number synchronization
- **Artifact Publishing**: (Planned) Upload and distribute builds to teams

## 📥 Installation

### Windows (Chocolatey)
```bash
choco install hatch
```

### macOS (Homebrew)
```bash
brew install hatch
```

### From Source
```bash
git clone https://github.com/yourusername/hatch.git
cd hatch
cargo build --release
# Add target/release/hatch to your PATH
```

## 🚀 Getting Started

### Initialize a New Project
```bash
hatch init my_app
cd my_app
```

### Migrate Existing Flutter Project
```bash
# In your Flutter project directory
hatch migrate
# This converts pubspec.yaml to hatch.json
```

### Install Dependencies
```bash
hatch install
# Uses smart resolution by default
# Generates .dart_tool/package_config.json for Flutter/Dart
```

### Add Dependencies
```bash
# Add a regular dependency
hatch add http ^1.0.0

# Add a dev dependency
hatch add --dev mockito ^5.0.0

# For git/path dependencies, edit hatch.json directly:
# "my_package": {
#   "version": "any",
#   "git": "https://github.com/user/package.git",
#   "ref": "main"
# }
```

## 📄 Manifest Format (hatch.json)

```json
{
  "name": "my_flutter_app",
  "description": "An awesome Flutter app",
  "version": "1.0.0",

  "sdk": {
    "flutter": "3.24.2",
    "dart": ">=3.5.0 <4.0.0"
  },

  "require": {
    "http": "^1.0.0",
    "provider": "^6.0.0",
    "local_package": {
      "version": "any",
      "path": "../packages/local_package"
    },
    "git_package": {
      "version": "^1.0.0",
      "git": "https://github.com/example/package.git",
      "ref": "main"
    },
    "private_package": {
      "version": "^2.0.0",
      "nest": "company-registry"
    }
  },

  "require-dev": {
    "test": "^1.24.0",
    "mockito": "^5.4.0"
  },

  "profiles": {
    "production": {
      "require": {
        "sentry_flutter": "^7.0.0"
      }
    },
    "test": {
      "require-dev": {
        "flutter_test": {"sdk": "flutter"}
      },
      "scripts": {
        "pre-install": "echo 'Setting up test environment'"
      }
    }
  },

  "nests": [
    {
      "name": "company-registry",
      "url": "https://packages.company.com",
      "auth": true
    }
  ],

  "scripts": {
    "pre-install": "echo 'Installing dependencies...'",
    "post-install": "echo 'Installation complete!'",
    "test": "flutter test",
    "build": "flutter build apk"
  }
}
```

## 🛠️ CLI Commands

### Core Commands

| Command | Description |
|---------|-------------|
| `hatch init [name]` | Initialize a new Hatch project |
| `hatch install` | Install dependencies (smart resolution) |
| `hatch update [packages...]` | Update specific packages or all |
| `hatch add <package> [version]` | Add a dependency |
| `hatch remove <package>` | Remove a dependency |
| `hatch why <package>` | Explain why a package is installed |
| `hatch migrate` | Convert pubspec.yaml to hatch.json |

### Advanced Commands

| Command | Description |
|---------|-------------|
| `hatch profile [name]` | (Not yet implemented) Switch to or show current profile |
| `hatch run <script>` | Run a script from manifest |
| `hatch fvm use <version>` | Set Flutter version for project |
| `hatch sdk-update` | Update Flutter/Dart SDK versions |
| `hatch cache clear` | Clear package cache |
| `hatch cache stats` | Show cache statistics |
| `hatch cache remove <pkg>` | Remove specific package from cache |

### Options

- `-v, -vv, -vvv, -vvvv` - Increase verbosity levels
- `-q, --quiet` - Suppress output
- `--profile <name>` - Use specific profile
- `--project-dir <path>` - Specify project directory

## 🏗️ Resolution Strategy

Hatch uses the Ultra Resolver for dependency resolution.

### Ultra Resolver
- Parallel metadata fetching for speed
- Batch-based resolution tracking dependency levels
- Caching for improved performance
- Progressive resolution with conflict detection
- Handles complex dependency trees

## 📦 Package Sources

### pub.dev Packages
```json
"require": {
  "http": "^1.0.0"
}
```

### Local Packages
```json
"require": {
  "my_package": {
    "version": "any",
    "path": "../packages/my_package"
  }
}
```

### Git Dependencies (Not Implemented)
Configuration is supported in manifest but resolution is not implemented:
```json
"require": {
  "my_package": {
    "version": "^1.0.0",
    "git": "https://github.com/user/repo.git",
    "ref": "main"  // branch, tag, or commit
  }
}
```

### Private Registries (Not Implemented)
Configuration is supported and validated but resolution is not implemented:
```json
"nests": [
  {
    "name": "company",
    "url": "https://packages.company.com",
    "auth": true
  }
],
"require": {
  "private_package": {
    "version": "^1.0.0",
    "nest": "company"
  }
}
```

## 🎨 Profiles

Profiles allow environment-specific dependencies and scripts:

```json
"profiles": {
  "development": {
    "require-dev": {
      "flutter_launcher_icons": "^0.13.0"
    }
  },
  "production": {
    "require": {
      "firebase_crashlytics": "^3.0.0"
    },
    "scripts": {
      "pre-install": "flutter clean"
    }
  },
  "test": {
    "require-dev": {
      "mockito": "^5.0.0",
      "flutter_test": {"sdk": "flutter"}
    }
  }
}
```

Usage:
```bash
# Install with specific profile
hatch install --profile production

# Profile switching is not yet implemented
# To use a profile, specify it during install
```

## 🔒 Lockfile (hatch.lock)

The lockfile ensures reproducible builds across teams:

```yaml
lockfile_version: 1
packages:
  http:
    version: 1.1.0
    resolved: pub.dev/http@1.1.0
    integrity: sha256:abcd1234...
    dependencies:
      http_parser: ^4.0.0
    dev: false
    registry: pub.dev
flutter_version: 3.24.2
dart_version: ">=3.5.0 <4.0.0"
generated_at: 2025-09-26T12:00:00Z
hatch_version: 0.1.0
```

## 🗄️ Cache Management

Hatch maintains a global package cache for efficiency:

### Cache Structure
```
~/.hatch/cache/
├── packages/
│   └── pub.dev/
│       ├── http/
│       │   └── 1.1.0/
│       └── provider/
│           └── 6.0.5/
├── downloads/
└── metadata/
```

### Cache Commands
```bash
# Show cache statistics
hatch cache stats

# Clear entire cache
hatch cache clear --force

# Remove specific package
hatch cache remove http
hatch cache remove http 1.1.0
```

### Environment Variables
- `HATCH_CACHE_DIR` - Custom cache directory (useful for CI/CD)
- `HATCH_API_KEY` - API key for backend services

## 🔄 Flutter Version Management

Hatch integrates with FVM for Flutter SDK management:

```bash
# List available Flutter versions
hatch fvm list

# Use specific Flutter version
hatch fvm use 3.24.2

# Install Flutter version
hatch fvm install 3.24.2

# Sync FVM configuration
hatch fvm sync
```

## 📊 Dependency Validation

Hatch validates dependencies to prevent common issues:

- **Conflicting sources**: Can't specify both `git` and `path` for same dependency
- **Invalid references**: `ref` requires `git` source
- **Undefined registries**: Referenced nests must be defined
- **Version constraints**: Supports `^`, `~`, `>=`, `any`, `*`, and ranges

## 🏃 Scripts

Automate common tasks with scripts:

```json
"scripts": {
  "pre-install": "flutter clean",
  "post-install": "dart run build_runner build",
  "test": "flutter test",
  "build:apk": "flutter build apk --release",
  "analyze": "flutter analyze",
  "format": "dart format ."
}
```

Run scripts:
```bash
hatch run test
hatch run build:apk
```

## 🚀 Performance

Hatch is designed for speed:

- **Parallel Downloads**: Download multiple packages simultaneously
- **Smart Caching**: Metadata and package caching with integrity checks
- **Optimized Resolution**: Multiple strategies for different scenarios
- **Direct Package Loading**: No intermediate pubspec.yaml generation
- **Batch Processing**: Visual feedback on dependency resolution levels

## 📖 Documentation

- [Manifest Format](docs/manifest.md) - Complete hatch.json reference
- [Dependency Sources](docs/dependencies.md) - Package source types and configuration
- [Profiles & Scripts](docs/profiles.md) - Environment management
- [Cache Management](docs/cache.md) - Cache structure and optimization
- [Resolution Strategies](docs/resolution.md) - Resolver algorithms explained
- [Private Registries](docs/registries.md) - Setting up Hatch Nests
- [Migration Guide](docs/migration.md) - Moving from pub to Hatch
- [CI/CD Integration](docs/ci-cd.md) - Using Hatch in pipelines

## 🤝 Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## 📝 License

MIT License - see [LICENSE](LICENSE) for details.

## 🙏 Acknowledgments

- Flutter team for the amazing framework
- Rust community for excellent crates
- Contributors and early adopters

---

<div align="center">
Built with ❤️ in Rust for the Flutter community
</div>