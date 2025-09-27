# Edge Case Tests for Dependency Validation

This directory contains test cases for dependency validation edge cases.

## Test Cases

### 1. Conflicting Sources
- ✅ `path` + `nest`: Error - only one source allowed
- ✅ `git` + `path`: Error - only one source allowed
- ✅ `git` + `nest`: Error - only one source allowed
- ✅ All three sources: Error - only one source allowed

### 2. Invalid Git References
- ✅ `ref` without `git`: Error - ref requires git source

### 3. Undefined Nest References
- ✅ Nest reference without definition: Error - undefined nest

### 4. Valid Cases
- ✅ Simple string dependency: Works
- ✅ Path dependency alone: Works
- ✅ Git dependency with ref: Works
- ✅ Nest dependency with definition: Works
- ✅ Version constraint "any": Works (equivalent to "*")

## Running Tests

```bash
# Test conflicting sources
cd test_conflict_path_nest && ../../target/release/hatch.exe install
# Expected: Error about conflicting sources

# Test invalid ref
cd test_conflict_ref && ../../target/release/hatch.exe install
# Expected: Error about ref without git

# Test undefined nest
cd test_undefined_nest && ../../target/release/hatch.exe install
# Expected: Error about undefined nest

# Test valid git with ref
cd test_git_with_ref && ../../target/release/hatch.exe install
# Expected: Success
```