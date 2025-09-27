# Dependency Sources and Configuration

Hatch supports multiple dependency sources, allowing you to pull packages from pub.dev, git repositories, local paths, and private registries. This guide covers all dependency source types and their configuration.

## Table of Contents
- [Dependency Format](#dependency-format)
- [pub.dev Packages](#pubdev-packages)
- [Local Path Dependencies](#local-path-dependencies)
- [Git Dependencies](#git-dependencies)
- [Private Registries (Hatch Nests)](#private-registries-hatch-nests)
- [Flutter SDK Dependencies](#flutter-sdk-dependencies)
- [Version Constraints](#version-constraints)
- [Dependency Validation](#dependency-validation)
- [Troubleshooting](#troubleshooting)

## Dependency Format

Hatch supports two formats for declaring dependencies:

### Simple Format (String)
For packages from pub.dev with version constraints only:
```json
"require": {
  "http": "^1.0.0",
  "provider": "^6.0.0"
}
```

### Complex Format (Object)
For packages requiring additional configuration:
```json
"require": {
  "my_package": {
    "version": "^1.0.0",
    "path": "../packages/my_package",    // Local path
    "git": "https://github.com/...",      // Git repository
    "ref": "main",                        // Git reference
    "nest": "company-registry"            // Private registry
  }
}
```

**Important**: Only ONE source type (path, git, or nest) can be specified per dependency.

## pub.dev Packages

The default and simplest way to include dependencies from the official Dart package repository.

### Basic Usage
```json
"require": {
  "http": "^1.1.0",
  "dio": "^5.0.0",
  "flutter_bloc": "^8.1.0"
}
```

### With Specific Version
```json
"require": {
  "provider": "6.0.5",  // Exact version
  "http": "^1.0.0",     // Compatible versions
  "dio": ">=5.0.0"      // Minimum version
}
```

### Dev Dependencies
```json
"require-dev": {
  "test": "^1.24.0",
  "mockito": "^5.4.0",
  "build_runner": "^2.4.0"
}
```

## Local Path Dependencies

Perfect for monorepos, local package development, and sharing code between projects.

### Relative Path
```json
"require": {
  "shared_utils": {
    "version": "any",
    "path": "../packages/shared_utils"
  }
}
```

### Absolute Path
```json
"require": {
  "local_package": {
    "version": "any",
    "path": "/Users/developer/packages/local_package"
  }
}
```

### Windows Path
```json
"require": {
  "local_package": {
    "version": "any",
    "path": "C:\\projects\\packages\\local_package"
  }
}
```

### Monorepo Structure Example
```
my_project/
├── apps/
│   ├── mobile/
│   │   └── hatch.json
│   └── web/
│       └── hatch.json
├── packages/
│   ├── core/
│   │   ├── lib/
│   │   └── hatch.json
│   ├── ui/
│   │   ├── lib/
│   │   └── hatch.json
│   └── utils/
│       ├── lib/
│       └── hatch.json
```

Apps reference packages:
```json
// apps/mobile/hatch.json
{
  "require": {
    "core": {
      "version": "any",
      "path": "../../packages/core"
    },
    "ui": {
      "version": "any",
      "path": "../../packages/ui"
    }
  }
}
```

## Git Dependencies (Not Yet Implemented)

Git dependency configuration is supported in the manifest format but resolution is not yet implemented.

### Basic Git Dependency
```json
"require": {
  "my_package": {
    "version": "any",
    "git": "https://github.com/username/my_package.git"
  }
}
```

### With Branch
```json
"require": {
  "my_package": {
    "version": "any",
    "git": "https://github.com/username/my_package.git",
    "ref": "develop"
  }
}
```

### With Tag
```json
"require": {
  "my_package": {
    "version": "any",
    "git": "https://github.com/username/my_package.git",
    "ref": "v1.2.3"
  }
}
```

### With Commit SHA
```json
"require": {
  "my_package": {
    "version": "any",
    "git": "https://github.com/username/my_package.git",
    "ref": "a1b2c3d4e5f6"
  }
}
```

### SSH Git URLs
```json
"require": {
  "private_package": {
    "version": "any",
    "git": "git@github.com:company/private_package.git",
    "ref": "main"
  }
}
```

### GitLab/Bitbucket
```json
"require": {
  "gitlab_package": {
    "version": "any",
    "git": "https://gitlab.com/username/package.git"
  },
  "bitbucket_package": {
    "version": "any",
    "git": "https://bitbucket.org/username/package.git"
  }
}
```

## Private Registries (Hatch Nests) - Not Yet Implemented

Hatch Nests configuration is supported and validated but resolution from private registries is not yet implemented.

### Configure Nests
First, define your private registries:
```json
"nests": [
  {
    "name": "company-registry",
    "url": "https://packages.company.com",
    "auth": true
  },
  {
    "name": "team-registry",
    "url": "https://packages.team.local",
    "auth": false
  }
]
```

### Use Nest Packages
```json
"require": {
  "internal_sdk": {
    "version": "^2.0.0",
    "nest": "company-registry"
  },
  "shared_lib": {
    "version": "^1.5.0",
    "nest": "team-registry"
  }
}
```

### Authentication
Create `hatch_auth.json` or `hatch_auth.local.json`:
```json
{
  "nests": {
    "company-registry": {
      "token": "your-api-token-here",
      "username": "optional-username",
      "password": "optional-password"
    }
  }
}
```

Environment variable alternative:
```bash
export HATCH_NEST_COMPANY_TOKEN="your-api-token"
```

### Scoped Packages
```json
"require": {
  "@company/analytics": {
    "version": "^3.0.0",
    "nest": "company-registry"
  }
}
```

## Flutter SDK Dependencies (Not Implemented)

SDK dependencies configuration is supported in the manifest but resolution is not implemented.

### Flutter Test
```json
"require-dev": {
  "flutter_test": {
    "sdk": "flutter"
  }
}
```

### Flutter Driver
```json
"require-dev": {
  "flutter_driver": {
    "sdk": "flutter"
  }
}
```

### Flutter Localizations
```json
"require": {
  "flutter_localizations": {
    "sdk": "flutter"
  }
}
```

## Version Constraints

Hatch supports various version constraint formats:

### Caret (^) - Compatible
```json
"http": "^1.0.0"  // >=1.0.0 <2.0.0
"http": "^0.1.0"  // >=0.1.0 <0.2.0
```

### Tilde (~) - Approximately
```json
"dio": "~5.1.0"   // >=5.1.0 <5.2.0
```

### Ranges
```json
"provider": ">=6.0.0 <7.0.0"
"http": ">1.0.0 <=2.0.0"
```

### Exact Version
```json
"flutter_bloc": "8.1.3"
```

### Any Version
```json
"local_package": "any"
"local_package": "*"
```

### Pre-release Versions
```json
"beta_package": "1.0.0-beta.1"
"alpha_package": "2.0.0-alpha"
```

## Dependency Validation

Hatch validates dependencies to prevent common configuration errors:

### Conflicting Sources
❌ **Invalid** - Multiple sources specified:
```json
"my_package": {
  "version": "^1.0.0",
  "git": "https://github.com/...",
  "path": "../local"  // ERROR: Can't have both git and path
}
```

✅ **Valid** - Single source:
```json
"my_package": {
  "version": "^1.0.0",
  "git": "https://github.com/..."
}
```

### Git Reference Without Git
❌ **Invalid** - ref without git:
```json
"my_package": {
  "version": "^1.0.0",
  "ref": "main"  // ERROR: ref requires git source
}
```

✅ **Valid** - ref with git:
```json
"my_package": {
  "version": "^1.0.0",
  "git": "https://github.com/...",
  "ref": "main"
}
```

### Undefined Nest
❌ **Invalid** - Using undefined nest:
```json
"require": {
  "private_pkg": {
    "version": "^1.0.0",
    "nest": "undefined-nest"  // ERROR: Nest not defined
  }
}
```

✅ **Valid** - Using defined nest:
```json
"nests": [
  {
    "name": "company",
    "url": "https://packages.company.com"
  }
],
"require": {
  "private_pkg": {
    "version": "^1.0.0",
    "nest": "company"  // OK: Nest is defined
  }
}
```

## Advanced Configuration

### Mixed Sources
```json
"require": {
  // From pub.dev
  "http": "^1.0.0",

  // From local path
  "shared": {
    "version": "any",
    "path": "../shared"
  },

  // From git
  "experimental": {
    "version": "any",
    "git": "https://github.com/lab/experimental.git",
    "ref": "feature-branch"
  },

  // From private registry
  "internal": {
    "version": "^2.0.0",
    "nest": "company"
  }
}
```

### Profile-Specific Dependencies
```json
"profiles": {
  "development": {
    "require": {
      "dev_tools": {
        "version": "any",
        "path": "../dev_tools"
      }
    }
  },
  "production": {
    "require": {
      "monitoring": {
        "version": "^1.0.0",
        "nest": "company"
      }
    }
  }
}
```

### Override Dependencies
In `hatch.local.json` (git-ignored):
```json
{
  "require": {
    "problematic_package": {
      "version": "any",
      "path": "../debug/problematic_package"
    }
  }
}
```

## Troubleshooting

### Package Not Found
```bash
Error: Package 'unknown_pkg' not found
```
**Solution**: Check package name and ensure source is configured correctly.

### Version Conflict
```bash
Error: Conflicting versions for 'http'
```
**Solution**: Use overrides or adjust version constraints.

### Authentication Failed
```bash
Error: Authentication failed for nest 'company-registry'
```
**Solution**: Check `hatch_auth.json` or environment variables.

### Git Clone Failed
```bash
Error: Failed to clone git repository
```
**Solutions**:
- Check repository URL
- Ensure git is installed
- Verify SSH keys for private repos
- Check network connectivity

### Path Not Found
```bash
Error: Path dependency not found: ../packages/shared
```
**Solutions**:
- Verify relative path from project root
- Check if package exists at specified location
- Ensure path uses correct separators for OS

### Circular Dependency
```bash
Warning: Circular dependency detected
```
**Solution**: Refactor packages to remove circular references.

## Best Practices

1. **Use Exact Versions in Production**
   ```json
   "require": {
     "critical_package": "1.2.3"  // Not ^1.2.3
   }
   ```

2. **Separate Dev Dependencies**
   ```json
   "require-dev": {
     "test": "^1.24.0"
   }
   ```

3. **Document Private Dependencies**
   ```json
   "require": {
     "internal_sdk": {
       "version": "^2.0.0",
       "nest": "company",
       "// comment": "Contact team@company.com for access"
     }
   }
   ```

4. **Use Profiles for Environments**
   ```json
   "profiles": {
     "ci": {
       "require": {
         "ci_tools": "^1.0.0"
       }
     }
   }
   ```

5. **Version Local Packages**
   Even for local packages, specify version constraints:
   ```json
   "shared": {
     "version": "^1.0.0",  // Not "any"
     "path": "../shared"
   }
   ```