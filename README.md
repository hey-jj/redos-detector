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

let result = is_safe("(a+)+$", "", &Config::default()).unwrap();
if !result.is_safe() {
    println!("{}", redos_detector::to_friendly(&result, &Default::default()));
}
```

`is_safe(source, flags, config)` reads flags from the string part after a regex,
where `u i s m` change matching and `g y d` are accepted and ignored.
`is_safe_pattern(pattern, config)` takes a raw pattern with explicit options.
`downgrade_pattern(pattern, unicode)` rewrites unsupported patterns into supported
ones. `to_friendly(result, config)` renders a result as text.

## Behavior

The analyzer targets a subset of ECMA-262 semantics. Offsets are UTF-16 code
unit indices. Backreferences to unmatched groups are treated as the empty
string, following JavaScript. Case folding uses the ECMA-262 simple-uppercase
rule in non-unicode mode.

The supported grammar is literals, character classes, escapes, groups including
lookbehind, quantifiers, anchors, numbered backreferences, and unicode property
escapes in unicode mode. Named groups, modifier groups, and the `v` flag are not
supported. The `i` and `u` flags cannot be combined.

The score is a severity, not a yes/no flag. `is_safe()` is true when no analysis
limit was hit, so the verdict is a threshold on the score, not `score > 1`. A
pattern with finite super-linear ambiguity such as `^a?a?a?a?$` scores `6` and
reads as safe when `max_score` is `None`. Set `max_score` to the level of
backtracking you will tolerate. The default cap catches this polynomial blowup;
raising or removing the cap lets it through.

Recoverable input problems return `Error`: an unsupported flag, a conflicting
config, or a pattern that fails to parse. A successful check returns a `Report`.
When the analysis hits a cap the report carries an `AnalysisLimit` in its `error`
field, a separate axis from input validation.

## License

Licensed under the [MIT license](LICENSE).
