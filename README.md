# redos-detector

Static ReDoS analysis for ECMA-262 regular expressions.

ReDoS (regular-expression denial of service) happens when a pattern can match the
same input many ways, forcing a backtracking engine to do exponential work. This
crate scores that risk without running the pattern. It enumerates the ways an
input prefix could match and counts how many distinct paths can match the same
prefix. A score of `1` means every input matches at most one way. An infinite
score means backtracking is unbounded.

## Install

```toml
[dependencies]
redos-detector = "0.1"
```

## Use

```rust
use redos_detector::{is_safe, Config};

let result = is_safe("(a+)+$", "", &Config::default());
if !result.safe {
    println!("{}", redos_detector::to_friendly(&result, &Default::default()));
}
```

`is_safe(source, flags, config)` reads flags from the string part after a regex,
where `u i s m` change matching and `g y d` are accepted and ignored.
`is_safe_pattern(pattern, config)` takes a raw pattern with explicit options.
`downgrade_pattern(pattern, unicode)` rewrites unsupported patterns into supported
ones. `to_friendly(result, config)` renders a result as text.

## Behavior

The analyzer targets ECMA-262 semantics. Offsets are UTF-16 code unit indices.
Backreferences to unmatched groups are treated as the empty string, following
JavaScript. Case folding uses the ECMA-262 simple-uppercase rule in non-unicode
mode.

Invalid input panics with the message an engine would surface: validation errors,
unsupported flags, oversized quantifier counts, and references that need
downgrading.

## License

Licensed under the [MIT license](LICENSE).
