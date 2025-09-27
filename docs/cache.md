# Cache Management

Hatch maintains a sophisticated caching system to optimize package downloads, metadata fetching, and dependency resolution. This guide covers cache structure, management, and optimization strategies.

## Table of Contents
- [Cache Structure](#cache-structure)
- [Cache Location](#cache-location)
- [Cache Commands](#cache-commands)
- [Cache Optimization](#cache-optimization)
- [CI/CD Caching](#cicd-caching)
- [Troubleshooting](#troubleshooting)

## Cache Structure

The Hatch cache is organized into three main directories:

```
~/.hatch/cache/
├── packages/       # Extracted package contents
│   └── pub.dev/
│       ├── http/
│       │   ├── 1.1.0/
│       │   │   ├── lib/
│       │   │   ├── pubspec.yaml
│       │   │   └── ...
│       │   └── 1.0.0/
│       └── provider/
│           └── 6.0.5/
├── downloads/      # Package tarballs (.tar.gz)
│   ├── http-1.1.0.tar.gz
│   ├── provider-6.0.5.tar.gz
│   └── ...
└── metadata/       # Package metadata JSON
    └── pub.dev/
        ├── http.json
        ├── provider.json
        └── ...
```

### Packages Directory
Contains extracted package contents ready for use:
- **Immutable**: Once extracted, packages are never modified
- **Versioned**: Each version is stored separately
- **Registry-scoped**: Organized by registry (pub.dev, nests, etc.)

### Downloads Directory
Stores compressed package archives:
- **Temporary**: Can be safely deleted to save space
- **Re-downloaded**: Fetched again if needed
- **Integrity checked**: Verified against checksums

### Metadata Directory
Caches package metadata from registries:
- **TTL-based**: Refreshed after expiry (default: 24 hours)
- **JSON format**: Quick parsing and lookup
- **Version lists**: All available versions per package

## Cache Location

### Default Location
By default, Hatch stores its cache in:
- **Unix/macOS**: `~/.hatch/cache/`
- **Windows**: `%USERPROFILE%\.hatch\cache\`

### Custom Location
Set a custom cache directory using environment variable:
```bash
export HATCH_CACHE_DIR=/path/to/custom/cache
hatch install
```

**Requirements**:
- Must be an absolute path
- Must have read/write permissions
- Should have adequate disk space

### CI/CD Cache Location
For CI/CD environments, use a persistent volume:
```yaml
# GitHub Actions example
env:
  HATCH_CACHE_DIR: ${{ github.workspace }}/.hatch-cache
```

```yaml
# GitLab CI example
variables:
  HATCH_CACHE_DIR: ${CI_PROJECT_DIR}/.hatch-cache
```

## Cache Commands

### View Cache Statistics
```bash
hatch cache stats
```
Shows cache location, total size, and package count.

### Clear Entire Cache
```bash
# With confirmation
hatch cache clear

# Force clear without confirmation
hatch cache clear --force
```

### Remove Specific Package
```bash
# Remove all versions of a package
hatch cache remove http

# Remove specific version
hatch cache remove http 1.1.0
```

### Planned Cache Commands
The following commands are planned for future releases:
- `hatch cache list` - List cached packages
- `hatch cache prune` - Remove unused packages
- `hatch cache verify` - Verify package integrity
- `hatch cache refresh` - Refresh metadata
- `hatch cache warm` - Pre-populate cache
- Clear specific cache types (downloads/metadata/packages)

## Cache Optimization

### Automatic Cleanup

Hatch automatically manages cache:
1. **Removes corrupted packages** during integrity checks
2. **Expires old metadata** based on TTL
3. **Deduplicates identical files** across versions

### Manual Optimization

Currently, manual optimization requires clearing the entire cache:
```bash
hatch cache clear --force
```

Future versions will support more granular cache management.

### Cache Settings

Configure cache behavior in `.hatch/config.json`:
```json
{
  "cache": {
    "ttl": 86400,           // Metadata TTL in seconds (24h)
    "max_size": "10GB",     // Maximum cache size
    "auto_clean": true,     // Auto-cleanup old packages
    "parallel_downloads": 5  // Concurrent downloads
  }
}
```

### Environment Variables

Currently supported:
```bash
# Custom cache directory
HATCH_CACHE_DIR=/custom/path
```

The following variables are planned for future releases:
- `HATCH_CACHE_TTL` - Metadata cache TTL
- `HATCH_MAX_PARALLEL` - Maximum parallel downloads
- `HATCH_NO_CACHE` - Disable cache
- `HATCH_TIMEOUT` - Connection timeout

## CI/CD Caching

### GitHub Actions

```yaml
name: Flutter CI

on: [push, pull_request]

jobs:
  build:
    runs-on: ubuntu-latest

    steps:
    - uses: actions/checkout@v3

    - name: Cache Hatch packages
      uses: actions/cache@v3
      with:
        path: ~/.hatch/cache
        key: ${{ runner.os }}-hatch-${{ hashFiles('**/hatch.lock') }}
        restore-keys: |
          ${{ runner.os }}-hatch-

    - name: Install Hatch
      run: |
        curl -fsSL https://get.hatch.dev | bash

    - name: Install dependencies
      run: hatch install
```

### GitLab CI

```yaml
variables:
  HATCH_CACHE_DIR: ${CI_PROJECT_DIR}/.hatch-cache

cache:
  key: ${CI_COMMIT_REF_SLUG}
  paths:
    - .hatch-cache/

before_script:
  - curl -fsSL https://get.hatch.dev | bash

build:
  script:
    - hatch install
    - flutter build apk
```

### Docker

```dockerfile
FROM rust:1.70 AS hatch-builder
WORKDIR /build
COPY . .
RUN cargo build --release

FROM flutter:3.24.2

# Install Hatch
COPY --from=hatch-builder /build/target/release/hatch /usr/local/bin/

# Setup cache volume
VOLUME /root/.hatch/cache

# Copy project
WORKDIR /app
COPY . .

# Install dependencies with cache
RUN hatch install

# Build app
RUN flutter build apk
```

Docker Compose with cache volume:
```yaml
version: '3.8'

services:
  app:
    build: .
    volumes:
      - hatch-cache:/root/.hatch/cache

volumes:
  hatch-cache:
```

## Cache Integrity

### Checksum Verification

Hatch verifies package integrity using SHA256 checksums:
```yaml
# In hatch.lock
packages:
  http:
    version: 1.1.0
    integrity: sha256:a1b2c3d4e5f6...
```

### Corruption Detection

Hatch automatically verifies package integrity during installation. If a corrupted package is detected, it will be re-downloaded automatically.

## Performance Tuning

### Optimize for Speed

Currently, speed optimization is built-in with parallel downloads and smart caching. Additional tuning options are planned for future releases.

### Optimize for Size

```bash
# Clear entire cache to reclaim space
hatch cache clear --force

# Remove specific packages no longer needed
hatch cache remove <package>
```

### Optimize for Reliability

Hatch currently uses default settings optimized for reliability. Additional configuration options are planned for future releases.

## Troubleshooting

### Cache Corruption

**Symptoms**: Random build failures, missing files
```bash
# Clear and rebuild cache
hatch cache clear --force
hatch install --force
```

### Disk Space Issues

**Error**: "No space left on device"
```bash
# Check cache size
hatch cache stats

# Clear cache to free space
hatch cache clear --force
```

### Permission Errors

**Error**: "Permission denied"
```bash
# Fix permissions (Unix/macOS)
chmod -R u+rw ~/.hatch/cache

# Use different cache location
export HATCH_CACHE_DIR=/tmp/hatch-cache
```

### Network Cache Issues

**Error**: "Failed to download package"
```bash
# Clear entire cache and retry
hatch cache clear --force
hatch install

# Use different registry mirror
export PUB_SERVER=https://pub.flutter-io.cn
```

### Stale Metadata

**Symptoms**: Not finding latest package versions
```bash
# Clear cache to force fresh metadata
hatch cache clear --force

# Reduce TTL for faster updates
export HATCH_CACHE_TTL=300  # 5 minutes
```

## Cache Strategies

### Development Environment
- **Large TTL**: Reduce network requests
- **Keep downloads**: Quick switching between versions
- **Auto-cleanup**: Maintain reasonable size

### CI/CD Environment
- **Persistent cache**: Share across builds
- **Minimal downloads**: Remove after extraction
- **Strict verification**: Ensure integrity

### Production Builds
- **Fresh metadata**: Get latest security updates
- **Verified packages**: Full integrity checks
- **Clean cache**: Start from known state

### Offline Development
- **Pre-populate cache**: Download all dependencies
- **No expiry**: Disable metadata TTL
- **Local registry**: Mirror packages locally

## Advanced Cache Management

The following advanced cache features are planned for future releases:

- **Cache Warming**: Pre-populate cache for offline use
- **Cache Export/Import**: Share cache between machines
- **Cache Server**: Run local cache server for teams
- **Selective cache management**: Clear specific cache types
- **Cache verification**: Verify package integrity

## Best Practices

1. **Regular Cleanup**: Periodically clear cache if it grows too large
2. **Monitor Size**: Set up alerts for cache size limits
3. **Backup Strategy**: Include cache in CI/CD artifacts
4. **Security**: Verify integrity in production builds
5. **Documentation**: Document custom cache locations in README

## FAQ

**Q: Can I share cache between projects?**
A: Yes, the cache is global by default and shared across all projects.

**Q: Is the cache safe to delete?**
A: Yes, packages will be re-downloaded as needed.

**Q: How much space does the cache use?**
A: Typically 500MB-2GB for medium projects, check with `hatch cache stats`.

**Q: Can I use a network cache?**
A: Yes, mount a network drive and set `HATCH_CACHE_DIR`.

**Q: Does cache work offline?**
A: Yes, if packages are already cached, no network is needed.