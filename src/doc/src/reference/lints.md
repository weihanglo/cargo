# Lints

Note: [Cargo's linting system is unstable](unstable.md#lintscargo) and can only be used on nightly toolchains

## Warn-by-default

These lints are all set to the 'warn' level by default.
- [`invalid_license_expression`](#invalid_license_expression)
- [`unknown_lints`](#unknown_lints)

## `invalid_license_expression`
Set to `warn` by default

### What it does
Checks for invalid SPDX license expressions in the `package.license` field.

SPDX (Software Package Data Exchange) is a standard format for communicating license information. 
This lint validates that license expressions follow the SPDX specification, which uses specific 
operators and syntax rules.

### Why it is bad
- Invalid license expressions can cause confusion about the actual license terms
- Tools that parse SPDX expressions may fail to understand the license, leading to build failures or incorrect license detection
- Package registries and dependency analyzers rely on valid SPDX expressions for license compliance
- Inconsistent license expressions make it harder to understand licensing across the ecosystem
- Legal tools and compliance systems may not recognize non-standard license expressions

### Examples

#### Invalid expressions (will trigger this lint):
```toml
[package]
name = "foo"
version = "0.1.0"
license = "MIT / Apache-2.0"  # Invalid: uses "/" instead of "OR"
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "MIT and Apache-2.0"  # Invalid: uses lowercase "and" instead of "AND"
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "MIT, Apache-2.0"  # Invalid: uses comma instead of "OR"
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "GPL-3.0 with exception"  # Invalid: uses lowercase "with" instead of "WITH"
```

#### Valid expressions (will not trigger this lint):
```toml
[package]
name = "foo"
version = "0.1.0"
license = "MIT OR Apache-2.0"  # Valid: uses proper "OR" operator
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "MIT AND Apache-2.0"  # Valid: uses proper "AND" operator
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "GPL-3.0-or-later WITH Classpath-exception-2.0"  # Valid: uses proper "WITH" operator
```

```toml
[package]
name = "foo"
version = "0.1.0"
license = "(MIT OR Apache-2.0) AND BSD-3-Clause"  # Valid: complex expression with parentheses
```

### Migration Guide

#### Common fixes:

1. **Replace "/" with "OR":**
   - Before: `license = "MIT / Apache-2.0"`
   - After: `license = "MIT OR Apache-2.0"`

2. **Replace lowercase operators with uppercase:**
   - Before: `license = "MIT and Apache-2.0"`
   - After: `license = "MIT AND Apache-2.0"`
   
   - Before: `license = "MIT or Apache-2.0"`
   - After: `license = "MIT OR Apache-2.0"`
   
   - Before: `license = "GPL-3.0 with exception"`
   - After: `license = "GPL-3.0 WITH exception"`

3. **Replace commas and semicolons with "OR":**
   - Before: `license = "MIT, Apache-2.0"`
   - After: `license = "MIT OR Apache-2.0"`
   
   - Before: `license = "MIT; Apache-2.0"`
   - After: `license = "MIT OR Apache-2.0"`

4. **Fix parentheses issues:**
   - Before: `license = "MIT OR (Apache-2.0"`
   - After: `license = "MIT OR (Apache-2.0)"`

#### SPDX operators:
- `OR`: Use when the software can be used under either license
- `AND`: Use when both licenses apply simultaneously
- `WITH`: Use for license exceptions (e.g., GPL with linking exception)

#### Parentheses:
Use parentheses to group expressions and clarify precedence:
- `(MIT OR Apache-2.0) AND BSD-3-Clause`

### Configuration

This lint is set to `warn` by default and will become `deny` in future Cargo editions.

You can configure the lint level in your `Cargo.toml`:

```toml
[lints.cargo]
invalid_license_expression = "deny"  # Make it an error
# or
invalid_license_expression = "allow"  # Disable the lint
```

For workspace-level configuration:
```toml
[workspace.lints.cargo]
invalid_license_expression = "warn"
```

### Resources
- [SPDX License List](https://spdx.org/licenses/)
- [SPDX License Expression Syntax](https://spdx.github.io/spdx-spec/v2.3/SPDX-license-expressions/)
- [Cargo Book: The license field](https://doc.rust-lang.org/cargo/reference/manifest.html#the-license-field)


## `unknown_lints`
Set to `warn` by default

### What it does
Checks for unknown lints in the `[lints.cargo]` table

### Why it is bad
- The lint name could be misspelled, leading to confusion as to why it is
  not working as expected
- The unknown lint could end up causing an error if `cargo` decides to make
  a lint with the same name in the future

### Example
```toml
[lints.cargo]
this-lint-does-not-exist = "warn"
```


