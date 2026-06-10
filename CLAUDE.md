# Claude Development Methodology

This document describes the development workflow, file organization, and usage instructions for the Hatch project.

## CORE CONSTRAINT: NO macOS — EVER

The entire reason hatch exists is to build, sign, and ship iOS apps (including
App Store / TestFlight) **without ever touching a Mac**. This is non-negotiable.

- NEVER propose, offer, or design any solution that uses macOS, a Mac, a cloud
  Mac, a macOS CI runner, Xcode, `actool`, Apple's `ld`, or any other Apple
  tool that only runs on macOS. Not even "just once" or "just for one artifact".
- When Apple's ingestion requires something Apple's tools normally produce (e.g.
  an actool `Assets.car`, an Apple-`ld`-linked binary), the answer is to
  replicate it natively on Linux/Windows (reverse-engineer the format, use
  open-source tooling, etc.) — NOT to fall back to a Mac.
- "It needs a Mac" is never an acceptable conclusion. Keep finding the Mac-free
  way.

## TECH DEBT: port `mkcar.py` (Assets.car writer) to native Rust

`src/ios/tools/mkcar.py` is a Python reverse-engineering scratchpad that is
currently embedded into the binary (`include_str!`) and shelled out through
`python3` (plus the `lzfse` CLI) inside `pipeline.sh`. This is a **temporary**
arrangement so we can iterate on the CAR/BOM/LZFSE format against Apple's
ingestion. It adds `python3` + `lzfse`-CLI runtime dependencies to the build
environment and violates the "implement in Rust" standard.

