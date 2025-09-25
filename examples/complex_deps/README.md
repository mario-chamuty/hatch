# Complex Dependencies Example

This example shows Hatch handling complex dependency scenarios including:
- Multiple dependency sources
- Development dependencies
- Build tools integration
- Profile-based dependencies
- Version conflict resolution

## Features Demonstrated

### 1. Mixed Dependencies
- Regular dependencies (flutter_bloc, dio)
- Dev dependencies (build_runner, freezed)
- SDK dependencies (flutter_test)

### 2. Profiles
Different dependency sets for different environments:
```yaml
profiles:
  production:
    require:
      sentry_flutter: ^7.10.0

  dev:
    require:
      flutter_dotenv: ^5.1.0
      logger: ^2.0.0
```

### 3. Scripts
Automated tasks for development:
```yaml
scripts:
  generate: "flutter pub run build_runner build"
  watch: "flutter pub run build_runner watch"
  test: "flutter test"
```

### 4. Conflict Resolution
Using aliases to resolve version conflicts:
```yaml
overrides:
  # Force specific versions to resolve conflicts
  analyzer: "5.13.0 as 6.0.0"
  collection: "1.17.2 as 1.18.0"
```

## Running

```bash
# Install default dependencies
hatch install

# Install with dev profile
hatch install --profile dev

# Install with production profile
hatch install --profile production

# Run scripts
hatch run generate
hatch run test
```

## Real-World Scenarios

This example mimics real Flutter projects that use:
- **Code generation** (freezed, json_serializable)
- **State management** (flutter_bloc)
- **HTTP clients** (dio)
- **Error tracking** (sentry_flutter)
- **Local storage** (shared_preferences)