**Once the Assets.car is CONFIRMED accepted by App Store ingestion (ITMS-90596
clears), the implementation MUST be moved to native Rust** — a proper module
(e.g. `src/ios/assets_car.rs`) using the existing `image` crate for PNG decode
and an LZFSE crate (`lzfse` 0.2 FFI to Apple's reference C, or `lzfse_rust`)
for compression, generating the `.car` bytes in-process and dropping the
`python3`/`lzfse`-CLI steps from `pipeline.sh`. Do not consider the icon work
"done" until this port is complete.

## CAR format reference catalogs + spec (`temp/`)

To fix the `Assets.car` (ITMS-90596) work properly, `temp/` holds genuine
`actool`-compiled catalogs downloaded/extracted as ground-truth references, a
full byte-level format spec, and the dissection tool. These are reference
material (git-ignored scratch), NOT build inputs.

- **`temp/CAR_FORMAT.md`** – the authoritative byte-level `.car`/BOM/CoreUI
  format spec: BOM container + invariants, CARHEADER (436B), KEYFORMAT (token
  order, 7-token@CoreUI-691 vs 10-token@CoreUI-970), EXTENDED_METADATA, the
  FACETKEYS/RENDITIONS/APPEARANCEKEYS/BITMAPKEYS trees, the 184-byte CSI header,
  rendition layout types (empirically corrected over the blogs), the TLV info
  list, MLEC/KCBC/LZFSE payloads, and the app-icon (`0x3F2` SISM) assembly. Built
  from web reverse-engineering (Timac, dbg.re, Apple Wiki, iineva/bom) PLUS
  first-hand dissection. Read this before touching `mkcar.py`.
- **`temp/dissect_car.py`** – exhaustive dumper: `python3 temp/dissect_car.py <file.car>`.
- Reference catalogs (newest first) + their saved dumps `dissect_*.txt`:
  - `reference_feather_972.car` – **NEWEST: CoreUI-972 / Xcode 26**, extracted
    from a current 2026 shipping app (khcrysalis/Feather v2.8.2 IPA). App icon is
    the iOS-26 **Icon Composer / "liquid glass" layered** format: one 1024 `0xc`
    direct (Any/Dark/Tintable) + `0x3fb` IconStack + `0x3fc` IconGroup + `0x3f1`
    colors + `0x3fd` gradients. NO per-size PNGs.
  - `reference_utm_970.car` – CoreUI-970 / Xcode 26, the only App-Store-INGESTION-PROVEN
    one. Has BOTH `0x3eb` packed per-size icons AND the layered stack. Uses
    `Subtype=1792` for the iphone variant.
  - `iineva_Assets.car` – CoreUI-691 / Xcode 12.5, clean + simple (7 renditions),
    app icon as `0xc` **direct** + `0x3f2`. Proves packing is an optimization,
    not a requirement (NOT confirmed ingestion-tested though).
  - `acextract_iphone.car` – CoreUI-374 (~2016), heavy `0x3eb` packing.

  TWO app-icon paradigms: (a) traditional flat appiconset (per-size PNGs ->
  `0xc`/`0x3eb` renditions + `0x3f2`) which is what `flutter_launcher_icons`
  produces and what mkcar must emit; (b) Icon Composer layered (single 1024 +
  `0x3fb`/`0x3fc`/`0x3fd` stack) for the new Xcode `.icon` tool, which mkcar does
  NOT need.

Key conclusion: mkcar output is byte-identical to these references in every
comparable structure (BOM, CSI header, TLV, SISM, keys); the remaining gap is
the modern **`Subtype=1792` iphone keying** (a second `0x3f2` facet for
`Idiom=1,Subtype=1792` + matching renditions) and, in the proven UTM reference,
the `0x3eb` packed form. Those are the next mkcar changes to try for 90596.

## File Structure & Purpose

### Core Documentation Files

- **`CLAUDE.md`** (this file): Development methodology and project overview
- **`PLAN.md`**: Comprehensive implementation plan with checkboxes for tracking progress
- **`HATCH.md`**: Original specification and architecture documentation (renamed from previous CLAUDE.md)

### Development Files

- **`Cargo.toml`**: Rust project configuration and dependencies
- **`src/`**: Source code organized by functional modules
- **`.hatch.json`**: Local settings file (git-ignored, stores FVM version, user preferences)

## How to Use PLAN.md

The `PLAN.md` file is the central tracking document for implementation progress:

### Checkbox System
- `[ ]` = Task not started
- `[x]` = Task completed
- Mark tasks as complete by changing `[ ]` to `[x]`

### Phase-Based Organization
The plan is organized into 10 phases:
1. **Phase 1-3**: Core foundation and basic commands (MVP)
2. **Phase 4-6**: Backend integration and registry system
3. **Phase 7-10**: Advanced features, plugins, and release

### Using the Plan
1. **Start with Phase 1**: Complete all Phase 1 tasks before moving to Phase 2
2. **Update Checkboxes**: Mark completed tasks immediately
3. **Track Dependencies**: Some tasks depend on others - follow the logical order
4. **Reference During Development**: Use as a checklist when implementing features

## How to Use HATCH.md

The `HATCH.md` file contains the original specification and serves as the reference:

### Architecture Reference
- Review architecture decisions before implementing
- Use manifest examples as test cases
- Follow API specifications exactly

### Feature Specifications
- Each feature description includes requirements
- Use CLI command examples as acceptance criteria
- Reference Docker configurations for deployment

## Development Workflow

### 1. Setup Phase
```bash
# Initialize Rust project
cargo init --name hatch
cd hatch

# Setup git repository
git init
git add .
git commit -m "Initial Rust project setup"
```

### 2. Incremental Development
1. **Choose Next Task**: Select from current phase in PLAN.md
2. **Implement Feature**: Write code following Rust best practices
3. **Write Tests**: Add unit tests for new functionality
4. **Update Plan**: Mark task as complete in PLAN.md
5. **Commit Changes**: Git commit with descriptive message

### 3. Testing Strategy
- **Unit Tests**: Test individual functions and modules
- **Integration Tests**: Test CLI commands end-to-end
- **Manual Testing**: Test with real Flutter projects

### 4. Code Organization Principles

#### Module Structure
Follow the planned directory structure from PLAN.md:
```
src/
├── main.rs           # CLI entry point
├── cli/              # Command handling
├── config/           # Configuration management
├── manifest/         # Manifest parsing
├── resolver/         # Dependency resolution
├── registry/         # Registry implementations
├── fvm/              # FVM integration
├── generator/        # File generation
├── backend/          # Backend API
├── submodules/       # Submodule handling
└── utils/            # Shared utilities
```

#### Coding Standards
- **Error Handling**: Use `anyhow` for error propagation
- **Async Code**: Use `tokio` for async operations
- **Serialization**: Use `serde` for YAML/JSON handling
- **CLI Parsing**: Use `clap` for command-line interface
- **Logging**: Use `log` with `env_logger` for debugging

## Configuration Management

### .hatch.json Structure
```json
{
  "version": "1.0.0",
  "flutter": {
    "version": "3.24.2",
    "fvm_path": "/Users/username/.fvm"
  },
  "registries": {
    "pub": "https://pub.dev",
    "hatch": "https://hatch.dev"
  },
  "build": {
    "backend_url": "https://hatch.example.com/api",
    "api_key_env": "HATCH_API_KEY"
  },
  "cache": {
    "ttl": 3600,
    "max_size": "100MB"
  }
}
```

### Settings Priority
1. Command-line arguments (highest)
2. Project `.hatch.json`
3. Global `.hatch.json` (`~/.hatch.json`)
4. Environment variables
5. Built-in defaults (lowest)

## Implementation Priorities

### MVP Requirements (Must Have)
- Basic CLI commands (`init`, `install`, `add`, `remove`, `update`)
- Manifest parsing (`hatch.yaml`/`hatch.json`)
- FVM integration
- pub.dev registry support
- pubspec.yaml generation

### Phase 2 Features (Should Have)
- Profile management
- Dependency analysis (`why` command)
- Submodule support
- Smart conflict resolution

### Advanced Features (Nice to Have)
- Custom registry support
- Web dashboard integration
- Plugin system
- Build management

## Testing & Quality Assurance

### Required Tests
- **Unit Tests**: Every public function
- **Integration Tests**: CLI command execution
- **Property Tests**: Dependency resolution edge cases
- **Performance Tests**: Large dependency trees

### Quality Metrics
- Code coverage > 80%
- All clippy warnings resolved
- Documentation for public APIs
- Examples for CLI commands

## Release Process

### Version Management
- Follow semantic versioning (semver)
- Update version in `Cargo.toml`
- Tag releases in git
- Generate changelog from git commits

### Distribution Channels
- GitHub Releases (binaries)
- crates.io (Rust package)
- Homebrew (macOS)
- Chocolatey (Windows)
- Docker Hub (container)

## Troubleshooting Common Issues

### Development Environment
- Ensure Rust 1.70+ is installed
- Install required system dependencies (OpenSSL, etc.)
- Use `cargo check` for fast compilation checks
- Use `cargo clippy` for linting

### FVM Integration
- Test with multiple Flutter versions
- Handle FVM installation edge cases
- Support both global and project FVM configs

### Dependency Resolution
- Test with complex dependency trees
- Handle circular dependencies gracefully
- Provide clear error messages for conflicts

## Contributing Guidelines

### Code Style
- Use `cargo fmt` for code formatting
- Follow Rust naming conventions
- Add rustdoc comments for public APIs
- Keep functions small and focused

### Git Workflow
- Create feature branches for new work
- Write descriptive commit messages
- Squash commits before merging
- Update PLAN.md with progress

### Pull Request Process
1. Update PLAN.md checkboxes
2. Add/update tests
3. Update documentation
4. Ensure CI passes
5. Request code review

This methodology ensures consistent development practices and clear progress tracking throughout the Hatch implementation.
- always use fvm flutter version 3.35.2